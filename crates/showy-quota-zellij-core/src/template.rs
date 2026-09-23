//! Plain-text quota templates. A template expands once per provider, using
//! that provider's window with the lowest remaining percentage.
use crate::config::RenderConfig;
use crate::metrics::{provider_metrics, ProviderMetric, WindowMetric};
use crate::render::{format_countdown, provider_sigil, RenderError};
use crate::sketchybar::sanitize_field;

pub const DEFAULT_PROMPT_FORMAT: &str = "{sigil} {used}% {countdown}{stale}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateScope {
    PerProvider,
    Worst,
}

#[derive(Debug, Clone, Copy)]
enum Field {
    Provider,
    Sigil,
    Used,
    Remaining,
    Countdown,
    Class,
    Window,
    Stale,
}

enum Part<'a> {
    Literal(&'a str),
    Brace(char),
    Field(Field),
}

pub struct Template<'a> {
    parts: Vec<Part<'a>>,
}

impl<'a> Template<'a> {
    /// Parse before cache access so invalid formats fail even with no cache.
    pub fn parse(spec: &'a str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut chars = spec.char_indices().peekable();
        let mut literal_start = 0;
        while let Some((at, ch)) = chars.next() {
            if ch != '{' && ch != '}' {
                continue;
            }
            if chars.peek().is_some_and(|(_, next)| *next == ch) {
                if literal_start < at {
                    parts.push(Part::Literal(&spec[literal_start..at]));
                }
                chars.next();
                parts.push(Part::Brace(ch));
                literal_start = at + 2;
                continue;
            }
            if ch == '}' {
                return Err(String::from("unescaped closing brace in template"));
            }
            if literal_start < at {
                parts.push(Part::Literal(&spec[literal_start..at]));
            }
            let name_start = at + 1;
            let end = loop {
                match chars.next() {
                    Some((end, '}')) => break end,
                    Some((_, '{')) => return Err(String::from("unclosed placeholder in template")),
                    Some(_) => (),
                    None => return Err(String::from("unclosed placeholder in template")),
                }
            };
            let name = &spec[name_start..end];
            let field = match name {
                "provider" => Field::Provider,
                "sigil" => Field::Sigil,
                "used" => Field::Used,
                "remaining" => Field::Remaining,
                "countdown" => Field::Countdown,
                "class" => Field::Class,
                "window" => Field::Window,
                "stale" => Field::Stale,
                _ => return Err(format!("unknown template placeholder: {name}")),
            };
            parts.push(Part::Field(field));
            literal_start = end + 1;
        }
        if literal_start < spec.len() {
            parts.push(Part::Literal(&spec[literal_start..]));
        }
        Ok(Self { parts })
    }

    pub fn render(
        &self,
        payload: &[u8],
        config: &RenderConfig,
        now_epoch: i64,
        join: &str,
        stale: bool,
        scope: TemplateScope,
    ) -> Result<String, RenderError> {
        let metrics = provider_metrics(payload, config, now_epoch)?;
        Ok(self.render_metrics(&metrics, config, join, stale, scope))
    }

    pub(crate) fn render_metrics(
        &self,
        metrics: &[ProviderMetric],
        config: &RenderConfig,
        join: &str,
        stale: bool,
        scope: TemplateScope,
    ) -> String {
        let candidates = metrics.iter().filter_map(worst_window);
        let mut output = String::new();
        match scope {
            TemplateScope::PerProvider => {
                let mut first = true;
                for candidate in candidates {
                    if !first {
                        output.push_str(join);
                    }
                    self.expand(&candidate, config, stale, &mut output);
                    first = false;
                }
            }
            TemplateScope::Worst => {
                if let Some(candidate) = candidates.min_by_key(|candidate| candidate.remaining) {
                    self.expand(&candidate, config, stale, &mut output);
                }
            }
        }
        output
    }

    fn expand(
        &self,
        candidate: &Candidate<'_>,
        config: &RenderConfig,
        stale: bool,
        output: &mut String,
    ) {
        let start = output.len();
        for part in &self.parts {
            match part {
                Part::Literal(value) => output.push_str(value),
                Part::Brace(value) => output.push(*value),
                Part::Field(field) => match field {
                    Field::Provider => push_payload_text(output, candidate.provider),
                    Field::Sigil => push_payload_text(output, &provider_sigil(candidate.provider)),
                    Field::Used => output.push_str(&candidate.used.to_string()),
                    Field::Remaining => output.push_str(&candidate.remaining.to_string()),
                    Field::Countdown => {
                        if let Some(minutes) = candidate.minutes {
                            output.push_str(&format_countdown(minutes));
                        }
                    }
                    Field::Class => output.push_str(config.severity(candidate.remaining).as_str()),
                    Field::Window => push_payload_text(output, candidate.window),
                    Field::Stale => {
                        if stale {
                            while output.len() > start && output.ends_with(' ') {
                                output.pop();
                            }
                            output.push(' ');
                            output.push_str(&config.stale_glyph);
                        }
                    }
                },
            }
        }
        // A missing reset should not leave a dangling space in the default
        // prompt. Trimming is local to each expansion, not the join separator.
        let trimmed = output[start..].trim_end_matches(' ').len();
        output.truncate(start + trimmed);
    }
}

fn push_payload_text(output: &mut String, value: &str) {
    if value.chars().any(char::is_control) {
        output.push_str(&sanitize_field(value));
    } else {
        output.push_str(value);
    }
}

struct Candidate<'a> {
    provider: &'a str,
    window: &'a str,
    used: i32,
    remaining: i32,
    minutes: Option<i64>,
}

fn worst_window(metric: &ProviderMetric) -> Option<Candidate<'_>> {
    let mut selected: Option<Candidate<'_>> = None;
    for (name, window) in [
        ("primary", metric.windows.primary.as_ref()),
        ("secondary", metric.windows.secondary.as_ref()),
        ("tertiary", metric.windows.tertiary.as_ref()),
    ] {
        if let Some(window) = window {
            select(&mut selected, candidate(&metric.provider, name, window));
        }
    }
    for extra in metric
        .extra_rate_windows
        .iter()
        .filter(|extra| extra.usage_known)
    {
        if let (Some(used), Some(remaining)) = (extra.used_percent, extra.remaining_percent) {
            select(
                &mut selected,
                Candidate {
                    provider: &metric.provider,
                    window: extra.title.as_deref().unwrap_or("extra"),
                    used,
                    remaining,
                    minutes: extra.minutes_until_reset,
                },
            );
        }
    }
    selected
}

fn select<'a>(selected: &mut Option<Candidate<'a>>, next: Candidate<'a>) {
    if selected
        .as_ref()
        .is_none_or(|current| next.remaining < current.remaining)
    {
        *selected = Some(next);
    }
}

fn candidate<'a>(provider: &'a str, name: &'a str, window: &WindowMetric) -> Candidate<'a> {
    Candidate {
        provider,
        window: name,
        used: window.used_percent,
        remaining: window.remaining_percent,
        minutes: window.minutes_until_reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::{emit_prompt_segment, PromptOptions};
    const NOW: i64 = 4_070_908_800;
    const PAYLOAD: &[u8] = br#"[
        {"provider":"codex","usage":{"primary":{"usedPercent":40,"resetsAt":"2099-01-01T02:00:00Z"},"secondary":{"usedPercent":80,"resetsAt":"2099-01-01T01:00:00Z"}}},
        {"provider":"claude","usage":{"primary":{"usedPercent":10,"resetsAt":"2099-01-01T03:00:00Z"}}}
    ]"#;

    fn config() -> RenderConfig {
        RenderConfig {
            reset_description_timezone_offset_minutes: Some(0),
            ..RenderConfig::default()
        }
    }

    #[test]
    fn default_matches_existing_prompt_for_reset_absence_and_staleness() {
        let config = config();
        let template = Template::parse(DEFAULT_PROMPT_FORMAT).unwrap();
        for payload in [
            PAYLOAD,
            include_bytes!("../../../test/fixtures/codexbar-mixed.json").as_slice(),
            br#"[{"provider":"codex","usage":{"primary":{"usedPercent":42}}}]"#,
        ] as [&[u8]; 3]
        {
            for stale in [false, true] {
                let actual = template
                    .render(payload, &config, NOW, " ", stale, TemplateScope::Worst)
                    .unwrap();
                let previous = emit_prompt_segment(
                    payload,
                    &config,
                    NOW,
                    PromptOptions {
                        provider_filter: &[],
                        ansi: false,
                        stale,
                    },
                )
                .unwrap();
                assert_eq!(actual, previous);
            }
        }
    }

    #[test]
    fn custom_per_provider_format_and_window_scope() {
        let actual =
            Template::parse("{provider}:{window}:{sigil}:{used}/{remaining}:{class}:{countdown}")
                .unwrap()
                .render(
                    PAYLOAD,
                    &config(),
                    NOW,
                    " | ",
                    false,
                    TemplateScope::PerProvider,
                )
                .unwrap();
        assert_eq!(
            actual,
            "codex:secondary:CX:80/20:warn:1h | claude:primary:CL:10/90:good:3h"
        );
    }

    #[test]
    fn payload_strings_strip_control_characters() {
        let payload = br#"[
            {"provider":"codex","usage":{
                "primary":{"usedPercent":30},
                "extraRateWindows":[{"title":"Model\u001b]0;hijack\u0007\u007f\u0085 pool","window":{"usedPercent":90}}]
            }}
        ]"#;
        let output = Template::parse("{provider}:{window}")
            .unwrap()
            .render(payload, &config(), NOW, ",", false, TemplateScope::Worst)
            .unwrap();
        assert_eq!(output, "codex:Model]0;hijack pool");
        assert!(!output.chars().any(char::is_control));
    }

    #[test]
    fn escaped_braces_and_unknown_placeholders() {
        let actual = Template::parse("{{{provider}}}")
            .unwrap()
            .render(PAYLOAD, &config(), NOW, ",", false, TemplateScope::Worst)
            .unwrap();
        assert_eq!(actual, "{codex}");
        assert_eq!(
            Template::parse("{mystery}").err().as_deref(),
            Some("unknown template placeholder: mystery")
        );
        assert!(Template::parse("{used").is_err());
        assert!(Template::parse("used}").is_err());
    }
}
