//! SketchyBar notch placement planner.
//!
//! Decides which providers sit right of the MacBook notch, whether countdown
//! labels hide, and which providers collapse into the `+N` item, then builds
//! the `sketchybar --set` arguments that apply the plan. The plugin feeds it
//! one batched geometry query (`--emit sketchybar-layout`); before this module
//! the same planner ran as jq on the plugin's hot path.
//!
//! Geometry comes only from SketchyBar itself: two 1pt anchors at
//! `showy_quota.notch_q` and `showy_quota.notch_e` mark the notch gap exactly
//! as SketchyBar reserves it, so the planner needs no model-specific notch
//! size. The notch display is the one maximising the gap between the `q`
//! anchor's right edge and the `e` anchor's right edge. Note `estart` is the
//! `e` anchor's RIGHT edge, not its left: providers placed right of the notch
//! start at `estart`.
//!
//! Non-obvious rules:
//!
//! - `rect` returns nothing when an item has no rect on a display, or when
//!   the rect origin sits at or left of -9000. SketchyBar parks off-display
//!   items there, so that counts as missing.
//! - `drawn` requires `geometry.drawing == "on"` plus a usable rect.
//! - A gap under 20pt is no notch: everything stays left (`notch: false`,
//!   without measurement keys).
//! - The three measurement inputs (`label_extra`, `gap_full`, `gap_compact`)
//!   are the previous plan's values. Labels hidden by a compact plan cannot
//!   be measured, so the last measured values carry them; the planner
//!   replaces each one it can measure now. First-run defaults are the label
//!   width plus 3 and the icon padding plus 3.
//! - Gaps between neighbours only count when in `[0, 64)`. A pair whose left
//!   provider shows its label measures the full gap; otherwise the compact
//!   gap. The first measured value of each kind wins, else the carried one.
//! - `left_need`/`right_need` measure what each wing needs for a split after
//!   provider `k`, including the stale/degraded tail widths on the wing that
//!   holds every provider. `best` is the widest split both wings afford.
//! - The overflow branch never draws under the notch: it keeps the widest
//!   left prefix (always at least one provider), fills the right wing, and
//!   summarizes the rest in the `+N` item.
//! - A missing or `false` position/plan field counts as absent, and an empty
//!   candidate list falls back to the carried values.
//!
//! The module is pure: no file I/O, no process spawning.

use std::collections::HashMap;

use serde_json::Value;

/// Carried state from the previous plan file (`notch-layout.json`). Tolerant:
/// any input that is not a JSON object yields `PreviousPlan::default()` with
/// `valid == false`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreviousPlan {
    /// True when the raw text parsed as a JSON object.
    pub valid: bool,
    /// `.compact == true` in the previous plan.
    pub compact: bool,
    /// `.hidden` (strings only; non-strings are dropped).
    pub hidden: Vec<String>,
    /// `.label_extra` when a JSON number.
    pub label_extra: Option<f64>,
    /// `.gap_full` when a JSON number.
    pub gap_full: Option<f64>,
    /// `.gap_compact` when a JSON number.
    pub gap_compact: Option<f64>,
}

/// Parse the previous plan file content tolerantly. Mirrors the shell, which
/// carries nothing when the plan is not a JSON object.
pub fn parse_previous_plan(raw: &str) -> PreviousPlan {
    let value: Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) => return PreviousPlan::default(),
    };
    let obj = match value.as_object() {
        Some(obj) => obj,
        None => return PreviousPlan::default(),
    };
    PreviousPlan {
        valid: true,
        compact: obj.get("compact").and_then(Value::as_bool) == Some(true),
        hidden: obj
            .get("hidden")
            .and_then(Value::as_array)
            .map(|hidden| {
                hidden
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        label_extra: obj.get("label_extra").and_then(Value::as_f64),
        gap_full: obj.get("gap_full").and_then(Value::as_f64),
        gap_compact: obj.get("gap_compact").and_then(Value::as_f64),
    }
}

/// Planner inputs from the environment. The parent maps a non-numeric
/// `SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH` ("dynamic" or garbage) to 32 before
/// filling `label_width`.
#[derive(Debug, Clone, PartialEq)]
pub struct NotchSettings {
    /// `SHOWY_QUOTA_SKETCHYBAR_NOTCH_MARGIN` (default 4).
    pub margin: f64,
    /// `SHOWY_QUOTA_SKETCHYBAR_ICON_PADDING_LEFT` (default 5).
    pub icon_padding_left: f64,
    /// `SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH` as a number (parent maps
    /// "dynamic"/garbage to 32).
    pub label_width: f64,
    /// `SHOWY_QUOTA_SKETCHYBAR_ICON_WIDTH` (default 22).
    pub icon_width: f64,
    /// `SHOWY_QUOTA_SKETCHYBAR_BAR_WIDTH` (default 83).
    pub bar_width: f64,
}

/// The planned placement plus the `sketchybar` arguments that apply it.
#[derive(Debug, Clone, PartialEq)]
pub struct NotchLayout {
    /// The plan JSON the shell writes to notch-layout.json. Same keys and
    /// value shapes as the jq output:
    /// `{"notch":..,"compact":..,"right":[..],"hidden":[..],"overflow":N}`
    /// and, when the planner reached the measurement stage,
    /// `"label_extra","gap_full","gap_compact"`. Whole numbers print without
    /// a fraction (35, not 35.0); other numbers print as serde_json does.
    pub plan_json: String,
    /// True when countdown labels stay hidden.
    pub compact: bool,
    /// Providers placed right of the notch (`position=e`).
    pub right: Vec<String>,
    /// Providers that fit nowhere; summarized by the `+N` item.
    pub hidden: Vec<String>,
    /// Count summarized by the `+N` item.
    pub overflow: usize,
    /// `sketchybar` arguments that apply the plan.
    pub args: Vec<String>,
    /// True when the new plan draws more than `previous`: previous.compact
    /// && !new.compact, or some provider in previous.hidden is no longer
    /// hidden. The shell then re-runs the render.
    pub reveal: bool,
}

/// Plan the notch split from one batched SketchyBar query reply.
///
/// `measured` is the raw stdout of one batched
/// `sketchybar --query bar --query displays --query <item>...` call: a stream
/// of concatenated JSON documents (bar object, displays array, then one
/// object per item). Returns None when `measured` holds no JSON document at
/// all (empty or whitespace-only reply: SketchyBar dropped it) — the caller
/// then keeps the current placement. Documents that fail to parse end the
/// stream (keep what parsed before them). `providers` are provider ids in
/// render order.
pub fn notch_layout(
    measured: &[u8],
    providers: &[String],
    settings: &NotchSettings,
    previous: &PreviousPlan,
) -> Option<NotchLayout> {
    let mut docs: Vec<Value> = Vec::new();
    let stream = serde_json::Deserializer::from_slice(measured).into_iter::<Value>();
    for doc in stream {
        match doc {
            Ok(value) => docs.push(value),
            Err(_) => break,
        }
    }
    // A reply cut short before the notch anchors carries no geometry to plan
    // from. Planning "no notch" from it would pull every provider left, maybe
    // under the notch, so treat it like a dropped reply.
    let has_item = |name: &str| {
        docs.iter()
            .skip(2)
            .any(|doc| doc.get("name").and_then(Value::as_str) == Some(name))
    };
    if docs.is_empty()
        || (!providers.is_empty()
            && !(has_item("showy_quota.notch_q") && has_item("showy_quota.notch_e")))
    {
        return None;
    }
    let plan = plan_layout(&docs, providers, settings, previous);
    let plan_json = render_plan(&plan);
    let args = apply_args(providers, &plan);
    // Mirrors the shell reveal check: the new plan draws more than the old
    // one when labels come back, or when a hidden provider is visible again.
    let reveal = (previous.compact && !plan.compact)
        || previous.hidden.iter().any(|id| !plan.hidden.contains(id));
    Some(NotchLayout {
        plan_json,
        compact: plan.compact,
        right: plan.right,
        hidden: plan.hidden,
        overflow: plan.overflow,
        args,
        reveal,
    })
}

/// Internal plan: `meas` is `None` for the no-notch plan, which carries no
/// measurement keys (mirrors jq `none`).
struct Plan {
    notch: bool,
    compact: bool,
    right: Vec<String>,
    hidden: Vec<String>,
    overflow: usize,
    meas: Option<(f64, f64, f64)>,
}

fn none_plan() -> Plan {
    Plan {
        notch: false,
        compact: false,
        right: Vec::new(),
        hidden: Vec::new(),
        overflow: 0,
        meas: None,
    }
}

/// Usable rect `(x, w)` on a display, or `None` when the item has no rect
/// there or SketchyBar parked it off-display (origin at or left of -9000).
fn rect(item: &Value, display: &str) -> Option<(f64, f64)> {
    let rect = item.get("bounding_rects")?.get(display)?;
    if rect.is_null() {
        return None;
    }
    let x = rect.get("origin")?.get(0)?.as_f64()?;
    if x <= -9000.0 {
        return None;
    }
    let w = rect.get("size")?.get(0)?.as_f64()?;
    Some((x, w))
}

/// An item counts as drawn when it exists, has `geometry.drawing == "on"`,
/// and has a usable rect on the display.
fn drawn(item: Option<&Value>, display: &str) -> bool {
    match item {
        Some(it) if !it.is_null() => {
            it.get("geometry")
                .and_then(|g| g.get("drawing"))
                .and_then(Value::as_str)
                == Some("on")
                && rect(it, display).is_some()
        }
        _ => false,
    }
}

/// Slot position with the jq fallback (`$s.geometry.position // "left"`):
/// missing, null, or false counts as `"left"`. A present non-string position
/// matches no side, so it maps to `None`.
fn slot_pos(slot: Option<&Value>) -> Option<String> {
    match slot
        .and_then(|s| s.get("geometry"))
        .and_then(|g| g.get("position"))
    {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Some("left".to_string()),
        Some(Value::String(pos)) => Some(pos.clone()),
        Some(_) => None,
    }
}

/// Raw position for neighbour items (no `"left"` default; mirrors the jq
/// `== "q"` / `== "center"` comparisons directly).
fn raw_pos(item: &Value) -> Option<&str> {
    item.get("geometry")?.get("position")?.as_str()
}

/// jq string interpolation of a scalar for the `"display-\(id)"` match.
fn interp(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

/// Right edge of an item's rect when the item is drawn on the display.
fn edge(item: Option<&Value>, display: &str) -> Option<f64> {
    let item = item.filter(|it| drawn(Some(it), display))?;
    rect(item, display).map(|(x, w)| x + w)
}

fn plan_layout(
    docs: &[Value],
    providers: &[String],
    settings: &NotchSettings,
    previous: &PreviousPlan,
) -> Plan {
    if providers.is_empty() {
        return none_plan();
    }
    // Item documents are objects with a `name` key, from index 2 on.
    // `$all[0]` is the bar (`padding_right`), `$all[1]` the displays array.
    let items: Vec<&Value> = docs
        .iter()
        .skip(2)
        .filter(|doc| doc.is_object() && doc.get("name").and_then(Value::as_str).is_some())
        .collect();
    // Later duplicates win, mirroring jq `from_entries`.
    let mut by: HashMap<&str, &Value> = HashMap::new();
    for item in &items {
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            by.insert(name, item);
        }
    }
    let (Some(qa), Some(ea)) = (
        by.get("showy_quota.notch_q").copied(),
        by.get("showy_quota.notch_e").copied(),
    ) else {
        return none_plan();
    };

    // The notch display maximises the q-anchor right edge to e-anchor right
    // edge gap. First maximum wins on ties.
    struct Gap {
        display: String,
        qe: f64,
        estart: f64,
        gap: f64,
    }
    let mut notch: Option<Gap> = None;
    if let Some(rects) = qa.get("bounding_rects").and_then(Value::as_object) {
        for display in rects.keys() {
            if !drawn(Some(qa), display) || !drawn(Some(ea), display) {
                continue;
            }
            if let (Some((qx, qw)), Some((ex, ew))) = (rect(qa, display), rect(ea, display)) {
                let qe = qx + qw;
                let estart = ex + ew;
                let replace = notch.as_ref().is_none_or(|best| estart - qe > best.gap);
                if replace {
                    notch = Some(Gap {
                        display: display.clone(),
                        qe,
                        estart,
                        gap: estart - qe,
                    });
                }
            }
        }
    }
    let notch = match notch {
        Some(notch) if notch.gap >= 20.0 => notch,
        _ => return none_plan(),
    };
    let display = notch.display.as_str();

    struct Row {
        id: String,
        pos: Option<String>,
        left: Option<f64>,
        #[allow(dead_code)]
        slot_r: Option<f64>,
        label_r: Option<f64>,
        compact: Option<f64>,
        label_extra: Option<f64>,
        right_r: Option<f64>,
    }
    let mut rows: Vec<Row> = Vec::with_capacity(providers.len());
    for id in providers {
        let icon = by.get(format!("showy_quota.{id}.icon").as_str()).copied();
        let slot = by.get(format!("showy_quota.{id}.slot").as_str()).copied();
        let label = by.get(format!("showy_quota.{id}.label").as_str()).copied();
        // Left edge over the drawn icon/slot rects; `min` of none is missing.
        let mut left: Option<f64> = None;
        for item in [icon, slot].into_iter().flatten() {
            if !drawn(Some(item), display) {
                continue;
            }
            if let Some((x, _)) = rect(item, display) {
                left = Some(left.map_or(x, |best: f64| best.min(x)));
            }
        }
        let slot_r = edge(slot, display);
        let label_r = edge(label, display);
        rows.push(Row {
            id: id.clone(),
            pos: slot_pos(slot),
            left,
            slot_r,
            label_r,
            compact: match (left, slot_r) {
                (Some(left), Some(slot_r)) => Some(slot_r - left),
                _ => None,
            },
            label_extra: match (label_r, slot_r) {
                (Some(label_r), Some(slot_r)) => Some(label_r - slot_r),
                _ => None,
            },
            // `.label_r // .slot_r`: the label edge when shown, else slot.
            right_r: label_r.or(slot_r),
        });
    }

    // Carried measurements fill whatever cannot be measured live.
    let carried_label = previous.label_extra.unwrap_or(settings.label_width + 3.0);
    let carried_full = previous
        .gap_full
        .unwrap_or(settings.icon_padding_left + 3.0);
    let carried_compact = previous
        .gap_compact
        .unwrap_or(settings.icon_padding_left + 3.0);
    let compact_min = settings.icon_width + settings.bar_width;

    let label_extra = rows
        .iter()
        .filter_map(|row| row.label_extra)
        .next()
        .unwrap_or(carried_label);
    let compact_width = rows
        .iter()
        .filter_map(|row| row.compact)
        .fold(None::<f64>, |best, width| {
            Some(best.map_or(width, |best: f64| best.max(width)))
        })
        .unwrap_or(compact_min);
    // Neighbour gaps in `[0, 64)` between same-side providers. A pair whose
    // left provider shows its label measures the full gap.
    let mut full_gaps: Vec<f64> = Vec::new();
    let mut compact_gaps: Vec<f64> = Vec::new();
    for pair in rows.windows(2) {
        let (first, second) = (&pair[0], &pair[1]);
        if first.pos != second.pos {
            continue;
        }
        if let (Some(right_r), Some(left)) = (first.right_r, second.left) {
            let gap = left - right_r;
            if (0.0..64.0).contains(&gap) {
                if first.label_r.is_some() {
                    full_gaps.push(gap);
                } else {
                    compact_gaps.push(gap);
                }
            }
        }
    }
    let gap_full = full_gaps.into_iter().next().unwrap_or(carried_full);
    let gap_compact = compact_gaps.into_iter().next().unwrap_or(carried_compact);

    let full_widths: Vec<f64> = rows
        .iter()
        .map(|row| row.compact.unwrap_or(compact_width) + label_extra)
        .collect();
    let compact_widths: Vec<f64> = rows
        .iter()
        .map(|row| row.compact.unwrap_or(compact_width))
        .collect();

    // A provider that is not on the left (or has no left edge) means the
    // pill is mid-move: leave it alone.
    let Some(start) = rows[0].left else {
        return none_plan();
    };
    if rows[0].pos.as_deref() != Some("left") {
        return none_plan();
    }

    // Neighbours outside the showy-quota namespace bound both wings.
    let others: Vec<&Value> = items
        .iter()
        .filter(|item| {
            !item
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.starts_with("showy_quota"))
        })
        .copied()
        .collect();
    let mut left_limit = notch.qe;
    for item in &others {
        if matches!(raw_pos(item), Some("q" | "center")) && drawn(Some(item), display) {
            if let Some((x, _)) = rect(item, display) {
                if x >= start && x < left_limit {
                    left_limit = x;
                }
            }
        }
    }
    let mut display_w: Option<f64> = None;
    if let Some(displays) = docs.get(1).and_then(Value::as_array) {
        for candidate in displays {
            let id = candidate.get("arrangement-id").map(interp);
            if id.map(|id| format!("display-{id}")).as_deref() == Some(display) {
                display_w = candidate
                    .get("frame")
                    .and_then(|frame| frame.get("w"))
                    .and_then(Value::as_f64);
                break;
            }
        }
    }
    let bar_pad = docs
        .first()
        .and_then(|bar| bar.get("padding_right"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let right_anchor = display_w.map_or(1e9, |w| w - bar_pad);
    let mut right_limit = right_anchor;
    for item in &others {
        if matches!(raw_pos(item), Some("right" | "center")) && drawn(Some(item), display) {
            if let Some((x, _)) = rect(item, display) {
                if x >= notch.estart && x < right_limit {
                    right_limit = x;
                }
            }
        }
    }
    let mut user_e = 0.0;
    for item in &others {
        if raw_pos(item) == Some("e") && drawn(Some(item), display) {
            if let Some((_, w)) = rect(item, display) {
                user_e += w + gap_full;
            }
        }
    }
    let mut tail = 0.0;
    for key in ["showy_quota.stale", "showy_quota.degraded"] {
        if let Some(item) = by.get(key).copied() {
            if drawn(Some(item), display) {
                if let Some((_, w)) = rect(item, display) {
                    tail += w + 6.0;
                }
            }
        }
    }
    let overflow_w = by
        .get("showy_quota.overflow")
        .copied()
        .filter(|item| drawn(Some(item), display))
        .and_then(|item| rect(item, display))
        .map_or(28.0, |(_, w)| w);

    let room_left = left_limit - settings.margin - start;
    let room_right = right_limit - settings.margin - notch.estart - user_e;
    let count = rows.len();
    let left_need = |widths: &[f64], gap: f64, k: usize| -> f64 {
        widths[..k].iter().sum::<f64>()
            + (k as f64 - 1.0) * gap
            + if k == count { tail } else { 0.0 }
    };
    let right_need = |widths: &[f64], gap: f64, k: usize| -> f64 {
        if k == count {
            0.0
        } else {
            gap + widths[k..].iter().sum::<f64>() + (count as f64 - k as f64 - 1.0) * gap + tail
        }
    };
    // Widest split both wings afford (providers `[0, k)` stay left).
    let best = |widths: &[f64], gap: f64| -> Option<usize> {
        let mut found: Option<usize> = None;
        for k in 1..=count {
            if left_need(widths, gap, k) <= room_left && right_need(widths, gap, k) <= room_right {
                found = Some(k);
            }
        }
        found
    };
    let ids = |from: usize, to: usize| -> Vec<String> {
        rows[from..to].iter().map(|row| row.id.clone()).collect()
    };
    let measured = |compact: bool, right: Vec<String>, hidden: Vec<String>, overflow: usize| Plan {
        notch: true,
        compact,
        right,
        hidden,
        overflow,
        meas: Some((label_extra, gap_full, gap_compact)),
    };

    if let Some(kf) = best(&full_widths, gap_full) {
        return measured(false, ids(kf, count), Vec::new(), 0);
    }
    if let Some(kc) = best(&compact_widths, gap_compact) {
        return measured(true, ids(kc, count), Vec::new(), 0);
    }
    // Nothing fits even without labels: keep the widest prefix on the left
    // (always at least one provider), fill the right wing, and summarize the
    // rest in the "+N" item. Never draw under the notch.
    let mut k = 1;
    for candidate in 1..=count {
        if compact_widths[..candidate].iter().sum::<f64>() + (candidate as f64 - 1.0) * gap_compact
            <= room_left
        {
            k = candidate;
        }
    }
    let mut m = 0;
    for candidate in 0..=(count - k) {
        if gap_compact
            + compact_widths[k..k + candidate].iter().sum::<f64>()
            + candidate as f64 * gap_compact
            + overflow_w
            + tail
            <= room_right
        {
            m = candidate;
        }
    }
    measured(true, ids(k, k + m), ids(k + m, count), count - k - m)
}

/// Whole numbers print without a fraction (35, not 35.0); other numbers print
/// as serde_json does.
fn fmt_num(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 9.0e15 {
        // The range guard keeps the float-to-int cast exact.
        format!("{}", value as i64)
    } else if let Some(number) = serde_json::Number::from_f64(value) {
        number.to_string()
    } else {
        String::from("null")
    }
}

fn fmt_strs(items: &[String]) -> String {
    let mut out = String::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(item).unwrap_or_else(|_| String::from("\"\"")));
    }
    out
}

fn render_plan(plan: &Plan) -> String {
    let mut json = format!(
        "{{\"notch\":{},\"compact\":{},\"right\":[{}],\"hidden\":[{}],\"overflow\":{}",
        plan.notch,
        plan.compact,
        fmt_strs(&plan.right),
        fmt_strs(&plan.hidden),
        plan.overflow,
    );
    if let Some((label_extra, gap_full, gap_compact)) = plan.meas {
        json.push_str(&format!(
            ",\"label_extra\":{},\"gap_full\":{},\"gap_compact\":{}}}",
            fmt_num(label_extra),
            fmt_num(gap_full),
            fmt_num(gap_compact),
        ));
    } else {
        json.push('}');
    }
    json
}

fn provider_regex(id: &str) -> String {
    crate::sketchybar_frame::provider_item_regex(id)
}

/// Port of the `sketchybar` argument loop in `apply_notch_layout`, in exact
/// order: per provider (in render order) the position move plus the
/// hide/shrink follow-up, then the overflow, stale, and degraded tail items.
fn apply_args(providers: &[String], plan: &Plan) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut tail = "left";
    for id in providers {
        let east = plan.right.contains(id) || plan.hidden.contains(id);
        let position = if east { "e" } else { "left" };
        if east {
            tail = "e";
        }
        let regex = provider_regex(id);
        args.push(String::from("--set"));
        args.push(regex.clone());
        args.push(format!("position={position}"));
        // Shrink in the same batch as the move, so nothing sits under the
        // notch while the next render catches up.
        if plan.hidden.contains(id) {
            args.push(String::from("--set"));
            args.push(regex);
            args.push(String::from("drawing=off"));
        } else if plan.compact {
            args.push(String::from("--set"));
            args.push(format!("showy_quota.{id}.label"));
            args.push(String::from("drawing=off"));
        }
    }
    if plan.overflow > 0 {
        tail = "e";
    }
    args.push(String::from("--set"));
    args.push(String::from("showy_quota.overflow"));
    args.push(format!("position={tail}"));
    if plan.overflow > 0 {
        args.push(String::from("--set"));
        args.push(String::from("showy_quota.overflow"));
        args.push(String::from("drawing=on"));
        args.push(format!("label=+{}", plan.overflow));
    } else {
        args.push(String::from("--set"));
        args.push(String::from("showy_quota.overflow"));
        args.push(String::from("drawing=off"));
    }
    args.push(String::from("--set"));
    args.push(String::from("showy_quota.stale"));
    args.push(format!("position={tail}"));
    args.push(String::from("--set"));
    args.push(String::from("showy_quota.degraded"));
    args.push(format!("position={tail}"));
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic item mirroring `notch_snapshot` in test/render_test.sh:
    /// hidden items park at x=-9999 with drawing off.
    fn item(name: &str, pos: &str, x: f64, w: f64, on: bool) -> Value {
        let (origin_x, drawing) = if on { (x, "on") } else { (-9999.0, "off") };
        serde_json::json!({
            "name": name,
            "geometry": {"position": pos, "drawing": drawing},
            "bounding_rects": {"display-1": {"origin": [origin_x, 0], "size": [w, 32]}},
        })
    }

    /// Synthetic batched query: bar, displays, notch anchors, a right-side
    /// neighbour, then provider icon/slot/label triples laid left-to-right
    /// from x=51 (145pt pitch with labels, 110pt without).
    fn snapshot(ids: &[&str], rx: f64, qx: f64, ex: f64, labels: bool) -> Vec<u8> {
        let mut docs: Vec<Value> = vec![
            serde_json::json!({"padding_right": 18, "items": []}),
            serde_json::json!([{"arrangement-id": 1,
                "frame": {"x": 0, "y": 0, "w": 1728, "h": 1117}}]),
            item("showy_quota.notch_q", "q", qx, 1.0, true),
            item("showy_quota.notch_e", "e", ex, 1.0, true),
            item("clock", "right", rx, 60.0, true),
        ];
        let pitch = if labels { 145.0 } else { 110.0 };
        for (index, id) in ids.iter().enumerate() {
            let x = 51.0 + pitch * index as f64;
            docs.push(item(
                &format!("showy_quota.{id}.icon"),
                "left",
                x,
                22.0,
                true,
            ));
            docs.push(item(
                &format!("showy_quota.{id}.slot"),
                "left",
                x + 19.0,
                83.0,
                true,
            ));
            docs.push(item(
                &format!("showy_quota.{id}.label"),
                "left",
                x + 105.0,
                32.0,
                labels,
            ));
        }
        let mut out = String::new();
        for doc in &docs {
            out.push_str(&serde_json::to_string(doc).unwrap());
            out.push('\n');
        }
        out.into_bytes()
    }

    fn ids(count: usize) -> Vec<String> {
        (1..=count).map(|i| format!("p{i}")).collect()
    }

    fn str_ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_string()).collect()
    }

    fn settings() -> NotchSettings {
        NotchSettings {
            margin: 4.0,
            icon_padding_left: 5.0,
            label_width: 32.0,
            icon_width: 22.0,
            bar_width: 83.0,
        }
    }

    fn carried(label_extra: f64, gap_full: f64, gap_compact: f64) -> PreviousPlan {
        PreviousPlan {
            valid: true,
            compact: false,
            hidden: Vec::new(),
            label_extra: Some(label_extra),
            gap_full: Some(gap_full),
            gap_compact: Some(gap_compact),
        }
    }

    fn plan_of(
        count: usize,
        previous: &PreviousPlan,
        rx: f64,
        qx: f64,
        ex: f64,
        labels: bool,
    ) -> NotchLayout {
        let provider_ids = ids(count);
        let names: Vec<&str> = (1..=count)
            .map(|i| match i {
                1 => "p1",
                2 => "p2",
                3 => "p3",
                4 => "p4",
                5 => "p5",
                6 => "p6",
                7 => "p7",
                8 => "p8",
                9 => "p9",
                _ => "p10",
            })
            .collect();
        notch_layout(
            &snapshot(&names, rx, qx, ex, labels),
            &provider_ids,
            &settings(),
            previous,
        )
        .expect("planner returns a plan")
    }

    #[test]
    fn keeps_pill_left_when_it_ends_before_notch() {
        let layout = plan_of(4, &carried(35.0, 8.0, 8.0), 1324.0, 763.0, 964.0, true);
        assert_eq!(
            layout.plan_json,
            "{\"notch\":true,\"compact\":false,\"right\":[],\"hidden\":[],\
             \"overflow\":0,\"label_extra\":35,\"gap_full\":8,\"gap_compact\":8}",
        );
    }

    #[test]
    fn right_wing_stops_at_the_display_edge_without_a_neighbour() {
        // A neighbour off the right edge must not widen the right wing past
        // the display: the plan equals one with the neighbour at the edge
        // (display width 1728 less the bar's 18pt right padding).
        let previous = carried(35.0, 8.0, 8.0);
        let offscreen = plan_of(10, &previous, 5000.0, 763.0, 964.0, true);
        let at_edge = plan_of(10, &previous, 1710.0, 763.0, 964.0, true);
        assert_eq!(offscreen.plan_json, at_edge.plan_json);
        assert!(offscreen.overflow > 0 || offscreen.compact);
    }

    #[test]
    fn a_reply_cut_before_the_anchors_counts_as_dropped() {
        let reply = b"{\"items\":[]}\n[]\n";
        assert!(notch_layout(reply, &ids(2), &settings(), &PreviousPlan::default()).is_none());
    }
    #[test]
    fn moves_crossing_providers_right() {
        let layout = plan_of(6, &carried(35.0, 8.0, 8.0), 1324.0, 763.0, 964.0, true);
        assert_eq!(
            layout.plan_json,
            "{\"notch\":true,\"compact\":false,\"right\":[\"p5\",\"p6\"],\"hidden\":[],\
             \"overflow\":0,\"label_extra\":35,\"gap_full\":8,\"gap_compact\":8}",
        );
        assert!(!layout.compact);
        assert_eq!(layout.right, str_ids(&["p5", "p6"]));
        assert!(layout.hidden.is_empty());
        assert_eq!(layout.overflow, 0);
    }

    #[test]
    fn hides_labels_before_right_wing_hits_neighbour() {
        let layout = plan_of(8, &carried(35.0, 8.0, 8.0), 1200.0, 763.0, 964.0, true);
        assert_eq!(
            layout.plan_json,
            "{\"notch\":true,\"compact\":true,\"right\":[\"p7\",\"p8\"],\"hidden\":[],\
             \"overflow\":0,\"label_extra\":35,\"gap_full\":8,\"gap_compact\":8}",
        );
    }

    #[test]
    fn summarizes_unfittable_providers_as_overflow() {
        let layout = plan_of(10, &carried(35.0, 8.0, 8.0), 1200.0, 763.0, 964.0, true);
        assert_eq!(
            layout.plan_json,
            "{\"notch\":true,\"compact\":true,\"right\":[\"p7\"],\
             \"hidden\":[\"p8\",\"p9\",\"p10\"],\"overflow\":3,\
             \"label_extra\":35,\"gap_full\":8,\"gap_compact\":8}",
        );
    }

    #[test]
    fn leaves_display_without_gap_on_left() {
        let layout = plan_of(6, &carried(35.0, 8.0, 8.0), 1324.0, 863.0, 864.0, true);
        assert_eq!(
            layout.plan_json,
            "{\"notch\":false,\"compact\":false,\"right\":[],\"hidden\":[],\"overflow\":0}",
        );
    }

    #[test]
    fn rebuilds_full_widths_from_carried_measurements() {
        // Labels are hidden, so the full width comes from the carried
        // label_extra (35) while the gap still measures live (8).
        let layout = plan_of(6, &carried(35.0, 8.0, 99.0), 1324.0, 763.0, 964.0, false);
        assert_eq!(
            layout.plan_json,
            "{\"notch\":true,\"compact\":false,\"right\":[\"p5\",\"p6\"],\"hidden\":[],\
             \"overflow\":0,\"label_extra\":35,\"gap_full\":8,\"gap_compact\":8}",
        );
    }

    #[test]
    fn measures_label_width_and_gap_when_labels_show() {
        // Bogus carried values (1, 1) are replaced by live measurements.
        let layout = plan_of(6, &carried(1.0, 1.0, 8.0), 1324.0, 763.0, 964.0, true);
        assert_eq!(
            layout.plan_json,
            "{\"notch\":true,\"compact\":false,\"right\":[\"p5\",\"p6\"],\"hidden\":[],\
             \"overflow\":0,\"label_extra\":35,\"gap_full\":8,\"gap_compact\":8}",
        );
    }

    #[test]
    fn empty_or_whitespace_measured_returns_none() {
        let providers = ids(2);
        assert!(notch_layout(b"", &providers, &settings(), &PreviousPlan::default()).is_none());
        assert!(notch_layout(
            b"  \n  \n",
            &providers,
            &settings(),
            &PreviousPlan::default()
        )
        .is_none());
    }

    #[test]
    fn leading_garbage_measured_returns_none() {
        let providers = ids(2);
        assert!(notch_layout(
            b"not json",
            &providers,
            &settings(),
            &PreviousPlan::default()
        )
        .is_none());
    }

    #[test]
    fn truncated_stream_without_anchors_counts_as_dropped() {
        // The bar parses, then garbage ends the stream before any anchor.
        let measured = b"{\"padding_right\": 18, \"items\": []}\n[1,2";
        assert!(notch_layout(measured, &ids(2), &settings(), &PreviousPlan::default()).is_none());
    }

    #[test]
    fn empty_providers_keep_left_placement() {
        let measured = snapshot(&["p1"], 1324.0, 763.0, 964.0, true);
        let layout =
            notch_layout(&measured, &[], &settings(), &PreviousPlan::default()).expect("plan");
        assert!(!layout.compact);
        assert!(layout.right.is_empty());
        assert!(!layout.reveal);
    }

    #[test]
    fn parse_previous_plan_rejects_non_objects() {
        for raw in [
            "",
            "   ",
            "[]",
            "[1]",
            "null",
            "42",
            "\"x\"",
            "not json {",
            "{\"a\":",
        ] {
            let plan = parse_previous_plan(raw);
            assert_eq!(plan, PreviousPlan::default(), "input {raw:?}");
            assert!(!plan.valid, "input {raw:?}");
        }
    }

    #[test]
    fn parse_previous_plan_reads_valid_object() {
        let plan = parse_previous_plan(
            "{\"compact\":true,\"hidden\":[\"p8\",\"p9\"],\
             \"label_extra\":35,\"gap_full\":8,\"gap_compact\":99}",
        );
        assert_eq!(
            plan,
            PreviousPlan {
                valid: true,
                compact: true,
                hidden: str_ids(&["p8", "p9"]),
                label_extra: Some(35.0),
                gap_full: Some(8.0),
                gap_compact: Some(99.0),
            }
        );
    }

    #[test]
    fn parse_previous_plan_drops_wrong_types() {
        let plan = parse_previous_plan(
            "{\"compact\":\"yes\",\"hidden\":[\"a\",1,null,\"b\"],\
             \"label_extra\":\"35\",\"gap_full\":8.5}",
        );
        assert!(plan.valid);
        assert!(!plan.compact);
        assert_eq!(plan.hidden, str_ids(&["a", "b"]));
        assert_eq!(plan.label_extra, None);
        assert_eq!(plan.gap_full, Some(8.5));
        assert_eq!(plan.gap_compact, None);
    }

    #[test]
    fn reveal_when_compact_plan_returns_to_full() {
        let previous = PreviousPlan {
            valid: true,
            compact: true,
            ..PreviousPlan::default()
        };
        let layout = plan_of(4, &previous, 1324.0, 763.0, 964.0, true);
        assert!(!layout.compact);
        assert!(layout.reveal);
    }

    #[test]
    fn apply_args_park_overflow_tail_left_when_empty() {
        let layout = plan_of(4, &carried(35.0, 8.0, 8.0), 1324.0, 763.0, 964.0, true);
        assert_eq!(layout.overflow, 0);
        let tail = layout.args[layout.args.len() - 12..].to_vec();
        assert_eq!(
            tail,
            [
                "--set",
                "showy_quota.overflow",
                "position=left",
                "--set",
                "showy_quota.overflow",
                "drawing=off",
                "--set",
                "showy_quota.stale",
                "position=left",
                "--set",
                "showy_quota.degraded",
                "position=left",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn reveal_when_provider_leaves_hidden() {
        let previous = PreviousPlan {
            valid: true,
            compact: true,
            hidden: str_ids(&["p9"]),
            ..PreviousPlan::default()
        };
        // Eight providers fit with labels hidden and nothing hidden.
        let layout = plan_of(8, &previous, 1200.0, 763.0, 964.0, true);
        assert!(layout.hidden.is_empty());
        assert!(layout.reveal);
    }

    #[test]
    fn no_reveal_when_plan_hides_more() {
        // p8 stays hidden, so nothing is revealed even though the plan is new.
        let previous = PreviousPlan {
            valid: true,
            hidden: str_ids(&["p8"]),
            ..PreviousPlan::default()
        };
        let layout = plan_of(10, &previous, 1200.0, 763.0, 964.0, true);
        assert_eq!(layout.hidden, str_ids(&["p8", "p9", "p10"]));
        assert!(!layout.reveal);
    }

    #[test]
    fn no_reveal_for_steady_full_plan() {
        let layout = plan_of(4, &PreviousPlan::default(), 1324.0, 763.0, 964.0, true);
        assert!(!layout.reveal);
    }

    #[test]
    fn apply_args_cover_right_hidden_compact_overflow() {
        let layout = plan_of(10, &carried(35.0, 8.0, 8.0), 1200.0, 763.0, 964.0, true);
        assert!(layout.compact);
        assert_eq!(layout.right, str_ids(&["p7"]));
        assert_eq!(layout.hidden, str_ids(&["p8", "p9", "p10"]));
        assert_eq!(layout.overflow, 3);

        let mut expected: Vec<String> = Vec::new();
        for id in ["p1", "p2", "p3", "p4", "p5", "p6", "p7"] {
            expected.push(String::from("--set"));
            expected.push(format!("/^showy_quota\\.{id}\\.[^.]*$/"));
            expected.push(String::from(if id == "p7" {
                "position=e"
            } else {
                "position=left"
            }));
            expected.push(String::from("--set"));
            expected.push(format!("showy_quota.{id}.label"));
            expected.push(String::from("drawing=off"));
        }
        for id in ["p8", "p9", "p10"] {
            expected.push(String::from("--set"));
            expected.push(format!("/^showy_quota\\.{id}\\.[^.]*$/"));
            expected.push(String::from("position=e"));
            expected.push(String::from("--set"));
            expected.push(format!("/^showy_quota\\.{id}\\.[^.]*$/"));
            expected.push(String::from("drawing=off"));
        }
        for arg in [
            "--set",
            "showy_quota.overflow",
            "position=e",
            "--set",
            "showy_quota.overflow",
            "drawing=on",
            "label=+3",
            "--set",
            "showy_quota.stale",
            "position=e",
            "--set",
            "showy_quota.degraded",
            "position=e",
        ] {
            expected.push(String::from(arg));
        }
        assert_eq!(layout.args, expected);
    }

    #[test]
    fn apply_args_escape_dots_in_provider_id() {
        let providers = str_ids(&["a.b"]);
        let layout = notch_layout(
            &snapshot(&["a.b"], 1324.0, 763.0, 964.0, true),
            &providers,
            &settings(),
            &carried(35.0, 8.0, 8.0),
        )
        .expect("plan");
        assert_eq!(
            layout.args,
            [
                "--set",
                "/^showy_quota\\.a\\.b\\.[^.]*$/",
                "position=left",
                "--set",
                "showy_quota.overflow",
                "position=left",
                "--set",
                "showy_quota.overflow",
                "drawing=off",
                "--set",
                "showy_quota.stale",
                "position=left",
                "--set",
                "showy_quota.degraded",
                "position=left",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>(),
        );
    }
}
