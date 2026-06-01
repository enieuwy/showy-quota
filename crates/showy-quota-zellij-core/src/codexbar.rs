use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderRecord {
    pub provider: String,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub primary: Option<UsageWindow>,
    #[serde(default)]
    pub secondary: Option<UsageWindow>,
    #[serde(default)]
    pub tertiary: Option<UsageWindow>,
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

pub fn parse_usage_payload(payload: &[u8]) -> Result<Vec<ProviderRecord>, serde_json::Error> {
    let records: Vec<ProviderRecord> = serde_json::from_slice(payload)?;
    if records.iter().all(valid_provider_record) {
        Ok(records)
    } else {
        Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid CodexBar usage payload",
        )))
    }
}

pub fn payload_has_renderable_provider(records: &[ProviderRecord]) -> bool {
    records.iter().any(is_renderable)
}

pub fn is_renderable(record: &ProviderRecord) -> bool {
    record.error.is_none()
        && valid_provider_id(&record.provider)
        && record
            .usage
            .as_ref()
            .and_then(|usage| usage.primary.as_ref())
            .and_then(|window| window.used_percent)
            .is_some()
}

pub fn valid_provider_id(provider: &str) -> bool {
    !provider.is_empty()
        && provider
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn valid_provider_record(record: &ProviderRecord) -> bool {
    valid_provider_id(&record.provider)
        && record.usage.as_ref().map_or(true, |usage| {
            valid_window(usage.primary.as_ref())
                && valid_window(usage.secondary.as_ref())
                && valid_window(usage.tertiary.as_ref())
        })
}

fn valid_window(window: Option<&UsageWindow>) -> bool {
    window.map_or(true, |window| window.used_percent.is_some())
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
/// Returns the enabled, regex-validated provider ids in the order CodexBar
/// reported them, or an error when the payload is not a JSON array.
///
/// Mirror of the shell fetcher's two-pass discovery: if the inventory has
/// records but none pass `valid_provider_id`, the whole response is treated
/// as a discovery failure to avoid silently publishing an empty cache when
/// CodexBar is misconfigured. An empty array (or all-disabled response) is a
/// canonical empty inventory and returns `Ok(vec![])` so callers can decide
/// to publish `[]` rather than block.
pub fn parse_provider_config_payload(payload: &[u8]) -> Result<Vec<String>, ProviderConfigError> {
    let raw: Vec<ProviderConfigRecord> =
        serde_json::from_slice(payload).map_err(ProviderConfigError::Parse)?;
    let mut any_enabled = false;
    let mut enabled_valid: Vec<String> = Vec::new();
    for record in raw {
        if !record.enabled {
            continue;
        }
        any_enabled = true;
        if valid_provider_id(&record.provider) {
            if !enabled_valid.iter().any(|id| id == &record.provider) {
                enabled_valid.push(record.provider);
            }
        }
    }
    if any_enabled && enabled_valid.is_empty() {
        return Err(ProviderConfigError::AllInvalid);
    }
    Ok(enabled_valid)
}

#[derive(Debug)]
pub enum ProviderConfigError {
    /// The discovery payload was not a JSON array of provider config records.
    Parse(serde_json::Error),
    /// The discovery payload had enabled records but none passed
    /// [`valid_provider_id`]; treat as discovery failure rather than empty.
    AllInvalid,
}

impl std::fmt::Display for ProviderConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "invalid codexbar config providers payload: {err}"),
            Self::AllInvalid => {
                f.write_str("codexbar config providers returned only invalid provider ids")
            }
        }
    }
}

impl std::error::Error for ProviderConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(err) => Some(err),
            Self::AllInvalid => None,
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
        assert!(matches!(err, ProviderConfigError::AllInvalid));
    }

    #[test]
    fn parse_provider_config_keeps_mixed_valid_invalid_subset() {
        // Invalid ids are silently skipped when at least one valid id passes.
        let payload = br#"[
            {"provider": "ok.id",  "enabled": true},
            {"provider": "bad/id", "enabled": true}
        ]"#;
        let ids = parse_provider_config_payload(payload).expect("partial valid");
        assert_eq!(ids, vec!["ok.id"]);
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
}
