use serde::Serialize;
use serde_json::Value;

use crate::codexbar::{
    valid_provider_id, NamedWindow, ProviderRecord, UsageWindow, MAX_USAGE_JSON_BYTES,
};
use crate::config::RenderConfig;
use crate::render::RenderError;
use crate::reset::minutes_until;

#[derive(Serialize)]
pub(crate) struct ProviderMetric {
    pub(crate) provider: String,
    pub(crate) windows: WindowsMetric,
    #[serde(rename = "extraRateWindows")]
    pub(crate) extra_rate_windows: Vec<ExtraWindowMetric>,
    error: Option<ErrorMetric>,
}

#[derive(Serialize)]
pub(crate) struct WindowsMetric {
    pub(crate) primary: Option<WindowMetric>,
    pub(crate) secondary: Option<WindowMetric>,
    pub(crate) tertiary: Option<WindowMetric>,
}

#[derive(Clone, Serialize)]
pub(crate) struct WindowMetric {
    #[serde(rename = "usedPercent")]
    pub(crate) used_percent: i32,
    #[serde(rename = "remainingPercent")]
    pub(crate) remaining_percent: i32,
    #[serde(rename = "resetsAt")]
    resets_at: Option<String>,
    #[serde(rename = "resetDescription")]
    reset_description: Option<String>,
    #[serde(rename = "windowMinutes")]
    window_minutes: Option<i64>,
    #[serde(rename = "minutesUntilReset")]
    pub(crate) minutes_until_reset: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct ExtraWindowMetric {
    pub(crate) title: Option<String>,
    #[serde(rename = "usageKnown")]
    pub(crate) usage_known: bool,
    #[serde(rename = "usedPercent")]
    pub(crate) used_percent: Option<i32>,
    #[serde(rename = "remainingPercent")]
    pub(crate) remaining_percent: Option<i32>,
    #[serde(rename = "resetsAt")]
    resets_at: Option<String>,
    #[serde(rename = "resetDescription")]
    reset_description: Option<String>,
    #[serde(rename = "windowMinutes")]
    window_minutes: Option<i64>,
    #[serde(rename = "minutesUntilReset")]
    pub(crate) minutes_until_reset: Option<i64>,
}

#[derive(Serialize)]
struct ErrorMetric {
    kind: ErrorKind,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum ErrorKind {
    Auth,
    Cookies,
    Network,
    Unknown,
}

pub fn emit_provider_metrics(
    payload: &[u8],
    config: &RenderConfig,
    now_epoch: i64,
) -> Result<String, RenderError> {
    let metrics = provider_metrics(payload, config, now_epoch)?;
    serde_json::to_string(&metrics).map_err(|_| RenderError::InvalidPayload)
}

pub(crate) fn provider_metrics(
    payload: &[u8],
    config: &RenderConfig,
    now_epoch: i64,
) -> Result<Vec<ProviderMetric>, RenderError> {
    let records = parse_display_payload(payload)?;
    let mut metrics: Vec<ProviderMetric> = records
        .iter()
        .filter(|record| passes_provider_filters(record, config))
        .filter_map(|record| provider_metric(record, config, now_epoch))
        .collect();

    sort_metrics(&mut metrics, config);
    Ok(metrics)
}

/// Parse the array transport and provider identity contract shared by the
/// display emitters. Positional windows with no measurement are valid unknown
/// state; their typed shape is still enforced by serde.
pub(crate) fn parse_display_payload(payload: &[u8]) -> Result<Vec<ProviderRecord>, RenderError> {
    if payload.len() > MAX_USAGE_JSON_BYTES {
        return Err(RenderError::InvalidPayload);
    }
    let records: Vec<ProviderRecord> =
        serde_json::from_slice(payload).map_err(|_| RenderError::InvalidPayload)?;
    if records
        .iter()
        .all(|record| valid_provider_id(&record.provider))
    {
        Ok(records)
    } else {
        Err(RenderError::InvalidPayload)
    }
}

fn provider_metric(
    record: &ProviderRecord,
    config: &RenderConfig,
    now_epoch: i64,
) -> Option<ProviderMetric> {
    let has_renderable_window = has_renderable_window(record);
    match (record.error.as_ref(), has_renderable_window) {
        (None, true) => Some(renderable_metric(record, config, now_epoch)),
        (Some(error), false) => Some(error_metric(&record.provider, error)),
        _ => None,
    }
}

fn renderable_metric(
    record: &ProviderRecord,
    config: &RenderConfig,
    now_epoch: i64,
) -> ProviderMetric {
    let usage = record
        .usage
        .as_ref()
        .expect("renderable metric requires usage");
    ProviderMetric {
        provider: record.provider.clone(),
        windows: WindowsMetric {
            primary: window_metric(usage.primary.as_ref(), config, now_epoch),
            secondary: window_metric(usage.secondary.as_ref(), config, now_epoch),
            tertiary: window_metric(usage.tertiary.as_ref(), config, now_epoch),
        },
        extra_rate_windows: usage
            .extra_rate_windows
            .iter()
            .map(|extra| extra_window_metric(extra, config, now_epoch))
            .collect(),
        error: None,
    }
}

fn error_metric(provider: &str, error: &Value) -> ProviderMetric {
    let message = sanitize_error_message(error_raw(error));
    ProviderMetric {
        provider: provider.to_owned(),
        windows: WindowsMetric {
            primary: None,
            secondary: None,
            tertiary: None,
        },
        extra_rate_windows: Vec::new(),
        error: Some(ErrorMetric {
            kind: error_kind(&message),
            message,
        }),
    }
}

fn passes_provider_filters(record: &ProviderRecord, config: &RenderConfig) -> bool {
    valid_provider_id(&record.provider)
        && (config.providers.is_empty() || contains(&config.providers, &record.provider))
        && !contains(&config.providers_exclude, &record.provider)
}

fn has_renderable_window(record: &ProviderRecord) -> bool {
    record.usage.as_ref().is_some_and(|usage| {
        [
            usage.primary.as_ref(),
            usage.secondary.as_ref(),
            usage.tertiary.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|window| numeric_percent(window.used_percent).is_some())
            || usage.extra_rate_windows.iter().any(|extra| {
                extra.usage_known != Some(false)
                    && extra
                        .window
                        .as_ref()
                        .is_some_and(|window| numeric_percent(window.used_percent).is_some())
            })
    })
}

fn window_metric(
    window: Option<&UsageWindow>,
    config: &RenderConfig,
    now_epoch: i64,
) -> Option<WindowMetric> {
    let window = window?;
    let used_percent = numeric_percent(window.used_percent)?;
    Some(WindowMetric {
        used_percent,
        remaining_percent: 100 - used_percent,
        resets_at: window.resets_at.clone(),
        reset_description: window.reset_description.clone(),
        window_minutes: window.window_minutes(),
        minutes_until_reset: window.reset_value().and_then(|reset| {
            minutes_until(
                reset,
                now_epoch,
                config.reset_description_timezone_offset_minutes,
            )
        }),
    })
}

fn extra_window_metric(
    extra: &NamedWindow,
    config: &RenderConfig,
    now_epoch: i64,
) -> ExtraWindowMetric {
    let metric = if extra.usage_known == Some(false) {
        None
    } else {
        window_metric(extra.window.as_ref(), config, now_epoch)
    };

    match metric {
        Some(metric) => ExtraWindowMetric {
            title: extra.title.clone(),
            usage_known: true,
            used_percent: Some(metric.used_percent),
            remaining_percent: Some(metric.remaining_percent),
            resets_at: metric.resets_at,
            reset_description: metric.reset_description,
            window_minutes: metric.window_minutes,
            minutes_until_reset: metric.minutes_until_reset,
        },
        None => ExtraWindowMetric {
            title: extra.title.clone(),
            usage_known: false,
            used_percent: None,
            remaining_percent: None,
            resets_at: None,
            reset_description: None,
            window_minutes: None,
            minutes_until_reset: None,
        },
    }
}

fn numeric_percent(value: Option<f64>) -> Option<i32> {
    value
        .filter(|value| value.is_finite())
        .map(|value| value.floor().clamp(0.0, 100.0) as i32)
}

fn error_raw(error: &Value) -> &Value {
    if let Value::Object(map) = error {
        map.get("message")
            .or_else(|| map.get("error"))
            .or_else(|| map.get("reason"))
            .or_else(|| map.get("description"))
            .unwrap_or(error)
    } else {
        error
    }
}

fn is_email_local(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

fn is_email_domain(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

// Character class for a credential-shaped run (base64/base64url/hex/token bodies).
fn is_secret_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '_' | '-' | '=')
}

// Redact `Cookie:`/`Authorization:` header fragments (through end of line),
// case-insensitively, on a word boundary. Runs before control-stripping so the
// end-of-line bound is still meaningful.
fn redact_headers(input: &str) -> String {
    const KEYWORDS: [&[char]; 2] = [
        &['c', 'o', 'o', 'k', 'i', 'e'],
        &[
            'a', 'u', 't', 'h', 'o', 'r', 'i', 'z', 'a', 't', 'i', 'o', 'n',
        ],
    ];
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    'outer: while i < chars.len() {
        for kw in KEYWORDS {
            if i + kw.len() <= chars.len()
                && (i == 0 || !chars[i - 1].is_ascii_alphanumeric())
                && chars[i..i + kw.len()]
                    .iter()
                    .zip(kw.iter())
                    .all(|(c, k)| c.to_ascii_lowercase() == *k)
            {
                let mut j = i + kw.len();
                while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ':' {
                    let mut k = j + 1;
                    while k < chars.len() && chars[k] != '\r' && chars[k] != '\n' {
                        k += 1;
                    }
                    out.push_str("<redacted-header>");
                    i = k;
                    continue 'outer;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// Replace email addresses (local@domain.tld, tld >= 2 alpha) with a placeholder.
fn redact_emails(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        if let Some(end) = match_email(&chars, i) {
            out.push_str("<redacted-email>");
            i = end;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn match_email(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    while i < chars.len() && is_email_local(chars[i]) {
        i += 1;
    }
    if i == start || i >= chars.len() || chars[i] != '@' {
        return None;
    }
    let domain_start = i + 1;
    let mut end = domain_start;
    while end < chars.len() && is_email_domain(chars[end]) {
        end += 1;
    }
    // Backtrack the greedy domain run until it ends in `.<alpha>{2,}`.
    while end > domain_start + 1 {
        let domain = &chars[domain_start..end];
        if let Some(dot) = domain.iter().rposition(|&c| c == '.') {
            let tld = &domain[dot + 1..];
            if tld.len() >= 2 && tld.iter().all(|c| c.is_ascii_alphabetic()) {
                return Some(end);
            }
        }
        end -= 1;
    }
    None
}

// Replace maximal runs of >= 20 credential-shaped chars with a placeholder.
// A superset of the shell's hex/base64/bearer-token redactions.
fn redact_secret_runs(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        if is_secret_char(chars[i]) {
            let start = i;
            while i < chars.len() && is_secret_char(chars[i]) {
                i += 1;
            }
            if i - start >= 20 {
                out.push_str("<redacted-secret>");
            } else {
                out.extend(&chars[start..i]);
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

// Redact the current user's home-directory prefix from paths.
fn redact_home(input: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if home.len() >= 2 && home != "/" => input.replace(&home, "<redacted-home>"),
        _ => input.to_string(),
    }
}

// Strip credential/identity material from a provider error message before it
// crosses the CLI/plugin output boundary (logs, pasted diagnostics). Mirrors the
// shell `sanitize_error_message` in bin/showy-quota-state so every consumer of
// the metrics renderer is protected, not only the shell-state wrapper.
fn sanitize_error_message(value: &Value) -> String {
    let raw = match value {
        Value::Null => String::new(),
        Value::String(message) => message.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    let headers = redact_headers(&raw);
    let without_controls: String = headers.chars().filter(|ch| !ch.is_control()).collect();
    let collapsed = without_controls
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let no_home = redact_home(&collapsed);
    let no_email = redact_emails(&no_home);
    let redacted = redact_secret_runs(&no_email);
    redacted.chars().take(160).collect()
}

fn error_kind(message: &str) -> ErrorKind {
    let lower = message.to_lowercase();
    if lower.contains("auth")
        || lower.contains("login")
        || lower.contains("session")
        || lower.contains("token")
    {
        ErrorKind::Auth
    } else if lower.contains("cookie") {
        ErrorKind::Cookies
    } else if lower.contains("timeout")
        || lower.contains("connect")
        || lower.contains("network")
        || lower.contains("refused")
    {
        ErrorKind::Network
    } else {
        ErrorKind::Unknown
    }
}

fn sort_metrics(metrics: &mut [ProviderMetric], config: &RenderConfig) {
    if !config.providers.is_empty() {
        let order = &config.providers;
        metrics.sort_by(|a, b| metric_cmp(a, b, order));
    } else if !config.provider_order.is_empty() {
        let order = &config.provider_order;
        metrics.sort_by(|a, b| metric_cmp(a, b, order));
    }
}

fn metric_cmp(a: &ProviderMetric, b: &ProviderMetric, order: &[String]) -> std::cmp::Ordering {
    position(order, &a.provider)
        .cmp(&position(order, &b.provider))
        .then_with(|| a.provider.cmp(&b.provider))
}

fn position(items: &[String], provider: &str) -> usize {
    items
        .iter()
        .position(|item| item == provider)
        .unwrap_or(1_000_000)
}

fn contains(items: &[String], provider: &str) -> bool {
    items.iter().any(|item| item == provider)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    const NOW: i64 = 4_070_908_800;

    fn config() -> RenderConfig {
        RenderConfig {
            provider_order: Vec::new(),
            reset_description_timezone_offset_minutes: Some(0),
            ..RenderConfig::default()
        }
    }

    #[test]
    fn sanitize_error_message_redacts_secrets_email_and_headers() {
        let raw = Value::String(
            "auth failed for alice@example.test token \
             0123456789abcdef0123456789abcdef01234567; Authorization: Bearer abc"
                .to_string(),
        );
        let out = sanitize_error_message(&raw);
        assert!(!out.contains("alice@example.test"), "email leaked: {out}");
        assert!(
            !out.contains("0123456789abcdef0123456789abcdef01234567"),
            "token leaked: {out}"
        );
        assert!(
            out.contains("<redacted-email>"),
            "no email placeholder: {out}"
        );
        assert!(
            out.contains("<redacted-secret>"),
            "no secret placeholder: {out}"
        );
        assert!(
            out.contains("<redacted-header>"),
            "no header placeholder: {out}"
        );
    }

    #[test]
    fn sanitize_error_message_leaves_ordinary_text() {
        let raw = Value::String("network timeout after 3 retries".to_string());
        assert_eq!(
            sanitize_error_message(&raw),
            "network timeout after 3 retries"
        );
    }

    fn emit(payload: &[u8]) -> String {
        emit_provider_metrics(payload, &config(), NOW).expect("metrics json")
    }

    fn emit_value(payload: &[u8]) -> Value {
        serde_json::from_str(&emit(payload)).expect("metrics value")
    }

    #[test]
    fn renderable_schema_field_order_and_null_tertiary() {
        let output = emit(
            br#"[{"provider":"codex","usage":{"primary":{"usedPercent":42.9,"resetsAt":"2099-01-01T01:00:00Z","resetDescription":"Resets Jan 1, 2099 1:00 AM","windowMinutes":300},"secondary":{"usedPercent":101},"tertiary":null}}]"#,
        );

        assert_eq!(
            output,
            "[{\"provider\":\"codex\",\"windows\":{\"primary\":{\"usedPercent\":42,\"remainingPercent\":58,\"resetsAt\":\"2099-01-01T01:00:00Z\",\"resetDescription\":\"Resets Jan 1, 2099 1:00 AM\",\"windowMinutes\":300,\"minutesUntilReset\":60},\"secondary\":{\"usedPercent\":100,\"remainingPercent\":0,\"resetsAt\":null,\"resetDescription\":null,\"windowMinutes\":null,\"minutesUntilReset\":null},\"tertiary\":null},\"extraRateWindows\":[],\"error\":null}]"
        );
    }

    #[test]
    fn error_only_records_are_bucketed_by_kind() {
        let value = emit_value(
            br#"[
                {"provider":"authp","error":{"message":"login token expired"}},
                {"provider":"cookiep","error":{"error":"cookie jar missing"}},
                {"provider":"netp","error":{"reason":"connect timeout refused"}},
                {"provider":"unknownp","error":{"description":"quota unavailable"}}
            ]"#,
        );

        let kinds: Vec<_> = value
            .as_array()
            .expect("array")
            .iter()
            .map(|metric| metric["error"]["kind"].as_str().expect("kind"))
            .collect();
        assert_eq!(kinds, vec!["auth", "cookies", "network", "unknown"]);
        assert!(value.as_array().expect("array").iter().all(|metric| {
            metric["windows"] == json!({"primary": null, "secondary": null, "tertiary": null})
                && metric["extraRateWindows"] == json!([])
        }));
    }

    #[test]
    fn error_message_is_sanitized_and_truncated() {
        // Space-separated words exceed the 160-char cap without forming a >=20
        // credential-shaped run (which the redactor would collapse), so this
        // exercises truncation, not redaction.
        let long = format!("  alpha\u{0007}   beta \n gamma {}  ", "word ".repeat(45));
        let payload =
            serde_json::to_vec(&json!([{ "provider": "codex", "error": { "message": long } }]))
                .expect("payload");
        let value = emit_value(&payload);
        let message = value[0]["error"]["message"].as_str().expect("message");

        assert_eq!(message.chars().count(), 160);
        assert!(message.starts_with("alpha beta gamma "));
        assert!(!message.chars().any(char::is_control));
        assert!(!message.contains("  "));
    }

    #[test]
    fn known_and_unknown_extra_windows_are_flattened() {
        let value = emit_value(
            br#"[{"provider":"codex","usage":{"primary":{"usedPercent":1},"extraRateWindows":[{"title":"Known","window":{"usedPercent":12.7,"resetsAt":"2099-01-01T01:00:00Z","windowMinutes":60}},{"title":"Unknown","usageKnown":false,"window":{"usedPercent":99,"resetsAt":"2099-01-01T01:00:00Z","windowMinutes":60}},{"title":"Missing Window"}]}}]"#,
        );

        assert_eq!(
            value[0]["extraRateWindows"],
            json!([
                {
                    "title": "Known",
                    "usageKnown": true,
                    "usedPercent": 12,
                    "remainingPercent": 88,
                    "resetsAt": "2099-01-01T01:00:00Z",
                    "resetDescription": null,
                    "windowMinutes": 60,
                    "minutesUntilReset": 60
                },
                {
                    "title": "Unknown",
                    "usageKnown": false,
                    "usedPercent": null,
                    "remainingPercent": null,
                    "resetsAt": null,
                    "resetDescription": null,
                    "windowMinutes": null,
                    "minutesUntilReset": null
                },
                {
                    "title": "Missing Window",
                    "usageKnown": false,
                    "usedPercent": null,
                    "remainingPercent": null,
                    "resetsAt": null,
                    "resetDescription": null,
                    "windowMinutes": null,
                    "minutesUntilReset": null
                }
            ])
        );
    }

    #[test]
    fn error_with_numeric_window_is_excluded_by_state_rule() {
        let value = emit_value(
            br#"[
                {"provider":"codex","error":{"message":"login failed"},"usage":{"primary":{"usedPercent":10}}},
                {"provider":"claude","usage":{"primary":{"usedPercent":20}}}
            ]"#,
        );

        assert_eq!(value.as_array().expect("array").len(), 1);
        assert_eq!(value[0]["provider"], "claude");
        assert!(value[0]["error"].is_null());
    }

    #[test]
    fn rejects_non_array_and_wrongly_typed_windows() {
        for payload in [
            br#"{}"#.as_slice(),
            br#""quota""#.as_slice(),
            br#"42"#.as_slice(),
            br#"[{"provider":"codex","usage":{"primary":"invalid"}}]"#.as_slice(),
        ] {
            assert!(matches!(
                emit_provider_metrics(payload, &config(), NOW),
                Err(RenderError::InvalidPayload)
            ));
        }
    }

    #[test]
    fn rejects_invalid_provider_ids() {
        assert!(matches!(
            emit_provider_metrics(
                br#"[{"provider":".","usage":{"primary":{"usedPercent":10}}}]"#,
                &config(),
                NOW,
            ),
            Err(RenderError::InvalidPayload)
        ));
    }
}
