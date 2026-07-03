use std::borrow::Cow;
use std::cmp::Ordering;

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Duration, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

use crate::codexbar::{
    is_renderable, parse_usage_payload, NamedWindow, ProviderRecord, Usage, UsageWindow,
};
use crate::config::RenderConfig;
use crate::palette::hex_to_rgb;

#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    pub color: bool,
    pub stale: bool,
    pub degraded_cli: bool,
    pub now_epoch: i64,
}

#[derive(Debug)]
pub enum RenderError {
    InvalidPayload,
}

pub fn render_zellij(
    payload: &[u8],
    config: &RenderConfig,
    options: RenderOptions,
) -> Result<String, RenderError> {
    let records = parse_usage_payload(payload).map_err(|_| RenderError::InvalidPayload)?;
    Ok(render_records(&records, config, options))
}

fn render_records(
    records: &[ProviderRecord],
    config: &RenderConfig,
    options: RenderOptions,
) -> String {
    let mut records: Vec<&ProviderRecord> = records
        .iter()
        .filter(|record| is_renderable(record))
        .collect();
    filter_and_sort(&mut records, config);

    let chunk_bg = &config.palette_bg;
    let stale_color = &config.palette_stale;
    let countdown_warn = &config.palette_countdown_warn;
    let mut out = String::new();

    if records.is_empty() {
        dim(&mut out, options.color);
        out.push_str("AI idle");
        reset(&mut out, options.color);
    } else {
        // Model-pooled providers (auto-detected `dual2`) are split into one
        // synthetic per-family `dual` provider each (`AGᴳ`, `AGᶜ`); everything
        // else renders as-is. Each unit then flows through the normal dual path.
        let mut units: Vec<(Cow<ProviderRecord>, String)> = Vec::new();
        for record in records {
            match expand_pooled(record, config) {
                Some(families) => {
                    for (sigil, synthetic) in families {
                        units.push((Cow::Owned(synthetic), sigil));
                    }
                }
                None => units.push((Cow::Borrowed(record), provider_sigil(&record.provider))),
            }
        }
        for (idx, (record, sigil)) in units.iter().enumerate() {
            if idx > 0 {
                out.push(' ');
            }
            render_provider(
                &mut out,
                record,
                sigil,
                config,
                options,
                chunk_bg,
                stale_color,
            );
        }
    }

    if options.stale {
        out.push(' ');
        style_text(
            &mut out,
            &config.stale_glyph,
            Some(countdown_warn),
            Some(chunk_bg),
            Weight::Bold,
            options.color,
        );
    }
    if options.degraded_cli {
        out.push(' ');
        style_text(
            &mut out,
            &config.degraded_cli_glyph,
            Some(countdown_warn),
            Some(chunk_bg),
            Weight::Bold,
            options.color,
        );
    }
    out.push('\n');
    out
}

fn filter_and_sort(records: &mut Vec<&ProviderRecord>, config: &RenderConfig) {
    records.retain(|record| {
        (config.providers.is_empty() || contains(&config.providers, &record.provider))
            && !contains(&config.providers_exclude, &record.provider)
    });

    if !config.providers.is_empty() {
        let allow = &config.providers;
        records.sort_by(|a, b| provider_cmp(a, b, allow));
    } else if !config.provider_order.is_empty() {
        let order = &config.provider_order;
        records.sort_by(|a, b| provider_cmp(a, b, order));
    }
}

fn provider_cmp(a: &ProviderRecord, b: &ProviderRecord, order: &[String]) -> Ordering {
    let a_pos = position(order, &a.provider);
    let b_pos = position(order, &b.provider);
    a_pos.cmp(&b_pos).then_with(|| a.provider.cmp(&b.provider))
}

fn position(items: &[String], provider: &str) -> usize {
    items
        .iter()
        .position(|item| item == provider)
        .unwrap_or(1_000_000)
}

fn contains(items: &[String], provider: &str) -> bool {
    items.iter().any(|item| item == provider)
}

fn render_provider(
    out: &mut String,
    record: &ProviderRecord,
    sigil: &str,
    config: &RenderConfig,
    options: RenderOptions,
    chunk_bg: &str,
    stale_color: &str,
) {
    let usage = record
        .usage
        .as_ref()
        .expect("renderable provider has usage");
    // Slots are semantic: primary/secondary/tertiary map to fixed rows and
    // roles. A slot only counts when that exact window reports a numeric
    // usedPercent; missing slots stay missing (used = -1, remaining = 0)
    // instead of later windows shifting up.
    let primary = semantic_slot(&usage.primary);
    let secondary = semantic_slot(&usage.secondary);
    let tertiary = semantic_slot(&usage.tertiary);

    // Cursor-style shared-cycle pools (Total/Auto/API) report one resetsAt and
    // windowMinutes across their slots: parallel usage categories within a
    // single monthly budget, not a live tier over a longer cap. Keep them at
    // full brightness and draw a single pacing marker instead of dimming every
    // row and repeating the identical marker.
    let shared = shared_cycle(&[primary, secondary, tertiary]);

    let p_used = primary.map_or(-1, UsageWindow::used_pct_floor);
    let s_used = secondary.map_or(-1, UsageWindow::used_pct_floor);
    let p_remaining = if p_used >= 0 { 100 - p_used } else { 0 };
    let s_remaining = if s_used >= 0 { 100 - s_used } else { 0 };

    let p_reset = primary.and_then(UsageWindow::reset_value);
    let minutes = p_reset.and_then(|reset| {
        minutes_until(
            reset,
            options.now_epoch,
            config.reset_description_timezone_offset_minutes,
        )
    });
    let minutes = if options.stale && minutes == Some(0) {
        p_reset
            .and_then(|reset| {
                reset_epoch(
                    reset,
                    options.now_epoch,
                    config.reset_description_timezone_offset_minutes,
                )
            })
            .filter(|epoch| *epoch > options.now_epoch)
            .map(|_| 0)
    } else {
        minutes
    };
    let countdown = if primary.is_some() {
        primary_label(minutes, p_remaining, p_reset)
    } else {
        // No primary window at all (e.g. Antigravity): nothing consumed and
        // nothing to count down, which is the existing "idle" contract.
        String::from("idle")
    };

    let time_color = if options.stale {
        config.palette_stale.as_str()
    } else if minutes.is_some_and(|m| m < config.time_warn_minutes) {
        config.palette_countdown_warn.as_str()
    } else {
        config.palette_countdown.as_str()
    };

    let surface_color = &config.palette_surface;
    let p_long = !shared && is_long_window(primary, config.dim_window_minutes);
    let s_long = !shared && is_long_window(secondary, config.dim_window_minutes);
    let slots = [primary, secondary, tertiary];
    let assembled_windows = distinct_render_windows(&slots, &usage.extra_rate_windows, 4);
    let bar_mode = terminal_mode_for_provider(
        config,
        &record.provider,
        tertiary.is_some(),
        pooled_auto(&slots, &usage.extra_rate_windows),
        assembled_windows.len(),
    );
    // Only mono4 still assembles per-pool family lanes here; dual2 pooled
    // providers are pre-expanded into standalone dual records upstream.
    let families = if bar_mode == "mono4" {
        pool_families(&slots, &usage.extra_rate_windows, config, options.stale)
    } else {
        Vec::new()
    };
    let mut primary_color = config.window_color(p_remaining, p_long);
    let mut secondary_color = config.window_color(s_remaining, s_long);

    // Lanes for the single-color stacked bodies. mono3 uses the three positional
    // slots (absent slots stay empty and never shift up); mono4 uses the
    // assembled per-pool windows.
    let mut mono_lanes: Vec<Lane> = match bar_mode.as_str() {
        "mono4" if assembled_windows.len() >= 4 => {
            let family_lanes: Vec<Lane> = families
                .iter()
                .take(2)
                .flat_map(|family| [family.top, family.bottom])
                .collect();
            if family_lanes.iter().filter(|lane| lane.present).count() >= 4 {
                family_lanes
            } else {
                assembled_windows
                    .iter()
                    .take(4)
                    .map(|window| Lane::from_window(window, config, options.stale))
                    .collect()
            }
        }
        "mono3" if tertiary.is_none() && assembled_windows.len() >= 3 => assembled_windows
            .iter()
            .take(3)
            .map(|window| Lane::from_window(window, config, options.stale))
            .collect(),
        "mono3" => [primary, secondary, tertiary]
            .into_iter()
            .map(|slot| Lane::from_slot(slot, config, options.stale))
            .collect(),
        _ => Vec::new(),
    };
    if shared {
        for lane in &mut mono_lanes {
            lane.is_long = false;
        }
    }
    let mut mono_color = if mono_lanes.is_empty() {
        String::new()
    } else {
        let color = mono_chunk_color(config, &mono_lanes);
        primary_color.clone_from(&color);
        color
    };
    if options.stale {
        primary_color = stale_color.to_string();
        secondary_color = stale_color.to_string();
        mono_color = stale_color.to_string();
    }

    let separator_fg = chunk_bg;
    let separator_bg = &primary_color;
    style_text(
        out,
        &config.cap_left,
        Some(&primary_color),
        Some(chunk_bg),
        Weight::Normal,
        options.color,
    );
    style_text(
        out,
        sigil,
        Some(chunk_bg),
        Some(&primary_color),
        Weight::Bold,
        options.color,
    );
    style_text(
        out,
        "▕",
        Some(separator_fg),
        Some(separator_bg),
        Weight::Normal,
        options.color,
    );

    let marker_primary_reset = if options.stale { None } else { p_reset };
    let marker_primary_window = if options.stale {
        None
    } else {
        primary.and_then(UsageWindow::window_minutes)
    };
    let marker_secondary_reset = if options.stale || shared {
        None
    } else {
        secondary.and_then(UsageWindow::reset_value)
    };
    let marker_secondary_window = if options.stale || shared {
        None
    } else {
        secondary.and_then(UsageWindow::window_minutes)
    };
    let width = config.zellij_bar_width.clamp(8, 400);
    if !mono_lanes.is_empty() {
        let markers = mono_marker_cells(config, &mono_lanes, width, options.now_epoch);
        mono_lane_bar(out, config, options, &mono_lanes, &mono_color, &markers);
    } else {
        dual_metric_bar(
            out,
            config,
            options,
            DualArgs {
                p_remaining,
                s_remaining,
                p_reset: marker_primary_reset,
                p_window: marker_primary_window,
                s_reset: marker_secondary_reset,
                s_window: marker_secondary_window,
                primary_color: &primary_color,
                secondary_color: &secondary_color,
            },
        );
    }

    style_text(
        out,
        &countdown,
        Some(time_color),
        Some(surface_color),
        Weight::Bold,
        options.color,
    );
    style_text(
        out,
        &config.cap_right,
        Some(surface_color),
        Some(chunk_bg),
        Weight::Normal,
        options.color,
    );
}

/// A usage slot is present only when that exact window reports a numeric
/// usedPercent; renderers never shift later windows into earlier slots.
fn semantic_slot(window: &Option<UsageWindow>) -> Option<&UsageWindow> {
    window
        .as_ref()
        .filter(|window| window.used_percent.is_some())
}

/// A window is "long-horizon" (a weekly/monthly cap, rendered dimmed) when it
/// reports a windowMinutes at or beyond the dim threshold. Windows without a
/// known horizon stay bright.
fn is_long_window(window: Option<&UsageWindow>, dim_window_minutes: i64) -> bool {
    window
        .and_then(UsageWindow::window_minutes)
        .is_some_and(|minutes| minutes >= dim_window_minutes)
}

/// True when at least two present positional slots share one billing cycle:
/// identical non-null resetsAt/resetDescription and windowMinutes. Cursor's
/// Total/Auto/API pools are parallel usage categories inside a single monthly
/// budget rather than a live tier over a longer cap, so renderers keep them at
/// full brightness and draw a single pacing marker. Any present slot missing a
/// reset/window, or differing from the others, disqualifies the set.
fn shared_cycle(slots: &[Option<&UsageWindow>]) -> bool {
    let mut reference: Option<(&str, i64)> = None;
    let mut count = 0u32;
    for window in slots.iter().copied().flatten() {
        let (Some(reset), Some(minutes)) = (window.reset_value(), window.window_minutes()) else {
            return false;
        };
        match reference {
            None => reference = Some((reset, minutes)),
            Some(prev) if prev == (reset, minutes) => {}
            Some(_) => return false,
        }
        count += 1;
    }
    count >= 2
}

struct DualArgs<'a> {
    p_remaining: i32,
    s_remaining: i32,
    p_reset: Option<&'a str>,
    p_window: Option<i64>,
    s_reset: Option<&'a str>,
    s_window: Option<i64>,
    primary_color: &'a str,
    secondary_color: &'a str,
}

fn dual_metric_bar(
    out: &mut String,
    config: &RenderConfig,
    options: RenderOptions,
    args: DualArgs<'_>,
) {
    let width = config.zellij_bar_width.clamp(8, 400);
    let surface_color = &config.palette_surface;
    let elapsed_color = &config.palette_elapsed;
    let p_fill = filled_cells(args.p_remaining, width);
    let s_fill = filled_cells(args.s_remaining, width);
    let p_marker = elapsed_marker_cell(
        args.p_reset,
        args.p_window,
        width,
        options.now_epoch,
        config.reset_description_timezone_offset_minutes,
    );
    let s_marker = elapsed_marker_cell(
        args.s_reset,
        args.s_window,
        width,
        options.now_epoch,
        config.reset_description_timezone_offset_minutes,
    );

    for i in 0..width {
        // Top half = primary window, bottom half = secondary window. Each row
        // shows its own pacing marker via the elapsed color: primary on the
        // foreground of the upper-half-block, secondary on the background.
        let top_color = if Some(i) == p_marker {
            elapsed_color
        } else if i < p_fill {
            args.primary_color
        } else {
            surface_color
        };
        let bottom_color = if Some(i) == s_marker {
            elapsed_color
        } else if i < s_fill {
            args.secondary_color
        } else {
            surface_color
        };
        style_text(
            out,
            "▀",
            Some(top_color),
            Some(bottom_color),
            Weight::Normal,
            options.color,
        );
    }
    style_text(
        out,
        "▏",
        Some(&config.palette_bg),
        Some(surface_color),
        Weight::Normal,
        options.color,
    );
}

#[derive(Clone, Copy)]
struct Lane<'a> {
    remaining: i32,
    reset: Option<&'a str>,
    window: Option<i64>,
    is_long: bool,
    present: bool,
}

impl<'a> Lane<'a> {
    fn empty() -> Lane<'a> {
        Lane {
            remaining: 0,
            reset: None,
            window: None,
            is_long: false,
            present: false,
        }
    }

    fn from_window(window: &'a UsageWindow, config: &RenderConfig, stale: bool) -> Lane<'a> {
        Lane {
            remaining: 100 - window.used_pct_floor(),
            reset: if stale { None } else { window.reset_value() },
            window: if stale { None } else { window.window_minutes() },
            is_long: window
                .window_minutes()
                .is_some_and(|minutes| minutes >= config.dim_window_minutes),
            present: true,
        }
    }

    fn from_slot(slot: Option<&'a UsageWindow>, config: &RenderConfig, stale: bool) -> Lane<'a> {
        match slot {
            Some(window) => Lane::from_window(window, config, stale),
            None => Lane {
                remaining: 0,
                reset: None,
                window: None,
                is_long: false,
                present: false,
            },
        }
    }

    fn from_named(named: &'a NamedWindow, config: &RenderConfig, stale: bool) -> Lane<'a> {
        match named.window.as_ref() {
            Some(window) if named.usage_known != Some(false) => {
                Lane::from_window(window, config, stale)
            }
            _ => Lane {
                remaining: 0,
                reset: None,
                window: None,
                is_long: false,
                present: false,
            },
        }
    }
}

/// One color for the whole stacked chunk (mono3/mono4): the representative
/// window's severity, dimmed only when every present lane is a long-horizon cap.
fn mono_chunk_color(config: &RenderConfig, lanes: &[Lane<'_>]) -> String {
    let remaining = if config.mono_color_mode == "primary" {
        lanes.first().map_or(0, |lane| lane.remaining)
    } else {
        lanes
            .iter()
            .filter(|lane| lane.present)
            .map(|lane| lane.remaining)
            .min()
            .unwrap_or(0)
    };
    let mut any = false;
    let mut all_long = true;
    for lane in lanes.iter().filter(|lane| lane.present) {
        any = true;
        all_long &= lane.is_long;
    }
    config.window_color(remaining, any && all_long)
}

/// Resolve the configured marker slots to (column, color) pairs. The first
/// marker uses `palette_elapsed`, the rest `palette_elapsed_long`; markers whose
/// window has no parseable reset (or are stale) are dropped.
fn mono_marker_cells<'a>(
    config: &'a RenderConfig,
    lanes: &[Lane<'_>],
    width: usize,
    now_epoch: i64,
) -> Vec<(usize, &'a str)> {
    let colors = [
        config.palette_elapsed.as_str(),
        config.palette_elapsed_long.as_str(),
    ];
    let mut cells = Vec::new();
    for (rank, name) in config.mono_markers.iter().enumerate() {
        let index = match name.as_str() {
            "primary" => 0,
            "secondary" => 1,
            "tertiary" => 2,
            "quaternary" => 3,
            _ => continue,
        };
        let Some(lane) = lanes.get(index) else {
            continue;
        };
        if !lane.present {
            continue;
        }
        let Some(col) = elapsed_marker_cell(
            lane.reset,
            lane.window,
            width,
            now_epoch,
            config.reset_description_timezone_offset_minutes,
        ) else {
            continue;
        };
        cells.push((col, colors[rank.min(colors.len() - 1)]));
    }
    cells
}

/// Render the single-color stacked body: three lanes pack into sextants, four
/// into octants. Each configured marker replaces its column with a colored `│`.
fn mono_lane_bar(
    out: &mut String,
    config: &RenderConfig,
    options: RenderOptions,
    lanes: &[Lane<'_>],
    mono_color: &str,
    markers: &[(usize, &str)],
) {
    let width = config.zellij_bar_width.clamp(8, 400);
    let surface_color = &config.palette_surface;
    let fills: Vec<usize> = lanes
        .iter()
        .map(|lane| filled_cells(lane.remaining, width))
        .collect();
    let octant = lanes.len() >= 4;
    for i in 0..width {
        if let Some((_, color)) = markers.iter().find(|(col, _)| *col == i) {
            style_text(
                out,
                "│",
                Some(color),
                Some(surface_color),
                Weight::Normal,
                options.color,
            );
            continue;
        }
        let mut mask = 0i32;
        for (index, &fill) in fills.iter().enumerate() {
            if i < fill {
                mask |= 1 << index;
            }
        }
        let glyph = if octant {
            octant_mask_char(mask)
        } else {
            sextant_mask_char(mask)
        };
        let cell_color = if mask == 0 { surface_color } else { mono_color };
        style_text(
            out,
            glyph,
            Some(cell_color),
            Some(surface_color),
            Weight::Normal,
            options.color,
        );
    }
    style_text(
        out,
        "▏",
        Some(&config.palette_bg),
        Some(surface_color),
        Weight::Normal,
        options.color,
    );
}

/// 4-lane (2x4) octant glyph for a full-width row mask: bit0 = top lane ...
/// bit3 = bottom lane. Combinations absent from the Unicode 16 octant block fall
/// back to the matching quarter/half/full block element.
fn octant_mask_char(mask: i32) -> &'static str {
    match mask & 0b1111 {
        0b0000 => " ",
        0b0001 => "\u{1FB82}",
        0b0010 => "\u{1CD06}",
        0b0011 => "\u{2580}",
        0b0100 => "\u{1CD27}",
        0b0101 => "\u{1CD2A}",
        0b0110 => "\u{1CD33}",
        0b0111 => "\u{1FB85}",
        0b1000 => "\u{2582}",
        0b1001 => "\u{1CDAE}",
        0b1010 => "\u{1CDB7}",
        0b1011 => "\u{1CDBA}",
        0b1100 => "\u{2584}",
        0b1101 => "\u{1CDDD}",
        0b1110 => "\u{2586}",
        _ => "\u{2588}",
    }
}

struct Family<'a> {
    top: Lane<'a>,
    bottom: Lane<'a>,
}

/// Group a provider's quota pools into per-family duals (top = short/live
/// horizon, bottom = long/cap horizon). Present `extraRateWindows` are paired
/// two-at-a-time in CodexBar's per-family session→weekly emission order;
/// positional slots not already carried by an extra (matched on the
/// render-window dedup key) form a leading "main" family. A provider whose
/// pools live entirely in the extras (e.g. Antigravity) yields one family per
/// pool; a provider with a secondary extra pool (e.g. Codex + Spark) yields its
/// main slots plus the extra pool. `usageKnown:false` windows render empty.
fn pool_families<'a>(
    slots: &[Option<&'a UsageWindow>; 3],
    extras: &'a [NamedWindow],
    config: &RenderConfig,
    stale: bool,
) -> Vec<Family<'a>> {
    let present_extras: Vec<&'a NamedWindow> = extras
        .iter()
        .filter(|named| {
            named
                .window
                .as_ref()
                .is_some_and(|window| window.used_percent.is_some())
        })
        .collect();

    let mut families: Vec<Family<'a>> = Vec::new();
    let unmatched: Vec<&'a UsageWindow> = slots
        .iter()
        .flatten()
        .copied()
        .filter(|slot| !extra_contains(&present_extras, slot))
        .collect();
    if let Some(main) = main_family(&unmatched, config, stale) {
        families.push(main);
    }
    for pair in present_extras.chunks(2) {
        families.push(Family {
            top: Lane::from_named(pair[0], config, stale),
            bottom: pair
                .get(1)
                .map_or_else(Lane::empty, |named| Lane::from_named(named, config, stale)),
        });
    }
    families
}

/// True when a positional slot is already carried by a present extra window,
/// matched on the render-window dedup key (windowMinutes + resetsAt).
fn extra_contains(extras: &[&NamedWindow], slot: &UsageWindow) -> bool {
    extras.iter().any(|named| {
        named.window.as_ref().is_some_and(|window| {
            window.window_minutes() == slot.window_minutes()
                && window.resets_at.as_deref() == slot.resets_at.as_deref()
        })
    })
}

/// First alphanumeric of a title, uppercased, as the per-family tag.
fn family_label(title: Option<&str>) -> char {
    title
        .and_then(|title| title.chars().find(|c| c.is_alphanumeric()))
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('?')
}

/// Build the leading "main" family from positional slots not represented in the
/// extras: shortest horizon on top (live), longest on the bottom (cap). `None`
/// when every slot is subsumed.
fn main_family<'a>(
    unmatched: &[&'a UsageWindow],
    config: &RenderConfig,
    stale: bool,
) -> Option<Family<'a>> {
    let mut sorted = unmatched.to_vec();
    sorted.sort_by_key(|window| window.window_minutes().unwrap_or(i64::MAX));
    let top = *sorted.first()?;
    let bottom = *sorted.last()?;
    Some(Family {
        top: Lane::from_window(top, config, stale),
        bottom: if std::ptr::eq(top, bottom) {
            Lane::empty()
        } else {
            Lane::from_window(bottom, config, stale)
        },
    })
}

/// Up to `max` distinct renderable windows for collapse decisions and non-pooled
/// mono4/mono3 fallback lanes: positional slots first, then extra rate windows
/// with known usage, deduped by the render-window key (windowMinutes + resetsAt).
fn distinct_render_windows<'a>(
    slots: &[Option<&'a UsageWindow>; 3],
    extras: &'a [NamedWindow],
    max: usize,
) -> Vec<&'a UsageWindow> {
    let mut out: Vec<&'a UsageWindow> = Vec::new();
    let mut seen: Vec<(Option<i64>, Option<&'a str>)> = Vec::new();
    for window in slots.iter().copied().flatten() {
        let key = (window.window_minutes(), window.resets_at.as_deref());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(window);
        if out.len() >= max {
            return out;
        }
    }
    for named in extras {
        let Some(window) = named.window.as_ref() else {
            continue;
        };
        if window.used_percent.is_none() || named.usage_known == Some(false) {
            continue;
        }
        let key = (window.window_minutes(), window.resets_at.as_deref());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(window);
        if out.len() >= max {
            return out;
        }
    }
    out
}

/// A provider is model-pooled in `auto` mode when its `extraRateWindows` carry
/// every present positional slot (matched on windowMinutes + resetsAt): the
/// extras are then the canonical, complete dataset, so per-pool families drive
/// the bar instead of the (possibly cross-family) positional slots.
fn pooled_auto(slots: &[Option<&UsageWindow>; 3], extras: &[NamedWindow]) -> bool {
    let present_extras: Vec<&NamedWindow> = extras
        .iter()
        .filter(|named| {
            named
                .window
                .as_ref()
                .is_some_and(|window| window.used_percent.is_some())
        })
        .collect();
    if present_extras.is_empty() {
        return false;
    }
    slots
        .iter()
        .flatten()
        .all(|slot| extra_contains(&present_extras, slot))
}

/// Owned per-family windows for the split: a model-pooled provider becomes one
/// synthetic `dual` provider per pool (top = short/live, bottom = long/cap).
/// Mirrors `pool_families` grouping but yields cloned `UsageWindow`s so each
/// record flows through the normal `dual` path. `usageKnown:false` placeholders
/// keep their pairing slot but render empty (used_percent cleared).
struct FamilyWindows {
    label: char,
    primary: UsageWindow,
    secondary: Option<UsageWindow>,
}

fn family_windows(
    provider: &str,
    slots: &[Option<&UsageWindow>; 3],
    extras: &[NamedWindow],
) -> Vec<FamilyWindows> {
    let present_extras: Vec<&NamedWindow> = extras
        .iter()
        .filter(|named| {
            named
                .window
                .as_ref()
                .is_some_and(|window| window.used_percent.is_some())
        })
        .collect();

    let mut families: Vec<FamilyWindows> = Vec::new();
    let mut unmatched: Vec<&UsageWindow> = slots
        .iter()
        .flatten()
        .copied()
        .filter(|slot| !extra_contains(&present_extras, slot))
        .collect();
    if !unmatched.is_empty() {
        unmatched.sort_by_key(|window| window.window_minutes().unwrap_or(i64::MAX));
        let secondary = if unmatched.len() > 1 {
            Some(unmatched[unmatched.len() - 1].clone())
        } else {
            None
        };
        families.push(FamilyWindows {
            label: family_label(Some(provider)),
            primary: unmatched[0].clone(),
            secondary,
        });
    }
    for pair in present_extras.chunks(2) {
        families.push(FamilyWindows {
            label: family_label(pair[0].title.as_deref()),
            primary: named_window(pair[0]),
            secondary: pair.get(1).map(|named| named_window(named)),
        });
    }
    families
}

/// Clone a named extra's window, clearing `usedPercent` for `usageKnown:false`
/// placeholders so the lane renders empty rather than as fake live quota.
fn named_window(named: &NamedWindow) -> UsageWindow {
    let mut window = named.window.clone().unwrap_or(UsageWindow {
        used_percent: None,
        resets_at: None,
        reset_description: None,
        window_minutes: None,
    });
    if named.usage_known == Some(false) {
        window.used_percent = None;
    }
    window
}

/// Superscript form of a family initial for the split sigil (`AG` -> `AGᴳ`),
/// falling back to the plain letter where no modifier-letter glyph exists.
fn superscript(label: char) -> char {
    match label.to_ascii_uppercase() {
        'A' => 'ᴬ',
        'B' => 'ᴮ',
        'C' => 'ᶜ',
        'D' => 'ᴰ',
        'E' => 'ᴱ',
        'F' => 'ᶠ',
        'G' => 'ᴳ',
        'H' => 'ᴴ',
        'I' => 'ᴵ',
        'J' => 'ᴶ',
        'K' => 'ᴷ',
        'L' => 'ᴸ',
        'M' => 'ᴹ',
        'N' => 'ᴺ',
        'O' => 'ᴼ',
        'P' => 'ᴾ',
        'R' => 'ᴿ',
        'S' => 'ˢ',
        'T' => 'ᵀ',
        'U' => 'ᵁ',
        'W' => 'ᵂ',
        'X' => 'ˣ',
        'Z' => 'ᶻ',
        other => other,
    }
}

/// Expand a model-pooled provider into one synthetic `dual` provider per pool
/// (`AGᴳ`, `AGᶜ`). `Some` only when the resolved mode is `dual2` with >=2
/// families; a single pool (or any other mode, e.g. `mono4`) renders normally.
fn expand_pooled(
    record: &ProviderRecord,
    config: &RenderConfig,
) -> Option<Vec<(String, ProviderRecord)>> {
    let usage = record.usage.as_ref()?;
    let slots = [
        semantic_slot(&usage.primary),
        semantic_slot(&usage.secondary),
        semantic_slot(&usage.tertiary),
    ];
    let pooled = pooled_auto(&slots, &usage.extra_rate_windows);
    let mode = terminal_mode_for_provider(
        config,
        &record.provider,
        slots[2].is_some(),
        pooled,
        distinct_render_windows(&slots, &usage.extra_rate_windows, 4).len(),
    );
    if mode != "dual2" {
        return None;
    }
    let families = family_windows(&record.provider, &slots, &usage.extra_rate_windows);
    if families.len() < 2 {
        return None;
    }
    let base = provider_sigil(&record.provider);
    Some(
        families
            .into_iter()
            .take(2)
            .map(|family| {
                let sigil = format!("{base}{}", superscript(family.label));
                let synthetic = ProviderRecord {
                    provider: record.provider.clone(),
                    error: None,
                    usage: Some(Usage {
                        primary: Some(family.primary),
                        secondary: family.secondary,
                        tertiary: None,
                        extra_rate_windows: Vec::new(),
                    }),
                };
                (sigil, synthetic)
            })
            .collect(),
    )
}

fn filled_cells(remaining: i32, width: usize) -> usize {
    let remaining = remaining.clamp(0, 100) as usize;
    let mut filled = remaining * width / 100;
    if remaining > 0 && filled == 0 {
        filled = 1;
    }
    filled
}

fn sextant_mask_char(mask: i32) -> &'static str {
    match mask {
        0 => " ",
        1 => "🬂",
        2 => "🬋",
        3 => "🬎",
        4 => "🬭",
        5 => "🬰",
        6 => "🬹",
        7 => "█",
        _ => " ",
    }
}

fn elapsed_marker_cell(
    reset_at: Option<&str>,
    window_minutes: Option<i64>,
    width: usize,
    now_epoch: i64,
    reset_description_offset_minutes: Option<i16>,
) -> Option<usize> {
    let window_minutes = window_minutes?;
    if window_minutes <= 0 || width == 0 {
        return None;
    }
    let reset_epoch = reset_epoch(reset_at?, now_epoch, reset_description_offset_minutes)?;
    let duration = window_minutes.checked_mul(60)?;
    let start_epoch = reset_epoch.checked_sub(duration)?;
    let elapsed = (now_epoch - start_epoch).clamp(0, duration);
    // Compute in u64 so a large window_minutes does not truncate/overflow when
    // cast to a 32-bit usize on wasm32; the quotient is <= width and fits usize.
    let remaining = (duration - elapsed) as u64;
    let mut marker = (remaining.saturating_mul(width as u64) / duration as u64) as usize;
    if marker >= width {
        marker = width - 1;
    }
    Some(marker)
}

fn terminal_mode_for_provider(
    config: &RenderConfig,
    provider: &str,
    has_tertiary: bool,
    pooled: bool,
    assembled_window_count: usize,
) -> String {
    let requested = match config.terminal_bar_mode.as_str() {
        "dual" => "dual",
        "dual2" => "dual2",
        "mono3" => "mono3",
        "mono4" => "mono4",
        // `auto`: explicit per-provider override, else the family body for an
        // auto-detected model-pooled provider, else the positional dual.
        _ => config
            .mode_for(provider)
            .unwrap_or(if pooled { "dual2" } else { "dual" }),
    };
    // mono4 is an explicit four-lane body: render it only when the assembled
    // distinct-window set has all four lanes. With three assembled windows,
    // collapse to mono3; with fewer, follow the existing mono3→dual chain.
    let mode = match requested {
        "mono4" if assembled_window_count >= 4 => "mono4",
        "mono4" if assembled_window_count == 3 => "mono3",
        "mono4" => "dual",
        "mono3" if has_tertiary => "mono3",
        "mono3" => "dual",
        "dual2" => "dual2",
        _ => "dual",
    };
    mode.to_string()
}

fn provider_sigil(provider: &str) -> String {
    match provider {
        "codex" => "CX".into(),
        "claude" => "CL".into(),
        "cursor" => "CR".into(),
        "opencode" => "OC".into(),
        "opencodego" => "OG".into(),
        "alibaba" => "AL".into(),
        "factory" | "droid" => "FA".into(),
        "gemini" => "GE".into(),
        "antigravity" => "AG".into(),
        "copilot" => "CP".into(),
        "zai" => "ZA".into(),
        "minimax" => "MX".into(),
        "kimi" => "KM".into(),
        "kimik2" => "K2".into(),
        "kilo" => "KL".into(),
        "kiro" => "KR".into(),
        "vertexai" => "VA".into(),
        "augment" => "AU".into(),
        "jetbrains" => "JB".into(),
        "amp" => "AM".into(),
        "ollama" => "OL".into(),
        "synthetic" => "SY".into(),
        "warp" => "WP".into(),
        "openrouter" => "OR".into(),
        "windsurf" => "WS".into(),
        "perplexity" => "PX".into(),
        "abacus" => "AB".into(),
        "mistral" => "MS".into(),
        "deepseek" => "DS".into(),
        "codebuff" => "CB".into(),
        other => other.chars().take(2).flat_map(char::to_uppercase).collect(),
    }
}

fn primary_label(minutes: Option<i64>, remaining: i32, reset_value: Option<&str>) -> String {
    if let Some(minutes) = minutes {
        return format_countdown(minutes);
    }
    if reset_value.is_none() && remaining >= 100 {
        return "idle".into();
    }
    "?".into()
}

fn format_countdown(minutes: i64) -> String {
    if minutes <= 0 {
        return "now".into();
    }
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    let mins = minutes % 60;
    if hours < 24 {
        if mins == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}:{mins:02}")
        }
    } else {
        let days = hours / 24;
        if days < 14 {
            format!("{days}d")
        } else {
            format!("{}w", days / 7)
        }
    }
}

fn minutes_until(
    raw: &str,
    now_epoch: i64,
    reset_description_offset_minutes: Option<i16>,
) -> Option<i64> {
    let epoch = reset_epoch(raw, now_epoch, reset_description_offset_minutes)?;
    Some(((epoch - now_epoch).max(0)) / 60)
}

fn reset_epoch(
    raw: &str,
    now_epoch: i64,
    reset_description_offset_minutes: Option<i16>,
) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "null" {
        return None;
    }
    if let Some(epoch) = parse_offset_datetime(raw) {
        return Some(epoch);
    }

    let desc = raw
        .strip_prefix("Resets ")
        .or_else(|| raw.strip_prefix("resets "))?;

    parse_description_epoch(desc, now_epoch, reset_description_offset_minutes)
}

fn parse_offset_datetime(raw: &str) -> Option<i64> {
    if let Ok(parsed) = OffsetDateTime::parse(raw, &Rfc3339) {
        return Some(parsed.unix_timestamp());
    }
    let normalized = normalize_colonless_offset(raw)?;
    OffsetDateTime::parse(&normalized, &Rfc3339)
        .ok()
        .map(|parsed| parsed.unix_timestamp())
}

fn normalize_colonless_offset(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    if bytes.len() < 5 {
        return None;
    }
    let sign = bytes.len() - 5;
    if !matches!(bytes[sign], b'+' | b'-') {
        return None;
    }
    if !bytes[sign + 1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut normalized = String::with_capacity(raw.len() + 1);
    normalized.push_str(&raw[..raw.len() - 2]);
    normalized.push(':');
    normalized.push_str(&raw[raw.len() - 2..]);
    Some(normalized)
}

fn local_offset_at(datetime: OffsetDateTime) -> UtcOffset {
    UtcOffset::local_offset_at(datetime)
        .or_else(|_| UtcOffset::current_local_offset())
        .unwrap_or(UtcOffset::UTC)
}

fn configured_reset_description_offset(offset_minutes: Option<i16>) -> Option<UtcOffset> {
    let seconds = i32::from(offset_minutes?).checked_mul(60)?;
    UtcOffset::from_whole_seconds(seconds).ok()
}

fn assume_local(
    local: PrimitiveDateTime,
    now_epoch: i64,
    reset_description_offset_minutes: Option<i16>,
) -> Option<OffsetDateTime> {
    if let Some(offset) = configured_reset_description_offset(reset_description_offset_minutes) {
        return Some(local.assume_offset(offset));
    }
    let now = OffsetDateTime::from_unix_timestamp(now_epoch).ok()?;
    let mut offset = local_offset_at(now);
    for _ in 0..2 {
        let candidate = local.assume_offset(offset);
        let next = local_offset_at(candidate);
        if next == offset {
            return Some(candidate);
        }
        offset = next;
    }
    Some(local.assume_offset(offset))
}
fn parse_description_epoch(
    desc: &str,
    now_epoch: i64,
    reset_description_offset_minutes: Option<i16>,
) -> Option<i64> {
    let now = OffsetDateTime::from_unix_timestamp(now_epoch).ok()?;
    let local_offset = configured_reset_description_offset(reset_description_offset_minutes)
        .unwrap_or_else(|| local_offset_at(now));
    let short = format_description!(
        "[month repr:short] [day padding:none], [year] [hour repr:12 padding:none]:[minute] [period case:upper]"
    );
    let long = format_description!(
        "[month repr:long] [day padding:none], [year] [hour repr:12 padding:none]:[minute] [period case:upper]"
    );
    if let Ok(parsed) =
        PrimitiveDateTime::parse(desc, &short).or_else(|_| PrimitiveDateTime::parse(desc, &long))
    {
        return assume_local(parsed, now_epoch, reset_description_offset_minutes)
            .map(|parsed| parsed.unix_timestamp());
    }

    let clock = parse_time_12h(desc)?;
    let today = now.to_offset(local_offset).date();
    let mut local = PrimitiveDateTime::new(today, clock);
    let mut epoch =
        assume_local(local, now_epoch, reset_description_offset_minutes)?.unix_timestamp();
    if epoch < now_epoch {
        local = local.checked_add(Duration::days(1))?;
        epoch = assume_local(local, now_epoch, reset_description_offset_minutes)?.unix_timestamp();
    }
    Some(epoch)
}

fn parse_time_12h(desc: &str) -> Option<Time> {
    let (time_part, period) = desc.rsplit_once(' ')?;
    let (hour, minute) = time_part.split_once(':')?;
    let mut hour: u8 = hour.parse().ok()?;
    let minute: u8 = minute.parse().ok()?;
    if !(1..=12).contains(&hour) || minute > 59 {
        return None;
    }
    match period {
        "AM" => {
            if hour == 12 {
                hour = 0;
            }
        }
        "PM" => {
            if hour != 12 {
                hour += 12;
            }
        }
        _ => return None,
    }
    Time::from_hms(hour, minute, 0).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Weight {
    Normal,
    Bold,
}

fn style_text(
    out: &mut String,
    text: &str,
    fg_hex: Option<&str>,
    bg_hex: Option<&str>,
    weight: Weight,
    color: bool,
) {
    if color {
        if weight == Weight::Bold {
            out.push_str("\x1b[1m");
        }
        if let Some(hex) = fg_hex {
            fg(out, hex);
        }
        if let Some(hex) = bg_hex {
            bg(out, hex);
        }
    }
    out.push_str(text);
    reset(out, color);
}

fn fg(out: &mut String, hex: &str) {
    let (r, g, b) = hex_to_rgb(hex);
    out.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
}

fn bg(out: &mut String, hex: &str) {
    let (r, g, b) = hex_to_rgb(hex);
    out.push_str(&format!("\x1b[48;2;{r};{g};{b}m"));
}

fn reset(out: &mut String, color: bool) {
    if color {
        out.push_str("\x1b[0m");
    }
}

fn dim(out: &mut String, color: bool) {
    if color {
        out.push_str("\x1b[2m");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn render_idle(stale: bool, degraded_cli: bool) -> String {
        render_zellij(
            include_bytes!("../../../test/fixtures/codexbar-empty.json"),
            &RenderConfig::default(),
            RenderOptions {
                color: false,
                stale,
                degraded_cli,
                now_epoch: 4_070_908_800,
            },
        )
        .expect("rendered idle fixture")
    }

    #[test]
    fn idle_render_appends_degraded_marker() {
        assert_eq!(render_idle(false, true), "AI idle ⚠cli\n");
    }

    #[test]
    fn idle_render_appends_stale_marker() {
        assert_eq!(render_idle(true, false), "AI idle ⚠\n");
    }

    #[test]
    fn idle_render_appends_stale_and_degraded_markers() {
        assert_eq!(render_idle(true, true), "AI idle ⚠ ⚠cli\n");
    }

    #[test]
    fn idle_render_uses_configured_degraded_marker() {
        let config = RenderConfig {
            degraded_cli_glyph: "CLI".into(),
            ..RenderConfig::default()
        };

        let output = render_zellij(
            include_bytes!("../../../test/fixtures/codexbar-empty.json"),
            &config,
            RenderOptions {
                color: false,
                stale: false,
                degraded_cli: true,
                now_epoch: 4_070_908_800,
            },
        )
        .expect("rendered idle fixture");

        assert_eq!(output, "AI idle CLI\n");
    }

    #[test]
    fn countdown_format_matches_shell_contract() {
        assert_eq!(format_countdown(0), "now");
        assert_eq!(format_countdown(12), "12m");
        assert_eq!(format_countdown(60), "1h");
        assert_eq!(format_countdown(271), "4:31");
        assert_eq!(format_countdown(60 * 24 * 2), "2d");
        assert_eq!(format_countdown(60 * 24 * 35), "5w");
    }

    #[test]
    fn parses_reset_description_time_only() {
        // Pin the offset to UTC so the result is deterministic regardless of the
        // host timezone. now = 2024-01-01 00:00:00 UTC; "11:59 PM" resolves to the
        // same UTC day at 23:59:00.
        assert_eq!(
            reset_epoch("Resets 11:59 PM", 1_704_067_200, Some(0)),
            Some(1_704_153_540)
        );
        // When the parsed time is earlier than now, it rolls to the next day.
        // now = 2024-01-01 23:00:00 UTC; "1:00 AM" rolls forward to 2024-01-02.
        assert_eq!(
            reset_epoch("Resets 1:00 AM", 1_704_150_000, Some(0)),
            Some(1_704_157_200)
        );
    }

    #[test]
    fn parses_colonless_iso8601_offset() {
        assert_eq!(
            reset_epoch("2099-01-01T01:40:00+0000", 0, None),
            reset_epoch("2099-01-01T01:40:00+00:00", 0, None)
        );
        assert_eq!(
            reset_epoch("2099-01-01T01:40:00.123-0730", 0, None),
            reset_epoch("2099-01-01T01:40:00.123-07:30", 0, None)
        );
    }

    #[test]
    fn reset_description_uses_configured_timezone_offset() {
        assert_eq!(
            reset_epoch("Resets Jun 2, 2026 4:30 PM", 1_780_401_600, Some(0)),
            Some(1_780_417_800)
        );
        assert_eq!(
            reset_epoch("Resets Jun 2, 2026 4:30 PM", 1_780_401_600, Some(-420)),
            Some(1_780_443_000)
        );
    }

    #[test]
    fn no_color_render_contains_no_ansi_escapes() {
        let output = render_zellij(
            include_bytes!("../../../test/fixtures/codexbar-mixed.json"),
            &RenderConfig::default(),
            RenderOptions {
                color: false,
                stale: false,
                degraded_cli: false,
                now_epoch: 4_070_908_800,
            },
        )
        .expect("rendered fixture");

        assert!(!output.contains('\x1b'), "{output:?}");
        assert!(output.contains("CL"));
    }

    #[test]
    fn renders_provider_without_primary_window() {
        // Antigravity now defaults to dual2; force mono3 to exercise the
        // mono3 null-primary slot mapping (secondary→middle, tertiary→bottom).
        let config = RenderConfig {
            terminal_bar_mode: "mono3".into(),
            ..RenderConfig::default()
        };
        let output = render_zellij(
            br#"[
                {
                    "provider": "antigravity",
                    "usage": {
                        "primary": null,
                        "secondary": {"usedPercent": 0},
                        "tertiary": {"usedPercent": 25}
                    }
                }
            ]"#,
            &config,
            RenderOptions {
                color: false,
                stale: false,
                degraded_cli: false,
                now_epoch: 4_070_908_800,
            },
        )
        .expect("rendered secondary-only provider");

        assert!(output.contains("AG"), "{output}");
        // No primary window: nothing to count down.
        assert!(output.contains("idle"), "{output}");
        // Forced mono3 renders the secondary-only shape (width 12):
        // secondary fills the middle row (100 remaining → 12 cells) and
        // tertiary the bottom row (75 remaining → 9 cells), so cells are
        // middle+bottom (mask 6) then middle-only (mask 2).
        assert!(output.contains("🬹"), "{output}");
        assert!(output.contains("🬋"), "{output}");
        // The top row must stay empty: no glyph with the primary bit set.
        for top_lit in ["🬂", "🬎", "🬰", "█"] {
            assert!(
                !output.contains(top_lit),
                "missing primary must not light the top row: {output}"
            );
        }
    }

    fn cycle_window(reset: &str, minutes: i64) -> UsageWindow {
        UsageWindow {
            used_percent: Some(10.0),
            resets_at: Some(reset.to_string()),
            reset_description: None,
            window_minutes: Some(minutes),
        }
    }

    #[test]
    fn shared_cycle_requires_uniform_reset_and_window() {
        let a = cycle_window("2099-01-15T00:00:00Z", 43200);
        let b = cycle_window("2099-01-15T00:00:00Z", 43200);
        assert!(shared_cycle(&[Some(&a), Some(&b), None]));

        // Different reset disqualifies (independent cycles).
        let other_reset = cycle_window("2099-01-16T00:00:00Z", 43200);
        assert!(!shared_cycle(&[Some(&a), Some(&other_reset), None]));

        // Different horizon disqualifies (live tier vs longer cap).
        let short = cycle_window("2099-01-15T00:00:00Z", 300);
        assert!(!shared_cycle(&[Some(&a), Some(&short), None]));

        // A single present slot is not a shared cycle.
        assert!(!shared_cycle(&[Some(&a), None, None]));

        // A present slot missing reset/window disqualifies the set.
        let bare = UsageWindow {
            used_percent: Some(5.0),
            resets_at: None,
            reset_description: None,
            window_minutes: None,
        };
        assert!(!shared_cycle(&[Some(&a), Some(&bare)]));
    }

    #[test]
    fn shared_cycle_pools_render_bright_with_single_marker() {
        // Cursor's Total/Auto/API share one billing cycle (same resetsAt +
        // windowMinutes), so a forced dual body keeps both rows at full
        // brightness (no long-horizon dimming) and draws only the primary
        // pacing marker rather than the identical secondary one.
        let config = RenderConfig {
            terminal_bar_mode: "dual".into(),
            zellij_bar_width: 8,
            ..RenderConfig::default()
        };
        let output = render_zellij(
            include_bytes!("../../../test/fixtures/codexbar-cursor.json"),
            &config,
            RenderOptions {
                color: true,
                stale: false,
                degraded_cli: false,
                now_epoch: 4_070_908_800,
            },
        )
        .expect("rendered cursor fixture");

        // Fills are bright "good" (25be6a); without shared-cycle handling the
        // 30-day horizon (43200 >= dim threshold) would dim this color away.
        assert!(output.contains("38;2;37;190;106"), "{output}");
        // Primary pacing marker (be95ff) drawn as a foreground...
        assert!(output.contains("38;2;190;149;255"), "{output}");
        // ...but the redundant secondary marker (same column, drawn as a
        // background) is suppressed.
        assert!(!output.contains("48;2;190;149;255"), "{output}");
    }
}
