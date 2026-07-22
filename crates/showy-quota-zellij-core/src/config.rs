use std::collections::BTreeMap;

const GLYPH_MAX_CHARS: usize = 16;

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
    pub palette_dim_good: Option<String>,
    pub palette_dim_warn: Option<String>,
    pub palette_dim_bad: Option<String>,
    pub palette_dim_unknown: Option<String>,
    pub palette_dim_scale: String,
    pub palette_bg: String,
    pub palette_surface: String,
    pub palette_track: String,
    pub palette_icon_text: String,
    pub palette_countdown: String,
    pub palette_countdown_warn: String,
    pub palette_stale: String,
    pub palette_elapsed: String,
    pub palette_elapsed_long: String,
    pub stale_glyph: String,
    pub degraded_cli_glyph: String,
    pub error_glyph: String,

    pub reset_description_timezone_offset_minutes: Option<i16>,
    pub good_min_remaining: i32,
    pub warn_min_remaining: i32,
    pub time_warn_minutes: i64,
    pub dim_window_minutes: i64,

    pub zellij_bar_width: usize,
    pub tmux_bar_width: Option<usize>,
    pub terminal_bar_mode: String,
    pub provider_modes: Vec<(String, String)>,
    pub mono_color_mode: String,
    pub mono_markers: Vec<String>,
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
            palette_dim_good: None,
            palette_dim_warn: None,
            palette_dim_bad: None,
            palette_dim_unknown: None,
            palette_dim_scale: "0.55".into(),
            palette_bg: "161616".into(),
            palette_surface: "2a2a2a".into(),
            palette_track: "3a3a4a".into(),
            palette_icon_text: "f2f4f8".into(),
            palette_countdown: "7b8496".into(),
            palette_countdown_warn: "ee5396".into(),
            palette_stale: "6c7086".into(),
            palette_elapsed: "be95ff".into(),
            palette_elapsed_long: "3ddbd9".into(),
            stale_glyph: "⚠".into(),
            degraded_cli_glyph: "⚠cli".into(),
            error_glyph: "⚠".into(),
            reset_description_timezone_offset_minutes: None,
            good_min_remaining: 40,
            warn_min_remaining: 15,
            time_warn_minutes: 30,
            dim_window_minutes: 10080,
            zellij_bar_width: 12,
            tmux_bar_width: None,
            terminal_bar_mode: "auto".into(),
            provider_modes: vec![
                ("gemini".into(), "mono3".into()),
                ("cursor".into(), "mono3".into()),
            ],
            mono_color_mode: "lowest".into(),
            mono_markers: vec!["primary".into()],
            cap_left: "".into(),
            cap_right: "".into(),
        }
    }
}

impl RenderConfig {
    pub fn from_env() -> Self {
        let env: BTreeMap<String, String> = std::env::vars().collect();
        Self::from_env_map(&env)
    }

    pub fn from_env_map(env: &BTreeMap<String, String>) -> Self {
        let mut config = Self::default();
        config.apply_getter(|name| env.get(name).cloned());
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
            "SHOWY_QUOTA_PALETTE_DIM_GOOD",
            &mut self.palette_dim_good,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_DIM_WARN",
            &mut self.palette_dim_warn,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_DIM_BAD",
            &mut self.palette_dim_bad,
        );
        assign_option(
            &get,
            "SHOWY_QUOTA_PALETTE_DIM_UNKNOWN",
            &mut self.palette_dim_unknown,
        );
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_DIM_SCALE",
            &mut self.palette_dim_scale,
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
        assign_string(
            &get,
            "SHOWY_QUOTA_PALETTE_ELAPSED_LONG",
            &mut self.palette_elapsed_long,
        );
        assign_glyph(&get, "SHOWY_QUOTA_STALE_GLYPH", &mut self.stale_glyph);
        assign_glyph(
            &get,
            "SHOWY_QUOTA_DEGRADED_CLI_GLYPH",
            &mut self.degraded_cli_glyph,
        );
        assign_glyph(&get, "SHOWY_QUOTA_ERROR_GLYPH", &mut self.error_glyph);
        self.reset_description_timezone_offset_minutes = get_timezone_offset_minutes(
            &get,
            "SHOWY_QUOTA_RESET_DESCRIPTION_TIMEZONE_OFFSET",
            self.reset_description_timezone_offset_minutes,
        );

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
        // Keep the Warn band reachable: if a user inverts the thresholds
        // (good_min < warn_min), the `remaining >= good_min` test in color_key
        // would win first and Warn could never fire. Swap them so both
        // thresholds stay meaningful instead of silently dropping a state.
        if self.good_min_remaining < self.warn_min_remaining {
            std::mem::swap(&mut self.good_min_remaining, &mut self.warn_min_remaining);
        }
        self.time_warn_minutes = get_i64(
            &get,
            "SHOWY_QUOTA_TIME_WARN_MINUTES",
            self.time_warn_minutes,
        );
        self.dim_window_minutes = get_i64(
            &get,
            "SHOWY_QUOTA_DIM_WINDOW_MINUTES",
            self.dim_window_minutes,
        );

        self.zellij_bar_width =
            get_usize(&get, "SHOWY_QUOTA_ZELLIJ_BAR_WIDTH", self.zellij_bar_width);
        self.tmux_bar_width = get("SHOWY_QUOTA_TMUX_BAR_WIDTH")
            .and_then(|value| value.parse().ok())
            .or(self.tmux_bar_width);
        assign_string(
            &get,
            "SHOWY_QUOTA_TERMINAL_BAR_MODE",
            &mut self.terminal_bar_mode,
        );
        self.provider_modes =
            get_provider_modes(&get, "SHOWY_QUOTA_PROVIDER_MODES", &self.provider_modes);
        assign_string(
            &get,
            "SHOWY_QUOTA_MONO_COLOR_MODE",
            &mut self.mono_color_mode,
        );
        self.mono_markers = get_csv(&get, "SHOWY_QUOTA_MONO_MARKERS", &self.mono_markers);
        assign_glyph(&get, "SHOWY_QUOTA_CAP_LEFT", &mut self.cap_left);
        assign_glyph(&get, "SHOWY_QUOTA_CAP_RIGHT", &mut self.cap_right);
    }

    /// Explicit per-provider terminal body override, if any.
    pub fn mode_for(&self, provider: &str) -> Option<&str> {
        self.provider_modes
            .iter()
            .find(|(name, _)| name == provider)
            .map(|(_, mode)| mode.as_str())
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
        "SHOWY_QUOTA_DEGRADED_CLI_GLYPH" => &["degraded_cli_glyph"],
        "SHOWY_QUOTA_ERROR_GLYPH" => &["error_glyph"],
        "SHOWY_QUOTA_RESET_DESCRIPTION_TIMEZONE_OFFSET" => &["reset_description_timezone_offset"],
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

fn assign_glyph<F>(get: &F, name: &str, target: &mut String)
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(value) = get(name) {
        if valid_glyph(&value) {
            *target = value;
        }
    }
}

fn valid_glyph(value: &str) -> bool {
    value.chars().count() <= GLYPH_MAX_CHARS
        && !value
            .chars()
            .any(|ch| matches!(ch, '\u{0000}'..='\u{001f}' | '\u{007f}'..='\u{009f}'))
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

fn get_provider_modes<F>(get: &F, name: &str, current: &[(String, String)]) -> Vec<(String, String)>
where
    F: Fn(&str) -> Option<String>,
{
    match get(name) {
        None => current.to_vec(),
        Some(value) => value
            .split(',')
            .filter_map(|entry| {
                let (provider, mode) = entry.split_once('=')?;
                let provider = provider.trim();
                let mode = mode.trim();
                if provider.is_empty() || mode.is_empty() {
                    None
                } else {
                    Some((provider.to_string(), mode.to_string()))
                }
            })
            .collect(),
    }
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
        .filter(|value: &i32| *value >= 0)
        .unwrap_or(default)
}

fn get_i64<F>(get: &F, name: &str, default: i64) -> i64
where
    F: Fn(&str) -> Option<String>,
{
    get(name)
        .and_then(|value| value.parse().ok())
        .filter(|value: &i64| *value >= 0)
        .unwrap_or(default)
}

fn get_timezone_offset_minutes<F>(get: &F, name: &str, default: Option<i16>) -> Option<i16>
where
    F: Fn(&str) -> Option<String>,
{
    get(name).map_or(default, |value| parse_timezone_offset_minutes(&value))
}

fn parse_timezone_offset_minutes(value: &str) -> Option<i16> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("utc") || value == "Z" || value == "+00:00" || value == "-00:00" {
        return Some(0);
    }
    let bytes = value.as_bytes();
    if bytes.len() != 6 || !matches!(bytes[0], b'+' | b'-') || bytes[3] != b':' {
        return None;
    }
    if !bytes[1..3].iter().all(u8::is_ascii_digit) || !bytes[4..6].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let hours: i16 = value[1..3].parse().ok()?;
    let minutes: i16 = value[4..6].parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    let total = hours.checked_mul(60)?.checked_add(minutes)?;
    if bytes[0] == b'-' {
        total.checked_neg()
    } else {
        Some(total)
    }
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

    #[test]
    fn degraded_cli_glyph_can_be_configured_from_kdl() {
        let mut kdl = BTreeMap::new();
        kdl.insert("degraded_cli_glyph".into(), "CLI".into());

        let config = RenderConfig::from_kdl_config(&kdl);

        assert_eq!(config.degraded_cli_glyph, "CLI");
    }

    #[test]
    fn error_glyph_can_be_configured_from_env_and_kdl_alias() {
        let mut env = BTreeMap::new();
        env.insert("SHOWY_QUOTA_ERROR_GLYPH".into(), "!!".into());
        assert_eq!(RenderConfig::from_env_map(&env).error_glyph, "!!");

        let mut kdl = BTreeMap::new();
        kdl.insert("error_glyph".into(), "?".into());
        assert_eq!(RenderConfig::from_kdl_config(&kdl).error_glyph, "?");
    }

    #[test]
    fn glyph_config_rejects_control_chars_and_long_values() {
        let mut kdl = BTreeMap::new();
        kdl.insert("stale_glyph".into(), "\u{1b}[31m!".into());
        kdl.insert("degraded_cli_glyph".into(), "abcdefghijklmnopq".into());
        kdl.insert("error_glyph".into(), "bad\n".into());

        let config = RenderConfig::from_kdl_config(&kdl);

        assert_eq!(config.stale_glyph, RenderConfig::default().stale_glyph);
        assert_eq!(
            config.degraded_cli_glyph,
            RenderConfig::default().degraded_cli_glyph
        );
        assert_eq!(config.error_glyph, RenderConfig::default().error_glyph);
    }

    #[test]
    fn cap_glyph_rejects_esc_byte() {
        let mut env = BTreeMap::new();
        env.insert("SHOWY_QUOTA_CAP_LEFT".into(), "\u{1b}[".into());
        env.insert("SHOWY_QUOTA_CAP_RIGHT".into(), "\u{80}".into());

        let config = RenderConfig::from_env_map(&env);

        assert_eq!(config.cap_left, RenderConfig::default().cap_left);
        assert_eq!(config.cap_right, RenderConfig::default().cap_right);
    }

    #[test]
    fn glyph_config_accepts_shell_valid_empty_and_sixteen_char_values() {
        let mut kdl = BTreeMap::new();
        kdl.insert("stale_glyph".into(), String::new());
        kdl.insert("degraded_cli_glyph".into(), "abcdefghijklmnop".into());
        kdl.insert("error_glyph".into(), "abcdefghijklmnop".into());

        let config = RenderConfig::from_kdl_config(&kdl);

        assert_eq!(config.stale_glyph, "");
        assert_eq!(config.degraded_cli_glyph, "abcdefghijklmnop");
        assert_eq!(config.error_glyph, "abcdefghijklmnop");
    }

    #[test]
    fn env_glyph_config_falls_back_on_invalid_values() {
        let key = "SHOWY_QUOTA_STALE_GLYPH";
        let previous = std::env::var(key).ok();
        std::env::set_var(key, "bad\n");
        let config = RenderConfig::from_env();
        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }

        assert_eq!(config.stale_glyph, RenderConfig::default().stale_glyph);
    }

    #[test]
    fn reset_description_timezone_offset_parses_from_kdl() {
        let mut kdl = BTreeMap::new();
        kdl.insert("reset_description_timezone_offset".into(), "-07:30".into());

        let config = RenderConfig::from_kdl_config(&kdl);

        assert_eq!(config.reset_description_timezone_offset_minutes, Some(-450));
    }

    #[test]
    fn invalid_reset_description_timezone_offset_is_ignored() {
        let mut kdl = BTreeMap::new();
        kdl.insert(
            "reset_description_timezone_offset".into(),
            "America/Los_Angeles".into(),
        );

        let config = RenderConfig::from_kdl_config(&kdl);

        assert_eq!(config.reset_description_timezone_offset_minutes, None);
    }

    #[test]
    fn parse_timezone_offset_minutes_handles_aliases_and_bounds() {
        assert_eq!(parse_timezone_offset_minutes("utc"), Some(0));
        assert_eq!(parse_timezone_offset_minutes("UTC"), Some(0));
        assert_eq!(parse_timezone_offset_minutes("Z"), Some(0));
        assert_eq!(parse_timezone_offset_minutes("+00:00"), Some(0));
        assert_eq!(parse_timezone_offset_minutes("-00:00"), Some(0));
        assert_eq!(parse_timezone_offset_minutes("+09:00"), Some(540));
        assert_eq!(parse_timezone_offset_minutes("-05:30"), Some(-330));
        // Out-of-range hours/minutes, missing separator, short/long, non-digit.
        assert_eq!(parse_timezone_offset_minutes("+25:00"), None);
        assert_eq!(parse_timezone_offset_minutes("+12:60"), None);
        assert_eq!(parse_timezone_offset_minutes("+1234"), None);
        assert_eq!(parse_timezone_offset_minutes("+5:00"), None);
        assert_eq!(parse_timezone_offset_minutes("0700"), None);
        assert_eq!(parse_timezone_offset_minutes("+ab:cd"), None);
        assert_eq!(parse_timezone_offset_minutes(""), None);
    }

    #[test]
    fn from_env_reads_environment_overrides() {
        // from_env mirrors from_kdl_config but sources SHOWY_QUOTA_* from the
        // process environment. Save/restore the knob so the shared process env
        // is left clean for other tests.
        let key = "SHOWY_QUOTA_GOOD_MIN_REMAINING";
        let previous = std::env::var(key).ok();
        std::env::set_var(key, "77");
        let from_env = RenderConfig::from_env();
        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
        assert_eq!(from_env.good_min_remaining, 77);

        // The KDL path applies the same key identically.
        let mut kdl = BTreeMap::new();
        kdl.insert(key.to_string(), "77".to_string());
        assert_eq!(RenderConfig::from_kdl_config(&kdl).good_min_remaining, 77);
    }

    #[test]
    fn negative_threshold_and_duration_overrides_fall_back_to_defaults() {
        let mut env = BTreeMap::new();
        let mut kdl = BTreeMap::new();
        for name in [
            "GOOD_MIN_REMAINING",
            "WARN_MIN_REMAINING",
            "TIME_WARN_MINUTES",
            "DIM_WINDOW_MINUTES",
        ] {
            env.insert(format!("SHOWY_QUOTA_{name}"), "-1".into());
            kdl.insert(name.to_ascii_lowercase(), "-1".into());
        }
        let defaults = RenderConfig::default();

        for config in [
            RenderConfig::from_env_map(&env),
            RenderConfig::from_kdl_config(&kdl),
        ] {
            assert_eq!(config.good_min_remaining, defaults.good_min_remaining);
            assert_eq!(config.warn_min_remaining, defaults.warn_min_remaining);
            assert_eq!(config.time_warn_minutes, defaults.time_warn_minutes);
            assert_eq!(config.dim_window_minutes, defaults.dim_window_minutes);
        }
    }

    #[test]
    fn inverted_thresholds_are_swapped_to_keep_warn_reachable() {
        let mut kdl = BTreeMap::new();
        kdl.insert("SHOWY_QUOTA_GOOD_MIN_REMAINING".into(), "15".into());
        kdl.insert("SHOWY_QUOTA_WARN_MIN_REMAINING".into(), "40".into());
        let config = RenderConfig::from_kdl_config(&kdl);
        assert_eq!(config.good_min_remaining, 40);
        assert_eq!(config.warn_min_remaining, 15);
    }

    #[test]
    fn from_env_map_reads_terminal_renderer_overrides() {
        let mut env = BTreeMap::new();
        env.insert("SHOWY_QUOTA_ZELLIJ_BAR_WIDTH".into(), "9".into());
        env.insert("SHOWY_QUOTA_TMUX_BAR_WIDTH".into(), "17".into());
        env.insert(
            "SHOWY_QUOTA_PROVIDER_MODES".into(),
            "codex=dual2,claude=mono4".into(),
        );
        env.insert("SHOWY_QUOTA_CAP_LEFT".into(), "#".into());
        env.insert("SHOWY_QUOTA_CAP_RIGHT".into(), String::new());

        let config = RenderConfig::from_env_map(&env);

        assert_eq!(config.zellij_bar_width, 9);
        assert_eq!(config.tmux_bar_width, Some(17));
        assert_eq!(
            config.provider_modes,
            vec![
                ("codex".to_string(), "dual2".to_string()),
                ("claude".to_string(), "mono4".to_string()),
            ]
        );
        assert_eq!(config.cap_left, "#");
        assert_eq!(config.cap_right, "");
    }
}
