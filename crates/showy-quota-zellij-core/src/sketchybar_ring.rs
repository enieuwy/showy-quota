//! SketchyBar ring body: one ring per model family.
//!
//! Opt-in via `SHOWY_QUOTA_SKETCHYBAR_BODY=ring` (default `rows`, whose
//! behaviour, output, and tests are unchanged). The ring shows each family’s
//! longest window; shorter windows are horizontal bars under the label,
//! shortest first. Equal-length windows are breakdown parts of the ring’s own
//! window (Cursor’s Cursor / Third Party split): they draw as normal bars
//! with no pace knob. Extra pools (`extraRateWindows`) are omitted, except
//! Antigravity, which draws two pool units (G = Gemini, C = Claude + GPT).
//!
//! A pace value is the percent of time left in the window, computed from the
//! same reset/window math as the rows’ elapsed markers — only windows with a
//! usable reset carry one.

use serde_json::Value;
use std::collections::HashMap;

use crate::codexbar::{
    is_errored, is_renderable, valid_provider_id, ProviderRecord, MAX_USAGE_JSON_BYTES,
};
use crate::config::RenderConfig;
use crate::metrics::{error_kind, normalized_status_indicator, sanitize_error_message, ErrorKind};
use crate::providers::{font_icon, sigil};
use crate::render::{format_countdown, RenderError};
use crate::reset::{minutes_until, reset_clock};
use crate::sketchybar::{
    elapsed_marker_x, marker_percentage_from_x, passes_filters, sort_records, SketchybarOptions,
};

/// One window in a ring unit: the ring itself or one bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingWindow {
    /// Window name (`Weekly`, `Session`, `Total`, …).
    pub title: String,
    /// Window length in minutes, when CodexBar reports it.
    pub minutes: Option<i64>,
    /// Remaining quota percent, 0–100.
    pub remaining: i64,
    /// Percent of time left in the window. `None` draws no pace tick/knob.
    pub expected: Option<i64>,
    /// Raw reset string (`resetsAt // resetDescription // ""`).
    pub reset: String,
    /// Popup reset column (`2h 40m`, `idle`, `?`); the header says "resets in".
    pub reset_text: String,
    /// A part of the ring’s own window rather than a shorter window: no pace.
    pub breakdown: bool,
}

/// A provider CodexBar could not measure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingError {
    /// Strip label: `auth`, `net`, or the configured error glyph.
    pub kind_label: String,
    /// Sanitized `error.message`; empty when CodexBar withheld quota silently.
    pub message: String,
    /// Last-known ring-window remaining percent, when the cache kept usage.
    pub last_remaining: Option<i64>,
    /// Last-known usage clock (`HH:MM`), from `usage.updatedAt`.
    pub last_at: Option<String>,
}

/// A live provider whose status page reports an incident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingIncident {
    pub indicator: String,
    pub description: String,
    pub url: String,
}

/// Codex banked rate-limit resets (`usage.codexResetCredits`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingBanked {
    pub count: i64,
    pub note: String,
}

/// One ring on the strip: `provider`, or `provider.g` / `provider.c` pools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingUnit {
    /// Item id segment: the provider, or an Antigravity pool (`antigravity.g`).
    pub unit: String,
    pub provider: String,
    pub title: String,
    pub logo_glyph: String,
    pub logo_font: String,
    pub logo_pad: i64,
    pub logo_y: i64,
    /// Antigravity pool letter (`G` / `C`).
    pub pool: Option<char>,
    pub ring: RingWindow,
    /// Shorter windows, shortest first; at most two (three slots exist).
    pub bars: Vec<RingWindow>,
    /// Strip countdown label: the shortest window’s countdown.
    pub label: String,
    /// Countdown minutes behind the label, when the reset parses.
    pub label_minutes: Option<i64>,
    /// Title of the window the label counts down to.
    pub shortest_title: String,
    /// Grey note under the popup rows.
    pub note: String,
    /// Widest popup row name; the popup columns align to it (minimum 6).
    pub name_width: usize,
    /// Whole-strip or per-provider staleness: grey, no pace.
    pub stale: bool,
    pub error: Option<RingError>,
    pub incident: Option<RingIncident>,
    pub banked: Option<RingBanked>,
}

const SLOT_NAMES: [&str; 3] = ["primary", "secondary", "tertiary"];

/// Per-tick numbers every window assembly needs.
#[derive(Debug, Clone, Copy)]
struct TickCtx {
    now_epoch: i64,
    bar_width: i64,
    tz: Option<i16>,
    stale: bool,
}

/// The ring units for one tick, in pill order. Filters and sorts providers
/// exactly like the rows path; only the per-provider assembly differs.
pub fn ring_units(
    payload: &[u8],
    config: &RenderConfig,
    now_epoch: i64,
    options: SketchybarOptions,
) -> Result<Vec<RingUnit>, RenderError> {
    let indexed = parse_ring_payload(payload)?;
    let mut visible: Vec<&(ProviderRecord, Value)> = indexed
        .iter()
        .filter(|(record, _)| {
            (is_renderable(record) || is_errored(record)) && passes_filters(record, config)
        })
        .collect();
    {
        let mut records: Vec<&ProviderRecord> = visible.iter().map(|pair| &pair.0).collect();
        sort_records(&mut records, config);
        let order: Vec<String> = records
            .iter()
            .map(|record| record.provider.clone())
            .collect();
        visible.sort_by_key(|(record, _)| {
            order
                .iter()
                .position(|provider| provider == &record.provider)
                .unwrap_or(usize::MAX)
        });
    }
    let mut units = Vec::new();
    for (record, raw) in visible {
        units.extend(provider_units(record, raw, config, now_epoch, &options));
    }
    Ok(units)
}

/// Same transport filter as `parse_display_payload`, keeping each surviving
/// record’s raw element for the fields the typed record does not carry
/// (`rateWindowLabels`, `codexResetCredits`, `status.description`,
/// `usage.updatedAt`).
fn parse_ring_payload(payload: &[u8]) -> Result<Vec<(ProviderRecord, Value)>, RenderError> {
    if payload.len() > MAX_USAGE_JSON_BYTES {
        return Err(RenderError::InvalidPayload);
    }
    let value: Value = serde_json::from_slice(payload).map_err(|_| RenderError::InvalidPayload)?;
    let Value::Array(records) = value else {
        return Err(RenderError::InvalidPayload);
    };
    Ok(records
        .into_iter()
        .filter_map(|raw| {
            serde_json::from_value::<ProviderRecord>(raw.clone())
                .ok()
                .filter(|record| valid_provider_id(&record.provider))
                .map(|record| (record, raw))
        })
        .collect())
}

fn provider_units(
    record: &ProviderRecord,
    raw: &Value,
    config: &RenderConfig,
    now_epoch: i64,
    options: &SketchybarOptions,
) -> Vec<RingUnit> {
    let stale = options.stale_for(&record.provider);
    let tz = config.reset_description_timezone_offset_minutes;
    let bar_width = options.bar_width.clamp(2, 4_096);
    if is_errored(record) {
        return vec![error_unit(record, raw, config, now_epoch, stale)];
    }
    if record.provider == "antigravity" {
        let pools = antigravity_units(record, raw, config, now_epoch, bar_width, tz, stale);
        if !pools.is_empty() {
            return pools;
        }
    }
    let tick = TickCtx {
        now_epoch,
        bar_width,
        tz,
        stale,
    };
    vec![family_unit(
        &record.provider,
        &display_name(&record.provider),
        None,
        positional_windows(record, raw),
        record,
        raw,
        config,
        tick,
    )]
}

/// Positional windows with a numeric `usedPercent`, in slot order.
fn positional_windows(record: &ProviderRecord, raw: &Value) -> Vec<RawWindow> {
    let labels = string_map(raw.get("rateWindowLabels"));
    let usage = match record.usage.as_ref() {
        Some(usage) => usage,
        None => return Vec::new(),
    };
    let slots = [
        usage.primary.as_ref(),
        usage.secondary.as_ref(),
        usage.tertiary.as_ref(),
    ];
    SLOT_NAMES
        .iter()
        .zip(slots)
        .filter_map(|(slot, window)| {
            let window = window?;
            let used = window.used_percent?;
            Some(RawWindow {
                title: labels
                    .get(*slot)
                    .cloned()
                    .unwrap_or_else(|| capitalize(slot)),
                minutes: window.window_minutes(),
                used,
                reset: window.reset_value().unwrap_or_default().to_owned(),
                unknown: false,
            })
        })
        .collect()
}

struct RawWindow {
    title: String,
    minutes: Option<i64>,
    used: f64,
    reset: String,
    /// A `usageKnown:false` placeholder: drawn full with no pace.
    unknown: bool,
}

/// Eight arguments, each used: identity, windows, and tick numbers stay
/// separate rather than hiding in one bag struct.
#[allow(clippy::too_many_arguments)]
fn family_unit(
    unit: &str,
    title: &str,
    pool: Option<char>,
    raws: Vec<RawWindow>,
    record: &ProviderRecord,
    raw: &Value,
    config: &RenderConfig,
    tick: TickCtx,
) -> RingUnit {
    // The ring is the longest window; ties keep slot order. Bars are the
    // rest, shortest first; windows as long as the ring are breakdown parts.
    let ring_index = (0..raws.len()).max_by(|&a, &b| {
        raws[a]
            .minutes
            .cmp(&raws[b].minutes)
            .then_with(|| b.cmp(&a))
    });
    let mut order: Vec<usize> = (0..raws.len()).collect();
    order.sort_by(|&a, &b| {
        raws[a]
            .minutes
            .cmp(&raws[b].minutes)
            .then_with(|| a.cmp(&b))
    });
    let ring_minutes = ring_index.and_then(|index| raws.get(index).and_then(|raw| raw.minutes));
    let bars: Vec<RingWindow> = order
        .into_iter()
        .filter(|index| Some(*index) != ring_index)
        .map(|index| {
            let breakdown = raws[index].minutes == ring_minutes;
            assemble(&raws[index], breakdown, tick)
        })
        .take(2)
        .collect();
    // A lone window draws the ring only; a missing window draws nothing.
    let ring = match ring_index.and_then(|index| raws.get(index)) {
        Some(raw) => assemble(raw, false, tick),
        None => RingWindow {
            title: String::new(),
            minutes: None,
            remaining: 0,
            expected: None,
            reset: String::new(),
            reset_text: "idle".into(),
            breakdown: false,
        },
    };
    let shortest_title;
    let shortest_remaining;
    let (label, label_minutes) = {
        let shortest = bars.first().unwrap_or(&ring);
        shortest_title = shortest.title.clone();
        shortest_remaining = shortest.remaining;
        countdown(shortest, tick.now_epoch, tick.tz)
    };
    let label = if config.severity_glyphs && !tick.stale {
        format!(
            "{}{label}",
            config.severity(shortest_remaining as i32).marker()
        )
    } else {
        label
    };
    let note = unit_note(
        &label,
        &shortest_title,
        ring.remaining == 0,
        bars.is_empty(),
    );
    let name_width = std::iter::once(ring.title.len())
        .chain(bars.iter().map(|bar| bar.title.len()))
        .max()
        .unwrap_or(0)
        .max(6);
    let (logo_glyph, logo_font, logo_pad, logo_y) = logo_for(&record.provider);
    // Antigravity pools share the marker with their letter badge, so the
    // logo sits 1.5 pt left of centre (padding 3 below app_pad) and the
    // logo-plus-letter pair reads centred.
    let logo_pad = if pool.is_some() {
        (logo_pad - 3).max(0)
    } else {
        logo_pad
    };

    RingUnit {
        unit: unit.to_owned(),
        provider: record.provider.clone(),
        title: title.to_owned(),
        logo_glyph,
        logo_font,
        logo_pad,
        logo_y,
        pool,
        ring,
        bars,
        label,
        label_minutes,
        shortest_title,
        note,
        name_width,
        stale: tick.stale,
        error: None,
        incident: incident_for(record, raw),
        banked: banked_for(raw, tick.now_epoch, tick.tz),
    }
}

fn assemble(raw: &RawWindow, breakdown: bool, tick: TickCtx) -> RingWindow {
    let remaining = if raw.unknown {
        100
    } else {
        (100 - raw.used.floor().clamp(0.0, 100.0) as i64).clamp(0, 100)
    };
    let expected = if tick.stale || breakdown || raw.unknown {
        None
    } else {
        expected_percent(
            &raw.reset,
            raw.minutes,
            tick.now_epoch,
            tick.bar_width,
            tick.tz,
        )
    };
    RingWindow {
        title: sanitize(&raw.title),
        minutes: raw.minutes,
        remaining,
        expected,
        reset: raw.reset.clone(),
        reset_text: reset_text(&raw.reset, tick.now_epoch, tick.tz),
        breakdown,
    }
}

/// Percent of time left in the window, from the same reset/window math as the
/// rows’ elapsed markers. `None` draws no pace tick or knob.
fn expected_percent(
    reset: &str,
    minutes: Option<i64>,
    now_epoch: i64,
    bar_width: i64,
    tz: Option<i16>,
) -> Option<i64> {
    let win = minutes.filter(|minutes| *minutes > 0)?.to_string();
    let marker = elapsed_marker_x(reset, &win, now_epoch, bar_width, tz)?;
    marker_percentage_from_x(marker, bar_width)
}

fn reset_text(reset: &str, now_epoch: i64, tz: Option<i16>) -> String {
    match minutes_until(reset, now_epoch, tz) {
        // The popup header already says "resets in"; the cell is the duration.
        Some(minutes) => friendly_duration(minutes),
        None if reset.is_empty() => "idle".into(),
        None => "?".into(),
    }
}

fn countdown(window: &RingWindow, now_epoch: i64, tz: Option<i16>) -> (String, Option<i64>) {
    match minutes_until(&window.reset, now_epoch, tz) {
        Some(minutes) => (format_countdown(minutes), Some(minutes)),
        None if window.reset.is_empty() && window.remaining >= 100 => ("idle".into(), None),
        None => ("?".into(), None),
    }
}

fn unit_note(label: &str, shortest_title: &str, ring_empty: bool, has_bars: bool) -> String {
    if label == "idle" {
        return format!("idle = the {shortest_title} window has not started");
    }
    let base = if has_bars {
        format!("{label} = time until the bar resets")
    } else {
        format!("{label} = time until the ring resets")
    };
    if ring_empty {
        format!("red track = pool empty · {base}")
    } else {
        base
    }
}

/// Antigravity’s two pools from `extraRateWindows`: Gemini versus the rest.
/// Each pool with a measured window becomes one unit; the pool letter is the
/// `ring.marker.badge`.
fn antigravity_units(
    record: &ProviderRecord,
    raw: &Value,
    config: &RenderConfig,
    now_epoch: i64,
    bar_width: i64,
    tz: Option<i16>,
    stale: bool,
) -> Vec<RingUnit> {
    let usage = match record.usage.as_ref() {
        Some(usage) => usage,
        None => return Vec::new(),
    };
    let mut gemini: Vec<RawWindow> = Vec::new();
    let mut other: Vec<RawWindow> = Vec::new();
    for extra in &usage.extra_rate_windows {
        let window = match extra.window.as_ref() {
            Some(window) => window,
            None => continue,
        };
        let used = match window.used_percent {
            Some(used) => used,
            None => continue,
        };
        let haystack = format!(
            "{} {}",
            extra.id.as_deref().unwrap_or_default(),
            extra.title.as_deref().unwrap_or_default()
        );
        let target = if haystack.to_lowercase().contains("gemini") {
            &mut gemini
        } else {
            &mut other
        };
        target.push(RawWindow {
            title: sanitize(extra.title.as_deref().unwrap_or_default()),
            minutes: window.window_minutes(),
            used,
            reset: window.reset_value().unwrap_or_default().to_owned(),
            unknown: extra.usage_known == Some(false),
        });
    }
    // Untitled extras still name their pool; fall back to the pool noun.
    for (windows, fallback) in [(&mut gemini, "Gemini"), (&mut other, "Claude+GPT")] {
        for window in windows.iter_mut() {
            if window.title.is_empty() {
                window.title = fallback.to_owned();
            }
        }
    }
    [
        ("g", 'G', "Antigravity · G = Gemini pool", gemini),
        ("c", 'C', "Antigravity · C = Claude + GPT pool", other),
    ]
    .into_iter()
    .filter(|(_, _, _, windows)| !windows.is_empty())
    .map(|(suffix, pool, title, windows)| {
        family_unit(
            &format!("{}.{}", record.provider, suffix),
            title,
            Some(pool),
            windows,
            record,
            raw,
            config,
            TickCtx {
                now_epoch,
                bar_width,
                tz,
                stale,
            },
        )
    })
    .collect()
}

fn error_unit(
    record: &ProviderRecord,
    raw: &Value,
    config: &RenderConfig,
    now_epoch: i64,
    stale: bool,
) -> RingUnit {
    let message = record
        .error
        .as_ref()
        .map(sanitize_error_message)
        .unwrap_or_default();
    let kind_label = if message.is_empty() {
        config.error_glyph.clone()
    } else {
        match error_kind(&message) {
            ErrorKind::Auth | ErrorKind::Cookies => "auth".into(),
            ErrorKind::Network => "net".into(),
            ErrorKind::Unknown => config.error_glyph.clone(),
        }
    };
    // The fetch keeps the previous cache’s usage beside a fresh error, so an
    // errored provider can still show its last-known arc, dimmed grey. The
    // arc is the longest window’s, like the live ring.
    let last_remaining = record.usage.as_ref().and_then(|usage| {
        [
            usage.primary.as_ref(),
            usage.secondary.as_ref(),
            usage.tertiary.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter(|window| window.used_percent.is_some())
        .max_by(|a, b| {
            a.window_minutes().cmp(&b.window_minutes()).then_with(|| {
                b.used_percent
                    .partial_cmp(&a.used_percent)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        })
        .and_then(|window| window.used_percent)
        .map(|used| (100 - used.floor().clamp(0.0, 100.0) as i64).clamp(0, 100))
    });
    let last_at = last_remaining.and_then(|_| {
        raw.get("usage")
            .and_then(|usage| usage.get("updatedAt"))
            .and_then(|at| at.as_str())
            .and_then(|at| {
                reset_clock(
                    at,
                    now_epoch,
                    config.reset_description_timezone_offset_minutes,
                )
            })
    });
    let note = match (last_remaining, &last_at) {
        (Some(remaining), Some(at)) => format!("last known {remaining}% at {at}"),
        (Some(remaining), None) => format!("last known {remaining}%"),
        _ => "no quota data · click opens CodexBar".into(),
    };
    let (logo_glyph, logo_font, logo_pad, logo_y) = logo_for(&record.provider);
    RingUnit {
        unit: record.provider.clone(),
        provider: record.provider.clone(),
        title: display_name(&record.provider),
        logo_glyph,
        logo_font,
        logo_pad,
        logo_y,
        pool: None,
        ring: RingWindow {
            title: String::new(),
            minutes: None,
            remaining: last_remaining.unwrap_or(0),
            expected: None,
            reset: String::new(),
            reset_text: "idle".into(),
            breakdown: false,
        },
        bars: Vec::new(),
        label: kind_label.clone(),
        label_minutes: None,
        shortest_title: String::new(),
        note,
        name_width: 6,
        stale,
        error: Some(RingError {
            kind_label,
            message,
            last_remaining,
            last_at,
        }),
        incident: incident_for(record, raw),
        banked: None,
    }
}

fn incident_for(record: &ProviderRecord, raw: &Value) -> Option<RingIncident> {
    let indicator = normalized_status_indicator(record)?;
    let description = raw
        .get("status")
        .and_then(|status| status.get("description"))
        .and_then(|description| description.as_str())
        .map(sanitize)
        .filter(|description| !description.is_empty())
        .unwrap_or_else(|| indicator.clone());
    let url = raw
        .get("status")
        .and_then(|status| status.get("url"))
        .and_then(|url| url.as_str())
        .map(sanitize)
        .unwrap_or_default();
    Some(RingIncident {
        indicator,
        description,
        url,
    })
}

fn banked_for(raw: &Value, now_epoch: i64, tz: Option<i16>) -> Option<RingBanked> {
    let credits = raw.get("usage")?.get("codexResetCredits")?;
    let count = credits.get("availableCount")?.as_i64()?;
    if count <= 0 {
        return None;
    }
    let plural = if count == 1 { "reset" } else { "resets" };
    let next = credits
        .get("credits")
        .and_then(Value::as_array)
        .and_then(|credits| {
            credits
                .iter()
                .filter_map(|credit| credit.get("expires_at").and_then(Value::as_str))
                .filter_map(|expires| minutes_until(expires, now_epoch, tz).map(|m| (m, expires)))
                .min_by_key(|(minutes, _)| *minutes)
        });
    let note = match next {
        Some((minutes, expires)) => match expiry_day(expires, now_epoch, tz) {
            Some(day) => format!(
                "{count} free {plural} banked · next expires in {} ({day})",
                friendly_duration(minutes),
            ),
            None => format!(
                "{count} free {plural} banked · next expires in {}",
                friendly_duration(minutes),
            ),
        },
        None => format!("{count} free {plural} banked"),
    };
    Some(RingBanked { count, note })
}

/// `5 Oct` for an expiry timestamp, in UTC so the date never depends on the
/// host timezone. Empty when the timestamp does not parse.
fn expiry_day(expires: &str, now_epoch: i64, tz: Option<i16>) -> Option<String> {
    let minutes = minutes_until(expires, now_epoch, tz)?;
    let epoch = now_epoch + minutes * 60;
    let date = time::OffsetDateTime::from_unix_timestamp(epoch)
        .ok()?
        .date();
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    Some(format!(
        "{} {}",
        date.day(),
        MONTHS
            .get(date.month() as usize - 1)
            .copied()
            .unwrap_or("?")
    ))
}

/// `2h 40m` / `6d 11h`, the popup duration shape.
pub(crate) fn friendly_duration(minutes: i64) -> String {
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 1440 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{}d {}h", minutes / 1440, minutes % 1440 / 60)
    }
}

/// `30d` / `7d` / `5h` window-length labels.
pub(crate) fn friendly_length(minutes: Option<i64>) -> String {
    match minutes {
        Some(minutes) if minutes > 0 && minutes % 1440 == 0 => format!("{}d", minutes / 1440),
        Some(minutes) if minutes > 0 && minutes % 60 == 0 => format!("{}h", minutes / 60),
        Some(minutes) if minutes > 0 => format!("{minutes}m"),
        _ => "?".into(),
    }
}

fn display_name(provider: &str) -> String {
    match provider {
        "codex" => "Codex".into(),
        "claude" => "Claude".into(),
        "antigravity" => "Antigravity".into(),
        "cursor" => "Cursor".into(),
        "muse" => "Muse".into(),
        "copilot" => "Copilot".into(),
        "commandcode" => "Command Code".into(),
        _ => capitalize(provider),
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Provider logo: the app-font glyph centred in the ring, or the SF Pro
/// glyphs the demo measured (`∞` high, `⌘` left-nudged), else the sigil.
fn logo_for(provider: &str) -> (String, String, i64, i64) {
    match provider {
        "muse" => ("∞".into(), "SF Pro:Bold:13.0".into(), 0, 1),
        "commandcode" => ("⌘".into(), "SF Pro:Bold:11.0".into(), 1, 0),
        _ => match font_icon(provider) {
            Some(glyph) => (
                glyph.to_owned(),
                "sketchybar-app-font:Regular:12.0".into(),
                app_font_pad(12.0),
                0,
            ),
            None => (
                sigil(provider).unwrap_or("·").to_owned(),
                "SF Pro:Bold:12.0".into(),
                0,
                0,
            ),
        },
    }
}

/// Centring nudge for an app-font glyph at `size` points: the ink sits ~0.21
/// em left of the advance-box centre (plus half of SketchyBar’s `ceil()` of
/// the 1.411 em advance), and `padding_left` moves it right by half.
pub(crate) fn app_font_pad(size: f64) -> i64 {
    let advance = 1.411 * size;
    (2.0 * (0.206 * size + (advance.ceil() - advance) / 2.0)).round() as i64
}

fn string_map(value: Option<&Value>) -> HashMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), sanitize(text))))
                .collect()
        })
        .unwrap_or_default()
}

fn sanitize(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_786_118_400; // 2026-08-04T00:00:00Z

    fn config() -> RenderConfig {
        RenderConfig::default()
    }

    fn utc_config() -> RenderConfig {
        RenderConfig {
            reset_description_timezone_offset_minutes: Some(0),
            ..Default::default()
        }
    }

    fn options() -> SketchybarOptions<'static> {
        SketchybarOptions {
            stale: false,
            degraded_cli: false,
            bar_width: 80,
            stale_providers: &[],
        }
    }

    fn units(payload: &str) -> Vec<RingUnit> {
        ring_units(payload.as_bytes(), &config(), NOW, options()).expect("units")
    }

    fn reset_at(minutes_from_now: i64) -> String {
        time::OffsetDateTime::from_unix_timestamp(NOW + minutes_from_now * 60)
            .expect("epoch")
            .format(time::macros::format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
            ))
            .expect("format")
    }

    #[test]
    fn the_ring_is_the_longest_window_and_bars_run_shortest_first() {
        let payload = format!(
            r#"[{{"provider":"commandcode","usage":{{
                "primary":{{"usedPercent":0.0,"windowMinutes":300}},
                "secondary":{{"usedPercent":45.0,"resetsAt":"{}","windowMinutes":10080}},
                "tertiary":{{"usedPercent":52.0,"resetsAt":"{}","windowMinutes":43200}}}}}}]"#,
            reset_at(160),
            reset_at(20_000)
        );
        let units = units(&payload);
        assert_eq!(units.len(), 1);
        let unit = &units[0];
        assert_eq!(unit.unit, "commandcode");
        assert_eq!(unit.ring.minutes, Some(43200));
        assert_eq!(unit.ring.remaining, 48);
        assert_eq!(unit.bars.len(), 2);
        assert_eq!(unit.bars[0].minutes, Some(300));
        assert_eq!(unit.bars[1].minutes, Some(10080));
        assert!(!unit.bars.iter().any(|bar| bar.breakdown));
        // The label counts down to the shortest window, which never started.
        assert_eq!(unit.label, "idle");
    }

    #[test]
    fn a_missing_window_draws_nothing_and_the_label_stays_centred() {
        let payload = format!(
            r#"[{{"provider":"codex","usage":{{
                "primary":{{}},
                "secondary":{{"usedPercent":30.0,"resetsAt":"{}","windowMinutes":10080}},
                "tertiary":{{}}}}}}]"#,
            reset_at(9349)
        );
        let units = units(&payload);
        assert_eq!(units.len(), 1);
        assert!(units[0].bars.is_empty());
        assert_eq!(units[0].ring.minutes, Some(10080));
        assert_eq!(units[0].ring.remaining, 70);
    }

    #[test]
    fn equal_length_windows_are_breakdown_parts_with_no_pace() {
        let payload = format!(
            r#"[{{"provider":"cursor","usage":{{
                "primary":{{"usedPercent":43.0,"resetsAt":"{}","windowMinutes":43200}},
                "secondary":{{"usedPercent":45.0,"resetsAt":"{}","windowMinutes":43200}},
                "tertiary":{{"usedPercent":41.0,"resetsAt":"{}","windowMinutes":43200}}}}}}]"#,
            reset_at(20_000),
            reset_at(20_000),
            reset_at(20_000)
        );
        let units = units(&payload);
        assert_eq!(units.len(), 1);
        let unit = &units[0];
        assert_eq!(unit.ring.title, "Primary");
        assert_eq!(unit.bars.len(), 2);
        assert!(unit.bars.iter().all(|bar| bar.breakdown));
        assert!(unit.bars.iter().all(|bar| bar.expected.is_none()));
        assert!(unit.ring.expected.is_some());
    }

    #[test]
    fn antigravity_draws_one_unit_per_pool() {
        let payload = format!(
            r#"[{{"provider":"antigravity","usage":{{
                "primary":{{"usedPercent":87.0,"resetsAt":"{}","windowMinutes":300}},
                "secondary":{{"usedPercent":100.0,"resetsAt":"{}","windowMinutes":10080}},
                "extraRateWindows":[
                    {{"id":"antigravity-quota-summary-gemini-5h","title":"Gemini 5-hour",
                      "window":{{"usedPercent":87.0,"resetsAt":"{}","windowMinutes":300}}}},
                    {{"id":"antigravity-quota-summary-gemini-weekly","title":"Gemini weekly",
                      "window":{{"usedPercent":70.0,"resetsAt":"{}","windowMinutes":10080}}}},
                    {{"id":"antigravity-quota-summary-3p-5h","title":"Claude/GPT 5-hour","usageKnown":false,
                      "window":{{"usedPercent":0.0,"windowMinutes":300}}}},
                    {{"id":"antigravity-quota-summary-3p-weekly","title":"Claude/GPT weekly",
                      "window":{{"usedPercent":100.0,"resetsAt":"{}","windowMinutes":10080}}}}
                ]}}}}]"#,
            reset_at(109),
            reset_at(1085),
            reset_at(109),
            reset_at(8000),
            reset_at(1085)
        );
        let units = units(&payload);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].unit, "antigravity.g");
        assert_eq!(units[0].pool, Some('G'));
        assert_eq!(units[0].logo_pad, 2);
        assert_eq!(units[0].ring.remaining, 30);
        assert_eq!(units[0].bars.len(), 1);
        assert_eq!(units[0].bars[0].remaining, 13);
        assert_eq!(units[1].unit, "antigravity.c");
        assert_eq!(units[1].pool, Some('C'));
        assert_eq!(units[1].ring.remaining, 0);
        // The unknown 5h pool draws full with no pace.
        assert_eq!(units[1].bars.len(), 1);
        assert_eq!(units[1].bars[0].remaining, 100);
        assert_eq!(units[1].bars[0].expected, None);
    }

    #[test]
    fn an_error_without_last_known_usage_draws_the_kind_label() {
        let units =
            units(r#"[{"provider":"cursor","error":{"message":"Safari cookies not readable."}}]"#);
        assert_eq!(units.len(), 1);
        let unit = &units[0];
        let error = unit.error.as_ref().expect("error");
        assert_eq!(error.kind_label, "auth");
        assert_eq!(unit.label, "auth");
        assert!(unit.bars.is_empty());
        assert_eq!(error.last_remaining, None);
        assert!(unit.note.contains("no quota data"));
    }

    #[test]
    fn an_error_with_last_known_usage_keeps_the_arc_and_its_clock() {
        let payload = format!(
            r#"[{{"provider":"claude","error":{{"message":"connect timed out"}},
                "usage":{{"secondary":{{"usedPercent":27.0,"resetsAt":"{}","windowMinutes":10080}},
                           "updatedAt":"2026-08-03T21:40:00Z"}}}}]"#,
            reset_at(860)
        );
        let units = ring_units(payload.as_bytes(), &utc_config(), NOW, options()).expect("units");
        let error = units[0].error.as_ref().expect("error");
        assert_eq!(error.kind_label, "net");
        assert_eq!(error.last_remaining, Some(73));
        assert_eq!(error.last_at.as_deref(), Some("21:40"));
        assert!(units[0].note.contains("last known 73% at 21:40"));
    }

    #[test]
    fn an_incident_tints_the_unit_and_reports_the_status_page() {
        let payload = format!(
            r#"[{{"provider":"codex","status":{{"indicator":"minor","description":"Elevated error rates","url":"https://status.openai.com/"}},
                "usage":{{"secondary":{{"usedPercent":30.0,"resetsAt":"{}","windowMinutes":10080}}}}}}]"#,
            reset_at(9349)
        );
        let units = units(&payload);
        let incident = units[0].incident.as_ref().expect("incident");
        assert_eq!(incident.indicator, "minor");
        assert_eq!(incident.description, "Elevated error rates");
        assert_eq!(incident.url, "https://status.openai.com/");
    }

    #[test]
    fn banked_resets_report_the_count_and_next_expiry() {
        let payload = format!(
            r#"[{{"provider":"codex","usage":{{
                "secondary":{{"usedPercent":30.0,"resetsAt":"{}","windowMinutes":10080}},
                "codexResetCredits":{{"availableCount":2,"credits":[
                    {{"expires_at":"2026-10-05T04:20:20Z","status":"available"}},
                    {{"expires_at":"2026-10-22T20:47:00Z","status":"available"}}]}}}}}}]"#,
            reset_at(9349)
        );
        let units = units(&payload);
        let banked = units[0].banked.as_ref().expect("banked");
        assert_eq!(banked.count, 2);
        assert!(
            banked.note.contains("2 free resets banked"),
            "{}",
            banked.note
        );
        assert!(banked.note.contains("(5 Oct)"), "{}", banked.note);
    }

    #[test]
    fn app_font_logos_centre_on_their_ink() {
        assert_eq!(app_font_pad(12.0), 5);
        assert_eq!(app_font_pad(10.0), 5);
        let payload = format!(
            r#"[{{"provider":"codex","usage":{{"secondary":{{"usedPercent":30.0,"resetsAt":"{}","windowMinutes":10080}}}}}}]"#,
            reset_at(9349)
        );
        let units = units(&payload);
        assert_eq!(units[0].logo_glyph, ":codex:");
        assert_eq!(units[0].logo_pad, 5);
    }
}
