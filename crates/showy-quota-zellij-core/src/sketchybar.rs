//! SketchyBar row emitter.
//!
//! Ports the SketchyBar plugin's per-tick jq/date compute (renderable
//! filtering, slot/pooled row assembly, elapsed markers, countdown label,
//! shared-cycle and stale handling) into the native renderer so the shell
//! plugin only assembles `sketchybar --set` arguments from final strings.
//!
//! Output framing (fields separated by US `\x1f`, records by newline):
//!
//! ```text
//! <stale 0|1> US <degraded_cli 0|1>
//! provider US label US label_argb US status US status_url
//!          US p_present US p_rem US p_marker US p_argb
//!          US s_present US s_rem US s_marker US s_argb
//!          US t_present US t_rem US t_marker US t_argb
//!          US q_present US q_rem US q_marker US q_argb
//! ```
//!
//! Marker fields are empty when no elapsed marker should draw. Every color is
//! a final `0xffRRGGBB` SketchyBar literal. The row semantics mirror the
//! previous shell/jq pipeline byte-for-byte, including its quirks (absent
//! lanes render remaining `0` with the bad-severity highlight; a shared cycle
//! only suppresses the secondary/tertiary markers).

use serde_json::Value;

use crate::codexbar::{is_renderable, NamedWindow, ProviderRecord, UsageWindow};
use crate::config::RenderConfig;
use crate::render::{format_countdown, RenderError};
use crate::reset::{minutes_until, reset_epoch};

const FIELD_SEP: char = '\u{001f}';
const LANE_COUNT: usize = 4;

#[derive(Debug, Clone, Copy)]
pub struct SketchybarOptions {
    pub stale: bool,
    pub degraded_cli: bool,
    /// `SHOWY_QUOTA_PNG_BAR_W`: slider width in pixels. Elapsed markers are
    /// quantized to this width and converted to a slider percentage.
    pub bar_width: i64,
}

/// One assembled slider lane. `rem` is the integer remaining percent the jq
/// row carried; `reset`/`win` keep the row's raw strings (`resetsAt //
/// resetDescription // ""` and raw `windowMinutes`), which drive marker math,
/// long-horizon dimming, and the shared-cycle key exactly like the shell did.
struct Lane {
    rem: i64,
    reset: String,
    win: String,
}

pub fn emit_sketchybar(
    payload: &[u8],
    config: &RenderConfig,
    now_epoch: i64,
    options: SketchybarOptions,
) -> Result<String, RenderError> {
    let value: Value = serde_json::from_slice(payload).map_err(|_| RenderError::InvalidPayload)?;
    let mut out = header(options);
    let Value::Array(records) = value else {
        return Ok(out);
    };
    let records: Vec<ProviderRecord> =
        serde_json::from_value(Value::Array(records)).map_err(|_| RenderError::InvalidPayload)?;

    let mut renderable: Vec<&ProviderRecord> = records
        .iter()
        .filter(|record| is_renderable(record) && passes_filters(record, config))
        .collect();
    sort_records(&mut renderable, config);

    for record in renderable {
        out.push('\n');
        provider_line(&mut out, record, config, now_epoch, options);
    }
    Ok(out)
}

fn header(options: SketchybarOptions) -> String {
    format!(
        "{}{FIELD_SEP}{}",
        u8::from(options.stale),
        u8::from(options.degraded_cli)
    )
}

fn passes_filters(record: &ProviderRecord, config: &RenderConfig) -> bool {
    (config.providers.is_empty() || contains(&config.providers, &record.provider))
        && !contains(&config.providers_exclude, &record.provider)
}

fn contains(items: &[String], provider: &str) -> bool {
    items.iter().any(|item| item == provider)
}

/// Mirror of the shell `showy_quota_filter_renderable` ordering: the
/// allow-list order wins, then the display order preference, else the
/// payload order; ties break on the provider id.
fn sort_records(records: &mut [&ProviderRecord], config: &RenderConfig) {
    let order = if !config.providers.is_empty() {
        &config.providers
    } else if !config.provider_order.is_empty() {
        &config.provider_order
    } else {
        return;
    };
    records.sort_by(|a, b| {
        position(order, &a.provider)
            .cmp(&position(order, &b.provider))
            .then_with(|| a.provider.cmp(&b.provider))
    });
}

fn position(items: &[String], provider: &str) -> usize {
    items
        .iter()
        .position(|item| item == provider)
        .unwrap_or(1_000_000)
}

fn provider_line(
    out: &mut String,
    record: &ProviderRecord,
    config: &RenderConfig,
    now_epoch: i64,
    options: SketchybarOptions,
) {
    let lanes = provider_lanes(record);
    let tz = config.reset_description_timezone_offset_minutes;

    let rem: Vec<i64> = lanes
        .iter()
        .map(|lane| lane.as_ref().map_or(0, |lane| lane.rem.clamp(0, 100)))
        .collect();
    let mut markers: Vec<Option<i64>> = lanes
        .iter()
        .map(|lane| {
            lane.as_ref().and_then(|lane| {
                let x = elapsed_marker_x(&lane.reset, &lane.win, now_epoch, options.bar_width, tz)?;
                marker_percentage_from_x(x, options.bar_width)
            })
        })
        .collect();
    let mut long: Vec<bool> = lanes
        .iter()
        .map(|lane| {
            lane.as_ref()
                .is_some_and(|lane| is_long_window_str(&lane.win, config.dim_window_minutes))
        })
        .collect();

    // Countdown label + color from the primary lane only.
    let (label, mut label_color) = match lanes[0].as_ref() {
        None => ("idle".to_string(), argb(&config.palette_countdown)),
        Some(lane) => {
            let minutes = if lane.reset.is_empty() {
                None
            } else {
                minutes_until(&lane.reset, now_epoch, tz)
            };
            let label = match minutes {
                Some(minutes) => format_countdown(minutes),
                None if lane.reset.is_empty() && lane.rem >= 100 => "idle".into(),
                None => "?".into(),
            };
            let color = match minutes {
                Some(minutes) if minutes < config.time_warn_minutes => {
                    argb(&config.palette_countdown_warn)
                }
                _ => argb(&config.palette_countdown),
            };
            (label, color)
        }
    };

    // Parallel pools on one billing cycle (e.g. Cursor Total/Auto/API): keep
    // only the primary pacing marker and undim every row. Quirk preserved
    // from the shell: the quaternary marker is not suppressed.
    if lanes_shared_cycle(&lanes) {
        markers[1] = None;
        markers[2] = None;
        long.iter_mut().for_each(|flag| *flag = false);
    }

    let mut highlights: Vec<String> = rem
        .iter()
        .zip(long.iter())
        .map(|(remaining, is_long)| argb(&config.window_color(*remaining as i32, *is_long)))
        .collect();

    if options.stale {
        let stale_argb = argb(&config.palette_stale);
        label_color = stale_argb.clone();
        highlights.iter_mut().for_each(|c| *c = stale_argb.clone());
        markers.iter_mut().for_each(|m| *m = None);
    }

    let (status, status_url) = provider_status(record);

    out.push_str(&record.provider);
    push_field(out, &label);
    push_field(out, &label_color);
    push_field(out, &status);
    push_field(out, &status_url);
    for index in 0..LANE_COUNT {
        push_field(out, if lanes[index].is_some() { "1" } else { "0" });
        push_field(out, &rem[index].to_string());
        push_field(
            out,
            &markers[index].map(|m| m.to_string()).unwrap_or_default(),
        );
        push_field(out, &highlights[index]);
    }
}

fn push_field(out: &mut String, value: &str) {
    out.push(FIELD_SEP);
    out.push_str(value);
}

fn argb(hex: &str) -> String {
    format!("0xff{hex}")
}

/// `.status.indicator // "none"` and `.status.url // ""`, with control
/// characters stripped so payload data cannot break the record framing.
fn provider_status(record: &ProviderRecord) -> (String, String) {
    let indicator = record
        .status
        .as_ref()
        .and_then(|status| status.indicator.clone())
        .unwrap_or_else(|| "none".into());
    let url = record
        .status
        .as_ref()
        .and_then(|status| status.url.clone())
        .unwrap_or_default();
    (sanitize_field(&indicator), sanitize_field(&url))
}

fn sanitize_field(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

/// Assemble up to four lanes. Slots are semantic: primary/secondary/tertiary
/// map to fixed rows and missing slots stay empty. When every present
/// positional slot is mirrored by a measured `extraRateWindows` entry, the
/// pooled layout takes over and lanes come from the extras — a
/// `usageKnown:false` extra keeps its lane drawn as an empty track (rem 0, no
/// marker) so a transiently-thin family does not vanish.
fn provider_lanes(record: &ProviderRecord) -> [Option<Lane>; LANE_COUNT] {
    let Some(usage) = record.usage.as_ref() else {
        return [None, None, None, None];
    };

    let slots = usage.render_slots();
    let extras: Vec<&NamedWindow> = usage
        .extra_rate_windows
        .iter()
        .filter(|extra| {
            extra
                .window
                .as_ref()
                .is_some_and(|window| window.used_percent.is_some())
        })
        .collect();

    let extra_keys: Vec<(Option<i64>, Option<&str>)> = extras
        .iter()
        .map(|extra| {
            let window = extra.window.as_ref().expect("extras keep their window");
            (window.window_minutes(), window.resets_at.as_deref())
        })
        .collect();
    let unmatched = slots.iter().flatten().any(|window| {
        let key = (window.window_minutes(), window.resets_at.as_deref());
        !extra_keys.contains(&key)
    });
    // A coincidental (windowMinutes, resetsAt) collision between a positional
    // slot and a single extra is not model pooling; require the extras to
    // carry more pools than the positional view exposes (mirror of
    // render.rs::pooled_auto).
    let present_positional = slots.iter().flatten().count();
    let pooled = !extras.is_empty() && !unmatched && extras.len() > present_positional;

    let mut lanes: [Option<Lane>; LANE_COUNT] = [None, None, None, None];
    if pooled {
        for (index, extra) in extras.into_iter().take(LANE_COUNT).enumerate() {
            lanes[index] = Some(if extra.usage_known == Some(false) {
                Lane {
                    rem: 0,
                    reset: String::new(),
                    win: String::new(),
                }
            } else {
                lane_from(extra.window.as_ref().expect("extras keep their window"))
            });
        }
    } else {
        for (index, window) in slots.into_iter().enumerate() {
            lanes[index] = window.map(lane_from);
        }
    }
    lanes
}

/// jq `row(w)`: remaining percent plus the raw reset/window strings. The jq
/// `//` operator only skips null, so an empty `resetsAt` string is kept and
/// does not fall through to `resetDescription`.
fn lane_from(window: &UsageWindow) -> Lane {
    Lane {
        rem: i64::from(100 - window.used_pct_floor()),
        reset: window
            .resets_at
            .clone()
            .or_else(|| window.reset_description.clone())
            .unwrap_or_default(),
        win: window
            .window_minutes
            .map(|minutes| minutes.to_string())
            .unwrap_or_default(),
    }
}

/// Mirror of the shell `showy_quota_shared_cycle` over assembled rows: every
/// present lane must carry an identical non-empty reset and window, and at
/// least two lanes must be present.
fn lanes_shared_cycle(lanes: &[Option<Lane>; LANE_COUNT]) -> bool {
    let mut reference: Option<(&str, &str)> = None;
    let mut count = 0u32;
    for lane in lanes.iter().flatten() {
        if lane.reset.is_empty() || lane.win.is_empty() {
            return false;
        }
        let key = (lane.reset.as_str(), lane.win.as_str());
        match reference {
            None => reference = Some(key),
            Some(previous) if previous == key => {}
            Some(_) => return false,
        }
        count += 1;
    }
    count >= 2
}

/// Mirror of the shell `showy_quota_is_long_window` on the row's raw window
/// string: a non-negative integer at or beyond the dim horizon.
fn is_long_window_str(win: &str, dim_window_minutes: i64) -> bool {
    parse_uint(win).is_some_and(|minutes| minutes >= dim_window_minutes)
}

/// Shell `^[0-9]+$` plus i64 range; anything else fails like the shell's
/// guard clauses did.
fn parse_uint(value: &str) -> Option<i64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Mirror of the shell `elapsed_marker_x`: pixel position of the pacing
/// marker on a `bar_width`-wide slider, or None when the lane has no usable
/// reset/window (or the window arithmetic would overflow, matching the
/// shell's checked_mul guard).
fn elapsed_marker_x(
    reset: &str,
    win: &str,
    now_epoch: i64,
    bar_width: i64,
    tz_offset_minutes: Option<i16>,
) -> Option<i64> {
    if reset.is_empty() || bar_width <= 1 {
        return None;
    }
    let window_minutes = parse_uint(win)?;
    if window_minutes <= 0 {
        return None;
    }
    let reset_epoch = reset_epoch(reset, now_epoch, tz_offset_minutes)?;
    let duration = window_minutes.checked_mul(60)?;
    let start = reset_epoch.checked_sub(duration)?;
    let elapsed = now_epoch.checked_sub(start)?.clamp(0, duration);
    let marker =
        (i128::from(duration - elapsed) * i128::from(bar_width) / i128::from(duration)) as i64;
    Some(marker.clamp(0, bar_width - 1))
}

/// Mirror of the shell `marker_percentage_from_x`: nearest-percent position
/// of a marker pixel on the slider.
fn marker_percentage_from_x(marker: i64, bar_width: i64) -> Option<i64> {
    if bar_width <= 1 {
        return None;
    }
    let marker = marker.clamp(0, bar_width - 1);
    Some((marker * 100 + (bar_width - 1) / 2) / (bar_width - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR_W: i64 = 80;

    fn options() -> SketchybarOptions {
        SketchybarOptions {
            stale: false,
            degraded_cli: false,
            bar_width: BAR_W,
        }
    }

    fn emit(payload: &str, config: &RenderConfig, now: i64, options: SketchybarOptions) -> String {
        emit_sketchybar(payload.as_bytes(), config, now, options).expect("emit succeeds")
    }

    fn lines(rendered: &str) -> Vec<Vec<String>> {
        rendered
            .lines()
            .map(|line| line.split(FIELD_SEP).map(str::to_string).collect())
            .collect()
    }

    #[test]
    fn header_carries_stale_and_degraded_flags() {
        let config = RenderConfig::default();
        let rendered = emit(
            "[]",
            &config,
            1_700_000_000,
            SketchybarOptions {
                stale: true,
                degraded_cli: false,
                bar_width: BAR_W,
            },
        );
        assert_eq!(rendered, format!("1{FIELD_SEP}0"));
    }

    #[test]
    fn positional_slots_render_fixed_lanes_and_countdown() {
        let config = RenderConfig::default();
        let now = 1_700_000_000;
        // Reset in 100 minutes on a 300-minute window.
        let payload = r#"[{
            "provider": "codex",
            "usage": {
                "primary": {"usedPercent": 25, "resetsAt": "2023-11-14T23:53:20Z", "windowMinutes": 300},
                "secondary": {"usedPercent": 90.9, "resetsAt": "2023-11-20T22:23:20Z", "windowMinutes": 10080}
            }
        }]"#;
        let rendered = emit(payload, &config, now, options());
        let rows = lines(&rendered);
        assert_eq!(rows.len(), 2);
        let row = &rows[1];
        assert_eq!(row[0], "codex");
        assert_eq!(row[1], "1:40", "countdown label from primary reset");
        assert_eq!(row[2], "0xff7b8496", "calm countdown color");
        assert_eq!(row[3], "none");
        assert_eq!(row[4], "");
        // Primary lane: present, rem 75, bright good highlight.
        assert_eq!(row[5], "1");
        assert_eq!(row[6], "75");
        assert_eq!(row[7], "33", "elapsed marker percent");
        assert_eq!(row[8], "0xff25be6a");
        // Secondary lane: floor(90.9) = 90 used -> rem 10, long window dims bad.
        assert_eq!(row[9], "1");
        assert_eq!(row[10], "10");
        assert_eq!(row[12], "0xff822d52", "dimmed bad highlight");
        // Tertiary/quaternary absent: rem 0, no marker, bright bad highlight.
        assert_eq!(row[13], "0");
        assert_eq!(row[14], "0");
        assert_eq!(row[15], "");
        assert_eq!(row[16], "0xffee5396");
        assert_eq!(row[17], "0");
        assert_eq!(row[18], "0");
        assert_eq!(row[19], "");
        assert_eq!(row[20], "0xffee5396");
    }

    #[test]
    fn marker_positions_match_shell_math() {
        // 300-minute window, reset 100 minutes out: elapsed 12000 of 18000s.
        // marker = (18000 - 12000) * 80 / 18000 = 26; pct = (26*100+39)/79 = 33.
        assert_eq!(
            elapsed_marker_x("2023-11-14T23:53:20Z", "300", 1_700_000_000, BAR_W, None),
            Some(26)
        );
        assert_eq!(marker_percentage_from_x(26, BAR_W), Some(33));
        // Degenerate widths never emit markers (shell parity).
        assert_eq!(
            elapsed_marker_x("2023-11-14T23:53:20Z", "300", 1_700_000_000, 1, None),
            None
        );
        assert_eq!(marker_percentage_from_x(5, 1), None);
        // Absurd windows fail the overflow guard instead of wrapping.
        assert_eq!(
            elapsed_marker_x(
                "2023-11-14T23:53:20Z",
                "9223372036854775807",
                1_700_000_000,
                BAR_W,
                None
            ),
            None
        );
    }

    #[test]
    fn shared_cycle_keeps_primary_marker_and_undims() {
        let config = RenderConfig::default();
        let now = 1_700_000_000;
        // Three pools on one monthly cycle (Cursor-style).
        let payload = r#"[{
            "provider": "cursor",
            "usage": {
                "primary": {"usedPercent": 10, "resetsAt": "2023-12-14T22:13:20Z", "windowMinutes": 43200},
                "secondary": {"usedPercent": 20, "resetsAt": "2023-12-14T22:13:20Z", "windowMinutes": 43200},
                "tertiary": {"usedPercent": 95, "resetsAt": "2023-12-14T22:13:20Z", "windowMinutes": 43200}
            }
        }]"#;
        let rendered = emit(payload, &config, now, options());
        let row = &lines(&rendered)[1];
        assert!(!row[7].is_empty(), "primary marker survives");
        assert_eq!(row[11], "", "secondary marker suppressed");
        assert_eq!(row[15], "", "tertiary marker suppressed");
        // Undimmed: monthly window would normally dim, shared cycle stays bright.
        assert_eq!(row[8], "0xff25be6a");
        assert_eq!(row[16], "0xffee5396");
    }

    #[test]
    fn pooled_extras_take_over_lanes_and_keep_unknown_lane_empty() {
        let config = RenderConfig::default();
        let now = 1_700_000_000;
        let payload = r#"[{
            "provider": "antigravity",
            "usage": {
                "primary": {"usedPercent": 30, "resetsAt": "2023-11-15T02:13:20Z", "windowMinutes": 300},
                "extraRateWindows": [
                    {"title": "Claude", "usageKnown": true,
                     "window": {"usedPercent": 30, "resetsAt": "2023-11-15T02:13:20Z", "windowMinutes": 300}},
                    {"title": "GPT", "usageKnown": false,
                     "window": {"usedPercent": 0, "resetsAt": "2023-11-15T02:13:20Z", "windowMinutes": 300}},
                    {"title": "Image", "usageKnown": true,
                     "window": {"usedPercent": 55, "resetsAt": "2023-11-15T02:13:20Z", "windowMinutes": 300}}
                ]
            }
        }]"#;
        let rendered = emit(payload, &config, now, options());
        let row = &lines(&rendered)[1];
        // Lane 1: measured Claude pool.
        assert_eq!(row[5], "1");
        assert_eq!(row[6], "70");
        // Lane 2: usageKnown:false renders an empty, marker-less track.
        assert_eq!(row[9], "1");
        assert_eq!(row[10], "0");
        assert_eq!(row[11], "");
        assert_eq!(row[12], "0xffee5396");
        // Lane 3: measured Image pool.
        assert_eq!(row[13], "1");
        assert_eq!(row[14], "45");
        // Lane 4 absent.
        assert_eq!(row[17], "0");
    }

    #[test]
    fn unmatched_positional_slot_disables_pooling() {
        let config = RenderConfig::default();
        let now = 1_700_000_000;
        // Secondary has no matching extra -> positional layout.
        let payload = r#"[{
            "provider": "antigravity",
            "usage": {
                "primary": {"usedPercent": 30, "resetsAt": "2023-11-15T02:13:20Z", "windowMinutes": 300},
                "secondary": {"usedPercent": 10, "resetsAt": "2023-11-20T22:23:20Z", "windowMinutes": 10080},
                "extraRateWindows": [
                    {"title": "Claude", "usageKnown": true,
                     "window": {"usedPercent": 30, "resetsAt": "2023-11-15T02:13:20Z", "windowMinutes": 300}}
                ]
            }
        }]"#;
        let rendered = emit(payload, &config, now, options());
        let row = &lines(&rendered)[1];
        assert_eq!(row[5], "1", "primary stays positional");
        assert_eq!(row[6], "70");
        assert_eq!(row[9], "1", "secondary stays positional");
        assert_eq!(row[10], "90");
        assert_eq!(row[13], "0", "no pooled tertiary lane");
    }

    #[test]
    fn stale_greys_everything_and_clears_markers() {
        let config = RenderConfig::default();
        let now = 1_700_000_000;
        let payload = r#"[{
            "provider": "codex",
            "usage": {
                "primary": {"usedPercent": 25, "resetsAt": "2023-11-14T23:53:20Z", "windowMinutes": 300}
            }
        }]"#;
        let rendered = emit(
            payload,
            &config,
            now,
            SketchybarOptions {
                stale: true,
                degraded_cli: true,
                bar_width: BAR_W,
            },
        );
        let rows = lines(&rendered);
        assert_eq!(rows[0], vec!["1", "1"]);
        let row = &rows[1];
        assert_eq!(row[1], "1:40", "label text survives stale");
        assert_eq!(row[2], "0xff6c7086", "stale label color");
        assert_eq!(row[7], "", "stale suppresses markers");
        assert_eq!(row[8], "0xff6c7086");
        assert_eq!(row[12], "0xff6c7086");
    }

    #[test]
    fn label_edges_idle_and_unknown() {
        let config = RenderConfig::default();
        let now = 1_700_000_000;
        // Label comes from the primary lane. Untouched no-reset primary -> idle;
        // consumed no-reset primary -> '?'. When the primary slot is absent the
        // live window left-compacts into the primary lane and drives the label:
        // gemini's untouched no-reset secondary promotes to an idle primary.
        let payload = r#"[
            {"provider": "codex", "usage": {"primary": {"usedPercent": 0}}},
            {"provider": "claude", "usage": {"primary": {"usedPercent": 40}}},
            {"provider": "gemini", "usage": {"secondary": {"usedPercent": 0}}}
        ]"#;
        let rendered = emit(payload, &config, now, options());
        let rows = lines(&rendered);
        assert_eq!(rows[1][0], "codex");
        assert_eq!(rows[1][1], "idle");
        assert_eq!(rows[2][1], "?");
        assert_eq!(rows[3][0], "gemini");
        assert_eq!(rows[3][1], "idle", "promoted no-reset full window is idle");
        assert_eq!(
            rows[3][5], "1",
            "live window promoted into the primary lane"
        );
        assert_eq!(rows[3][9], "0", "secondary lane vacated by promotion");
    }

    #[test]
    fn null_primary_promotes_live_window_and_depools_key_collision() {
        let config = RenderConfig::default();
        let now = 1_700_000_000; // 2023-11-14T22:13:20Z
                                 // Codex after OpenAI removed the 5h limit: primary is null, the weekly
                                 // is the live cap, and a Spark weekly coincidentally shares its
                                 // (windowMinutes, resetsAt) key. The single extra must NOT make Codex
                                 // look model-pooled (one extra vs one positional slot), so the
                                 // positional weekly's usage wins (100 remaining, not the extra's 60),
                                 // and it left-compacts into the primary lane with the weekly countdown.
        let payload = r#"[{
            "provider": "codex",
            "usage": {
                "primary": null,
                "secondary": {"usedPercent": 0, "resetsAt": "2023-11-20T22:13:20Z", "windowMinutes": 10080},
                "extraRateWindows": [
                    {"title": "Codex Spark Weekly", "usageKnown": true,
                     "window": {"usedPercent": 40, "resetsAt": "2023-11-20T22:13:20Z", "windowMinutes": 10080}}
                ]
            }
        }]"#;
        let row = &lines(&emit(payload, &config, now, options()))[1];
        assert_eq!(row[0], "codex");
        assert_eq!(row[1], "6d", "promoted weekly drives the countdown");
        assert_eq!(row[5], "1", "weekly promoted into the primary lane");
        assert_eq!(row[6], "100", "positional weekly wins, not the Spark extra");
        assert_eq!(row[9], "0", "no secondary lane");
    }

    #[test]
    fn filters_errors_and_orders_providers() {
        let config = RenderConfig {
            provider_order: vec!["gemini".into(), "codex".into()],
            ..RenderConfig::default()
        };
        let payload = r#"[
            {"provider": "codex", "usage": {"primary": {"usedPercent": 10}}},
            {"provider": "broken", "error": {"message": "nope"}},
            {"provider": "gemini", "usage": {"primary": {"usedPercent": 20}}}
        ]"#;
        let rendered = emit(payload, &config, 1_700_000_000, options());
        let rows = lines(&rendered);
        assert_eq!(rows.len(), 3, "error-only provider is dropped");
        assert_eq!(rows[1][0], "gemini");
        assert_eq!(rows[2][0], "codex");
    }

    #[test]
    fn status_fields_pass_through_sanitized() {
        let config = RenderConfig::default();
        let payload = r#"[{
            "provider": "codex",
            "usage": {"primary": {"usedPercent": 10}},
            "status": {"indicator": "major", "url": "https://status.example.com/x\u001fy"}
        }]"#;
        let rendered = emit(payload, &config, 1_700_000_000, options());
        let row = &lines(&rendered)[1];
        assert_eq!(row[3], "major");
        assert_eq!(
            row[4], "https://status.example.com/xy",
            "control chars stripped"
        );
    }
}
