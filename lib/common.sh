#!/usr/bin/env bash
# showy-quota — shared helpers.
#
# This file is sourced by every script in bin/ and by the SketchyBar item +
# plugin. It must stay self-contained: no external commands at load time.

if [ "${BASH_VERSINFO[0]:-0}" -lt 4 ]; then
    printf 'showy-quota: bash 4+ required (running %s). On macOS, install Homebrew bash and ensure it precedes /bin/bash on PATH.\n' "${BASH_VERSION:-unknown}" >&2
    # shellcheck disable=SC2317
    return 1 2>/dev/null || exit 1
fi

set -uo pipefail

# ── config loading ─────────────────────────────────────────────────────

showy_quota_load_config() {
    local config_dir="${XDG_CONFIG_HOME:-${HOME}/.config}/showy-quota"
    local config_file="${config_dir}/config.env"
    local theme=""
    local theme_path=""
    local repo_root

    if [[ -z "${SHOWY_QUOTA_NO_CONFIG:-}" && -r "${config_file}" ]]; then
        # shellcheck disable=SC1090
        . "${config_file}"
    fi

    theme="${SHOWY_QUOTA_THEME:-}"
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

    printf 'showy-quota: theme %q not found\n' "${theme}" >&2
    return 1
}
showy_quota_load_config

# ── defaults ───────────────────────────────────────────────────────────

: "${SHOWY_QUOTA_REFRESH_SECONDS:=120}"
: "${SHOWY_QUOTA_LOCK_WAIT_TENTHS:=100}"
: "${SHOWY_QUOTA_CACHE_DIR:=${XDG_CACHE_HOME:-${HOME}/.cache}/showy-quota}"
: "${SHOWY_QUOTA_CODEXBAR_BIN:=codexbar}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_URL=http://127.0.0.1:8080}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_PORT:=}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_INTERVAL_SECONDS:=60}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS:=30}"
: "${SHOWY_QUOTA_MANAGE_SERVE:=1}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS:=10}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS:=10}"
: "${SHOWY_QUOTA_CODEXBAR_CLI_TIMEOUT_SECONDS:=20}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_FAILURES_BEFORE_RESTART:=3}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_FAILURES_BEFORE_CLI:=${SHOWY_QUOTA_CODEXBAR_SERVE_FAILURES_BEFORE_RESTART}}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_FAILURE_BACKOFF_SECONDS:=60}"
: "${SHOWY_QUOTA_CODEXBAR_CLI_FAILURE_BACKOFF_SECONDS:=${SHOWY_QUOTA_REFRESH_SECONDS}}"
: "${SHOWY_QUOTA_CODEXBAR_CONFIG_PROVIDERS_TIMEOUT_SECONDS:=5}"
: "${SHOWY_QUOTA_CODEXBAR_CONFIG_PROVIDERS_BACKOFF_SECONDS:=60}"
: "${SHOWY_QUOTA_PROVIDER_FAILURE_BACKOFF_SECONDS:=${SHOWY_QUOTA_REFRESH_SECONDS}}"
: "${SHOWY_QUOTA_PROVIDERS:=}"
: "${SHOWY_QUOTA_PROVIDERS_EXCLUDE:=}"
: "${SHOWY_QUOTA_PROVIDER_ORDER:=codex,claude,copilot,opencode,gemini}"
: "${SHOWY_QUOTA_INCLUDE_STATUS:=1}"

: "${SHOWY_QUOTA_PALETTE_PRIMARY_GOOD:=25be6a}"
: "${SHOWY_QUOTA_PALETTE_PRIMARY_WARN:=f0af00}"
: "${SHOWY_QUOTA_PALETTE_PRIMARY_BAD:=ee5396}"
: "${SHOWY_QUOTA_PALETTE_PRIMARY_UNKNOWN:=6c7086}"
: "${SHOWY_QUOTA_PALETTE_DIM_SCALE:=0.55}"
: "${SHOWY_QUOTA_DIM_WINDOW_MINUTES:=10080}"
: "${SHOWY_QUOTA_PALETTE_BG:=161616}"
: "${SHOWY_QUOTA_PALETTE_SURFACE:=2a2a2a}"
: "${SHOWY_QUOTA_PALETTE_TRACK:=3a3a4a}"
: "${SHOWY_QUOTA_PALETTE_ICON_TEXT:=f2f4f8}"
: "${SHOWY_QUOTA_PALETTE_COUNTDOWN:=7b8496}"
: "${SHOWY_QUOTA_PALETTE_COUNTDOWN_WARN:=${SHOWY_QUOTA_PALETTE_PRIMARY_BAD}}"
: "${SHOWY_QUOTA_PALETTE_STALE:=${SHOWY_QUOTA_PALETTE_PRIMARY_UNKNOWN}}"
: "${SHOWY_QUOTA_PALETTE_ELAPSED:=be95ff}"
: "${SHOWY_QUOTA_PALETTE_ELAPSED_LONG:=3ddbd9}"
: "${SHOWY_QUOTA_STALE_GLYPH:=⚠}"
: "${SHOWY_QUOTA_DEGRADED_CLI_GLYPH:=⚠cli}"

: "${SHOWY_QUOTA_GOOD_MIN_REMAINING:=40}"
: "${SHOWY_QUOTA_WARN_MIN_REMAINING:=15}"
: "${SHOWY_QUOTA_TIME_WARN_MINUTES:=30}"

: "${SHOWY_QUOTA_CODEXBAR_RESOURCES:=/Applications/CodexBar.app/Contents/Resources}"
: "${SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE:=${SHOWY_QUOTA_CACHE_DIR}/sketchybar}"
: "${SHOWY_QUOTA_SKETCHYBAR_CLICK:=open -b com.steipete.codexbar}"
: "${SHOWY_QUOTA_SKETCHYBAR_UPDATE_FREQ:=10}"
: "${SHOWY_QUOTA_PNG_BAR_W:=80}"
: "${SHOWY_QUOTA_PNG_BAR_H:=18}"
: "${SHOWY_QUOTA_SKETCHYBAR_ICON_WIDTH:=22}"
: "${SHOWY_QUOTA_SKETCHYBAR_ICON_PADDING_LEFT:=5}"
: "${SHOWY_QUOTA_SKETCHYBAR_ICON_SCALE:=0.28}"
: "${SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE:=svg}"
: "${SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT:=sketchybar-app-font:Regular:14.0}"
: "${SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT_PADDING_RIGHT:=2}"
: "${SHOWY_QUOTA_SKETCHYBAR_BAR_WIDTH:=$((SHOWY_QUOTA_PNG_BAR_W + 3))}"
: "${SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH:=32}"

: "${SHOWY_QUOTA_SKETCHYBAR_COMPACT_PROVIDER_COUNT:=5}"
: "${SHOWY_QUOTA_SKETCHYBAR_PILL_RADIUS:=14}"
: "${SHOWY_QUOTA_SKETCHYBAR_PILL_HEIGHT:=28}"
: "${SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR:=0xcc24273a}"
: "${SHOWY_QUOTA_ZELLIJ_WIDGET:=pipe_showy_quota}"
: "${SHOWY_QUOTA_ZELLIJ_PIPE_NAME:=showy-quota}"
: "${SHOWY_QUOTA_ZELLIJ_PIPE_INTERVAL:=10}"
: "${SHOWY_QUOTA_ZELLIJ_PIPE_TIMEOUT_TENTHS:=20}"
: "${SHOWY_QUOTA_ZELLIJ_BAR_WIDTH:=12}"
: "${SHOWY_QUOTA_TERMINAL_BAR_MODE:=auto}"
: "${SHOWY_QUOTA_PROVIDER_MODES:=gemini=mono3,antigravity=mono3}"
: "${SHOWY_QUOTA_MONO_COLOR_MODE:=lowest}"
: "${SHOWY_QUOTA_MONO_MARKERS:=primary}"
: "${SHOWY_QUOTA_ZELLIJ_BIN:=zellij}"
: "${SHOWY_QUOTA_ZELLIJ_PLUGIN:=}"
: "${SHOWY_QUOTA_USAGE_FILE:=${SHOWY_QUOTA_CACHE_DIR}/usage.json}"
: "${SHOWY_QUOTA_USAGE_STAMP:=${SHOWY_QUOTA_CACHE_DIR}/usage.json.updated-at}"
: "${SHOWY_QUOTA_SOURCE_FILE:=${SHOWY_QUOTA_CACHE_DIR}/source}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_PID_FILE:=${SHOWY_QUOTA_CACHE_DIR}/codexbar-serve.pid}"
: "${SHOWY_QUOTA_USAGE_LOCK:=${SHOWY_QUOTA_CACHE_DIR}/usage.lock}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_FAILURE_STAMP:=${SHOWY_QUOTA_CACHE_DIR}/serve-failed-at}"
: "${SHOWY_QUOTA_CODEXBAR_SERVE_FAILURE_COUNT_FILE:=${SHOWY_QUOTA_CACHE_DIR}/serve-failed-count}"
: "${SHOWY_QUOTA_CODEXBAR_CLI_FAILURE_STAMP:=${SHOWY_QUOTA_CACHE_DIR}/cli-failed-at}"
: "${SHOWY_QUOTA_CODEXBAR_CONFIG_PROVIDERS_FAILURE_STAMP:=${SHOWY_QUOTA_CACHE_DIR}/config-providers-failed-at}"
: "${SHOWY_QUOTA_PROVIDER_FAILURE_DIR:=${SHOWY_QUOTA_CACHE_DIR}/provider-failures}"

declare -gA SHOWY_QUOTA_ROLE_PALETTE_CACHE=()

# ── small utilities ────────────────────────────────────────────────────

showy_quota_log() {
    [[ "${SHOWY_QUOTA_DEBUG:-0}" == "1" ]] || return 0
    printf '[showy-quota] %s\n' "$*" >&2
}

showy_quota_die() {
    printf 'showy-quota: %s\n' "$*" >&2
    exit 1
}

showy_quota_have() { command -v "$1" >/dev/null 2>&1; }

showy_quota_now_epoch() {
    if [[ -n "${SHOWY_QUOTA_NOW_EPOCH:-}" && "${SHOWY_QUOTA_NOW_EPOCH}" =~ ^[0-9]+$ ]]; then
        printf '%s\n' "${SHOWY_QUOTA_NOW_EPOCH}"
    else
        date +%s
    fi
}

showy_quota_age_seconds() {
    # Seconds since file mtime; prints '999999999' when missing.
    local path="$1"
    [[ -f "${path}" ]] || { printf '999999999\n'; return; }
    local now mtime
    now=$(showy_quota_now_epoch)
    if mtime=$(stat -f %m "${path}" 2>/dev/null); then :
    elif mtime=$(stat -c %Y "${path}" 2>/dev/null); then :
    else mtime="${now}"; fi
    printf '%s\n' $((now - mtime))
}

showy_quota_cache_source() {
    local source="unknown"
    if [[ -r "${SHOWY_QUOTA_SOURCE_FILE}" ]]; then
        IFS= read -r source < "${SHOWY_QUOTA_SOURCE_FILE}" || source="unknown"
    fi
    case "${source}" in
        serve|cli) printf '%s\n' "${source}" ;;
        *)         printf 'unknown\n' ;;
    esac
}

showy_quota_cache_degraded_cli() {
    [[ "$(showy_quota_cache_source)" == "cli" ]]
}

# Emit provider ids (one per line, deduplicated, original order preserved) from
# a CodexBar usage payload. Validates each id with the same regex as the JSON
# schema check so unsafe ids never leak into argv.
showy_quota_provider_ids_from_payload() {
    local file="$1"
    [[ -s "${file}" ]] || return 1
    showy_quota_have jq || return 1
    jq -r '
        if type == "array" then
            reduce .[] as $r (
                [];
                if ($r.provider? | type == "string")
                   and ($r.provider | test("^[A-Za-z0-9_.-]+$"))
                   and (index($r.provider) == null)
                then . + [$r.provider]
                else .
                end
            ) | .[]
        else empty end
    ' "${file}" 2>/dev/null
}

showy_quota_stale_after_seconds() { printf '%s\n' $((SHOWY_QUOTA_REFRESH_SECONDS * 2)); }

showy_quota_cache_stale_for() {
    local age
    age=$(showy_quota_age_seconds "$1")
    (( age > SHOWY_QUOTA_REFRESH_SECONDS * 2 ))
}

showy_quota_parse_local_epoch() {
    local fmt="$1" value="$2"
    if date -j -f "${fmt}" "${value}" '+%s' 2>/dev/null; then
        return 0
    fi
    if showy_quota_have gdate; then
        gdate -d "${value}" '+%s' 2>/dev/null && return 0
    fi
    date -d "${value}" '+%s' 2>/dev/null
}

showy_quota_reset_description_epoch() {
    local raw="$1"
    [[ -n "${raw}" && "${raw}" != "null" ]] || return 1
    local desc="${raw#Resets }"
    [[ "${desc}" != "${raw}" ]] || desc="${raw#resets }"
    [[ "${desc}" != "${raw}" ]] || return 1

    local epoch
    for fmt in '%b %d, %Y %I:%M %p' '%B %d, %Y %I:%M %p'; do
        if epoch=$(showy_quota_parse_local_epoch "${fmt}" "${desc}"); then
            printf '%s\n' "${epoch}"
            return 0
        fi
    done

    local today now
    today=$(date '+%Y-%m-%d')
    if epoch=$(showy_quota_parse_local_epoch '%Y-%m-%d %I:%M %p' "${today} ${desc}"); then
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
showy_quota_reset_epoch() {
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
    if showy_quota_have gdate; then
        gdate -d "${raw}" '+%s' 2>/dev/null && return 0
    fi
    if date -d "${raw}" '+%s' 2>/dev/null; then
        return 0
    fi
    showy_quota_reset_description_epoch "${raw}" && return 0
    return 1
}

# Minutes (rounded down) until reset. Prints '' on failure, '0' if already
# past.
showy_quota_minutes_until() {
    local reset_at="$1"
    local epoch
    epoch=$(showy_quota_reset_epoch "${reset_at}") || return 1
    local now
    now=$(showy_quota_now_epoch)
    local diff=$(( epoch - now ))
    (( diff < 0 )) && diff=0
    printf '%s\n' $((diff / 60))
}

# Compact countdown: 'now' / '12m' / '3h' / '3:45' / '2d' / '5w'.
showy_quota_format_countdown() {
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

showy_quota_primary_label() {
    local minutes="$1" remaining="$2" reset_value="${3:-}"
    if [[ -n "${minutes}" ]]; then
        showy_quota_format_countdown "${minutes}"
        return
    fi
    if [[ -z "${reset_value}" && "${remaining}" =~ ^-?[0-9]+$ ]] && (( remaining >= 100 )); then
        printf 'idle'
        return
    fi
    showy_quota_format_countdown "${minutes}"
}

# Map remaining-percent → palette key (good|warn|bad|unknown).
showy_quota_color_key() {
    local remaining="$1"
    [[ "${remaining}" =~ ^-?[0-9]+([.][0-9]+)?$ ]] || { printf 'unknown'; return; }
    remaining="${remaining%%.*}"
    [[ "${remaining}" == "-0" ]] && remaining=0
    if (( remaining >= SHOWY_QUOTA_GOOD_MIN_REMAINING )); then printf 'good'
    elif (( remaining >= SHOWY_QUOTA_WARN_MIN_REMAINING )); then printf 'warn'
    else printf 'bad'
    fi
}

showy_quota_scale_component() {
    local value="$1" factor_num="$2" factor_den="$3"
    local scaled=$(( (value * factor_num) / factor_den ))
    (( scaled < 0 )) && scaled=0
    (( scaled > 255 )) && scaled=255
    printf '%02x' "${scaled}"
}

showy_quota_normalize_hex() {
    local hex="$1"
    hex="${hex#\#}"
    [[ "${hex}" =~ ^[[:xdigit:]]{6}$ ]] || showy_quota_die "invalid palette hex: $1"
    printf '%s' "${hex,,}"
}

showy_quota_scale_hex() {
    local hex="$1"
    local factor="${2:-1}"
    hex="$(showy_quota_normalize_hex "${hex}")"
    [[ "${factor}" =~ ^([0-9]+([.][0-9]+)?|[.][0-9]+)$ ]] || showy_quota_die "invalid palette scale: ${factor}"

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
        "$(showy_quota_scale_component "${r}" "${factor_num}" "${factor_den}")" \
        "$(showy_quota_scale_component "${g}" "${factor_num}" "${factor_den}")" \
        "$(showy_quota_scale_component "${b}" "${factor_num}" "${factor_den}")"
}

# Hex color (no '#') for a global palette token.
showy_quota_palette() {
    local value
    case "$1" in
        bg)      value="${SHOWY_QUOTA_PALETTE_BG}" ;;
        surface) value="${SHOWY_QUOTA_PALETTE_SURFACE}" ;;
        track)   value="${SHOWY_QUOTA_PALETTE_TRACK}" ;;
        icon_text)      value="${SHOWY_QUOTA_PALETTE_ICON_TEXT}" ;;
        countdown)      value="${SHOWY_QUOTA_PALETTE_COUNTDOWN}" ;;
        countdown_warn) value="${SHOWY_QUOTA_PALETTE_COUNTDOWN_WARN}" ;;
        stale)          value="${SHOWY_QUOTA_PALETTE_STALE}" ;;
        elapsed)        value="${SHOWY_QUOTA_PALETTE_ELAPSED}" ;;
        elapsed_long)   value="${SHOWY_QUOTA_PALETTE_ELAPSED_LONG}" ;;
        *)       showy_quota_die "unknown global palette token: $1" ;;
    esac
    showy_quota_normalize_hex "${value}"
}

# Hex color (no '#') for the primary palette at a severity.
showy_quota_primary_palette() {
    local severity="$1"
    local cache_key="primary:${severity}" severity_upper var_name result
    if [[ -n "${SHOWY_QUOTA_ROLE_PALETTE_CACHE[${cache_key}]+x}" ]]; then
        printf '%s' "${SHOWY_QUOTA_ROLE_PALETTE_CACHE[${cache_key}]}"
        return 0
    fi
    case "${severity}" in
        good|warn|bad|unknown) severity_upper="${severity^^}" ;;
        *) showy_quota_die "unknown palette severity: ${severity}" ;;
    esac
    var_name="SHOWY_QUOTA_PALETTE_PRIMARY_${severity_upper}"
    result="$(showy_quota_normalize_hex "${!var_name}")"
    SHOWY_QUOTA_ROLE_PALETTE_CACHE["${cache_key}"]="${result}"
    printf '%s' "${result}"
}

# Hex color (no '#') for the dimmed long-horizon (weekly/monthly cap) palette at
# a severity: explicit SHOWY_QUOTA_PALETTE_DIM_<SEV> override, otherwise the
# primary palette scaled by SHOWY_QUOTA_PALETTE_DIM_SCALE.
showy_quota_dim_palette() {
    local severity="$1"
    local cache_key="dim:${severity}" severity_upper override_var primary_var result
    if [[ -n "${SHOWY_QUOTA_ROLE_PALETTE_CACHE[${cache_key}]+x}" ]]; then
        printf '%s' "${SHOWY_QUOTA_ROLE_PALETTE_CACHE[${cache_key}]}"
        return 0
    fi
    case "${severity}" in
        good|warn|bad|unknown) severity_upper="${severity^^}" ;;
        *) showy_quota_die "unknown palette severity: ${severity}" ;;
    esac
    override_var="SHOWY_QUOTA_PALETTE_DIM_${severity_upper}"
    if [[ -n "${!override_var:-}" ]]; then
        result="${!override_var}"
    else
        primary_var="SHOWY_QUOTA_PALETTE_PRIMARY_${severity_upper}"
        result="$(showy_quota_scale_hex "${!primary_var}" "${SHOWY_QUOTA_PALETTE_DIM_SCALE}")"
    fi
    result="$(showy_quota_normalize_hex "${result}")"
    SHOWY_QUOTA_ROLE_PALETTE_CACHE["${cache_key}"]="${result}"
    printf '%s' "${result}"
}

# Is a window a long-horizon cap (weekly/monthly)? Args: $1 = windowMinutes.
# A window dims only when its horizon is at or beyond SHOWY_QUOTA_DIM_WINDOW_MINUTES.
showy_quota_is_long_window() {
    local window_minutes="$1"
    if [[ "${window_minutes}" =~ ^[0-9]+$ ]] && (( window_minutes >= SHOWY_QUOTA_DIM_WINDOW_MINUTES )); then
        printf '1'
    else
        printf '0'
    fi
}

# Hex color for a usage window: severity palette of its remaining percent,
# dimmed when the window is a long-horizon cap. Args: $1 = remaining, $2 = is_long.
showy_quota_window_color() {
    local remaining="$1" is_long="${2:-0}" severity
    severity="$(showy_quota_color_key "${remaining}")"
    if [[ "${is_long}" == "1" ]]; then
        showy_quota_dim_palette "${severity}"
    else
        showy_quota_primary_palette "${severity}"
    fi
}

# Validate that codexbar JSON looks like an array of provider objects.
showy_quota_json_valid() {
    local file="$1"
    [[ -s "${file}" ]] || return 1
    showy_quota_have jq || return 1
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
