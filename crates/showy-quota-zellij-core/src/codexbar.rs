use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderRecord {
    pub provider: String,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub status: Option<ProviderStatus>,
}

/// Optional CodexBar provider status block (`status.indicator` +
/// `status.url`). SketchyBar tints provider icons and points click-through
/// at the status page when an incident is active.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderStatus {
    #[serde(default)]
    pub indicator: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub primary: Option<UsageWindow>,
    #[serde(default)]
    pub secondary: Option<UsageWindow>,
    #[serde(default)]
    pub tertiary: Option<UsageWindow>,
    #[serde(rename = "extraRateWindows", default)]
    pub extra_rate_windows: Vec<NamedWindow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UsageWindow {
    #[serde(rename = "usedPercent", default)]
    pub used_percent: Option<f64>,
    #[serde(rename = "resetsAt", default)]
    pub resets_at: Option<String>,
    #[serde(rename = "resetDescription", default)]
    pub reset_description: Option<String>,
    #[serde(rename = "windowMinutes", default)]
    pub window_minutes: Option<i64>,
}

/// An additional named rate window CodexBar carries losslessly in
/// `usage.extraRateWindows` (per-model / per-horizon pools). `usage_known ==
/// Some(false)` marks a window whose `usedPercent` is a placeholder, not a real
/// measurement, and must not be rendered as live quota.
#[derive(Debug, Clone, Deserialize)]
pub struct NamedWindow {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub window: Option<UsageWindow>,
    #[serde(rename = "usageKnown", default)]
    pub usage_known: Option<bool>,
}

impl UsageWindow {
    pub fn used_pct_floor(&self) -> i32 {
        pct(self.used_percent)
    }

    pub fn reset_value(&self) -> Option<&str> {
        self.resets_at
            .as_deref()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                self.reset_description
                    .as_deref()
                    .filter(|value| !value.is_empty())
            })
    }

    pub fn window_minutes(&self) -> Option<i64> {
        self.window_minutes.filter(|value| *value >= 0)
    }
}

impl Usage {
    pub fn has_renderable_window(&self) -> bool {
        [
            self.primary.as_ref(),
            self.secondary.as_ref(),
            self.tertiary.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|window| window.used_percent.is_some())
    }

    /// Semantic positional slots for the renderers. A window occupies a slot
    /// only when it reports a numeric `usedPercent`. When the primary slot is
    /// absent, the present windows left-compact into the leading slots so the
    /// live window drives the primary row and countdown instead of an empty
    /// top row and an `idle` label — e.g. Codex after OpenAI temporarily
    /// removed the 5h limit, where `usage.primary` is null and the weekly cap
    /// is the live window. A present primary leaves the slots in positional
    /// order (no shifting). This is a render-layer view only; the raw
    /// positional windows stay untouched (e.g. `showy-quota-state`).
    pub fn render_slots(&self) -> [Option<&UsageWindow>; 3] {
        let raw = [
            slot(self.primary.as_ref()),
            slot(self.secondary.as_ref()),
            slot(self.tertiary.as_ref()),
        ];
        if raw[0].is_some() {
            return raw;
        }
        let mut compacted: [Option<&UsageWindow>; 3] = [None, None, None];
        for (index, window) in raw.into_iter().flatten().enumerate() {
            compacted[index] = Some(window);
        }
        compacted
    }
}

/// A window occupies its positional slot only when it reports a numeric
/// `usedPercent`; a present-but-empty object never counts as live quota.
fn slot(window: Option<&UsageWindow>) -> Option<&UsageWindow> {
    window.filter(|window| window.used_percent.is_some())
}

/// Upper bound on a CodexBar usage payload accepted by the parser. Mirrors the
/// `--max-filesize` ceiling the shell fetcher applies to the HTTP `/usage`
/// probe so the CLI-fallback and cache parse paths are bounded too: a buggy or
/// compromised `codexbar` emitting multi-gigabyte JSON cannot exhaust memory on
/// every render tick.
pub const MAX_USAGE_JSON_BYTES: usize = 5 * 1024 * 1024;

/// Parse the CodexBar usage array transport. Only transport-level failures
/// (unparseable JSON, non-array top level, oversize payload) are fatal; a
/// record failing [`valid_provider_record`] is dropped rather than
/// discarding the whole payload, so one malformed provider cannot blank the
/// strip for every other provider. An all-invalid array yields `Ok(vec![])`.
///
/// Because invalid records are dropped, the result is a strict SUBSEQUENCE of
/// the input array. A caller that also needs the raw JSON element a record came
/// from MUST use [`parse_usage_payload_indexed`] and index by the returned
/// position — zipping this list against the raw array pairs record `i` with
/// element `i`, which silently binds one provider's validated id to another
/// record's data as soon as anything is dropped.
pub fn parse_usage_payload(payload: &[u8]) -> Result<Vec<ProviderRecord>, serde_json::Error> {
    Ok(parse_usage_payload_indexed(payload)?
        .into_iter()
        .map(|(_, record)| record)
        .collect())
}

/// [`parse_usage_payload`], but each surviving record is paired with its index
/// in the original JSON array so a caller can recover the exact raw element it
/// was parsed from.
pub fn parse_usage_payload_indexed(
    payload: &[u8],
) -> Result<Vec<(usize, ProviderRecord)>, serde_json::Error> {
    if payload.len() > MAX_USAGE_JSON_BYTES {
        return Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "CodexBar usage payload exceeds size cap",
        )));
    }
    let records: Vec<ProviderRecord> = serde_json::from_slice(payload)?;
    Ok(records
        .into_iter()
        .enumerate()
        .filter(|(_, record)| valid_provider_record(record))
        .collect())
}

/// A cache file or `--json` input is either a bare CodexBar provider array
/// (legacy transport; degrades to `source = "unknown"`) or a cache envelope
/// object `{"schema":"showy-quota/cache@1","source":"serve"|"cli"|"unknown","providers":[...]}`.
/// Dispatches on the first non-whitespace byte and, for an envelope,
/// extracts `providers` via [`serde_json::value::RawValue`] so the inner
/// array (up to [`MAX_USAGE_JSON_BYTES`]) is sliced out verbatim rather than
/// parsed into [`ProviderRecord`]s and re-serialized — this runs on every
/// render tick.
///
/// Any shape this does not recognize as a well-formed envelope — oversize
/// input, unparseable JSON, a missing `providers` field — degrades to the
/// original bytes with `source = "unknown"` rather than erroring: a
/// genuinely corrupt payload is already handled by the caller's existing
/// validity/quarantine path, and this helper must never become a new hard
/// failure point. An envelope with an absent, null, or non-string `source`
/// likewise degrades only the source to `"unknown"`, keeping the extracted
/// `providers` array.
pub fn unwrap_cache_transport(bytes: Vec<u8>) -> (Vec<u8>, String) {
    if bytes.len() > MAX_USAGE_JSON_BYTES {
        return (bytes, String::from("unknown"));
    }
    match bytes.iter().find(|byte| !byte.is_ascii_whitespace()) {
        Some(b'{') => match serde_json::from_slice::<CacheEnvelope>(&bytes) {
            Ok(envelope) => {
                let inner = envelope.providers.get().as_bytes().to_vec();
                // Constrain to the tokens the publisher writes, mirroring the
                // shell's `showy_quota_cache_source`. `usage.json` is now an
                // attacker-shaped data document rather than a private one-token
                // metadata file, and this value is reachable through the public
                // `CacheFreshness.source`, so it must not carry arbitrary text.
                let source = match envelope.source.as_ref().and_then(|v| v.as_str()) {
                    Some(value @ ("serve" | "cli")) => String::from(value),
                    _ => String::from("unknown"),
                };
                (inner, source)
            }
            Err(_) => (bytes, String::from("unknown")),
        },
        _ => (bytes, String::from("unknown")),
    }
}

#[derive(Debug, Deserialize)]
struct CacheEnvelope {
    providers: Box<serde_json::value::RawValue>,
    #[serde(default)]
    source: Option<serde_json::Value>,
}

pub fn payload_has_renderable_provider(records: &[ProviderRecord]) -> bool {
    records.iter().any(is_renderable)
}

pub fn is_errored(record: &ProviderRecord) -> bool {
    record.error.is_some() && valid_provider_id(&record.provider) && !is_renderable(record)
}

pub(crate) fn is_renderable(record: &ProviderRecord) -> bool {
    record.error.is_none()
        && valid_provider_id(&record.provider)
        && record
            .usage
            .as_ref()
            .is_some_and(Usage::has_renderable_window)
}

/// Shared provider-id predicate, byte-for-byte with the shell's
/// `valid_provider_id`: the character-class check plus rejection of leading
/// dashes (argv flag injection) and the path components `.` and `..`, which
/// would escape per-provider stamp paths in the shell data plane.
pub fn valid_provider_id(provider: &str) -> bool {
    !provider.is_empty()
        && provider.len() <= 64
        && !provider.starts_with('-')
        && provider != "."
        && provider != ".."
        && provider
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn valid_provider_record(record: &ProviderRecord) -> bool {
    valid_provider_id(&record.provider)
        && record.usage.as_ref().is_none_or(|usage| {
            valid_window(usage.primary.as_ref())
                && valid_window(usage.secondary.as_ref())
                && valid_window(usage.tertiary.as_ref())
        })
}

fn valid_window(window: Option<&UsageWindow>) -> bool {
    window.is_none_or(|window| window.used_percent.is_some())
}

fn pct(value: Option<f64>) -> i32 {
    match value {
        Some(value) if value.is_finite() => value.floor().clamp(0.0, 100.0) as i32,
        _ => -1,
    }
}

/// Provider inventory record returned by `codexbar config providers --format json`.
/// Only `provider` and `enabled` matter for fallback discovery; other fields
/// (e.g. `defaultEnabled`, `displayName`) are intentionally ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfigRecord {
    pub provider: String,
    #[serde(default)]
    pub enabled: bool,
}

/// Parse the output of `codexbar config providers --format json [--pretty]`.
/// Returns the enabled, validated provider ids in the order CodexBar
/// reported them (duplicates collapsed), or an error when the payload is not
/// a JSON array.
///
/// Mirror of the shell fetcher's strict discovery contract: ANY enabled
/// record whose id fails [`valid_provider_id`] marks the whole inventory as
/// untrustworthy and is a discovery failure — silently dropping a corrupted
/// record would hide a real provider without an error, and an all-invalid
/// inventory must never publish an empty cache over real data. An empty
/// array (or all-disabled response) is a canonical empty inventory and
/// returns `Ok(vec![])` so callers can decide to publish `[]` rather than
/// block.
pub fn parse_provider_config_payload(payload: &[u8]) -> Result<Vec<String>, ProviderConfigError> {
    let raw: Vec<ProviderConfigRecord> =
        serde_json::from_slice(payload).map_err(ProviderConfigError::Parse)?;
    let mut enabled_valid: Vec<String> = Vec::new();
    for record in raw {
        if !record.enabled {
            continue;
        }
        if !valid_provider_id(&record.provider) {
            return Err(ProviderConfigError::InvalidInventory);
        }
        if !enabled_valid.iter().any(|id| id == &record.provider) {
            enabled_valid.push(record.provider);
        }
    }
    Ok(enabled_valid)
}

#[derive(Debug)]
pub enum ProviderConfigError {
    /// The discovery payload was not a JSON array of provider config records.
    Parse(serde_json::Error),
    /// The discovery payload had an enabled record whose id failed
    /// [`valid_provider_id`]; the inventory is untrustworthy, treat as a
    /// discovery failure rather than dropping records or publishing empty.
    InvalidInventory,
}

impl std::fmt::Display for ProviderConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "invalid codexbar config providers payload: {err}"),
            Self::InvalidInventory => {
                f.write_str("codexbar config providers returned invalid provider ids")
            }
        }
    }
}

impl std::error::Error for ProviderConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(err) => Some(err),
            Self::InvalidInventory => None,
        }
    }
}

/// Extract the regex-validated, first-seen provider ids from an aggregate
/// `codexbar usage` payload. Useful when CodexBar's `config providers`
/// command is unavailable but a prior cached usage payload exists.
pub fn provider_ids_from_records(records: &[ProviderRecord]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for record in records {
        if !valid_provider_id(&record.provider) {
            continue;
        }
        if !seen.iter().any(|id| id == &record.provider) {
            seen.push(record.provider.clone());
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_config_keeps_enabled_in_codexbar_order() {
        let payload = br#"[
            {"provider": "codex",      "enabled": true,  "defaultEnabled": true},
            {"provider": "claude",     "enabled": true,  "defaultEnabled": false},
            {"provider": "cursor",     "enabled": false, "defaultEnabled": false},
            {"provider": "opencodego", "enabled": true,  "defaultEnabled": false}
        ]"#;
        let ids = parse_provider_config_payload(payload).expect("valid payload");
        assert_eq!(ids, vec!["codex", "claude", "opencodego"]);
    }

    #[test]
    fn parse_provider_config_dedupes_repeated_ids() {
        let payload = br#"[
            {"provider": "codex", "enabled": true},
            {"provider": "codex", "enabled": true}
        ]"#;
        let ids = parse_provider_config_payload(payload).expect("valid payload");
        assert_eq!(ids, vec!["codex"]);
    }

    #[test]
    fn parse_provider_config_accepts_empty_inventory() {
        let ids = parse_provider_config_payload(b"[]").expect("valid payload");
        assert!(ids.is_empty(), "canonical empty inventory must succeed");
    }

    #[test]
    fn parse_provider_config_all_disabled_is_empty_inventory() {
        let payload = br#"[
            {"provider": "codex",  "enabled": false},
            {"provider": "claude", "enabled": false}
        ]"#;
        let ids = parse_provider_config_payload(payload).expect("valid payload");
        assert!(ids.is_empty());
    }

    #[test]
    fn parse_provider_config_rejects_all_invalid_enabled_ids() {
        let payload = br#"[{"provider": "bad/id", "enabled": true}]"#;
        let err = parse_provider_config_payload(payload).expect_err("must fail");
        assert!(matches!(err, ProviderConfigError::InvalidInventory));
    }

    #[test]
    fn parse_provider_config_rejects_mixed_valid_invalid_inventory() {
        // Strict contract, mirroring the shell fetcher: one corrupted record
        // poisons the batch — dropping it would silently hide a provider.
        let payload = br#"[
            {"provider": "ok.id",  "enabled": true},
            {"provider": "bad/id", "enabled": true}
        ]"#;
        let err = parse_provider_config_payload(payload).expect_err("must fail");
        assert!(matches!(err, ProviderConfigError::InvalidInventory));
    }

    #[test]
    fn parse_provider_config_rejects_dot_provider_ids() {
        let payload = br#"[{"provider": ".", "enabled": true}]"#;
        let err = parse_provider_config_payload(payload).expect_err("must fail");
        assert!(matches!(err, ProviderConfigError::InvalidInventory));
    }

    #[test]
    fn parse_provider_config_rejects_non_array() {
        assert!(matches!(
            parse_provider_config_payload(br#"{"providers": []}"#),
            Err(ProviderConfigError::Parse(_))
        ));
    }

    #[test]
    fn provider_ids_from_records_skips_invalid_and_dedupes() {
        let records = parse_usage_payload(
            br#"[
                {"provider": "claude"},
                {"provider": "codex"},
                {"provider": "claude"}
            ]"#,
        )
        .expect("valid payload");
        let ids = provider_ids_from_records(&records);
        assert_eq!(ids, vec!["claude", "codex"]);
    }

    #[test]
    fn secondary_only_usage_is_renderable_with_semantic_slots() {
        let records = parse_usage_payload(
            br#"[
                {
                    "provider": "antigravity",
                    "usage": {
                        "primary": null,
                        "secondary": {"usedPercent": 0},
                        "tertiary": {"usedPercent": 25}
                    }
                }
            ]"#,
        )
        .expect("valid payload");

        assert!(is_renderable(&records[0]));
        // Slots are semantic: a missing primary stays missing instead of
        // being backfilled by later windows.
        let usage = records[0].usage.as_ref().unwrap();
        assert!(usage.primary.is_none());
        assert_eq!(usage.secondary.as_ref().unwrap().used_percent, Some(0.0));
        assert_eq!(usage.tertiary.as_ref().unwrap().used_percent, Some(25.0));
    }

    #[test]
    fn errored_records_require_error_and_valid_non_renderable_provider() {
        let errored = ProviderRecord {
            provider: "codex".into(),
            error: Some(serde_json::json!({"message": "failed"})),
            usage: None,
            status: None,
        };
        assert!(is_errored(&errored));

        let invalid = ProviderRecord {
            provider: "bad/id".into(),
            error: Some(serde_json::json!({"message": "failed"})),
            usage: None,
            status: None,
        };
        assert!(!is_errored(&invalid));

        let renderable = ProviderRecord {
            provider: "codex".into(),
            error: None,
            usage: Some(Usage {
                primary: Some(UsageWindow {
                    used_percent: Some(10.0),
                    resets_at: None,
                    reset_description: None,
                    window_minutes: None,
                }),
                secondary: None,
                tertiary: None,
                extra_rate_windows: Vec::new(),
            }),
            status: None,
        };
        assert!(!is_errored(&renderable));
    }

    #[test]
    fn valid_provider_id_accepts_alnum_and_separators() {
        assert!(valid_provider_id("codex"));
        assert!(valid_provider_id("Codex123"));
        assert!(valid_provider_id("open_code.go-1"));
    }

    #[test]
    fn valid_provider_id_rejects_empty_and_invalid_chars() {
        assert!(!valid_provider_id(""));
        assert!(!valid_provider_id("bad/id"));
        assert!(!valid_provider_id("has space"));
        assert!(!valid_provider_id("bang!"));
        // Non-ASCII bytes are rejected even when alphabetic.
        assert!(!valid_provider_id("café"));
    }

    #[test]
    fn valid_provider_id_rejects_leading_dash() {
        assert!(!valid_provider_id("-codex"));
    }

    #[test]
    fn valid_provider_id_rejects_over_64_chars() {
        assert!(valid_provider_id(&"a".repeat(64)));
        assert!(!valid_provider_id(&"a".repeat(65)));
    }

    #[test]
    fn parse_usage_payload_rejects_oversize_input() {
        let mut payload = Vec::with_capacity(MAX_USAGE_JSON_BYTES + 16);
        payload.push(b'[');
        payload.resize(MAX_USAGE_JSON_BYTES + 1, b' ');
        let err = parse_usage_payload(&payload).expect_err("oversize payload must be rejected");
        assert!(
            err.is_io(),
            "size-cap rejection should be an IO-category error"
        );
    }

    #[test]
    fn valid_window_treats_absent_window_as_valid() {
        assert!(valid_window(None));
    }

    #[test]
    fn valid_window_requires_used_percent_when_present() {
        let with_pct: UsageWindow =
            serde_json::from_str(r#"{"usedPercent": 42}"#).expect("window json");
        assert!(valid_window(Some(&with_pct)));

        let without_pct: UsageWindow =
            serde_json::from_str(r#"{"resetsAt": "2099-01-01T00:00:00Z"}"#).expect("window json");
        assert!(!valid_window(Some(&without_pct)));
    }

    #[test]
    fn parse_usage_payload_rejects_non_array_json() {
        assert!(parse_usage_payload(br#"{"provider": "codex"}"#).is_err());
        assert!(parse_usage_payload(b"not json").is_err());
    }

    #[test]
    fn parse_usage_payload_drops_empty_provider_id() {
        let records = parse_usage_payload(br#"[{"provider": ""}]"#).expect("parses");
        assert!(records.is_empty());
    }

    #[test]
    fn parse_usage_payload_drops_invalid_provider_id() {
        let records = parse_usage_payload(br#"[{"provider": "bad/id"}]"#).expect("parses");
        assert!(records.is_empty());
    }

    #[test]
    fn parse_usage_payload_drops_window_without_used_percent() {
        // A present window object missing usedPercent fails per-record validation;
        // the record is dropped rather than rejecting the whole payload.
        let payload = br#"[
            {"provider": "codex", "usage": {"secondary": {"resetsAt": "2099-01-01T00:00:00Z"}}}
        ]"#;
        let records = parse_usage_payload(payload).expect("parses");
        assert!(records.is_empty());
    }

    #[test]
    fn parse_usage_payload_keeps_valid_record_and_drops_invalid_sibling() {
        let payload = br#"[
            {"provider": "codex", "usage": {"primary": {"usedPercent": 5}}},
            {"provider": "bad/id", "usage": {"primary": {"usedPercent": 5}}}
        ]"#;
        let records = parse_usage_payload(payload).expect("parses");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].provider, "codex");
    }

    #[test]
    fn parse_usage_payload_accepts_error_only_and_empty_records() {
        // Error-only records and an empty array are valid by design.
        assert!(parse_usage_payload(br#"[{"provider": "codex", "error": "boom"}]"#).is_ok());
        assert!(parse_usage_payload(b"[]").is_ok());
    }

    #[test]
    fn unwrap_cache_transport_extracts_envelope_providers_and_source() {
        let bytes =
            br#"{"schema":"showy-quota/cache@1","source":"cli","providers":[{"provider":"codex"}]}"#
                .to_vec();
        let (payload, source) = unwrap_cache_transport(bytes);
        assert_eq!(payload, br#"[{"provider":"codex"}]"#.to_vec());
        assert_eq!(source, "cli");
    }

    #[test]
    fn unwrap_cache_transport_leaves_bare_array_unchanged() {
        let bytes = br#"[{"provider":"codex"}]"#.to_vec();
        let (payload, source) = unwrap_cache_transport(bytes.clone());
        assert_eq!(payload, bytes);
        assert_eq!(source, "unknown");
    }

    #[test]
    fn unwrap_cache_transport_degrades_missing_source_to_unknown() {
        let bytes = br#"{"schema":"showy-quota/cache@1","providers":[1,2]}"#.to_vec();
        let (payload, source) = unwrap_cache_transport(bytes);
        assert_eq!(payload, b"[1,2]".to_vec());
        assert_eq!(source, "unknown");
    }

    #[test]
    fn unwrap_cache_transport_degrades_null_source_to_unknown() {
        let bytes = br#"{"source":null,"providers":[1,2]}"#.to_vec();
        let (payload, source) = unwrap_cache_transport(bytes);
        assert_eq!(payload, b"[1,2]".to_vec());
        assert_eq!(source, "unknown");
    }

    #[test]
    fn unwrap_cache_transport_degrades_non_string_source_to_unknown() {
        let bytes = br#"{"source":42,"providers":[1,2]}"#.to_vec();
        let (payload, source) = unwrap_cache_transport(bytes);
        assert_eq!(payload, b"[1,2]".to_vec());
        assert_eq!(source, "unknown");
    }

    #[test]
    fn unwrap_cache_transport_malformed_object_degrades_without_panicking() {
        // No `providers` field at all: not a well-formed envelope.
        let bytes = br#"{"schema":"showy-quota/cache@1","source":"cli"}"#.to_vec();
        let (payload, source) = unwrap_cache_transport(bytes.clone());
        assert_eq!(
            payload, bytes,
            "malformed envelope must degrade to original bytes"
        );
        assert_eq!(source, "unknown");

        // Truncated/invalid JSON object must not panic either.
        let truncated = br#"{"providers":[1,2"#.to_vec();
        let (payload, source) = unwrap_cache_transport(truncated.clone());
        assert_eq!(payload, truncated);
        assert_eq!(source, "unknown");
    }

    #[test]
    fn unwrap_cache_transport_tolerates_leading_whitespace() {
        let envelope_bytes = b"  \n\t{\"source\":\"serve\",\"providers\":[9]}".to_vec();
        let (payload, source) = unwrap_cache_transport(envelope_bytes);
        assert_eq!(payload, b"[9]".to_vec());
        assert_eq!(source, "serve");

        let array_bytes = b"   [9]".to_vec();
        let (payload, source) = unwrap_cache_transport(array_bytes.clone());
        assert_eq!(payload, array_bytes);
        assert_eq!(source, "unknown");
    }

    #[test]
    fn unwrap_cache_transport_respects_oversize_ceiling() {
        let oversize = vec![b' '; MAX_USAGE_JSON_BYTES + 1];
        let (payload, source) = unwrap_cache_transport(oversize.clone());
        assert_eq!(payload, oversize);
        assert_eq!(source, "unknown");
    }
}
