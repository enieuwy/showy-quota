#!/usr/bin/env bash
# showy-quota — SketchyBar bootstrap.
#
# Source from sketchybarrc with:   source "$ITEM_DIR/showy_quota.sh"
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
        cd -- "${dir}/../../.." && pwd -P
    }
    REPO_ROOT="$(resolve_repo_root)"

    if [[ -z "${SHOWY_QUOTA_SKETCHYBAR_PILL_RADIUS:-}" && -n "${PILL_RADIUS:-}" ]]; then
        export SHOWY_QUOTA_SKETCHYBAR_PILL_RADIUS="${PILL_RADIUS}"
    fi
    if [[ -z "${SHOWY_QUOTA_SKETCHYBAR_PILL_HEIGHT:-}" && -n "${PILL_HEIGHT:-}" ]]; then
        export SHOWY_QUOTA_SKETCHYBAR_PILL_HEIGHT="${PILL_HEIGHT}"
    fi

    # shellcheck disable=SC1091
    . "${REPO_ROOT}/lib/common.sh"
    # shellcheck disable=SC1091
    . "${REPO_ROOT}/lib/strip.sh"

    # The plugin runs as a child process now, so export every showy-quota knob the
    # user may have set in sketchybarrc before sourcing this bootstrap file.
    while IFS= read -r cb_var; do
        declare -gx "${cb_var}"
    done < <(compgen -A variable SHOWY_QUOTA_)

    PLUGIN_PATH="${REPO_ROOT}/adapters/sketchybar/plugins/showy_quota.sh"
    [[ -x "${PLUGIN_PATH}" ]] || PLUGIN_PATH="${PLUGIN_DIR:-}/showy_quota.sh"
    [[ -n "${PLUGIN_PATH}" && -r "${PLUGIN_PATH}" ]] || exit 0

    sketchybar --add item showy_quota.trigger left \
               --set showy_quota.trigger \
                   drawing=off \
                   updates=on \
                   update_freq="${SHOWY_QUOTA_SKETCHYBAR_UPDATE_FREQ}" \
                   script="${PLUGIN_PATH}"

    SHOWY_QUOTA_SKETCHYBAR_FORCE_REDECLARE=1 "${PLUGIN_PATH}"
) || true
