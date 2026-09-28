#!/usr/bin/env bash
# showy-quota — ring-mode popup script. Two modes, chosen by the plugin's
# subscriptions (SHOWY_QUOTA_SKETCHYBAR_POPUP):
#
# click (default): every item of a unit runs this with the unit's ring item
# as $1 on `mouse.clicked`. A left click opens that unit's popup and closes
# any other; a second left click closes it. One hidden closer item runs this
# with `--close-all` on `mouse.exited.global`, which fires only when the
# pointer leaves both the bar and its popups. Moving the pointer starts no
# process except that one closer.
#
# hover: every item of a unit runs this on `mouse.entered mouse.exited
# mouse.exited.global`. SketchyBar runs one process per event, so the exit of
# one unit item can land after the entry into its neighbour (a ring and its
# pace overlay overlap). An entry waits HOVER_DWELL_SECONDS and opens only if
# no later event happened, so a pointer passing over the strip on its way to
# a window's top row opens nothing. An exit waits 0.35 s and closes only if
# no entry happened since.
#
# SketchyBar spawns handlers in event order, and a pid is fixed at fork, so
# pid order is spawn order. A delayed older event must never overwrite a
# newer one: each writer claims the state file only when no newer event holds
# it, and a refused claim means a newer event already decided, so the writer
# exits without touching the popup. Writes go through a temp file plus rename
# so a concurrent reader never sees a half-written state.
#
# Pids wrap (macOS: at 99999), so a raw numeric compare goes wrong after a
# wrap and a stale large pid would refuse every later claim forever. Two
# guards: the compare is modular, and a recorded event only blocks for
# HOVER_RACE_SECONDS. Racing handlers spawn within milliseconds; an older
# record has no claim left to protect.

set -uo pipefail

SB="${SKETCHYBAR:-sketchybar}"
parent="${1:-}"
readonly RING_POPUPS='/^showy_quota\..*\.ring$/'
# Click mode: which unit's popup a click opened, so the closer calls
# sketchybar only when a popup is open.
open_file="${TMPDIR:-/tmp}/showy-quota-popup.open"

if [[ "${parent}" == "--close-all" ]]; then
    [[ -e "${open_file}" ]] || exit 0
    rm -f -- "${open_file}" 2>/dev/null
    "${SB}" --set "${RING_POPUPS}" popup.drawing=off >/dev/null 2>&1
    exit 0
fi

# The parent becomes part of a temp path and a sketchybar argument; only the
# unit-id charset the renderer emits may pass.
case "${parent}" in
    "" | *[!A-Za-z0-9_.-]* | .* | *..*) exit 0 ;;
esac

state="${TMPDIR:-/tmp}/showy-quota-hover.${parent}"

readonly PID_SPACE=100000
readonly HOVER_RACE_SECONDS=2
readonly HOVER_DWELL_SECONDS=0.4
now="${EPOCHSECONDS:-$(date +%s)}"

# Succeeds when pid $1 was spawned after pid $2, modulo the pid wrap.
pid_is_newer() {
    local d=$(( (10#$1 - 10#$2 + PID_SPACE) % PID_SPACE ))
    (( d > 0 && d < PID_SPACE / 2 ))
}

# Claim the state file for (`word`, pid): refuse only when a recent record
# (within HOVER_RACE_SECONDS) holds a newer pid. Absent, malformed, legacy
# two-field, or stale records are overwritten. Returns 1 on refusal, in which
# case the caller must leave the popup alone. On success, `claimed` holds the
# written record.
claim_hover_state() {
    local word="$1" pid="$2" current rec_word rec_pid rec_time extra
    current=$(cat "${state}" 2>/dev/null) || current=""
    read -r rec_word rec_pid rec_time extra <<< "${current}"
    case "${rec_word}:${extra}" in
        in: | out:)
            case "${rec_pid}:${rec_time}" in
                :* | *: | *[!0-9:]*) ;; # Malformed or legacy: overwrite below.
                *)
                    if (( now - 10#${rec_time} <= HOVER_RACE_SECONDS )) \
                        && pid_is_newer "${rec_pid}" "${pid}"; then
                        return 1
                    fi
                    ;;
            esac
            ;;
    esac
    claimed="${word} ${pid} ${now}"
    local tmp
    tmp=$(mktemp "${state}.XXXXXX" 2>/dev/null) || return 1
    printf '%s' "${claimed}" > "${tmp}" 2>/dev/null || {
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
    mouse.clicked)
        # Right click runs the item's click_script (open CodexBar); only a
        # left click toggles the popup.
        [[ "${BUTTON:-left}" == "left" ]] || exit 0
        if [[ "$(cat "${open_file}" 2>/dev/null)" == "${parent}" ]]; then
            rm -f -- "${open_file}" 2>/dev/null
            "${SB}" --set "${parent}" popup.drawing=off >/dev/null 2>&1
        else
            printf '%s' "${parent}" > "${open_file}" 2>/dev/null
            "${SB}" --set "${RING_POPUPS}" popup.drawing=off \
                --set "${parent}" popup.drawing=on >/dev/null 2>&1
        fi
        ;;
    mouse.entered)
        claim_hover_state "in" "$$" || exit 0
        sleep "${HOVER_DWELL_SECONDS}"
        [[ $(cat "${state}" 2>/dev/null) == "${claimed}" ]] \
            && "${SB}" --set "${parent}" popup.drawing=on >/dev/null 2>&1
        ;;
    mouse.exited | mouse.exited.global)
        claim_hover_state "out" "$$" || exit 0
        sleep 0.35
        [[ $(cat "${state}" 2>/dev/null) == "${claimed}" ]] \
            && "${SB}" --set "${parent}" popup.drawing=off >/dev/null 2>&1
        ;;
esac
