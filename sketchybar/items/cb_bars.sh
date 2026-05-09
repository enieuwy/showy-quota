#!/usr/bin/env bash
# codexbar-bars — SketchyBar item declarations.
#
# Defines one icon + bar + label triple per provider returned by
# `cb-bars-fetch`. The bar PNG and provider icon PNG are generated lazily
# by plugins/cb_bars.sh; this file only declares the items and registers
# the plugin trigger.
#
# Source from sketchybarrc with:   source "$ITEM_DIR/cb_bars.sh"
#
# IMPORTANT: this file is sourced from the user's sketchybarrc. We MUST
# NOT leak `set -euo pipefail` to the caller — a later non-critical
# sketchybar command failure or unset variable in the user's existing
# config would otherwise abort the whole bar reload. Every operation
# below runs in a subshell.

(
    set -euo pipefail

    # Resolve our shared lib. The repo's lib/ ships next to bin/. When
    # the scripts are symlinked into ~/.config/sketchybar/items, follow
    # the link chain to find the lib directory.
    resolve_repo_root() {
        local self="${BASH_SOURCE[0]}"
        # Resolve symlinks iteratively (handles relative + chained links).
        while [[ -L "${self}" ]]; do
            local link
            link=$(readlink "${self}")
            if [[ "${link}" == /* ]]; then
                self="${link}"
            else
                self="$(cd -- "$(dirname -- "${self}")" && pwd -P)/${link}"
            fi
        done
        local dir
        dir=$(cd -- "$(dirname -- "${self}")" && pwd -P)
        cd -- "${dir}/../.." && pwd -P
    }
    REPO_ROOT="$(resolve_repo_root)"

    # shellcheck disable=SC1091
    . "${REPO_ROOT}/lib/common.sh"
    # shellcheck disable=SC1091
    . "${REPO_ROOT}/lib/strip.sh"

    cb_bars_have jq || { printf 'cb_bars: jq required\n' >&2; exit 0; }

    PLUGIN_PATH="${REPO_ROOT}/sketchybar/plugins/cb_bars.sh"
    [[ -x "${PLUGIN_PATH}" ]] || PLUGIN_PATH="${PLUGIN_DIR:-}/cb_bars.sh"

    CLICK="${CB_BARS_SKETCHYBAR_CLICK}"
    FETCH="${CB_BARS_FETCH_BIN:-${REPO_ROOT}/bin/cb-bars-fetch}"

    # Pull provider list from cache (or live fetch on first run).
    data=$("${FETCH}" 2>/dev/null || printf '[]')
    providers=$(printf '%s' "${data}" \
        | cb_bars_filter_renderable \
        | jq -r '.[].provider')

    [[ -n "${providers}" ]] || exit 0

    # Trigger item: invisible, just runs the plugin on a timer to
    # refresh all provider items. SketchyBar evaluates this once per
    # UPDATE_FREQ.
    sketchybar --add item cb_bars.trigger left \
               --set cb_bars.trigger \
                   drawing=off \
                   updates=on \
                   update_freq="${CB_BARS_SKETCHYBAR_UPDATE_FREQ}" \
                   script="${PLUGIN_PATH}"

    bracket_items=()
    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        sketchybar --add item "cb_bars.${pid}.icon" left \
                   --set "cb_bars.${pid}.icon" \
                       icon.drawing=off \
                       label.drawing=off \
                       background.image="cb_bars.${pid}.icon" \
                       background.image.drawing=on \
                       background.image.scale=0.6 \
                       background.color=0x00000000 \
                       background.height=0 \
                       padding_left=6 \
                       padding_right=0 \
                       width="${CB_BARS_SKETCHYBAR_ICON_WIDTH}" \
                       click_script="${CLICK}"

        sketchybar --add item "cb_bars.${pid}.bar" left \
                   --set "cb_bars.${pid}.bar" \
                       icon.drawing=off \
                       label.drawing=off \
                       background.image="cb_bars.${pid}.bar" \
                       background.image.drawing=on \
                       background.image.scale=1.0 \
                       background.color=0x00000000 \
                       background.height=0 \
                       padding_left=2 \
                       padding_right=2 \
                       width="${CB_BARS_SKETCHYBAR_BAR_WIDTH}" \
                       click_script="${CLICK}"

        sketchybar --add item "cb_bars.${pid}.label" left \
                   --set "cb_bars.${pid}.label" \
                       icon.drawing=off \
                       label.font.size=11 \
                       label.padding_left=0 \
                       label.padding_right=4 \
                       background.color=0x00000000 \
                       background.height=0 \
                       click_script="${CLICK}"

        bracket_items+=("cb_bars.${pid}.icon" "cb_bars.${pid}.bar" "cb_bars.${pid}.label")
    done <<< "${providers}"

    # Wrap the trio in a single pill so it visually cohabits with other
    # SketchyBar brackets in the user's existing config.
    sketchybar --add bracket cb_bars_bracket "${bracket_items[@]}" \
               --set cb_bars_bracket \
                   background.color=0xcc24273a \
                   background.corner_radius="${PILL_RADIUS:-14}" \
                   background.height="${PILL_HEIGHT:-28}"
) || true
