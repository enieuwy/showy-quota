#!/usr/bin/env bash
# codexbar-bars — shared helpers.
#
# This file is sourced by every script in bin/ and by the SketchyBar item +
# plugin. It must stay self-contained: no external commands at load time.

set -uo pipefail

# ── config loading ─────────────────────────────────────────────────────

cb_bars_load_config() {
    local config_dir="${XDG_CONFIG_HOME:-${HOME}/.config}/codexbar-bars"
    local config_file="${config_dir}/config.env"

    if [[ -z "${CB_BARS_NO_CONFIG:-}" && -r "${config_file}" ]]; then
        # shellcheck disable=SC1090
        . "${config_file}"
    fi
}
cb_bars_load_config

# ── defaults ───────────────────────────────────────────────────────────

: "${CB_BARS_REFRESH_SECONDS:=120}"
: "${CB_BARS_LOCK_WAIT_TENTHS:=100}"
: "${CB_BARS_CACHE_DIR:=${XDG_CACHE_HOME:-${HOME}/.cache}/codexbar-bars}"
: "${CB_BARS_CODEXBAR_BIN:=codexbar}"
: "${CB_BARS_PROVIDERS:=}"
: "${CB_BARS_INCLUDE_STATUS:=0}"

: "${CB_BARS_PALETTE_GOOD:=25be6a}"
: "${CB_BARS_PALETTE_WARN:=f0af00}"
: "${CB_BARS_PALETTE_BAD:=ee5396}"
: "${CB_BARS_PALETTE_UNKNOWN:=6c7086}"
: "${CB_BARS_PALETTE_TRACK:=3a3a4a}"
: "${CB_BARS_PALETTE_TEXT:=f2f4f8}"

: "${CB_BARS_GOOD_MIN_REMAINING:=40}"
: "${CB_BARS_WARN_MIN_REMAINING:=15}"
: "${CB_BARS_TIME_WARN_MINUTES:=30}"

: "${CB_BARS_CODEXBAR_RESOURCES:=/Applications/CodexBar.app/Contents/Resources}"
: "${CB_BARS_SKETCHYBAR_IMAGE_CACHE:=${CB_BARS_CACHE_DIR}/sketchybar}"
: "${CB_BARS_SKETCHYBAR_CLICK:=open -b com.steipete.codexbar}"
: "${CB_BARS_SKETCHYBAR_UPDATE_FREQ:=120}"
: "${CB_BARS_PNG_BAR_W:=80}"
: "${CB_BARS_PNG_BAR_H:=18}"
: "${CB_BARS_SKETCHYBAR_ICON_WIDTH:=22}"
: "${CB_BARS_SKETCHYBAR_ICON_PADDING_LEFT:=5}"
: "${CB_BARS_SKETCHYBAR_ICON_SCALE:=0.28}"
: "${CB_BARS_SKETCHYBAR_BAR_WIDTH:=$((CB_BARS_PNG_BAR_W + 4))}"

: "${CB_BARS_ZELLIJ_WIDGET:=pipe_codexbar}"
: "${CB_BARS_ZELLIJ_PIPE_NAME:=cb-bars}"
: "${CB_BARS_ZELLIJ_PIPE_INTERVAL:=10}"
: "${CB_BARS_ZELLIJ_BIN:=zellij}"

: "${CB_BARS_USAGE_FILE:=${CB_BARS_CACHE_DIR}/usage.json}"
: "${CB_BARS_USAGE_STAMP:=${CB_BARS_CACHE_DIR}/usage.json.updated-at}"
: "${CB_BARS_USAGE_LOCK:=${CB_BARS_CACHE_DIR}/usage.lock}"

# ── small utilities ────────────────────────────────────────────────────

cb_bars_log() {
    [[ -n "${CB_BARS_DEBUG:-}" ]] || return 0
    printf '[cb-bars] %s\n' "$*" >&2
}

cb_bars_die() {
    printf 'cb-bars: %s\n' "$*" >&2
    exit 1
}

cb_bars_have() { command -v "$1" >/dev/null 2>&1; }

cb_bars_now_epoch() { date +%s; }

cb_bars_age_seconds() {
    # Seconds since file mtime; prints '999999999' when missing.
    local path="$1"
    [[ -f "${path}" ]] || { printf '999999999\n'; return; }
    local now mtime
    now=$(date +%s)
    if mtime=$(stat -f %m "${path}" 2>/dev/null); then :
    elif mtime=$(stat -c %Y "${path}" 2>/dev/null); then :
    else mtime="${now}"; fi
    printf '%s\n' $((now - mtime))
}

cb_bars_parse_local_epoch() {
    local fmt="$1" value="$2"
    if date -j -f "${fmt}" "${value}" '+%s' 2>/dev/null; then
        return 0
    fi
    if cb_bars_have gdate; then
        gdate -d "${value}" '+%s' 2>/dev/null && return 0
    fi
    date -d "${value}" '+%s' 2>/dev/null
}

cb_bars_reset_description_epoch() {
    local raw="$1"
    [[ -n "${raw}" && "${raw}" != "null" ]] || return 1
    local desc="${raw#Resets }"
    [[ "${desc}" != "${raw}" ]] || desc="${raw#resets }"
    [[ "${desc}" != "${raw}" ]] || return 1

    local epoch
    for fmt in '%b %d, %Y %I:%M %p' '%B %d, %Y %I:%M %p'; do
        if epoch=$(cb_bars_parse_local_epoch "${fmt}" "${desc}"); then
            printf '%s\n' "${epoch}"
            return 0
        fi
    done

    local today now
    today=$(date '+%Y-%m-%d')
    if epoch=$(cb_bars_parse_local_epoch '%Y-%m-%d %I:%M %p' "${today} ${desc}"); then
        now=$(date +%s)
        if (( epoch < now )); then
            epoch=$((epoch + 86400))
        fi
        printf '%s\n' "${epoch}"
        return 0
    fi

    return 1
}

# Convert ISO8601 'resetsAt' (with Z, fractional seconds, ±HH:MM offset, etc.)
# to a unix epoch. Prints nothing on failure.
cb_bars_reset_epoch() {
    local raw="$1"
    [[ -n "${raw}" && "${raw}" != "null" ]] || return 1

    # Normalize: strip fractional seconds (regardless of suffix style),
    # collapse +HH:MM → +HHMM, replace Z with +0000.
    local cleaned
    cleaned=$(printf '%s' "${raw}" \
        | sed -E 's/\.[0-9]+(Z|[+-][0-9]{2}:?[0-9]{2})?$/\1/' \
        | sed -E 's/Z$/+0000/' \
        | sed -E 's/([+-][0-9]{2}):([0-9]{2})$/\1\2/')

    # macOS BSD date.
    if date -j -f '%Y-%m-%dT%H:%M:%S%z' "${cleaned}" '+%s' 2>/dev/null; then
        return 0
    fi
    # GNU date (Linux).
    if cb_bars_have gdate; then
        gdate -d "${raw}" '+%s' 2>/dev/null && return 0
    fi
    if date -d "${raw}" '+%s' 2>/dev/null; then
        return 0
    fi
    cb_bars_reset_description_epoch "${raw}" && return 0
    return 1
}

# Minutes (rounded down) until reset. Prints '' on failure, '0' if already
# past.
cb_bars_minutes_until() {
    local reset_at="$1"
    local epoch
    epoch=$(cb_bars_reset_epoch "${reset_at}") || return 1
    local now
    now=$(date +%s)
    local diff=$(( epoch - now ))
    (( diff < 0 )) && diff=0
    printf '%s\n' $((diff / 60))
}

# Compact countdown: 'now' / '12m' / '3h45m' / '2d' / '5w'.
cb_bars_format_countdown() {
    local minutes="$1"
    [[ -n "${minutes}" ]] || { printf '?'; return; }
    if (( minutes <= 0 )); then printf 'now'; return; fi
    if (( minutes < 60 )); then printf '%dm' "${minutes}"; return; fi
    local hours=$((minutes / 60)) m=$((minutes % 60))
    if (( hours < 24 )); then
        if (( m == 0 )); then printf '%dh' "${hours}"
        else printf '%dh%dm' "${hours}" "${m}"
        fi
        return
    fi
    local days=$((hours / 24))
    if (( days < 14 )); then printf '%dd' "${days}"
    else printf '%dw' $((days / 7))
    fi
}

# Map remaining-percent → palette key (good|warn|bad|unknown).
cb_bars_color_key() {
    local remaining="$1"
    [[ "${remaining}" =~ ^-?[0-9]+([.][0-9]+)?$ ]] || { printf 'unknown'; return; }
    remaining="${remaining%%.*}"
    [[ "${remaining}" == "-0" ]] && remaining=0
    if (( remaining >= CB_BARS_GOOD_MIN_REMAINING )); then printf 'good'
    elif (( remaining >= CB_BARS_WARN_MIN_REMAINING )); then printf 'warn'
    else printf 'bad'
    fi
}

# Hex color (no '#') for a palette key.
cb_bars_palette() {
    case "$1" in
        good)    printf '%s' "${CB_BARS_PALETTE_GOOD}" ;;
        warn)    printf '%s' "${CB_BARS_PALETTE_WARN}" ;;
        bad)     printf '%s' "${CB_BARS_PALETTE_BAD}" ;;
        unknown) printf '%s' "${CB_BARS_PALETTE_UNKNOWN}" ;;
        track)   printf '%s' "${CB_BARS_PALETTE_TRACK}" ;;
        text)    printf '%s' "${CB_BARS_PALETTE_TEXT}" ;;
        *)       printf '%s' "${CB_BARS_PALETTE_UNKNOWN}" ;;
    esac
}

# Validate that codexbar JSON looks like an array of provider objects.
cb_bars_json_valid() {
    local file="$1"
    [[ -s "${file}" ]] || return 1
    cb_bars_have jq || return 1
    jq -e '
        type == "array" and
        all(.[]; type == "object"
            and (.provider | type == "string" and test("^[A-Za-z0-9_.-]+$"))
            and (
                (.usage // null) == null
                or (
                    (.usage | type) == "object"
                    and all([.usage.primary, .usage.secondary, .usage.tertiary][];
                        . == null
                        or (type == "object" and (.usedPercent | type) == "number")
                    )
                )
            )
        )
    ' "${file}" >/dev/null 2>&1
}
