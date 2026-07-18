use crate::config::RenderConfig;
use crate::metrics::{provider_metrics, ProviderMetric, WindowMetric};
use crate::render::{format_countdown, provider_sigil, RenderError};

const UNKNOWN_SEGMENT: &str = "AI ?";

#[derive(Debug, Clone, Copy)]
pub struct PromptOptions<'a> {
    pub provider_filter: &'a [String],
    pub ansi: bool,
    pub stale: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PromptCandidate<'a> {
    provider: &'a str,
    used: i32,
    remaining: i32,
    minutes: Option<i64>,
}

pub fn emit_prompt_segment(
    payload: &[u8],
    config: &RenderConfig,
    now_epoch: i64,
    options: PromptOptions<'_>,
) -> Result<String, RenderError> {
    let metrics = provider_metrics(payload, config, now_epoch)?;
    Ok(render_prompt_segment(&metrics, config, options))
}

fn render_prompt_segment(
    metrics: &[ProviderMetric],
    config: &RenderConfig,
    options: PromptOptions<'_>,
) -> String {
    let Some(candidate) = select_candidate(metrics, options.provider_filter) else {
        return String::from(UNKNOWN_SEGMENT);
    };

    let mut segment = format!("{} {}%", provider_sigil(candidate.provider), candidate.used);
    if let Some(minutes) = candidate.minutes {
        segment.push(' ');
        segment.push_str(&format_countdown(minutes));
    }

    let mut out = if options.ansi && std::env::var_os("NO_COLOR").is_none() {
        format!(
            "\u{1b}[{}m{}\u{1b}[0m",
            config.severity_ansi_code(candidate.remaining),
            segment
        )
    } else {
        segment
    };

    if options.stale {
        out.push(' ');
        out.push_str(&config.stale_glyph);
    }
    out
}

fn select_candidate<'a>(
    metrics: &'a [ProviderMetric],
    provider_filter: &[String],
) -> Option<PromptCandidate<'a>> {
    let mut selected = None;
    for metric in metrics {
        if !provider_filter.is_empty()
            && !provider_filter
                .iter()
                .any(|provider| provider == &metric.provider)
        {
            continue;
        }
        for window in [
            metric.windows.primary.as_ref(),
            metric.windows.secondary.as_ref(),
            metric.windows.tertiary.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let candidate = candidate(metric.provider.as_str(), window);
            if selected
                .is_none_or(|current: PromptCandidate<'_>| candidate.remaining < current.remaining)
            {
                selected = Some(candidate);
            }
        }
    }
    selected
}

fn candidate<'a>(provider: &'a str, window: &WindowMetric) -> PromptCandidate<'a> {
    PromptCandidate {
        provider,
        used: window.used_percent,
        remaining: window.remaining_percent,
        minutes: window.minutes_until_reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 4_070_908_800;

    fn config() -> RenderConfig {
        RenderConfig {
            provider_order: Vec::new(),
            reset_description_timezone_offset_minutes: Some(0),
            ..RenderConfig::default()
        }
    }

    fn prompt(payload: &[u8], options: PromptOptions<'_>) -> String {
        emit_prompt_segment(payload, &config(), NOW, options).expect("prompt")
    }

    fn options<'a>(provider_filter: &'a [String]) -> PromptOptions<'a> {
        PromptOptions {
            provider_filter,
            ansi: false,
            stale: false,
        }
    }

    #[test]
    fn prompt_selects_lowest_remaining_and_preserves_tie_order() {
        let output = prompt(
            br#"[
                {"provider":"codex","usage":{"primary":{"usedPercent":40,"resetsAt":"2099-01-01T02:00:00Z"},"secondary":{"usedPercent":80,"resetsAt":"2099-01-01T01:00:00Z"}}},
                {"provider":"claude","usage":{"primary":{"usedPercent":80,"resetsAt":"2099-01-01T03:00:00Z"}}}
            ]"#,
            options(&[]),
        );
        assert_eq!(output, "CX 80% 1h");
    }

    #[test]
    fn prompt_filters_requested_providers() {
        let requested = vec![String::from("claude")];
        let output = prompt(
            br#"[
                {"provider":"codex","usage":{"primary":{"usedPercent":99,"resetsAt":"2099-01-01T01:00:00Z"}}},
                {"provider":"claude","usage":{"primary":{"usedPercent":10,"resetsAt":"2099-01-01T02:00:00Z"}}}
            ]"#,
            options(&requested),
        );
        assert_eq!(output, "CL 10% 2h");
    }

    #[test]
    fn prompt_omits_countdown_when_minutes_are_null() {
        let output = prompt(
            br#"[{"provider":"codex","usage":{"primary":{"usedPercent":42}}}]"#,
            options(&[]),
        );
        assert_eq!(output, "CX 42%");
    }

    #[test]
    fn prompt_empty_metrics_are_unknown() {
        assert_eq!(prompt(br#"[]"#, options(&[])), "AI ?");
        assert_eq!(
            prompt(
                br#"[{"provider":"codex","usage":{"primary":{"usedPercent":null}}}]"#,
                options(&[])
            ),
            "AI ?"
        );
    }

    #[test]
    fn countdown_boundaries_match_shell() {
        assert_eq!(format_countdown(0), "now");
        assert_eq!(format_countdown(59), "59m");
        assert_eq!(format_countdown(60), "1h");
        assert_eq!(format_countdown(90), "1:30");
        assert_eq!(format_countdown(1439), "23:59");
        assert_eq!(format_countdown(1440), "1d");
        assert_eq!(format_countdown(20_159), "13d");
        assert_eq!(format_countdown(20_160), "2w");
    }

    #[test]
    fn severity_codes_track_configured_thresholds() {
        // Defaults: good_min_remaining = 40, warn_min_remaining = 15. The prompt
        // shares these with the palette, so colors match the multiplexer bar.
        let config = config();
        assert_eq!(config.severity_ansi_code(40), 32);
        assert_eq!(config.severity_ansi_code(39), 33);
        assert_eq!(config.severity_ansi_code(15), 33);
        assert_eq!(config.severity_ansi_code(14), 31);
    }

    #[test]
    fn stale_glyph_appends_after_segment() {
        let mut config = config();
        config.stale_glyph = "STALE".into();
        let output = emit_prompt_segment(
            br#"[{"provider":"codex","usage":{"primary":{"usedPercent":42}}}]"#,
            &config,
            NOW,
            PromptOptions {
                provider_filter: &[],
                ansi: false,
                stale: true,
            },
        )
        .expect("prompt");
        assert_eq!(output, "CX 42% STALE");
    }
}
