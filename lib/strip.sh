#!/usr/bin/env bash
# showy-bar — strip rendering helpers used by both the Zellij ANSI
# renderer and the tmux markup renderer.
#
# Sourced after lib/common.sh.

# Provider id → short two-or-three letter sigil shown when no Nerd Font icon
# is available. Keep these stable so users can recognize them in the strip.
showy_bar_provider_sigil() {
    case "$1" in
        codex)         printf 'CX' ;;
        claude)        printf 'CL' ;;
        cursor)        printf 'CR' ;;
        opencode)      printf 'OC' ;;
        opencodego)    printf 'OG' ;;
        alibaba)       printf 'AL' ;;
        factory|droid) printf 'FA' ;;
        gemini)        printf 'GE' ;;
        antigravity)   printf 'AG' ;;
        copilot)       printf 'CP' ;;
        zai)           printf 'ZA' ;;
        minimax)       printf 'MX' ;;
        kimi)          printf 'KM' ;;
        kimik2)        printf 'K2' ;;
        kilo)          printf 'KL' ;;
        kiro)          printf 'KR' ;;
        vertexai)      printf 'VA' ;;
        augment)       printf 'AU' ;;
        jetbrains)     printf 'JB' ;;
        amp)           printf 'AM' ;;
        ollama)        printf 'OL' ;;
        synthetic)     printf 'SY' ;;
        warp)          printf 'WP' ;;
        openrouter)    printf 'OR' ;;
        windsurf)      printf 'WS' ;;
        perplexity)    printf 'PX' ;;
        abacus)        printf 'AB' ;;
        mistral)       printf 'MS' ;;
        deepseek)      printf 'DS' ;;
        codebuff)      printf 'CB' ;;
        *)             printf '%s' "${1:0:2}" | tr '[:lower:]' '[:upper:]' ;;
    esac
}

# Filter the cached JSON down to renderable provider records, honoring
# SHOWY_BAR_PROVIDERS, SHOWY_BAR_PROVIDERS_EXCLUDE, and provider ordering when
# set. Reads from stdin, writes JSON array to stdout.
showy_bar_filter_renderable() {
    local allow="${SHOWY_BAR_PROVIDERS:-}"
    local exclude="${SHOWY_BAR_PROVIDERS_EXCLUDE:-}"
    local order="${SHOWY_BAR_PROVIDER_ORDER:-}"
    jq --arg allow "${allow}" --arg exclude "${exclude}" --arg order "${order}" '
        def list($raw):
            $raw
            | split(",")
            | map(gsub("^\\s+|\\s+$"; ""))
            | map(select(length > 0));
        def pos($items; $provider):
            ($items | index($provider)) as $idx
            | if $idx == null then 1000000 else $idx end;
        (list($allow)) as $allow_list
        | (list($exclude)) as $exclude_list
        | (list($order)) as $order_list
        | [ .[] | select(
            (.error // null) == null
            and (.provider | type == "string" and test("^[A-Za-z0-9_.-]+$"))
            and (.usage.primary.usedPercent | type == "number")
            and (.provider as $p | (($allow_list | length) == 0 or ($allow_list | index($p) != null)))
            and (.provider as $p | ($exclude_list | index($p) == null))
        ) ] as $filtered
        | if ($allow_list | length) > 0 then
            $filtered | sort_by([(.provider as $p | pos($allow_list; $p)), .provider])
          elif ($order_list | length) > 0 then
            $filtered | sort_by([(.provider as $p | pos($order_list; $p)), .provider])
          else
            $filtered
          end
    '
}

# Render a single window slot as JSON: { used_pct, remaining_pct, reset_at,
# window_minutes, color }.
showy_bar_window_jq() {
    cat <<'JQ'
def window_obj(slot):
    if (slot // null) == null then null
    else
        slot.usedPercent as $u
        | { used_pct: ($u // 0),
            remaining_pct: (100 - ($u // 0)),
            reset_at: (slot.resetsAt // null),
            window_minutes: (slot.windowMinutes // 0),
            reset_description: (slot.resetDescription // null) }
    end;
JQ
}

# Build a compact bar (8 cells of unicode block) for a 0..100 percentage.
# Args: $1 = percent (int 0..100). Echoes 8 chars.
showy_bar_block_bar() {
    local pct="${1:-0}"
    [[ "${pct}" =~ ^-?[0-9]+$ ]] || pct=0
    (( pct < 0 )) && pct=0
    (( pct > 100 )) && pct=100
    # Each of the 8 cells represents 12.5%; show ▆ for filled, ▁ for track.
    # Using unambiguous half-block + bottom-block characters renders the
    # same in any terminal with a default font.
    local cells=8 i out=""
    local filled=$(( (pct * cells + 50) / 100 ))
    (( pct > 0 && filled == 0 )) && filled=1
    for (( i=0; i<cells; i++ )); do
        if (( i < filled )); then out+="█"
        else out+="░"
        fi
    done
    printf '%s' "${out}"
}

# Width-aware fill count for compact strip renderers.
# Args: $1 = remaining percent, $2 = number of cells.
showy_bar_filled_cells() {
    local remaining="${1:-0}" width="$2"
    [[ "${remaining}" =~ ^-?[0-9]+$ ]] || remaining=0
    (( remaining < 0 )) && remaining=0
    (( remaining > 100 )) && remaining=100
    local filled=$(( (remaining * width) / 100 ))
    (( remaining > 0 && filled == 0 )) && filled=1
    printf '%s\n' "${filled}"
}

# Unicode sextant/block mosaic glyphs for one terminal cell split into three
# stacked rows: primary(top)=1, secondary(middle)=2, tertiary(bottom)=4.
showy_bar_sextant_mask_char() {
    case "$1" in
        0) printf ' ' ;;
        1) printf '🬂' ;; # BLOCK SEXTANT-12
        2) printf '🬋' ;; # BLOCK SEXTANT-34
        3) printf '🬎' ;; # BLOCK SEXTANT-1234
        4) printf '🬭' ;; # BLOCK SEXTANT-56
        5) printf '🬰' ;; # BLOCK SEXTANT-1256
        6) printf '🬹' ;; # BLOCK SEXTANT-3456
        7) printf '█' ;;
        *) printf ' ' ;;
    esac
}

# Pacing marker cell for a reset window. Returns non-zero when no marker
# can be computed.
# Args: $1 = reset timestamp/description, $2 = window minutes, $3 = width.
showy_bar_elapsed_marker_cell() {
    local reset_at="$1" window_minutes="$2" width="$3"
    [[ -n "${reset_at}" && "${window_minutes}" =~ ^[0-9]+$ ]] || return 1
    (( window_minutes > 0 )) || return 1

    local reset_epoch duration start_epoch now elapsed marker
    reset_epoch=$(showy_bar_reset_epoch "${reset_at}") || return 1
    duration=$((window_minutes * 60))
    start_epoch=$((reset_epoch - duration))
    now=$(showy_bar_now_epoch)
    elapsed=$((now - start_epoch))
    (( elapsed < 0 )) && elapsed=0
    (( elapsed > duration )) && elapsed="${duration}"
    marker=$(( (duration - elapsed) * width / duration ))
    (( marker < 0 )) && marker=0
    (( marker >= width )) && marker=$((width - 1))
    printf '%s\n' "${marker}"
}

# Pacing marker boundary for a reset window. Returns non-zero when no marker
# can be computed. Unlike showy_bar_elapsed_marker_cell, this prints a boundary
# index in 0..width so renderers can insert a separator between cells.
# Args: $1 = reset timestamp/description, $2 = window minutes, $3 = width.
showy_bar_elapsed_marker_boundary() {
    local reset_at="$1" window_minutes="$2" width="$3"
    [[ -n "${reset_at}" && "${window_minutes}" =~ ^[0-9]+$ && "${width}" =~ ^[0-9]+$ ]] || return 1
    (( window_minutes > 0 && width > 0 )) || return 1

    local reset_epoch duration start_epoch now elapsed boundary
    reset_epoch=$(showy_bar_reset_epoch "${reset_at}") || return 1
    duration=$((window_minutes * 60))
    start_epoch=$((reset_epoch - duration))
    now=$(showy_bar_now_epoch)
    elapsed=$((now - start_epoch))
    (( elapsed < 0 )) && elapsed=0
    (( elapsed > duration )) && elapsed="${duration}"
    boundary=$(( (duration - elapsed) * width / duration ))
    (( boundary < 0 )) && boundary=0
    (( boundary > width )) && boundary="${width}"
    printf '%s\n' "${boundary}"
}

showy_bar_shared_window_marker_boundary() {
    local width="$1"
    local p_used="$2" p_reset="$3" p_window="$4"
    local s_used="$5" s_reset="$6" s_window="$7"
    local t_used="$8" t_reset="$9" t_window="${10}"
    local count=0 ref_reset="" ref_window="" ref_epoch="" epoch

    if [[ "${p_used}" =~ ^[0-9]+$ && -n "${p_reset}" && "${p_window}" =~ ^[0-9]+$ ]] && (( p_window > 0 )); then
        epoch=$(showy_bar_reset_epoch "${p_reset}") || epoch=""
        if [[ -n "${epoch}" ]]; then
            ref_reset="${p_reset}"
            ref_window="${p_window}"
            ref_epoch="${epoch}"
            count=1
        fi
    fi

    if [[ "${s_used}" =~ ^[0-9]+$ && -n "${s_reset}" && "${s_window}" =~ ^[0-9]+$ ]] && (( s_window > 0 )); then
        epoch=$(showy_bar_reset_epoch "${s_reset}") || epoch=""
        if [[ -n "${epoch}" ]]; then
            if (( count == 0 )); then
                ref_reset="${s_reset}"
                ref_window="${s_window}"
                ref_epoch="${epoch}"
                count=1
            elif [[ "${s_window}" == "${ref_window}" && "${epoch}" == "${ref_epoch}" ]]; then
                count=$((count + 1))
            else
                return 1
            fi
        fi
    fi

    if [[ "${t_used}" =~ ^[0-9]+$ && -n "${t_reset}" && "${t_window}" =~ ^[0-9]+$ ]] && (( t_window > 0 )); then
        epoch=$(showy_bar_reset_epoch "${t_reset}") || epoch=""
        if [[ -n "${epoch}" ]]; then
            if (( count == 0 )); then
                ref_reset="${t_reset}"
                ref_window="${t_window}"
                ref_epoch="${epoch}"
                count=1
            elif [[ "${t_window}" == "${ref_window}" && "${epoch}" == "${ref_epoch}" ]]; then
                count=$((count + 1))
            else
                return 1
            fi
        fi
    fi

    (( count >= 2 )) || return 1
    showy_bar_elapsed_marker_boundary "${ref_reset}" "${ref_window}" "${width}"
}

showy_bar_row_marker_boundary() {
    local width="$1" used="$2" reset_at="$3" window_minutes="$4"
    [[ "${used}" =~ ^[0-9]+$ ]] || return 1
    showy_bar_elapsed_marker_boundary "${reset_at}" "${window_minutes}" "${width}"
}

showy_bar_mono3_marker_boundary() {
    local width="$1"
    local p_used="$2" p_reset="$3" p_window="$4"
    local s_used="$5" s_reset="$6" s_window="$7"
    local t_used="$8" t_reset="$9" t_window="${10}"

    case "${SHOWY_BAR_MONO3_MARKER_SOURCE:-primary}" in
        primary|"")
            showy_bar_row_marker_boundary "${width}" "${p_used}" "${p_reset}" "${p_window}"
            ;;
        secondary)
            showy_bar_row_marker_boundary "${width}" "${s_used}" "${s_reset}" "${s_window}"
            ;;
        tertiary)
            showy_bar_row_marker_boundary "${width}" "${t_used}" "${t_reset}" "${t_window}"
            ;;
        shared)
            showy_bar_shared_window_marker_boundary "${width}" \
                "${p_used}" "${p_reset}" "${p_window}" \
                "${s_used}" "${s_reset}" "${s_window}" \
                "${t_used}" "${t_reset}" "${t_window}"
            ;;
        none)
            return 1
            ;;
        *)
            showy_bar_row_marker_boundary "${width}" "${p_used}" "${p_reset}" "${p_window}"
            ;;
    esac
}


showy_bar_min_remaining() {
    local p_remaining="$1" s_remaining="$2" t_remaining="$3"
    local p_used="$4" s_used="$5" t_used="$6"
    local lowest=""

    if [[ "${p_used}" =~ ^[0-9]+$ && "${p_remaining}" =~ ^-?[0-9]+$ ]]; then
        lowest="${p_remaining}"
    fi
    if [[ "${s_used}" =~ ^[0-9]+$ && "${s_remaining}" =~ ^-?[0-9]+$ ]] && [[ -z "${lowest}" || "${s_remaining}" -lt "${lowest}" ]]; then
        lowest="${s_remaining}"
    fi
    if [[ "${t_used}" =~ ^[0-9]+$ && "${t_remaining}" =~ ^-?[0-9]+$ ]] && [[ -z "${lowest}" || "${t_remaining}" -lt "${lowest}" ]]; then
        lowest="${t_remaining}"
    fi

    printf '%s\n' "${lowest:-0}"
}

showy_bar_mono3_color() {
    local p_remaining="$1" s_remaining="$2" t_remaining="$3"
    local p_used="$4" s_used="$5" t_used="$6"
    local remaining

    case "${SHOWY_BAR_MONO3_COLOR_MODE:-lowest}" in
        primary)
            remaining="${p_remaining}"
            ;;
        lowest|"")
            remaining=$(showy_bar_min_remaining "${p_remaining}" "${s_remaining}" "${t_remaining}" "${p_used}" "${s_used}" "${t_used}")
            ;;
        *)
            remaining=$(showy_bar_min_remaining "${p_remaining}" "${s_remaining}" "${t_remaining}" "${p_used}" "${s_used}" "${t_used}")
            ;;
    esac

    showy_bar_role_palette primary "$(showy_bar_color_key "${remaining}")"
}

showy_bar_csv_contains() {
    local list="${1:-}" needle="$2" item
    local -a items=()

    [[ -n "${needle}" ]] || return 1
    local IFS=,
    read -r -a items <<< "${list}"
    for item in "${items[@]}"; do
        item="${item#"${item%%[![:space:]]*}"}"
        item="${item%"${item##*[![:space:]]}"}"
        [[ "${item}" == "${needle}" ]] && return 0
    done
    return 1
}

showy_bar_terminal_mode_for_provider() {
    local provider="$1"

    case "${SHOWY_BAR_TERMINAL_BAR_MODE:-auto}" in
        dual)
            printf 'dual\n'
            ;;
        sextant3)
            printf 'sextant3\n'
            ;;
        mono3)
            printf 'mono3\n'
            ;;
        auto|"")
            if showy_bar_csv_contains "${SHOWY_BAR_MONO3_PROVIDERS_EXCLUDE:-}" "${provider}"; then
                printf 'dual\n'
            elif showy_bar_csv_contains "${SHOWY_BAR_MONO3_PROVIDERS:-}" "${provider}"; then
                printf 'mono3\n'
            else
                printf 'dual\n'
            fi
            ;;
        *)
            printf 'dual\n'
            ;;
    esac
}

# Choose the dominant color for a provider record (uses the lowest of all
# windows' remaining-percents).
showy_bar_provider_color_key() {
    local jq_in="$1"
    local lowest
    lowest=$(printf '%s' "${jq_in}" | jq -r '
        def pct(x): [0, ([100, (x | tonumber | floor)] | min)] | max;
        [ .usage.primary, .usage.secondary, .usage.tertiary ]
        | map(select(. != null and (.usedPercent | type == "number")) | (100 - pct(.usedPercent)))
        | (min // 0)
    ')
    showy_bar_color_key "${lowest}"
}
