//! Choose the provider with the most quota in its selected usage window.
use serde::Serialize;

use crate::config::RenderConfig;
use crate::metrics::{provider_metrics, WindowMetric};
use crate::render::RenderError;

#[derive(Debug, Clone, Copy)]
pub struct PickOptions<'a> {
    pub provider_filter: &'a [String],
    pub window: &'a str,
    pub min_remaining: i32,
    pub json: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PickResult<'a> {
    provider: &'a str,
    window: &'static str,
    remaining_percent: i32,
    minutes_until_reset: Option<i64>,
}

/// None means that no renderable provider meets the requested floor.
pub fn emit_pick(
    payload: &[u8],
    config: &RenderConfig,
    now_epoch: i64,
    options: PickOptions<'_>,
) -> Result<Option<String>, RenderError> {
    let metrics = provider_metrics(payload, config, now_epoch)?;
    let mut best: Option<PickResult<'_>> = None;
    for metric in &metrics {
        if !options.provider_filter.is_empty()
            && !options
                .provider_filter
                .iter()
                .any(|id| id == &metric.provider)
        {
            continue;
        }
        let windows = [
            ("primary", metric.windows.primary.as_ref()),
            ("secondary", metric.windows.secondary.as_ref()),
            ("tertiary", metric.windows.tertiary.as_ref()),
        ];
        // A provider is only as healthy as its least-remaining selected window.
        let selected = if options.window == "worst" {
            windows
                .into_iter()
                .filter_map(|(name, window)| window.map(|w| (name, w)))
                .min_by_key(|(_, window)| window.remaining_percent)
        } else {
            windows
                .into_iter()
                .find(|(name, _)| *name == options.window)
                .and_then(|(name, window)| window.map(|w| (name, w)))
        };
        let Some((window_name, window)) = selected else {
            continue;
        };
        if window.remaining_percent < options.min_remaining {
            continue;
        }
        let candidate = pick_result(&metric.provider, window_name, window);
        if best
            .as_ref()
            .is_none_or(|current| candidate.remaining_percent > current.remaining_percent)
        {
            best = Some(candidate);
        }
    }
    best.map(|candidate| {
        if options.json {
            serde_json::to_string(&candidate).map_err(|_| RenderError::InvalidPayload)
        } else {
            Ok(candidate.provider.to_owned())
        }
    })
    .transpose()
}

fn pick_result<'a>(provider: &'a str, name: &'static str, window: &WindowMetric) -> PickResult<'a> {
    PickResult {
        provider,
        window: name,
        remaining_percent: window.remaining_percent,
        minutes_until_reset: window.minutes_until_reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_highest_floor_uses_worst_window_and_skips_errors() {
        let payload = br#"[
            {"provider":"codex","usage":{"primary":{"usedPercent":5},"secondary":{"usedPercent":95}}},
            {"provider":"claude","usage":{"primary":{"usedPercent":40,"resetsAt":"2099-01-01T01:00:00Z"}}},
            {"provider":"broken","error":{"kind":"network","message":"down"}}
        ]"#;
        let config = RenderConfig::default();
        let opts = PickOptions {
            provider_filter: &[],
            window: "worst",
            min_remaining: 50,
            json: true,
        };
        let result = emit_pick(payload, &config, 4_070_908_800, opts)
            .unwrap()
            .unwrap();
        let selected: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(selected["provider"], "claude");
        assert_eq!(selected["remainingPercent"], 60);
        assert_eq!(selected["window"], "primary");
        assert_eq!(selected["minutesUntilReset"], 60);
        assert!(emit_pick(
            payload,
            &config,
            4_070_908_800,
            PickOptions {
                min_remaining: 61,
                ..opts
            }
        )
        .unwrap()
        .is_none());
        assert_eq!(
            emit_pick(
                payload,
                &config,
                4_070_908_800,
                PickOptions {
                    window: "primary",
                    min_remaining: 0,
                    json: false,
                    ..opts
                }
            )
            .unwrap()
            .as_deref(),
            Some("codex")
        );
    }
}
