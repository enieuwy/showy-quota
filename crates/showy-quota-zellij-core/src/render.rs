use std::cmp::Ordering;

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Duration, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

use crate::codexbar::{is_renderable, parse_usage_payload, ProviderRecord, UsageWindow};
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

pub fn render_records(
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
        for (idx, record) in records.into_iter().enumerate() {
            if idx > 0 {
                out.push(' ');
            }
            render_provider(&mut out, record, config, options, chunk_bg, stale_color);
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

    let p_used = primary.map_or(-1, UsageWindow::used_pct_floor);
    let s_used = secondary.map_or(-1, UsageWindow::used_pct_floor);
    let t_used = tertiary.map_or(-1, UsageWindow::used_pct_floor);
    let p_remaining = if p_used >= 0 { 100 - p_used } else { 0 };
    let s_remaining = if s_used >= 0 { 100 - s_used } else { 0 };
    let t_remaining = if t_used >= 0 { 100 - t_used } else { 0 };

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
    let p_long = is_long_window(primary, config.dim_window_minutes);
    let s_long = is_long_window(secondary, config.dim_window_minutes);
    let t_long = is_long_window(tertiary, config.dim_window_minutes);
    let mut primary_color = config.window_color(p_remaining, p_long);
    let mut secondary_color = config.window_color(s_remaining, s_long);
    let mut tertiary_color = config.window_color(t_remaining, t_long);
    let bar_mode = terminal_mode_for_provider(config, &record.provider, tertiary.is_some());
    let mut mono_color = String::new();
    if bar_mode == "mono3" {
        // mono3 has one color for the whole chunk, so dimming is all-or-nothing:
        // dim only when every present window is a long-horizon cap (e.g.
        // all-monthly), bright when any live short tier is present.
        let mut any = false;
        let mut all_long = true;
        for (used, long) in [(p_used, p_long), (s_used, s_long), (t_used, t_long)] {
            if used >= 0 {
                any = true;
                all_long &= long;
            }
        }
        let remaining = mono3_remaining(
            config,
            p_remaining,
            s_remaining,
            t_remaining,
            p_used,
            s_used,
            t_used,
        );
        mono_color = config.window_color(remaining, any && all_long);
        primary_color.clone_from(&mono_color);
    }
    if options.stale {
        primary_color = stale_color.to_string();
        secondary_color = stale_color.to_string();
        tertiary_color = stale_color.to_string();
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
        &provider_sigil(&record.provider),
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
    let marker_secondary_reset = if options.stale {
        None
    } else {
        secondary.and_then(UsageWindow::reset_value)
    };
    let marker_secondary_window = if options.stale {
        None
    } else {
        secondary.and_then(UsageWindow::window_minutes)
    };
    let marker_tertiary_reset = if options.stale {
        None
    } else {
        tertiary.and_then(UsageWindow::reset_value)
    };
    let marker_tertiary_window = if options.stale {
        None
    } else {
        tertiary.and_then(UsageWindow::window_minutes)
    };

    match bar_mode.as_str() {
        "sextant3" => tri_metric_bar(
            out,
            config,
            options,
            TriArgs {
                p_remaining,
                s_remaining,
                t_remaining,
                primary_color: &primary_color,
                secondary_color: &secondary_color,
                tertiary_color: &tertiary_color,
            },
        ),
        "mono3" => tri_metric_bar_mono(
            out,
            config,
            options,
            MonoArgs {
                p_remaining,
                s_remaining,
                t_remaining,
                p_used,
                s_used,
                t_used,
                p_reset: marker_primary_reset,
                s_reset: marker_secondary_reset,
                t_reset: marker_tertiary_reset,
                p_window: marker_primary_window,
                s_window: marker_secondary_window,
                t_window: marker_tertiary_window,
                mono_color: &mono_color,
            },
        ),
        _ => dual_metric_bar(
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
        ),
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
    window.as_ref().filter(|window| window.used_percent.is_some())
}

/// A window is "long-horizon" (a weekly/monthly cap, rendered dimmed) when it
/// reports a windowMinutes at or beyond the dim threshold. Windows without a
/// known horizon stay bright.
fn is_long_window(window: Option<&UsageWindow>, dim_window_minutes: i64) -> bool {
    window
        .and_then(UsageWindow::window_minutes)
        .is_some_and(|minutes| minutes >= dim_window_minutes)
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
    let width = config.zellij_bar_width.max(8);
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

struct TriArgs<'a> {
    p_remaining: i32,
    s_remaining: i32,
    t_remaining: i32,
    primary_color: &'a str,
    secondary_color: &'a str,
    tertiary_color: &'a str,
}

fn tri_metric_bar(
    out: &mut String,
    config: &RenderConfig,
    options: RenderOptions,
    args: TriArgs<'_>,
) {
    let width = config.zellij_bar_width.max(8);
    let surface_color = &config.palette_surface;
    let p_fill = filled_cells(args.p_remaining, width);
    let s_fill = filled_cells(args.s_remaining, width);
    let t_fill = filled_cells(args.t_remaining, width);

    for i in 0..width {
        let mut mask = 0;
        if i < p_fill {
            mask |= 1;
        }
        if i < s_fill {
            mask |= 2;
        }
        if i < t_fill {
            mask |= 4;
        }
        let cell_color = if mask & 4 != 0 {
            args.tertiary_color
        } else if mask & 2 != 0 {
            args.secondary_color
        } else if mask & 1 != 0 {
            args.primary_color
        } else {
            surface_color
        };
        style_text(
            out,
            sextant_mask_char(mask),
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

struct MonoArgs<'a> {
    p_remaining: i32,
    s_remaining: i32,
    t_remaining: i32,
    p_used: i32,
    s_used: i32,
    t_used: i32,
    p_reset: Option<&'a str>,
    s_reset: Option<&'a str>,
    t_reset: Option<&'a str>,
    p_window: Option<i64>,
    s_window: Option<i64>,
    t_window: Option<i64>,
    mono_color: &'a str,
}

fn tri_metric_bar_mono(
    out: &mut String,
    config: &RenderConfig,
    options: RenderOptions,
    args: MonoArgs<'_>,
) {
    let width = config.zellij_bar_width.max(8);
    let surface_color = &config.palette_surface;
    let elapsed_color = &config.palette_elapsed;
    let p_fill = filled_cells(args.p_remaining, width);
    let s_fill = filled_cells(args.s_remaining, width);
    let t_fill = filled_cells(args.t_remaining, width);
    let mut marker = mono3_marker_boundary(config, &args, width, options.now_epoch);

    if config.mono3_marker_style == "replace" && marker == Some(width) {
        marker = Some(width - 1);
    }

    for i in 0..width {
        if marker == Some(i) {
            style_text(
                out,
                "│",
                Some(elapsed_color),
                Some(surface_color),
                Weight::Normal,
                options.color,
            );
            if config.mono3_marker_style == "replace" {
                continue;
            }
        }

        let mut mask = 0;
        if i < p_fill {
            mask |= 1;
        }
        if i < s_fill {
            mask |= 2;
        }
        if i < t_fill {
            mask |= 4;
        }
        let cell_color = if mask == 0 {
            surface_color
        } else {
            args.mono_color
        };
        style_text(
            out,
            sextant_mask_char(mask),
            Some(cell_color),
            Some(surface_color),
            Weight::Normal,
            options.color,
        );
    }

    if config.mono3_marker_style != "replace" && marker == Some(width) {
        style_text(
            out,
            "│",
            Some(elapsed_color),
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
    let mut marker = ((duration - elapsed) as usize).saturating_mul(width) / duration as usize;
    if marker >= width {
        marker = width - 1;
    }
    Some(marker)
}

fn elapsed_marker_boundary(
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
    let boundary = ((duration - elapsed) as usize).saturating_mul(width) / duration as usize;
    Some(boundary.min(width))
}

fn mono3_marker_boundary(
    config: &RenderConfig,
    args: &MonoArgs<'_>,
    width: usize,
    now_epoch: i64,
) -> Option<usize> {
    match config.mono3_marker_source.as_str() {
        "primary" | "" => row_marker_boundary(
            args.p_used,
            args.p_reset,
            args.p_window,
            width,
            now_epoch,
            config.reset_description_timezone_offset_minutes,
        ),
        "secondary" => row_marker_boundary(
            args.s_used,
            args.s_reset,
            args.s_window,
            width,
            now_epoch,
            config.reset_description_timezone_offset_minutes,
        ),
        "tertiary" => row_marker_boundary(
            args.t_used,
            args.t_reset,
            args.t_window,
            width,
            now_epoch,
            config.reset_description_timezone_offset_minutes,
        ),
        "shared" => shared_window_marker_boundary(args, width, now_epoch, config),
        "none" => None,
        _ => row_marker_boundary(
            args.p_used,
            args.p_reset,
            args.p_window,
            width,
            now_epoch,
            config.reset_description_timezone_offset_minutes,
        ),
    }
}

fn row_marker_boundary(
    used: i32,
    reset: Option<&str>,
    window: Option<i64>,
    width: usize,
    now_epoch: i64,
    reset_description_offset_minutes: Option<i16>,
) -> Option<usize> {
    if used < 0 {
        return None;
    }
    elapsed_marker_boundary(
        reset,
        window,
        width,
        now_epoch,
        reset_description_offset_minutes,
    )
}

fn shared_window_marker_boundary(
    args: &MonoArgs<'_>,
    width: usize,
    now_epoch: i64,
    config: &RenderConfig,
) -> Option<usize> {
    let mut count = 0;
    let mut ref_epoch = None;
    let mut ref_window = None;
    for (used, reset, window) in [
        (args.p_used, args.p_reset, args.p_window),
        (args.s_used, args.s_reset, args.s_window),
        (args.t_used, args.t_reset, args.t_window),
    ] {
        if used < 0 || window.unwrap_or(0) <= 0 {
            continue;
        }
        let epoch = reset.and_then(|reset| {
            reset_epoch(
                reset,
                now_epoch,
                config.reset_description_timezone_offset_minutes,
            )
        });
        if let (Some(epoch), Some(window)) = (epoch, window) {
            match (ref_epoch, ref_window) {
                (None, None) => {
                    ref_epoch = Some(epoch);
                    ref_window = Some(window);
                    count = 1;
                }
                (Some(prev_epoch), Some(prev_window))
                    if prev_epoch == epoch && prev_window == window =>
                {
                    count += 1;
                }
                _ => return None,
            }
        }
    }
    if count >= 2 {
        if let (Some(ref_epoch_val), Some(ref_window_val)) = (ref_epoch, ref_window) {
            // Find which row established the shared window and use its reset string.
            let matched_reset = [
                (args.p_reset, args.p_window, ref_epoch_val, ref_window_val),
                (args.s_reset, args.s_window, ref_epoch_val, ref_window_val),
                (args.t_reset, args.t_window, ref_epoch_val, ref_window_val),
            ]
            .iter()
            .filter_map(|&(reset, opt_window, re, rw)| {
                let epoch = reset.and_then(|r| {
                    reset_epoch(
                        r,
                        now_epoch,
                        config.reset_description_timezone_offset_minutes,
                    )
                });
                if epoch == Some(re) && opt_window == Some(rw) {
                    reset
                } else {
                    None
                }
            })
            .next();
            elapsed_marker_boundary(
                matched_reset,
                Some(ref_window_val),
                width,
                now_epoch,
                config.reset_description_timezone_offset_minutes,
            )
        } else {
            None
        }
    } else {
        None
    }
}

fn mono3_remaining(
    config: &RenderConfig,
    p_remaining: i32,
    s_remaining: i32,
    t_remaining: i32,
    p_used: i32,
    s_used: i32,
    t_used: i32,
) -> i32 {
    match config.mono3_color_mode.as_str() {
        "primary" => p_remaining,
        "lowest" | "" => min_remaining(
            p_remaining,
            s_remaining,
            t_remaining,
            p_used,
            s_used,
            t_used,
        ),
        _ => min_remaining(
            p_remaining,
            s_remaining,
            t_remaining,
            p_used,
            s_used,
            t_used,
        ),
    }
}

fn min_remaining(
    p_remaining: i32,
    s_remaining: i32,
    t_remaining: i32,
    p_used: i32,
    s_used: i32,
    t_used: i32,
) -> i32 {
    let mut lowest = None;
    for (remaining, used) in [
        (p_remaining, p_used),
        (s_remaining, s_used),
        (t_remaining, t_used),
    ] {
        if used >= 0 {
            lowest = Some(lowest.map_or(remaining, |current: i32| current.min(remaining)));
        }
    }
    lowest.unwrap_or(0)
}

fn terminal_mode_for_provider(config: &RenderConfig, provider: &str, has_tertiary: bool) -> String {
    let mode = match config.terminal_bar_mode.as_str() {
        "dual" => "dual",
        "sextant3" => "sextant3",
        "mono3" => "mono3",
        "auto" | "" => {
            if contains(&config.mono3_providers_exclude, provider) {
                "dual"
            } else if contains(&config.mono3_providers, provider) {
                "mono3"
            } else {
                "dual"
            }
        }
        _ => "dual",
    };
    // A provider with no tertiary window has only two pools; the fixed
    // three-lane modes would draw the absent lane as an empty bar. Collapse
    // to the two-lane `dual` layout, matching SketchyBar's `has_t` gate which
    // drops the tertiary row when it carries no usedPercent.
    if !has_tertiary && matches!(mode, "mono3" | "sextant3") {
        "dual".to_string()
    } else {
        mode.to_string()
    }
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
        let epoch = reset_epoch("Resets 11:59 PM", 1_704_067_200, None).expect("reset epoch");
        assert!(epoch > 1_704_067_200);
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
            &RenderConfig::default(),
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
        // Default config renders antigravity as mono3 (width 12):
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
}
