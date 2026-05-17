#!/usr/bin/env bash
# showy-bar — SketchyBar plugin: render per-provider icon + native usage
# slider rows, and update each provider's items.
#
# Invoked by the showy_bar.trigger item every SHOWY_BAR_SKETCHYBAR_UPDATE_FREQ
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
        --remove "showy_bar.${pid}.primary" \
        --remove "showy_bar.${pid}.secondary" \
        --remove "showy_bar.${pid}.tertiary" \
        --remove "showy_bar.${pid}.secondary_marker" \
        --remove "showy_bar.${pid}.tertiary_marker" \
        --remove "showy_bar.${pid}.slot" \
        --remove "showy_bar.${pid}.label" >/dev/null 2>&1 || true
}

declare_marker_item() {
    local pid="$1" marker_role="$2" name
    name="showy_bar.${pid}.${marker_role}_marker"
    sketchybar --add slider "${name}" left "${SHOWY_BAR_PNG_BAR_W}" \
               --set "${name}" \
                   drawing=off \
                   slider.percentage=0 \
                   slider.highlight_color=0x00000000 \
                   slider.background.color=0x00000000 \
                   slider.background.height="${NATIVE_ROW_HEIGHT}" \
                   slider.background.corner_radius=0 \
                   slider.knob.drawing=on \
                   slider.knob.color=0x00000000 \
                   slider.knob.width=1 \
                   slider.knob.padding_left=0 \
                   slider.knob.padding_right=0 \
                   slider.knob.background.drawing=on \
                   slider.knob.background.color="${ELAPSED_ARGB}" \
                   slider.knob.background.height="${NATIVE_ROW_HEIGHT}" \
                   slider.knob.background.corner_radius=0 \
                   icon.drawing=off \
                   label.drawing=off \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left=0 \
                   padding_right=0 \
                   width=0 \
                   click_script="${CLICK}" >/dev/null 2>&1 || true
}

declare_provider_items() {
    local pid="$1"
    remove_provider_items "${pid}"

    sketchybar --add item "showy_bar.${pid}.icon" left \
               --set "showy_bar.${pid}.icon" \
                   icon.drawing=off \
                   label.drawing=off \
                   background.image.drawing=off \
                   background.image.scale="${SHOWY_BAR_SKETCHYBAR_ICON_SCALE}" \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left="${SHOWY_BAR_SKETCHYBAR_ICON_PADDING_LEFT}" \
                   padding_right=0 \
                   width="${SHOWY_BAR_SKETCHYBAR_ICON_WIDTH}" \
                   click_script="${CLICK}" >/dev/null 2>&1 || true

    sketchybar --add slider "showy_bar.${pid}.primary" left "${SHOWY_BAR_PNG_BAR_W}" \
               --set "showy_bar.${pid}.primary" \
                   slider.percentage=0 \
                   slider.highlight_color=0x00000000 \
                   slider.background.color="${TRACK_ARGB}" \
                   slider.background.height="${NATIVE_ROW_HEIGHT}" \
                   slider.background.corner_radius="${NATIVE_ROW_RADIUS}" \
                   slider.knob.drawing=off \
                   icon.drawing=off \
                   label.drawing=off \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left=0 \
                   padding_right=0 \
                   width=0 \
                   click_script="${CLICK}" >/dev/null 2>&1 || true

    sketchybar --add slider "showy_bar.${pid}.secondary" left "${SHOWY_BAR_PNG_BAR_W}" \
               --set "showy_bar.${pid}.secondary" \
                   slider.percentage=0 \
                   slider.highlight_color=0x00000000 \
                   slider.background.color="${TRACK_ARGB}" \
                   slider.background.height="${NATIVE_ROW_HEIGHT}" \
                   slider.background.corner_radius="${NATIVE_ROW_RADIUS}" \
                   slider.knob.drawing=off \
                   icon.drawing=off \
                   label.drawing=off \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left=0 \
                   padding_right=0 \
                   width=0 \
                   click_script="${CLICK}" >/dev/null 2>&1 || true

    sketchybar --add slider "showy_bar.${pid}.tertiary" left "${SHOWY_BAR_PNG_BAR_W}" \
               --set "showy_bar.${pid}.tertiary" \
                   drawing=off \
                   slider.percentage=0 \
                   slider.highlight_color=0x00000000 \
                   slider.background.color="${TRACK_ARGB}" \
                   slider.background.height="${NATIVE_ROW_HEIGHT}" \
                   slider.background.corner_radius="${NATIVE_ROW_RADIUS}" \
                   slider.knob.drawing=off \
                   icon.drawing=off \
                   label.drawing=off \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left=0 \
                   padding_right=0 \
                   width=0 \
                   click_script="${CLICK}" >/dev/null 2>&1 || true

    declare_marker_item "${pid}" secondary
    declare_marker_item "${pid}" tertiary

    sketchybar --add item "showy_bar.${pid}.slot" left \
               --set "showy_bar.${pid}.slot" \
                   icon.drawing=off \
                   label.drawing=off \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left=0 \
                   padding_right=0 \
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
        && sketchybar_item_exists "showy_bar.${pid}.primary" \
        && sketchybar_item_exists "showy_bar.${pid}.secondary" \
        && sketchybar_item_exists "showy_bar.${pid}.tertiary" \
        && sketchybar_item_exists "showy_bar.${pid}.secondary_marker" \
        && sketchybar_item_exists "showy_bar.${pid}.tertiary_marker" \
        && sketchybar_item_exists "showy_bar.${pid}.slot" \
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
        bracket_items+=(
            "showy_bar.${pid}.icon"
            "showy_bar.${pid}.primary"
            "showy_bar.${pid}.secondary"
            "showy_bar.${pid}.tertiary"
            "showy_bar.${pid}.secondary_marker"
            "showy_bar.${pid}.tertiary_marker"
            "showy_bar.${pid}.slot"
            "showy_bar.${pid}.label"
        )
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

HAVE_MAGICK=0
showy_bar_have magick && HAVE_MAGICK=1

# Bar geometry. Bars sit inside SketchyBar's pill; tweak via env.
: "${SHOWY_BAR_PNG_BAR_W:=80}"
NATIVE_ROW_HEIGHT=6
# Default 3 == NATIVE_ROW_HEIGHT/2 → fully rounded ends. Set to 0 for a
# squared track; intermediate values yield partial rounding.
NATIVE_ROW_RADIUS="${SHOWY_BAR_SKETCHYBAR_ROW_RADIUS:-3}"

# ── ARGB helpers ─────────────────────────────────────────────────────

# 6-char hex (no '#') → 0xff RRGGBB SketchyBar literal.
argb_from_hex() { printf '0xff%s' "$1"; }

# 6-char hex → '#RRGGBB' for ImageMagick.
mhex() { printf '#%s' "$1"; }

PRIMARY_WARN_HEX="$(showy_bar_role_palette primary warn)"
PRIMARY_BAD_HEX="$(showy_bar_role_palette primary bad)"
PRIMARY_UNKNOWN_HEX="$(showy_bar_role_palette primary unknown)"
TRACK_HEX="$(showy_bar_palette track)"
TRACK_ARGB="$(argb_from_hex "${TRACK_HEX}")"
ICON_TEXT_HEX="$(showy_bar_palette icon_text)"
COUNTDOWN_HEX="$(showy_bar_palette countdown)"
COUNTDOWN_ARGB="$(argb_from_hex "${COUNTDOWN_HEX}")"
COUNTDOWN_WARN_HEX="$(showy_bar_palette countdown_warn)"
COUNTDOWN_WARN_ARGB="$(argb_from_hex "${COUNTDOWN_WARN_HEX}")"
ELAPSED_HEX="$(showy_bar_palette elapsed)"
ELAPSED_ARGB="$(argb_from_hex "${ELAPSED_HEX}")"

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

# ── provider icon: native app-font experiment ────────────────────────
provider_font_icon() {
    case "$1" in
        antigravity) printf ':antigravity:' ;;
        claude)      printf ':claude:' ;;
        codex)       printf ':codex:' ;;
        copilot)     printf ':copilot:' ;;
        cursor)      printf ':cursor:' ;;
        deepseek)    printf ':deepseek:' ;;
        gemini)      printf ':gemini:' ;;
        kiro)        printf ':kiro:' ;;
        ollama)      printf ':ollama:' ;;
        openai)      printf ':openai:' ;;
        perplexity)  printf ':perplexity:' ;;
        warp)        printf ':warp:' ;;
        abacus|abacusai|alibaba|alibaba-coding-plan|amp|augment|codebuff|commandcode|crof|doubao|factory|jetbrains|kilo|kimi|kimik2|manus|mimo|minimax|mistral|opencode|opencodego|openrouter|stepfun|synthetic|venice|vertexai|windsurf|zai)
                    return 1 ;;
        *)           return 1 ;;
    esac
}

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
    (( HAVE_MAGICK )) || return 1

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

# ── native stacked bar helpers ───────────────────────────────────────

clamp_slider_percentage() {
    local pct="${1:-0}"
    [[ "${pct}" =~ ^-?[0-9]+$ ]] || pct=0
    (( pct < 0 )) && pct=0
    (( pct > 100 )) && pct=100
    printf '%s\n' "${pct}"
}

marker_percentage_from_x() {
    local marker="$1" w="${SHOWY_BAR_PNG_BAR_W}"
    [[ "${marker}" =~ ^[0-9]+$ ]] || return 1
    (( w > 1 )) || return 1
    (( marker < 0 )) && marker=0
    (( marker >= w )) && marker=$((w - 1))
    printf '%s\n' $(( (marker * 100 + (w - 1) / 2) / (w - 1) ))
}

slider_click_script() {
    local item="$1" pct="$2"
    printf 'command -v sketchybar >/dev/null 2>&1 && sketchybar --set %s slider.percentage=%s >/dev/null 2>&1; %s' \
        "$(shell_quote "${item}")" \
        "$(clamp_slider_percentage "${pct}")" \
        "${CLICK}"
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

    icon=""
    font_icon=""
    if [[ "${SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_MODE}" == "font" ]]; then
        font_icon=$(provider_font_icon "${pid}" || true)
        [[ -n "${font_icon}" ]] || icon=$(provider_icon_png "${pid}" "${status}" || true)
    else
        icon=$(provider_icon_png "${pid}" "${status}" || true)
    fi
    marker_s=$(elapsed_marker_x "${s_reset}" "${s_window}" || true)
    marker_t=$(elapsed_marker_x "${t_reset}" "${t_window}" || true)

    rem_p_pct=$(clamp_slider_percentage "${rem_p}")
    rem_s_pct=$(clamp_slider_percentage "${rem_s}")
    rem_t_pct=$(clamp_slider_percentage "${rem_t}")
    marker_s_pct=$(marker_percentage_from_x "${marker_s}" || true)
    marker_t_pct=$(marker_percentage_from_x "${marker_t}" || true)

    has_t=0
    [[ -n "${rem_t}" ]] && has_t=1
    if (( has_t )); then
        primary_y=7
        secondary_y=0
        tertiary_y=-7
    else
        primary_y=4
        secondary_y=-4
        tertiary_y=-4
    fi

    minutes=""
    if [[ -n "${p_reset}" ]]; then
        minutes=$(showy_bar_minutes_until "${p_reset}" || true)
    fi
    label=""; color=""
    icon_click=$(click_script_for_status "${status}" "${status_url}")
    font_icon_color="$(argb_from_hex "${ICON_TEXT_HEX}")"
    font_icon_item_width=$((SHOWY_BAR_SKETCHYBAR_ICON_WIDTH + SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_FONT_PADDING_RIGHT))
    status_icon_hex=$(status_color_for_indicator "${status}" || true)
    if [[ -n "${status_icon_hex}" ]]; then
        font_icon_color="$(argb_from_hex "${status_icon_hex}")"
    fi
    { IFS= read -r label; IFS= read -r color; } < <(label_for_minutes "${minutes}" "${rem_p}" "${p_reset}") || true
    [[ -n "${color}" ]] || color="${COUNTDOWN_ARGB}"

    primary_item="showy_bar.${pid}.primary"
    secondary_item="showy_bar.${pid}.secondary"
    tertiary_item="showy_bar.${pid}.tertiary"
    secondary_marker_item="showy_bar.${pid}.secondary_marker"
    tertiary_marker_item="showy_bar.${pid}.tertiary_marker"

    primary_click=$(slider_click_script "${primary_item}" "${rem_p_pct}")
    secondary_click=$(slider_click_script "${secondary_item}" "${rem_s_pct}")
    tertiary_click=$(slider_click_script "${tertiary_item}" "${rem_t_pct}")
    secondary_marker_click=$(slider_click_script "${secondary_marker_item}" "${marker_s_pct:-0}")
    tertiary_marker_click=$(slider_click_script "${tertiary_marker_item}" "${marker_t_pct:-0}")

    args=(
        --set "showy_bar.${pid}.label" drawing=on label="${label}" label.color="${color}" background.color=0x00000000 background.height=0
    )
    if [[ -n "${font_icon}" ]]; then
        args+=( --set "showy_bar.${pid}.icon" drawing=on icon.drawing=on icon="${font_icon}" icon.font="${SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_FONT}" icon.color="${font_icon_color}" icon.align=center icon.width="${SHOWY_BAR_SKETCHYBAR_ICON_WIDTH}" icon.padding_left=0 icon.padding_right=0 label.drawing=off background.image.drawing=off background.color=0x00000000 background.height=0 padding_left="${SHOWY_BAR_SKETCHYBAR_ICON_PADDING_LEFT}" padding_right=0 width="${font_icon_item_width}" click_script="${icon_click}" )
    elif [[ -n "${icon}" && -s "${icon}" ]]; then
        args+=( --set "showy_bar.${pid}.icon" drawing=on icon.drawing=off label.drawing=off background.image="${icon}" background.image.drawing=on background.image.scale="${SHOWY_BAR_SKETCHYBAR_ICON_SCALE}" background.color=0x00000000 background.height=0 padding_left="${SHOWY_BAR_SKETCHYBAR_ICON_PADDING_LEFT}" padding_right=0 width="${SHOWY_BAR_SKETCHYBAR_ICON_WIDTH}" click_script="${icon_click}" )
    else
        args+=( --set "showy_bar.${pid}.icon" drawing=off click_script="${CLICK}" )
    fi

    args+=(
        --set "${primary_item}" drawing=on slider.percentage="${rem_p_pct}" slider.highlight_color="$(argb_from_hex "$(showy_bar_role_color primary "${rem_p_pct}")")" slider.background.color="${TRACK_ARGB}" slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius="${NATIVE_ROW_RADIUS}" slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${primary_y}" click_script="${primary_click}"
        --set "${secondary_item}" drawing=on slider.percentage="${rem_s_pct}" slider.highlight_color="$(argb_from_hex "$(showy_bar_role_color secondary "${rem_s_pct}")")" slider.background.color="${TRACK_ARGB}" slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius="${NATIVE_ROW_RADIUS}" slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${secondary_y}" click_script="${secondary_click}"
    )

    if (( has_t )); then
        args+=( --set "${tertiary_item}" drawing=on slider.percentage="${rem_t_pct}" slider.highlight_color="$(argb_from_hex "$(showy_bar_role_color tertiary "${rem_t_pct}")")" slider.background.color="${TRACK_ARGB}" slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius="${NATIVE_ROW_RADIUS}" slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${tertiary_y}" click_script="${tertiary_click}" )
    else
        args+=( --set "${tertiary_item}" drawing=off slider.percentage=0 background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${tertiary_y}" click_script="${tertiary_click}" )
    fi

    if [[ -n "${marker_s_pct}" ]]; then
        args+=( --set "${secondary_marker_item}" drawing=on slider.percentage="${marker_s_pct}" slider.highlight_color=0x00000000 slider.background.color=0x00000000 slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius=0 slider.knob.drawing=on slider.knob.color=0x00000000 slider.knob.width=1 slider.knob.padding_left=0 slider.knob.padding_right=0 slider.knob.background.drawing=on slider.knob.background.color="${ELAPSED_ARGB}" slider.knob.background.height="${NATIVE_ROW_HEIGHT}" slider.knob.background.corner_radius=0 background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${secondary_y}" click_script="${secondary_marker_click}" )
    else
        args+=( --set "${secondary_marker_item}" drawing=off slider.percentage=0 y_offset="${secondary_y}" click_script="${secondary_marker_click}" )
    fi

    if (( has_t )) && [[ -n "${marker_t_pct}" ]]; then
        args+=( --set "${tertiary_marker_item}" drawing=on slider.percentage="${marker_t_pct}" slider.highlight_color=0x00000000 slider.background.color=0x00000000 slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius=0 slider.knob.drawing=on slider.knob.color=0x00000000 slider.knob.width=1 slider.knob.padding_left=0 slider.knob.padding_right=0 slider.knob.background.drawing=on slider.knob.background.color="${ELAPSED_ARGB}" slider.knob.background.height="${NATIVE_ROW_HEIGHT}" slider.knob.background.corner_radius=0 background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${tertiary_y}" click_script="${tertiary_marker_click}" )
    else
        args+=( --set "${tertiary_marker_item}" drawing=off slider.percentage=0 y_offset="${tertiary_y}" click_script="${tertiary_marker_click}" )
    fi

    args+=( --set "showy_bar.${pid}.slot" drawing=on icon.drawing=off label.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width="${SHOWY_BAR_SKETCHYBAR_BAR_WIDTH}" click_script="${CLICK}" )

    if (( ${#args[@]} > 0 )); then
        sketchybar "${args[@]}" >/dev/null 2>&1 || true
    fi
done <<< "${rows}"
