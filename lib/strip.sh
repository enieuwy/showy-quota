#!/usr/bin/env bash
# codexbar-bars — strip rendering helpers used by both the Zellij ANSI
# renderer and the tmux markup renderer.
#
# Sourced after lib/common.sh.

# Provider id → short two-or-three letter sigil shown when no Nerd Font icon
# is available. Keep these stable so users can recognize them in the strip.
cb_bars_provider_sigil() {
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
# CB_BARS_PROVIDERS when set. Reads from stdin, writes JSON array to stdout.
cb_bars_filter_renderable() {
    local allow="${CB_BARS_PROVIDERS:-}"
    if [[ -z "${allow}" ]]; then
        jq '[ .[] | select((.error // null) == null and (.usage.primary // null) != null) ]'
        return
    fi
    # Comma-separated → jq array literal.
    local jq_list=""
    local IFS=,
    # shellcheck disable=SC2206
    local items=($allow)
    local first=1
    for it in "${items[@]}"; do
        it="${it// /}"
        [[ -z "${it}" ]] && continue
        if (( first )); then jq_list="\"${it}\""; first=0
        else jq_list="${jq_list},\"${it}\""; fi
    done
    jq --argjson allow "[${jq_list}]" \
       '[ .[] | select((.error // null) == null and (.usage.primary // null) != null and (.provider as $p | $allow | index($p))) ]'
}

# Render a single window slot as JSON: { used_pct, remaining_pct, reset_at,
# window_minutes, color }.
cb_bars_window_jq() {
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
cb_bars_block_bar() {
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

# Choose the dominant color for a provider record (uses the lowest of all
# windows' remaining-percents).
cb_bars_provider_color_key() {
    local jq_in="$1"
    local lowest
    lowest=$(printf '%s' "${jq_in}" | jq -r '
        [ .usage.primary, .usage.secondary, .usage.tertiary ]
        | map(select(. != null) | (100 - (.usedPercent // 0)))
        | (min // 0)
    ')
    cb_bars_color_key "${lowest}"
}
