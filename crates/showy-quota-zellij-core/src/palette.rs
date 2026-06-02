use crate::config::RenderConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Primary,
    Secondary,
    Tertiary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Good,
    Warn,
    Bad,
    Unknown,
}

impl RenderConfig {
    pub fn color_key(&self, remaining: i32) -> Severity {
        if remaining >= self.good_min_remaining {
            Severity::Good
        } else if remaining >= self.warn_min_remaining {
            Severity::Warn
        } else {
            Severity::Bad
        }
    }

    pub fn role_color(&self, role: Role, remaining: i32) -> String {
        self.role_palette(role, self.color_key(remaining))
    }

    pub fn role_palette(&self, role: Role, severity: Severity) -> String {
        match role {
            Role::Primary => self.primary_palette(severity),
            Role::Secondary => self
                .secondary_override(severity)
                .cloned()
                .unwrap_or_else(|| {
                    scale_hex(
                        &self.primary_palette(severity),
                        &self.palette_secondary_scale,
                    )
                }),
            Role::Tertiary => self
                .tertiary_override(severity)
                .cloned()
                .unwrap_or_else(|| {
                    scale_hex(
                        &self.primary_palette(severity),
                        &self.palette_tertiary_scale,
                    )
                }),
        }
    }

    pub fn primary_palette(&self, severity: Severity) -> String {
        match severity {
            Severity::Good => self.palette_primary_good.clone(),
            Severity::Warn => self.palette_primary_warn.clone(),
            Severity::Bad => self.palette_primary_bad.clone(),
            Severity::Unknown => self.palette_primary_unknown.clone(),
        }
    }

    fn secondary_override(&self, severity: Severity) -> Option<&String> {
        match severity {
            Severity::Good => self.palette_secondary_good.as_ref(),
            Severity::Warn => self.palette_secondary_warn.as_ref(),
            Severity::Bad => self.palette_secondary_bad.as_ref(),
            Severity::Unknown => self.palette_secondary_unknown.as_ref(),
        }
    }

    fn tertiary_override(&self, severity: Severity) -> Option<&String> {
        match severity {
            Severity::Good => self.palette_tertiary_good.as_ref(),
            Severity::Warn => self.palette_tertiary_warn.as_ref(),
            Severity::Bad => self.palette_tertiary_bad.as_ref(),
            Severity::Unknown => self.palette_tertiary_unknown.as_ref(),
        }
    }
}

pub fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return (0, 0, 0);
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    (r, g, b)
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
    let den = factor_den.max(1);
    ((value as u64 * factor_num) / den).min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_secondary_scale_matches_shell_integer_math() {
        assert_eq!(scale_hex("25be6a", "0.55"), "14683a");
        assert_eq!(scale_hex("f0af00", "0.55"), "846000");
        assert_eq!(scale_hex("ee5396", "0.55"), "822d52");
    }

    #[test]
    fn palette_helpers_accept_leading_hash() {
        assert_eq!(hex_to_rgb("#25be6a"), (0x25, 0xbe, 0x6a));
        assert_eq!(scale_hex("#25be6a", "0.55"), "14683a");
    }
}
