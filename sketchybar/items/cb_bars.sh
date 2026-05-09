#!/usr/bin/env bash
# codexbar-bars — SketchyBar item declarations.
#
# Defines one icon + bar + label triple per provider returned by
# `cb-bars-fetch`. The bar PNG and provider icon PNG are generated lazily
# by plugins/cb_bars.sh; this file only declares the items and registers
# the plugin trigger.
#
# Source from sketchybarrc with:   source "$ITEM_DIR/cb_bars.sh"

set -euo pipefail

# Resolve our shared lib. The repo's lib/ ships next to bin/. When the
# scripts are symlinked into ~/.config/sketchybar/items, follow the link
# to find the lib directory.
resolve_repo_root() {
    local self="${BASH_SOURCE[0]}"
    if [[ -L "${self}" ]]; then
        self=$(readlink "${self}")
    fi
    local dir
    dir=$(cd -- "$(dirname -- "${self}")" && pwd -P)
    # ${dir}/../.. brings us from sketchybar/items/ to repo root.
    cd -- "${dir}/../.." && pwd -P
}
REPO_ROOT="$(resolve_repo_root)"

# shellcheck disable=SC1091
. "${REPO_ROOT}/lib/common.sh"

# shellcheck disable=SC2317   # both branches reachable depending on source-vs-exec
cb_bars_have jq || { printf 'cb_bars: jq required\n' >&2; return 0 2>/dev/null || exit 0; }

PLUGIN_PATH="${REPO_ROOT}/sketchybar/plugins/cb_bars.sh"
[[ -x "${PLUGIN_PATH}" ]] || PLUGIN_PATH="${PLUGIN_DIR:-}/cb_bars.sh"

CLICK="${CB_BARS_SKETCHYBAR_CLICK}"

# Pull provider list from cache (or live fetch on first run).
data=$("${REPO_ROOT}/bin/cb-bars-fetch" 2>/dev/null || printf '[]')
providers=$(printf '%s' "${data}" \
    | jq -r '[ .[] | select((.error // null) == null and (.usage.primary // null) != null) | .provider ] | .[]')

if [[ -z "${providers}" ]]; then
    # shellcheck disable=SC2317
    return 0 2>/dev/null || exit 0
fi

# Trigger item: invisible, just runs the plugin on a timer to refresh all
# provider items. SketchyBar evaluates this once per UPDATE_FREQ.
sketchybar --add item cb_bars.trigger left \
           --set cb_bars.trigger \
               drawing=off \
               updates=on \
               update_freq="${CB_BARS_SKETCHYBAR_UPDATE_FREQ}" \
               script="${PLUGIN_PATH}"

bracket_items=""
for pid in ${providers}; do
    sketchybar --add item "cb_bars.${pid}.icon" left \
               --set "cb_bars.${pid}.icon" \
                   icon.drawing=off \
                   label.drawing=off \
                   background.image="cb_bars.${pid}.icon" \
                   background.image.scale=0.6 \
                   background.image.padding_left=2 \
                   background.image.padding_right=2 \
                   click_script="${CLICK}"

    sketchybar --add item "cb_bars.${pid}.bar" left \
               --set "cb_bars.${pid}.bar" \
                   icon.drawing=off \
                   label.drawing=off \
                   background.image="cb_bars.${pid}.bar" \
                   background.image.padding_left=2 \
                   background.image.padding_right=2 \
                   click_script="${CLICK}"

    sketchybar --add item "cb_bars.${pid}.label" left \
               --set "cb_bars.${pid}.label" \
                   icon.drawing=off \
                   label.font.size=11 \
                   label.padding_left=0 \
                   label.padding_right=4 \
                   click_script="${CLICK}"

    bracket_items="${bracket_items} cb_bars.${pid}.icon cb_bars.${pid}.bar cb_bars.${pid}.label"
done

# Wrap the trio in a single pill so it visually cohabits with other
# SketchyBar brackets in the user's existing config.
# bracket_items intentionally word-splits — sketchybar wants distinct args.
# shellcheck disable=SC2086
sketchybar --add bracket cb_bars_bracket ${bracket_items} \
           --set cb_bars_bracket \
               background.color=0xcc24273a \
               background.corner_radius="${PILL_RADIUS:-14}" \
               background.height="${PILL_HEIGHT:-28}"
