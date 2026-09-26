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

# The parent becomes part of a temp path and a sketchybar argument; only the
# unit-id charset the renderer emits may pass.
case "${parent}" in
    "" | *[!A-Za-z0-9_.-]* | .* | *..*) exit 0 ;;
esac

state="${TMPDIR:-/tmp}/showy-quota-hover.${parent}"

readonly PID_SPACE=100000
readonly HOVER_RACE_SECONDS=2
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
    mouse.entered)
        claim_hover_state "in" "$$" || exit 0
        "${SB}" --set "${parent}" popup.drawing=on >/dev/null 2>&1
        ;;
    mouse.exited | mouse.exited.global)
        claim_hover_state "out" "$$" || exit 0
        sleep 0.35
        [[ $(cat "${state}" 2>/dev/null) == "${claimed}" ]] \
            && "${SB}" --set "${parent}" popup.drawing=off >/dev/null 2>&1
        ;;
esac
