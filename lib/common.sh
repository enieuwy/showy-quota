#!/usr/bin/env bash
# showy-bar — shared helpers.
#
# This file is sourced by every script in bin/ and by the SketchyBar item +
# plugin. It must stay self-contained: no external commands at load time.

set -uo pipefail

# ── config loading ─────────────────────────────────────────────────────

showy_bar_load_config() {
    local config_dir="${XDG_CONFIG_HOME:-${HOME}/.config}/showy-bar"
    local config_file="${config_dir}/config.env"
    local theme=""
    local theme_path=""
    local repo_root

    if [[ -z "${SHOWY_BAR_NO_CONFIG:-}" && -r "${config_file}" ]]; then
        # shellcheck disable=SC1090
        . "${config_file}"
    fi

    theme="${SHOWY_BAR_THEME:-}"
    [[ -n "${theme}" ]] || return 0

    repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
    for theme_path in \
        "${config_dir}/themes/${theme}.env" \
        "${repo_root}/share/themes/${theme}.env"
    do
        if [[ -r "${theme_path}" ]]; then
            # shellcheck disable=SC1090
            . "${theme_path}"
            return 0
        fi
    done

    printf 'showy-bar: theme %q not found\n' "${theme}" >&2
    return 1
}
showy_bar_load_config

# ── defaults ───────────────────────────────────────────────────────────

: "${SHOWY_BAR_REFRESH_SECONDS:=120}"
: "${SHOWY_BAR_LOCK_WAIT_TENTHS:=100}"
: "${SHOWY_BAR_CACHE_DIR:=${XDG_CACHE_HOME:-${HOME}/.cache}/showy-bar}"
: "${SHOWY_BAR_CODEXBAR_BIN:=codexbar}"
: "${SHOWY_BAR_PROVIDERS:=}"
: "${SHOWY_BAR_PROVIDERS_EXCLUDE:=}"
: "${SHOWY_BAR_PROVIDER_ORDER:=codex,claude,opencode,gemini}"
: "${SHOWY_BAR_INCLUDE_STATUS:=1}"

: "${SHOWY_BAR_PALETTE_PRIMARY_GOOD:=25be6a}"
: "${SHOWY_BAR_PALETTE_PRIMARY_WARN:=f0af00}"
: "${SHOWY_BAR_PALETTE_PRIMARY_BAD:=ee5396}"
: "${SHOWY_BAR_PALETTE_PRIMARY_UNKNOWN:=6c7086}"
: "${SHOWY_BAR_PALETTE_SECONDARY_SCALE:=0.55}"
: "${SHOWY_BAR_PALETTE_TERTIARY_SCALE:=0.55}"
: "${SHOWY_BAR_PALETTE_BG:=161616}"
: "${SHOWY_BAR_PALETTE_SURFACE:=2a2a2a}"
: "${SHOWY_BAR_PALETTE_TRACK:=3a3a4a}"
: "${SHOWY_BAR_PALETTE_ICON_TEXT:=f2f4f8}"
: "${SHOWY_BAR_PALETTE_COUNTDOWN:=7b8496}"
: "${SHOWY_BAR_PALETTE_COUNTDOWN_WARN:=${SHOWY_BAR_PALETTE_PRIMARY_BAD}}"
: "${SHOWY_BAR_PALETTE_ELAPSED:=be95ff}"

: "${SHOWY_BAR_GOOD_MIN_REMAINING:=40}"
: "${SHOWY_BAR_WARN_MIN_REMAINING:=15}"
: "${SHOWY_BAR_TIME_WARN_MINUTES:=30}"

: "${SHOWY_BAR_CODEXBAR_RESOURCES:=/Applications/CodexBar.app/Contents/Resources}"
: "${SHOWY_BAR_SKETCHYBAR_IMAGE_CACHE:=${SHOWY_BAR_CACHE_DIR}/sketchybar}"
: "${SHOWY_BAR_SKETCHYBAR_CLICK:=open -b com.steipete.codexbar}"
: "${SHOWY_BAR_SKETCHYBAR_UPDATE_FREQ:=120}"
: "${SHOWY_BAR_PNG_BAR_W:=80}"
: "${SHOWY_BAR_PNG_BAR_H:=18}"
: "${SHOWY_BAR_SKETCHYBAR_ICON_WIDTH:=22}"
: "${SHOWY_BAR_SKETCHYBAR_ICON_PADDING_LEFT:=5}"
: "${SHOWY_BAR_SKETCHYBAR_ICON_SCALE:=0.28}"
: "${SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_MODE:=svg}"
: "${SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_FONT:=sketchybar-app-font:Regular:14.0}"
: "${SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_FONT_PADDING_RIGHT:=2}"
: "${SHOWY_BAR_SKETCHYBAR_BAR_WIDTH:=$((SHOWY_BAR_PNG_BAR_W + 4))}"

: "${SHOWY_BAR_SKETCHYBAR_COMPACT_PROVIDER_COUNT:=5}"
: "${SHOWY_BAR_SKETCHYBAR_PILL_RADIUS:=14}"
: "${SHOWY_BAR_SKETCHYBAR_PILL_HEIGHT:=28}"
: "${SHOWY_BAR_SKETCHYBAR_PILL_COLOR:=0xcc24273a}"
: "${SHOWY_BAR_ZELLIJ_WIDGET:=pipe_showy_bar}"
: "${SHOWY_BAR_ZELLIJ_PIPE_NAME:=showy-bar}"
: "${SHOWY_BAR_ZELLIJ_PIPE_INTERVAL:=10}"
: "${SHOWY_BAR_ZELLIJ_PIPE_TIMEOUT_TENTHS:=20}"
: "${SHOWY_BAR_ZELLIJ_BAR_WIDTH:=12}"
: "${SHOWY_BAR_ZELLIJ_BIN:=zellij}"
: "${SHOWY_BAR_ZELLIJ_PLUGIN:=}"

: "${SHOWY_BAR_USAGE_FILE:=${SHOWY_BAR_CACHE_DIR}/usage.json}"
: "${SHOWY_BAR_USAGE_STAMP:=${SHOWY_BAR_CACHE_DIR}/usage.json.updated-at}"
: "${SHOWY_BAR_USAGE_LOCK:=${SHOWY_BAR_CACHE_DIR}/usage.lock}"

declare -gA SHOWY_BAR_ROLE_PALETTE_CACHE=()

# ── small utilities ────────────────────────────────────────────────────

showy_bar_log() {
    [[ -n "${SHOWY_BAR_DEBUG:-}" ]] || return 0
    printf '[showy-bar] %s\n' "$*" >&2
}

showy_bar_die() {
    printf 'showy-bar: %s\n' "$*" >&2
    exit 1
}

showy_bar_have() { command -v "$1" >/dev/null 2>&1; }

showy_bar_now_epoch() {
    if [[ -n "${SHOWY_BAR_NOW_EPOCH:-}" && "${SHOWY_BAR_NOW_EPOCH}" =~ ^[0-9]+$ ]]; then
        printf '%s\n' "${SHOWY_BAR_NOW_EPOCH}"
    else
        date +%s
    fi
}

showy_bar_age_seconds() {
    # Seconds since file mtime; prints '999999999' when missing.
    local path="$1"
    [[ -f "${path}" ]] || { printf '999999999\n'; return; }
    local now mtime
    now=$(showy_bar_now_epoch)
    if mtime=$(stat -f %m "${path}" 2>/dev/null); then :
    elif mtime=$(stat -c %Y "${path}" 2>/dev/null); then :
    else mtime="${now}"; fi
    printf '%s\n' $((now - mtime))
}

showy_bar_parse_local_epoch() {
    local fmt="$1" value="$2"
    if date -j -f "${fmt}" "${value}" '+%s' 2>/dev/null; then
        return 0
    fi
    if showy_bar_have gdate; then
        gdate -d "${value}" '+%s' 2>/dev/null && return 0
    fi
    date -d "${value}" '+%s' 2>/dev/null
}

showy_bar_reset_description_epoch() {
    local raw="$1"
    [[ -n "${raw}" && "${raw}" != "null" ]] || return 1
    local desc="${raw#Resets }"
    [[ "${desc}" != "${raw}" ]] || desc="${raw#resets }"
    [[ "${desc}" != "${raw}" ]] || return 1

    local epoch
    for fmt in '%b %d, %Y %I:%M %p' '%B %d, %Y %I:%M %p'; do
        if epoch=$(showy_bar_parse_local_epoch "${fmt}" "${desc}"); then
            printf '%s\n' "${epoch}"
            return 0
        fi
    done

    local today now
    today=$(date '+%Y-%m-%d')
    if epoch=$(showy_bar_parse_local_epoch '%Y-%m-%d %I:%M %p' "${today} ${desc}"); then
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
showy_bar_reset_epoch() {
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
    if showy_bar_have gdate; then
        gdate -d "${raw}" '+%s' 2>/dev/null && return 0
    fi
    if date -d "${raw}" '+%s' 2>/dev/null; then
        return 0
    fi
    showy_bar_reset_description_epoch "${raw}" && return 0
    return 1
}

# Minutes (rounded down) until reset. Prints '' on failure, '0' if already
# past.
showy_bar_minutes_until() {
    local reset_at="$1"
    local epoch
    epoch=$(showy_bar_reset_epoch "${reset_at}") || return 1
    local now
    now=$(showy_bar_now_epoch)
    local diff=$(( epoch - now ))
    (( diff < 0 )) && diff=0
    printf '%s\n' $((diff / 60))
}

# Compact countdown: 'now' / '12m' / '3h' / '3:45' / '2d' / '5w'.
showy_bar_format_countdown() {
    local minutes="$1"
    [[ -n "${minutes}" ]] || { printf '?'; return; }
    if (( minutes <= 0 )); then printf 'now'; return; fi
    if (( minutes < 60 )); then printf '%dm' "${minutes}"; return; fi
    local hours=$((minutes / 60)) m=$((minutes % 60))
    if (( hours < 24 )); then
        if (( m == 0 )); then printf '%dh' "${hours}"
        else printf '%d:%02d' "${hours}" "${m}"
        fi
        return
    fi
    local days=$((hours / 24))
    if (( days < 14 )); then printf '%dd' "${days}"
    else printf '%dw' $((days / 7))
    fi
}

showy_bar_primary_label() {
    local minutes="$1" remaining="$2" reset_value="${3:-}"
    if [[ -n "${minutes}" ]]; then
        showy_bar_format_countdown "${minutes}"
        return
    fi
    if [[ -z "${reset_value}" && "${remaining}" =~ ^-?[0-9]+$ ]] && (( remaining >= 100 )); then
        printf 'idle'
        return
    fi
    showy_bar_format_countdown "${minutes}"
}

# Map remaining-percent → palette key (good|warn|bad|unknown).
showy_bar_color_key() {
    local remaining="$1"
    [[ "${remaining}" =~ ^-?[0-9]+([.][0-9]+)?$ ]] || { printf 'unknown'; return; }
    remaining="${remaining%%.*}"
    [[ "${remaining}" == "-0" ]] && remaining=0
    if (( remaining >= SHOWY_BAR_GOOD_MIN_REMAINING )); then printf 'good'
    elif (( remaining >= SHOWY_BAR_WARN_MIN_REMAINING )); then printf 'warn'
    else printf 'bad'
    fi
}

showy_bar_scale_component() {
    local value="$1" factor_num="$2" factor_den="$3"
    local scaled=$(( (value * factor_num) / factor_den ))
    (( scaled < 0 )) && scaled=0
    (( scaled > 255 )) && scaled=255
    printf '%02x' "${scaled}"
}

showy_bar_scale_hex() {
    local hex="$1"
    local factor="${2:-1}"
    [[ "${hex}" =~ ^[[:xdigit:]]{6}$ ]] || showy_bar_die "invalid palette hex: ${hex}"
    [[ "${factor}" =~ ^([0-9]+([.][0-9]+)?|[.][0-9]+)$ ]] || showy_bar_die "invalid palette scale: ${factor}"

    local factor_num factor_den=1 factor_int factor_frac
    if [[ "${factor}" == *.* ]]; then
        factor_int="${factor%%.*}"
        factor_frac="${factor#*.}"
        [[ -n "${factor_int}" ]] || factor_int=0
        factor_num=$((10#${factor_int}${factor_frac}))
        factor_den=1
        local i
        for (( i=0; i<${#factor_frac}; i++ )); do
            factor_den=$((factor_den * 10))
        done
    else
        factor_num=$((10#${factor}))
    fi

    local r=$((16#${hex:0:2}))
    local g=$((16#${hex:2:2}))
    local b=$((16#${hex:4:2}))
    printf '%s%s%s\n' \
        "$(showy_bar_scale_component "${r}" "${factor_num}" "${factor_den}")" \
        "$(showy_bar_scale_component "${g}" "${factor_num}" "${factor_den}")" \
        "$(showy_bar_scale_component "${b}" "${factor_num}" "${factor_den}")"
}

# Hex color (no '#') for a global palette token.
showy_bar_palette() {
    case "$1" in
        bg)      printf '%s' "${SHOWY_BAR_PALETTE_BG}" ;;
        surface) printf '%s' "${SHOWY_BAR_PALETTE_SURFACE}" ;;
        track)   printf '%s' "${SHOWY_BAR_PALETTE_TRACK}" ;;
        icon_text)      printf '%s' "${SHOWY_BAR_PALETTE_ICON_TEXT}" ;;
        countdown)      printf '%s' "${SHOWY_BAR_PALETTE_COUNTDOWN}" ;;
        countdown_warn) printf '%s' "${SHOWY_BAR_PALETTE_COUNTDOWN_WARN}" ;;
        elapsed) printf '%s' "${SHOWY_BAR_PALETTE_ELAPSED}" ;;
        *)       showy_bar_die "unknown global palette token: $1" ;;
    esac
}

# Hex color (no '#') for a role + severity pair.
showy_bar_role_palette() {
    local role="$1"
    local severity="$2"
    local cache_key="${role}:${severity}"
    local role_upper severity_upper result var_name scale_name primary_var

    if [[ -n "${SHOWY_BAR_ROLE_PALETTE_CACHE[${cache_key}]+x}" ]]; then
        printf '%s' "${SHOWY_BAR_ROLE_PALETTE_CACHE[${cache_key}]}"
        return 0
    fi

    case "${role}" in
        primary)
            role_upper="PRIMARY"
            ;;
        secondary)
            role_upper="SECONDARY"
            ;;
        tertiary)
            role_upper="TERTIARY"
            ;;
        *)
            showy_bar_die "unknown palette role: ${role}"
            ;;
    esac

    case "${severity}" in
        good|warn|bad|unknown)
            severity_upper="${severity^^}"
            ;;
        *)
            showy_bar_die "unknown palette severity: ${severity}"
            ;;
    esac

    var_name="SHOWY_BAR_PALETTE_${role_upper}_${severity_upper}"
    if [[ "${role}" == "primary" ]]; then
        result="${!var_name}"
    elif [[ -n "${!var_name:-}" ]]; then
        result="${!var_name}"
    else
        primary_var="SHOWY_BAR_PALETTE_PRIMARY_${severity_upper}"
        scale_name="SHOWY_BAR_PALETTE_${role_upper}_SCALE"
        result="$(showy_bar_scale_hex "${!primary_var}" "${!scale_name}")"
    fi

    SHOWY_BAR_ROLE_PALETTE_CACHE["${cache_key}"]="${result}"
    printf '%s' "${result}"
}

showy_bar_role_color() {
    local role="$1"
    local remaining="$2"
    showy_bar_role_palette "${role}" "$(showy_bar_color_key "${remaining}")"
}

# Validate that codexbar JSON looks like an array of provider objects.
showy_bar_json_valid() {
    local file="$1"
    [[ -s "${file}" ]] || return 1
    showy_bar_have jq || return 1
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
