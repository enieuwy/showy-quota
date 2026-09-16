use std::borrow::Cow;
use std::cmp::Ordering;

use serde::Serialize;

use crate::codexbar::{is_errored, is_renderable, NamedWindow, ProviderRecord, Usage, UsageWindow};
use crate::config::RenderConfig;
use crate::palette::{hex_to_rgb, normalized_hex, Severity};
use crate::reset::{minutes_until, reset_clock, reset_epoch};

#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    pub color: bool,
    pub stale: bool,
    pub degraded_cli: bool,
    pub now_epoch: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Zellij,
    Tmux,
}

impl OutputFormat {
    fn bar_width(self, config: &RenderConfig) -> usize {
        match self {
            OutputFormat::Zellij => config.zellij_bar_width,
            OutputFormat::Tmux => config.tmux_bar_width.unwrap_or(config.zellij_bar_width),
        }
        .clamp(8, 400)
    }
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
    render_with_format(payload, config, options, OutputFormat::Zellij)
}

pub fn render_tmux(
    payload: &[u8],
    config: &RenderConfig,
    options: RenderOptions,
) -> Result<String, RenderError> {
    render_with_format(payload, config, options, OutputFormat::Tmux)
}

fn render_with_format(
    payload: &[u8],
    config: &RenderConfig,
    options: RenderOptions,
    output_format: OutputFormat,
) -> Result<String, RenderError> {
    let records = parse_render_payload(payload).map_err(|_| RenderError::InvalidPayload)?;
    Ok(render_records(&records, config, options, output_format))
}

/// Parse the array transport. `render_records` filters per-record via
/// `is_renderable`/`is_errored`, and `slot()` already drops windows lacking
/// `usedPercent`, so this is a plain deserialize: only transport-level
/// failures (unparseable JSON, non-array top level) are fatal here.
fn parse_render_payload(payload: &[u8]) -> Result<Vec<ProviderRecord>, serde_json::Error> {
    let records: Vec<ProviderRecord> = serde_json::from_slice(payload)?;
    Ok(records)
}

enum RenderUnit<'a> {
    Provider(Box<Cow<'a, ProviderRecord>>, String),
    Error(&'a ProviderRecord, String),
}

/// One rendered chunk of the strip, kept separate instead of concatenated.
///
/// `text` is byte-identical to the chunk `render_zellij` / `render_tmux` emit
/// for the same provider, so a surface that stacks chunks as rows shows the
/// same glyphs, caps, markers and countdown as the single-line strip. Pooled
/// providers still expand into one row per family (`AGᴳ`, `AGᶜ`) and stacked
/// modes (mono3/mono4) still occupy exactly one row, because rows are the
/// renderer's own chunks rather than raw usage windows.
///
/// `severity` and `dim` report the band showy-quota coloured the chunk with, for
/// surfaces that cannot carry colour inside the text: a Herdr sidebar token
/// strips control bytes, so it needs the band by name. Strip-level state
/// (`stale`, `degraded_cli`) stays with the caller that supplied it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedRow {
    pub provider: String,
    pub sigil: String,
    pub text: String,
    /// `None` for a provider that reported an error: those chunks carry an
    /// error label rather than a usage bar.
    pub severity: Option<Severity>,
    pub dim: bool,
    /// The hex this chunk's band resolves to, so a surface needs no palette
    /// knowledge of its own. A stale strip greys its chunks; that override is
    /// the caller's to apply, since it also owns the `stale` flag.
    pub color: String,
    pub error: bool,
}

/// Serialise `render_rows` output as a JSON array, the transport shared by the
/// display emitters.
pub fn emit_rows(
    payload: &[u8],
    config: &RenderConfig,
    options: RenderOptions,
    output_format: OutputFormat,
) -> Result<String, RenderError> {
    let rows = render_rows(payload, config, options, output_format)?;
    serde_json::to_string(&rows).map_err(|_| RenderError::InvalidPayload)
}

/// Render the strip as one entry per chunk. An empty result means the same as
/// the strip's `AI idle`: no provider reported renderable usage.
pub fn render_rows(
    payload: &[u8],
    config: &RenderConfig,
    options: RenderOptions,
    output_format: OutputFormat,
) -> Result<Vec<RenderedRow>, RenderError> {
    let records = parse_render_payload(payload).map_err(|_| RenderError::InvalidPayload)?;
    let mut records: Vec<&ProviderRecord> = records
        .iter()
        .filter(|record| is_renderable(record) || is_errored(record))
        .collect();
    filter_and_sort(&mut records, config);

    let chunk_bg = &config.palette_bg;
    Ok(collect_units(&records, config)
        .iter()
        .map(|unit| {
            let mut text = String::new();
            match unit {
                RenderUnit::Provider(record, sigil) => {
                    let band =
                        render_provider(&mut text, record, sigil, config, options, output_format);
                    let severity = config.severity(band.remaining);
                    RenderedRow {
                        provider: record.provider.clone(),
                        sigil: sigil.clone(),
                        text,
                        severity: Some(severity),
                        dim: band.dim,
                        color: config.severity_color(severity, band.dim),
                        error: false,
                    }
                }
                RenderUnit::Error(record, sigil) => {
                    render_error_provider(
                        &mut text,
                        record,
                        sigil,
                        config,
                        options,
                        output_format,
                        chunk_bg,
                    );
                    RenderedRow {
                        provider: record.provider.clone(),
                        sigil: sigil.clone(),
                        text,
                        severity: None,
                        dim: false,
                        color: normalized_hex(&config.palette_countdown_warn),
                        error: true,
                    }
                }
            }
        })
        .collect())
}

/// One line per quota window, for a surface that owns vertical space (an SSH
/// pane on a phone, a tall sidebar) instead of a single status-bar line.
///
/// The horizontal strip packs two windows into one line with half blocks and
/// three or four into sextant/octant mosaics because it owns exactly one line
/// and pays for every column. Here the axis inverts: rows are cheap, so every
/// window gets its own full-height bar, horizon label, exact remaining percent
/// and countdown. Nothing is mosaic-encoded, so this view needs no
/// octant-capable terminal.
///
/// Two strip conventions are deliberately dropped. Long-horizon windows are not
/// dimmed: dim exists to say "this is a weekly/monthly cap" in a body with no
/// room to write it, and this view prints the horizon in its own column, so dim
/// would spend contrast restating a literal label. Pacing markers are drawn as a
/// `│` tick over the track rather than a colored cell, so a marker can never be
/// mistaken for usage or punch a hole in a full bar.
pub fn render_vertical(
    payload: &[u8],
    config: &RenderConfig,
    options: RenderOptions,
) -> Result<String, RenderError> {
    let records = parse_render_payload(payload).map_err(|_| RenderError::InvalidPayload)?;
    let mut records: Vec<&ProviderRecord> = records
        .iter()
        .filter(|record| is_renderable(record) || is_errored(record))
        .collect();
    filter_and_sort(&mut records, config);

    let format = OutputFormat::Zellij;
    let width = config.vertical_bar_width.clamp(8, 400);
    let chunk_bg = &config.palette_bg;
    let mut out = String::new();

    if records.is_empty() {
        dim(&mut out, format, options.color);
        out.push_str("AI idle");
        reset(&mut out, format, options.color);
        out.push('\n');
    }

    // Units are bound rather than iterated inline: the window borrows below live
    // until every line is rendered, because the label and chip columns are
    // measured across all providers so each bar starts at the same column.
    let units = collect_units(&records, config);
    let mut groups: Vec<(&str, Vec<VerticalWindow<'_>>)> = Vec::new();
    for unit in &units {
        match unit {
            RenderUnit::Error(record, sigil) => {
                render_error_provider(&mut out, record, sigil, config, options, format, chunk_bg);
                out.push('\n');
            }
            RenderUnit::Provider(record, sigil) => {
                let windows = vertical_windows(record, config, options.now_epoch);
                if !windows.is_empty() {
                    groups.push((sigil.as_str(), windows));
                }
            }
        }
    }

    let label_width = groups
        .iter()
        .flat_map(|(_, windows)| windows.iter())
        .map(|window| window.label.chars().count())
        .max()
        .unwrap_or(0);
    // Pooled providers carry a family superscript (`AGᴳ`), so the chip column is
    // measured too: without it a three-cell chip would push one provider's bar
    // out of the shared column.
    let sigil_width = groups
        .iter()
        .map(|(sigil, _)| sigil.chars().count())
        .max()
        .unwrap_or(2);

    // `urgency` answers "what is about to bite" by flattening the provider
    // blocks, so every line then carries its own chip and no blank separators
    // are drawn. `provider` keeps CodexBar's grouping with one blank line
    // between blocks, which is what makes the grouping readable at a glance.
    let mut lines: Vec<(&str, &VerticalWindow<'_>, bool)> = Vec::new();
    if config.vertical_sort == "urgency" {
        let mut flat: Vec<(usize, &str, &VerticalWindow<'_>)> = groups
            .iter()
            .enumerate()
            .flat_map(|(group_index, (sigil, windows))| {
                windows
                    .iter()
                    .map(move |window| (group_index, *sigil, window))
            })
            .collect();
        flat.sort_by(|a, b| {
            a.2.remaining
                .cmp(&b.2.remaining)
                .then_with(|| {
                    a.2.minutes
                        .unwrap_or(i64::MAX)
                        .cmp(&b.2.minutes.unwrap_or(i64::MAX))
                })
                // Ties keep the order the providers arrived in, which is
                // CodexBar's order as filtered by `provider_order`. Comparing
                // the sigil instead sorted alphabetically, so an urgency tie
                // put CL before CX under the default codex,claude order and
                // contradicted the documented behaviour.
                .then_with(|| a.0.cmp(&b.0))
                // Within one provider, fall back to its own window order.
                // Comparing labels would sort by superscript codepoint
                // (² before ¹) and scramble same-cycle pools.
                .then_with(|| a.2.order.cmp(&b.2.order))
        });
        lines.extend(
            flat.into_iter()
                .map(|(_, sigil, window)| (sigil, window, true)),
        );
    } else {
        for (sigil, windows) in &groups {
            for (index, window) in windows.iter().enumerate() {
                lines.push((sigil, window, index == 0));
            }
        }
    }

    let group_breaks = config.vertical_sort != "urgency";
    for (index, (sigil, window, head)) in lines.iter().enumerate() {
        if group_breaks && *head && index > 0 {
            out.push('\n');
        }
        // The chip labels the provider block once. Continuation lines hold its
        // width but not its color: a tinted, letterless stub reads as the first
        // cells of the bar.
        let chip = if *head {
            format!("{sigil:<sigil_width$}")
        } else {
            " ".repeat(sigil_width)
        };
        render_vertical_line(
            &mut out,
            config,
            options,
            VerticalLine {
                chip: &chip,
                chip_filled: *head,
                window,
                width,
                label_width,
            },
        );
        out.push('\n');
    }

    // Strip-level state gets its own trailing line: a vertical view has no
    // shared line to hang the markers on.
    let mut glyphs: Vec<&str> = Vec::new();
    if options.stale {
        glyphs.push(config.stale_glyph.as_str());
    }
    if options.degraded_cli {
        glyphs.push(config.degraded_cli_glyph.as_str());
    }
    for (index, glyph) in glyphs.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        style_text(
            &mut out,
            glyph,
            Some(&config.palette_countdown_warn),
            Some(chunk_bg),
            Weight::Bold,
            format,
            options.color,
        );
    }
    if !glyphs.is_empty() {
        out.push('\n');
    }

    Ok(out)
}

/// One window of the vertical view, already resolved to what a line needs.
struct VerticalWindow<'a> {
    label: String,
    remaining: i32,
    reset: Option<&'a str>,
    window: Option<i64>,
    /// Minutes until this window's own reset, for its countdown and for
    /// `urgency` ordering.
    minutes: Option<i64>,
    /// Position within the provider's own window list, so `urgency` ties keep
    /// CodexBar's ordering.
    order: usize,
}

/// A window selected for the view, with the name it ended up carrying. A
/// positional slot starts nameless and can inherit the title of an extra that
/// republishes it.
struct PickedWindow<'a> {
    window: &'a UsageWindow,
    slot: Option<usize>,
    title: Option<&'a str>,
}

fn vertical_windows<'a>(
    record: &'a ProviderRecord,
    config: &RenderConfig,
    now_epoch: i64,
) -> Vec<VerticalWindow<'a>> {
    let Some(usage) = record.usage.as_ref() else {
        return Vec::new();
    };
    let slots = usage.render_slots();
    let extras = &usage.extra_rate_windows;

    // Positional slots are distinct measurements by definition and are never
    // deduplicated against each other: Cursor's Total/Auto/API report one
    // identical reset, horizon and usage, yet they are three separate pools.
    let mut picked: Vec<PickedWindow<'a>> = Vec::new();
    for (index, slot) in slots.iter().copied().enumerate() {
        if let Some(window) = slot {
            picked.push(PickedWindow {
                window,
                slot: Some(index),
                title: None,
            });
        }
    }
    // An extra that republishes a kept window does not earn a second line — but
    // it does carry the name CodexBar gave that measurement (Antigravity's
    // `Gemini weekly` / `Claude/GPT weekly`), so the title is transferred to the
    // slot instead of discarded with the duplicate.
    for named in extras {
        if !has_known_extra_usage(named) {
            continue;
        }
        let Some(window) = named.window.as_ref() else {
            continue;
        };
        if let Some(kept) = picked
            .iter_mut()
            .find(|kept| same_render_window(kept.window, window))
        {
            if kept.title.is_none() {
                kept.title = named.title.as_deref();
            }
            continue;
        }
        picked.push(PickedWindow {
            window,
            slot: None,
            title: named.title.as_deref(),
        });
    }

    let mut out: Vec<VerticalWindow<'a>> = Vec::with_capacity(picked.len());
    for (order, entry) in picked.iter().enumerate() {
        let horizon = horizon_label(entry.window.window_minutes());
        // A tag is only earned where the horizon alone cannot identify the
        // window. A named window sharing a horizon takes the strip's existing
        // superscript family tag (AGᴳ / AGᶜ), because the name is real
        // information. A nameless slot falls back to its slot ordinal, and only
        // when another nameless slot shares the horizon (Cursor's three monthly
        // pools): beside a named window the other tag already distinguishes it.
        let same_horizon =
            |other: &&PickedWindow<'_>| horizon_label(other.window.window_minutes()) == horizon;
        let shared_horizon = picked.iter().filter(same_horizon).count() > 1;
        let label = match (entry.title, entry.slot) {
            (Some(title), _) if shared_horizon => {
                format!("{horizon}{}", superscript(family_label(Some(title))))
            }
            (None, Some(index))
                if picked
                    .iter()
                    .filter(same_horizon)
                    .filter(|other| other.title.is_none() && other.slot.is_some())
                    .count()
                    > 1 =>
            {
                format!("{horizon}{}", ordinal_superscript(index))
            }
            _ => horizon,
        };
        let reset = entry.window.reset_value();
        out.push(VerticalWindow {
            label,
            remaining: 100 - entry.window.used_pct_floor(),
            reset,
            window: entry.window.window_minutes(),
            minutes: reset.and_then(|value| {
                minutes_until(
                    value,
                    now_epoch,
                    config.reset_description_timezone_offset_minutes,
                )
            }),
            order,
        });
    }
    out
}

/// Two records describe the same measured window: same horizon, same reset and
/// same usage. Usage is part of the identity because distinct pools legitimately
/// share a reset and horizon (Claude's weekly cap and its `Fable only` pool).
fn same_render_window(a: &UsageWindow, b: &UsageWindow) -> bool {
    a.window_minutes() == b.window_minutes()
        && a.reset_value() == b.reset_value()
        && a.used_pct_floor() == b.used_pct_floor()
}

/// Superscript digit for a positional slot, so same-cycle slots stay ordered
/// and distinguishable without inventing names CodexBar did not publish.
fn ordinal_superscript(index: usize) -> char {
    match index {
        0 => '¹',
        1 => '²',
        2 => '³',
        _ => '⁴',
    }
}

/// Compact horizon tag for a window length: `45m`, `5h`, `7d`, `1mo`. Cycles of
/// four weeks or more are labelled in months rather than raw days, because a 30d
/// and a 31d cycle are both "monthly" and printing the calendar length invites a
/// comparison that carries no meaning. Unknown lengths render `?` rather than a
/// guessed horizon.
fn horizon_label(minutes: Option<i64>) -> String {
    let Some(minutes) = minutes.filter(|minutes| *minutes > 0) else {
        return "?".into();
    };
    if minutes < 60 {
        return format!("{minutes}m");
    }
    if minutes < 1440 {
        return format!("{}h", minutes / 60);
    }
    let days = minutes / 1440;
    if days < 28 {
        return format!("{days}d");
    }
    format!("{}mo", (days + 15) / 30)
}

struct VerticalLine<'a> {
    chip: &'a str,
    /// A chip carrying a sigil is filled with the window's colour; a blank
    /// continuation chip stays on the page background, because a tinted,
    /// letterless stub reads as the leading cells of the bar.
    chip_filled: bool,
    window: &'a VerticalWindow<'a>,
    width: usize,
    label_width: usize,
}

fn render_vertical_line(
    out: &mut String,
    config: &RenderConfig,
    options: RenderOptions,
    line: VerticalLine<'_>,
) {
    let format = OutputFormat::Zellij;
    let chunk_bg = &config.palette_bg;
    let surface = &config.palette_surface;
    let window = line.window;

    // No dim variant: the horizon is printed in its own column, so dimming would
    // restate a literal label at the cost of contrast.
    let color = if options.stale {
        config.palette_stale.clone()
    } else {
        config.window_color(window.remaining, false)
    };
    // A stale snapshot cannot place a pacing marker, matching the strip's marker
    // suppression. The countdown still renders — it is the reading, not the
    // pacing — and its stale colour says the snapshot is old.
    let (marker_reset, marker_window) = if options.stale {
        (None, None)
    } else {
        (window.reset, window.window)
    };
    let countdown = primary_label(window.minutes, window.remaining, window.reset);
    let time_color = if options.stale {
        config.palette_stale.as_str()
    } else if window
        .minutes
        .is_some_and(|value| value < config.time_warn_minutes)
    {
        config.palette_countdown_warn.as_str()
    } else {
        config.palette_countdown.as_str()
    };

    let chip_bg = if line.chip_filled { &color } else { chunk_bg };
    style_text(
        out,
        cap_text(format, &config.cap_left).as_ref(),
        Some(chip_bg),
        Some(chunk_bg),
        Weight::Normal,
        format,
        options.color,
    );
    style_text(
        out,
        line.chip,
        Some(chunk_bg),
        Some(chip_bg),
        Weight::Bold,
        format,
        options.color,
    );
    // Close the chip into a pill: with the numbers now outside the plate, the
    // right cap belongs to the chip, not to a trailing surface.
    style_text(
        out,
        cap_text(format, &config.cap_right).as_ref(),
        Some(chip_bg),
        Some(chunk_bg),
        Weight::Normal,
        format,
        options.color,
    );
    // The label sits outside the plate on the page background: sharing the
    // track's background hid where measurement begins.
    style_text(
        out,
        &format!(" {:<width$} ", window.label, width = line.label_width),
        Some(&config.palette_countdown),
        Some(chunk_bg),
        Weight::Normal,
        format,
        options.color,
    );
    style_text(
        out,
        "▕",
        Some(chunk_bg),
        Some(surface),
        Weight::Normal,
        format,
        options.color,
    );

    let fill = filled_cells(window.remaining, line.width);
    let marker = elapsed_marker_cell(
        marker_reset,
        marker_window,
        line.width,
        options.now_epoch,
        config.reset_description_timezone_offset_minutes,
    );
    for cell in 0..line.width {
        let filled = cell < fill;
        // The pacing marker is a tick drawn *over* the track, so it never
        // consumes a measured cell: the cell it lands on keeps its fill state as
        // the background.
        let (glyph, fg) = if Some(cell) == marker {
            ("│", config.palette_elapsed.as_str())
        } else if filled {
            ("█", color.as_str())
        } else {
            ("█", surface.as_str())
        };
        let bg = if filled {
            color.as_str()
        } else {
            surface.as_str()
        };
        style_text(
            out,
            glyph,
            Some(fg),
            Some(bg),
            Weight::Normal,
            format,
            options.color,
        );
    }
    style_text(
        out,
        "▏",
        Some(chunk_bg),
        Some(surface),
        Weight::Normal,
        format,
        options.color,
    );

    // The percentage is the primary reading, so it keeps full contrast and never
    // inherits a dimmed band.
    style_text(
        out,
        &format!(" {:>3}% ", window.remaining.clamp(0, 100)),
        Some(&color),
        Some(chunk_bg),
        Weight::Bold,
        format,
        options.color,
    );
    // Countdowns are padded to the widest form (`idle`, `1:23`) so every line
    // ends at the same column.
    style_text(
        out,
        &format!("{countdown:<4}"),
        Some(time_color),
        Some(chunk_bg),
        Weight::Bold,
        format,
        options.color,
    );
    // Four rows reading `1d` say nothing about *when*. This view has the columns
    // to answer it, so the clock is on unless the caller wants the line shorter.
    if config.vertical_reset_clock {
        let clock = window
            .reset
            .and_then(|value| {
                reset_clock(
                    value,
                    options.now_epoch,
                    config.reset_description_timezone_offset_minutes,
                )
            })
            .unwrap_or_else(|| "     ".into());
        style_text(
            out,
            &format!(" {clock}"),
            Some(&config.palette_countdown),
            Some(chunk_bg),
            Weight::Normal,
            format,
            options.color,
        );
    }
}

/// Model-pooled providers (auto-detected `dual2`) are split into one synthetic
/// per-family `dual` provider each (`AGᴳ`, `AGᶜ`); everything else renders
/// as-is. Each unit then flows through the normal dual path.
fn collect_units<'a>(records: &[&'a ProviderRecord], config: &RenderConfig) -> Vec<RenderUnit<'a>> {
    let mut units: Vec<RenderUnit<'a>> = Vec::new();
    for record in records {
        if is_errored(record) {
            units.push(RenderUnit::Error(record, provider_sigil(&record.provider)));
            continue;
        }
        match expand_pooled(record, config) {
            Some(families) => {
                for (sigil, synthetic) in families {
                    units.push(RenderUnit::Provider(Box::new(Cow::Owned(synthetic)), sigil));
                }
            }
            None => units.push(RenderUnit::Provider(
                Box::new(Cow::Borrowed(record)),
                provider_sigil(&record.provider),
            )),
        }
    }
    units
}

fn render_records(
    records: &[ProviderRecord],
    config: &RenderConfig,
    options: RenderOptions,
    output_format: OutputFormat,
) -> String {
    let mut records: Vec<&ProviderRecord> = records
        .iter()
        .filter(|record| is_renderable(record) || is_errored(record))
        .collect();
    filter_and_sort(&mut records, config);

    let chunk_bg = &config.palette_bg;
    let countdown_warn = &config.palette_countdown_warn;
    let mut out = String::new();

    if records.is_empty() {
        dim(&mut out, output_format, options.color);
        match output_format {
            OutputFormat::Zellij => {
                out.push_str("AI idle");
                reset(&mut out, output_format, options.color);
            }
            OutputFormat::Tmux => {
                style_text(
                    &mut out,
                    "AI idle",
                    Some(&config.palette_primary_unknown),
                    None,
                    Weight::Normal,
                    output_format,
                    options.color,
                );
                return out;
            }
        }
    } else {
        let units = collect_units(&records, config);
        for (idx, unit) in units.iter().enumerate() {
            if idx > 0 {
                separator_space(&mut out, output_format, chunk_bg, options.color);
            }
            match unit {
                RenderUnit::Provider(record, sigil) => {
                    render_provider(&mut out, record, sigil, config, options, output_format);
                }
                RenderUnit::Error(record, sigil) => render_error_provider(
                    &mut out,
                    record,
                    sigil,
                    config,
                    options,
                    output_format,
                    chunk_bg,
                ),
            }
        }
    }

    if options.stale {
        separator_space(&mut out, output_format, chunk_bg, options.color);
        style_text(
            &mut out,
            &config.stale_glyph,
            Some(countdown_warn),
            Some(chunk_bg),
            Weight::Bold,
            output_format,
            options.color,
        );
    }
    if options.degraded_cli {
        separator_space(&mut out, output_format, chunk_bg, options.color);
        style_text(
            &mut out,
            &config.degraded_cli_glyph,
            Some(countdown_warn),
            Some(chunk_bg),
            Weight::Bold,
            output_format,
            options.color,
        );
    }
    if output_format == OutputFormat::Zellij {
        out.push('\n');
    }
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

fn render_error_provider(
    out: &mut String,
    record: &ProviderRecord,
    sigil: &str,
    config: &RenderConfig,
    options: RenderOptions,
    output_format: OutputFormat,
    chunk_bg: &str,
) {
    debug_assert!(is_errored(record));
    let error_color = &config.palette_countdown_warn;
    let cap_left = cap_text(output_format, &config.cap_left);
    style_text(
        out,
        cap_left.as_ref(),
        Some(error_color),
        Some(chunk_bg),
        Weight::Normal,
        output_format,
        options.color,
    );
    style_text(
        out,
        sigil,
        Some(chunk_bg),
        Some(error_color),
        Weight::Bold,
        output_format,
        options.color,
    );
    let label = center_pad(
        &format!("{}err", config.error_glyph),
        output_format.bar_width(config),
    );
    style_text(
        out,
        &label,
        Some(error_color),
        Some(chunk_bg),
        Weight::Normal,
        output_format,
        options.color,
    );
    let cap_right = cap_text(output_format, &config.cap_right);
    style_text(
        out,
        cap_right.as_ref(),
        Some(chunk_bg),
        Some(chunk_bg),
        Weight::Normal,
        output_format,
        options.color,
    );
}

/// Center `text` within a field of `width` characters by padding both sides
/// with spaces, so a short label (e.g. the error chunk) occupies the same
/// visual footprint as the indicator bar it replaces. Text at or beyond
/// `width` is returned unpadded.
fn center_pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        return text.to_string();
    }
    let pad = width - len;
    let left = pad / 2;
    let right = pad - left;
    let mut out = String::with_capacity(width);
    out.push_str(&" ".repeat(left));
    out.push_str(text);
    out.push_str(&" ".repeat(right));
    out
}

/// The severity band a chunk was coloured with, reported so surfaces that
/// cannot carry colour inside the text itself can reproduce showy-quota's own
/// choice. See `RenderedRow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowBand {
    remaining: i32,
    dim: bool,
}

fn render_provider(
    out: &mut String,
    record: &ProviderRecord,
    sigil: &str,
    config: &RenderConfig,
    options: RenderOptions,
    output_format: OutputFormat,
) -> RowBand {
    let chunk_bg = &config.palette_bg;
    let stale_color = &config.palette_stale;
    let usage = record
        .usage
        .as_ref()
        .expect("renderable provider has usage");
    // Slots are semantic: primary/secondary/tertiary map to fixed rows and
    // roles. A window occupies a slot only when it reports a numeric
    // usedPercent. When the primary slot is absent the present windows
    // left-compact upward so the live window drives the primary row and
    // countdown (see Usage::render_slots).
    let slots = usage.render_slots();
    let [primary, secondary, tertiary] = slots;

    // Cursor-style shared-cycle pools (Total/Auto/API) report one resetsAt and
    // windowMinutes across their slots: parallel usage categories within a
    // single monthly budget, not a live tier over a longer cap. Keep them at
    // full brightness and draw a single pacing marker instead of dimming every
    // row and repeating the identical marker.
    let assembled_windows = distinct_render_windows(&slots, &usage.extra_rate_windows, 4);
    let quaternary = assembled_windows.get(3).copied();
    let shared = shared_cycle(&[primary, secondary, tertiary, quaternary]);

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
        // The shell mono3 fallback refreshes its assembled-window reset fields
        // after the stale marker-clearing branch; preserve that byte contract.
        "mono3" if tertiary.is_none() && assembled_windows.len() >= 3 => assembled_windows
            .iter()
            .take(3)
            .map(|window| Lane::from_window(window, config, false))
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
    let mut band = RowBand {
        remaining: p_remaining,
        dim: p_long,
    };
    let mut mono_color = if mono_lanes.is_empty() {
        String::new()
    } else {
        let (remaining, dim) = mono_chunk_band(config, &mono_lanes);
        band = RowBand { remaining, dim };
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
    let cap_left = cap_text(output_format, &config.cap_left);
    style_text(
        out,
        cap_left.as_ref(),
        Some(&primary_color),
        Some(chunk_bg),
        Weight::Normal,
        output_format,
        options.color,
    );
    style_text(
        out,
        sigil,
        Some(chunk_bg),
        Some(&primary_color),
        Weight::Bold,
        output_format,
        options.color,
    );
    style_text(
        out,
        "▕",
        Some(separator_fg),
        Some(separator_bg),
        Weight::Normal,
        output_format,
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
    if !mono_lanes.is_empty() {
        let markers = mono_marker_cells(
            config,
            &mono_lanes,
            output_format.bar_width(config),
            options.now_epoch,
        );
        mono_lane_bar(
            out,
            config,
            options,
            output_format,
            &mono_lanes,
            &mono_color,
            &markers,
        );
    } else if primary.is_some() && secondary.is_none() && tertiary.is_none() {
        // Exactly one live window (a lone primary, e.g. Codex once the 5h
        // limit is dropped): one full-height bar, no empty second row. A
        // middle gap ([primary, None, tertiary]) keeps the dual body so the
        // tertiary is not dropped.
        single_metric_bar(
            out,
            config,
            options,
            output_format,
            SingleArgs {
                remaining: p_remaining,
                reset: marker_primary_reset,
                window: marker_primary_window,
                color: &primary_color,
            },
        );
    } else {
        dual_metric_bar(
            out,
            config,
            options,
            output_format,
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
        output_format,
        options.color,
    );
    let cap_right = cap_text(output_format, &config.cap_right);
    style_text(
        out,
        cap_right.as_ref(),
        Some(surface_color),
        Some(chunk_bg),
        Weight::Normal,
        output_format,
        options.color,
    );
    band
}

/// A window is "long-horizon" (a weekly/monthly cap, rendered dimmed) when it
/// reports a windowMinutes at or beyond the dim threshold. Windows without a
/// known horizon stay bright.
fn is_long_window(window: Option<&UsageWindow>, dim_window_minutes: i64) -> bool {
    window
        .and_then(UsageWindow::window_minutes)
        .is_some_and(|minutes| minutes >= dim_window_minutes)
}

/// True when at least two present render slots share one billing cycle:
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
    output_format: OutputFormat,
    args: DualArgs<'_>,
) {
    let width = output_format.bar_width(config);
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
            output_format,
            options.color,
        );
    }
    style_text(
        out,
        "▏",
        Some(&config.palette_bg),
        Some(surface_color),
        Weight::Normal,
        output_format,
        options.color,
    );
}

/// Arguments for the single full-height bar body.
struct SingleArgs<'a> {
    remaining: i32,
    reset: Option<&'a str>,
    window: Option<i64>,
    color: &'a str,
}

/// Render a single full-height bar (`█`) for a provider with exactly one live
/// window — e.g. Codex once OpenAI removed the 5h limit, leaving only the
/// weekly cap. One limit is one bar: this avoids a half-empty dual row or a
/// lone sextant lane stranded above empty rows.
fn single_metric_bar(
    out: &mut String,
    config: &RenderConfig,
    options: RenderOptions,
    output_format: OutputFormat,
    args: SingleArgs<'_>,
) {
    let width = output_format.bar_width(config);
    let surface_color = &config.palette_surface;
    let elapsed_color = &config.palette_elapsed;
    let fill = filled_cells(args.remaining, width);
    let marker = elapsed_marker_cell(
        args.reset,
        args.window,
        width,
        options.now_epoch,
        config.reset_description_timezone_offset_minutes,
    );
    for i in 0..width {
        let cell_color = if Some(i) == marker {
            elapsed_color
        } else if i < fill {
            args.color
        } else {
            surface_color
        };
        style_text(
            out,
            "█",
            Some(cell_color),
            Some(surface_color),
            Weight::Normal,
            output_format,
            options.color,
        );
    }
    style_text(
        out,
        "▏",
        Some(&config.palette_bg),
        Some(surface_color),
        Weight::Normal,
        output_format,
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

/// The representative window for a stacked chunk (mono3/mono4): its remaining
/// percentage, and whether every present lane is a long-horizon cap (which is
/// what dims the chunk).
fn mono_chunk_band(config: &RenderConfig, lanes: &[Lane<'_>]) -> (i32, bool) {
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
    (remaining, any && all_long)
}

/// One color for the whole stacked chunk (mono3/mono4): the representative
/// window's severity, dimmed only when every present lane is a long-horizon cap.
fn mono_chunk_color(config: &RenderConfig, lanes: &[Lane<'_>]) -> String {
    let (remaining, dim) = mono_chunk_band(config, lanes);
    config.window_color(remaining, dim)
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
    let mut emitted = 0usize;
    for name in &config.mono_markers {
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
        cells.push((col, colors[emitted.min(colors.len() - 1)]));
        emitted += 1;
    }
    cells
}

/// Render the single-color stacked body: three lanes pack into sextants, four
/// into octants. Each configured marker replaces its column with a colored `│`.
fn mono_lane_bar(
    out: &mut String,
    config: &RenderConfig,
    options: RenderOptions,
    output_format: OutputFormat,
    lanes: &[Lane<'_>],
    mono_color: &str,
    markers: &[(usize, &str)],
) {
    let width = output_format.bar_width(config);
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
                output_format,
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
            output_format,
            options.color,
        );
    }
    style_text(
        out,
        "▏",
        Some(&config.palette_bg),
        Some(surface_color),
        Weight::Normal,
        output_format,
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
/// positional slots not already carried by a known extra (matched on the
/// render-window dedup key) form a leading "main" family. A provider whose
/// pools live entirely in the extras (e.g. Antigravity) yields one family per
/// pool; a provider with a secondary extra pool (e.g. Codex + Spark) yields its
/// main slots plus the extra pool. `usageKnown:false` windows keep their empty
/// visual lane but cannot replace a measured positional slot.
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

/// True when an extra carries a numeric, measured usage value.
fn has_known_extra_usage(named: &NamedWindow) -> bool {
    named.usage_known != Some(false)
        && named
            .window
            .as_ref()
            .is_some_and(|window| window.used_percent.is_some())
}

/// True when a positional slot is already carried by a known extra window,
/// matched on the render-window dedup key (windowMinutes + canonical reset).
fn extra_contains(extras: &[&NamedWindow], slot: &UsageWindow) -> bool {
    extras.iter().any(|named| {
        has_known_extra_usage(named)
            && named.window.as_ref().is_some_and(|window| {
                window.window_minutes() == slot.window_minutes()
                    && window.reset_value() == slot.reset_value()
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

/// Up to `max` renderable windows for collapse decisions and non-pooled
/// mono4/mono3 fallback lanes: positional slots first, then extra rate windows
/// with known usage. This mirrors the shell jq row builder byte-for-byte,
/// including preserving duplicate render-window keys.
fn distinct_render_windows<'a>(
    slots: &[Option<&'a UsageWindow>; 3],
    extras: &'a [NamedWindow],
    max: usize,
) -> Vec<&'a UsageWindow> {
    let mut out: Vec<&'a UsageWindow> = Vec::new();
    for window in slots.iter().copied().flatten() {
        out.push(window);
        if out.len() >= max {
            return out;
        }
    }
    for named in extras {
        if !has_known_extra_usage(named) {
            continue;
        }
        let Some(window) = named.window.as_ref() else {
            continue;
        };
        out.push(window);
        if out.len() >= max {
            return out;
        }
    }
    out
}

/// A provider is model-pooled in `auto` mode when known `extraRateWindows`
/// carry every present positional slot (matched on windowMinutes + canonical
/// reset): the extras are then the canonical, complete dataset, so per-pool
/// families drive the bar instead of the (possibly cross-family) positional
/// slots.
fn pooled_auto(slots: &[Option<&UsageWindow>; 3], extras: &[NamedWindow]) -> bool {
    let present_extras: Vec<&NamedWindow> = extras
        .iter()
        .filter(|named| has_known_extra_usage(named))
        .collect();
    if present_extras.is_empty() {
        return false;
    }
    // A coincidental (windowMinutes, reset) collision between a positional slot
    // and a single extra (e.g. Codex's main weekly vs its Spark weekly) is not
    // model pooling. Require the extras to carry MORE pools than the positional
    // view exposes, so they are genuinely the canonical superset rather than a
    // same-shaped parallel pool.
    let present_positional = slots.iter().flatten().count();
    if present_extras.len() <= present_positional {
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
/// keep their visual pairing slot but cannot replace a positional measurement.
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
    let slots = usage.render_slots();
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
                    status: None,
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
    // Widen to i128 before the subtraction: `now_epoch` is externally settable
    // (SHOWY_QUOTA_NOW_EPOCH) and `start_epoch` can be a large negative i64, so a
    // plain i64 subtraction could overflow (panic in debug, wrap in release).
    let elapsed = ((now_epoch as i128 - start_epoch as i128).clamp(0, duration as i128)) as i64;
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

pub(crate) fn provider_sigil(provider: &str) -> String {
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

pub(crate) fn format_countdown(minutes: i64) -> String {
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
    output_format: OutputFormat,
    color: bool,
) {
    match output_format {
        OutputFormat::Zellij => {
            if color {
                if weight == Weight::Bold {
                    out.push_str("\x1b[1m");
                }
                if let Some(hex) = fg_hex {
                    ansi_fg(out, hex);
                }
                if let Some(hex) = bg_hex {
                    ansi_bg(out, hex);
                }
            }
            out.push_str(text);
            reset(out, output_format, color);
        }
        OutputFormat::Tmux => {
            let mut sep = "";
            out.push_str("#[");
            if let Some(hex) = fg_hex {
                out.push_str("fg=#");
                out.push_str(hex);
                sep = ",";
            }
            if let Some(hex) = bg_hex {
                out.push_str(sep);
                out.push_str("bg=#");
                out.push_str(hex);
                sep = ",";
            }
            if weight == Weight::Bold {
                out.push_str(sep);
                out.push_str("bold");
            }
            out.push(']');
            push_tmux_text(out, text);
            reset(out, output_format, color);
        }
    }
}

fn separator_space(out: &mut String, output_format: OutputFormat, bg_hex: &str, color: bool) {
    match output_format {
        OutputFormat::Zellij => out.push(' '),
        OutputFormat::Tmux => style_text(
            out,
            " ",
            Some(bg_hex),
            Some(bg_hex),
            Weight::Normal,
            output_format,
            color,
        ),
    }
}

fn cap_text(_output_format: OutputFormat, text: &str) -> Cow<'_, str> {
    Cow::Borrowed(text)
}

fn push_tmux_text(out: &mut String, text: &str) {
    let mut start = 0usize;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'#' {
            out.push_str(&text[start..index]);
            out.push_str("##");
            start = index + 1;
        }
    }
    out.push_str(&text[start..]);
}

fn ansi_fg(out: &mut String, hex: &str) {
    let (r, g, b) = hex_to_rgb(hex);
    out.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
}

fn ansi_bg(out: &mut String, hex: &str) {
    let (r, g, b) = hex_to_rgb(hex);
    out.push_str(&format!("\x1b[48;2;{r};{g};{b}m"));
}

fn reset(out: &mut String, output_format: OutputFormat, color: bool) {
    match output_format {
        OutputFormat::Zellij if color => out.push_str("\x1b[0m"),
        OutputFormat::Tmux => out.push_str("#[default]"),
        OutputFormat::Zellij => {}
    }
}

fn dim(out: &mut String, output_format: OutputFormat, color: bool) {
    match output_format {
        OutputFormat::Zellij if color => out.push_str("\x1b[2m"),
        OutputFormat::Tmux => out.push_str("#[dim]"),
        OutputFormat::Zellij => {}
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

    fn base_options(color: bool) -> RenderOptions {
        RenderOptions {
            color,
            stale: false,
            degraded_cli: false,
            now_epoch: 4_070_908_800,
        }
    }

    const CLAUDE_RENDERABLE: &[u8] = br#"[
        {
            "provider": "claude",
            "usage": {
                "primary": {
                    "usedPercent": 10,
                    "resetsAt": "2099-01-01T01:00:00Z",
                    "windowMinutes": 300
                },
                "secondary": {"usedPercent": 20}
            }
        }
    ]"#;

    const CLAUDE_THEN_CURSOR_ERROR: &[u8] = br#"[
        {
            "provider": "claude",
            "usage": {
                "primary": {
                    "usedPercent": 10,
                    "resetsAt": "2099-01-01T01:00:00Z",
                    "windowMinutes": 300
                },
                "secondary": {"usedPercent": 20}
            }
        },
        {
            "provider": "cursor",
            "error": {"message": "network failed"}
        }
    ]"#;

    #[test]
    fn zellij_error_only_renders_error_chunks_instead_of_idle() {
        let output = render_zellij(
            include_bytes!("../../../test/fixtures/codexbar-error-only.json"),
            &RenderConfig::default(),
            base_options(false),
        )
        .expect("rendered error-only fixture");

        assert_eq!(
            output,
            "\u{e0b6}CR    ⚠err    \u{e0b4} \u{e0b6}FA    ⚠err    \u{e0b4}\n"
        );
        assert!(!output.contains("AI idle"), "{output}");
    }

    #[test]
    fn tmux_error_only_renders_error_chunks_instead_of_idle() {
        let output = render_tmux(
            include_bytes!("../../../test/fixtures/codexbar-error-only.json"),
            &RenderConfig::default(),
            base_options(false),
        )
        .expect("rendered error-only fixture");

        assert!(
            output.contains(
                "#[fg=#161616,bg=#ee5396,bold]CR#[default]#[fg=#ee5396,bg=#161616]    ⚠err    #[default]"
            ),
            "{output}"
        );
        assert!(
            output.contains(
                "#[fg=#161616,bg=#ee5396,bold]FA#[default]#[fg=#ee5396,bg=#161616]    ⚠err    #[default]"
            ),
            "{output}"
        );
        assert!(!output.contains("AI idle"), "{output}");
    }

    #[test]
    fn mixed_render_keeps_provider_chunk_before_error_chunk() {
        let config = RenderConfig::default();
        let renderable = render_zellij(CLAUDE_RENDERABLE, &config, base_options(false))
            .expect("rendered renderable provider");
        let mixed = render_zellij(CLAUDE_THEN_CURSOR_ERROR, &config, base_options(false))
            .expect("rendered mixed provider/error payload");
        let renderable_chunk = renderable.trim_end_matches('\n');

        let suffix = mixed
            .strip_prefix(renderable_chunk)
            .expect("mixed output keeps renderable chunk prefix unchanged");
        assert_eq!(suffix, " \u{e0b6}CR    ⚠err    \u{e0b4}\n");
    }

    #[test]
    fn custom_error_glyph_is_honored() {
        let config = RenderConfig {
            error_glyph: "!!".into(),
            ..RenderConfig::default()
        };
        let output = render_zellij(
            include_bytes!("../../../test/fixtures/codexbar-error-only.json"),
            &config,
            base_options(false),
        )
        .expect("rendered error-only fixture");

        assert!(output.contains("!!err"), "{output}");
        assert!(!output.contains("⚠err"), "{output}");
    }

    #[test]
    fn center_pad_centers_short_text_and_leaves_long_text_untouched() {
        assert_eq!(center_pad("ab", 6), "  ab  ");
        assert_eq!(center_pad("abc", 6), " abc  ");
        assert_eq!(center_pad("abcdefgh", 6), "abcdefgh");
        assert_eq!(center_pad("⚠err", 12), "    ⚠err    ");
    }

    #[test]
    fn invalid_id_errored_records_are_dropped_to_idle() {
        let output = render_zellij(
            br#"[{"provider": "bad/id", "error": {"message": "nope"}}]"#,
            &RenderConfig::default(),
            base_options(false),
        )
        .expect("rendered invalid-id error payload");

        assert_eq!(output, "AI idle\n");
    }

    #[test]
    fn window_missing_used_percent_on_one_record_does_not_block_the_other() {
        // A sibling record with a present-but-empty window object (no
        // usedPercent) must not fail the whole payload: `slot()` drops that
        // window from its record, leaving the other provider's strip intact.
        let output = render_zellij(
            br#"[
                {"provider": "codex", "usage": {"primary": {"usedPercent": 42}}},
                {"provider": "claude", "usage": {"secondary": {"resetsAt": "2099-01-01T00:00:00Z"}}}
            ]"#,
            &RenderConfig::default(),
            base_options(false),
        )
        .expect("malformed sibling window must not fail the whole payload");

        assert!(output.contains("CX"), "{output}");
    }

    #[test]
    fn tmux_style_text_emits_markup_chunk() {
        let mut out = String::new();

        style_text(
            &mut out,
            "CL",
            Some("161616"),
            Some("25be6a"),
            Weight::Bold,
            OutputFormat::Tmux,
            false,
        );

        assert_eq!(out, "#[fg=#161616,bg=#25be6a,bold]CL#[default]");
    }

    #[test]
    fn tmux_style_text_doubles_hash_in_text() {
        let mut out = String::new();

        style_text(
            &mut out,
            "#(",
            Some("161616"),
            Some("25be6a"),
            Weight::Normal,
            OutputFormat::Tmux,
            false,
        );

        assert_eq!(out, "#[fg=#161616,bg=#25be6a]##(#[default]");
    }

    #[test]
    fn tmux_idle_is_dim_unknown_and_omits_trailing_markers() {
        let output = render_tmux(
            include_bytes!("../../../test/fixtures/codexbar-empty.json"),
            &RenderConfig::default(),
            RenderOptions {
                color: false,
                stale: true,
                degraded_cli: true,
                now_epoch: 4_070_908_800,
            },
        )
        .expect("rendered tmux idle fixture");

        assert_eq!(output, "#[dim]#[fg=#6c7086]AI idle#[default]");
    }

    #[test]
    fn tmux_gap_and_cap_hash_escaping_match_shell_markup() {
        let mut out = String::new();

        separator_space(&mut out, OutputFormat::Tmux, "161616", false);
        let cap = cap_text(OutputFormat::Tmux, "#R#");
        style_text(
            &mut out,
            cap.as_ref(),
            Some("25be6a"),
            Some("161616"),
            Weight::Normal,
            OutputFormat::Tmux,
            false,
        );

        assert_eq!(
            out,
            "#[fg=#161616,bg=#161616] #[default]#[fg=#25be6a,bg=#161616]##R###[default]"
        );
    }

    #[test]
    fn mono_marker_colors_use_visible_rank_after_gap() {
        let config = RenderConfig {
            mono_markers: vec!["primary".into(), "secondary".into()],
            ..RenderConfig::default()
        };
        let lanes = [
            Lane::empty(),
            Lane {
                remaining: 90,
                reset: Some("2024-01-01T02:00:00Z"),
                window: Some(120),
                is_long: false,
                present: true,
            },
        ];

        let cells = mono_marker_cells(&config, &lanes, 8, 1_704_067_200);

        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].1, config.palette_elapsed.as_str());
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
    fn parses_reset_description_meridiem_boundary() {
        // `12 AM` and `12 PM` are the two special-cased arms of the meridiem
        // conversion: 12 AM means hour 0, 12 PM means hour 12. Every other hour
        // takes the third arm, which the test above already covers. Pin the
        // offset to UTC; now = 2024-01-01 06:00:00 UTC.
        //
        // 12 PM is noon on the same day, four hours ahead of now. Dropping the
        // `hour != 12` guard would push it to hour 24, which `Time::from_hms`
        // rejects, so the arm would return None instead of a later time.
        assert_eq!(
            reset_epoch("Resets 12:00 PM", 1_704_088_800, Some(0)),
            Some(1_704_110_400)
        );
        // 12 AM is midnight, which is already behind now, so it rolls to the
        // next day. Dropping the `hour == 12` arm would read it as noon and
        // return 1_704_110_400 — the same value as 12 PM above, and no roll.
        assert_eq!(
            reset_epoch("Resets 12:00 AM", 1_704_088_800, Some(0)),
            Some(1_704_153_600)
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
    fn elapsed_marker_cell_does_not_overflow_at_extreme_now_epoch() {
        // now_epoch near i64::MAX with a window whose start_epoch is deeply
        // negative must not overflow the elapsed subtraction (panic in debug,
        // wrap in release). A tiny reset epoch with a large window drives
        // start_epoch negative. The marker is still clamped into [0, width).
        let marker = elapsed_marker_cell(
            Some("1970-01-02T00:00:00Z"),
            Some(10_080),
            10,
            i64::MAX,
            Some(0),
        );
        assert!(matches!(marker, Some(m) if m < 10));
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
    fn promotes_live_window_when_primary_absent() {
        // OpenAI temporarily removed Codex's 5h limit: usage.primary is null
        // and the weekly cap is the live window. It must drive the primary row
        // and countdown (a real reset, not an empty top + idle), and the
        // coincidental Spark weekly must not make Codex look model-pooled.
        let config = RenderConfig::default();
        let output = render_zellij(
            br#"[
                {
                    "provider": "codex",
                    "usage": {
                        "primary": null,
                        "secondary": {"usedPercent": 0, "windowMinutes": 10080, "resetsAt": "2099-01-07T00:00:00Z"},
                        "tertiary": null,
                        "extraRateWindows": [
                            {"id": "codex-spark-weekly", "title": "Codex Spark Weekly", "window": {"usedPercent": 0, "windowMinutes": 10080, "resetsAt": "2099-01-07T00:00:00Z"}}
                        ]
                    }
                }
            ]"#,
            &config,
            RenderOptions {
                color: true,
                stale: false,
                degraded_cli: false,
                now_epoch: 4_070_908_800,
            },
        )
        .expect("rendered promoted-primary provider");

        assert!(output.contains("CX"), "{output}");
        // The promoted weekly counts down instead of showing an idle top row.
        assert!(output.contains("6d"), "{output}");
        assert!(!output.contains("idle"), "{output}");
        // Not model-pooled: a single CX chunk, never a per-family split.
        assert!(!output.contains('ˢ') && !output.contains('ᶜ'), "{output}");
        // One live window renders as a single full-height bar (`█`): dim-good
        // fill over the empty surface. There is no half-block (`▀`) split and
        // therefore no empty second row.
        assert!(
            output.contains("\u{1b}[38;2;20;104;58m\u{1b}[48;2;42;42;42m\u{2588}"),
            "weekly must render as a full-height fill: {output}"
        );
        assert!(
            !output.contains('\u{2580}'),
            "single window must not use a half-block: {output}"
        );
    }

    #[test]
    fn middle_gap_keeps_dual_body_not_single_bar() {
        // [primary, None, tertiary]: two live windows with a missing secondary.
        // The single-bar path is only for exactly one window, so this must keep
        // the dual body (half-blocks) and not collapse — dropping the tertiary.
        let config = RenderConfig {
            terminal_bar_mode: "dual".into(),
            zellij_bar_width: 8,
            ..RenderConfig::default()
        };
        let output = render_zellij(
            br#"[
                {
                    "provider": "codex",
                    "usage": {
                        "primary": {"usedPercent": 20, "windowMinutes": 300, "resetsAt": "2099-01-01T05:00:00Z"},
                        "secondary": null,
                        "tertiary": {"usedPercent": 40, "windowMinutes": 10080, "resetsAt": "2099-01-07T00:00:00Z"}
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
        .expect("rendered middle-gap provider");
        assert!(
            output.contains('\u{2580}'),
            "middle gap must keep the dual half-block body: {output}"
        );
        assert!(
            !output.contains('\u{2588}'),
            "middle gap must not collapse to a single full bar: {output}"
        );
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
    fn extra_contains_uses_reset_description_when_timestamp_is_absent() {
        let slot = UsageWindow {
            used_percent: Some(10.0),
            resets_at: None,
            reset_description: Some("Resets Monday at noon".into()),
            window_minutes: Some(10_080),
        };
        let different_reset = NamedWindow {
            id: None,
            title: None,
            window: Some(UsageWindow {
                used_percent: Some(20.0),
                resets_at: None,
                reset_description: Some("Resets Friday at noon".into()),
                window_minutes: Some(10_080),
            }),
            usage_known: None,
        };
        let matching_reset = NamedWindow {
            id: None,
            title: None,
            window: Some(UsageWindow {
                used_percent: Some(30.0),
                resets_at: None,
                reset_description: Some("Resets Monday at noon".into()),
                window_minutes: Some(10_080),
            }),
            usage_known: None,
        };

        assert!(!extra_contains(&[&different_reset], &slot));
        assert!(extra_contains(&[&matching_reset], &slot));

        let other_slot = UsageWindow {
            used_percent: Some(15.0),
            resets_at: None,
            reset_description: Some("Resets Tuesday at noon".into()),
            window_minutes: Some(10_080),
        };
        let unrelated_one = NamedWindow {
            id: None,
            title: None,
            window: Some(UsageWindow {
                used_percent: Some(40.0),
                resets_at: None,
                reset_description: Some("Resets Wednesday at noon".into()),
                window_minutes: Some(10_080),
            }),
            usage_known: None,
        };
        let unrelated_two = NamedWindow {
            id: None,
            title: None,
            window: Some(UsageWindow {
                used_percent: Some(50.0),
                resets_at: None,
                reset_description: Some("Resets Thursday at noon".into()),
                window_minutes: Some(10_080),
            }),
            usage_known: None,
        };
        let slots = [Some(&slot), Some(&other_slot), None];
        let extras = vec![matching_reset, unrelated_one, unrelated_two];

        assert!(!pooled_auto(&slots, &extras));
    }

    #[test]
    fn unknown_extra_cannot_pool_or_replace_measured_slot() {
        let measured = cycle_window("2099-01-15T00:00:00Z", 300);
        let unknown_duplicate = NamedWindow {
            id: None,
            title: Some("Unknown".into()),
            window: Some(measured.clone()),
            usage_known: Some(false),
        };
        let known_other_pool = NamedWindow {
            id: None,
            title: Some("Known".into()),
            window: Some(cycle_window("2099-01-22T00:00:00Z", 10_080)),
            usage_known: None,
        };
        let slots = [Some(&measured), None, None];
        let extras = vec![unknown_duplicate, known_other_pool];

        assert!(!pooled_auto(&slots, &extras));

        let families = family_windows("antigravity", &slots, &extras);
        assert_eq!(families[0].primary.used_percent, Some(10.0));
        assert_eq!(families[1].primary.used_percent, None);
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
    fn shared_cycle_false_when_fourth_lane_reset_differs() {
        let config = RenderConfig {
            terminal_bar_mode: "mono4".into(),
            zellij_bar_width: 8,
            palette_dim_good: Some("010203".into()),
            mono_markers: Vec::new(),
            ..RenderConfig::default()
        };
        let output = render_zellij(
            br#"[
                {
                    "provider": "codex",
                    "usage": {
                        "primary": {
                            "usedPercent": 10,
                            "resetsAt": "2099-01-15T00:00:00Z",
                            "windowMinutes": 43200
                        },
                        "secondary": {
                            "usedPercent": 10,
                            "resetsAt": "2099-01-15T00:00:00Z",
                            "windowMinutes": 43200
                        },
                        "tertiary": {
                            "usedPercent": 10,
                            "resetsAt": "2099-01-15T00:00:00Z",
                            "windowMinutes": 43200
                        },
                        "extraRateWindows": [
                            {
                                "window": {
                                    "usedPercent": 10,
                                    "resetsAt": "2099-01-16T00:00:00Z",
                                    "windowMinutes": 43200
                                }
                            }
                        ]
                    }
                }
            ]"#,
            &config,
            RenderOptions {
                color: true,
                stale: false,
                degraded_cli: false,
                now_epoch: 4_070_908_800,
            },
        )
        .expect("rendered mono4 fixture");

        assert!(output.contains("38;2;1;2;3"), "{output}");
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
    fn row_options(now_epoch: i64) -> RenderOptions {
        RenderOptions {
            color: false,
            stale: false,
            degraded_cli: false,
            now_epoch,
        }
    }

    #[test]
    fn rows_are_the_renderer_own_chunks_not_raw_windows() {
        let config = RenderConfig::default();
        let options = row_options(4_070_908_800);

        // A model-pooled provider expands to one row per family, even though it
        // is a single record with several windows.
        let pooled = render_rows(
            include_bytes!("../../../test/fixtures/codexbar-antigravity-quad.json"),
            &config,
            options,
            OutputFormat::Zellij,
        )
        .expect("rendered rows");
        let sigils: Vec<&str> = pooled.iter().map(|row| row.sigil.as_str()).collect();
        assert_eq!(sigils, vec!["AGᴳ", "AGᶜ"]);

        // A stacked-mode provider keeps every lane inside one chunk, so it must
        // not be split into a row per window.
        let stacked = render_rows(
            include_bytes!("../../../test/fixtures/codexbar-cursor.json"),
            &config,
            options,
            OutputFormat::Zellij,
        )
        .expect("rendered rows");
        assert_eq!(stacked.len(), 1, "{stacked:?}");
        assert!(stacked[0].text.contains('│'), "{stacked:?}");
        assert!(stacked.iter().all(|row| !row.text.contains('\x1b')));
    }
    #[test]
    fn rows_rejoin_into_the_single_line_strip() {
        let payload = include_bytes!("../../../test/fixtures/codexbar-realistic.json");
        let config = RenderConfig::default();
        let options = row_options(4_070_908_800);

        let rows = render_rows(payload, &config, options, OutputFormat::Zellij)
            .expect("rendered rows")
            .iter()
            .map(|row| row.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        let strip = render_zellij(payload, &config, options).expect("rendered strip");

        // Rows must be the strip's own chunks: same glyphs, caps and countdown,
        // only the separator and trailing newline belong to the strip.
        assert_eq!(format!("{rows}\n"), strip);
    }

    #[test]
    fn rows_report_the_band_used_to_colour_each_chunk() {
        let payload = include_bytes!("../../../test/fixtures/codexbar-low.json");
        let config = RenderConfig::default();
        let plain = render_rows(
            payload,
            &config,
            row_options(4_070_908_800),
            OutputFormat::Zellij,
        )
        .expect("rendered rows");
        let colored = render_rows(
            payload,
            &config,
            RenderOptions {
                color: true,
                stale: false,
                degraded_cli: false,
                now_epoch: 4_070_908_800,
            },
            OutputFormat::Zellij,
        )
        .expect("rendered rows");

        assert!(!plain.is_empty());
        assert_eq!(plain.len(), colored.len());
        for (row, drawn) in plain.iter().zip(&colored) {
            let severity = row.severity.expect("usage chunk carries a band");
            assert_eq!(row.color, config.severity_color(severity, row.dim));
            // The reported colour is the one actually drawn: the sigil cell uses
            // the chunk colour as its background.
            let (r, g, b) = hex_to_rgb(&row.color);
            assert!(
                drawn.text.contains(&format!("48;2;{r};{g};{b}")),
                "{drawn:?} missing {r};{g};{b}"
            );
        }
        assert!(
            plain.iter().any(|row| row.severity == Some(Severity::Bad)),
            "{plain:?}"
        );
    }

    #[test]
    fn error_rows_carry_no_band() {
        let rows = render_rows(
            include_bytes!("../../../test/fixtures/codexbar-error-only.json"),
            &RenderConfig::default(),
            row_options(4_070_908_800),
            OutputFormat::Zellij,
        )
        .expect("rendered rows");

        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| row.error && row.severity.is_none()));
    }

    #[test]
    fn rows_are_empty_when_nothing_is_renderable() {
        let rows = render_rows(
            include_bytes!("../../../test/fixtures/codexbar-empty.json"),
            &RenderConfig::default(),
            row_options(4_070_908_800),
            OutputFormat::Zellij,
        )
        .expect("rendered rows");

        // The strip would print `AI idle`; rows leave that affordance to the
        // surface instead of inventing a provider row.
        assert!(rows.is_empty(), "{rows:?}");
        assert!(render_zellij(
            include_bytes!("../../../test/fixtures/codexbar-empty.json"),
            &RenderConfig::default(),
            row_options(4_070_908_800),
        )
        .expect("rendered strip")
        .contains("AI idle"));
    }

    #[test]
    fn stale_rows_keep_their_band_for_the_surface_to_override() {
        let payload = include_bytes!("../../../test/fixtures/codexbar-realistic.json");
        let config = RenderConfig::default();
        let rows = render_rows(
            payload,
            &config,
            RenderOptions {
                color: false,
                stale: true,
                degraded_cli: false,
                now_epoch: 4_070_908_800,
            },
            OutputFormat::Zellij,
        )
        .expect("rendered rows");

        // Staleness greys the strip, but the band still describes the usage, so
        // a surface can pick its own stale styling without losing the reading.
        assert!(rows.iter().any(|row| row.severity.is_some()), "{rows:?}");
    }

    // One live 5h window, one weekly cap, and a distinct weekly pool that
    // shares the cap's reset — the shape that made the horizontal strip pack
    // windows into half blocks in the first place.
    const VERTICAL_CLAUDE: &[u8] = br#"[
        {
            "provider": "claude",
            "usage": {
                "primary": {
                    "usedPercent": 10,
                    "resetsAt": "2099-01-01T01:00:00Z",
                    "windowMinutes": 300
                },
                "secondary": {
                    "usedPercent": 20,
                    "resetsAt": "2099-01-03T00:00:00Z",
                    "windowMinutes": 10080
                },
                "extraRateWindows": [
                    {
                        "title": "Fable only",
                        "window": {
                            "usedPercent": 28,
                            "resetsAt": "2099-01-03T00:00:00Z",
                            "windowMinutes": 10080
                        }
                    }
                ]
            }
        }
    ]"#;

    // Cursor's Total/Auto/API: three separate pools reporting one identical
    // reset, horizon and usage on a single monthly cycle.
    const VERTICAL_SHARED_CYCLE: &[u8] = br#"[
        {
            "provider": "cursor",
            "usage": {
                "primary": {
                    "usedPercent": 90,
                    "resetsAt": "2099-01-20T00:00:00Z",
                    "windowMinutes": 44640
                },
                "secondary": {
                    "usedPercent": 90,
                    "resetsAt": "2099-01-20T00:00:00Z",
                    "windowMinutes": 44640
                },
                "tertiary": {
                    "usedPercent": 90,
                    "resetsAt": "2099-01-20T00:00:00Z",
                    "windowMinutes": 44640
                }
            }
        }
    ]"#;

    fn vertical_lines(
        payload: &[u8],
        config: &RenderConfig,
        options: RenderOptions,
    ) -> Vec<String> {
        render_vertical(payload, config, options)
            .expect("rendered vertical view")
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Cells inside the plate, so a bar's width can be measured without caring
    /// which glyph a cell drew (a pacing tick is a cell too).
    fn plate_cells(line: &str) -> usize {
        let body = line
            .split_once('▕')
            .and_then(|(_, rest)| rest.split_once('▏'))
            .map(|(body, _)| body)
            .unwrap_or_default();
        body.chars().count()
    }

    #[test]
    fn vertical_gives_every_window_its_own_line_with_its_own_reading() {
        let lines = vertical_lines(
            VERTICAL_CLAUDE,
            &RenderConfig::default(),
            base_options(false),
        );

        // Three windows, three lines: no half-block packing, and each line
        // carries the percent and countdown of its own window rather than the
        // provider's primary.
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].starts_with("\u{e0b6}CL\u{e0b4} 5h"), "{lines:?}");
        assert!(lines[0].contains(" 90% 1h"), "{lines:?}");
        assert!(lines[1].contains(" 80% 2d"), "{lines:?}");
        assert!(lines[2].contains(" 72% 2d"), "{lines:?}");
        // The chip labels the block once, and a continuation chip carries no
        // colour: a tinted letterless stub reads as the first cells of the bar.
        assert!(lines[1].starts_with("\u{e0b6}  \u{e0b4}"), "{lines:?}");
        assert!(lines[2].starts_with("\u{e0b6}  \u{e0b4}"), "{lines:?}");
    }

    #[test]
    fn vertical_keeps_the_label_outside_the_plate() {
        let lines = vertical_lines(
            VERTICAL_CLAUDE,
            &RenderConfig::default(),
            base_options(false),
        );

        // The label precedes the plate edge. Inside the plate it shared the
        // track's background, which hid where measurement begins.
        for line in &lines {
            let (head, _) = line.split_once('▕').expect("plate edge");
            assert!(head.contains('d') || head.contains('h'), "{lines:?}");
        }
        assert!(lines[0].contains(" 5h  ▕"), "{lines:?}");
        assert!(lines[1].contains(" 7d  ▕"), "{lines:?}");
        assert!(lines[2].contains(" 7dᶠ ▕"), "{lines:?}");
    }

    #[test]
    fn vertical_tags_only_the_windows_a_horizon_cannot_identify() {
        let lines = vertical_lines(
            VERTICAL_CLAUDE,
            &RenderConfig::default(),
            base_options(false),
        );

        // The 5h window is alone on its horizon, and the weekly slot is the only
        // nameless slot on its own, so only the named extra earns a tag.
        assert!(lines[0].contains(" 5h  "), "{lines:?}");
        assert!(lines[1].contains(" 7d  "), "{lines:?}");
        assert!(lines[2].contains(" 7dᶠ "), "{lines:?}");
    }

    #[test]
    fn vertical_names_a_slot_from_the_extra_that_republishes_it() {
        // Antigravity's shape: two weekly slots whose only distinguishing
        // information is the title CodexBar publishes on the matching extras.
        const POOLED: &[u8] = br#"[
            {
                "provider": "antigravity",
                "usage": {
                    "primary": {
                        "usedPercent": 100,
                        "resetsAt": "2099-01-03T00:00:00Z",
                        "windowMinutes": 10080
                    },
                    "secondary": {
                        "usedPercent": 100,
                        "resetsAt": "2099-01-05T00:00:00Z",
                        "windowMinutes": 10080
                    },
                    "extraRateWindows": [
                        {
                            "title": "Gemini weekly",
                            "window": {
                                "usedPercent": 100,
                                "resetsAt": "2099-01-03T00:00:00Z",
                                "windowMinutes": 10080
                            }
                        },
                        {
                            "title": "Claude/GPT weekly",
                            "window": {
                                "usedPercent": 100,
                                "resetsAt": "2099-01-05T00:00:00Z",
                                "windowMinutes": 10080
                            }
                        }
                    ]
                }
            }
        ]"#;

        let lines = vertical_lines(POOLED, &RenderConfig::default(), base_options(false));

        // The duplicate extras add no lines, but their names beat slot ordinals:
        // the distinction the reader cares about is the model family.
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains(" 7dᴳ "), "{lines:?}");
        assert!(lines[1].contains(" 7dᶜ "), "{lines:?}");
    }

    #[test]
    fn vertical_keeps_same_cycle_slots_as_separate_ordinal_lines() {
        let lines = vertical_lines(
            VERTICAL_SHARED_CYCLE,
            &RenderConfig::default(),
            base_options(false),
        );

        // Identical reset, horizon and usage do not make these one window: they
        // are three pools, so they keep three lines, ordered by slot. With no
        // titles to borrow, ordinals are the only honest distinction.
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains(" 1mo¹ "), "{lines:?}");
        assert!(lines[1].contains(" 1mo² "), "{lines:?}");
        assert!(lines[2].contains(" 1mo³ "), "{lines:?}");
    }

    #[test]
    fn vertical_labels_a_monthly_cycle_in_months_not_calendar_days() {
        // 44640 minutes is 31 days and 43200 is 30; both are one monthly cycle,
        // and printing the day count invites a comparison that means nothing.
        assert_eq!(horizon_label(Some(44640)), "1mo");
        assert_eq!(horizon_label(Some(43200)), "1mo");
        assert_eq!(horizon_label(Some(10080)), "7d");
        assert_eq!(horizon_label(Some(300)), "5h");
        assert_eq!(horizon_label(Some(45)), "45m");
        assert_eq!(horizon_label(None), "?");
    }

    #[test]
    fn vertical_never_dims_a_long_horizon_window() {
        let config = RenderConfig::default();
        let lines = vertical_lines(VERTICAL_CLAUDE, &config, base_options(true));

        // The weekly window (80% remaining, `dim_window_minutes` reached) prints
        // the bright band: this view writes the horizon in its own column, so dim
        // would only cost contrast on the row's most important glyphs.
        let (r, g, b) = hex_to_rgb(&config.severity_color(Severity::Good, false));
        assert!(
            lines[1].contains(&format!("38;2;{r};{g};{b}m")),
            "{lines:?}"
        );
        let (r, g, b) = hex_to_rgb(&config.severity_color(Severity::Good, true));
        assert!(
            !lines[1].contains(&format!("38;2;{r};{g};{b}m")),
            "{lines:?}"
        );
    }

    #[test]
    fn vertical_pacing_marker_never_consumes_a_measured_cell() {
        const FULL: &[u8] = br#"[
            {
                "provider": "claude",
                "usage": {
                    "primary": {
                        "usedPercent": 0,
                        "resetsAt": "2099-01-01T02:30:00Z",
                        "windowMinutes": 300
                    }
                }
            }
        ]"#;

        let config = RenderConfig::default();
        let lines = vertical_lines(FULL, &config, base_options(true));

        // A full bar with a marker mid-body used to show a coloured hole. The
        // tick is drawn over the track, so the cell keeps its fill as background.
        assert!(lines[0].contains('│'), "{lines:?}");
        let (r, g, b) = hex_to_rgb(&config.severity_color(Severity::Good, false));
        // Count inside the plate only: the sigil chip is painted with the same
        // fill colour and would otherwise be counted as a bar cell.
        let plate = lines[0]
            .split_once('▕')
            .and_then(|(_, rest)| rest.split_once('▏'))
            .map(|(body, _)| body)
            .expect("plate body");
        let filled = plate.matches(&format!("48;2;{r};{g};{b}m")).count();
        assert_eq!(filled, config.vertical_bar_width, "{lines:?}");
    }

    #[test]
    fn vertical_bar_width_sets_the_body_and_is_clamped_to_a_readable_floor() {
        let mut config = RenderConfig {
            vertical_bar_width: 8,
            ..RenderConfig::default()
        };
        let lines = vertical_lines(VERTICAL_CLAUDE, &config, base_options(false));
        assert_eq!(plate_cells(&lines[0]), 8, "{lines:?}");

        // A width below the floor would render a bar too coarse to read a
        // percentage from; the strip clamps the same way.
        config.vertical_bar_width = 1;
        let lines = vertical_lines(VERTICAL_CLAUDE, &config, base_options(false));
        assert_eq!(plate_cells(&lines[0]), 8, "{lines:?}");
    }

    #[test]
    fn vertical_separates_provider_blocks_with_one_blank_line() {
        let payload = [
            String::from_utf8(VERTICAL_CLAUDE.to_vec()).expect("utf8"),
            String::from_utf8(VERTICAL_SHARED_CYCLE.to_vec()).expect("utf8"),
        ]
        .join(",")
        .replace("],[", ",");
        let lines = vertical_lines(
            payload.as_bytes(),
            &RenderConfig::default(),
            base_options(false),
        );

        // Grouping is otherwise carried only by which chip has letters, which is
        // too subtle to parse at a glance on a phone.
        assert_eq!(lines.len(), 7, "{lines:?}");
        assert_eq!(lines[3], "", "{lines:?}");
        assert!(lines[4].starts_with("\u{e0b6}CR\u{e0b4}"), "{lines:?}");
    }

    #[test]
    fn vertical_urgency_order_puts_the_window_about_to_run_out_first() {
        let payload = [
            String::from_utf8(VERTICAL_CLAUDE.to_vec()).expect("utf8"),
            String::from_utf8(VERTICAL_SHARED_CYCLE.to_vec()).expect("utf8"),
        ]
        .join(",")
        .replace("],[", ",");
        let config = RenderConfig {
            vertical_sort: "urgency".into(),
            ..RenderConfig::default()
        };
        let lines = vertical_lines(payload.as_bytes(), &config, base_options(false));

        // Provider blocks are flattened, so every line carries its own chip and
        // no blank separators are drawn.
        assert_eq!(lines.len(), 6, "{lines:?}");
        assert!(lines.iter().all(|line| !line.is_empty()), "{lines:?}");
        assert!(lines[0].contains("CR"), "{lines:?}");
        assert!(lines[0].contains(" 10%"), "{lines:?}");
        assert!(lines[5].contains("CL"), "{lines:?}");
        assert!(lines[5].contains(" 90%"), "{lines:?}");
        // Same-cycle ties keep CodexBar's own slot order rather than sorting by
        // superscript codepoint.
        assert!(lines[0].contains("1mo¹"), "{lines:?}");
        assert!(lines[1].contains("1mo²"), "{lines:?}");
        assert!(lines[2].contains("1mo³"), "{lines:?}");
    }

    #[test]
    fn vertical_reset_clock_answers_when_by_default() {
        let config = RenderConfig {
            reset_description_timezone_offset_minutes: Some(0),
            ..RenderConfig::default()
        };
        let lines = vertical_lines(VERTICAL_CLAUDE, &config, base_options(false));

        // Rows that all read `2d` say nothing about *when*; the clock answers it
        // in six columns without replacing the relative countdown. Countdowns are
        // padded to the widest form, so `1h` carries trailing space.
        assert!(lines[0].contains(" 90% 1h "), "{lines:?}");
        assert!(lines[0].ends_with(" 01:00"), "{lines:?}");
        assert!(lines[1].ends_with(" 00:00"), "{lines:?}");
    }

    #[test]
    fn vertical_reset_clock_can_be_traded_back_for_six_columns() {
        let plain = RenderConfig {
            vertical_reset_clock: false,
            reset_description_timezone_offset_minutes: Some(0),
            ..RenderConfig::default()
        };
        let lines = vertical_lines(VERTICAL_CLAUDE, &plain, base_options(false));
        let clocked = vertical_lines(
            VERTICAL_CLAUDE,
            &RenderConfig {
                reset_description_timezone_offset_minutes: Some(0),
                ..RenderConfig::default()
            },
            base_options(false),
        );

        // Opting out costs the wall time but nothing else: the countdown stays,
        // and the six columns are the only difference in the line.
        assert!(lines[0].contains(" 90% 1h"), "{lines:?}");
        assert!(!lines[0].contains("01:00"), "{lines:?}");
        assert_eq!(
            clocked[0].chars().count() - lines[0].chars().count(),
            6,
            "{lines:?} {clocked:?}"
        );
    }

    #[test]
    fn vertical_puts_strip_level_state_on_its_own_trailing_line() {
        let config = RenderConfig::default();
        let lines = vertical_lines(
            VERTICAL_CLAUDE,
            &config,
            RenderOptions {
                color: false,
                stale: true,
                degraded_cli: true,
                now_epoch: 4_070_908_800,
            },
        );

        // Stale/degraded are properties of the snapshot, not of one window, and
        // a vertical view has no shared line to trail them on.
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_eq!(
            lines[3],
            format!("{} {}", config.stale_glyph, config.degraded_cli_glyph),
            "{lines:?}"
        );
        // A stale snapshot cannot place a pacing marker, but the countdown is the
        // reading rather than the pacing, so it survives and only turns grey.
        assert!(!lines[0].contains('│'), "{lines:?}");
        assert!(lines[0].contains(" 90% 1h"), "{lines:?}");
    }

    #[test]
    fn vertical_renders_idle_when_nothing_is_renderable() {
        let lines = vertical_lines(
            include_bytes!("../../../test/fixtures/codexbar-empty.json"),
            &RenderConfig::default(),
            base_options(false),
        );

        assert_eq!(lines, vec!["AI idle".to_string()], "{lines:?}");
    }
}
