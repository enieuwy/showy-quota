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
use crate::sketchybar::{RowLane, SketchybarRow, SketchybarRows, LANE_COUNT};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSettings {
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

/// Anchored regex for every item of one provider. Provider ids are validated
/// to `[A-Za-z0-9_.-]`; only the dot needs escaping.
pub fn provider_item_regex(provider: &str) -> String {
    format!("/^showy_quota\\.{}\\./", provider.replace('.', "\\."))
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
    if !host.split('.').all(label_ok) || host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return false;
    }
    let lower = host.to_ascii_lowercase();
    !(lower == "localhost" || lower.ends_with(".localhost"))
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

fn items_present(live: &HashSet<&str>, providers: &[&String], notch: bool) -> bool {
    let has = |name: &str| live.contains(name);
    let providers_present = providers.iter().all(|provider| {
        PROVIDER_ITEM_ROLES
            .iter()
            .all(|role| has(&format!("showy_quota.{provider}.{role}")))
    });
    providers_present
        && has("showy_quota.stale")
        && has("showy_quota.degraded")
        && (providers.is_empty() || has("showy_quota.overflow"))
        && (!notch || (has("showy_quota.notch_q") && has("showy_quota.notch_e")))
        && (providers.is_empty() || has("showy_quota_bracket"))
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
            props(&frame.args, "/^showy_quota\\.claude\\./"),
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
}
