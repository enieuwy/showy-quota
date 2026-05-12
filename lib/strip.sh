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
# SHOWY_BAR_PROVIDERS and SHOWY_BAR_PROVIDERS_EXCLUDE when set. Reads from stdin,
# writes JSON array to stdout.
showy_bar_filter_renderable() {
    local allow="${SHOWY_BAR_PROVIDERS:-}"
    local exclude="${SHOWY_BAR_PROVIDERS_EXCLUDE:-}"
    jq --arg allow "${allow}" --arg exclude "${exclude}" '
        def list($raw):
            $raw
            | split(",")
            | map(gsub("^\\s+|\\s+$"; ""))
            | map(select(length > 0));
        (list($allow)) as $allow_list
        | (list($exclude)) as $exclude_list
        | [ .[] | select(
            (.error // null) == null
            and (.provider | type == "string" and test("^[A-Za-z0-9_.-]+$"))
            and (.usage.primary.usedPercent | type == "number")
            and (.provider as $p | (($allow_list | length) == 0 or ($allow_list | index($p) != null)))
            and (.provider as $p | ($exclude_list | index($p) == null))
        ) ]
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
