#!/usr/bin/env bash
# showy-quota — ring-mode hover script: show the unit's popup while the
# pointer is over any item of the unit.
#
# Every item of a ring unit (ring, pace overlay, bars, label) runs this with
# the unit's ring item as $1 on `mouse.entered mouse.exited
# mouse.exited.global`. SketchyBar runs one process per event, so the exit of
# one unit item can land after the entry into its neighbour (a ring and its
# pace overlay overlap). An exit waits 0.35 s and closes only if no entry
# happened since.
#
# SketchyBar spawns handlers in event order, so a larger pid means a newer
# event. A delayed older event must never overwrite a newer one: each writer
# claims the state file only when the recorded pid is its own or older, and
# a refused claim means a newer event already decided, so the writer exits
# without touching the popup. Writes go through a temp file plus rename so a
# concurrent reader never sees a half-written state.

set -uo pipefail

SB="${SKETCHYBAR:-sketchybar}"
parent="${1:-}"

# The parent becomes part of a temp path and a sketchybar argument; only the
# unit-id charset the renderer emits may pass.
case "${parent}" in
    "" | *[!A-Za-z0-9_.-]* | .* | *..*) exit 0 ;;
esac

state="${TMPDIR:-/tmp}/showy-quota-hover.${parent}"

# Claim the state file for (`word`, pid): overwrite only when the recorded
# event is absent, malformed, or older-or-equal. Returns 1 when a newer event
# already claimed it, in which case the caller must leave the popup alone.
claim_hover_state() {
    local word="$1" pid="$2" current recorded
    current=$(cat "${state}" 2>/dev/null) || current=""
    case "${current}" in
        in\ * | out\ *)
            recorded="${current#* }"
            case "${recorded}" in
                "" | *[!0-9]*) ;; # Foreign junk: overwrite below.
                # Numeric compare: a larger pid is the newer event.
                *) (( 10#${recorded} > 10#${pid} )) && return 1 ;;
            esac
            ;;
        "") ;;
        *) ;;
    esac
    local tmp
    tmp=$(mktemp "${state}.XXXXXX" 2>/dev/null) || return 1
    printf '%s %s' "${word}" "${pid}" > "${tmp}" 2>/dev/null || {
        rm -f -- "${tmp}" 2>/dev/null
        return 1
    }
    mv -f -- "${tmp}" "${state}" 2>/dev/null || {
        rm -f -- "${tmp}" 2>/dev/null
        return 1
    }
    return 0
}

case "${SENDER:-}" in
    mouse.entered)
        claim_hover_state "in" "$$" || exit 0
        "${SB}" --set "${parent}" popup.drawing=on >/dev/null 2>&1
        ;;
    mouse.exited | mouse.exited.global)
        claim_hover_state "out" "$$" || exit 0
        sleep 0.35
        [[ $(cat "${state}" 2>/dev/null) == "out $$" ]] \
            && "${SB}" --set "${parent}" popup.drawing=off >/dev/null 2>&1
        ;;
esac
