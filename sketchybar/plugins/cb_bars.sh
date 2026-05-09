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

cb_bars_have jq || exit 0
cb_bars_have magick || {
    cb_bars_log "magick (ImageMagick 7+) required for sketchybar plugin"
    exit 0
}

FETCH="${CB_BARS_FETCH_BIN:-${REPO_ROOT}/bin/cb-bars-fetch}"
CACHE_DIR="${CB_BARS_SKETCHYBAR_IMAGE_CACHE}"
mkdir -p "${CACHE_DIR}"

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

color_for_remaining() {
    local rem="$1"
    case "$(cb_bars_color_key "${rem}")" in
        good) printf '%s' "${GOOD_HEX}" ;;
        warn) printf '%s' "${WARN_HEX}" ;;
        bad)  printf '%s' "${BAD_HEX}" ;;
        *)    printf '%s' "${UNKNOWN_HEX}" ;;
    esac
}

# ── provider icon: lazily render SVG → PNG ───────────────────────────

provider_icon_png() {
    local pid="$1"
    local out="${CACHE_DIR}/icon-${pid}.png"
    [[ -s "${out}" ]] && { printf '%s\n' "${out}"; return 0; }

    # Per-process tmp file in the same directory so `mv` is atomic.
    local tmp
    tmp=$(mktemp "${CACHE_DIR}/.icon-${pid}.XXXXXX") || return 1

    local svg="${CB_BARS_CODEXBAR_RESOURCES}/ProviderIcon-${pid}.svg"
    if [[ ! -r "${svg}" ]]; then
        # Render a small grey circle with sigil text as a fallback.
        local sigil
        sigil=$(cb_bars_provider_sigil "${pid}")
        if ! magick -size 64x64 xc:none \
                    -fill "$(mhex "${UNKNOWN_HEX}")" \
                    -draw "circle 32,32 32,4" \
                    -fill "$(mhex "$(cb_bars_palette text)")" \
                    -gravity center -pointsize 28 -annotate 0 "${sigil}" \
                    "PNG32:${tmp}" >/dev/null 2>&1; then
            rm -f "${tmp}"; return 1
        fi
    else
        if ! magick -background none -density 300 "${svg}" \
                    -resize 64x64 "PNG32:${tmp}" >/dev/null 2>&1; then
            rm -f "${tmp}"; return 1
        fi
    fi
    mv -f "${tmp}" "${out}"
    printf '%s\n' "${out}"
}

# ── stacked-bar PNG ──────────────────────────────────────────────────

# Args: provider primary_remaining secondary_remaining tertiary_remaining
# Pass '' (empty) for missing windows. Echoes path on success.
render_bar_png() {
    local pid="$1" rem_p="$2" rem_s="$3" rem_t="$4"
    local out="${CACHE_DIR}/bar-${pid}.png"

    local has_t=0
    [[ -n "${rem_t}" ]] && has_t=1

    local rows=2
    (( has_t )) && rows=3

    local image_h="${CB_BARS_PNG_BAR_H}"
    if (( rows == 3 )); then
        # Allow a slightly taller image when stacking three rows. Snap to
        # 22 px which leaves 7+7+7 = 21 px usable (+1 px corner padding).
        image_h=22
    fi

    # Row boundaries (top..bottom inclusive) per row count. Rows are
    # contiguous so there is no visible vertical gap.
    local r1_top r1_bot r2_top r2_bot r3_top r3_bot
    if (( rows == 2 )); then
        r1_top=2; r1_bot=8
        r2_top=10; r2_bot=$(( image_h - 2 ))
    else
        r1_top=1; r1_bot=7
        r2_top=8; r2_bot=14
        r3_top=15; r3_bot=$(( image_h - 1 ))
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

    add_track "${r1_top}" "${r1_bot}"
    add_fill "${f1}" "${r1_top}" "${r1_bot}" "${c1}"
    add_track "${r2_top}" "${r2_bot}"
    add_fill "${f2}" "${r2_top}" "${r2_bot}" "${c2}"
    if (( has_t )); then
        add_track "${r3_top}" "${r3_bot}"
        add_fill "${f3}" "${r3_top}" "${r3_bot}" "${c3}"
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
    local minutes="$1"
    local label
    label=$(cb_bars_format_countdown "${minutes}")
    local color="${TEXT_ARGB}"
    if [[ -n "${minutes}" && "${minutes}" -lt "${CB_BARS_TIME_WARN_MINUTES}" ]] 2>/dev/null; then
        color=$(argb_from_hex "${BAD_HEX}")
    fi
    printf '%s\n%s\n' "${label}" "${color}"
}

# ── main ─────────────────────────────────────────────────────────────

data=$("${FETCH}" 2>/dev/null || printf '[]')

# We only render providers that have usage.primary; everything else gets
# implicitly hidden because it was never registered as an item.
rows=$(printf '%s' "${data}" | jq -r '
    def pct(x): if x == null then 0 else ([0, ([100, (x|tonumber|floor)] | min)] | max) end;
    [ .[] | select((.error // null) == null and (.usage.primary // null) != null) ]
    | .[] | [
        .provider,
        (100 - pct(.usage.primary.usedPercent // 0)),
        (.usage.primary.resetsAt // ""),
        (if .usage.secondary then (100 - pct(.usage.secondary.usedPercent)) else "" end),
        (if .usage.tertiary  then (100 - pct(.usage.tertiary.usedPercent))  else "" end)
    ] | map(tostring) | join("\u001f")')

while IFS=$'\x1f' read -r pid rem_p p_reset rem_s rem_t; do
    [[ -n "${pid}" ]] || continue

    icon=$(provider_icon_png "${pid}" || true)
    bar=$(render_bar_png "${pid}" "${rem_p}" "${rem_s}" "${rem_t}" || true)

    minutes=""
    if [[ -n "${p_reset}" ]]; then
        minutes=$(cb_bars_minutes_until "${p_reset}" || true)
    fi
    label=""; color=""
    { IFS= read -r label; IFS= read -r color; } < <(label_for_minutes "${minutes}") || true
    [[ -n "${color}" ]] || color="${TEXT_ARGB}"

    args=()
    if [[ -n "${icon}" && -s "${icon}" ]]; then
        args+=( --set "cb_bars.${pid}.icon" background.image="${icon}" )
    fi
    if [[ -n "${bar}" && -s "${bar}" ]]; then
        args+=( --set "cb_bars.${pid}.bar" background.image="${bar}" )
    fi
    args+=( --set "cb_bars.${pid}.label" label="${label}" label.color="${color}" )

    if (( ${#args[@]} > 0 )); then
        sketchybar "${args[@]}" >/dev/null 2>&1 || true
    fi
done <<< "${rows}"
