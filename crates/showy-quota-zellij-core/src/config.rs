use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct RenderConfig {
    pub providers: Vec<String>,
    pub providers_exclude: Vec<String>,
    pub provider_order: Vec<String>,
    pub include_status: bool,

    pub palette_primary_good: String,
    pub palette_primary_warn: String,
    pub palette_primary_bad: String,
    pub palette_primary_unknown: String,
    pub palette_secondary_good: Option<String>,
    pub palette_secondary_warn: Option<String>,
    pub palette_secondary_bad: Option<String>,
    pub palette_secondary_unknown: Option<String>,
    pub palette_tertiary_good: Option<String>,
    pub palette_tertiary_warn: Option<String>,
    pub palette_tertiary_bad: Option<String>,
    pub palette_tertiary_unknown: Option<String>,
    pub palette_secondary_scale: String,
    pub palette_tertiary_scale: String,
    pub palette_bg: String,
    pub palette_surface: String,
    pub palette_track: String,
    pub palette_icon_text: String,
    pub palette_countdown: String,
    pub palette_countdown_warn: String,
    pub palette_stale: String,
    pub palette_elapsed: String,
    pub stale_glyph: String,

    pub good_min_remaining: i32,
    pub warn_min_remaining: i32,
    pub time_warn_minutes: i64,

    pub zellij_bar_width: usize,
    pub terminal_bar_mode: String,
    pub mono3_providers: Vec<String>,
    pub mono3_providers_exclude: Vec<String>,
    pub mono3_color_mode: String,
    pub mono3_marker_source: String,
    pub mono3_marker_style: String,
    pub cap_left: String,
    pub cap_right: String,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            providers_exclude: Vec::new(),
            provider_order: csv("codex,claude,copilot,opencode,gemini"),
            include_status: true,
            palette_primary_good: "25be6a".into(),
            palette_primary_warn: "f0af00".into(),
            palette_primary_bad: "ee5396".into(),
            palette_primary_unknown: "6c7086".into(),
            palette_secondary_good: None,
            palette_secondary_warn: None,
            palette_secondary_bad: None,
            palette_secondary_unknown: None,
            palette_tertiary_good: None,
            palette_tertiary_warn: None,
            palette_tertiary_bad: None,
            palette_tertiary_unknown: None,
            palette_secondary_scale: "0.55".into(),
            palette_tertiary_scale: "0.55".into(),
            palette_bg: "161616".into(),
            palette_surface: "2a2a2a".into(),
            palette_track: "3a3a4a".into(),
            palette_icon_text: "f2f4f8".into(),
            palette_countdown: "7b8496".into(),
            palette_countdown_warn: "ee5396".into(),
            palette_stale: "6c7086".into(),
            palette_elapsed: "be95ff".into(),
            stale_glyph: "⚠".into(),
            good_min_remaining: 40,
            warn_min_remaining: 15,
            time_warn_minutes: 30,
            zellij_bar_width: 12,
            terminal_bar_mode: "auto".into(),
            mono3_providers: csv("gemini,antigravity"),
            mono3_providers_exclude: Vec::new(),
            mono3_color_mode: "lowest".into(),
            mono3_marker_source: "primary".into(),
            mono3_marker_style: "replace".into(),
            cap_left: "".into(),
            cap_right: "".into(),
        }
    }
}

impl RenderConfig {
    pub fn from_env() -> Self {
        let mut config = Self::default();
        let get = |name: &str| std::env::var(name).ok();
        config.apply_getter(|name| get(name));
        config
    }

    pub fn from_kdl_config(kdl: &BTreeMap<String, String>) -> Self {
        let mut config = Self::default();
        config.apply_getter(|name| get_from_kdl(kdl, name));
        config
    }

    fn apply_getter<F>(&mut self, get: F)
    where
        F: Fn(&str) -> Option<String>,
    {
        self.providers = get_csv(&get, "SHOWY_QUOTA_PROVIDERS", &self.providers);
        self.providers_exclude = get_csv(
            &get,
            "SHOWY_QUOTA_PROVIDERS_EXCLUDE",
            &self.providers_exclude,
        );
        self.provider_order = get_csv(&get, "SHOWY_QUOTA_PROVIDER_ORDER", &self.provider_order);
        self.include_status = get_bool(&get, "SHOWY_QUOTA_INCLUDE_STATUS", self.include_status);

        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_PRIMARY_GOOD",
            &mut self.palette_primary_good,
        );
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_PRIMARY_WARN",
            &mut self.palette_primary_warn,
        );
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_PRIMARY_BAD",
            &mut self.palette_primary_bad,
        );
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_PRIMARY_UNKNOWN",
            &mut self.palette_primary_unknown,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_SECONDARY_GOOD",
            &mut self.palette_secondary_good,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_SECONDARY_WARN",
            &mut self.palette_secondary_warn,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_SECONDARY_BAD",
            &mut self.palette_secondary_bad,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_SECONDARY_UNKNOWN",
            &mut self.palette_secondary_unknown,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_TERTIARY_GOOD",
            &mut self.palette_tertiary_good,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_TERTIARY_WARN",
            &mut self.palette_tertiary_warn,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_TERTIARY_BAD",
            &mut self.palette_tertiary_bad,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_TERTIARY_UNKNOWN",
            &mut self.palette_tertiary_unknown,
        );
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_SECONDARY_SCALE",
            &mut self.palette_secondary_scale,
        );
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_TERTIARY_SCALE",
            &mut self.palette_tertiary_scale,
        );
        assign_string(&get, "SHOWY_QUOTA_PALETTE_BG", &mut self.palette_bg);
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_SURFACE",
            &mut self.palette_surface,
        );
        assign_string(&get, "SHOWY_QUOTA_PALETTE_TRACK", &mut self.palette_track);
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_ICON_TEXT",
            &mut self.palette_icon_text,
        );
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_COUNTDOWN",
            &mut self.palette_countdown,
        );
        if let Some(value) = get("SHOWY_QUOTA_PALETTE_COUNTDOWN_WARN") {
            self.palette_countdown_warn = value;
        } else {
            self.palette_countdown_warn
                .clone_from(&self.palette_primary_bad);
        }
        if let Some(value) = get("SHOWY_QUOTA_PALETTE_STALE") {
            self.palette_stale = value;
        } else {
            self.palette_stale.clone_from(&self.palette_primary_unknown);
        }
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_ELAPSED",
            &mut self.palette_elapsed,
        );
        assign_string(&get, "SHOWY_QUOTA_STALE_GLYPH", &mut self.stale_glyph);

        self.good_min_remaining = get_i32(
            &get,
            "SHOWY_QUOTA_GOOD_MIN_REMAINING",
            self.good_min_remaining,
        );
        self.warn_min_remaining = get_i32(
            &get,
            "SHOWY_QUOTA_WARN_MIN_REMAINING",
            self.warn_min_remaining,
        );
        self.time_warn_minutes = get_i64(
            &get,
            "SHOWY_QUOTA_TIME_WARN_MINUTES",
            self.time_warn_minutes,
        );

        self.zellij_bar_width =
            get_usize(&get, "SHOWY_QUOTA_ZELLIJ_BAR_WIDTH", self.zellij_bar_width);
        assign_string(
            &get,
            "SHOWY_QUOTA_TERMINAL_BAR_MODE",
            &mut self.terminal_bar_mode,
        );
        self.mono3_providers = get_csv(&get, "SHOWY_QUOTA_MONO3_PROVIDERS", &self.mono3_providers);
        self.mono3_providers_exclude = get_csv(
            &get,
            "SHOWY_QUOTA_MONO3_PROVIDERS_EXCLUDE",
            &self.mono3_providers_exclude,
        );
        assign_string(
            &get,
            "SHOWY_QUOTA_MONO3_COLOR_MODE",
            &mut self.mono3_color_mode,
        );
        assign_string(
            &get,
            "SHOWY_QUOTA_MONO3_MARKER_SOURCE",
            &mut self.mono3_marker_source,
        );
        assign_string(
            &get,
            "SHOWY_QUOTA_MONO3_MARKER_STYLE",
            &mut self.mono3_marker_style,
        );
        assign_string(&get, "SHOWY_QUOTA_CAP_LEFT", &mut self.cap_left);
        assign_string(&get, "SHOWY_QUOTA_CAP_RIGHT", &mut self.cap_right);
    }
}

fn get_from_kdl(kdl: &BTreeMap<String, String>, env_name: &str) -> Option<String> {
    if let Some(value) = kdl.get(env_name) {
        return Some(value.clone());
    }
    for alias in kdl_aliases(env_name) {
        if let Some(value) = kdl.get(*alias) {
            return Some(value.clone());
        }
    }
    let lower = env_name
        .strip_prefix("SHOWY_QUOTA_")
        .unwrap_or(env_name)
        .to_ascii_lowercase();
    kdl.get(&lower).cloned()
}

fn kdl_aliases(env_name: &str) -> &'static [&'static str] {
    match env_name {
        "SHOWY_QUOTA_ZELLIJ_BAR_WIDTH" => &["bar_width"],
        "SHOWY_QUOTA_PROVIDERS" => &["providers"],
        "SHOWY_QUOTA_PROVIDERS_EXCLUDE" => &["providers_exclude"],
        "SHOWY_QUOTA_PROVIDER_ORDER" => &["provider_order"],
        _ => &[],
    }
}

fn assign_string<F>(get: &F, name: &str, target: &mut String)
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(value) = get(name) {
        *target = value;
    }
}

fn assign_option<F>(get: &F, name: &str, target: &mut Option<String>)
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(value) = get(name) {
        *target = if value.is_empty() { None } else { Some(value) };
    }
}

fn get_csv<F>(get: &F, name: &str, current: &[String]) -> Vec<String>
where
    F: Fn(&str) -> Option<String>,
{
    get(name).map_or_else(|| current.to_vec(), |value| csv(&value))
}

fn get_bool<F>(get: &F, name: &str, default: bool) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    get(name).map_or(default, |value| {
        value == "1" || value.eq_ignore_ascii_case("true")
    })
}

fn get_i32<F>(get: &F, name: &str, default: i32) -> i32
where
    F: Fn(&str) -> Option<String>,
{
    get(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn get_i64<F>(get: &F, name: &str, default: i64) -> i64
where
    F: Fn(&str) -> Option<String>,
{
    get(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn get_usize<F>(get: &F, name: &str, default: usize) -> usize
where
    F: Fn(&str) -> Option<String>,
{
    get(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

pub fn csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_and_stale_palettes_follow_primary_defaults() {
        let mut kdl = BTreeMap::new();
        kdl.insert("palette_primary_bad".into(), "111111".into());
        kdl.insert("palette_primary_unknown".into(), "222222".into());

        let config = RenderConfig::from_kdl_config(&kdl);

        assert_eq!(config.palette_countdown_warn, "111111");
        assert_eq!(config.palette_stale, "222222");
    }

    #[test]
    fn warning_and_stale_palette_overrides_win() {
        let mut kdl = BTreeMap::new();
        kdl.insert("palette_primary_bad".into(), "111111".into());
        kdl.insert("palette_primary_unknown".into(), "222222".into());
        kdl.insert("palette_countdown_warn".into(), "333333".into());
        kdl.insert("palette_stale".into(), "444444".into());

        let config = RenderConfig::from_kdl_config(&kdl);

        assert_eq!(config.palette_countdown_warn, "333333");
        assert_eq!(config.palette_stale, "444444");
    }
}
