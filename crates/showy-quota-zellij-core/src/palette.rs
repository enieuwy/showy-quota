use crate::config::RenderConfig;

/// Severity band for a remaining percentage. Public so surfaces that cannot
/// carry colour in their payload (a Herdr sidebar token strips control bytes,
/// for example) can name the band showy-quota itself chose and map it to a
/// colour on their own side, instead of re-deriving thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Good,
    Warn,
    Bad,
}

impl Severity {
    /// Stable lowercase identifier for serialised output.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Good => "good",
            Severity::Warn => "warn",
            Severity::Bad => "bad",
        }
    }
}

impl RenderConfig {
    fn color_key(&self, remaining: i32) -> Severity {
        self.severity(remaining)
    }

    /// Severity band for a remaining percentage, using the configured
    /// `good_min_remaining` / `warn_min_remaining` thresholds.
    pub fn severity(&self, remaining: i32) -> Severity {
        if remaining >= self.good_min_remaining {
            Severity::Good
        } else if remaining >= self.warn_min_remaining {
            Severity::Warn
        } else {
            Severity::Bad
        }
    }

    /// ANSI SGR color code (32 green / 33 yellow / 31 red) for a remaining
    /// percentage, using the SAME configured thresholds as `color_key`. Shared
    /// with the prompt renderer so shell prompts and multiplexer bars agree.
    pub fn severity_ansi_code(&self, remaining: i32) -> i32 {
        match self.color_key(remaining) {
            Severity::Good => 32,
            Severity::Warn => 33,
            Severity::Bad => 31,
        }
    }

    /// Color for a usage window: the severity palette, dimmed when the window
    /// is a long-horizon (weekly/monthly) cap rather than a live short tier.
    pub fn window_color(&self, remaining: i32, is_long: bool) -> String {
        self.severity_color(self.severity(remaining), is_long)
    }

    /// Color for an already-resolved band, so a surface handed a `Severity` can
    /// reproduce the exact hex showy-quota would have drawn without re-deriving
    /// thresholds or dim scaling.
    ///
    /// Always returns a valid 6-digit hex. Configured palette overrides are
    /// copied verbatim by config parsing, and this value is serialized into the
    /// `rows` transport instead of only reaching an ANSI escape through
    /// `hex_to_rgb`, so it has to degrade here too or a malformed override would
    /// ship a non-hex string to a consumer that cannot render it.
    pub fn severity_color(&self, severity: Severity, is_long: bool) -> String {
        let raw = if is_long {
            self.dim_palette(severity)
        } else {
            self.primary_palette(severity)
        };
        normalized_hex(&raw)
    }

    fn primary_palette(&self, severity: Severity) -> String {
        match severity {
            Severity::Good => self.palette_primary_good.clone(),
            Severity::Warn => self.palette_primary_warn.clone(),
            Severity::Bad => self.palette_primary_bad.clone(),
        }
    }

    /// Dimmed palette for long-horizon windows: explicit override when set,
    /// otherwise the primary palette scaled down by `palette_dim_scale`.
    fn dim_palette(&self, severity: Severity) -> String {
        self.dim_override(severity)
            .cloned()
            .unwrap_or_else(|| scale_hex(&self.primary_palette(severity), &self.palette_dim_scale))
    }

    fn dim_override(&self, severity: Severity) -> Option<&String> {
        match severity {
            Severity::Good => self.palette_dim_good.as_ref(),
            Severity::Warn => self.palette_dim_warn.as_ref(),
            Severity::Bad => self.palette_dim_bad.as_ref(),
        }
    }
}

/// Fallback colour for a hex string this parser cannot understand, matching
/// the shell side's `SHOWY_QUOTA_PALETTE_PRIMARY_UNKNOWN` default (`6c7086`)
/// so a malformed configured hex degrades to the same "unknown" colour on
/// both ends instead of silently rendering pure black.
const FALLBACK_RGB: (u8, u8, u8) = (0x6c, 0x70, 0x86);

/// Parse a `#`-optional 6-digit hex colour into `(r, g, b)`. The whole string
/// is validated before any channel is decoded, so a malformed value (wrong
/// length, non-hex characters, or a mix of both) always degrades to
/// `FALLBACK_RGB` in full — never a partial mix of real and fallback bytes.
pub fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return FALLBACK_RGB;
    }
    match (
        u8::from_str_radix(&hex[0..2], 16),
        u8::from_str_radix(&hex[2..4], 16),
        u8::from_str_radix(&hex[4..6], 16),
    ) {
        (Ok(r), Ok(g), Ok(b)) => (r, g, b),
        _ => FALLBACK_RGB,
    }
}

/// Canonical lowercase 6-digit hex (no `#`) for a configured colour, degrading a
/// value this parser cannot understand to `FALLBACK_RGB`. Use this wherever a
/// palette string is handed to a consumer as text rather than decoded into an
/// ANSI escape, so both surfaces degrade identically.
pub(crate) fn normalized_hex(hex: &str) -> String {
    let (r, g, b) = hex_to_rgb(hex);
    format!("{r:02x}{g:02x}{b:02x}")
}

fn scale_hex(hex: &str, factor: &str) -> String {
    let (factor_num, factor_den) = parse_factor(factor).unwrap_or((1, 1));
    let (r, g, b) = hex_to_rgb(hex);
    format!(
        "{:02x}{:02x}{:02x}",
        scale_component(r, factor_num, factor_den),
        scale_component(g, factor_num, factor_den),
        scale_component(b, factor_num, factor_den)
    )
}

fn parse_factor(raw: &str) -> Option<(u64, u64)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some((int, frac)) = raw.split_once('.') {
        if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let int = if int.is_empty() { "0" } else { int };
        let num = format!("{}{}", int, frac).parse().ok()?;
        let den = 10_u64.checked_pow(frac.len() as u32)?;
        Some((num, den))
    } else if raw.bytes().all(|b| b.is_ascii_digit()) {
        Some((raw.parse().ok()?, 1))
    } else {
        None
    }
}

fn scale_component(value: u8, factor_num: u64, factor_den: u64) -> u8 {
    // Widen to u128 before multiplying: a pathological config scale (e.g.
    // factor_num near u64::MAX) would otherwise overflow `value as u64 *
    // factor_num`, panicking in debug and wrapping in release.
    let den = u128::from(factor_den.max(1));
    ((u128::from(value) * u128::from(factor_num)) / den).min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dim_scale_matches_shell_integer_math() {
        assert_eq!(scale_hex("25be6a", "0.55"), "14683a");
        assert_eq!(scale_hex("f0af00", "0.55"), "846000");
        assert_eq!(scale_hex("ee5396", "0.55"), "822d52");
    }

    #[test]
    fn palette_helpers_accept_leading_hash() {
        assert_eq!(hex_to_rgb("#25be6a"), (0x25, 0xbe, 0x6a));
        assert_eq!(scale_hex("#25be6a", "0.55"), "14683a");
    }

    #[test]
    fn hex_to_rgb_parses_valid_six_digit_hex() {
        assert_eq!(hex_to_rgb("25be6a"), (0x25, 0xbe, 0x6a));
    }

    #[test]
    fn hex_to_rgb_degrades_to_fallback_on_malformed_input() {
        // showy-quota-00685b1ec34e8659: malformed hex must never silently
        // become pure black (the old per-channel `unwrap_or(0)` behaviour)
        // and a partially-valid string must never mix real and fallback
        // bytes — every invalid case below returns the fallback triple.
        const FALLBACK: (u8, u8, u8) = (0x6c, 0x70, 0x86);
        assert_eq!(hex_to_rgb("1234f"), FALLBACK, "5-digit hex");
        assert_eq!(hex_to_rgb("1234567"), FALLBACK, "7-digit hex");
        assert_eq!(hex_to_rgb("notahex"), FALLBACK, "non-hex string");
        assert_eq!(hex_to_rgb(""), FALLBACK, "empty string");
        assert_eq!(hex_to_rgb("ff00zz"), FALLBACK, "partially-valid string");
    }

    #[test]
    fn severity_color_always_yields_a_valid_hex_for_the_rows_transport() {
        // `severity_color` is serialized as text into the rows transport, not
        // only decoded into an ANSI escape, so a malformed configured override
        // must degrade here rather than shipping a non-hex string downstream.
        let mut config = RenderConfig {
            palette_primary_good: "garbage".into(),
            palette_primary_warn: "#AABBCC".into(),
            ..RenderConfig::default()
        };
        config.palette_dim_good = Some("nothex".into());

        for (severity, is_long) in [
            (Severity::Good, false),
            (Severity::Good, true),
            (Severity::Warn, false),
            (Severity::Bad, false),
        ] {
            let hex = config.severity_color(severity, is_long);
            assert_eq!(hex.len(), 6, "{severity:?}/{is_long} -> {hex}");
            assert!(
                hex.bytes().all(|b| b.is_ascii_hexdigit()),
                "{severity:?}/{is_long} -> {hex}"
            );
        }

        // A malformed value degrades to the shared fallback, and a valid
        // `#`-prefixed override is canonicalised rather than passed through.
        assert_eq!(config.severity_color(Severity::Good, false), "6c7086");
        assert_eq!(config.severity_color(Severity::Good, true), "6c7086");
        assert_eq!(config.severity_color(Severity::Warn, false), "aabbcc");
    }

    #[test]
    fn scale_hex_clamps_huge_factor_without_overflow() {
        // A pathological integer scale parses to (u64::MAX, 1); the widened
        // multiply must clamp each channel to 0xff instead of overflowing.
        assert_eq!(scale_hex("25be6a", "18446744073709551615"), "ffffff");
        // A zero channel stays zero regardless of the factor.
        assert_eq!(scale_component(0, u64::MAX, 1), 0);
    }

    #[test]
    fn severity_bands_follow_configured_thresholds_and_match_window_color() {
        let config = RenderConfig::default();

        assert_eq!(config.severity(config.good_min_remaining), Severity::Good);
        assert_eq!(
            config.severity(config.good_min_remaining - 1),
            Severity::Warn
        );
        assert_eq!(config.severity(config.warn_min_remaining), Severity::Warn);
        assert_eq!(
            config.severity(config.warn_min_remaining - 1),
            Severity::Bad
        );

        // The exposed band must name the same choice `window_color` makes, so a
        // surface that maps bands to colours itself cannot drift from the strip.
        for remaining in [100, 40, 39, 15, 14, 0] {
            let severity = config.severity(remaining);
            assert_eq!(
                config.window_color(remaining, false),
                config.primary_palette(severity),
                "bright {remaining}"
            );
            assert_eq!(
                config.window_color(remaining, true),
                config.dim_palette(severity),
                "dim {remaining}"
            );
        }
    }

    #[test]
    fn severity_identifiers_are_stable() {
        assert_eq!(Severity::Good.as_str(), "good");
        assert_eq!(Severity::Warn.as_str(), "warn");
        assert_eq!(Severity::Bad.as_str(), "bad");
    }
}
