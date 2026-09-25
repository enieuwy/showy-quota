//! SketchyBar frame: the `sketchybar` arguments for one tick.
//!
//! The plugin used to build these arguments in shell, forking a subshell per
//! click script and re-sending every item on every tick. This module builds
//! them from [`SketchybarRows`] and diffs each provider against the frame the
//! plugin sent last, so a tick where nothing changed sends nothing. It also
//! decides whether the items must be declared again. The shell keeps the host
//! work: it declares the items, rasterizes provider icons, and runs
//! `sketchybar`.
//!
//! Wire format (`--emit sketchybar-frame`): one record per line, fields
//! separated by US (`\x1f`), first field a tag.
//!
//! ```text
//! state     US <redeclare reason, or "-"> US <background refresh 0|1>
//! providers US <id> US <id> ...          providers in pill order
//! icon      US <id> US <status> US <png path>   one per icon to rasterize
//! query     US <item> US <item> ...      live item names, for the notch planner
//! set       US <arg> US <arg> ...        the `sketchybar` arguments to send
//! ```

use std::collections::{HashMap, HashSet};

use crate::config::RenderConfig;
use crate::palette::scale_hex;
use crate::sketchybar::{RowLane, SketchybarRow, SketchybarRows, LANE_COUNT};
use crate::sketchybar_ring::{friendly_length, RingIncident, RingUnit, RingWindow};

/// Bump when icon rendering semantics change so stale cached PNGs are replaced.
pub const ICON_CACHE_VERSION: &str = "5";

/// Every item one provider owns, in bracket order. The plugin declares the
/// same list (`PROVIDER_ITEM_ROLES` in `adapters/sketchybar/plugins/showy_quota.sh`).
pub const PROVIDER_ITEM_ROLES: [&str; 11] = [
    "icon",
    "primary",
    "secondary",
    "tertiary",
    "quaternary",
    "secondary_marker",
    "tertiary_marker",
    "quaternary_marker",
    "primary_marker",
    "slot",
    "label",
];

const DEFAULT_CLICK: &str = "open -b com.steipete.codexbar";
const DEFAULT_ICON_FONT: &str = "sketchybar-app-font:Regular:14.0";
const DEFAULT_ICON_SCALE: &str = "0.28";
const ROW_HEIGHT: i64 = 6;
const LANE_ROLES: [&str; LANE_COUNT] = ["primary", "secondary", "tertiary", "quaternary"];
const TRIGGER_ITEM: &str = "showy_quota.trigger";
/// Frame key for the stale/degraded items. Provider ids cannot start with `@`.
const TAIL_KEY: &str = "@tail";
const WIRE_SEP: char = '\u{001f}';

/// SketchyBar-only settings, read from the `SHOWY_QUOTA_*` environment the
/// shell exports. Numbers follow the shell's `showy_quota_uint` rules; colors
/// follow `showy_quota_normalize_hex_or_default`.
/// Strip body. `rows` is today’s pill (the default, unchanged); `ring` is
/// the opt-in ring mode. Anything else falls back to `rows`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    Rows,
    Ring,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSettings {
    pub body: Body,
    pub click: String,
    pub row_radius: i64,
    pub label_width: i64,
    pub slot_width: i64,
    pub icon_width: i64,
    pub icon_padding_left: i64,
    pub icon_font_padding_right: i64,
    pub icon_scale: String,
    pub icon_font_mode: bool,
    pub icon_font: String,
    /// Directory holding rasterized provider icons. Empty disables PNG icons.
    pub icon_dir: String,
    pub notch: bool,
    /// Points kept clear of the notch and the right-hand neighbour.
    pub notch_margin: i64,
    pub track: String,
    pub elapsed: String,
    pub countdown_warn: String,
    pub icon_text: String,
    pub primary_warn: String,
    pub primary_bad: String,
    pub primary_unknown: String,
    pub stale_glyph: String,
    pub degraded_glyph: String,
    pub stale: String,
    pub countdown: String,
    pub time_warn_minutes: i64,
    pub primary_good: String,
    pub good_min_remaining: i64,
    pub warn_min_remaining: i64,
}

impl FrameSettings {
    pub fn from_getter<F>(get: F, config: &RenderConfig) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let uint = |name: &str, fallback: i64| uint_setting(get(name).as_deref(), fallback, 4096);
        let slider_width = uint("SHOWY_QUOTA_PNG_BAR_W", 80);
        let click = get("SHOWY_QUOTA_SKETCHYBAR_CLICK")
            .filter(|click| click_command_is_safe(click))
            .unwrap_or_else(|| DEFAULT_CLICK.into());
        let icon_scale = get("SHOWY_QUOTA_SKETCHYBAR_ICON_SCALE")
            .filter(|scale| decimal(scale))
            .unwrap_or_else(|| DEFAULT_ICON_SCALE.into());
        let icon_font = get("SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT")
            .filter(|font| !font.is_empty() && !font.chars().any(char::is_control))
            .unwrap_or_else(|| DEFAULT_ICON_FONT.into());
        let icon_dir = get("SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE")
            .filter(|dir| !dir.chars().any(char::is_control))
            .unwrap_or_default();
        Self {
            body: match get("SHOWY_QUOTA_SKETCHYBAR_BODY").as_deref() {
                Some("ring") => Body::Ring,
                _ => Body::Rows,
            },
            click,
            row_radius: uint("SHOWY_QUOTA_SKETCHYBAR_ROW_RADIUS", 3),
            label_width: uint("SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH", 32),
            slot_width: uint("SHOWY_QUOTA_SKETCHYBAR_BAR_WIDTH", slider_width + 3),
            icon_width: uint("SHOWY_QUOTA_SKETCHYBAR_ICON_WIDTH", 22),
            icon_padding_left: uint("SHOWY_QUOTA_SKETCHYBAR_ICON_PADDING_LEFT", 5),
            icon_font_padding_right: uint(
                "SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT_PADDING_RIGHT",
                2,
            ),
            icon_scale,
            icon_font_mode: get("SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE").as_deref()
                == Some("font"),
            icon_font,
            icon_dir,
            notch: get("SHOWY_QUOTA_SKETCHYBAR_PLACEMENT").as_deref() == Some("notch"),
            notch_margin: uint("SHOWY_QUOTA_SKETCHYBAR_NOTCH_MARGIN", 4),
            track: hex_or(&config.palette_track, "3a3a4a"),
            elapsed: hex_or(&config.palette_elapsed, "be95ff"),
            countdown_warn: hex_or(&config.palette_countdown_warn, "ee5396"),
            icon_text: hex_or(&config.palette_icon_text, "f2f4f8"),
            primary_warn: hex_or(&config.palette_primary_warn, "f0af00"),
            primary_bad: hex_or(&config.palette_primary_bad, "ee5396"),
            primary_unknown: hex_or(&config.palette_primary_unknown, "6c7086"),
            stale_glyph: config.stale_glyph.clone(),
            degraded_glyph: config.degraded_cli_glyph.clone(),
            stale: hex_or(&config.palette_stale, "6c7086"),
            countdown: hex_or(&config.palette_countdown, "7b8496"),
            time_warn_minutes: config.time_warn_minutes,
            primary_good: hex_or(&config.palette_primary_good, "25be6a"),
            good_min_remaining: i64::from(config.good_min_remaining),
            warn_min_remaining: i64::from(config.warn_min_remaining),
        }
    }

    /// The PNG the plugin rasterizes for this provider and icon status. The
    /// cache key carries the colors the icon is tinted with, so a palette
    /// change renders a new file instead of reusing a stale one.
    pub fn icon_png_path(&self, provider: &str, icon_status: &str) -> Option<String> {
        if self.icon_dir.is_empty() {
            return None;
        }
        let suffix = if icon_status == "error" || self.status_color(icon_status).is_some() {
            format!("-{icon_status}")
        } else {
            String::new()
        };
        Some(format!(
            "{}/icon-v{ICON_CACHE_VERSION}-{provider}-{}-{}-{}-{}{suffix}.png",
            self.icon_dir.trim_end_matches('/'),
            self.icon_text,
            self.primary_unknown,
            self.primary_warn,
            self.primary_bad,
        ))
    }

    fn status_color(&self, status: &str) -> Option<&str> {
        match status {
            "minor" | "maintenance" => Some(&self.primary_warn),
            "major" | "critical" => Some(&self.primary_bad),
            "unknown" => Some(&self.primary_unknown),
            _ => None,
        }
    }
}

/// An icon the plugin must rasterize before the next frame can show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconRequest {
    pub provider: String,
    pub status: String,
    pub path: String,
}

pub struct FrameInputs<'a> {
    pub rows: &'a SketchybarRows,
    pub settings: &'a FrameSettings,
    /// False while the notch plan hides countdown labels.
    pub label_drawing: bool,
    /// Providers the notch plan collapsed into the `+N` item.
    pub hidden: &'a [String],
    /// The last frame file; `None` sends every provider.
    pub previous: Option<&'a str>,
    /// Whether a rasterized icon exists at the path.
    pub icon_ready: &'a dyn Fn(&str) -> bool,
    /// Whether the plugin can rasterize missing icons (ImageMagick present).
    pub icon_maker: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Arguments for the providers (and stale/degraded items) that changed.
    pub args: Vec<String>,
    /// The frame file to store for the next tick.
    pub frame_text: String,
    pub icon_requests: Vec<IconRequest>,
}

pub fn build_frame(inputs: &FrameInputs<'_>) -> Frame {
    let previous = inputs.previous.map(parse_frame_text);
    let mut args = Vec::new();
    let mut frame_text = String::new();
    let mut icon_requests = Vec::new();

    let mut push_unit = |key: &str, unit: Vec<String>| {
        let hash = format!("{:016x}", fnv1a(&unit));
        let unchanged = previous
            .as_ref()
            .is_some_and(|previous| previous.get(key) == Some(&hash));
        if !unchanged {
            args.extend(unit);
        }
        frame_text.push_str(key);
        frame_text.push(WIRE_SEP);
        frame_text.push_str(&hash);
        frame_text.push('\n');
    };

    for row in &inputs.rows.rows {
        let unit = if inputs.hidden.iter().any(|hidden| hidden == &row.provider) {
            vec![
                "--set".into(),
                provider_item_regex(&row.provider),
                "drawing=off".into(),
            ]
        } else {
            let icon = resolve_icon(row, inputs);
            if let IconChoice::Missing(Some(request)) = &icon {
                icon_requests.push(request.clone());
            }
            row_args(row, inputs.settings, inputs.label_drawing, &icon)
        };
        push_unit(&row.provider, unit);
    }
    push_unit(TAIL_KEY, tail_args(inputs.rows, inputs.settings));

    Frame {
        args,
        frame_text,
        icon_requests,
    }
}

enum IconChoice {
    Font(&'static str),
    Image(String),
    /// No drawable icon yet; carries the request when the plugin can make one.
    Missing(Option<IconRequest>),
}

fn resolve_icon(row: &SketchybarRow, inputs: &FrameInputs<'_>) -> IconChoice {
    let settings = inputs.settings;
    if settings.icon_font_mode {
        if let Some(glyph) = crate::providers::font_icon(&row.provider) {
            return IconChoice::Font(glyph);
        }
    }
    let status = icon_status(row);
    let Some(path) = settings.icon_png_path(&row.provider, status) else {
        return IconChoice::Missing(None);
    };
    if (inputs.icon_ready)(&path) {
        return IconChoice::Image(path);
    }
    IconChoice::Missing(inputs.icon_maker.then(|| IconRequest {
        provider: row.provider.clone(),
        status: status.to_owned(),
        path,
    }))
}

/// The PNG cache keys on the indicator. An errored row with no incident still
/// needs the warning tint, and `unknown` renders the fallback glyph untinted,
/// so an error without an indicator gets its own `error` status.
fn icon_status(row: &SketchybarRow) -> &str {
    if row.error && row.status == "none" {
        "error"
    } else {
        &row.status
    }
}

fn row_args(
    row: &SketchybarRow,
    settings: &FrameSettings,
    label_drawing: bool,
    icon: &IconChoice,
) -> Vec<String> {
    let pid = &row.provider;
    let mut args: Vec<String> = Vec::with_capacity(220);
    let mut set = |item: String, props: Vec<String>| {
        args.push("--set".into());
        args.push(item);
        args.extend(props);
    };

    set(
        format!("showy_quota.{pid}.label"),
        vec![
            format!("drawing={}", on_off(label_drawing)),
            format!("label={}", row.label),
            format!("label.color={}", row.label_argb),
            format!("label.width={}", settings.label_width),
            "label.align=left".into(),
            "background.color=0x00000000".into(),
            "background.height=0".into(),
        ],
    );

    // An errored row with no incident indicator has no status tint, but it is
    // still an error: the icon must read as a warning, not as healthy.
    let icon_click = click_script_for_status(settings, &row.status, &row.status_url);
    let icon_color = if row.error {
        Some(settings.countdown_warn.as_str())
    } else {
        settings.status_color(&row.status)
    }
    .unwrap_or(settings.icon_text.as_str());
    let icon_item = format!("showy_quota.{pid}.icon");
    match icon {
        IconChoice::Font(glyph) => set(
            icon_item,
            vec![
                "drawing=on".into(),
                "icon.drawing=on".into(),
                format!("icon={glyph}"),
                format!("icon.font={}", settings.icon_font),
                format!("icon.color=0xff{icon_color}"),
                "icon.align=center".into(),
                format!("icon.width={}", settings.icon_width),
                "icon.padding_left=0".into(),
                "icon.padding_right=0".into(),
                "label.drawing=off".into(),
                "background.image.drawing=off".into(),
                "background.color=0x00000000".into(),
                "background.height=0".into(),
                format!("padding_left={}", settings.icon_padding_left),
                "padding_right=0".into(),
                format!(
                    "width={}",
                    settings.icon_width + settings.icon_font_padding_right
                ),
                format!("click_script={icon_click}"),
            ],
        ),
        IconChoice::Image(path) => set(
            icon_item,
            vec![
                "drawing=on".into(),
                "icon.drawing=off".into(),
                "label.drawing=off".into(),
                format!("background.image={path}"),
                "background.image.drawing=on".into(),
                format!("background.image.scale={}", settings.icon_scale),
                "background.color=0x00000000".into(),
                "background.height=0".into(),
                format!("padding_left={}", settings.icon_padding_left),
                "padding_right=0".into(),
                format!("width={}", settings.icon_width),
                format!("click_script={icon_click}"),
            ],
        ),
        // The icon item is hidden, but its click target still covers the row
        // slot: keep the status-page action so an errored provider without a
        // drawable icon still reaches its status page.
        IconChoice::Missing(_) => set(
            icon_item,
            vec!["drawing=off".into(), format!("click_script={icon_click}")],
        ),
    }

    // An errored provider has no lane at all. Drawing its primary lane would
    // show a full bad-severity bar at zero remaining — "quota exhausted",
    // which is precisely what the data does not say.
    let has = |lane: &RowLane| !row.error && lane.present;
    let y = lane_offsets(has(&row.lanes[1]), has(&row.lanes[2]), has(&row.lanes[3]));

    for (index, role) in LANE_ROLES.iter().enumerate() {
        let lane = &row.lanes[index];
        let item = format!("showy_quota.{pid}.{role}");
        let click = format!(
            "click_script={}",
            slider_click_script(settings, &item, lane.remaining)
        );
        // The primary lane always draws unless the provider errored.
        let drawn = if index == 0 { !row.error } else { has(lane) };
        let props = if drawn {
            vec![
                "drawing=on".into(),
                format!("slider.percentage={}", lane.remaining),
                format!("slider.highlight_color={}", lane.argb),
                format!("slider.background.color=0xff{}", settings.track),
                format!("slider.background.height={ROW_HEIGHT}"),
                format!("slider.background.corner_radius={}", settings.row_radius),
                "slider.knob.drawing=off".into(),
                "background.color=0x00000000".into(),
                "background.height=0".into(),
                "padding_left=0".into(),
                "padding_right=0".into(),
                "width=0".into(),
                format!("y_offset={}", y[index]),
                click,
            ]
        } else {
            vec![
                "drawing=off".into(),
                "slider.percentage=0".into(),
                "background.color=0x00000000".into(),
                "background.height=0".into(),
                "padding_left=0".into(),
                "padding_right=0".into(),
                "width=0".into(),
                format!("y_offset={}", y[index]),
                click,
            ]
        };
        set(item, props);
    }

    for (index, role) in LANE_ROLES.iter().enumerate() {
        let lane = &row.lanes[index];
        let item = format!("showy_quota.{pid}.{role}_marker");
        let click = format!(
            "click_script={}",
            slider_click_script(settings, &item, lane.marker.unwrap_or(0))
        );
        let marker = lane.marker.filter(|_| index == 0 || has(lane));
        let props = match marker {
            Some(percent) => vec![
                "drawing=on".into(),
                format!("slider.percentage={percent}"),
                "slider.highlight_color=0x00000000".into(),
                "slider.background.color=0x00000000".into(),
                format!("slider.background.height={ROW_HEIGHT}"),
                "slider.background.corner_radius=0".into(),
                "slider.knob.drawing=on".into(),
                "slider.knob.color=0x00000000".into(),
                "slider.knob.width=1".into(),
                "slider.knob.padding_left=0".into(),
                "slider.knob.padding_right=0".into(),
                "slider.knob.background.drawing=on".into(),
                format!("slider.knob.background.color=0xff{}", settings.elapsed),
                format!("slider.knob.background.height={ROW_HEIGHT}"),
                "slider.knob.background.corner_radius=0".into(),
                "background.color=0x00000000".into(),
                "background.height=0".into(),
                "padding_left=0".into(),
                "padding_right=0".into(),
                "width=0".into(),
                format!("y_offset={}", y[index]),
                click,
            ],
            None => vec![
                "drawing=off".into(),
                "slider.percentage=0".into(),
                format!("y_offset={}", y[index]),
                click,
            ],
        };
        set(item, props);
    }

    set(
        format!("showy_quota.{pid}.slot"),
        vec![
            "drawing=on".into(),
            "icon.drawing=off".into(),
            "label.drawing=off".into(),
            "background.color=0x00000000".into(),
            "background.height=0".into(),
            "padding_left=0".into(),
            "padding_right=0".into(),
            format!("width={}", settings.slot_width),
            format!("click_script={}", settings.click),
        ],
    );
    args
}

/// Vertical slot of each lane in a 2–4 row stack. A single live window draws
/// one centered bar with no empty second row.
fn lane_offsets(has_s: bool, has_t: bool, has_q: bool) -> [i64; LANE_COUNT] {
    if has_q {
        [9, 3, -3, -9]
    } else if has_t {
        [7, 0, -7, -7]
    } else if has_s {
        [4, -4, -4, -4]
    } else {
        [0, 0, 0, 0]
    }
}

fn tail_args(rows: &SketchybarRows, settings: &FrameSettings) -> Vec<String> {
    let warn = format!("label.color=0xff{}", settings.countdown_warn);
    let click = format!("click_script={}", settings.click);
    let mut args: Vec<String> = vec!["--set".into(), "showy_quota.stale".into()];
    if rows.stale {
        args.extend([
            "drawing=on".into(),
            format!("label={}", settings.stale_glyph),
            warn.clone(),
            "icon.drawing=off".into(),
            "background.color=0x00000000".into(),
            "background.height=0".into(),
            "padding_left=4".into(),
            "padding_right=2".into(),
            click.clone(),
        ]);
    } else {
        args.push("drawing=off".into());
    }
    args.extend(["--set".into(), "showy_quota.degraded".into()]);
    if rows.degraded_cli {
        args.extend([
            "drawing=on".into(),
            format!("label={}", settings.degraded_glyph),
            warn,
            "icon.drawing=off".into(),
            "background.color=0x00000000".into(),
            "background.height=0".into(),
            "padding_left=2".into(),
            "padding_right=4".into(),
            click,
        ]);
    } else {
        args.push("drawing=off".into());
    }
    args
}

// ── ring body ──────────────────────────────────────────────────────────

/// Every item one ring unit owns, in bracket order. The plugin declares the
/// same list (`RING_UNIT_ITEM_ROLES` in
/// `adapters/sketchybar/plugins/showy_quota.sh`).
pub const RING_UNIT_ROLES: [&str; 15] = [
    "ring",
    "ring_pace",
    "bar0",
    "bar0_pace",
    "bar1",
    "bar1_pace",
    "label",
    "pop_title",
    "pop_header",
    "pop_row0",
    "pop_row1",
    "pop_row2",
    "pop_note",
    "pop_alert0",
    "pop_alert1",
];

/// Pill edge spacers; the bracket spans item rects without padding, so edges
/// and gaps are real spacer items declared by the plugin.
pub const RING_EDGE_A: &str = "showy_quota.edge.a";
pub const RING_EDGE_Z: &str = "showy_quota.edge.z";

/// Spacer before every unit but the first (`10` pt between Antigravity’s
/// pools, `22` pt between providers; widths are set at declaration).
pub fn ring_gap_item(unit: &str) -> String {
    format!("showy_quota.gap.{unit}")
}

/// Presence-checked items beyond the unit roles: both edges and every gap.
pub fn ring_extra_items(units: &[RingUnit]) -> Vec<String> {
    let mut extra = vec![RING_EDGE_A.to_owned()];
    for unit in units.iter().skip(1) {
        extra.push(ring_gap_item(&unit.unit));
    }
    extra.push(RING_EDGE_Z.to_owned());
    extra
}

const RING_DIAMETER: i64 = 26;
/// Extra ring-item width on the ring's left for the banked-reset badge. The
/// plugin shrinks the spacer before each unit by the same amount.
const RING_BADGE_ROOM: i64 = 7;
const RING_STROKE: &str = "3";
const RING_PACE_LEN: f64 = 3.5;
const RING_BAR_W: i64 = 28;
const RING_BAR_H: i64 = 4;
const RING_GAP: i64 = 5;
const RING_LABEL_FONT: &str = "SF Pro:Semibold:10.0";
const RING_POP_FONT: &str = "Hack Nerd Font:Regular:11.0";
const RING_POP_TITLE_FONT: &str = "SF Pro:Bold:12.0";
const RING_POP_NOTE_FONT: &str = "SF Pro:Regular:10.0";
const RING_POP_ALERT_FONT: &str = "SF Pro:Regular:11.0";
const RING_POP_ALERT_ICON_FONT: &str = "SF Pro:Bold:12.0";
const RING_POOL_FONT: &str = "SF Pro:Heavy:8.0";
/// Banked-reset disc: dark text on blue, as chosen in the demo.
const RING_BLUE: &str = "0xff78a9ff";
const RING_DARK: &str = "0xff161616";

impl FrameSettings {
    /// Status colour of a window at full brightness. The rows dim
    /// long-horizon windows; rings never do.
    pub(crate) fn ring_window_hex(&self, remaining: i64) -> String {
        if remaining >= self.good_min_remaining {
            self.primary_good.clone()
        } else if remaining >= self.warn_min_remaining {
            self.primary_warn.clone()
        } else {
            self.primary_bad.clone()
        }
    }

    pub(crate) fn ring_window_argb(&self, remaining: i64) -> String {
        format!("0xff{}", self.ring_window_hex(remaining))
    }

    /// Empty pool (0 % left): red-tinted track derived from the bad colour.
    pub(crate) fn empty_track_argb(&self) -> String {
        format!("0x66{}", self.primary_bad)
    }

    fn ring_stale_argb(&self) -> String {
        format!("0xff{}", self.stale)
    }

    fn ring_unknown_argb(&self) -> String {
        format!("0xff{}", self.primary_unknown)
    }

    fn ring_elapsed_argb(&self) -> String {
        format!("0xff{}", self.elapsed)
    }

    fn ring_track_argb(&self) -> String {
        format!("0xff{}", self.track)
    }

    fn ring_countdown_argb(&self) -> String {
        format!("0xff{}", self.countdown)
    }

    fn ring_text_argb(&self) -> String {
        format!("0xff{}", self.icon_text)
    }

    fn ring_warn_argb(&self) -> String {
        format!("0xff{}", self.countdown_warn)
    }
}

pub struct RingFrameInputs<'a> {
    pub units: &'a [RingUnit],
    pub settings: &'a FrameSettings,
    pub stale: bool,
    pub degraded_cli: bool,
    /// The last frame file; `None` sends every unit.
    pub previous: Option<&'a str>,
}

pub fn build_ring_frame(inputs: &RingFrameInputs<'_>) -> Frame {
    let previous = inputs.previous.map(parse_frame_text);
    let mut args = Vec::new();
    let mut frame_text = String::new();
    let icon_requests = Vec::new();

    let mut push_unit = |key: &str, unit: Vec<String>| {
        let hash = format!("{:016x}", fnv1a(&unit));
        let unchanged = previous
            .as_ref()
            .is_some_and(|previous| previous.get(key) == Some(&hash));
        if !unchanged {
            args.extend(unit);
        }
        frame_text.push_str(key);
        frame_text.push(WIRE_SEP);
        frame_text.push_str(&hash);
        frame_text.push('\n');
    };

    for unit in inputs.units {
        push_unit(&unit.unit, ring_unit_args(unit, inputs.settings));
    }
    push_unit(
        TAIL_KEY,
        ring_tail_args(inputs.stale, inputs.degraded_cli, inputs.settings),
    );

    Frame {
        args,
        frame_text,
        icon_requests,
    }
}

fn ring_tail_args(stale: bool, degraded_cli: bool, settings: &FrameSettings) -> Vec<String> {
    let warn = format!("label.color=0xff{}", settings.countdown_warn);
    let click = format!("click_script={}", settings.click);
    let mut args: Vec<String> = vec!["--set".into(), "showy_quota.stale".into()];
    if stale {
        args.extend([
            "drawing=on".into(),
            format!("label={}", settings.stale_glyph),
            warn.clone(),
            "icon.drawing=off".into(),
            "background.color=0x00000000".into(),
            "background.height=0".into(),
            "padding_left=4".into(),
            "padding_right=2".into(),
            click.clone(),
        ]);
    } else {
        args.push("drawing=off".into());
    }
    args.extend(["--set".into(), "showy_quota.degraded".into()]);
    if degraded_cli {
        args.extend([
            "drawing=on".into(),
            format!("label={}", settings.degraded_glyph),
            warn,
            "icon.drawing=off".into(),
            "background.color=0x00000000".into(),
            "background.height=0".into(),
            "padding_left=2".into(),
            "padding_right=4".into(),
            click,
        ]);
    } else {
        args.push("drawing=off".into());
    }
    args
}

/// Logo colour: red on error, the incident tint on outage, otherwise text.
/// Stale units keep their tint, as the rows’ icons do.
fn ring_logo_argb(unit: &RingUnit, settings: &FrameSettings) -> String {
    if unit.error.is_some() {
        return settings.ring_warn_argb();
    }
    match unit
        .incident
        .as_ref()
        .map(|incident| incident.indicator.as_str())
    {
        Some("minor") | Some("maintenance") => format!("0xff{}", settings.primary_warn),
        Some("major") | Some("critical") => format!("0xff{}", settings.primary_bad),
        _ => settings.ring_text_argb(),
    }
}

fn ring_click(unit: &RingUnit, settings: &FrameSettings) -> String {
    let (status, url) = match unit.incident.as_ref() {
        Some(incident) => (incident.indicator.as_str(), incident.url.as_str()),
        None => ("none", ""),
    };
    click_script_for_status(settings, status, url)
}

fn ring_unit_args(unit: &RingUnit, settings: &FrameSettings) -> Vec<String> {
    let prefix = format!("showy_quota.{}", unit.unit);
    let mut args: Vec<String> = Vec::with_capacity(320);
    let mut set = |item: String, props: Vec<String>| {
        args.push("--set".into());
        args.push(item);
        args.extend(props);
    };
    let error = unit.error.is_some();
    let stale = unit.stale;
    let logo = ring_logo_argb(unit, settings);
    let click = ring_click(unit, settings);

    let ring_color = if error {
        settings.ring_unknown_argb()
    } else if stale {
        settings.ring_stale_argb()
    } else {
        settings.ring_window_argb(unit.ring.remaining)
    };
    // An empty ring keeps the plain grey track: no arc is the whole signal.
    let ring_track = settings.ring_track_argb();
    let mut ring_props = vec![
        "drawing=on".into(),
        format!("ring.value={:.2}", unit.ring.remaining as f64 / 100.0),
        format!("ring.color={ring_color}"),
        format!("ring.track_color={ring_track}"),
        format!("ring.line_width={RING_STROKE}"),
        "ring.cap=round".into(),
        format!("ring.marker={}", unit.logo_glyph),
        "ring.marker.drawing=on".into(),
        "ring.marker.position=center".into(),
        format!("ring.marker.font={}", unit.logo_font),
        format!("ring.marker.color={logo}"),
        format!("ring.marker.padding_left={}", unit.logo_pad),
        "ring.marker.padding_right=0".into(),
        format!("ring.marker.y_offset={}", unit.logo_y),
        "icon.drawing=off".into(),
        "label.drawing=off".into(),
        "background.drawing=off".into(),
        "padding_left=0".into(),
        "padding_right=0".into(),
        format!("width={}", RING_DIAMETER + RING_BADGE_ROOM),
        "align=right".into(),
        "y_offset=0".into(),
        format!("click_script={click}"),
    ];
    match unit.pool {
        Some(pool) => ring_props.extend([
            format!("ring.marker.badge={pool}"),
            "ring.marker.badge.drawing=on".into(),
            format!("ring.marker.badge.font={RING_POOL_FONT}"),
            format!("ring.marker.badge.color={}", settings.ring_text_argb()),
            "ring.marker.badge.anchor=top_right".into(),
            "ring.marker.badge.align=center".into(),
            "ring.marker.badge.x_offset=0".into(),
            "ring.marker.badge.y_offset=0".into(),
        ]),
        None => ring_props.push("ring.marker.badge.drawing=off".into()),
    }
    match unit.banked.as_ref() {
        Some(banked) => ring_props.extend([
            format!("ring.badge={}", banked.count),
            "ring.badge.drawing=on".into(),
            format!("ring.badge.font={RING_POOL_FONT}"),
            format!("ring.badge.color={RING_DARK}"),
            // Top-left, pushed off the ring into the room the item keeps on
            // its left: on the right it would meet the label.
            "ring.badge.anchor=top_left".into(),
            "ring.badge.x_offset=-5".into(),
            "ring.badge.y_offset=2".into(),
            "ring.badge.align=center".into(),
            "ring.badge.width=11".into(),
            "ring.badge.background.drawing=on".into(),
            format!("ring.badge.background.color={RING_BLUE}"),
            "ring.badge.background.height=11".into(),
            "ring.badge.background.corner_radius=5".into(),
        ]),
        None => ring_props.push("ring.badge.drawing=off".into()),
    }
    set(format!("{prefix}.ring"), ring_props);

    // Pace tick: a butt-capped arc 3.5 pt long at the time left, stroked two
    // wider than the ring.
    match unit.ring.expected {
        Some(expected) if !error => {
            let pace = expected.clamp(0, 100) as f64;
            let span = RING_PACE_LEN / (std::f64::consts::PI * (RING_DIAMETER as f64 - 3.0));
            let start = 270.0 + 360.0 * pace / 100.0 - 180.0 * span;
            set(
                format!("{prefix}.ring_pace"),
                vec![
                    "drawing=on".into(),
                    format!("ring.value={span:.4}"),
                    format!("ring.start_angle={start:.2}"),
                    format!("ring.color={}", settings.ring_elapsed_argb()),
                    "ring.track_color=0x00000000".into(),
                    "ring.cap=butt".into(),
                    "ring.line_width=5".into(),
                    "ring.marker.drawing=off".into(),
                    "icon.drawing=off".into(),
                    "label.drawing=off".into(),
                    "background.drawing=off".into(),
                    format!("padding_left={}", -RING_DIAMETER),
                    "padding_right=0".into(),
                    "width=0".into(),
                    "y_offset=0".into(),
                    format!("click_script={}", settings.click),
                ],
            );
        }
        _ => set(
            format!("{prefix}.ring_pace"),
            vec!["drawing=off".into(), "ring.value=0".into()],
        ),
    }

    // Bars: one y_offset for a lone bar, a double stack for two.
    let double = unit.bars.len() == 2;
    let bar_y = |index: usize| {
        if double {
            if index == 0 {
                -1
            } else {
                -7
            }
        } else {
            -6
        }
    };
    let knob_h = if double { 6 } else { 8 };
    for index in 0..2 {
        let bar_item = format!("{prefix}.bar{index}");
        let knob_item = format!("{prefix}.bar{index}_pace");
        let bar_click = slider_click_script(
            settings,
            &bar_item,
            unit.bars.get(index).map(|bar| bar.remaining).unwrap_or(0),
        );
        let knob_click = slider_click_script(
            settings,
            &knob_item,
            unit.bars
                .get(index)
                .and_then(|bar| bar.expected)
                .unwrap_or(0),
        );
        let y = bar_y(index);
        match unit.bars.get(index) {
            Some(bar) => {
                let color = if stale {
                    settings.ring_stale_argb()
                } else {
                    settings.ring_window_argb(bar.remaining)
                };
                let track = if stale || bar.unknown || bar.remaining > 0 {
                    settings.ring_track_argb()
                } else {
                    settings.empty_track_argb()
                };
                set(
                    bar_item,
                    vec![
                        "drawing=on".into(),
                        format!("slider.percentage={}", bar.remaining),
                        format!("slider.highlight_color={color}"),
                        format!("slider.background.color={track}"),
                        format!("slider.background.height={RING_BAR_H}"),
                        format!("slider.background.corner_radius={RING_BAR_H}"),
                        "slider.knob.drawing=off".into(),
                        "icon.drawing=off".into(),
                        "label.drawing=off".into(),
                        "background.color=0x00000000".into(),
                        "background.height=0".into(),
                        format!("padding_left={RING_GAP}"),
                        "padding_right=0".into(),
                        "width=0".into(),
                        format!("y_offset={y}"),
                        format!("click_script={bar_click}"),
                    ],
                );
                match bar.expected {
                    Some(expected) if !bar.breakdown => set(
                        knob_item,
                        vec![
                            "drawing=on".into(),
                            format!("slider.percentage={}", expected.clamp(0, 100)),
                            "slider.highlight_color=0x00000000".into(),
                            "slider.background.color=0x00000000".into(),
                            format!("slider.background.height={knob_h}"),
                            "slider.knob.drawing=on".into(),
                            "slider.knob.color=0x00000000".into(),
                            "slider.knob.width=2".into(),
                            "slider.knob.padding_left=0".into(),
                            "slider.knob.padding_right=0".into(),
                            "slider.knob.background.drawing=on".into(),
                            format!(
                                "slider.knob.background.color={}",
                                settings.ring_elapsed_argb()
                            ),
                            format!("slider.knob.background.height={knob_h}"),
                            "slider.knob.background.corner_radius=0".into(),
                            "icon.drawing=off".into(),
                            "label.drawing=off".into(),
                            "background.color=0x00000000".into(),
                            "background.height=0".into(),
                            format!("padding_left={RING_GAP}"),
                            "padding_right=0".into(),
                            "width=0".into(),
                            format!("y_offset={y}"),
                            format!("click_script={knob_click}"),
                        ],
                    ),
                    _ => set(
                        knob_item,
                        vec![
                            "drawing=off".into(),
                            "slider.percentage=0".into(),
                            format!("y_offset={y}"),
                        ],
                    ),
                }
            }
            None => {
                set(
                    bar_item,
                    vec![
                        "drawing=off".into(),
                        "slider.percentage=0".into(),
                        format!("y_offset={y}"),
                    ],
                );
                set(
                    knob_item,
                    vec![
                        "drawing=off".into(),
                        "slider.percentage=0".into(),
                        format!("y_offset={y}"),
                    ],
                );
            }
        }
    }

    let label_y = if double {
        8
    } else if unit.bars.len() == 1 {
        6
    } else {
        0
    };
    let label_color = if error {
        settings.ring_warn_argb()
    } else if stale {
        settings.ring_stale_argb()
    } else if unit
        .label_minutes
        .is_some_and(|minutes| minutes < settings.time_warn_minutes)
    {
        settings.ring_warn_argb()
    } else {
        settings.ring_countdown_argb()
    };
    set(
        format!("{prefix}.label"),
        vec![
            "drawing=on".into(),
            format!("label={}", unit.label),
            format!("label.font={RING_LABEL_FONT}"),
            format!("label.color={label_color}"),
            format!("label.y_offset={label_y}"),
            "label.padding_left=0".into(),
            "label.padding_right=0".into(),
            format!("width={RING_BAR_W}"),
            "icon.drawing=off".into(),
            "background.drawing=off".into(),
            format!("padding_left={RING_GAP}"),
            "padding_right=0".into(),
            format!("click_script={}", settings.click),
        ],
    );

    ring_popup_args(unit, settings, &prefix, &mut set);
    args
}

/// Style-5 hover popup: title, dim header, one mini gauge per window with the
/// % left as a `label.badge`, a grey note, and alert rows.
fn ring_popup_args(
    unit: &RingUnit,
    settings: &FrameSettings,
    prefix: &str,
    set: &mut impl FnMut(String, Vec<String>),
) {
    let text = settings.ring_text_argb();
    let countdown = settings.ring_countdown_argb();
    let logo = ring_logo_argb(unit, settings);
    set(
        format!("{prefix}.pop_title"),
        vec![
            "drawing=on".into(),
            format!("icon={}", unit.logo_glyph),
            format!("icon.font={}", unit.logo_font),
            format!("icon.color={logo}"),
            "icon.padding_left=0".into(),
            "icon.padding_right=8".into(),
            format!("label={}", unit.title),
            format!("label.font={RING_POP_TITLE_FONT}"),
            format!("label.color={text}"),
            "padding_left=10".into(),
            "padding_right=10".into(),
        ],
    );

    let error = unit.error.is_some();
    if error {
        set(format!("{prefix}.pop_header"), vec!["drawing=off".into()]);
    } else {
        set(
            format!("{prefix}.pop_header"),
            vec![
                "drawing=on".into(),
                "icon.drawing=off".into(),
                format!(
                    "label={}",
                    popup_row_text(unit.name_width, "window", "len", Some("pace"), "resets in")
                ),
                format!("label.font={RING_POP_FONT}"),
                format!("label.color={countdown}"),
                "label.padding_left=58".into(),
                "label.badge.y_offset=1".into(),
                "label.badge.drawing=on".into(),
                "label.badge=left".into(),
                format!("label.badge.font={RING_POP_FONT}"),
                format!("label.badge.color={countdown}"),
                "label.badge.anchor=center_left".into(),
                "label.badge.width=30".into(),
                "label.badge.align=right".into(),
                "label.badge.x_offset=-40".into(),
                "padding_left=10".into(),
                "padding_right=10".into(),
            ],
        );
    }

    // Row 0 is always the ring window; rows 1–2 are bars (breakdown parts
    // draw thin and dim).
    let mut gauges: Vec<Option<(&RingWindow, bool)>> = vec![None, None, None];
    if !error {
        gauges[0] = Some((&unit.ring, true));
        for (index, bar) in unit.bars.iter().enumerate().take(2) {
            gauges[index + 1] = Some((bar, false));
        }
    }
    for (index, gauge) in gauges.into_iter().enumerate() {
        let item = format!("{prefix}.pop_row{index}");
        let Some((window, is_ring)) = gauge else {
            set(item, vec!["drawing=off".into()]);
            continue;
        };
        let color = if unit.stale {
            settings.ring_stale_argb()
        } else {
            settings.ring_window_argb(window.remaining)
        };
        let track = if is_ring || unit.stale || window.unknown || window.remaining > 0 {
            settings.ring_track_argb()
        } else {
            settings.empty_track_argb()
        };
        let pace = match window.expected {
            Some(expected) if !window.breakdown => {
                Some(format!("{:+}", window.remaining - expected))
            }
            _ => None,
        };
        let text = popup_row_text(
            unit.name_width,
            &window.title,
            &friendly_length(window.minutes),
            pace.as_deref(),
            &window.reset_text,
        );
        let mut props = vec![
            "drawing=on".into(),
            "icon.drawing=off".into(),
            format!("label={text}"),
            format!("label.font={RING_POP_FONT}"),
            format!("label.color={}", settings.ring_text_argb()),
            "label.padding_left=44".into(),
            "label.badge.y_offset=1".into(),
            "label.badge.drawing=on".into(),
            if window.unknown {
                "label.badge=?".into()
            } else {
                format!("label.badge={}%", window.remaining)
            },
            format!("label.badge.font={RING_POP_FONT}"),
            format!("label.badge.color={color}"),
            "label.badge.anchor=center_left".into(),
            "label.badge.width=30".into(),
            "label.badge.align=right".into(),
            "label.badge.x_offset=-40".into(),
            "padding_left=10".into(),
            "padding_right=10".into(),
        ];
        if is_ring {
            props.extend([
                format!("ring.value={:.2}", window.remaining as f64 / 100.0),
                format!("ring.color={color}"),
                format!("ring.track_color={track}"),
                "ring.line_width=2.5".into(),
                "ring.cap=round".into(),
                "ring.marker.drawing=off".into(),
            ]);
        } else {
            let height = if window.breakdown { 2 } else { 4 };
            let gauge_color = if window.breakdown {
                format!(
                    "0xff{}",
                    scale_hex(&settings.ring_window_hex(window.remaining), "0.55")
                )
            } else {
                color.clone()
            };
            props.extend([
                format!("slider.percentage={}", window.remaining),
                format!("slider.highlight_color={gauge_color}"),
                format!("slider.background.color={track}"),
                format!("slider.background.height={height}"),
                "slider.background.corner_radius=2".into(),
                "slider.knob.drawing=off".into(),
            ]);
        }
        set(item, props);
    }

    let note = match unit.incident.as_ref() {
        Some(incident) => incident_note(incident),
        None => unit.note.clone(),
    };
    if note.is_empty() {
        set(format!("{prefix}.pop_note"), vec!["drawing=off".into()]);
    } else {
        set(
            format!("{prefix}.pop_note"),
            vec![
                "drawing=on".into(),
                "icon.drawing=off".into(),
                format!("label={note}"),
                format!("label.font={RING_POP_NOTE_FONT}"),
                format!("label.color={countdown}"),
                "padding_left=68".into(),
                "padding_right=10".into(),
            ],
        );
    }

    let warn = settings.ring_warn_argb();
    let mut alerts: Vec<(String, String, String)> = Vec::new();
    if let Some(error) = unit.error.as_ref() {
        if !error.message.is_empty() {
            alerts.push(("⚠".into(), warn.clone(), error.message.clone()));
        }
    }
    if let Some(incident) = unit.incident.as_ref() {
        let color = match incident.indicator.as_str() {
            "minor" | "maintenance" => format!("0xff{}", settings.primary_warn),
            _ => format!("0xff{}", settings.primary_bad),
        };
        alerts.push(("●".into(), color, incident.description.clone()));
    }
    if let Some(banked) = unit.banked.as_ref() {
        alerts.push((
            banked.count.to_string(),
            RING_BLUE.into(),
            banked.note.clone(),
        ));
    }
    for (index, slot) in ["pop_alert0", "pop_alert1"].iter().enumerate() {
        match alerts.get(index) {
            Some((glyph, color, text)) => set(
                format!("{prefix}.{slot}"),
                vec![
                    "drawing=on".into(),
                    format!("icon={glyph}"),
                    format!("icon.font={RING_POP_ALERT_ICON_FONT}"),
                    format!("icon.color={color}"),
                    "icon.width=14".into(),
                    "icon.align=center".into(),
                    "icon.padding_left=0".into(),
                    "icon.padding_right=0".into(),
                    format!("label={text}"),
                    format!("label.font={RING_POP_ALERT_FONT}"),
                    format!("label.color={}", settings.ring_text_argb()),
                    "label.padding_left=8".into(),
                    "padding_left=10".into(),
                    "padding_right=10".into(),
                ],
            ),
            None => set(format!("{prefix}.{slot}"), vec!["drawing=off".into()]),
        }
    }
}

/// One style-5 popup line: `window len pace reset`, with the % left carried
/// by the `label.badge`.
fn popup_row_text(
    name_width: usize,
    name: &str,
    len: &str,
    pace: Option<&str>,
    reset: &str,
) -> String {
    format!(
        "{:<nw$}  {:>3}  {:>4}  {reset}",
        name,
        len,
        pace.unwrap_or_default(),
        nw = name_width,
        reset = reset
    )
}

fn incident_note(incident: &RingIncident) -> String {
    let (logo, kind) = match incident.indicator.as_str() {
        "minor" => ("yellow", "minor incident"),
        "maintenance" => ("yellow", "maintenance"),
        "major" => ("red", "major outage"),
        "critical" => ("red", "critical outage"),
        indicator => ("tinted", indicator),
    };
    let mut note = format!("{logo} logo = {kind}");
    if status_url_is_openable(&incident.url) {
        if let Some(host) = url_host(&incident.url) {
            note.push_str(&format!(" · click opens {host}"));
        }
    }
    note
}

fn url_host(url: &str) -> Option<String> {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .filter(|host| !host.is_empty())
        .map(str::to_owned)
}

/// Clicking a slider re-sets its own percentage (SketchyBar moves a slider on
/// click) and then runs the configured click action.
fn slider_click_script(settings: &FrameSettings, item: &str, percent: i64) -> String {
    format!(
        "command -v sketchybar >/dev/null 2>&1 && sketchybar --set {} slider.percentage={} >/dev/null 2>&1; {}",
        shell_quote(item),
        percent.clamp(0, 100),
        settings.click
    )
}

fn click_script_for_status(settings: &FrameSettings, status: &str, url: &str) -> String {
    if matches!(status, "minor" | "maintenance" | "major" | "critical")
        && status_url_is_openable(url)
    {
        return format!("open {}", shell_quote(url));
    }
    settings.click.clone()
}

/// Anchored regex for every item of one provider: the id, then exactly one
/// role (roles carry no dot), so provider `a` never matches `a.b`'s items.
/// Provider ids are validated to `[A-Za-z0-9_.-]`; only the dot needs escaping.
pub fn provider_item_regex(provider: &str) -> String {
    format!("/^showy_quota\\.{}\\.[^.]*$/", provider.replace('.', "\\."))
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\\''"))
}

/// The click action runs through a shell, so refuse metacharacters that would
/// chain or redirect commands.
pub fn click_command_is_safe(click: &str) -> bool {
    !click.is_empty()
        && !click.chars().any(|ch| {
            matches!(ch, ';' | '|' | '&' | '`' | '$' | '(' | ')' | '<' | '>') || ch.is_control()
        })
}

/// A status URL is opened only when it is plain http(s) to a named public
/// host: no credentials, no IP literal, no localhost, a valid port.
pub fn status_url_is_openable(url: &str) -> bool {
    if url.len() > 2048
        || url
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\\')
    {
        return false;
    }
    let Some(rest) = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port_ok = (1..=5).contains(&port.len())
                && port.bytes().all(|b| b.is_ascii_digit())
                && port
                    .parse::<u32>()
                    .is_ok_and(|port| (1..=65_535).contains(&port));
            if host.contains(':') || !port_ok {
                return false;
            }
            host
        }
        None => authority,
    };
    let label_ok = |label: &str| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            && bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
    };
    if !host.split('.').all(label_ok) || host_is_ipv4(host) {
        return false;
    }
    let lower = host.to_ascii_lowercase();
    !(lower == "localhost" || lower.ends_with(".localhost"))
}

/// The WHATWG URL parser (which browsers follow) reads a host as IPv4 when
/// its last label is a number: decimal, or `0x` hex. `0x7f000001` is
/// 127.0.0.1.
fn host_is_ipv4(host: &str) -> bool {
    let last = host.rsplit('.').next().unwrap_or(host);
    let hex = last.strip_prefix("0x").or_else(|| last.strip_prefix("0X"));
    match hex {
        Some(digits) => digits.bytes().all(|b| b.is_ascii_hexdigit()),
        None => !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()),
    }
}

// ── items and redeclare ────────────────────────────────────────────────

/// Item names from a `sketchybar --query bar` reply, in SketchyBar's order.
/// `None` when the reply is empty or unreadable: SketchyBar drops a reply
/// that takes over 100 ms, and a missing reply is not evidence of anything.
pub fn parse_bar_items(raw: &[u8]) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_slice(raw).ok()?;
    let object = value.as_object()?;
    Some(
        object
            .get("items")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// Why the items must be declared again, or `None` when the live items can
/// be reused. `items` is the live item list (`None` when unreadable);
/// `declared` is the provider list the last redeclare stored.
pub fn redeclare_reason(
    force: bool,
    items: Option<&[String]>,
    declared: &[String],
    desired: &[String],
    notch: bool,
) -> Option<&'static str> {
    if force {
        return Some("forced");
    }
    if let Some(items) = items {
        let live: HashSet<&str> = items.iter().map(String::as_str).collect();
        // A crash between the body stamp and its cleanup can leave ring
        // items behind; they never belong to the rows body.
        if live.iter().any(|item| is_ring_body_item(item)) {
            return Some("body");
        }
        let expected: Vec<&String> = desired
            .iter()
            .filter(|provider| declared.contains(provider))
            .collect();
        if !items_present(&live, &expected, notch) {
            return Some("missing");
        }
        if !items_follow_trigger(items) {
            return Some("order");
        }
    }
    // SketchyBar lays items out by `--add` order within a position group, so
    // an incremental add would append a new provider at the end regardless of
    // where it sorts. Any set or order change redeclares everything.
    (desired != declared).then_some("set")
}

/// A live item the rows body never owns: one provider's ring unit, or the
/// ring pill's edge/gap spacers. The plugin removes the other body's items
/// when it redeclares.
fn is_ring_body_item(item: &str) -> bool {
    let rest = match item.strip_prefix("showy_quota.") {
        Some(rest) => rest,
        None => return false,
    };
    rest.starts_with("gap.")
        || rest.starts_with("edge.")
        || rest.ends_with(".ring")
        || rest.ends_with(".ring_pace")
        || rest.ends_with(".bar0")
        || rest.ends_with(".bar0_pace")
        || rest.ends_with(".bar1")
        || rest.ends_with(".bar1_pace")
        || rest.contains(".pop_")
}

fn items_present(live: &HashSet<&str>, providers: &[&String], notch: bool) -> bool {
    let mut tail = vec!["showy_quota.stale", "showy_quota.degraded"];
    if !providers.is_empty() {
        tail.extend(["showy_quota.overflow", "showy_quota_bracket"]);
    }
    items_present_with_roles(live, providers, &PROVIDER_ITEM_ROLES, &tail, notch)
}

fn items_present_with_roles(
    live: &HashSet<&str>,
    units: &[&String],
    roles: &[&str],
    tail: &[&str],
    notch: bool,
) -> bool {
    let has = |name: &str| live.contains(name);
    units.iter().all(|unit| {
        roles
            .iter()
            .all(|role| has(&format!("showy_quota.{unit}.{role}")))
    }) && tail.iter().all(|name| has(name))
        && (!notch || (has("showy_quota.notch_q") && has("showy_quota.notch_e")))
}

/// A live item the ring body never owns: a rows slider lane, icon, slot,
/// the overflow item, or the notch anchors (ring mode stays left).
fn is_rows_body_item(item: &str) -> bool {
    item == "showy_quota.overflow"
        || item == "showy_quota.notch_q"
        || item == "showy_quota.notch_e"
        || {
            match item.strip_prefix("showy_quota.") {
                Some(rest) => {
                    rest.ends_with(".icon")
                        || rest.ends_with(".primary")
                        || rest.ends_with(".secondary")
                        || rest.ends_with(".tertiary")
                        || rest.ends_with(".quaternary")
                        || rest.ends_with(".slot")
                        || rest.ends_with("_marker")
                }
                None => false,
            }
        }
}

/// Ring-mode redeclare decision. `declared`/`desired` are unit ids (the state
/// file stores units in ring mode); `extra` covers the edge and gap spacers.
/// Ring mode never uses the notch anchors or the overflow item.
pub fn ring_redeclare_reason(
    force: bool,
    items: Option<&[String]>,
    declared: &[String],
    desired: &[String],
    extra: &[String],
) -> Option<&'static str> {
    if force {
        return Some("forced");
    }
    if let Some(items) = items {
        let live: HashSet<&str> = items.iter().map(String::as_str).collect();
        if live.iter().any(|item| is_rows_body_item(item)) {
            return Some("body");
        }
        let expected: Vec<&String> = desired
            .iter()
            .filter(|unit| declared.contains(unit))
            .collect();
        let mut tail: Vec<&str> = vec![
            "showy_quota.stale",
            "showy_quota.degraded",
            "showy_quota_bracket",
        ];
        tail.extend(extra.iter().map(String::as_str));
        // An empty strip declares no edges or bracket: nothing to check.
        if !expected.is_empty()
            && !items_present_with_roles(&live, &expected, &RING_UNIT_ROLES, &tail, false)
        {
            return Some("missing");
        }
        if !items_follow_trigger(items) {
            return Some("order");
        }
    }
    // Same ordering rule as the rows body: any set or order change redeclares
    // everything, so the pill keeps `--add` order.
    (desired != declared).then_some("set")
}

/// The bootstrap adds `showy_quota.trigger` where the user's sketchybarrc
/// wants the pill. A plugin run that outlives `sketchybar --reload` can
/// re-add provider items before the rc re-adds its earlier items, so any
/// showy-quota item listed before the trigger means the pill is out of place.
fn items_follow_trigger(items: &[String]) -> bool {
    if !items.iter().any(|item| item == TRIGGER_ITEM) {
        return true;
    }
    for item in items {
        if item == TRIGGER_ITEM {
            return true;
        }
        if item.starts_with("showy_quota.") || item.starts_with("showy_quota_") {
            return false;
        }
    }
    true
}

// ── wire output ─────────────────────────────────────────────────────────

pub struct FrameOutput<'a> {
    pub redeclare: Option<&'static str>,
    pub refresh: bool,
    pub providers: Vec<&'a str>,
    pub icon_requests: &'a [IconRequest],
    pub query: Option<&'a [String]>,
    pub args: &'a [String],
    /// Ring mode only: `unit=provider` pairs for the declaration loop.
    /// Unit and provider ids never contain `=`, so the shell splits safely.
    pub units: Option<&'a [String]>,
}

impl FrameOutput<'_> {
    pub fn to_wire(&self) -> String {
        let mut out = String::new();
        let state = [
            self.redeclare.unwrap_or("-"),
            if self.refresh { "1" } else { "0" },
        ];
        wire_line(&mut out, "state", state.iter().copied());
        wire_line(&mut out, "providers", self.providers.iter().copied());
        for request in self.icon_requests {
            wire_line(
                &mut out,
                "icon",
                [
                    request.provider.as_str(),
                    request.status.as_str(),
                    request.path.as_str(),
                ],
            );
        }
        if let Some(query) = self.query {
            wire_line(&mut out, "query", query.iter().map(String::as_str));
        }
        if let Some(units) = self.units {
            wire_line(&mut out, "units", units.iter().map(String::as_str));
        }
        wire_line(&mut out, "set", self.args.iter().map(String::as_str));
        out
    }
}

/// One wire record. Control characters are dropped from every field so data
/// can never break the framing; no legitimate argument carries one.
pub fn wire_line<'a>(out: &mut String, tag: &str, fields: impl IntoIterator<Item = &'a str>) {
    out.push_str(tag);
    for field in fields {
        out.push(WIRE_SEP);
        out.extend(field.chars().filter(|ch| !ch.is_control()));
    }
    out.push('\n');
}

// ── helpers ─────────────────────────────────────────────────────────────

fn parse_frame_text(text: &str) -> HashMap<&str, String> {
    text.lines()
        .filter_map(|line| line.split_once(WIRE_SEP))
        .map(|(key, hash)| (key, hash.to_owned()))
        .collect()
}

/// FNV-1a over the arguments. Stable across Rust versions, unlike
/// `DefaultHasher`, so a toolchain upgrade does not resend every item.
fn fnv1a(args: &[String]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for arg in args {
        for byte in arg.bytes().chain(std::iter::once(0x1f)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

fn uint_setting(raw: Option<&str>, fallback: i64, max: i64) -> i64 {
    match raw {
        Some(raw)
            if !raw.is_empty() && raw.len() <= 18 && raw.bytes().all(|b| b.is_ascii_digit()) =>
        {
            raw.parse::<i64>().map_or(fallback, |value| value.min(max))
        }
        _ => fallback,
    }
}

/// A plain decimal such as `0.28`, `1`, or `.5` (SketchyBar's image scale).
fn decimal(raw: &str) -> bool {
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    raw.len() <= 16
        && match raw.split_once('.') {
            Some((int, frac)) => (int.is_empty() || digits(int)) && digits(frac),
            None => digits(raw),
        }
}

/// `showy_quota_normalize_hex_or_default`: a 6-digit hex (optional `#`),
/// lowercased, or the token's documented default.
fn hex_or(raw: &str, fallback: &str) -> String {
    let hex = raw.strip_prefix('#').unwrap_or(raw);
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        hex.to_ascii_lowercase()
    } else {
        fallback.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketchybar::{sketchybar_rows, SketchybarOptions};
    use std::collections::BTreeMap;

    const NOW: i64 = 1_700_000_000;

    fn settings(env: &[(&str, &str)]) -> FrameSettings {
        let env: BTreeMap<String, String> = env
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        let config = RenderConfig::from_env_map(&env);
        FrameSettings::from_getter(|name| env.get(name).cloned(), &config)
    }

    fn rows(payload: &str) -> SketchybarRows {
        sketchybar_rows(
            payload.as_bytes(),
            &RenderConfig::default(),
            NOW,
            SketchybarOptions {
                stale: false,
                degraded_cli: false,
                bar_width: 80,
                stale_providers: &[],
            },
        )
        .expect("rows")
    }

    fn frame(rows: &SketchybarRows, settings: &FrameSettings, previous: Option<&str>) -> Frame {
        build_frame(&FrameInputs {
            rows,
            settings,
            label_drawing: true,
            hidden: &[],
            previous,
            icon_ready: &|_| false,
            icon_maker: false,
        })
    }

    /// The property list following `--set <item>` in `args`.
    fn props<'a>(args: &'a [String], item: &str) -> Vec<&'a str> {
        let start = args
            .windows(2)
            .position(|pair| pair[0] == "--set" && pair[1] == item)
            .unwrap_or_else(|| panic!("no --set {item} in {args:?}"));
        args[start + 2..]
            .iter()
            .take_while(|arg| *arg != "--set")
            .map(String::as_str)
            .collect()
    }

    fn items_for(providers: &[&str], notch: bool) -> Vec<String> {
        let mut items = vec!["front_app".to_owned(), TRIGGER_ITEM.to_owned()];
        if notch {
            items.push("showy_quota.notch_q".into());
            items.push("showy_quota.notch_e".into());
        }
        for provider in providers {
            for role in PROVIDER_ITEM_ROLES {
                items.push(format!("showy_quota.{provider}.{role}"));
            }
        }
        for name in [
            "showy_quota.overflow",
            "showy_quota.stale",
            "showy_quota.degraded",
            "showy_quota_bracket",
        ] {
            items.push(name.into());
        }
        items
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    const TWO: &str = r#"[
        {"provider":"codex","usage":{"primary":{"usedPercent":25,"resetsAt":"2023-11-14T23:53:20Z","windowMinutes":300}}},
        {"provider":"claude","usage":{"primary":{"usedPercent":40,"resetsAt":"2023-11-14T23:53:20Z","windowMinutes":300}}}
    ]"#;

    #[test]
    fn live_items_matching_the_stored_set_are_reused() {
        let items = items_for(&["codex", "claude"], false);
        let set = ids(&["codex", "claude"]);
        assert_eq!(
            redeclare_reason(false, Some(&items), &set, &set, false),
            None
        );
        assert_eq!(
            redeclare_reason(true, Some(&items), &set, &set, false),
            Some("forced")
        );
    }

    #[test]
    fn a_missing_item_or_anchor_forces_a_redeclare() {
        let set = ids(&["codex", "claude"]);
        let mut items = items_for(&["codex", "claude"], false);
        items.retain(|item| item != "showy_quota.claude.tertiary_marker");
        assert_eq!(
            redeclare_reason(false, Some(&items), &set, &set, false),
            Some("missing")
        );
        // Notch placement needs its two anchors as well.
        let items = items_for(&["codex", "claude"], false);
        assert_eq!(
            redeclare_reason(false, Some(&items), &set, &set, true),
            Some("missing")
        );
    }

    #[test]
    fn an_item_ahead_of_the_trigger_forces_a_redeclare() {
        let set = ids(&["codex"]);
        let mut items = items_for(&["codex"], false);
        items.retain(|item| item != "showy_quota.codex.icon");
        items.insert(0, "showy_quota.codex.icon".into());
        assert_eq!(
            redeclare_reason(false, Some(&items), &set, &set, false),
            Some("order")
        );
    }

    #[test]
    fn an_unreadable_item_list_still_catches_a_set_change() {
        let declared = ids(&["codex", "claude"]);
        assert_eq!(
            redeclare_reason(false, None, &declared, &declared, false),
            None
        );
        let reordered = ids(&["claude", "codex"]);
        assert_eq!(
            redeclare_reason(false, None, &declared, &reordered, false),
            Some("set")
        );
    }

    #[test]
    fn bar_items_parse_tolerates_a_dropped_reply() {
        assert_eq!(parse_bar_items(b""), None);
        assert_eq!(parse_bar_items(b"[!] timeout"), None);
        assert_eq!(parse_bar_items(b"{}"), Some(Vec::new()));
        assert_eq!(
            parse_bar_items(br#"{"items":["a","showy_quota.trigger"]}"#),
            Some(ids(&["a", "showy_quota.trigger"]))
        );
    }

    #[test]
    fn frame_sends_only_the_units_that_changed() {
        let settings = settings(&[]);
        let first = frame(&rows(TWO), &settings, None);
        assert!(first.args.contains(&"showy_quota.codex.label".to_owned()));
        assert!(first.args.contains(&"showy_quota.stale".to_owned()));

        let same = frame(&rows(TWO), &settings, Some(&first.frame_text));
        assert!(same.args.is_empty(), "nothing changed: {:?}", same.args);
        assert_eq!(same.frame_text, first.frame_text);

        let changed = TWO.replace("\"usedPercent\":40", "\"usedPercent\":50");
        let next = frame(&rows(&changed), &settings, Some(&first.frame_text));
        assert!(next.args.contains(&"showy_quota.claude.label".to_owned()));
        assert!(!next
            .args
            .iter()
            .any(|arg| arg.starts_with("showy_quota.codex.")));
        assert!(!next.args.contains(&"showy_quota.stale".to_owned()));
    }

    #[test]
    fn a_setting_outside_the_rows_resends_every_provider() {
        let first = frame(&rows(TWO), &settings(&[]), None);
        let recolored = settings(&[("SHOWY_QUOTA_PALETTE_ELAPSED", "ff0000")]);
        let next = frame(&rows(TWO), &recolored, Some(&first.frame_text));
        assert!(next.args.contains(&"showy_quota.codex.label".to_owned()));
        assert!(next.args.contains(&"showy_quota.claude.label".to_owned()));
        assert!(next
            .args
            .contains(&"slider.knob.background.color=0xffff0000".to_owned()));
    }

    #[test]
    fn hidden_providers_collapse_to_one_drawing_off() {
        let settings = settings(&[]);
        let rows = rows(TWO);
        let hidden = ids(&["claude"]);
        let frame = build_frame(&FrameInputs {
            rows: &rows,
            settings: &settings,
            label_drawing: false,
            hidden: &hidden,
            previous: None,
            icon_ready: &|_| false,
            icon_maker: false,
        });
        assert_eq!(
            props(&frame.args, "/^showy_quota\\.claude\\.[^.]*$/"),
            ["drawing=off"]
        );
        assert!(!frame.args.contains(&"showy_quota.claude.label".to_owned()));
        assert_eq!(
            props(&frame.args, "showy_quota.codex.label")[0],
            "drawing=off"
        );
    }

    #[test]
    fn lanes_stack_by_how_many_windows_are_live() {
        let payload = r#"[{"provider":"gemini","usage":{
            "primary":{"usedPercent":10,"resetsAt":"2023-11-14T23:53:20Z","windowMinutes":300},
            "secondary":{"usedPercent":20,"resetsAt":"2023-11-20T22:23:20Z","windowMinutes":10080},
            "tertiary":{"usedPercent":30,"resetsAt":"2023-12-14T22:13:20Z","windowMinutes":43200}}}]"#;
        let frame = frame(&rows(payload), &settings(&[]), None);
        let args = &frame.args;
        assert!(props(args, "showy_quota.gemini.primary").contains(&"y_offset=7"));
        assert!(props(args, "showy_quota.gemini.secondary").contains(&"y_offset=0"));
        assert!(props(args, "showy_quota.gemini.tertiary").contains(&"y_offset=-7"));
        assert_eq!(
            props(args, "showy_quota.gemini.quaternary")[0],
            "drawing=off"
        );
        assert_eq!(
            props(args, "showy_quota.gemini.tertiary_marker")[0],
            "drawing=on"
        );
        assert_eq!(
            props(args, "showy_quota.gemini.quaternary_marker")[0],
            "drawing=off"
        );
    }

    #[test]
    fn an_errored_provider_draws_its_label_and_a_warning_icon_only() {
        let payload = r#"[{"provider":"claude","error":{"message":"expired"}}]"#;
        let frame = frame(
            &rows(payload),
            &settings(&[("SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE", "font")]),
            None,
        );
        let args = &frame.args;
        assert_eq!(props(args, "showy_quota.claude.label")[0], "drawing=on");
        for role in LANE_ROLES {
            assert_eq!(
                props(args, &format!("showy_quota.claude.{role}"))[0],
                "drawing=off"
            );
        }
        assert!(props(args, "showy_quota.claude.icon").contains(&"icon.color=0xffee5396"));
    }

    #[test]
    fn missing_icons_are_requested_only_from_a_plugin_that_can_draw_them() {
        let rows = rows(TWO);
        let settings = settings(&[("SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE", "/tmp/sb/")]);
        let build = |icon_maker: bool, ready: &dyn Fn(&str) -> bool| {
            build_frame(&FrameInputs {
                rows: &rows,
                settings: &settings,
                label_drawing: true,
                hidden: &[],
                previous: None,
                icon_ready: ready,
                icon_maker,
            })
        };
        let path = "/tmp/sb/icon-v5-codex-f2f4f8-6c7086-f0af00-ee5396.png";

        let without_maker = build(false, &|_| false);
        assert!(without_maker.icon_requests.is_empty());
        assert_eq!(
            props(&without_maker.args, "showy_quota.codex.icon")[0],
            "drawing=off"
        );

        let with_maker = build(true, &|_| false);
        assert_eq!(with_maker.icon_requests[0].path, path);
        assert_eq!(with_maker.icon_requests[0].status, "none");

        let ready = build(true, &|candidate| candidate == path);
        assert!(props(&ready.args, "showy_quota.codex.icon")
            .contains(&format!("background.image={path}").as_str()));
        assert_eq!(ready.icon_requests.len(), 1, "claude still needs its icon");
    }

    #[test]
    fn icon_paths_key_on_status_and_palette() {
        let keyed = settings(&[
            ("SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE", "/c"),
            ("SHOWY_QUOTA_PALETTE_ICON_TEXT", "#ABCDEF"),
        ]);
        assert_eq!(
            keyed.icon_png_path("codex", "major").as_deref(),
            Some("/c/icon-v5-codex-abcdef-6c7086-f0af00-ee5396-major.png")
        );
        assert_eq!(
            keyed.icon_png_path("codex", "error").as_deref(),
            Some("/c/icon-v5-codex-abcdef-6c7086-f0af00-ee5396-error.png")
        );
        assert_eq!(
            keyed.icon_png_path("codex", "none").as_deref(),
            Some("/c/icon-v5-codex-abcdef-6c7086-f0af00-ee5396.png")
        );
        assert_eq!(settings(&[]).icon_png_path("codex", "none"), None);
    }

    #[test]
    fn status_urls_open_only_for_named_public_hosts() {
        for url in [
            "https://status.openai.com/",
            "http://status.example.com:8443/x?y#z",
        ] {
            assert!(status_url_is_openable(url), "{url}");
        }
        for url in [
            "ftp://status.example.com",
            "https://user@status.example.com",
            "https://127.0.0.1/",
            "https://localhost/",
            "https://app.localhost/",
            "https://status.example.com:0/",
            "https://status.example.com:65536/",
            "https://status example.com/",
            "https://-bad.example.com/",
            "https://[::1]/",
            "http://0x7f000001/",
            "http://0x7f.1/",
            "http://0x0/",
            "http://status.example.com.127/",
        ] {
            assert!(!status_url_is_openable(url), "{url}");
        }
    }

    #[test]
    fn unsafe_click_commands_fall_back_to_the_default() {
        assert!(click_command_is_safe("open -b com.steipete.codexbar"));
        for click in ["open x; rm -rf ~", "a | b", "$(x)", "a > b", "a\nb", ""] {
            assert!(!click_command_is_safe(click), "{click:?}");
        }
        assert_eq!(
            settings(&[("SHOWY_QUOTA_SKETCHYBAR_CLICK", "open x && y")]).click,
            DEFAULT_CLICK
        );
    }

    #[test]
    fn status_clicks_open_a_quoted_status_page() {
        let settings = settings(&[]);
        assert_eq!(
            click_script_for_status(&settings, "major", "https://status.example.com/it's"),
            "open 'https://status.example.com/it'\\''s'"
        );
        assert_eq!(
            click_script_for_status(&settings, "none", "https://status.example.com/"),
            DEFAULT_CLICK
        );
    }

    #[test]
    fn wire_records_drop_control_characters() {
        let mut out = String::new();
        wire_line(&mut out, "set", ["a\nb", "c\u{1f}d"]);
        assert_eq!(out, "set\u{1f}ab\u{1f}cd\n");
    }

    // ── ring body ────────────────────────────────────────────────────

    use crate::sketchybar_ring::{ring_units, RingUnit};

    fn ring_test_units(payload: &str, env: &[(&str, &str)]) -> Vec<RingUnit> {
        let map: BTreeMap<String, String> = env
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        let config = RenderConfig::from_env_map(&map);
        ring_units(
            payload.as_bytes(),
            &config,
            NOW,
            SketchybarOptions {
                stale: false,
                degraded_cli: false,
                bar_width: 80,
                stale_providers: &[],
            },
        )
        .expect("ring units")
    }

    fn ring_frame(payload: &str, env: &[(&str, &str)], previous: Option<&str>) -> Frame {
        let settings = settings(env);
        let units = ring_test_units(payload, env);
        build_ring_frame(&RingFrameInputs {
            units: &units,
            settings: &settings,
            stale: false,
            degraded_cli: false,
            previous,
        })
    }

    const RING_ENV: [(&str, &str); 1] = [("SHOWY_QUOTA_SKETCHYBAR_BODY", "ring")];

    #[test]
    fn the_ring_body_is_opt_in_and_anything_else_stays_rows() {
        assert_eq!(settings(&[]).body, Body::Rows);
        assert_eq!(settings(&RING_ENV).body, Body::Ring);
        assert_eq!(
            settings(&[("SHOWY_QUOTA_SKETCHYBAR_BODY", "circles")]).body,
            Body::Rows
        );
    }

    const THREE_WINDOWS: &str = r#"[{"provider":"commandcode","usage":{
        "primary":{"usedPercent":0.0,"windowMinutes":300},
        "secondary":{"usedPercent":45.0,"resetsAt":"2023-11-15T06:00:00Z","windowMinutes":10080},
        "tertiary":{"usedPercent":52.0,"resetsAt":"2023-12-10T06:00:00Z","windowMinutes":43200}}}]"#;

    #[test]
    fn ring_units_draw_the_longest_window_with_stacked_bars_and_pace() {
        let frame = ring_frame(THREE_WINDOWS, &RING_ENV, None);
        let args = &frame.args;
        let ring = props(args, "showy_quota.commandcode.ring");
        assert!(ring.contains(&"ring.value=0.48"), "{ring:?}");
        assert!(ring.contains(&"ring.color=0xff25be6a"), "{ring:?}");
        assert!(ring.contains(&"ring.line_width=3"), "{ring:?}");
        assert!(ring.contains(&"ring.marker=⌘"), "{ring:?}");
        assert!(ring.contains(&"ring.marker.padding_left=1"), "{ring:?}");
        // Double stack: 5h bar at -1, 7d bar at -7, label at +8.
        assert!(props(args, "showy_quota.commandcode.bar0").contains(&"y_offset=-1"));
        assert!(props(args, "showy_quota.commandcode.bar1").contains(&"y_offset=-7"));
        assert!(props(args, "showy_quota.commandcode.label").contains(&"label.y_offset=8"));
        assert_eq!(
            props(args, "showy_quota.commandcode.label")[1],
            "label=idle"
        );
        // Pace tick on the ring and a knob on the 7d bar; the idle 5h bar
        // has no expected value, so no knob.
        assert_eq!(
            props(args, "showy_quota.commandcode.ring_pace")[0],
            "drawing=on"
        );
        assert_eq!(
            props(args, "showy_quota.commandcode.bar0_pace")[0],
            "drawing=off"
        );
        assert_eq!(
            props(args, "showy_quota.commandcode.bar1_pace")[0],
            "drawing=on"
        );
        // The popup lists the ring window first with its % badge.
        let row0 = props(args, "showy_quota.commandcode.pop_row0");
        assert!(row0.contains(&"ring.value=0.48"), "{row0:?}");
        assert!(row0.contains(&"label.badge=48%"), "{row0:?}");
    }

    #[test]
    fn an_error_without_last_known_usage_has_no_arc_and_names_its_kind() {
        let payload = r#"[{"provider":"cursor","error":{"message":"auth expired"}}]"#;
        let frame = ring_frame(payload, &RING_ENV, None);
        let args = &frame.args;
        let ring = props(args, "showy_quota.cursor.ring");
        assert!(ring.contains(&"ring.value=0.00"), "{ring:?}");
        assert!(ring.contains(&"ring.color=0xff6c7086"), "{ring:?}");
        assert!(props(args, "showy_quota.cursor.label").contains(&"label=auth"));
        for role in ["bar0", "bar0_pace", "bar1", "bar1_pace"] {
            assert_eq!(
                props(args, &format!("showy_quota.cursor.{role}"))[0],
                "drawing=off"
            );
        }
        assert_eq!(
            props(args, "showy_quota.cursor.pop_header")[0],
            "drawing=off"
        );
        let alert = props(args, "showy_quota.cursor.pop_alert0");
        assert!(alert.contains(&"icon=⚠"), "{alert:?}");
        assert!(
            alert.iter().any(|prop| prop.contains("auth expired")),
            "{alert:?}"
        );
    }

    #[test]
    fn an_error_with_last_known_usage_keeps_a_grey_arc() {
        let payload = r#"[{"provider":"claude","error":{"message":"connect timed out"},
            "usage":{"secondary":{"usedPercent":27.0,"resetsAt":"2023-11-15T06:00:00Z","windowMinutes":10080},
                     "updatedAt":"2023-11-14T21:40:00Z"}}]"#;
        let frame = ring_frame(payload, &RING_ENV, None);
        let args = &frame.args;
        let ring = props(args, "showy_quota.claude.ring");
        assert!(ring.contains(&"ring.value=0.73"), "{ring:?}");
        assert!(ring.contains(&"ring.color=0xff6c7086"), "{ring:?}");
        // Neutral track, never the red empty-pool tint: the data is stale,
        // not exhausted.
        assert!(ring.contains(&"ring.track_color=0xff3a3a4a"), "{ring:?}");
        assert!(props(args, "showy_quota.claude.label").contains(&"label=net"));
        let note = props(args, "showy_quota.claude.pop_note");
        assert!(
            note.iter().any(|prop| prop.contains("last known 73%")),
            "{note:?}"
        );
    }

    #[test]
    fn an_incident_tints_the_logo_and_links_the_status_page() {
        let payload = r#"[{"provider":"codex",
            "status":{"indicator":"minor","description":"Elevated error rates","url":"https://status.openai.com/"},
            "usage":{"secondary":{"usedPercent":30.0,"resetsAt":"2023-11-15T06:00:00Z","windowMinutes":10080}}}]"#;
        let frame = ring_frame(payload, &RING_ENV, None);
        let args = &frame.args;
        let ring = props(args, "showy_quota.codex.ring");
        assert!(ring.contains(&"ring.marker.color=0xfff0af00"), "{ring:?}");
        assert!(
            ring.iter()
                .any(|prop| prop.contains("open 'https://status.openai.com/'")),
            "{ring:?}"
        );
        let alert = props(args, "showy_quota.codex.pop_alert0");
        assert!(alert.contains(&"icon=●"), "{alert:?}");
        assert!(
            alert
                .iter()
                .any(|prop| prop.contains("Elevated error rates")),
            "{alert:?}"
        );
        let note = props(args, "showy_quota.codex.pop_note");
        assert!(
            note.iter()
                .any(|prop| prop.contains("click opens status.openai.com")),
            "{note:?}"
        );
    }

    #[test]
    fn banked_resets_badge_the_ring_and_alert_the_popup() {
        let payload = r#"[{"provider":"codex","usage":{
            "secondary":{"usedPercent":30.0,"resetsAt":"2023-11-15T06:00:00Z","windowMinutes":10080},
            "codexResetCredits":{"availableCount":2,"credits":[
                {"expires_at":"2023-12-05T04:20:20Z","status":"available"},
                {"expires_at":"2023-12-22T20:47:00Z","status":"available"}]}}}]"#;
        let frame = ring_frame(payload, &RING_ENV, None);
        let args = &frame.args;
        let ring = props(args, "showy_quota.codex.ring");
        assert!(ring.contains(&"ring.badge=2"), "{ring:?}");
        assert!(
            ring.contains(&"ring.badge.background.color=0xff78a9ff"),
            "{ring:?}"
        );
        let alert = props(args, "showy_quota.codex.pop_alert0");
        assert!(alert.contains(&"icon=2"), "{alert:?}");
        assert!(
            alert
                .iter()
                .any(|prop| prop.contains("2 free resets banked")),
            "{alert:?}"
        );
    }

    #[test]
    fn antigravity_pools_carry_their_letters() {
        let payload = r#"[{"provider":"antigravity","usage":{
            "primary":{"usedPercent":87.0,"resetsAt":"2023-11-14T23:53:20Z","windowMinutes":300},
            "secondary":{"usedPercent":100.0,"resetsAt":"2023-11-15T06:00:00Z","windowMinutes":10080},
            "extraRateWindows":[
                {"id":"ag-gemini-5h","title":"Gemini 5-hour",
                 "window":{"usedPercent":87.0,"resetsAt":"2023-11-14T23:53:20Z","windowMinutes":300}},
                {"id":"ag-gemini-weekly","title":"Gemini weekly",
                 "window":{"usedPercent":70.0,"resetsAt":"2023-11-20T06:00:00Z","windowMinutes":10080}},
                {"id":"ag-3p-weekly","title":"Claude/GPT weekly",
                 "window":{"usedPercent":100.0,"resetsAt":"2023-11-15T06:00:00Z","windowMinutes":10080}}]}}]"#;
        let frame = ring_frame(payload, &RING_ENV, None);
        let args = &frame.args;
        assert!(props(args, "showy_quota.antigravity.g.ring").contains(&"ring.marker.badge=G"));
        assert!(props(args, "showy_quota.antigravity.c.ring").contains(&"ring.marker.badge=C"));
        // An exhausted pool shows the same grey track as a live one, no red.
        let track = |item: &str| {
            props(args, item)
                .into_iter()
                .find(|prop| prop.starts_with("ring.track_color="))
        };
        assert_eq!(
            track("showy_quota.antigravity.c.ring"),
            track("showy_quota.antigravity.g.ring")
        );
    }

    #[test]
    fn ring_frames_diff_per_unit_and_redeclare_on_missing_items() {
        let first = ring_frame(THREE_WINDOWS, &RING_ENV, None);
        assert!(first
            .args
            .contains(&"showy_quota.commandcode.ring".to_owned()));
        let same = ring_frame(THREE_WINDOWS, &RING_ENV, Some(&first.frame_text));
        assert!(same.args.is_empty(), "nothing changed: {:?}", same.args);

        let extra = ring_extra_items(&ring_test_units(THREE_WINDOWS, &RING_ENV));
        let live: Vec<String> = ["showy_quota.trigger", "showy_quota_bracket"]
            .iter()
            .map(|name| (*name).to_owned())
            .chain(
                RING_UNIT_ROLES
                    .iter()
                    .map(|role| format!("showy_quota.commandcode.{role}")),
            )
            .chain(
                ["showy_quota.stale", "showy_quota.degraded"]
                    .iter()
                    .map(|name| (*name).to_owned()),
            )
            .chain(extra)
            .collect();
        let declared = ["commandcode".to_owned()];
        let desired = ["commandcode".to_owned()];
        assert_eq!(
            ring_redeclare_reason(false, Some(&live), &declared, &desired, &[]),
            None
        );
        let mut ring_leftover = live.clone();
        ring_leftover.push("showy_quota.commandcode.ring".to_owned());
        assert_eq!(
            redeclare_reason(false, Some(&ring_leftover), &declared, &desired, false),
            Some("body")
        );
        let mut gone = live.clone();
        gone.retain(|item| item != "showy_quota.commandcode.bar1");
        assert_eq!(
            ring_redeclare_reason(false, Some(&gone), &declared, &desired, &[]),
            Some("missing")
        );
        let mut rows_leftover = live.clone();
        rows_leftover.push("showy_quota.commandcode.primary".to_owned());
        assert_eq!(
            ring_redeclare_reason(false, Some(&rows_leftover), &declared, &desired, &[]),
            Some("body")
        );
        assert_eq!(
            ring_redeclare_reason(
                false,
                Some(&live),
                &declared,
                &["commandcode".to_owned(), "antigravity.g".to_owned()],
                &[]
            ),
            Some("set")
        );
    }

    #[test]
    fn an_empty_ring_strip_settles_instead_of_redeclaring_every_tick() {
        // The plugin declares no edges or bracket when no unit is visible.
        let live = vec![
            "showy_quota.trigger".to_owned(),
            "showy_quota.stale".to_owned(),
            "showy_quota.degraded".to_owned(),
        ];
        let extra = ring_extra_items(&[]);
        assert_eq!(
            ring_redeclare_reason(false, Some(&live), &[], &[], &extra),
            None
        );
    }
}
