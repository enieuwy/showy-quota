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
