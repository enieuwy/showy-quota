use std::collections::BTreeMap;

const GLYPH_MAX_CHARS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ThresholdPolicy {
    pub good: i32,
    pub warn: i32,
    pub time: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThresholdOverride {
    pub good: Option<i32>,
    pub warn: Option<i32>,
    pub time: Option<i64>,
}

impl ThresholdPolicy {
    fn apply(&mut self, value: &ThresholdOverride) {
        self.good = value.good.unwrap_or(self.good);
        self.warn = value.warn.unwrap_or(self.warn);
        self.time = value.time.unwrap_or(self.time);
        if self.good < self.warn {
            std::mem::swap(&mut self.good, &mut self.warn);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderConfig {
    pub providers: Vec<String>,
    pub providers_exclude: Vec<String>,
    pub provider_order: Vec<String>,
    pub include_status: bool,
    /// Optional strip suffix; never changes the stale marker.
    pub freshness: String,
    pub severity_glyphs: bool,

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
    pub provider_thresholds: BTreeMap<String, ThresholdOverride>,
    pub window_thresholds: BTreeMap<String, ThresholdOverride>,
    pub windows: Vec<String>,
    pub window_mode: String,
    pub compact_provider_count: usize,
    pub width_budget: usize,
    pub compact_order: String,
    pub theme: String,
    pub theme_error: Option<String>,
    pub dim_window_minutes: i64,

    pub zellij_bar_width: usize,
    pub tmux_bar_width: Option<usize>,
    /// Bar body width for the vertical view. Wider than the status-bar strip
    /// because that surface pays for width in scarce status-line columns while
    /// the vertical view owns the pane; still narrow enough that a full line
    /// fits an SSH session on a phone without wrapping.
    pub vertical_bar_width: usize,
    /// `provider` (default) keeps CodexBar's provider blocks; `urgency` flattens
    /// them so the window closest to running out is the first line.
    pub vertical_sort: String,
    /// Append each window's local reset clock (`11:54`) after its countdown. On
    /// by default: rows that all read `1d` say nothing about *when*, and the six
    /// columns it costs are columns the vertical view has - a full line is 44,
    /// still inside an SSH session on a phone.
    pub vertical_reset_clock: bool,
    pub terminal_bar_mode: String,
    pub provider_modes: Vec<(String, String)>,
    pub mono_color_mode: String,
    pub mono_markers: Vec<String>,
    pub cap_left: String,
    pub cap_right: String,
}

include!("config_generated.rs");
include!("themes_generated.rs");

impl RenderConfig {
    pub fn from_env() -> Self {
        let env: BTreeMap<String, String> = std::env::vars().collect();
        Self::from_env_map(&env)
    }

    pub fn from_env_map(env: &BTreeMap<String, String>) -> Self {
        let mut config = Self::default();
        config.apply_getter(&|name| env.get(name).cloned());
        config
    }

    pub fn from_kdl_config(kdl: &BTreeMap<String, String>) -> Self {
        let mut config = Self::default();
        config.apply_getter(&|name| get_from_kdl(kdl, name));
        config
    }

    fn apply_getter(&mut self, get: &dyn Fn(&str) -> Option<String>) {
        let theme = get("SHOWY_QUOTA_THEME").filter(|name| !name.is_empty());
        if let Some(name) = &theme {
            self.theme.clone_from(name);
            if !known_theme(name) {
                self.theme_error = Some(format!("unknown theme: {name}"));
            }
        }
        self.apply_manifest(&|key| {
            get(key).or_else(|| {
                theme
                    .as_deref()
                    .and_then(|name| theme_value(name, key))
                    .map(str::to_owned)
            })
        });
        if self.good_min_remaining < self.warn_min_remaining {
            std::mem::swap(&mut self.good_min_remaining, &mut self.warn_min_remaining);
        }
        if self.windows.len() == 1
            && matches!(self.windows[0].as_str(), "session-only" | "weekly" | "all")
        {
            self.window_mode.clone_from(&self.windows[0]);
        }
    }

    /// Explicit per-provider terminal body override, if any.
    pub fn mode_for(&self, provider: &str) -> Option<&str> {
        self.provider_modes
            .iter()
            .find(|(name, _)| name == provider)
            .map(|(_, mode)| mode.as_str())
    }

    pub fn policy(&self, provider: &str, slot: &str, minutes: Option<i64>) -> ThresholdPolicy {
        let mut policy = ThresholdPolicy {
            good: self.good_min_remaining,
            warn: self.warn_min_remaining,
            time: self.time_warn_minutes,
        };
        if let Some(value) = self.provider_thresholds.get(provider) {
            policy.apply(value);
        }
        if self.window_thresholds.is_empty() {
            return policy;
        }
        let horizon = if minutes.is_some_and(|m| m >= self.dim_window_minutes) {
            "cap"
        } else {
            "live"
        };
        let mut scoped = String::with_capacity(provider.len() + 1 + horizon.len().max(slot.len()));
        scoped.push_str(provider);
        scoped.push('.');
        let prefix_len = scoped.len();
        for key in [horizon, slot] {
            if let Some(value) = self.window_thresholds.get(key) {
                policy.apply(value);
            }
            scoped.truncate(prefix_len);
            scoped.push_str(key);
            if let Some(value) = self.window_thresholds.get(&scoped) {
                policy.apply(value);
            }
        }
        policy
    }

    pub fn window_enabled(&self, slot: &str, minutes: Option<i64>) -> bool {
        match self.window_mode.as_str() {
            "session-only" => minutes.map_or(slot == "primary", |m| m < self.dim_window_minutes),
            "weekly" => minutes.is_some_and(|m| m >= self.dim_window_minutes),
            _ => self
                .windows
                .iter()
                .any(|value| value == slot || value == "all"),
        }
    }

    /// Apply visibility before any layout, countdown, marker or severity decision.
    pub(crate) fn filter_windows(&self, records: &mut Vec<crate::codexbar::ProviderRecord>) {
        if self.window_mode == "all"
            && ["primary", "secondary", "tertiary"].iter().all(|slot| {
                self.windows
                    .iter()
                    .any(|value| value == slot || value == "all")
            })
        {
            return;
        }
        records.retain_mut(|record| {
            let originally_renderable = crate::codexbar::is_renderable(record);
            if crate::codexbar::is_errored(record) {
                return true;
            }
            let Some(usage) = record.usage.as_mut() else {
                return true;
            };
            for (slot, window) in [
                ("primary", &mut usage.primary),
                ("secondary", &mut usage.secondary),
                ("tertiary", &mut usage.tertiary),
            ] {
                if !self.window_enabled(slot, window.as_ref().and_then(|w| w.window_minutes())) {
                    *window = None;
                }
            }
            // Keep each family's identity before hiding its horizons. Removing
            // the named entry would renumber pools and merge adjacent families.
            for named in &mut usage.extra_rate_windows {
                let minutes = named.window.as_ref().and_then(|w| w.window_minutes());
                let slot = if minutes.is_some_and(|m| m >= self.dim_window_minutes) {
                    "secondary"
                } else {
                    "primary"
                };
                if !self.window_enabled(slot, minutes) {
                    named.window = None;
                }
            }
            // Preserve a renderable provider whose selected horizon exists only
            // in a pool. Do not make an originally extras-only payload renderable.
            if originally_renderable
                && usage.primary.is_none()
                && usage.secondary.is_none()
                && usage.tertiary.is_none()
            {
                if let Some(window) = usage
                    .extra_rate_windows
                    .iter()
                    .filter(|named| named.usage_known != Some(false))
                    .filter_map(|named| named.window.as_ref())
                    .find(|window| window.used_percent.is_some_and(f64::is_finite))
                {
                    if window
                        .window_minutes()
                        .is_some_and(|m| m >= self.dim_window_minutes)
                    {
                        usage.secondary = Some(window.clone());
                    } else {
                        usage.primary = Some(window.clone());
                    }
                }
            }
            usage.primary.is_some()
                || usage.secondary.is_some()
                || usage.tertiary.is_some()
                || usage
                    .extra_rate_windows
                    .iter()
                    .any(|named| named.window.is_some())
        });
    }

    pub(crate) fn window_policy(
        &self,
        record: &crate::codexbar::ProviderRecord,
        window: Option<&crate::codexbar::UsageWindow>,
    ) -> ThresholdPolicy {
        let slot = record
            .usage
            .as_ref()
            .and_then(|usage| {
                [
                    ("primary", usage.primary.as_ref()),
                    ("secondary", usage.secondary.as_ref()),
                    ("tertiary", usage.tertiary.as_ref()),
                ]
                .into_iter()
                .find_map(|(slot, candidate)| match (candidate, window) {
                    (Some(a), Some(b)) if std::ptr::eq(a, b) => Some(slot),
                    _ => None,
                })
            })
            .unwrap_or("");
        self.policy(
            &record.provider,
            slot,
            window.and_then(|w| w.window_minutes()),
        )
    }
}

/// Semicolons separate providers; commas separate fields. A new `name=`
/// also starts a provider, preserving the existing provider-map CSV idiom.
fn parse_thresholds(value: &str) -> BTreeMap<String, ThresholdOverride> {
    let mut result = BTreeMap::new();
    let mut scope = "";
    for token in value.split([';', ',']) {
        let token = token.trim();
        let field = if let Some((name, field)) = token.split_once('=') {
            scope = name.trim();
            field
        } else {
            token
        };
        if scope.is_empty() {
            continue;
        }
        let Some((key, raw)) = field.split_once(':') else {
            continue;
        };
        let entry: &mut ThresholdOverride = result.entry(scope.to_owned()).or_default();
        match key.trim() {
            "good" => {
                entry.good = raw
                    .trim()
                    .parse::<i32>()
                    .ok()
                    .filter(|n| (0..=100).contains(n))
            }
            "warn" => {
                entry.warn = raw
                    .trim()
                    .parse::<i32>()
                    .ok()
                    .filter(|n| (0..=100).contains(n))
            }
            "time" => entry.time = raw.trim().parse::<i64>().ok().filter(|n| *n >= 0),
            _ => {}
        }
    }
    result
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

/// Canonical boolean parse shared with the shell (`showy_quota_bool` in
/// `lib/common.sh`) and `parse_bool` in `crates/showy-quota-zellij/src/main.rs`:
/// trimmed, case-insensitive `1|true|yes|on` -> true, `0|false|no|off` ->
/// false, anything else (including unset) -> `default`.
fn get_bool<F>(get: &F, name: &str, default: bool) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    match get(name)
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("1") | Some("true") | Some("yes") | Some("on") => true,
        Some("0") | Some("false") | Some("no") | Some("off") => false,
        _ => default,
    }
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

    #[test]
    fn get_bool_accepts_canonical_true_and_false_spellings() {
        let get = |value: &'static str| move |name: &str| (name == "V").then(|| value.to_string());

        for value in ["1", "true", "TRUE", "yes", "YES", "on", " on "] {
            assert!(
                get_bool(&get(value), "V", false),
                "{value:?} should be true"
            );
        }
        for value in ["0", "false", "FALSE", "no", "NO", "off", " off "] {
            assert!(
                !get_bool(&get(value), "V", true),
                "{value:?} should be false"
            );
        }
    }

    #[test]
    fn get_bool_is_case_and_whitespace_insensitive() {
        let get = |value: &'static str| move |name: &str| (name == "V").then(|| value.to_string());

        assert!(get_bool(&get("  Yes\t"), "V", false));
        assert!(!get_bool(&get("\tOFF  "), "V", true));
    }

    #[test]
    fn get_bool_falls_back_to_default_on_unrecognized_or_missing_value() {
        let get = |value: &'static str| move |name: &str| (name == "V").then(|| value.to_string());
        let missing = |_: &str| None;

        assert!(get_bool(&get("garbage"), "V", true));
        assert!(!get_bool(&get("garbage"), "V", false));
        assert!(get_bool(&missing, "V", true));
        assert!(!get_bool(&missing, "V", false));
    }
    #[test]
    fn freshness_and_severity_glyphs_accept_env_and_kdl() {
        let env = BTreeMap::from([
            ("SHOWY_QUOTA_FRESHNESS".into(), " AGE+SOURCE ".into()),
            ("SHOWY_QUOTA_SEVERITY_GLYPHS".into(), "on".into()),
        ]);
        let config = RenderConfig::from_env_map(&env);
        assert_eq!(config.freshness, "age+source");
        assert!(config.severity_glyphs);

        let kdl = BTreeMap::from([
            ("freshness".into(), "source".into()),
            ("severity_glyphs".into(), "true".into()),
        ]);
        let config = RenderConfig::from_kdl_config(&kdl);
        assert_eq!(config.freshness, "source");
        assert!(config.severity_glyphs);
        assert_eq!(RenderConfig::default().freshness, "off");
        assert!(!RenderConfig::default().severity_glyphs);
        let invalid = BTreeMap::from([("SHOWY_QUOTA_FRESHNESS".into(), "recent".into())]);
        assert_eq!(RenderConfig::from_env_map(&invalid).freshness, "off");
    }
}
