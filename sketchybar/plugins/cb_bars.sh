#!/usr/bin/env bash
# codexbar-bars — SketchyBar plugin: render per-provider icon + bar PNGs
# and update each provider's items.
#
# Invoked by the cb_bars.trigger item every CB_BARS_SKETCHYBAR_UPDATE_FREQ
# seconds. Reads the shared codexbar JSON cache, generates a small PNG
# strip per provider (track + primary + secondary [+ tertiary]), and
# writes it to the user's image cache. SketchyBar then reads the PNG by
# absolute path.

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
    cd -- "${dir}/../.." && pwd -P
}
REPO_ROOT="$(resolve_repo_root)"

# shellcheck disable=SC1091
. "${REPO_ROOT}/lib/common.sh"
# shellcheck disable=SC1091
. "${REPO_ROOT}/lib/strip.sh"

FETCH="${CB_BARS_FETCH_BIN:-${REPO_ROOT}/bin/cb-bars-fetch}"
CACHE_DIR="${CB_BARS_SKETCHYBAR_IMAGE_CACHE}"
mkdir -p "${CACHE_DIR}" || exit 0
STATE_FILE="${CACHE_DIR}/providers.txt"

hide_provider() {
    local pid="$1"
    sketchybar \
        --set "cb_bars.${pid}.icon" drawing=off \
        --set "cb_bars.${pid}.bar" drawing=off \
        --set "cb_bars.${pid}.label" drawing=off label="" >/dev/null 2>&1 || true
}

hide_state_providers() {
    [[ -f "${STATE_FILE}" ]] || return 0
    while IFS= read -r old_pid; do
        [[ -n "${old_pid}" ]] || continue
        hide_provider "${old_pid}"
    done < "${STATE_FILE}"
}

cb_bars_have jq || {
    cb_bars_log "jq required for sketchybar plugin"
    hide_state_providers
    exit 0
}
cb_bars_have magick || {
    cb_bars_log "magick (ImageMagick 7+) required for sketchybar plugin"
    hide_state_providers
    exit 0
}

# Bar geometry. Bars sit inside SketchyBar's pill; tweak via env.
: "${CB_BARS_PNG_BAR_W:=80}"
: "${CB_BARS_PNG_BAR_H:=18}"

# ── ARGB helpers ─────────────────────────────────────────────────────

# 6-char hex (no '#') → 0xff RRGGBB SketchyBar literal.
argb_from_hex() { printf '0xff%s' "$1"; }

# 6-char hex → '#RRGGBB' for ImageMagick.
mhex() { printf '#%s' "$1"; }

GOOD_HEX="$(cb_bars_palette good)"
WARN_HEX="$(cb_bars_palette warn)"
BAD_HEX="$(cb_bars_palette bad)"
UNKNOWN_HEX="$(cb_bars_palette unknown)"
TRACK_HEX="$(cb_bars_palette track)"
TEXT_ARGB="$(argb_from_hex "$(cb_bars_palette text)")"
ELAPSED_HEX="$(cb_bars_palette elapsed)"

color_for_remaining() {
    local rem="$1"
    case "$(cb_bars_color_key "${rem}")" in
        good) printf '%s' "${GOOD_HEX}" ;;
        warn) printf '%s' "${WARN_HEX}" ;;
        bad)  printf '%s' "${BAD_HEX}" ;;
        *)    printf '%s' "${UNKNOWN_HEX}" ;;
    esac
}

status_color_for_indicator() {
    case "${1:-none}" in
        minor|maintenance) printf '%s' "${WARN_HEX}" ;;
        major|critical)    printf '%s' "${BAD_HEX}" ;;
        unknown)           printf '%s' "${UNKNOWN_HEX}" ;;
        *)                 return 1 ;;
    esac
}

# ── provider icon: lazily render SVG → PNG ───────────────────────────
render_fallback_icon_png() {
    local pid="$1" tmp="$2"
    local sigil
    sigil=$(cb_bars_provider_sigil "${pid}")
    magick -size 64x64 xc:none \
        -fill "$(mhex "${UNKNOWN_HEX}")" \
        -draw "circle 32,32 32,4" \
        -fill "$(mhex "$(cb_bars_palette text)")" \
        -gravity center -pointsize 28 -annotate 0 "${sigil}" \
        "PNG32:${tmp}" >/dev/null 2>&1
}


provider_icon_png() {
    local pid="$1" status="${2:-none}"
    local status_color="" suffix="" out
    if status_color=$(status_color_for_indicator "${status}"); then
        suffix="-${status}"
    fi
    out="${CACHE_DIR}/icon-${pid}${suffix}.png"
    [[ -s "${out}" ]] && { printf '%s\n' "${out}"; return 0; }

    # Per-process tmp files in the same directory so `mv` is atomic.
    local tmp normal_tmp
    normal_tmp=$(mktemp "${CACHE_DIR}/.icon-${pid}.normal.XXXXXX") || return 1

    local svg="${CB_BARS_CODEXBAR_RESOURCES}/ProviderIcon-${pid}.svg"
    if [[ ! -r "${svg}" ]]; then
        if ! render_fallback_icon_png "${pid}" "${normal_tmp}"; then
            rm -f "${normal_tmp}"; return 1
        fi
    else
        if ! magick -background none -density 300 "${svg}" \
                    -resize 64x64 "PNG32:${normal_tmp}" >/dev/null 2>&1; then
            if ! render_fallback_icon_png "${pid}" "${normal_tmp}"; then
                rm -f "${normal_tmp}"; return 1
            fi
        fi
    fi

    if [[ -n "${status_color}" ]]; then
        tmp=$(mktemp "${CACHE_DIR}/.icon-${pid}.status.XXXXXX") || { rm -f "${normal_tmp}"; return 1; }
        if ! magick "${normal_tmp}" -alpha extract \
                    -background "$(mhex "${status_color}")" -alpha shape \
                    "PNG32:${tmp}" >/dev/null 2>&1; then
            rm -f "${tmp}"
            tmp="${normal_tmp}"
        else
            rm -f "${normal_tmp}"
        fi
    else
        tmp="${normal_tmp}"
    fi

    mv -f "${tmp}" "${out}"
    printf '%s\n' "${out}"
}

# ── stacked-bar PNG ──────────────────────────────────────────────────

# Args: provider primary_remaining secondary_remaining tertiary_remaining
#       secondary_elapsed_x tertiary_elapsed_x
# Pass '' (empty) for missing windows/markers. Echoes path on success.
render_bar_png() {
    local pid="$1" rem_p="$2" rem_s="$3" rem_t="$4" marker_s="${5:-}" marker_t="${6:-}"
    local out="${CACHE_DIR}/bar-${pid}.png"

    local has_t=0
    [[ -n "${rem_t}" ]] && has_t=1

    local rows=2
    (( has_t )) && rows=3

    local image_h="${CB_BARS_PNG_BAR_H}"
    if (( rows == 3 )); then
        # Allow a slightly taller image when stacking three rows. 22 px gives
        # us three 6 px bars with a 1 px gap between each row.
        image_h=22
    fi

    # Row boundaries (top..bottom inclusive) per row count.
    local r1_top r1_bot r2_top r2_bot r3_top r3_bot
    if (( rows == 2 )); then
        r1_top=2; r1_bot=8
        r2_top=10; r2_bot=$(( image_h - 2 ))
    else
        r1_top=1; r1_bot=6
        r2_top=8; r2_bot=13
        r3_top=15; r3_bot=20
    fi

    # Width fill (round half-up; never blank for nonzero remaining).
    fill_w() {
        local pct="${1:-0}"
        local w="${CB_BARS_PNG_BAR_W}"
        awk -v p="${pct}" -v w="${w}" 'BEGIN {
            f = int((p / 100.0) * w + 0.5);
            if (p > 0 && f == 0) f = 1;
            if (f < 0) f = 0; if (f > w) f = w;
            print f;
        }'
    }

    local f1 f2 f3
    f1=$(fill_w "${rem_p}")
    f2=$(fill_w "${rem_s}")
    (( has_t )) && f3=$(fill_w "${rem_t}")

    local c1 c2 c3
    c1=$(color_for_remaining "${rem_p}")
    c2=$(color_for_remaining "${rem_s}")
    (( has_t )) && c3=$(color_for_remaining "${rem_t}")

    local args=( -size "${CB_BARS_PNG_BAR_W}x${image_h}" xc:none )

    add_track() {
        args+=( -fill "$(mhex "${TRACK_HEX}")"
                -draw "roundrectangle 0,$1 $((CB_BARS_PNG_BAR_W - 1)),$2 3,3" )
    }
    add_fill() {
        local fill="$1" top="$2" bot="$3" hex="$4"
        (( fill > 0 )) || return 0
        args+=( -fill "$(mhex "${hex}")"
                -draw "roundrectangle 0,${top} $((fill - 1)),${bot} 3,3" )
    }
    add_marker() {
        local marker="$1" top="$2" bot="$3"
        [[ "${marker}" =~ ^[0-9]+$ ]] || return 0
        (( marker < 0 )) && marker=0
        (( marker >= CB_BARS_PNG_BAR_W )) && marker=$((CB_BARS_PNG_BAR_W - 1))
        args+=( -fill "$(mhex "${ELAPSED_HEX}")"
                -draw "rectangle ${marker},${top} ${marker},${bot}" )
    }


    add_track "${r1_top}" "${r1_bot}"
    add_fill "${f1}" "${r1_top}" "${r1_bot}" "${c1}"
    add_track "${r2_top}" "${r2_bot}"
    add_fill "${f2}" "${r2_top}" "${r2_bot}" "${c2}"
    if (( has_t )); then
        add_track "${r3_top}" "${r3_bot}"
        add_fill "${f3}" "${r3_top}" "${r3_bot}" "${c3}"
    fi
    add_marker "${marker_s}" "${r2_top}" "${r2_bot}"
    if (( has_t )); then
        add_marker "${marker_t}" "${r3_top}" "${r3_bot}"
    fi

    local tmp
    tmp=$(mktemp "${CACHE_DIR}/.bar-${pid}.XXXXXX") || return 1
    if magick "${args[@]}" "PNG32:${tmp}" >/dev/null 2>&1; then
        if [[ ! -f "${out}" ]] || ! cmp -s "${tmp}" "${out}"; then
            mv -f "${tmp}" "${out}"
        else
            rm -f "${tmp}"
        fi
        printf '%s\n' "${out}"
        return 0
    fi
    rm -f "${tmp}"
    return 1
}

# ── label rendering ──────────────────────────────────────────────────

label_for_minutes() {
    local minutes="$1" remaining="$2" reset_value="${3:-}"
    local label
    label=$(cb_bars_primary_label "${minutes}" "${remaining}" "${reset_value}")
    local color="${TEXT_ARGB}"
    if [[ -n "${minutes}" && "${minutes}" -lt "${CB_BARS_TIME_WARN_MINUTES}" ]] 2>/dev/null; then
        color=$(argb_from_hex "${BAD_HEX}")
    fi
    printf '%s\n%s\n' "${label}" "${color}"
}

# ── main ─────────────────────────────────────────────────────────────

data=$("${FETCH}" 2>/dev/null || printf '[]')

elapsed_marker_x() {
    local reset_at="$1" window_minutes="$2"
    [[ -n "${reset_at}" && "${window_minutes}" =~ ^[0-9]+$ ]] || return 1
    (( window_minutes > 0 )) || return 1

    local reset_epoch duration start_epoch now elapsed marker
    reset_epoch=$(cb_bars_reset_epoch "${reset_at}") || return 1
    duration=$((window_minutes * 60))
    start_epoch=$((reset_epoch - duration))
    now=$(date +%s)
    elapsed=$((now - start_epoch))
    (( elapsed < 0 )) && elapsed=0
    (( elapsed > duration )) && elapsed="${duration}"
    marker=$(( (duration - elapsed) * CB_BARS_PNG_BAR_W / duration ))
    (( marker < 0 )) && marker=0
    (( marker >= CB_BARS_PNG_BAR_W )) && marker=$((CB_BARS_PNG_BAR_W - 1))
    printf '%s\n' "${marker}"
}
# Providers absent from this filtered row set are hidden below, so stale
# SketchyBar items do not keep showing old quota data.
filtered=$(printf '%s' "${data}" | cb_bars_filter_renderable)
rows=$(printf '%s' "${filtered}" | jq -r '
    def pct(x): if x == null then 0 else ([0, ([100, (x|tonumber|floor)] | min)] | max) end;
    .[] | [
        .provider,
        (100 - pct(.usage.primary.usedPercent)),
        (.usage.primary.resetsAt // .usage.primary.resetDescription // ""),
        (if .usage.secondary then (100 - pct(.usage.secondary.usedPercent)) else "" end),
        (.usage.secondary.resetsAt // .usage.secondary.resetDescription // ""),
        (.usage.secondary.windowMinutes // ""),
        (if .usage.tertiary  then (100 - pct(.usage.tertiary.usedPercent))  else "" end),
        (.usage.tertiary.resetsAt // .usage.tertiary.resetDescription // ""),
        (.usage.tertiary.windowMinutes // ""),
        (.status.indicator // "none")
    ] | map(tostring) | join("\u001f")')
current_providers=$'\n'


while IFS=$'\x1f' read -r pid rem_p p_reset rem_s s_reset s_window rem_t t_reset t_window status; do
    [[ -n "${pid}" ]] || continue

    icon=$(provider_icon_png "${pid}" "${status}" || true)
    marker_s=$(elapsed_marker_x "${s_reset}" "${s_window}" || true)
    marker_t=$(elapsed_marker_x "${t_reset}" "${t_window}" || true)
    bar=$(render_bar_png "${pid}" "${rem_p}" "${rem_s}" "${rem_t}" "${marker_s}" "${marker_t}" || true)

    minutes=""
    if [[ -n "${p_reset}" ]]; then
        minutes=$(cb_bars_minutes_until "${p_reset}" || true)
    fi
    label=""; color=""
    { IFS= read -r label; IFS= read -r color; } < <(label_for_minutes "${minutes}" "${rem_p}" "${p_reset}") || true
    [[ -n "${color}" ]] || color="${TEXT_ARGB}"

    current_providers+="${pid}"$'\n'

    args=(
        --set "cb_bars.${pid}.label" drawing=on label="${label}" label.color="${color}" background.color=0x00000000 background.height=0
    )
    if [[ -n "${icon}" && -s "${icon}" ]]; then
        args+=( --set "cb_bars.${pid}.icon" drawing=on background.image="${icon}" background.image.drawing=on background.image.scale="${CB_BARS_SKETCHYBAR_ICON_SCALE}" background.color=0x00000000 background.height=0 padding_left="${CB_BARS_SKETCHYBAR_ICON_PADDING_LEFT}" padding_right=0 width="${CB_BARS_SKETCHYBAR_ICON_WIDTH}" )
    else
        args+=( --set "cb_bars.${pid}.icon" drawing=off )
    fi
    if [[ -n "${bar}" && -s "${bar}" ]]; then
        args+=( --set "cb_bars.${pid}.bar" drawing=on background.image="${bar}" background.image.drawing=on background.image.scale=1.0 background.color=0x00000000 background.height=0 padding_left=2 padding_right=2 width="${CB_BARS_SKETCHYBAR_BAR_WIDTH}" )
    else
        args+=( --set "cb_bars.${pid}.bar" drawing=off )
    fi

    if (( ${#args[@]} > 0 )); then
        sketchybar "${args[@]}" >/dev/null 2>&1 || true
    fi
done <<< "${rows}"

if [[ -f "${STATE_FILE}" ]]; then
    while IFS= read -r old_pid; do
        [[ -n "${old_pid}" ]] || continue
        case "${current_providers}" in
            *$'\n'"${old_pid}"$'\n'*) ;;
            *) hide_provider "${old_pid}" ;;
        esac
    done < "${STATE_FILE}"
fi

state_tmp=$(mktemp "${CACHE_DIR}/.providers.XXXXXX") || exit 0
printf '%s' "${current_providers}" | sed '/^$/d' > "${state_tmp}"
mv -f "${state_tmp}" "${STATE_FILE}"
