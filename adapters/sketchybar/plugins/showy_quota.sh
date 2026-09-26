#!/usr/bin/env bash
# showy-quota — SketchyBar plugin: render per-provider icon + native usage
# slider rows, and update each provider's items.
#
# Invoked by the showy_quota.trigger item every SHOWY_QUOTA_SKETCHYBAR_UPDATE_FREQ
# seconds. Reads the shared codexbar JSON cache, lazily caches provider icons,
# and updates SketchyBar-native slider rows for each usage window.

set +e
set -uo pipefail

# When this script is symlinked into the user's plugins dir, follow the
# chain to the original repo. Iterates because dotfile managers commonly
# create relative or chained symlinks.
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

# shellcheck disable=SC1091
. "${REPO_ROOT}/lib/common.sh"
# shellcheck disable=SC1091
. "${REPO_ROOT}/lib/strip.sh"

DEFAULT_CLICK="open -b com.steipete.codexbar"

click_command_is_safe() {
    local click="${1:-}"

    case "${click}" in
        *';'*|*'|'*|*'&'*|*'`'*|*'$'*|*'('*|*')'*|*'<'*|*'>'*)
            return 1
            ;;
    esac
    [[ ! "${click}" =~ [[:cntrl:]] ]]
}

validated_codexbar_resources() {
    local resources="${SHOWY_QUOTA_CODEXBAR_RESOURCES:-}" resolved

    [[ "${resources}" == /* && ${#resources} -le 1024 ]] || return 1
    [[ ! "${resources}" =~ [[:cntrl:]] ]] || return 1
    [[ -d "${resources}" && -r "${resources}" ]] || return 1
    if command -v realpath >/dev/null 2>&1; then
        resolved=$(realpath "${resources}" 2>/dev/null) || return 1
    else
        resolved=$(cd -- "${resources}" && pwd -P) || return 1
    fi
    [[ "${resolved}" == /* && -d "${resolved}" && -r "${resolved}" ]] || return 1
    [[ ! "${resolved}" =~ [[:cntrl:]] ]] || return 1
    printf '%s\n' "${resolved}"
}

FETCH="${SHOWY_QUOTA_FETCH_BIN:-${REPO_ROOT}/bin/showy-quota-fetch}"
FETCH="$(showy_quota_valid_bin "${FETCH}")" || FETCH="${REPO_ROOT}/bin/showy-quota-fetch"
CACHE_DIR="${SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE}"
if [[ ! -d "${CACHE_DIR}" ]]; then
    mkdir -p -- "${CACHE_DIR}" || exit 0
    chmod 700 "${CACHE_DIR}" 2>/dev/null || true
fi
STATE_FILE="${CACHE_DIR}/providers.txt"
# What this plugin last sent to SketchyBar, one hash per provider; the
# renderer diffs each tick against it and writes the next one.
FRAME_FILE="${CACHE_DIR}/frame.txt"
# A layout event that found a render in flight, or a plan that got no reply,
# leaves this note so the next render re-plans instead of the event being lost.
LAYOUT_PENDING_FILE="${CACHE_DIR}/layout.pending"
NOTCH_PLAN_FILE="${CACHE_DIR}/notch-layout.json"
NOTCH_ANCHORS=(showy_quota.notch_q showy_quota.notch_e)
# Every item one provider owns, in bracket order. Removal and the bracket walk
# this list; the renderer checks the same list (`PROVIDER_ITEM_ROLES` in
# crates/showy-quota-zellij-core/src/sketchybar_frame.rs) to find lost items.
PROVIDER_ITEM_ROLES=(icon primary secondary tertiary quaternary
    secondary_marker tertiary_marker quaternary_marker primary_marker slot label)
# Every item one ring unit owns; the strip items first, in bracket order. The renderer checks the
# same list (`RING_UNIT_ROLES` in
# crates/showy-quota-zellij-core/src/sketchybar_frame.rs) to find lost items.
RING_UNIT_ITEM_ROLES=(ring ring_pace bar0 bar0_pace bar1 bar1_pace label
    pop_title pop_header pop_row0 pop_row1 pop_row2 pop_note pop_alert0 pop_alert1)
# Ring geometry (points), chosen from the design spikes and mirrored by the
# renderer's ring frame: 26 pt ring, 3 pt stroke, 12 pt logos, 28x4 bars with
# a 5 pt ring gap, 22 pt between providers (10 pt between Antigravity pools),
# 10 pt pill edges. The ring item is RING_BADGE_ROOM wider than the ring, on
# its left, so the banked-reset badge can sit off the ring without clipping;
# the spacer before each unit gives that room back, so the strip keeps its size.
RING_DIAMETER=26
# The pace tick's item: 28 pt with a 6 pt stroke sits centred on the ring's
# 3 pt stroke (the renderer sends the same ring.width and line_width).
RING_PACE_DIAMETER=28
RING_BADGE_ROOM=7
RING_BAR_W=28
RING_EDGE=10
RING_PROVIDER_GAP=22
RING_POOL_GAP=10
BODY_FILE="${CACHE_DIR}/body.txt"
RING_PROBE_OK="${CACHE_DIR}/ring-capable"
RING_FALLBACK_LOGGED="${CACHE_DIR}/ring-fallback-logged"
# The hover script lives next to this plugin (or its installed copy).
HOVER_SCRIPT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)/showy_quota_hover.sh"
[[ -r "${HOVER_SCRIPT}" ]] || HOVER_SCRIPT="${REPO_ROOT}/adapters/sketchybar/plugins/showy_quota_hover.sh"
if click_command_is_safe "${SHOWY_QUOTA_SKETCHYBAR_CLICK}"; then
    CLICK="${SHOWY_QUOTA_SKETCHYBAR_CLICK}"
else
    CLICK="${DEFAULT_CLICK}"
fi
# CODEXBAR_RESOURCES and ICON_FONT_FILE feed only icon rasterization, which a
# tick with every icon cached never reaches; provider_icon_png resolves them
# on first use.
ICON_SOURCES_RESOLVED=0
CODEXBAR_RESOURCES=""
ICON_FONT_FILE=""
RENDER_LOCK_DIR="${CACHE_DIR}/render.lock"
RENDER_LOCK_OWNER="${RENDER_LOCK_DIR}/owner.pid"
# Temp icon files awaiting cleanup by the EXIT trap (release_render_lock).
# INVARIANT: only ever appended to from the current shell. Registering a temp
# inside a command substitution or a piped loop body would push it onto a
# subshell's copy of this array, and the trap would never see it — which is
# exactly why provider_icon_png returns its path via a caller-named variable
# rather than on stdout.
ICON_TMP_FILES=()

cleanup_icon_tmp_files() {
    ((${#ICON_TMP_FILES[@]} == 0)) && return 0
    rm -f -- "${ICON_TMP_FILES[@]}" 2>/dev/null || true
}


render_lock_age_seconds() {
    local now mtime
    now=$(showy_quota_now_epoch)
    if mtime=$(stat -f %m "${RENDER_LOCK_DIR}" 2>/dev/null) \
        || mtime=$(stat -c %Y "${RENDER_LOCK_DIR}" 2>/dev/null); then
        printf '%s\n' $((now - mtime))
        return 0
    fi
    return 1
}
# `lstart` follows the caller's locale (`Fri 25 Sep …` under en_AU, `Fri Sep
# 25 …` under C). A launchd-started SketchyBar runs the plugin without LANG,
# so a run from a shell with a locale read a live owner's start time as a
# reused pid and stole its lock. Pin the format.
render_process_start_time() {
    local pid="$1" start_time

    [[ "${pid}" =~ ^[0-9]+$ ]] || return 1
    start_time=$(LC_ALL=C ps -p "${pid}" -o lstart= 2>/dev/null) || return 1
    [[ -n "${start_time}" ]] || return 1
    printf '%s\n' "${start_time}"
}

render_lock_age_exceeds() {
    local max_age="$1" lock_age

    lock_age=$(render_lock_age_seconds) || return 1
    (( lock_age >= max_age || lock_age <= -max_age ))
}

read_render_lock_owner() {
    local owner_record

    RENDER_LOCK_OWNER_PID=""
    RENDER_LOCK_OWNER_START_TIME=""
    [[ -r "${RENDER_LOCK_OWNER}" ]] || return 1
    IFS= read -r owner_record < "${RENDER_LOCK_OWNER}" || return 1
    IFS=$'\t' read -r RENDER_LOCK_OWNER_PID RENDER_LOCK_OWNER_START_TIME <<< "${owner_record}"
    [[ "${RENDER_LOCK_OWNER_PID}" =~ ^[0-9]+$ ]]
}

reclaim_render_lock() {
    rm -f -- "${RENDER_LOCK_OWNER}"
    rmdir -- "${RENDER_LOCK_DIR}" 2>/dev/null
}

write_render_lock_owner() {
    local start_time=""

    start_time=$(render_process_start_time "$$" || true)
    printf '%s\t%s\n' "$$" "${start_time}" > "${RENDER_LOCK_OWNER}"
}


release_render_lock() {
    cleanup_icon_tmp_files
    if read_render_lock_owner && [[ "${RENDER_LOCK_OWNER_PID}" == "$$" ]]; then
        rm -f -- "${RENDER_LOCK_OWNER}"
        rmdir -- "${RENDER_LOCK_DIR}" 2>/dev/null || true
    fi
}

acquire_render_lock() {
    local owner_pid owner_start_time current_start_time
    local max_ownerless_age=30 max_render_age=300 malformed_owner attempt=0 max_attempts=3

    while (( attempt < max_attempts )); do
        if mkdir -- "${RENDER_LOCK_DIR}" 2>/dev/null; then
            write_render_lock_owner || {
                rmdir -- "${RENDER_LOCK_DIR}" 2>/dev/null || true
                return 1
            }
            trap release_render_lock EXIT
            return 0
        fi

        owner_pid=""
        owner_start_time=""
        current_start_time=""
        malformed_owner=0
        if read_render_lock_owner; then
            owner_pid="${RENDER_LOCK_OWNER_PID}"
            owner_start_time="${RENDER_LOCK_OWNER_START_TIME}"
            if kill -0 "${owner_pid}" 2>/dev/null; then
                current_start_time=$(render_process_start_time "${owner_pid}" || true)
                if [[ -n "${owner_start_time}" && -n "${current_start_time}" \
                    && "${owner_start_time}" != "${current_start_time}" ]]; then
                    showy_quota_log "reclaiming sketchybar render lock from reused pid ${owner_pid}"
                elif render_lock_age_exceeds "${max_render_age}"; then
                    showy_quota_log "reclaiming expired sketchybar render lock (pid ${owner_pid})"
                else
                    showy_quota_log "sketchybar render already in flight (pid ${owner_pid}); skipping"
                    return 1
                fi
            else
                showy_quota_log "reclaiming stale sketchybar render lock (pid ${owner_pid})"
            fi

            if ! reclaim_render_lock; then
                return 1
            fi
            ((attempt += 1))
            (( attempt < max_attempts )) && sleep "0.$((attempt * 5))"
            continue
        elif [[ -r "${RENDER_LOCK_OWNER}" ]]; then
            malformed_owner=1
        fi

        # A future-dated lock mtime (clock skew, hand-touched dir) yields a
        # negative age that would never satisfy the threshold and wedge rendering
        # forever; judge staleness by absolute distance from now instead.
        if render_lock_age_exceeds "${max_ownerless_age}"; then
            if (( malformed_owner )); then
                rm -f -- "${RENDER_LOCK_OWNER}"
            fi
            if ! rmdir -- "${RENDER_LOCK_DIR}" 2>/dev/null; then
                return 1
            fi
        else
            showy_quota_log "sketchybar render lock is ownerless; skipping"
            return 1
        fi

        ((attempt += 1))
        (( attempt < max_attempts )) && sleep "0.$((attempt * 5))"
    done

    showy_quota_log "sketchybar render lock retry limit reached; skipping"
    return 1
}

# Redraw as soon as the fetch lands instead of on the next timer tick: a
# slow fetch otherwise leaves the aged frame (possibly grey, stale) up for
# one more UPDATE_FREQ. Only a new cache triggers: a fetch that merely waited
# on another fetch's lock also exits 0, and triggering then would start a
# run that sees the same aged cache and forks yet another fetch.
start_background_refresh() {
    (
        mtime() { stat -f %m -- "$1" 2>/dev/null || stat -c %Y -- "$1" 2>/dev/null; }
        before=$(mtime "${SHOWY_QUOTA_USAGE_FILE}") || before=""
        "${FETCH}" </dev/null >/dev/null 2>&1 || true
        after=$(mtime "${SHOWY_QUOTA_USAGE_FILE}") || after=""
        [[ -n "${after}" && "${after}" != "${before}" ]] \
            && sketchybar --trigger showy_quota_refresh >/dev/null 2>&1
    ) </dev/null >/dev/null 2>&1 &
    disown "$!" 2>/dev/null || true
}


# Fill STATE_PROVIDERS with the provider list the last redeclare wrote.
# Assigns a global rather than printing, so callers need no subshell.
load_state_providers() {
    local pid
    STATE_PROVIDERS=""
    [[ -f "${STATE_FILE}" ]] || return 0
    while IFS= read -r pid || [[ -n "${pid}" ]]; do
        [[ -n "${pid}" ]] || continue
        STATE_PROVIDERS+="${STATE_PROVIDERS:+$'\n'}${pid}"
    done < "${STATE_FILE}"
}

provider_list_contains() {
    local list="${1-}" pid="$2"
    case $'\n'"${list}"$'\n' in
        *$'\n'"${pid}"$'\n'*) return 0 ;;
        *) return 1 ;;
    esac
}

write_state_providers() {
    local providers="${1-}" state_tmp mv_status
    state_tmp=$(mktemp "${CACHE_DIR}/.providers.XXXXXX") || return 1
    trap 'rm -f -- "${state_tmp}"; trap - HUP INT TERM; exit 129' HUP
    trap 'rm -f -- "${state_tmp}"; trap - HUP INT TERM; exit 130' INT
    trap 'rm -f -- "${state_tmp}"; trap - HUP INT TERM; exit 143' TERM
    if [[ -n "${providers}" ]]; then
        printf '%s\n' "${providers}" > "${state_tmp}"
    else
        : > "${state_tmp}"
    fi
    mv -f "${state_tmp}" "${STATE_FILE}"
    mv_status=$?
    if (( mv_status != 0 )); then
        rm -f -- "${state_tmp}"
    fi
    trap - HUP INT TERM
    return "${mv_status}"
}

# Declarations queue into SB_QUEUE and go out as one `sketchybar` call.
# One call per item made a full rebuild about 70 spawns long, and SketchyBar
# drops a reply that takes over 100 ms, so under load a rebuild ran for most
# of a minute with the pacing markers off. SketchyBar carries on past a
# failed command inside a batch (`--remove` of an absent item), so one call
# keeps the old per-call `|| true` semantics.
SB_QUEUE=()

flush_sketchybar_queue() {
    (( ${#SB_QUEUE[@]} > 0 )) || return 0
    sketchybar "${SB_QUEUE[@]}" >/dev/null 2>&1 || true
    SB_QUEUE=()
}

queue_provider_removal() {
    local pid="$1" role
    for role in "${PROVIDER_ITEM_ROLES[@]}"; do
        SB_QUEUE+=(--remove "showy_quota.${pid}.${role}")
    done
}

# A usage row slider. Extra leading properties (`drawing=off` for the rows a
# two-window provider does not draw) come before the shared ones.
queue_row_slider() {
    local name="showy_quota.$1.$2"
    shift 2
    SB_QUEUE+=(--add slider "${name}" left "${SHOWY_QUOTA_PNG_BAR_W}"
               --set "${name}" "$@"
                   slider.percentage=0
                   slider.highlight_color=0x00000000
                   slider.background.color="${TRACK_ARGB}"
                   slider.background.height="${NATIVE_ROW_HEIGHT}"
                   slider.background.corner_radius="${NATIVE_ROW_RADIUS}"
                   slider.knob.drawing=off
                   icon.drawing=off
                   label.drawing=off
                   background.color=0x00000000
                   background.height=0
                   padding_left=0
                   padding_right=0
                   width=0
                   click_script="${CLICK}")
}

queue_marker_slider() {
    local name="showy_quota.$1.$2_marker"
    SB_QUEUE+=(--add slider "${name}" left "${SHOWY_QUOTA_PNG_BAR_W}"
               --set "${name}"
                   drawing=off
                   slider.percentage=0
                   slider.highlight_color=0x00000000
                   slider.background.color=0x00000000
                   slider.background.height="${NATIVE_ROW_HEIGHT}"
                   slider.background.corner_radius=0
                   slider.knob.drawing=on
                   slider.knob.color=0x00000000
                   slider.knob.width=1
                   slider.knob.padding_left=0
                   slider.knob.padding_right=0
                   slider.knob.background.drawing=on
                   slider.knob.background.color="${ELAPSED_ARGB}"
                   slider.knob.background.height="${NATIVE_ROW_HEIGHT}"
                   slider.knob.background.corner_radius=0
                   icon.drawing=off
                   label.drawing=off
                   background.color=0x00000000
                   background.height=0
                   padding_left=0
                   padding_right=0
                   width=0
                   click_script="${CLICK}")
}

queue_provider_declaration() {
    local pid="$1" role
    queue_provider_removal "${pid}"

    SB_QUEUE+=(--add item "showy_quota.${pid}.icon" left
               --set "showy_quota.${pid}.icon"
                   icon.drawing=off
                   label.drawing=off
                   background.image.drawing=off
                   background.image.scale="${SHOWY_QUOTA_SKETCHYBAR_ICON_SCALE}"
                   background.color=0x00000000
                   background.height=0
                   padding_left="${SHOWY_QUOTA_SKETCHYBAR_ICON_PADDING_LEFT}"
                   padding_right=0
                   width="${SHOWY_QUOTA_SKETCHYBAR_ICON_WIDTH}"
                   click_script="${CLICK}")

    queue_row_slider "${pid}" primary
    queue_row_slider "${pid}" secondary
    queue_row_slider "${pid}" tertiary drawing=off
    queue_row_slider "${pid}" quaternary drawing=off

    for role in secondary primary tertiary quaternary; do
        queue_marker_slider "${pid}" "${role}"
    done

    SB_QUEUE+=(--add item "showy_quota.${pid}.slot" left
               --set "showy_quota.${pid}.slot"
                   icon.drawing=off
                   label.drawing=off
                   background.color=0x00000000
                   background.height=0
                   padding_left=0
                   padding_right=0
                   width="${SHOWY_QUOTA_SKETCHYBAR_BAR_WIDTH}"
                   click_script="${CLICK}"
               --add item "showy_quota.${pid}.label" left
               --set "showy_quota.${pid}.label"
                   icon.drawing=off
                   label.font.size=11
                   label.padding_left=0
                   label.padding_right=4
                   label.width="${SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH}"
                   label.align=left
                   background.color=0x00000000
                   background.height=0
                   click_script="${CLICK}")
}

# The trailing overflow/stale/degraded items and the bracket that spans every
# provider item plus those three.
queue_bracket() {
    local providers="${1-}" pid role
    local -a members=()

    SB_QUEUE+=(--remove showy_quota_bracket
               --remove showy_quota.overflow
               --add item showy_quota.overflow left
               --set showy_quota.overflow
                   drawing=off
                   label="+0"
                   label.font.size=11
                   label.color="${COUNTDOWN_WARN_ARGB}"
                   icon.drawing=off
                   background.color=0x00000000
                   background.height=0
                   padding_left=4
                   padding_right=4
                   click_script="${CLICK}"
               --remove showy_quota.stale
               --add item showy_quota.stale left
               --set showy_quota.stale
                   drawing=off
                   label="${SHOWY_QUOTA_STALE_GLYPH}"
                   label.color="${COUNTDOWN_WARN_ARGB}"
                   icon.drawing=off
                   background.color=0x00000000
                   background.height=0
                   padding_left=4
                   padding_right=2
                   click_script="${CLICK}"
               --remove showy_quota.degraded
               --add item showy_quota.degraded left
               --set showy_quota.degraded
                   drawing=off
                   label="${SHOWY_QUOTA_DEGRADED_CLI_GLYPH}"
                   label.color="${COUNTDOWN_WARN_ARGB}"
                   icon.drawing=off
                   background.color=0x00000000
                   background.height=0
                   padding_left=2
                   padding_right=4
                   click_script="${CLICK}")

    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        for role in "${PROVIDER_ITEM_ROLES[@]}"; do
            members+=("showy_quota.${pid}.${role}")
        done
    done <<< "${providers}"
    (( ${#members[@]} > 0 )) || return 0

    members+=(showy_quota.overflow showy_quota.stale showy_quota.degraded)
    SB_QUEUE+=(--add bracket showy_quota_bracket "${members[@]}"
               --set showy_quota_bracket
                   background.color="${SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR}"
                   background.corner_radius="${SHOWY_QUOTA_SKETCHYBAR_PILL_RADIUS}"
                   background.height="${SHOWY_QUOTA_SKETCHYBAR_PILL_HEIGHT}")
}

# Two 1pt anchors mark the notch gap exactly as SketchyBar reserves it: the
# `q` anchor ends at the notch's left edge, the `e` anchor starts at its right
# edge. They must precede the provider items in SketchyBar's item list, because
# `e` items lay out in list order and the planner measures the right wing from
# the `e` anchor.
queue_notch_anchors() {
    local anchor position
    for anchor in "${NOTCH_ANCHORS[@]}"; do
        position=q
        [[ "${anchor}" == *_e ]] && position=e
        SB_QUEUE+=(--remove "${anchor}"
                   --add item "${anchor}" "${position}"
                   --set "${anchor}"
                       width=1
                       icon.drawing=off
                       label.drawing=off
                       background.drawing=off
                       padding_left=0
                       padding_right=0
                       updates=off)
    done
}

queue_notch_anchor_removal() {
    local anchor
    for anchor in "${NOTCH_ANCHORS[@]}"; do
        SB_QUEUE+=(--remove "${anchor}")
    done
}

# ── ring body ────────────────────────────────────────────────────

# Identity of the SketchyBar daemon(s) the `sketchybar` client talks to:
# one `pid + start time + command` line per process. The start time pins the
# identity across pid reuse, and the command tells the fork (which implements
# the ring item) from stock. `lstart` follows the caller's locale and `ps`
# pads its columns to the caller's width (the render lock pins `LC_ALL=C`
# for the same reason), so the locale is pinned and whitespace squeezed:
# without that the same daemon reads back different identities tick to tick
# and the marker below never trusts. Assigns SKETCHYBAR_DAEMON_IDENTITY,
# empty when the daemons cannot be listed (then the probe marker falls back
# to age alone). Callers need no subshell.
sketchybar_daemon_identity() {
    SKETCHYBAR_DAEMON_IDENTITY=""
    local pids pid line identity=""
    pids=$(pgrep -x sketchybar 2>/dev/null) || return 0
    [[ -n "${pids}" ]] || return 0
    for pid in ${pids}; do
        [[ "${pid}" =~ ^[0-9]+$ ]] || return 0
        line=$(LC_ALL=C ps -p "${pid}" -o lstart= -o command= 2>/dev/null | tr -s '[:space:]' ' ') || return 0
        [[ -n "${line}" ]] || return 0
        identity+="${identity:+$'\n'}${pid} ${line}"
    done
    SKETCHYBAR_DAEMON_IDENTITY="${identity}"
}

# Effective strip body for this tick: rows, unless ring was requested AND the
# running SketchyBar implements the ring item (the fork
# github.com/enieuwy/SketchyBar). A stock SketchyBar accepts `--add ring` by
# creating a generic item, so the exit code proves nothing: the probe item is
# queried back and must report `"type": "ring"`. Probes at most once an hour
# per running daemon (the marker stores the daemon identity, so swapping the
# bar's binary re-probes); on stock SketchyBar logs once and falls back to
# rows. Assigns EFFECTIVE_BODY.
resolve_effective_body() {
    EFFECTIVE_BODY=rows
    [[ "${SHOWY_QUOTA_SKETCHYBAR_BODY:-rows}" == "ring" ]] || return 0
    local probe_age=999999 stored_identity=""
    sketchybar_daemon_identity
    if [[ -f "${RING_PROBE_OK}" ]]; then
        probe_age=$(showy_quota_age_seconds "${RING_PROBE_OK}" 2>/dev/null) || probe_age=999999
        # `cat`, not `$(< ... 2>/dev/null)`: the fast path returns empty
        # when a redirect is attached (observed on Bash 5.2), which would
        # re-probe every tick.
        stored_identity=$(cat "${RING_PROBE_OK}" 2>/dev/null) || stored_identity=""
        if (( probe_age < 3600 )) && [[ "${stored_identity}" == "${SKETCHYBAR_DAEMON_IDENTITY}" ]]; then
            EFFECTIVE_BODY=ring
            return 0
        fi
    fi
    # One probe item per process: the layout path probes outside the render
    # lock, and with one shared name a concurrent run's `--add` failed
    # ("already exists") and its cleanup removed the other run's probe, so
    # both fell back to rows and the strip flashed rows for a tick or more.
    local probe="showy_quota.ring_probe.$$"
    if sketchybar --add ring "${probe}" left "${RING_DIAMETER}" >/dev/null 2>&1 \
        && sketchybar --query "${probe}" 2>/dev/null \
            | grep -q -E '"type"[[:space:]]*:[[:space:]]*"ring"'; then
        sketchybar --remove "${probe}" >/dev/null 2>&1 || true
        printf '%s' "${SKETCHYBAR_DAEMON_IDENTITY}" > "${RING_PROBE_OK}" 2>/dev/null || true
        rm -f -- "${RING_FALLBACK_LOGGED}" 2>/dev/null || true
        EFFECTIVE_BODY=ring
    else
        # Stock SketchyBar keeps the generic item the failed probe created.
        sketchybar --remove "${probe}" >/dev/null 2>&1 || true
        rm -f -- "${RING_PROBE_OK}" 2>/dev/null || true
        if [[ ! -f "${RING_FALLBACK_LOGGED}" ]]; then
            showy_quota_log "SHOWY_QUOTA_SKETCHYBAR_BODY=ring needs the SketchyBar fork (ring item); this bar has none, falling back to rows"
            : > "${RING_FALLBACK_LOGGED}" 2>/dev/null || true
        fi
        EFFECTIVE_BODY=rows
    fi
}

# Logo for a ring unit's provider: the app-font glyph centred in the ring
# (pad 5 at 12 pt, mirroring the renderer's app_font_pad), the measured SF
# Pro glyphs for Muse / Command Code, else the provider sigil.
ring_logo_for_provider() {
    local pid="$1"
    RING_LOGO_GLYPH=""
    RING_LOGO_FONT="SF Pro:Bold:12.0"
    RING_LOGO_PAD=0
    RING_LOGO_Y=0
    case "${pid}" in
        muse)
            RING_LOGO_GLYPH="∞"
            RING_LOGO_FONT="SF Pro:Bold:13.0"
            RING_LOGO_Y=1
            return 0
            ;;
        commandcode)
            RING_LOGO_GLYPH="⌘"
            RING_LOGO_FONT="SF Pro:Bold:11.0"
            RING_LOGO_PAD=1
            return 0
            ;;
    esac
    if [[ -n "${SHOWY_QUOTA_PROVIDER_FONT_ICONS[${pid}]:-}" ]]; then
        RING_LOGO_GLYPH="${SHOWY_QUOTA_PROVIDER_FONT_ICONS[${pid}]}"
        RING_LOGO_FONT="sketchybar-app-font:Regular:12.0"
        RING_LOGO_PAD=5
    else
        RING_LOGO_GLYPH="$(showy_quota_provider_sigil "${pid}")"
    fi
}

# Remove every item of one ring unit (bars, label, popup rows included).
queue_ring_unit_removal() {
    local unit="$1"
    SB_QUEUE+=(--remove "/^showy_quota\.${unit//./\\.}\..*$/"
               --remove "showy_quota.gap.${unit}")
}

# Declare one ring unit. Static geometry and the hover subscription live
# here; the per-tick frame owns every dynamic value. The subscription must
# share the `--add` call: SketchyBar only creates the mouse tracking area
# while the item draws, so a subscription from a later call gets no events
# until the item redraws.
queue_ring_unit_declaration() {
    local unit="$1" pid="$2"
    local base="showy_quota.${unit}" ring="showy_quota.${unit}.ring"
    local hover_script role item
    queue_ring_unit_removal "${unit}"
    ring_logo_for_provider "${pid}"
    # Pool units share the marker with their letter badge: 1.5 pt left of
    # centre, mirroring the renderer's app_pad - 3.
    if [[ "${unit}" != "${pid}" ]]; then
        RING_LOGO_PAD=$(( RING_LOGO_PAD > 3 ? RING_LOGO_PAD - 3 : 0 ))
    fi
    load_host_settings
    # The fork runs `script` via `sh -c`, so each word is single-quoted the
    # way the renderer quotes item names inside click scripts
    # (`shell_quote` in sketchybar_frame.rs): a plugin path with spaces must
    # stay one word.
    hover_script="'${HOVER_SCRIPT//\'/\'\\\'\'}' '${ring}'"
    local text_argb="0xff${ICON_TEXT_HEX}"
    local track="${TRACK_ARGB}"
    SB_QUEUE+=(--add ring "${ring}" left "${RING_DIAMETER}"
               --set "${ring}"
                   icon.drawing=off
                   label.drawing=off
                   background.drawing=off
                   padding_left=0
                   padding_right=0
                   width="$(( RING_DIAMETER + RING_BADGE_ROOM ))"
                   align=right
                   y_offset=0
                   ring.line_width=3
                   ring.cap=round
                   ring.marker="${RING_LOGO_GLYPH}"
                   ring.marker.drawing=on
                   ring.marker.position=center
                   ring.marker.font="${RING_LOGO_FONT}"
                   ring.marker.color="${text_argb}"
                   ring.marker.padding_left="${RING_LOGO_PAD}"
                   ring.marker.padding_right=0
                   ring.marker.y_offset="${RING_LOGO_Y}"
                   popup.align=left
                   popup.y_offset=4
                   popup.height=20
                   popup.blur_radius=30
                   popup.background.color=0xd0161616
                   popup.background.corner_radius=10
                   popup.background.border_width=1
                   popup.background.border_color="${track}"
                   popup.background.shadow.drawing=on
                   click_script="${CLICK}"
                   script="${hover_script}"
               --subscribe "${ring}" mouse.entered mouse.exited mouse.exited.global
               --add ring "${base}.ring_pace" left "${RING_PACE_DIAMETER}"
               --set "${base}.ring_pace"
                   icon.drawing=off
                   label.drawing=off
                   background.drawing=off
                   padding_left=-"$(( RING_DIAMETER + (RING_PACE_DIAMETER - RING_DIAMETER) / 2 ))"
                   padding_right=0
                   width=0
                   y_offset=0
                   ring.track_color=0x00000000
                   ring.cap=butt
                   ring.line_width=5
                   ring.marker.drawing=off
                   click_script="${CLICK}"
                   script="${hover_script}"
               --subscribe "${base}.ring_pace" mouse.entered mouse.exited mouse.exited.global)
    for role in bar0 bar1; do
        item="${base}.${role}"
        SB_QUEUE+=(--add slider "${item}" left "${RING_BAR_W}"
                   --set "${item}"
                       slider.percentage=0
                       slider.highlight_color=0x00000000
                       slider.background.color="${track}"
                       slider.background.height=4
                       slider.background.corner_radius=4
                       slider.knob.drawing=off
                       icon.drawing=off
                       label.drawing=off
                       background.color=0x00000000
                       background.height=0
                       padding_left=5
                       padding_right=0
                       width=0
                       click_script="${CLICK}"
                       script="${hover_script}"
                   --subscribe "${item}" mouse.entered mouse.exited mouse.exited.global)
        item="${base}.${role}_pace"
        SB_QUEUE+=(--add slider "${item}" left "${RING_BAR_W}"
                   --set "${item}"
                       slider.percentage=0
                       slider.highlight_color=0x00000000
                       slider.background.color=0x00000000
                       slider.background.height=4
                       slider.knob.drawing=on
                       slider.knob.color=0x00000000
                       slider.knob.width=2
                       slider.knob.padding_left=0
                       slider.knob.padding_right=0
                       slider.knob.background.drawing=on
                       slider.knob.background.color="${ELAPSED_ARGB}"
                       slider.knob.background.height=8
                       slider.knob.background.corner_radius=0
                       icon.drawing=off
                       label.drawing=off
                       background.color=0x00000000
                       background.height=0
                       padding_left=5
                       padding_right=0
                       width=0
                       click_script="${CLICK}"
                       script="${hover_script}"
                   --subscribe "${item}" mouse.entered mouse.exited mouse.exited.global)
    done
    SB_QUEUE+=(--add item "${base}.label" left
               --set "${base}.label"
                   icon.drawing=off
                   label.font="SF Pro:Semibold:10.0"
                   label.padding_left=0
                   label.padding_right=0
                   width="${RING_BAR_W}"
                   background.color=0x00000000
                   background.height=0
                   padding_left=5
                   padding_right=0
                   click_script="${CLICK}"
                   script="${hover_script}"
               --subscribe "${base}.label" mouse.entered mouse.exited mouse.exited.global)
    # Popup rows: fixed slots, so the declaration never depends on the data.
    # A popup lists its items in add order: title, alerts, header, the window
    # rows (row 0 is always the ring window, a 14 pt mini ring; rows 1–2 are
    # bars), then the note that explains the label.
    for role in pop_title pop_alert0 pop_alert1 pop_header; do
        SB_QUEUE+=(--add item "${base}.${role}" "popup.${ring}"
                   --set "${base}.${role}" drawing=off)
    done
    SB_QUEUE+=(--add ring "${base}.pop_row0" "popup.${ring}" 14
               --set "${base}.pop_row0" drawing=off
               --add slider "${base}.pop_row1" "popup.${ring}" 14
               --set "${base}.pop_row1" drawing=off
               --add slider "${base}.pop_row2" "popup.${ring}" 14
               --set "${base}.pop_row2" drawing=off
               --add item "${base}.pop_note" "popup.${ring}"
               --set "${base}.pop_note" drawing=off)
}

queue_ring_spacer() {
    SB_QUEUE+=(--remove "$1"
               --add item "$1" left
               --set "$1"
                   width="$2"
                   icon.drawing=off
                   label.drawing=off
                   background.drawing=off
                   padding_left=0
                   padding_right=0)
}

# The pill edges, the gaps, and the bracket over one ring strip. Stale and
# degraded keep their rows-body names and behaviour; overflow has no ring use.
queue_ring_bracket() {
    local units_list="$1" unit role
    local -a members=()
    SB_QUEUE+=(--remove showy_quota_bracket
               --remove showy_quota.overflow
               --remove showy_quota.edge.a
               --remove showy_quota.edge.z
               --remove showy_quota.stale
               --add item showy_quota.stale left
               --set showy_quota.stale
                   drawing=off
                   label="${SHOWY_QUOTA_STALE_GLYPH}"
                   label.color="${COUNTDOWN_WARN_ARGB}"
                   icon.drawing=off
                   background.color=0x00000000
                   background.height=0
                   padding_left=4
                   padding_right=2
                   click_script="${CLICK}"
               --remove showy_quota.degraded
               --add item showy_quota.degraded left
               --set showy_quota.degraded
                   drawing=off
                   label="${SHOWY_QUOTA_DEGRADED_CLI_GLYPH}"
                   label.color="${COUNTDOWN_WARN_ARGB}"
                   icon.drawing=off
                   background.color=0x00000000
                   background.height=0
                   padding_left=2
                   padding_right=4
                   click_script="${CLICK}")
    local first=1
    while IFS= read -r unit; do
        [[ -n "${unit}" ]] || continue
        if (( first )); then
            first=0
        else
            local gap_width="$(( RING_PROVIDER_GAP - RING_BADGE_ROOM ))"
            [[ "${unit}" == "antigravity.c" ]] && gap_width="$(( RING_POOL_GAP - RING_BADGE_ROOM ))"
            queue_ring_spacer "showy_quota.gap.${unit}" "${gap_width}"
            members+=("showy_quota.gap.${unit}")
        fi
        for role in "${RING_UNIT_ITEM_ROLES[@]}"; do
            # Popup rows live in the popup, not on the strip.
            [[ "${role}" == pop_* ]] && continue
            members+=("showy_quota.${unit}.${role}")
        done
    done <<< "${units_list}"
    (( ${#members[@]} > 0 )) || return 0
    queue_ring_spacer showy_quota.edge.a "$(( RING_EDGE - RING_BADGE_ROOM ))"
    queue_ring_spacer showy_quota.edge.z "${RING_EDGE}"
    members=("showy_quota.edge.a" "${members[@]}" showy_quota.stale showy_quota.degraded "showy_quota.edge.z")
    # SketchyBar appends every new left item to the end of the bar, and the
    # spacers above are added after the unit items: put the strip in order.
    SB_QUEUE+=(--reorder "${members[@]}")
    SB_QUEUE+=(--add bracket showy_quota_bracket "${members[@]}"
               --set showy_quota_bracket
                   background.color="${SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR}"
                   background.corner_radius="${SHOWY_QUOTA_SKETCHYBAR_PILL_RADIUS}"
                   background.height="${SHOWY_QUOTA_SKETCHYBAR_PILL_HEIGHT}")
}

# Drop every rows-body item by name pattern, not by provider list: a saved
# list cannot name a provider that left the filtered set (or a switch the
# previous tick never recorded), and those strays would linger outside the
# bracket forever. The fork's `--remove` regex has no alternation, so each
# role gets its own anchored pattern; every pattern starts with
# `showy_quota\.`, so foreign items (`battery.ring`, `bluetooth.ring`) can
# never match. A regex with no match is a harmless `[?]` note. Stale,
# degraded, and the bracket keep their rows-body names in ring mode too, so
# they are not cleared here. The `.label` pattern also matches ring unit
# labels; every caller redeclares the incoming body right after, which
# restores them.
clear_rows_body_items() {
    local role
    for role in "${PROVIDER_ITEM_ROLES[@]}"; do
        SB_QUEUE+=(--remove "/^showy_quota\\..*\\.${role}$/")
    done
    SB_QUEUE+=(--remove showy_quota.overflow)
    queue_notch_anchor_removal
}

# Drop every ring-body item by name pattern, for the same reason: an
# interrupted switch can leave ring items no saved list names. Covers the
# unit roles (`RING_UNIT_ITEM_ROLES`) plus the gap and edge spacers. The
# `.label` pattern also matches rows provider labels; every caller
# redeclares the incoming body right after, which restores them.
clear_ring_body_items() {
    local role
    for role in "${RING_UNIT_ITEM_ROLES[@]}"; do
        SB_QUEUE+=(--remove "/^showy_quota\\..*\\.${role}$/")
    done
    SB_QUEUE+=(--remove "/^showy_quota\\.gap\\..*$/"
               --remove "/^showy_quota\\.edge\\..*$/")
}

notch_placement() {
    [[ "${SHOWY_QUOTA_SKETCHYBAR_PLACEMENT}" == "notch" ]]
}

# `showy-quota-render --emit sketchybar-*` answers in records, one per line,
# fields separated by US and the first field a tag (the format is documented
# in crates/showy-quota-zellij-core/src/sketchybar_frame.rs). Read the fields
# after the tag of record $2 into the array named by $1.
wire_fields_into() {
    local _wf_rest=""
    [[ "$2" == *$'\x1f'* ]] && _wf_rest="${2#*$'\x1f'}"
    IFS=$'\x1f' read -r -a "$1" <<< "${_wf_rest}"
}

parse_frame_output() {
    local line
    local -a fields=()
    FRAME_REDECLARE="-"
    FRAME_REFRESH=0
    FRAME_PROVIDERS=""
    FRAME_UNITS=()
    FRAME_HAS_QUERY=0
    FRAME_QUERY=()
    FRAME_ICONS=()
    FRAME_ARGS=()
    while IFS= read -r line; do
        case "${line%%$'\x1f'*}" in
            state)
                wire_fields_into fields "${line}"
                FRAME_REDECLARE="${fields[0]:--}"
                FRAME_REFRESH="${fields[1]:-0}"
                ;;
            providers)
                wire_fields_into fields "${line}"
                printf -v FRAME_PROVIDERS '%s\n' "${fields[@]}"
                FRAME_PROVIDERS="${FRAME_PROVIDERS%$'\n'}"
                ;;
            icon) FRAME_ICONS+=("${line}") ;;
            units) wire_fields_into FRAME_UNITS "${line}" ;;
            query)
                wire_fields_into FRAME_QUERY "${line}"
                FRAME_HAS_QUERY=1
                ;;
            set) wire_fields_into FRAME_ARGS "${line}" ;;
        esac
    done <<< "$1"
}

trigger_provider_change() {
    local providers="${1-}" pid
    local provider_count=0 provider_csv=""

    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        if [[ -n "${provider_csv}" ]]; then
            provider_csv+=","
        fi
        provider_csv+="${pid}"
        provider_count=$((provider_count + 1))
    done <<< "${providers}"

    sketchybar --trigger showy_quota_provider_change \
        SHOWY_QUOTA_PROVIDER_COUNT="${provider_count}" \
        SHOWY_QUOTA_PROVIDERS="${provider_csv}" >/dev/null 2>&1 || true
}

clear_declared_items() {
    local pid
    load_state_providers
    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        queue_provider_removal "${pid}"
    done <<< "${STATE_PROVIDERS}"
    SB_QUEUE+=(--remove showy_quota_bracket
               --remove showy_quota.overflow
               --remove showy_quota.stale
               --remove showy_quota.degraded)
    queue_notch_anchor_removal
    flush_sketchybar_queue
    rm -f -- "${NOTCH_PLAN_FILE}" "${FRAME_FILE}" 2>/dev/null || true
    write_state_providers "" || showy_quota_log "failed to clear sketchybar provider state"
}

if ! RENDER_BIN="$(showy_quota_resolve_render_bin "${REPO_ROOT}/bin" "${REPO_ROOT}")"; then
    showy_quota_log "showy-quota-render required for sketchybar plugin; run make render-bin"
    acquire_render_lock || exit 0
    clear_declared_items
    exit 0
fi

HAVE_MAGICK=0
showy_quota_have magick && HAVE_MAGICK=1
HAVE_RSVG=0
showy_quota_have rsvg-convert && HAVE_RSVG=1

# Point ImageMagick at our restrictive policy.xml so a provider SVG cannot make
# `magick` fetch a remote href (SSRF). Prepend so it wins over system configs;
# guard on the file so a copied (non-repo-relative) install still renders.
if (( HAVE_MAGICK )) && [[ -f "${REPO_ROOT}/adapters/sketchybar/imagemagick/policy.xml" ]]; then
    export MAGICK_CONFIGURE_PATH="${REPO_ROOT}/adapters/sketchybar/imagemagick${MAGICK_CONFIGURE_PATH:+:${MAGICK_CONFIGURE_PATH}}"
fi

# ── host settings ────────────────────────────────────────────────────

# 6-char hex → '#RRGGBB' for ImageMagick.
mhex() { printf '#%s' "$1"; }

# Geometry and colors for declaring items and drawing icons. A tick that
# declares nothing and draws no icon never needs them, and each palette
# lookup forks, so they load on first use.
HOST_SETTINGS_LOADED=0
load_host_settings() {
    (( HOST_SETTINGS_LOADED )) && return 0
    : "${SHOWY_QUOTA_PNG_BAR_W:=80}"
    NATIVE_ROW_HEIGHT=6
    # Default 3 == NATIVE_ROW_HEIGHT/2 → fully rounded ends. Set to 0 for a
    # squared track; intermediate values yield partial rounding.
    NATIVE_ROW_RADIUS=$(showy_quota_uint "${SHOWY_QUOTA_SKETCHYBAR_ROW_RADIUS:-3}" 3 4096)
    PRIMARY_WARN_HEX="$(showy_quota_primary_palette warn)"
    PRIMARY_BAD_HEX="$(showy_quota_primary_palette bad)"
    PRIMARY_UNKNOWN_HEX="$(showy_quota_primary_palette unknown)"
    TRACK_ARGB="0xff$(showy_quota_palette track)"
    ICON_TEXT_HEX="$(showy_quota_palette icon_text)"
    COUNTDOWN_WARN_HEX="$(showy_quota_palette countdown_warn)"
    COUNTDOWN_WARN_ARGB="0xff${COUNTDOWN_WARN_HEX}"
    ELAPSED_ARGB="0xff$(showy_quota_palette elapsed)"
    HOST_SETTINGS_LOADED=1
}

status_color_for_indicator_into() {
    case "${2:-none}" in
        minor|maintenance) printf -v "$1" '%s' "${PRIMARY_WARN_HEX}" ;;
        major|critical)    printf -v "$1" '%s' "${PRIMARY_BAD_HEX}" ;;
        unknown)           printf -v "$1" '%s' "${PRIMARY_UNKNOWN_HEX}" ;;
        *)                 printf -v "$1" '%s' ""; return 1 ;;
    esac
}

# ── provider icon: rasterize SVG → PNG ───────────────────────────────

# ImageMagick built without the fontconfig delegate — the Homebrew default —
# has an empty `magick -list font`, so bare `-annotate` dies with "unable to
# read font `'" and the drawn fallback icon never materializes. Resolve one
# concrete font file up front instead of trusting a font name.
resolve_icon_font_file() {
    local candidate
    for candidate in \
        "${SHOWY_QUOTA_SKETCHYBAR_ICON_FONT_FILE:-}" \
        /System/Library/Fonts/SFNS.ttf \
        /System/Library/Fonts/Helvetica.ttc \
        /System/Library/Fonts/Supplemental/Arial.ttf; do
        [[ -n "${candidate}" && -r "${candidate}" ]] || continue
        printf '%s' "${candidate}"
        return 0
    done
    return 1
}

resolve_icon_sources() {
    (( ICON_SOURCES_RESOLVED )) && return 0
    CODEXBAR_RESOURCES="$(validated_codexbar_resources || true)"
    ICON_FONT_FILE="$(resolve_icon_font_file || true)"
    ICON_SOURCES_RESOLVED=1
}

# Drawn sigil icon for providers whose SVG is missing or unrenderable. The disc
# color is passed in and the result is published untinted: the recolor path
# flattens an icon to a single hex via its alpha shape, which would erase the
# sigil letters it exists to show.
render_fallback_icon_png() {
    local pid="$1" tmp="$2" disc_hex="${3:-${PRIMARY_UNKNOWN_HEX}}"
    local sigil
    sigil=$(showy_quota_provider_sigil "${pid}")
    local -a disc=( -size 64x64 xc:none
        -fill "$(mhex "${disc_hex}")" -draw "circle 32,32 32,4" )
    if [[ -n "${ICON_FONT_FILE}" ]] \
       && magick "${disc[@]}" \
            -font "${ICON_FONT_FILE}" -fill "$(mhex "${ICON_TEXT_HEX}")" \
            -gravity center -pointsize 28 -annotate 0 "${sigil}" \
            "PNG32:${tmp}" >/dev/null 2>&1; then
        return 0
    fi
    # No usable font: a plain disc still gives the provider a visible,
    # clickable slot instead of an empty gap.
    magick "${disc[@]}" "PNG32:${tmp}" >/dev/null 2>&1
}

# A successful `magick` run is not proof of a visible icon: ImageMagick's
# internal MSVG decoder silently rasterizes stroke-only paths (fill="none"
# stroke="…", used by ~29 of CodexBar's provider SVGs) to a fully transparent
# image and still exits 0. Without this check the sigil fallback never fires
# and the provider gets an invisible icon slot.
icon_png_has_pixels() {
    local alpha
    alpha=$(magick "$1" -format '%[fx:maxima.a]' info: 2>/dev/null) || return 1
    [[ -n "${alpha}" && "${alpha}" != "0" ]]
}

# librsvg implements the full SVG spec (stroke-only paths included) and never
# loads remote hrefs, so it preserves the SSRF property that forcing MSVG:
# buys us — verified against librsvg 2.62. It is invoked directly rather than
# as an ImageMagick delegate so policy.xml's `delegate rights=none` stands.
# MSVG: stays as the fallback when rsvg-convert is absent.
rasterize_provider_svg() {
    local svg="$1" out="$2"
    if (( HAVE_RSVG )) \
       && rsvg-convert -a -w 64 -h 64 -f png -o "${out}" "${svg}" >/dev/null 2>&1 \
       && icon_png_has_pixels "${out}"; then
        return 0
    fi
    magick -background none -density 300 "MSVG:${svg}" \
        -resize 64x64 "PNG32:${out}" >/dev/null 2>&1 \
        && icon_png_has_pixels "${out}"
}

recolor_icon_png() {
    local src="$1" hex="$2" out="$3"
    magick "${src}" -alpha extract \
        -background "$(mhex "${hex}")" -alpha shape \
        "PNG32:${out}" >/dev/null 2>&1
}

should_tint_dark_icon_png() {
    local png="$1" stats r g b mean min max
    stats=$(magick "${png}" -alpha off -colorspace RGB -channel RGB -separate +channel -format '%[fx:round(1000*mean/QuantumRange)] ' info: 2>/dev/null) || return 1
    read -r r g b <<< "${stats}" || return 1
    [[ -n "${r}" && -n "${g}" && -n "${b}" ]] || return 1

    mean=$(( (r + g + b) / 3 ))
    min=$r
    max=$r
    (( g < min )) && min=$g
    (( b < min )) && min=$b
    (( g > max )) && max=$g
    (( b > max )) && max=$b
    (( mean < 150 && (max - min) < 30 ))
}

# Rasterize the provider icon the renderer asked for (`icon` records) into
# DEST. The renderer owns the path: it keys the file on the icon status and
# the palette, and draws the icon once the file exists. It must NOT be called
# in a command substitution: that subshell would discard the ICON_TMP_FILES
# registrations below and defeat EXIT-trap cleanup.
provider_icon_png() {
    (( HAVE_MAGICK )) || return 1

    local pid="$1" status="${2:-none}" dest="$3"
    local status_color="" tint_color="" drawn=0
    # Only ever write a flat icon file directly inside our own icon cache.
    [[ "${dest%/*}" -ef "${CACHE_DIR}" && "${dest##*/}" == icon-v*.png ]] || return 1
    load_host_settings
    # An errored row with no incident indicator arrives as `error`: paint the
    # warning tint into the fallback glyph so the icon reads as an error.
    if [[ "${status}" == "error" ]]; then
        status_color="${COUNTDOWN_WARN_HEX}"
    else
        status_color_for_indicator_into status_color "${status}"
    fi

    # Per-process tmp files in the same directory so `mv` is atomic.
    local tmp normal_tmp
    normal_tmp=$(mktemp "${CACHE_DIR}/.icon-${pid}.normal.XXXXXX") || return 1
    ICON_TMP_FILES+=("${normal_tmp}")

    resolve_icon_sources
    local svg=""
    [[ -n "${CODEXBAR_RESOURCES}" ]] && svg="${CODEXBAR_RESOURCES}/ProviderIcon-${pid}.svg"
    if [[ -z "${svg}" || ! -r "${svg}" ]] || ! rasterize_provider_svg "${svg}" "${normal_tmp}"; then
        drawn=1
        if ! render_fallback_icon_png "${pid}" "${normal_tmp}" \
                "${status_color:-${PRIMARY_UNKNOWN_HEX}}"; then
            rm -f "${normal_tmp}"; return 1
        fi
    fi

    if (( drawn )); then
        tint_color=""
    elif [[ -n "${status_color}" ]]; then
        tint_color="${status_color}"
    elif should_tint_dark_icon_png "${normal_tmp}"; then
        tint_color="${ICON_TEXT_HEX}"
    fi

    if [[ -n "${tint_color}" ]]; then
        tmp=$(mktemp "${CACHE_DIR}/.icon-${pid}.tint.XXXXXX") || { rm -f "${normal_tmp}"; return 1; }
        ICON_TMP_FILES+=("${tmp}")
        if ! recolor_icon_png "${normal_tmp}" "${tint_color}" "${tmp}"; then
            rm -f "${tmp}"
            tmp="${normal_tmp}"
        else
            rm -f "${normal_tmp}"
        fi
    else
        tmp="${normal_tmp}"
    fi

    # Fail closed: a failed publish must not hand back a path to a file that
    # does not exist, and must not leak the staged temp.
    if ! mv -f "${tmp}" "${dest}"; then
        rm -f "${tmp}"
        return 1
    fi
}

# ── notch placement ──────────────────────────────────────────────────

# Fill LAYOUT_ITEMS with the live item names (`--emit sketchybar-query`).
load_layout_items() {
    local bar out
    local -a fields=()
    LAYOUT_ITEMS=()
    bar=$(sketchybar --query bar 2>/dev/null) || return 1
    [[ -n "${bar}" ]] || return 1
    out=$("${RENDER_BIN}" --emit sketchybar-query <<< "${bar}" 2>/dev/null) || return 1
    wire_fields_into fields "${out%%$'\n'*}"
    LAYOUT_ITEMS=("${fields[@]}")
}

# Measure the live bar in one batched query and let the renderer plan which
# providers sit right of the notch (`--emit sketchybar-layout`, which also
# stores the plan). Positions change with `--set position=`, never
# remove/re-add, so a re-split does not tear the pill down. When the plan
# draws more than before (labels or a provider come back), re-run the plugin
# once so the full rows are drawn. Arguments after PROVIDERS are the live item
# names when the caller already has them.
apply_notch_layout() {
    local providers="${1-}" out line item
    shift
    notch_placement || return 0
    [[ -n "${providers}" ]] || return 0

    local -a query_args=(--query bar --query displays) fields=() args=()
    LAYOUT_ITEMS=("$@")
    # SketchyBar drops a reply that takes over 100 ms, which happens right
    # after a redeclare. An empty reply would plan "no notch" and pull every
    # provider left, so retry once, then leave the note for the next tick.
    local measured=""
    if (( ${#LAYOUT_ITEMS[@]} > 0 )) || load_layout_items \
        || { sleep 0.3; load_layout_items; }; then
        for item in "${LAYOUT_ITEMS[@]}"; do
            query_args+=(--query "${item}")
        done
        measured=$(sketchybar "${query_args[@]}" 2>/dev/null) || measured=""
    fi
    out=""
    if [[ -n "${measured}" ]]; then
        out=$("${RENDER_BIN}" --emit sketchybar-layout --plan "${NOTCH_PLAN_FILE}" \
            --layout-providers "${providers//$'\n'/,}" <<< "${measured}" 2>/dev/null) || out=""
    fi
    while IFS= read -r line; do
        case "${line%%$'\x1f'*}" in
            layout) wire_fields_into fields "${line}" ;;
            set) wire_fields_into args "${line}" ;;
        esac
    done <<< "${out}"
    if [[ "${fields[0]:-}" != "ok" ]]; then
        showy_quota_log "notch layout: no reply from sketchybar; re-planning next tick"
        : > "${LAYOUT_PENDING_FILE}" 2>/dev/null || true
        return 0
    fi
    if (( ${#args[@]} > 0 )) && ! sketchybar "${args[@]}" >/dev/null 2>&1; then
        : > "${LAYOUT_PENDING_FILE}" 2>/dev/null || true
    fi
    # Hiding happened above. Showing a label or provider again needs the full
    # row, so re-run the plugin once when the plan draws more than before.
    if [[ "${fields[1]:-0}" == "1" ]]; then
        ( sleep 0.3; sketchybar --trigger showy_quota_refresh ) </dev/null >/dev/null 2>&1 &
        disown "$!" 2>/dev/null || true
    fi
    return 0
}

# One ring-mode tick: declare what the frame's `units` record lists
# (`unit=provider` pairs), drop what left, and store the units as the
# declared set. The renderer already diffed the per-tick values; the icons
# loop below is a no-op in ring mode (no `icon` records).
ring_tick() {
    local pair unit pid
    local desired_units="" desired_providers="${FRAME_PROVIDERS}"
    for pair in ${FRAME_UNITS[@]+"${FRAME_UNITS[@]}"}; do
        unit="${pair%%=*}"
        [[ -n "${unit}" ]] || continue
        desired_units+="${desired_units:+$'\n'}${unit}"
    done
    redeclared=0
    if [[ "${FRAME_REDECLARE}" != "-" ]] || (( body_changed )); then
        redeclared=1
        case "${FRAME_REDECLARE}" in
            missing) showy_quota_log "sketchybar ring items missing; forcing redeclare" ;;
            order) showy_quota_log "sketchybar items precede showy_quota.trigger; forcing redeclare" ;;
            body) showy_quota_log "sketchybar rows items linger in ring mode; forcing redeclare" ;;
        esac
        load_host_settings
        load_state_providers
        # Notch placement is a rows-body feature; ring mode stays left.
        queue_notch_anchor_removal
        if (( body_changed )) && [[ "${prev_body}" != "ring" ]]; then
            # Rows to ring (or first run): no slider may survive.
            clear_rows_body_items
        elif (( body_changed )); then
            clear_ring_body_items
        elif [[ "${FRAME_REDECLARE}" == "body" ]]; then
            # A crashed switch left rows sliders behind; the saved list
            # cannot name a provider that has since left the filtered set,
            # so sweep the whole rows body by pattern.
            clear_rows_body_items
        else
            while IFS= read -r unit; do
                [[ -n "${unit}" ]] || continue
                provider_list_contains "${desired_units}" "${unit}"                     || queue_ring_unit_removal "${unit}"
            done <<< "${STATE_PROVIDERS}"
        fi
        while IFS= read -r pair; do
            unit="${pair%%=*}"
            pid="${pair#*=}"
            [[ -n "${unit}" && -n "${pid}" ]] && queue_ring_unit_declaration "${unit}" "${pid}"
        done <<< "$(printf '%s\n' ${FRAME_UNITS[@]+"${FRAME_UNITS[@]}"})"
        queue_ring_bracket "${desired_units}"
        flush_sketchybar_queue
        write_state_providers "${desired_units}" \
            || showy_quota_log "failed to update sketchybar ring state"
        printf '%s\n' "ring" > "${BODY_FILE}" 2>/dev/null || true
        trigger_provider_change "${desired_providers}"
    fi
}

# ── main ─────────────────────────────────────────────────────────────

# Neighbour geometry changed, quota data did not: these events only re-plan
# the notch split. The timer, showy_quota_refresh, wake, and a direct run all
# render. A layout event that finds a render in flight leaves a note; that
# render re-plans before it exits.
case "${SENDER:-}" in
    front_app_switched|display_change|showy_quota_layout)
        notch_placement || exit 0
        showy_quota_export_config
        resolve_effective_body
        [[ "${EFFECTIVE_BODY}" == "ring" ]] && exit 0
        if ! acquire_render_lock; then
            : > "${LAYOUT_PENDING_FILE}" 2>/dev/null || true
            exit 0
        fi
        [[ -e "${LAYOUT_PENDING_FILE}" ]] && rm -f -- "${LAYOUT_PENDING_FILE}"
        load_state_providers
        apply_notch_layout "${STATE_PROVIDERS}"
        exit 0
        ;;
esac

acquire_render_lock || exit 0

# All per-tick compute lives in the native renderer (`--emit
# sketchybar-frame`, crates/showy-quota-zellij-core/src/sketchybar_frame.rs):
# rows, the redeclare decision, and the `sketchybar` arguments, diffed against
# the frame this plugin sent last so a tick where nothing changed sends
# nothing. This script declares items, rasterizes icons, and runs sketchybar.
showy_quota_export_config
resolve_effective_body
# The renderer reads SHOWY_QUOTA_SKETCHYBAR_BODY itself; hand it the probed
# value so a stock-SketchyBar fallback renders rows, not missing rings.
SHOWY_QUOTA_SKETCHYBAR_BODY="${EFFECTIVE_BODY}"
prev_body=""
body_changed=0
# No stamp on first run: the rows items are the historical default, so an
# absent stamp means rows, not a switch. Forcing here would redeclare on
# every fresh cache dir.
if [[ -f "${BODY_FILE}" ]]; then
    prev_body=$(< "${BODY_FILE}")
    [[ "${prev_body}" != "${EFFECTIVE_BODY}" ]] && body_changed=1
fi
frame_flags=(--emit sketchybar-frame --from-cache --bar -
    --state "${STATE_FILE}" --frame "${FRAME_FILE}" --plan "${NOTCH_PLAN_FILE}")
showy_quota_bool "${SHOWY_QUOTA_SKETCHYBAR_FORCE_REDECLARE-}" 0 && frame_flags+=(--force-redeclare)
(( body_changed )) && frame_flags+=(--force-redeclare)
(( HAVE_MAGICK )) && frame_flags+=(--icon-maker)

# SketchyBar drops a reply that takes over 100 ms; the renderer then treats
# the item list as unknown rather than missing.
bar_items=$(sketchybar --query bar 2>/dev/null) || bar_items=""
render_frame() {
    frame_out=$("${RENDER_BIN}" "${frame_flags[@]}" "$@" <<< "${bar_items}" 2>/dev/null) || frame_out=""
}

# Cache first. A usable cache costs this tick one renderer run; only a
# missing or unusable cache pays for a synchronous fetch. An aged cache still
# renders now and refreshes in the background. If the fetch leaves no usable
# cache either, the empty frame tears the providers down while stale/degraded
# still reflect the file.
frame_out=""
[[ -s "${SHOWY_QUOTA_USAGE_FILE}" ]] && render_frame
if [[ -z "${frame_out}" ]]; then
    "${FETCH}" >/dev/null 2>&1 || true
    render_frame --or-empty
fi
parse_frame_output "${frame_out}"
(( FRAME_REFRESH )) && start_background_refresh

desired_providers="${FRAME_PROVIDERS}"
redeclared=0
if [[ "${EFFECTIVE_BODY}" == "ring" ]]; then
    ring_tick
else
if [[ "${FRAME_REDECLARE}" != "-" ]]; then
    redeclared=1
    case "${FRAME_REDECLARE}" in
        missing) showy_quota_log "sketchybar items missing; forcing redeclare" ;;
        order) showy_quota_log "sketchybar items precede showy_quota.trigger; forcing redeclare" ;;
        body) showy_quota_log "sketchybar ring items linger in rows mode; forcing redeclare" ;;
    esac
    load_host_settings
    load_state_providers
    if (( body_changed )) && [[ "${prev_body}" == "ring" ]]; then
        # Ring to rows: no ring, gap, or edge item may survive.
        clear_ring_body_items
    elif [[ "${FRAME_REDECLARE}" == "body" ]]; then
        # A crashed switch left ring items behind; sweep the whole ring
        # body by pattern, since no saved list names every stray.
        clear_ring_body_items
    fi
    # Sweep the rows body on every redeclare: an interrupted tick can leave
    # rows for a provider that neither the saved list nor the desired set
    # names (added, never recorded, then filtered out), and those partial
    # rows would otherwise linger outside the bracket forever. The declare
    # loop below restores every desired provider. This runs before the
    # notch anchors so the sweep never undoes their adds.
    clear_rows_body_items
    # Anchors go first so they precede every provider item in the `e` flow.
    if notch_placement; then
        queue_notch_anchors
    else
        queue_notch_anchor_removal
    fi
    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        provider_list_contains "${desired_providers}" "${pid}" || queue_provider_removal "${pid}"
    done <<< "${STATE_PROVIDERS}"
    while IFS= read -r pid; do
        [[ -n "${pid}" ]] && queue_provider_declaration "${pid}"
    done <<< "${desired_providers}"
    queue_bracket "${desired_providers}"
    flush_sketchybar_queue
    write_state_providers "${desired_providers}" || showy_quota_log "failed to update sketchybar provider state"
    printf '%s\n' "rows" > "${BODY_FILE}" 2>/dev/null || true
    trigger_provider_change "${desired_providers}"
fi
fi

frame_args=("${FRAME_ARGS[@]}")
# The renderer asks for icons it cannot draw yet. Rasterize them, then let it
# diff again: the second frame adds just the new icons.
icons_made=0
for record in "${FRAME_ICONS[@]}"; do
    icon_fields=()
    wire_fields_into icon_fields "${record}"
    # Direct call, never $( ): see the ICON_TMP_FILES invariant.
    provider_icon_png "${icon_fields[0]:-}" "${icon_fields[1]:-none}" "${icon_fields[2]:-}" \
        && icons_made=1
done
if (( icons_made )); then
    render_frame --assume-declared --or-empty
    parse_frame_output "${frame_out}"
    frame_args+=("${FRAME_ARGS[@]}")
fi

layout_due=0
if (( ${#frame_args[@]} > 0 )); then
    # The renderer already stored this frame; forget it if SketchyBar refused
    # the update, so the next tick sends every provider again.
    sketchybar "${frame_args[@]}" >/dev/null 2>&1 || rm -f -- "${FRAME_FILE}"
    layout_due=1
fi
# Wake can change the display set without any row changing.
[[ "${SENDER:-}" == "system_woke" ]] && layout_due=1
if [[ -e "${LAYOUT_PENDING_FILE}" ]]; then
    rm -f -- "${LAYOUT_PENDING_FILE}"
    layout_due=1
fi
if (( layout_due )) && [[ "${EFFECTIVE_BODY}" != "ring" ]]; then
    # A redeclare replaced the items the renderer listed; re-read them.
    if (( redeclared || ! FRAME_HAS_QUERY )); then
        apply_notch_layout "${desired_providers}"
    else
        apply_notch_layout "${desired_providers}" "${FRAME_QUERY[@]}"
    fi
    # Items a redeclare just added can lack geometry, and SketchyBar is often
    # still too busy to answer the planner's query (seen for over a second).
    # Re-plan once it settles; the note covers a trigger that also fails.
    if (( redeclared )) && notch_placement; then
        : > "${LAYOUT_PENDING_FILE}" 2>/dev/null || true
        ( sleep 2; sketchybar --trigger showy_quota_layout ) </dev/null >/dev/null 2>&1 &
        disown "$!" 2>/dev/null || true
    fi
fi
