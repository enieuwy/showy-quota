#!/usr/bin/env bash
# showy-bar — SketchyBar plugin: render per-provider icon + bar PNGs
# and update each provider's items.
#
# Invoked by the showy_bar.trigger item every SHOWY_BAR_SKETCHYBAR_UPDATE_FREQ
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

FETCH="${SHOWY_BAR_FETCH_BIN:-${REPO_ROOT}/bin/showy-bar-fetch}"
CACHE_DIR="${SHOWY_BAR_SKETCHYBAR_IMAGE_CACHE}"
mkdir -p "${CACHE_DIR}" || exit 0
STATE_FILE="${CACHE_DIR}/providers.txt"
CLICK="${SHOWY_BAR_SKETCHYBAR_CLICK}"

read_state_providers() {
    [[ -f "${STATE_FILE}" ]] || return 0
    while IFS= read -r pid || [[ -n "${pid}" ]]; do
        [[ -n "${pid}" ]] || continue
        printf '%s\n' "${pid}"
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
    local providers="${1-}" state_tmp
    state_tmp=$(mktemp "${CACHE_DIR}/.providers.XXXXXX") || return 1
    if [[ -n "${providers}" ]]; then
        printf '%s\n' "${providers}" > "${state_tmp}"
    else
        : > "${state_tmp}"
    fi
    mv -f "${state_tmp}" "${STATE_FILE}"
}

remove_provider_items() {
    local pid="$1"
    sketchybar \
        --remove "showy_bar.${pid}.icon" \
        --remove "showy_bar.${pid}.bar" \
        --remove "showy_bar.${pid}.label" >/dev/null 2>&1 || true
}

declare_provider_items() {
    local pid="$1"
    sketchybar --add item "showy_bar.${pid}.icon" left \
               --set "showy_bar.${pid}.icon" \
                   icon.drawing=off \
                   label.drawing=off \
                   background.image="showy_bar.${pid}.icon" \
                   background.image.drawing=on \
                   background.image.scale="${SHOWY_BAR_SKETCHYBAR_ICON_SCALE}" \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left="${SHOWY_BAR_SKETCHYBAR_ICON_PADDING_LEFT}" \
                   padding_right=0 \
                   width="${SHOWY_BAR_SKETCHYBAR_ICON_WIDTH}" \
                   click_script="${CLICK}" >/dev/null 2>&1 || true

    sketchybar --add item "showy_bar.${pid}.bar" left \
               --set "showy_bar.${pid}.bar" \
                   icon.drawing=off \
                   label.drawing=off \
                   background.image="showy_bar.${pid}.bar" \
                   background.image.drawing=on \
                   background.image.scale=1.0 \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left=2 \
                   padding_right=2 \
                   width="${SHOWY_BAR_SKETCHYBAR_BAR_WIDTH}" \
                   click_script="${CLICK}" >/dev/null 2>&1 || true

    sketchybar --add item "showy_bar.${pid}.label" left \
               --set "showy_bar.${pid}.label" \
                   icon.drawing=off \
                   label.font.size=11 \
                   label.padding_left=0 \
                   label.padding_right=4 \
                   background.color=0x00000000 \
                   background.height=0 \
                   click_script="${CLICK}" >/dev/null 2>&1 || true
}
sketchybar_item_exists() {
    sketchybar --query "$1" >/dev/null 2>&1
}

provider_items_declared() {
    local pid="$1"
    sketchybar_item_exists "showy_bar.${pid}.icon" \
        && sketchybar_item_exists "showy_bar.${pid}.bar" \
        && sketchybar_item_exists "showy_bar.${pid}.label"
}

declared_items_present() {
    local providers="${1-}" pid
    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        provider_items_declared "${pid}" || return 1
    done <<< "${providers}"
    [[ -z "${providers}" ]] || sketchybar_item_exists showy_bar_bracket
}


recreate_bracket() {
    local providers="${1-}" pid
    local bracket_items=()
    sketchybar --remove showy_bar_bracket >/dev/null 2>&1 || true

    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        bracket_items+=("showy_bar.${pid}.icon" "showy_bar.${pid}.bar" "showy_bar.${pid}.label")
    done <<< "${providers}"

    (( ${#bracket_items[@]} > 0 )) || return 0
    sketchybar --add bracket showy_bar_bracket "${bracket_items[@]}" \
               --set showy_bar_bracket \
                   background.color="${SHOWY_BAR_SKETCHYBAR_PILL_COLOR}" \
                   background.corner_radius="${SHOWY_BAR_SKETCHYBAR_PILL_RADIUS}" \
                   background.height="${SHOWY_BAR_SKETCHYBAR_PILL_HEIGHT}" >/dev/null 2>&1 || true
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

    sketchybar --trigger showy_bar_provider_change \
        SHOWY_BAR_PROVIDER_COUNT="${provider_count}" \
        SHOWY_BAR_PROVIDERS="${provider_csv}" >/dev/null 2>&1 || true
}

clear_declared_items() {
    local declared pid
    declared="$(read_state_providers)"
    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        remove_provider_items "${pid}"
    done <<< "${declared}"
    sketchybar --remove showy_bar_bracket >/dev/null 2>&1 || true
    write_state_providers "" || showy_bar_log "failed to clear sketchybar provider state"
}

showy_bar_have jq || {
    showy_bar_log "jq required for sketchybar plugin"
    clear_declared_items
    exit 0
}
showy_bar_have magick || {
    showy_bar_log "magick (ImageMagick 7+) required for sketchybar plugin"
    clear_declared_items
    exit 0
}

# Bar geometry. Bars sit inside SketchyBar's pill; tweak via env.
: "${SHOWY_BAR_PNG_BAR_W:=80}"
: "${SHOWY_BAR_PNG_BAR_H:=18}"

# ── ARGB helpers ─────────────────────────────────────────────────────

# 6-char hex (no '#') → 0xff RRGGBB SketchyBar literal.
argb_from_hex() { printf '0xff%s' "$1"; }

# 6-char hex → '#RRGGBB' for ImageMagick.
mhex() { printf '#%s' "$1"; }

PRIMARY_WARN_HEX="$(showy_bar_role_palette primary warn)"
PRIMARY_BAD_HEX="$(showy_bar_role_palette primary bad)"
PRIMARY_UNKNOWN_HEX="$(showy_bar_role_palette primary unknown)"
TRACK_HEX="$(showy_bar_palette track)"
ICON_TEXT_HEX="$(showy_bar_palette icon_text)"
COUNTDOWN_HEX="$(showy_bar_palette countdown)"
COUNTDOWN_ARGB="$(argb_from_hex "${COUNTDOWN_HEX}")"
COUNTDOWN_WARN_HEX="$(showy_bar_palette countdown_warn)"
COUNTDOWN_WARN_ARGB="$(argb_from_hex "${COUNTDOWN_WARN_HEX}")"
ELAPSED_HEX="$(showy_bar_palette elapsed)"

status_color_for_indicator() {
    case "${1:-none}" in
        minor|maintenance) printf '%s' "${PRIMARY_WARN_HEX}" ;;
        major|critical)    printf '%s' "${PRIMARY_BAD_HEX}" ;;
        unknown)           printf '%s' "${PRIMARY_UNKNOWN_HEX}" ;;
        *)                 return 1 ;;
    esac
}

shell_quote() {
    local raw="$1"
    printf "'"
    while [[ "${raw}" == *"'"* ]]; do
        printf '%s' "${raw%%\'*}"
        printf "'\\''"
        raw="${raw#*\'}"
    done
    printf "%s'" "${raw}"
}

status_url_is_openable() {
    case "${1:-}" in
        http://*|https://*) return 0 ;;
        *)                  return 1 ;;
    esac
}

click_script_for_status() {
    local status="${1:-none}" url="${2:-}"
    case "${status}" in
        minor|maintenance|major|critical)
            if status_url_is_openable "${url}"; then
                printf 'open %s' "$(shell_quote "${url}")"
                return
            fi
            ;;
    esac
    printf '%s' "${CLICK}"
}

# Bump when icon rendering semantics change so stale cached PNGs are replaced
# on the next plugin tick.
ICON_CACHE_VERSION="2"

# ── provider icon: lazily render SVG → PNG ───────────────────────────
render_fallback_icon_png() {
    local pid="$1" tmp="$2"
    local sigil
    sigil=$(showy_bar_provider_sigil "${pid}")
    magick -size 64x64 xc:none \
        -fill "$(mhex "${PRIMARY_UNKNOWN_HEX}")" \
        -draw "circle 32,32 32,4" \
        -fill "$(mhex "${ICON_TEXT_HEX}")" \
        -gravity center -pointsize 28 -annotate 0 "${sigil}" \
        "PNG32:${tmp}" >/dev/null 2>&1
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



provider_icon_png() {
    local pid="$1" status="${2:-none}"
    local status_color="" tint_color="" suffix="" out
    if status_color=$(status_color_for_indicator "${status}"); then
        suffix="-${status}"
    fi
    out="${CACHE_DIR}/icon-v${ICON_CACHE_VERSION}-${pid}${suffix}.png"
    [[ -s "${out}" ]] && { printf '%s\n' "${out}"; return 0; }

    # Per-process tmp files in the same directory so `mv` is atomic.
    local tmp normal_tmp
    normal_tmp=$(mktemp "${CACHE_DIR}/.icon-${pid}.normal.XXXXXX") || return 1

    local svg="${SHOWY_BAR_CODEXBAR_RESOURCES}/ProviderIcon-${pid}.svg"
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
        tint_color="${status_color}"
    elif should_tint_dark_icon_png "${normal_tmp}"; then
        tint_color="${ICON_TEXT_HEX}"
    fi

    if [[ -n "${tint_color}" ]]; then
        tmp=$(mktemp "${CACHE_DIR}/.icon-${pid}.tint.XXXXXX") || { rm -f "${normal_tmp}"; return 1; }
        if ! recolor_icon_png "${normal_tmp}" "${tint_color}" "${tmp}"; then
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

    local image_h="${SHOWY_BAR_PNG_BAR_H}"
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
        local w="${SHOWY_BAR_PNG_BAR_W}"
        [[ "${pct}" =~ ^[0-9]+$ ]] || pct=0
        (( pct < 0 )) && pct=0
        (( pct > 100 )) && pct=100
        local f=$(( (pct * w + 50) / 100 ))
        (( pct > 0 && f == 0 )) && f=1
        (( f < 0 )) && f=0
        (( f > w )) && f=$w
        printf '%s\n' "${f}"
    }

    local f1 f2 f3
    f1=$(fill_w "${rem_p}")
    f2=$(fill_w "${rem_s}")
    (( has_t )) && f3=$(fill_w "${rem_t}")

    local c1 c2 c3
    c1=$(showy_bar_role_color primary "${rem_p}")
    c2=$(showy_bar_role_color secondary "${rem_s}")
    (( has_t )) && c3=$(showy_bar_role_color tertiary "${rem_t}")

    local args=( -size "${SHOWY_BAR_PNG_BAR_W}x${image_h}" xc:none )

    add_track() {
        args+=( -fill "$(mhex "${TRACK_HEX}")"
                -draw "roundrectangle 0,$1 $((SHOWY_BAR_PNG_BAR_W - 1)),$2 3,3" )
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
        (( marker >= SHOWY_BAR_PNG_BAR_W )) && marker=$((SHOWY_BAR_PNG_BAR_W - 1))
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
    label=$(showy_bar_primary_label "${minutes}" "${remaining}" "${reset_value}")
    local color="${COUNTDOWN_ARGB}"
    if [[ -n "${minutes}" && "${minutes}" -lt "${SHOWY_BAR_TIME_WARN_MINUTES}" ]] 2>/dev/null; then
        color="${COUNTDOWN_WARN_ARGB}"
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
    reset_epoch=$(showy_bar_reset_epoch "${reset_at}") || return 1
    duration=$((window_minutes * 60))
    start_epoch=$((reset_epoch - duration))
    now=$(date +%s)
    elapsed=$((now - start_epoch))
    (( elapsed < 0 )) && elapsed=0
    (( elapsed > duration )) && elapsed="${duration}"
    marker=$(( (duration - elapsed) * SHOWY_BAR_PNG_BAR_W / duration ))
    (( marker < 0 )) && marker=0
    (( marker >= SHOWY_BAR_PNG_BAR_W )) && marker=$((SHOWY_BAR_PNG_BAR_W - 1))
    printf '%s\n' "${marker}"
}
filtered=$(printf '%s' "${data}" | showy_bar_filter_renderable)
desired_providers=$(printf '%s' "${filtered}" | jq -r '.[].provider')
declared_providers="$(read_state_providers)"
declared_item_providers="${declared_providers}"
force_redeclare=0
expected_live_providers=""
while IFS= read -r pid; do
    [[ -n "${pid}" ]] || continue
    if provider_list_contains "${declared_item_providers}" "${pid}"; then
        if [[ -n "${expected_live_providers}" ]]; then
            expected_live_providers+=$'\n'
        fi
        expected_live_providers+="${pid}"
    fi
done <<< "${desired_providers}"

if [[ "${SHOWY_BAR_SKETCHYBAR_FORCE_REDECLARE:-0}" == "1" ]]; then
    force_redeclare=1
    declared_item_providers=""
elif [[ -n "${expected_live_providers}" ]] && ! declared_items_present "${expected_live_providers}"; then
    force_redeclare=1
    declared_item_providers=""
    showy_bar_log "sketchybar items missing; forcing redeclare"
fi


if (( force_redeclare )) || [[ "${desired_providers}" != "${declared_providers}" ]]; then
    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        provider_list_contains "${desired_providers}" "${pid}" || remove_provider_items "${pid}"
    done <<< "${declared_providers}"

    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        provider_list_contains "${declared_item_providers}" "${pid}" || declare_provider_items "${pid}"
    done <<< "${desired_providers}"

    recreate_bracket "${desired_providers}"
    write_state_providers "${desired_providers}" || showy_bar_log "failed to update sketchybar provider state"
    trigger_provider_change "${desired_providers}"
fi

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
        (.status.indicator // "none"),
        (.status.url // "")
    ] | map(tostring) | join("\u001f")')

while IFS=$'\x1f' read -r pid rem_p p_reset rem_s s_reset s_window rem_t t_reset t_window status status_url; do
    [[ -n "${pid}" ]] || continue

    icon=$(provider_icon_png "${pid}" "${status}" || true)
    marker_s=$(elapsed_marker_x "${s_reset}" "${s_window}" || true)
    marker_t=$(elapsed_marker_x "${t_reset}" "${t_window}" || true)
    bar=$(render_bar_png "${pid}" "${rem_p}" "${rem_s}" "${rem_t}" "${marker_s}" "${marker_t}" || true)

    minutes=""
    if [[ -n "${p_reset}" ]]; then
        minutes=$(showy_bar_minutes_until "${p_reset}" || true)
    fi
    label=""; color=""
    icon_click=$(click_script_for_status "${status}" "${status_url}")
    { IFS= read -r label; IFS= read -r color; } < <(label_for_minutes "${minutes}" "${rem_p}" "${p_reset}") || true
    [[ -n "${color}" ]] || color="${COUNTDOWN_ARGB}"

    args=(
        --set "showy_bar.${pid}.label" drawing=on label="${label}" label.color="${color}" background.color=0x00000000 background.height=0
    )
    if [[ -n "${icon}" && -s "${icon}" ]]; then
        args+=( --set "showy_bar.${pid}.icon" drawing=on background.image="${icon}" background.image.drawing=on background.image.scale="${SHOWY_BAR_SKETCHYBAR_ICON_SCALE}" background.color=0x00000000 background.height=0 padding_left="${SHOWY_BAR_SKETCHYBAR_ICON_PADDING_LEFT}" padding_right=0 width="${SHOWY_BAR_SKETCHYBAR_ICON_WIDTH}" click_script="${icon_click}" )
    else
        args+=( --set "showy_bar.${pid}.icon" drawing=off click_script="${CLICK}" )
    fi
    if [[ -n "${bar}" && -s "${bar}" ]]; then
        args+=( --set "showy_bar.${pid}.bar" drawing=on background.image="${bar}" background.image.drawing=on background.image.scale=1.0 background.color=0x00000000 background.height=0 padding_left=2 padding_right=2 width="${SHOWY_BAR_SKETCHYBAR_BAR_WIDTH}" )
    else
        args+=( --set "showy_bar.${pid}.bar" drawing=off )
    fi

    if (( ${#args[@]} > 0 )); then
        sketchybar "${args[@]}" >/dev/null 2>&1 || true
    fi
done <<< "${rows}"
