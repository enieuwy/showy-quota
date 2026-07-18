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
mkdir -p -- "${CACHE_DIR}" || exit 0
chmod 700 "${CACHE_DIR}" 2>/dev/null || true
STATE_FILE="${CACHE_DIR}/providers.txt"
if click_command_is_safe "${SHOWY_QUOTA_SKETCHYBAR_CLICK}"; then
    CLICK="${SHOWY_QUOTA_SKETCHYBAR_CLICK}"
else
    CLICK="${DEFAULT_CLICK}"
fi
CODEXBAR_RESOURCES="$(validated_codexbar_resources || true)"
RENDER_LOCK_DIR="${CACHE_DIR}/render.lock"
RENDER_LOCK_OWNER="${RENDER_LOCK_DIR}/owner.pid"
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

release_render_lock() {
    cleanup_icon_tmp_files
    local owner_pid=""
    if [[ -r "${RENDER_LOCK_OWNER}" ]]; then
        IFS= read -r owner_pid < "${RENDER_LOCK_OWNER}" || owner_pid=""
    fi
    if [[ "${owner_pid}" == "$$" ]]; then
        rm -f -- "${RENDER_LOCK_OWNER}"
        rmdir -- "${RENDER_LOCK_DIR}" 2>/dev/null || true
    fi
}

acquire_render_lock() {
    local owner_pid lock_age
    local max_ownerless_age=30 malformed_owner attempt=0 max_attempts=3

    while (( attempt < max_attempts )); do
        if mkdir -- "${RENDER_LOCK_DIR}" 2>/dev/null; then
            printf '%s\n' "$$" > "${RENDER_LOCK_OWNER}" || {
                rmdir -- "${RENDER_LOCK_DIR}" 2>/dev/null || true
                return 1
            }
            trap release_render_lock EXIT
            return 0
        fi

        owner_pid=""
        lock_age=""
        malformed_owner=0
        if [[ -r "${RENDER_LOCK_OWNER}" ]]; then
            IFS= read -r owner_pid < "${RENDER_LOCK_OWNER}" || owner_pid=""
            if [[ "${owner_pid}" =~ ^[0-9]+$ ]]; then
                if kill -0 "${owner_pid}" 2>/dev/null; then
                    showy_quota_log "sketchybar render already in flight (pid ${owner_pid}); skipping"
                    return 1
                fi
                rm -f -- "${RENDER_LOCK_OWNER}"
                if ! rmdir -- "${RENDER_LOCK_DIR}" 2>/dev/null; then
                    return 1
                fi
                ((attempt += 1))
                (( attempt < max_attempts )) && sleep "0.$((attempt * 5))"
                continue
            else
                malformed_owner=1
            fi
        fi

        # A future-dated lock mtime (clock skew, hand-touched dir) yields a
        # negative age that would never satisfy the threshold and wedge rendering
        # forever; judge staleness by absolute distance from now instead.
        if lock_age=$(render_lock_age_seconds) \
            && { (( lock_age >= max_ownerless_age )) || (( lock_age <= -max_ownerless_age )); }; then
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

start_background_refresh() {
    ( "${FETCH}" </dev/null >/dev/null 2>&1 ) &
    disown "$!" 2>/dev/null || true
}


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

remove_provider_items() {
    local pid="$1"
    sketchybar \
        --remove "showy_quota.${pid}.icon" \
        --remove "showy_quota.${pid}.bar" \
        --remove "showy_quota.${pid}.primary" \
        --remove "showy_quota.${pid}.secondary" \
        --remove "showy_quota.${pid}.tertiary" \
        --remove "showy_quota.${pid}.quaternary" \
        --remove "showy_quota.${pid}.secondary_marker" \
        --remove "showy_quota.${pid}.tertiary_marker" \
        --remove "showy_quota.${pid}.quaternary_marker" \
        --remove "showy_quota.${pid}.primary_marker" \
        --remove "showy_quota.${pid}.slot" \
        --remove "showy_quota.${pid}.label" >/dev/null 2>&1 || true
}

declare_marker_item() {
    local pid="$1" marker_role="$2" name
    name="showy_quota.${pid}.${marker_role}_marker"
    sketchybar --add slider "${name}" left "${SHOWY_QUOTA_PNG_BAR_W}" \
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

declare_stale_item() {
    sketchybar --remove showy_quota.stale >/dev/null 2>&1 || true
    sketchybar --add item showy_quota.stale left \
               --set showy_quota.stale \
                   drawing=off \
                   label="${SHOWY_QUOTA_STALE_GLYPH}" \
                   label.color="${COUNTDOWN_WARN_ARGB}" \
                   icon.drawing=off \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left=4 \
                   padding_right=2 \
                   click_script="${CLICK}" >/dev/null 2>&1 || true
}

declare_degraded_item() {
    sketchybar --remove showy_quota.degraded >/dev/null 2>&1 || true
    sketchybar --add item showy_quota.degraded left \
               --set showy_quota.degraded \
                   drawing=off \
                   label="${SHOWY_QUOTA_DEGRADED_CLI_GLYPH}" \
                   label.color="${COUNTDOWN_WARN_ARGB}" \
                   icon.drawing=off \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left=2 \
                   padding_right=4 \
                   click_script="${CLICK}" >/dev/null 2>&1 || true
}

declare_provider_items() {
    local pid="$1"
    remove_provider_items "${pid}"

    sketchybar --add item "showy_quota.${pid}.icon" left \
               --set "showy_quota.${pid}.icon" \
                   icon.drawing=off \
                   label.drawing=off \
                   background.image.drawing=off \
                   background.image.scale="${SHOWY_QUOTA_SKETCHYBAR_ICON_SCALE}" \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left="${SHOWY_QUOTA_SKETCHYBAR_ICON_PADDING_LEFT}" \
                   padding_right=0 \
                   width="${SHOWY_QUOTA_SKETCHYBAR_ICON_WIDTH}" \
                   click_script="${CLICK}" >/dev/null 2>&1 || true

    sketchybar --add slider "showy_quota.${pid}.primary" left "${SHOWY_QUOTA_PNG_BAR_W}" \
               --set "showy_quota.${pid}.primary" \
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

    sketchybar --add slider "showy_quota.${pid}.secondary" left "${SHOWY_QUOTA_PNG_BAR_W}" \
               --set "showy_quota.${pid}.secondary" \
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

    sketchybar --add slider "showy_quota.${pid}.tertiary" left "${SHOWY_QUOTA_PNG_BAR_W}" \
               --set "showy_quota.${pid}.tertiary" \
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

    sketchybar --add slider "showy_quota.${pid}.quaternary" left "${SHOWY_QUOTA_PNG_BAR_W}" \
               --set "showy_quota.${pid}.quaternary" \
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
    declare_marker_item "${pid}" primary
    declare_marker_item "${pid}" tertiary
    declare_marker_item "${pid}" quaternary

    sketchybar --add item "showy_quota.${pid}.slot" left \
               --set "showy_quota.${pid}.slot" \
                   icon.drawing=off \
                   label.drawing=off \
                   background.color=0x00000000 \
                   background.height=0 \
                   padding_left=0 \
                   padding_right=0 \
                   width="${SHOWY_QUOTA_SKETCHYBAR_BAR_WIDTH}" \
                   click_script="${CLICK}" >/dev/null 2>&1 || true

    sketchybar --add item "showy_quota.${pid}.label" left \
               --set "showy_quota.${pid}.label" \
                   icon.drawing=off \
                   label.font.size=11 \
                   label.padding_left=0 \
                   label.padding_right=4 \
                   label.width="${SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH}" \
                   label.align=left \
                   background.color=0x00000000 \
                   background.height=0 \
                   click_script="${CLICK}" >/dev/null 2>&1 || true
}
sketchybar_item_exists() {
    sketchybar --query "$1" >/dev/null 2>&1
}

provider_items_declared() {
    local pid="$1"
    sketchybar_item_exists "showy_quota.${pid}.icon" \
        && sketchybar_item_exists "showy_quota.${pid}.primary" \
        && sketchybar_item_exists "showy_quota.${pid}.secondary" \
        && sketchybar_item_exists "showy_quota.${pid}.tertiary" \
        && sketchybar_item_exists "showy_quota.${pid}.quaternary" \
        && sketchybar_item_exists "showy_quota.${pid}.secondary_marker" \
        && sketchybar_item_exists "showy_quota.${pid}.tertiary_marker" \
        && sketchybar_item_exists "showy_quota.${pid}.quaternary_marker" \
        && sketchybar_item_exists "showy_quota.${pid}.primary_marker" \
        && sketchybar_item_exists "showy_quota.${pid}.slot" \
        && sketchybar_item_exists "showy_quota.${pid}.label"
}

declared_items_present() {
    local providers="${1-}" pid
    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        provider_items_declared "${pid}" || return 1
    done <<< "${providers}"
    sketchybar_item_exists showy_quota.stale || return 1
    sketchybar_item_exists showy_quota.degraded || return 1
    [[ -z "${providers}" ]] || sketchybar_item_exists showy_quota_bracket
}


recreate_bracket() {
    local providers="${1-}" pid
    local bracket_items=()
    local has_provider=0
    sketchybar --remove showy_quota_bracket >/dev/null 2>&1 || true
    declare_stale_item
    declare_degraded_item

    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        has_provider=1
        bracket_items+=(
            "showy_quota.${pid}.icon"
            "showy_quota.${pid}.primary"
            "showy_quota.${pid}.secondary"
            "showy_quota.${pid}.tertiary"
            "showy_quota.${pid}.quaternary"
            "showy_quota.${pid}.secondary_marker"
            "showy_quota.${pid}.tertiary_marker"
            "showy_quota.${pid}.quaternary_marker"
            "showy_quota.${pid}.primary_marker"
            "showy_quota.${pid}.slot"
            "showy_quota.${pid}.label"
        )
    done <<< "${providers}"

    if (( ! has_provider )); then
        return 0
    fi

    bracket_items+=("showy_quota.stale" "showy_quota.degraded")
    sketchybar --add bracket showy_quota_bracket "${bracket_items[@]}" \
               --set showy_quota_bracket \
                   background.color="${SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR}" \
                   background.corner_radius="${SHOWY_QUOTA_SKETCHYBAR_PILL_RADIUS}" \
                   background.height="${SHOWY_QUOTA_SKETCHYBAR_PILL_HEIGHT}" >/dev/null 2>&1 || true
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
    local declared pid
    declared="$(read_state_providers)"
    while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        remove_provider_items "${pid}"
    done <<< "${declared}"
    sketchybar --remove showy_quota_bracket >/dev/null 2>&1 || true
    sketchybar --remove showy_quota.stale >/dev/null 2>&1 || true
    sketchybar --remove showy_quota.degraded >/dev/null 2>&1 || true
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

# Point ImageMagick at our restrictive policy.xml so a provider SVG cannot make
# `magick` fetch a remote href (SSRF). Prepend so it wins over system configs;
# guard on the file so a copied (non-repo-relative) install still renders.
if (( HAVE_MAGICK )) && [[ -f "${REPO_ROOT}/adapters/sketchybar/imagemagick/policy.xml" ]]; then
    export MAGICK_CONFIGURE_PATH="${REPO_ROOT}/adapters/sketchybar/imagemagick${MAGICK_CONFIGURE_PATH:+:${MAGICK_CONFIGURE_PATH}}"
fi

# Bar geometry. Bars sit inside SketchyBar's pill; tweak via env.
: "${SHOWY_QUOTA_PNG_BAR_W:=80}"
NATIVE_ROW_HEIGHT=6
# Default 3 == NATIVE_ROW_HEIGHT/2 → fully rounded ends. Set to 0 for a
# squared track; intermediate values yield partial rounding.
NATIVE_ROW_RADIUS=$(showy_quota_uint "${SHOWY_QUOTA_SKETCHYBAR_ROW_RADIUS:-3}" 3 4096)

# ── ARGB helpers ─────────────────────────────────────────────────────

# 6-char hex (no '#') → 0xff RRGGBB SketchyBar literal.
argb_from_hex() { printf '0xff%s' "$1"; }

# 6-char hex → '#RRGGBB' for ImageMagick.
mhex() { printf '#%s' "$1"; }

PRIMARY_WARN_HEX="$(showy_quota_primary_palette warn)"
PRIMARY_BAD_HEX="$(showy_quota_primary_palette bad)"
PRIMARY_UNKNOWN_HEX="$(showy_quota_primary_palette unknown)"
TRACK_HEX="$(showy_quota_palette track)"
TRACK_ARGB="$(argb_from_hex "${TRACK_HEX}")"
ICON_TEXT_HEX="$(showy_quota_palette icon_text)"
COUNTDOWN_WARN_HEX="$(showy_quota_palette countdown_warn)"
COUNTDOWN_WARN_ARGB="$(argb_from_hex "${COUNTDOWN_WARN_HEX}")"
ELAPSED_HEX="$(showy_quota_palette elapsed)"
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
    local url="${1:-}" rest authority host port lower_host

    [[ ${#url} -le 2048 ]] || return 1
    [[ ! "${url}" =~ [[:cntrl:][:space:]] ]] || return 1
    [[ "${url}" != *\\* ]] || return 1
    case "${url}" in
        http://*|https://*) ;;
        *)                  return 1 ;;
    esac

    rest="${url#*://}"
    authority="${rest%%[/?#]*}"
    [[ -n "${authority}" && "${authority}" != *@* ]] || return 1
    if [[ "${authority}" == *:* ]]; then
        host="${authority%:*}"
        port="${authority##*:}"
        [[ "${host}" != *:* && "${port}" =~ ^[0-9]{1,5}$ ]] || return 1
        (( 10#${port} >= 1 && 10#${port} <= 65535 )) || return 1
    else
        host="${authority}"
    fi
    [[ "${host}" =~ ^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?)*$ ]] || return 1
    [[ ! "${host}" =~ ^[0-9.]+$ ]] || return 1

    lower_host="${host,,}"
    case "${lower_host}" in
        localhost|localhost.|*.localhost|*.localhost.) return 1 ;;
        *) return 0 ;;
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
ICON_CACHE_VERSION="3"

# ── provider icon: native app-font experiment ────────────────────────
provider_font_icon() {
    case "$1" in
        antigravity) printf ':antigravity:' ;;
        claude)      printf ':claude:' ;;
        codex)       printf ':codex:' ;;
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
    sigil=$(showy_quota_provider_sigil "${pid}")
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
    local status_color="" tint_color="" suffix="" out cache_key
    if status_color=$(status_color_for_indicator "${status}"); then
        suffix="-${status}"
    fi
    cache_key="${ICON_TEXT_HEX}-${PRIMARY_UNKNOWN_HEX}-${PRIMARY_WARN_HEX}-${PRIMARY_BAD_HEX}"
    out="${CACHE_DIR}/icon-v${ICON_CACHE_VERSION}-${pid}-${cache_key}${suffix}.png"
    [[ -s "${out}" ]] && { printf '%s\n' "${out}"; return 0; }

    # Per-process tmp files in the same directory so `mv` is atomic.
    local tmp normal_tmp
    normal_tmp=$(mktemp "${CACHE_DIR}/.icon-${pid}.normal.XXXXXX") || return 1
    ICON_TMP_FILES+=("${normal_tmp}")

    local svg=""
    [[ -n "${CODEXBAR_RESOURCES}" ]] && svg="${CODEXBAR_RESOURCES}/ProviderIcon-${pid}.svg"
    if [[ -z "${svg}" || ! -r "${svg}" ]]; then
        if ! render_fallback_icon_png "${pid}" "${normal_tmp}"; then
            rm -f "${normal_tmp}"; return 1
        fi
    else
        if ! magick -background none -density 300 "MSVG:${svg}" \
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

slider_click_script() {
    local item="$1" pct="$2"
    printf 'command -v sketchybar >/dev/null 2>&1 && sketchybar --set %s slider.percentage=%s >/dev/null 2>&1; %s' \
        "$(shell_quote "${item}")" \
        "$(clamp_slider_percentage "${pct}")" \
        "${CLICK}"
}

# ── main ─────────────────────────────────────────────────────────────

acquire_render_lock || exit 0
refresh_in_background=0
if "${FETCH}" --cache-only >/dev/null 2>&1; then
    if [[ -n "${SHOWY_QUOTA_CODEXBAR_SERVE_URL:-}" ]]; then
        refresh_threshold="${SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS:-60}"
        [[ "${refresh_threshold}" =~ ^[0-9]+$ ]] || refresh_threshold=60
    else
        refresh_threshold="${SHOWY_QUOTA_REFRESH_SECONDS:-120}"
        [[ "${refresh_threshold}" =~ ^[0-9]+$ ]] || refresh_threshold=120
    fi
    cache_age=$(showy_quota_age_seconds "${SHOWY_QUOTA_USAGE_FILE}")
    if [[ "${cache_age}" =~ ^[0-9]+$ ]] && (( cache_age >= refresh_threshold )); then
        refresh_in_background=1
    fi
else
    "${FETCH}" >/dev/null 2>&1 || true
fi

# Row compute (renderable filtering, elapsed markers, countdown labels,
# window colors, stale and shared-cycle handling) lives in the native
# renderer; this plugin only assembles SketchyBar items from the emitted
# fields. See crates/showy-quota-zellij-core/src/sketchybar.rs for the
# record format (US-separated fields, one provider per line, header first).
showy_quota_export_config
rows_payload=$("${RENDER_BIN}" --emit sketchybar --from-cache 2>/dev/null) || rows_payload=""

stale=0
degraded_cli=0
rows=""
if [[ -n "${rows_payload}" ]]; then
    header="${rows_payload%%$'\n'*}"
    [[ "${rows_payload}" == *$'\n'* ]] && rows="${rows_payload#*$'\n'}"
    IFS=$'\x1f' read -r header_stale header_degraded <<< "${header}"
    [[ "${header_stale}" == "1" ]] && stale=1
    [[ "${header_degraded}" == "1" ]] && degraded_cli=1
else
    # No renderable cache (cold start without codexbar, or an invalid
    # payload): mirror the empty-data path so items tear down while the
    # stale/degraded markers still reflect the on-disk cache state.
    showy_quota_cache_stale_for "${SHOWY_QUOTA_USAGE_FILE}" && stale=1
    if [[ "${SHOWY_QUOTA_DEGRADED_CLI:-}" == "1" ]] \
        || { [[ -z "${SHOWY_QUOTA_DEGRADED_CLI:-}" ]] && showy_quota_cache_degraded_cli; }; then
        degraded_cli=1
    fi
fi
if (( refresh_in_background )); then
    start_background_refresh
fi

desired_providers=""
while IFS=$'\x1f' read -r pid _; do
    [[ -n "${pid}" ]] || continue
    if [[ -n "${desired_providers}" ]]; then
        desired_providers+=$'\n'
    fi
    desired_providers+="${pid}"
done <<< "${rows}"
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

if [[ "${SHOWY_QUOTA_SKETCHYBAR_FORCE_REDECLARE:-0}" == "1" ]]; then
    force_redeclare=1
    declared_item_providers=""
elif ! declared_items_present "${expected_live_providers}"; then
    force_redeclare=1
    declared_item_providers=""
    showy_quota_log "sketchybar items missing; forcing redeclare"
elif [[ "${desired_providers}" != "${declared_providers}" ]]; then
    # Set or order changed. SketchyBar lays items out by `--add` order
    # within a position group, so an incremental add appends a new
    # provider to the end regardless of where it sorts in
    # desired_providers. Force a full teardown so positions match the
    # desired sort.
    force_redeclare=1
    declared_item_providers=""
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
    write_state_providers "${desired_providers}" || showy_quota_log "failed to update sketchybar provider state"
    trigger_provider_change "${desired_providers}"
fi

# shellcheck disable=SC2034  # p/s presence flags keep the record uniform
while IFS=$'\x1f' read -r pid label color status status_url \
    p_present rem_p_pct marker_p_pct primary_highlight \
    s_present rem_s_pct marker_s_pct secondary_highlight \
    t_present rem_t_pct marker_t_pct tertiary_highlight \
    q_present rem_q_pct marker_q_pct quaternary_highlight; do
    [[ -n "${pid}" ]] || continue

    icon=""
    font_icon=""
    if [[ "${SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE}" == "font" ]]; then
        font_icon=$(provider_font_icon "${pid}" || true)
        [[ -n "${font_icon}" ]] || icon=$(provider_icon_png "${pid}" "${status}" || true)
    else
        icon=$(provider_icon_png "${pid}" "${status}" || true)
    fi

    has_t=0
    [[ "${t_present}" == "1" ]] && has_t=1
    has_q=0
    [[ "${q_present}" == "1" ]] && has_q=1
    has_s=0
    [[ "${s_present}" == "1" ]] && has_s=1
    if (( has_q )); then
        primary_y=9
        secondary_y=3
        tertiary_y=-3
        quaternary_y=-9
    elif (( has_t )); then
        primary_y=7
        secondary_y=0
        tertiary_y=-7
        quaternary_y=-7
    elif (( has_s )); then
        primary_y=4
        secondary_y=-4
        tertiary_y=-4
        quaternary_y=-4
    else
        # Single live window (e.g. Codex once the 5h limit is dropped): one
        # centered full-height bar, no empty second row.
        primary_y=0
        secondary_y=0
        tertiary_y=0
        quaternary_y=0
    fi

    icon_click=$(click_script_for_status "${status}" "${status_url}")
    font_icon_color="$(argb_from_hex "${ICON_TEXT_HEX}")"
    font_icon_item_width=$((SHOWY_QUOTA_SKETCHYBAR_ICON_WIDTH + SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT_PADDING_RIGHT))
    status_icon_hex=$(status_color_for_indicator "${status}" || true)
    if [[ -n "${status_icon_hex}" ]]; then
        font_icon_color="$(argb_from_hex "${status_icon_hex}")"
    fi

    primary_item="showy_quota.${pid}.primary"
    secondary_item="showy_quota.${pid}.secondary"
    tertiary_item="showy_quota.${pid}.tertiary"
    quaternary_item="showy_quota.${pid}.quaternary"
    secondary_marker_item="showy_quota.${pid}.secondary_marker"
    tertiary_marker_item="showy_quota.${pid}.tertiary_marker"
    quaternary_marker_item="showy_quota.${pid}.quaternary_marker"
    primary_marker_item="showy_quota.${pid}.primary_marker"

    primary_click=$(slider_click_script "${primary_item}" "${rem_p_pct}")
    secondary_click=$(slider_click_script "${secondary_item}" "${rem_s_pct}")
    tertiary_click=$(slider_click_script "${tertiary_item}" "${rem_t_pct}")
    quaternary_click=$(slider_click_script "${quaternary_item}" "${rem_q_pct}")
    secondary_marker_click=$(slider_click_script "${secondary_marker_item}" "${marker_s_pct:-0}")
    tertiary_marker_click=$(slider_click_script "${tertiary_marker_item}" "${marker_t_pct:-0}")
    quaternary_marker_click=$(slider_click_script "${quaternary_marker_item}" "${marker_q_pct:-0}")
    primary_marker_click=$(slider_click_script "${primary_marker_item}" "${marker_p_pct:-0}")

    args=(
        --set "showy_quota.${pid}.label" drawing=on label="${label}" label.color="${color}" label.width="${SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH}" label.align=left background.color=0x00000000 background.height=0
    )
    if [[ -n "${font_icon}" ]]; then
        args+=( --set "showy_quota.${pid}.icon" drawing=on icon.drawing=on icon="${font_icon}" icon.font="${SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT}" icon.color="${font_icon_color}" icon.align=center icon.width="${SHOWY_QUOTA_SKETCHYBAR_ICON_WIDTH}" icon.padding_left=0 icon.padding_right=0 label.drawing=off background.image.drawing=off background.color=0x00000000 background.height=0 padding_left="${SHOWY_QUOTA_SKETCHYBAR_ICON_PADDING_LEFT}" padding_right=0 width="${font_icon_item_width}" click_script="${icon_click}" )
    elif [[ -n "${icon}" && -s "${icon}" ]]; then
        args+=( --set "showy_quota.${pid}.icon" drawing=on icon.drawing=off label.drawing=off background.image="${icon}" background.image.drawing=on background.image.scale="${SHOWY_QUOTA_SKETCHYBAR_ICON_SCALE}" background.color=0x00000000 background.height=0 padding_left="${SHOWY_QUOTA_SKETCHYBAR_ICON_PADDING_LEFT}" padding_right=0 width="${SHOWY_QUOTA_SKETCHYBAR_ICON_WIDTH}" click_script="${icon_click}" )
    else
        args+=( --set "showy_quota.${pid}.icon" drawing=off click_script="${CLICK}" )
    fi

    args+=(
        --set "${primary_item}" drawing=on slider.percentage="${rem_p_pct}" slider.highlight_color="${primary_highlight}" slider.background.color="${TRACK_ARGB}" slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius="${NATIVE_ROW_RADIUS}" slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${primary_y}" click_script="${primary_click}"
    )
    if (( has_s )); then
        args+=( --set "${secondary_item}" drawing=on slider.percentage="${rem_s_pct}" slider.highlight_color="${secondary_highlight}" slider.background.color="${TRACK_ARGB}" slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius="${NATIVE_ROW_RADIUS}" slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${secondary_y}" click_script="${secondary_click}" )
    else
        args+=( --set "${secondary_item}" drawing=off slider.percentage=0 background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${secondary_y}" click_script="${secondary_click}" )
    fi

    if (( has_t )); then
        args+=( --set "${tertiary_item}" drawing=on slider.percentage="${rem_t_pct}" slider.highlight_color="${tertiary_highlight}" slider.background.color="${TRACK_ARGB}" slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius="${NATIVE_ROW_RADIUS}" slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${tertiary_y}" click_script="${tertiary_click}" )
    else
        args+=( --set "${tertiary_item}" drawing=off slider.percentage=0 background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${tertiary_y}" click_script="${tertiary_click}" )
    fi

    if (( has_q )); then
        args+=( --set "${quaternary_item}" drawing=on slider.percentage="${rem_q_pct}" slider.highlight_color="${quaternary_highlight}" slider.background.color="${TRACK_ARGB}" slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius="${NATIVE_ROW_RADIUS}" slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${quaternary_y}" click_script="${quaternary_click}" )
    else
        args+=( --set "${quaternary_item}" drawing=off slider.percentage=0 background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${quaternary_y}" click_script="${quaternary_click}" )
    fi

    if [[ -n "${marker_p_pct}" ]]; then
        args+=( --set "${primary_marker_item}" drawing=on slider.percentage="${marker_p_pct}" slider.highlight_color=0x00000000 slider.background.color=0x00000000 slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius=0 slider.knob.drawing=on slider.knob.color=0x00000000 slider.knob.width=1 slider.knob.padding_left=0 slider.knob.padding_right=0 slider.knob.background.drawing=on slider.knob.background.color="${ELAPSED_ARGB}" slider.knob.background.height="${NATIVE_ROW_HEIGHT}" slider.knob.background.corner_radius=0 background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${primary_y}" click_script="${primary_marker_click}" )
    else
        args+=( --set "${primary_marker_item}" drawing=off slider.percentage=0 y_offset="${primary_y}" click_script="${primary_marker_click}" )
    fi

    if (( has_s )) && [[ -n "${marker_s_pct}" ]]; then
        args+=( --set "${secondary_marker_item}" drawing=on slider.percentage="${marker_s_pct}" slider.highlight_color=0x00000000 slider.background.color=0x00000000 slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius=0 slider.knob.drawing=on slider.knob.color=0x00000000 slider.knob.width=1 slider.knob.padding_left=0 slider.knob.padding_right=0 slider.knob.background.drawing=on slider.knob.background.color="${ELAPSED_ARGB}" slider.knob.background.height="${NATIVE_ROW_HEIGHT}" slider.knob.background.corner_radius=0 background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${secondary_y}" click_script="${secondary_marker_click}" )
    else
        args+=( --set "${secondary_marker_item}" drawing=off slider.percentage=0 y_offset="${secondary_y}" click_script="${secondary_marker_click}" )
    fi

    if (( has_t )) && [[ -n "${marker_t_pct}" ]]; then
        args+=( --set "${tertiary_marker_item}" drawing=on slider.percentage="${marker_t_pct}" slider.highlight_color=0x00000000 slider.background.color=0x00000000 slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius=0 slider.knob.drawing=on slider.knob.color=0x00000000 slider.knob.width=1 slider.knob.padding_left=0 slider.knob.padding_right=0 slider.knob.background.drawing=on slider.knob.background.color="${ELAPSED_ARGB}" slider.knob.background.height="${NATIVE_ROW_HEIGHT}" slider.knob.background.corner_radius=0 background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${tertiary_y}" click_script="${tertiary_marker_click}" )
    else
        args+=( --set "${tertiary_marker_item}" drawing=off slider.percentage=0 y_offset="${tertiary_y}" click_script="${tertiary_marker_click}" )
    fi

    if (( has_q )) && [[ -n "${marker_q_pct}" ]]; then
        args+=( --set "${quaternary_marker_item}" drawing=on slider.percentage="${marker_q_pct}" slider.highlight_color=0x00000000 slider.background.color=0x00000000 slider.background.height="${NATIVE_ROW_HEIGHT}" slider.background.corner_radius=0 slider.knob.drawing=on slider.knob.color=0x00000000 slider.knob.width=1 slider.knob.padding_left=0 slider.knob.padding_right=0 slider.knob.background.drawing=on slider.knob.background.color="${ELAPSED_ARGB}" slider.knob.background.height="${NATIVE_ROW_HEIGHT}" slider.knob.background.corner_radius=0 background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset="${quaternary_y}" click_script="${quaternary_marker_click}" )
    else
        args+=( --set "${quaternary_marker_item}" drawing=off slider.percentage=0 y_offset="${quaternary_y}" click_script="${quaternary_marker_click}" )
    fi

    args+=( --set "showy_quota.${pid}.slot" drawing=on icon.drawing=off label.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width="${SHOWY_QUOTA_SKETCHYBAR_BAR_WIDTH}" click_script="${CLICK}" )

    if (( ${#args[@]} > 0 )); then
        sketchybar "${args[@]}" >/dev/null 2>&1 || true
    fi
done <<< "${rows}"
if (( stale )); then
    sketchybar --set showy_quota.stale drawing=on label="${SHOWY_QUOTA_STALE_GLYPH}" label.color="${COUNTDOWN_WARN_ARGB}" icon.drawing=off background.color=0x00000000 background.height=0 padding_left=4 padding_right=2 click_script="${CLICK}" >/dev/null 2>&1 || true
else
    sketchybar --set showy_quota.stale drawing=off >/dev/null 2>&1 || true
fi
if (( degraded_cli )); then
    sketchybar --set showy_quota.degraded drawing=on label="${SHOWY_QUOTA_DEGRADED_CLI_GLYPH}" label.color="${COUNTDOWN_WARN_ARGB}" icon.drawing=off background.color=0x00000000 background.height=0 padding_left=2 padding_right=4 click_script="${CLICK}" >/dev/null 2>&1 || true
else
    sketchybar --set showy_quota.degraded drawing=off >/dev/null 2>&1 || true
fi
