//! SketchyBar rows.
//!
//! The per-provider compute behind the SketchyBar pill: renderable filtering,
//! slot/pooled lane assembly, elapsed markers, the countdown label, and
//! shared-cycle and stale handling. `sketchybar_frame` turns these rows into
//! the final `sketchybar --set` arguments.
//!
//! Every color is a final `0xffRRGGBB` SketchyBar literal. A marker is `None`
//! when no elapsed marker should draw. An `error` row is a provider CodexBar
//! could not read at all: no lane is present, and the pill draws its label
//! alone rather than a full bar at zero remaining. The lane semantics keep the
//! quirks of the original shell/jq pipeline (absent lanes carry remaining `0`
//! with the bad-severity highlight; a shared cycle only suppresses the
//! secondary/tertiary markers).

use crate::codexbar::{is_errored, is_renderable, NamedWindow, ProviderRecord, UsageWindow};
use crate::config::RenderConfig;
use crate::metrics::parse_display_payload;
use crate::render::{format_countdown, RenderError};
use crate::reset::{minutes_until, reset_epoch};

pub const LANE_COUNT: usize = 4;

/// The rows for one tick plus the whole-cache flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SketchybarRows {
    pub stale: bool,
    pub degraded_cli: bool,
    pub rows: Vec<SketchybarRow>,
}

/// One provider's pill: countdown label, status, and four slider lanes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SketchybarRow {
    pub provider: String,
    pub label: String,
    pub label_argb: String,
    /// Normalised status indicator (`none`, `minor`, `major`, ...).
    pub status: String,
    /// Status page URL with control characters stripped; empty when absent.
    pub status_url: String,
    pub lanes: [RowLane; LANE_COUNT],
    pub error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowLane {
    pub present: bool,
    /// Remaining percent, 0–100.
    pub remaining: i64,
    /// Elapsed-marker position as a slider percentage.
    pub marker: Option<i64>,
    pub argb: String,
}

const MIN_BAR_WIDTH: i64 = 2;
const MAX_BAR_WIDTH: i64 = 4_096;

#[derive(Debug, Clone, Copy)]
pub struct SketchybarOptions<'a> {
    pub stale: bool,
    pub degraded_cli: bool,
    /// `SHOWY_QUOTA_PNG_BAR_W`: slider width in pixels. Elapsed markers are
    /// quantized to this width and converted to a slider percentage.
    pub bar_width: i64,
    /// Providers whose own slice was carried forward past the stale horizon.
    /// Their rows grey and drop their pacing markers exactly like a wholly
    /// stale bar, while the fresh providers beside them keep their colours.
    pub stale_providers: &'a [String],
}

impl SketchybarOptions<'_> {
    pub(crate) fn stale_for(self, provider: &str) -> bool {
        self.stale
            || self
                .stale_providers
                .iter()
                .any(|stale| stale.as_str() == provider)
    }
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

pub fn sketchybar_rows(
    payload: &[u8],
    config: &RenderConfig,
    now_epoch: i64,
    options: SketchybarOptions,
) -> Result<SketchybarRows, RenderError> {
    let options = SketchybarOptions {
        bar_width: options.bar_width.clamp(MIN_BAR_WIDTH, MAX_BAR_WIDTH),
        ..options
    };
    let records = parse_display_payload(payload)?;

    let mut visible: Vec<&ProviderRecord> = records
        .iter()
        .filter(|record| {
            (is_renderable(record) || is_errored(record)) && passes_filters(record, config)
        })
        .collect();
    sort_records(&mut visible, config);

    let rows = visible
        .into_iter()
        .map(|record| {
            if is_errored(record) {
                error_row(record, config)
            } else {
                provider_row(record, config, now_epoch, options)
            }
        })
        .collect();
    Ok(SketchybarRows {
        stale: options.stale,
        degraded_cli: options.degraded_cli,
        rows,
    })
}

pub(crate) fn passes_filters(record: &ProviderRecord, config: &RenderConfig) -> bool {
    (config.providers.is_empty() || contains(&config.providers, &record.provider))
        && !contains(&config.providers_exclude, &record.provider)
}

fn contains(items: &[String], provider: &str) -> bool {
    items.iter().any(|item| item == provider)
}

/// Mirror of the shell `showy_quota_filter_renderable` ordering: the
/// allow-list order wins, then the display order preference, else the
/// payload order; ties break on the provider id.
pub(crate) fn sort_records(records: &mut [&ProviderRecord], config: &RenderConfig) {
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

fn provider_row(
    record: &ProviderRecord,
    config: &RenderConfig,
    now_epoch: i64,
    options: SketchybarOptions,
) -> SketchybarRow {
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

    // Countdown label + color from the positional primary slot, never from
    // assembled lane 0. Under the pooled layout lane 0 is an extras-derived row
    // that may be a `usageKnown:false` placeholder carrying no reset, which
    // printed `?` for a provider that does have a live countdown (Antigravity,
    // whose 5-hour pools go unknown once the weekly is exhausted) and disagreed
    // with the terminal strip — `render.rs::render_chunk` always reads the
    // positional primary.
    let (mut label, mut label_color) = match label_lane(record).as_ref() {
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

    if options.stale_for(&record.provider) {
        let stale_argb = argb(&config.palette_stale);
        label_color = stale_argb.clone();
        highlights.iter_mut().for_each(|c| *c = stale_argb.clone());
        markers.iter_mut().for_each(|m| *m = None);
    }
    if config.severity_glyphs && !options.stale_for(&record.provider) {
        if let Some(primary) = label_lane(record) {
            label.insert_str(0, config.severity(primary.rem as i32).marker());
        }
    }

    let (status, status_url) = provider_status(record);
    let lanes = std::array::from_fn(|index| RowLane {
        present: lanes[index].is_some(),
        remaining: rem[index],
        marker: markers[index],
        argb: highlights[index].clone(),
    });

    SketchybarRow {
        provider: record.provider.clone(),
        label,
        label_argb: label_color,
        status,
        status_url,
        lanes,
        error: false,
    }
}

/// A provider CodexBar could not read at all (expired login, network
/// failure). It keeps its place in the bar with an `⚠err` label and no
/// lanes: a provider that silently disappears reads as "not configured",
/// which is the one conclusion the data does not support. Marking the row
/// `error` lets the pill draw the label alone, because an absent lane
/// otherwise renders as a full bad-severity bar at zero remaining.
fn error_row(record: &ProviderRecord, config: &RenderConfig) -> SketchybarRow {
    let error_argb = argb(&config.palette_countdown_warn);
    let (status, status_url) = provider_status(record);
    SketchybarRow {
        provider: record.provider.clone(),
        label: sanitize_field(&format!("{}err", config.error_glyph)),
        label_argb: error_argb.clone(),
        status,
        status_url,
        lanes: std::array::from_fn(|_| RowLane {
            present: false,
            remaining: 0,
            marker: None,
            argb: error_argb.clone(),
        }),
        error: true,
    }
}

fn argb(hex: &str) -> String {
    format!("0xff{hex}")
}

/// `.status.indicator // "none"` and `.status.url // ""`, with control
/// characters stripped so payload data cannot break the record framing. The
/// indicator reads the same normalised token the metrics emitter and the
/// terminal tint use, so all three surfaces agree on what counts as active.
fn provider_status(record: &ProviderRecord) -> (String, String) {
    let indicator =
        crate::metrics::normalized_status_indicator(record).unwrap_or_else(|| "none".into());
    let url = record
        .status
        .as_ref()
        .and_then(|status| status.url.clone())
        .map(|url| {
            url.chars()
                .filter(|c| !c.is_control())
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .unwrap_or_default();
    (sanitize_field(&indicator), sanitize_field(&url))
}

pub(crate) fn sanitize_field(value: &str) -> String {
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

/// The window the countdown label reads: the positional primary slot after
/// `render_slots` left-compaction, matching the terminal strip. Deliberately
/// independent of `provider_lanes`, whose pooled branch replaces lane 0 with an
/// extras-derived row that need not be the provider's live window.
fn label_lane(record: &ProviderRecord) -> Option<Lane> {
    let usage = record.usage.as_ref()?;
    usage.render_slots()[0].map(lane_from)
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
pub(crate) fn elapsed_marker_x(
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
pub(crate) fn marker_percentage_from_x(marker: i64, bar_width: i64) -> Option<i64> {
    if bar_width <= 1 {
        return None;
    }
    let marker = marker.clamp(0, bar_width - 1);
    let denominator = bar_width.checked_sub(1)?;
    let numerator = marker.checked_mul(100)?.checked_add(denominator / 2)?;
    Some((numerator / denominator).clamp(0, 100))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR_W: i64 = 80;

    fn options() -> SketchybarOptions<'static> {
        SketchybarOptions {
            stale: false,
            degraded_cli: false,
            bar_width: BAR_W,
            stale_providers: &[],
        }
    }

    fn emit(
        payload: &str,
        config: &RenderConfig,
        now: i64,
        options: SketchybarOptions,
    ) -> SketchybarRows {
        sketchybar_rows(payload.as_bytes(), config, now, options).expect("rows succeed")
    }

    /// Flatten rows into positional fields so each test reads one lane slot
    /// by index: header `[stale, degraded]`, then per row `provider, label,
    /// label_argb, status, status_url`, four `present, remaining, marker,
    /// argb` blocks (an absent marker is empty), and `error`.
    fn lines(rows: &SketchybarRows) -> Vec<Vec<String>> {
        let flag = |value: bool| String::from(if value { "1" } else { "0" });
        let mut out = vec![vec![flag(rows.stale), flag(rows.degraded_cli)]];
        for row in &rows.rows {
            let mut fields = vec![
                row.provider.clone(),
                row.label.clone(),
                row.label_argb.clone(),
                row.status.clone(),
                row.status_url.clone(),
            ];
            for lane in &row.lanes {
                fields.push(flag(lane.present));
                fields.push(lane.remaining.to_string());
                fields.push(lane.marker.map(|m| m.to_string()).unwrap_or_default());
                fields.push(lane.argb.clone());
            }
            fields.push(flag(row.error));
            out.push(fields);
        }
        out
    }

    #[test]
    fn header_carries_stale_and_degraded_flags() {
        let config = RenderConfig::default();
        let rows = emit(
            "[]",
            &config,
            1_700_000_000,
            SketchybarOptions {
                stale: true,
                degraded_cli: false,
                bar_width: BAR_W,
                stale_providers: &[],
            },
        );
        assert!(rows.stale);
        assert!(!rows.degraded_cli);
        assert!(rows.rows.is_empty());
    }

    #[test]
    fn rejects_non_array_payloads() {
        let config = RenderConfig::default();
        for payload in ["{}", "null", "\"quota\"", "42"] {
            assert!(matches!(
                sketchybar_rows(payload.as_bytes(), &config, 1_700_000_000, options()),
                Err(RenderError::InvalidPayload)
            ));
        }
    }

    #[test]
    fn ignores_invalid_records_and_keeps_valid_rows() {
        let rendered = emit(
            r#"[
                {"provider":"codex","usage":{"primary":{"usedPercent":42}}},
                {"provider":"-evil","usage":{"primary":{"usedPercent":13}}},
                {"provider":"malformed","usage":{"primary":"invalid"}}
            ]"#,
            &RenderConfig::default(),
            1_700_000_000,
            options(),
        );
        let rows = lines(&rendered);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][0], "codex");
    }

    #[test]
    fn clamps_public_bar_width_and_keeps_markers_bounded() {
        let config = RenderConfig::default();
        let payload = r#"[{"provider":"codex","usage":{"primary":{"usedPercent":25,"resetsAt":"2023-11-14T23:53:20Z","windowMinutes":300}}}]"#;
        for bar_width in [-1, 0, 2, 4_096, i64::MAX] {
            let rendered = emit(
                payload,
                &config,
                1_700_000_000,
                SketchybarOptions {
                    bar_width,
                    ..options()
                },
            );
            let marker = lines(&rendered)[1][7].parse::<i64>().expect("marker");
            assert!((0..=100).contains(&marker));
        }
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
    fn pooled_unknown_lane_zero_does_not_hide_the_countdown() {
        let config = RenderConfig::default();
        let now = 1_700_000_000; // 2023-11-14T22:13:20Z
                                 // Antigravity with both 5-hour pools reported
                                 // usageKnown:false because the weeklies are
                                 // exhausted. Pooled lane 0 is then a
                                 // placeholder with no reset; the label must
                                 // still come from the positional weekly, which
                                 // is what the terminal strip prints.
        let payload = r#"[{
            "provider": "antigravity",
            "usage": {
                "primary": {"usedPercent": 100, "resetsAt": "2023-11-20T22:13:20Z", "windowMinutes": 10080},
                "secondary": {"usedPercent": 100, "resetsAt": "2023-11-22T22:13:20Z", "windowMinutes": 10080},
                "extraRateWindows": [
                    {"title": "Gemini 5-hour", "usageKnown": false,
                     "window": {"usedPercent": 0, "resetsAt": "2023-11-15T02:13:20Z", "windowMinutes": 300}},
                    {"title": "Gemini weekly", "usageKnown": true,
                     "window": {"usedPercent": 100, "resetsAt": "2023-11-20T22:13:20Z", "windowMinutes": 10080}},
                    {"title": "3p 5-hour", "usageKnown": false,
                     "window": {"usedPercent": 0, "resetsAt": "2023-11-15T02:13:20Z", "windowMinutes": 300}},
                    {"title": "3p weekly", "usageKnown": true,
                     "window": {"usedPercent": 100, "resetsAt": "2023-11-22T22:13:20Z", "windowMinutes": 10080}}
                ]
            }
        }]"#;
        let row = &lines(&emit(payload, &config, now, options()))[1];
        assert_eq!(row[1], "6d", "countdown tracks the positional weekly");
        assert_eq!(row[5], "1", "pooled lane 0 still drawn");
        assert_eq!(row[6], "0", "pooled lane 0 is the unknown placeholder");
        assert_eq!(row[7], "", "placeholder lane carries no marker");
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
                stale_providers: &[],
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
    fn errored_providers_keep_their_place_in_the_bar() {
        let config = RenderConfig {
            provider_order: vec!["gemini".into(), "broken".into(), "codex".into()],
            ..RenderConfig::default()
        };
        let payload = r#"[
            {"provider": "codex", "usage": {"primary": {"usedPercent": 10}}},
            {"provider": "broken", "error": {"message": "nope"}},
            {"provider": "gemini", "usage": {"primary": {"usedPercent": 20}}}
        ]"#;
        let rendered = emit(payload, &config, 1_700_000_000, options());
        let rows = lines(&rendered);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1][0], "gemini");
        assert_eq!(rows[2][0], "broken");
        assert_eq!(rows[3][0], "codex");

        let error_row = &rows[2];
        assert_eq!(error_row[1], "⚠err", "the label states the failure");
        assert_eq!(error_row.last().expect("error field"), "1");
        assert_eq!(error_row[5], "0", "an errored provider has no lane");
        assert_eq!(rows[1].last().expect("error field"), "0");
    }

    #[test]
    fn windowless_providers_keep_their_place_greyed() {
        // Muse Code while its server withholds quota: no error, no window.
        let config = RenderConfig {
            provider_order: vec!["codex".into(), "muse".into(), "cursor".into()],
            ..RenderConfig::default()
        };
        let payload = r#"[
            {"provider": "codex", "usage": {"primary": {"usedPercent": 10}}},
            {"provider": "muse", "error": null,
             "usage": {"primary": null, "secondary": null, "tertiary": null}},
            {"provider": "cursor", "usage": {"primary": {"usedPercent": 20}}}
        ]"#;
        let rendered = emit(payload, &config, 1_700_000_000, options());
        let rows = lines(&rendered);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[2][0], "muse", "it keeps its slot between its peers");
        assert_eq!(rows[2][1], "⚠err");
        assert_eq!(rows[2].last().expect("error field"), "1");
        assert_eq!(rows[2][5], "0", "no lane is drawn");
    }

    #[test]
    fn per_provider_stale_greys_only_that_row() {
        let config = RenderConfig::default();
        let payload = r#"[
            {"provider": "codex", "usage": {"primary": {"usedPercent": 10,
                "resetsAt": "2023-11-15T02:00:00Z", "windowMinutes": 300}}},
            {"provider": "claude", "usage": {"primary": {"usedPercent": 20,
                "resetsAt": "2023-11-15T02:00:00Z", "windowMinutes": 300}}}
        ]"#;
        let stale_providers = vec![String::from("claude")];
        let rendered = emit(
            payload,
            &config,
            1_700_000_000,
            SketchybarOptions {
                stale: false,
                degraded_cli: false,
                bar_width: BAR_W,
                stale_providers: &stale_providers,
            },
        );
        let rows = lines(&rendered);
        let stale_argb = argb(&config.palette_stale);
        assert_eq!(rows[1][0], "codex");
        assert_ne!(rows[1][8], stale_argb, "the fresh provider keeps its band");
        assert_ne!(rows[1][7], "", "and keeps its pacing marker");
        assert_eq!(rows[2][0], "claude");
        assert_eq!(rows[2][8], stale_argb);
        assert_eq!(rows[2][2], stale_argb, "countdown greys too");
        assert_eq!(rows[2][7], "", "a stale slice cannot place a marker");
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
    #[test]
    fn severity_glyphs_prefix_the_countdown_label() {
        let config = RenderConfig {
            severity_glyphs: true,
            ..RenderConfig::default()
        };
        let payload = r#"[
            {"provider":"codex","usage":{"primary":{"usedPercent":10}}},
            {"provider":"claude","usage":{"primary":{"usedPercent":70}}},
            {"provider":"copilot","usage":{"primary":{"usedPercent":95}}}
        ]"#;
        let rows = lines(&emit(payload, &config, 1_700_000_000, options()));
        assert_eq!(rows[1][1], "+?");
        assert_eq!(rows[2][1], "!?");
        assert_eq!(rows[3][1], "x?");
    }
}
