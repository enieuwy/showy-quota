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

set -uo pipefail

SB="${SKETCHYBAR:-sketchybar}"
parent="${1:-}"

# The parent becomes part of a temp path and a sketchybar argument; only the
# unit-id charset the renderer emits may pass.
case "${parent}" in
    "" | *[!A-Za-z0-9_.-]* | .* | *..*) exit 0 ;;
esac

state="${TMPDIR:-/tmp}/showy-quota-hover.${parent}"
case "${SENDER:-}" in
    mouse.entered)
        printf 'in %s' "$$" > "${state}"
        "${SB}" --set "${parent}" popup.drawing=on >/dev/null 2>&1
        ;;
    mouse.exited | mouse.exited.global)
        printf 'out %s' "$$" > "${state}"
        sleep 0.35
        [[ $(cat "${state}" 2>/dev/null) == "out $$" ]] \
            && "${SB}" --set "${parent}" popup.drawing=off >/dev/null 2>&1
        ;;
esac
