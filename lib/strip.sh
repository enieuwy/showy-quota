#!/usr/bin/env bash
# showy-quota — strip rendering helpers used by both the Zellij ANSI
# renderer and the tmux markup renderer.
#
# Sourced after lib/common.sh.

# Provider id → short two-or-three letter sigil shown when no Nerd Font icon
# is available. Keep these stable so users can recognize them in the strip.
showy_quota_provider_sigil() {
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

# Filter the cached JSON down to provider records with at least one numeric
# usage window, honoring SHOWY_QUOTA_PROVIDERS, SHOWY_QUOTA_PROVIDERS_EXCLUDE,
# and provider ordering when set. Reads from stdin, writes JSON array to stdout.
showy_quota_filter_renderable() {
    local allow="${SHOWY_QUOTA_PROVIDERS:-}"
    local exclude="${SHOWY_QUOTA_PROVIDERS_EXCLUDE:-}"
    local order="${SHOWY_QUOTA_PROVIDER_ORDER:-}"
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
            and ([
                .usage.primary,
                .usage.secondary,
                .usage.tertiary
            ] | any(. != null and (.usedPercent | type == "number")))
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
showy_quota_window_jq() {
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
showy_quota_block_bar() {
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
showy_quota_filled_cells() {
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
showy_quota_sextant_mask_char() {
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

# Unicode 16 octant mosaic glyphs for one cell split into four stacked rows:
# lane1(top)=1, lane2=2, lane3=4, lane4(bottom)=8. Masks absent from the octant
# block fall back to quarter/half/full block elements.
showy_quota_octant_mask_char() {
    case "$1" in
        0)  printf ' ' ;;
        1)  printf '\U0001FB82' ;; # UPPER ONE QUARTER BLOCK
        2)  printf '\U0001CD06' ;; # BLOCK OCTANT-34
        3)  printf '\U00002580' ;; # UPPER HALF BLOCK
        4)  printf '\U0001CD27' ;; # BLOCK OCTANT-56
        5)  printf '\U0001CD2A' ;; # BLOCK OCTANT-1256
        6)  printf '\U0001CD33' ;; # BLOCK OCTANT-3456
        7)  printf '\U0001FB85' ;; # UPPER THREE QUARTERS BLOCK
        8)  printf '\U00002582' ;; # LOWER ONE QUARTER BLOCK
        9)  printf '\U0001CDAE' ;; # BLOCK OCTANT-1278
        10) printf '\U0001CDB7' ;; # BLOCK OCTANT-3478
        11) printf '\U0001CDBA' ;; # BLOCK OCTANT-123478
        12) printf '\U00002584' ;; # LOWER HALF BLOCK
        13) printf '\U0001CDDD' ;; # BLOCK OCTANT-125678
        14) printf '\U00002586' ;; # LOWER THREE QUARTERS BLOCK
        15) printf '\U00002588' ;; # FULL BLOCK
        *)  printf ' ' ;;
    esac
}

# Single chunk color for mono3/mono4. Lanes passed as \x1f-joined parallel
# lists. Mirrors the core: representative remaining (lowest present, or primary
# slot when MONO_COLOR_MODE=primary), dimmed only when every present lane is a
# long-horizon cap and force_bright is 0. Args: remaining_list window_list
# present_list [force_bright].
showy_quota_mono_chunk_color() {
    local rem_s="$1" win_s="$2" present_s="$3" force_bright="${4:-0}"
    local -a rem win present
    local IFS=$'\x1f'
    read -r -a rem <<< "${rem_s}"
    read -r -a win <<< "${win_s}"
    read -r -a present <<< "${present_s}"
    IFS=$' \t\n'

    local n="${#rem[@]}" i rep="" any=0 all=1
    if [[ "${SHOWY_QUOTA_MONO_COLOR_MODE:-lowest}" == "primary" ]]; then
        rep="${rem[0]:-0}"
    else
        for (( i=0; i<n; i++ )); do
            [[ "${present[i]:-0}" == "1" ]] || continue
            if [[ -z "${rep}" || "${rem[i]}" -lt "${rep}" ]]; then
                rep="${rem[i]}"
            fi
        done
        [[ -n "${rep}" ]] || rep=0
    fi
    for (( i=0; i<n; i++ )); do
        [[ "${present[i]:-0}" == "1" ]] || continue
        any=1
        [[ "$(showy_quota_is_long_window "${win[i]:-}")" == "1" ]] || all=0
    done
    local dim=0
    (( any && all )) && dim=1
    (( force_bright )) && dim=0
    showy_quota_window_color "${rep}" "${dim}"
}

# Render the single-color stacked body via the given styler fn (style_text for
# Zellij ANSI, tmux_style_text for tmux markup); 3 lanes -> sextants, 4 -> octants.
# Each SHOWY_QUOTA_MONO_MARKERS slot replaces its column with a colored separator
# (first marker = palette elapsed, the rest = palette elapsed_long).
# Args: styler width mono_color remaining_list reset_list window_list present_list.
showy_quota_mono_lane_bar() {
    local styler="$1" width="$2" mono_color="$3"
    local rem_s="$4" reset_s="$5" win_s="$6" present_s="$7"
    [[ "${width}" =~ ^[0-9]+$ ]] || width=12
    (( width < 8 )) && width=8
    local surface bg
    surface="$(showy_quota_palette surface)"
    bg="$(showy_quota_palette bg)"

    local -a rem reset win present fill
    local IFS=$'\x1f'
    read -r -a rem <<< "${rem_s}"
    read -r -a reset <<< "${reset_s}"
    read -r -a win <<< "${win_s}"
    read -r -a present <<< "${present_s}"
    IFS=$' \t\n'

    local n="${#rem[@]}" i
    for (( i=0; i<n; i++ )); do
        fill[i]="$(showy_quota_filled_cells "${rem[i]:-0}" "${width}")"
    done

    local -A marker_at=()
    local rank=0 name idx col color
    local IFS_save="$IFS"
    IFS=,
    for name in ${SHOWY_QUOTA_MONO_MARKERS:-}; do
        name="${name#"${name%%[![:space:]]*}"}"
        name="${name%"${name##*[![:space:]]}"}"
        case "${name}" in
            primary) idx=0 ;;
            secondary) idx=1 ;;
            tertiary) idx=2 ;;
            quaternary) idx=3 ;;
            *) idx=-1 ;;
        esac
        if (( idx >= 0 && idx < n )) && [[ "${present[idx]:-0}" == "1" ]]; then
            col="$(showy_quota_elapsed_marker_cell "${reset[idx]}" "${win[idx]}" "${width}" || true)"
            if [[ "${col}" =~ ^[0-9]+$ && -z "${marker_at[${col}]+x}" ]]; then
                if (( rank == 0 )); then
                    color="$(showy_quota_palette elapsed)"
                else
                    color="$(showy_quota_palette elapsed_long)"
                fi
                marker_at[${col}]="${color}"
            fi
        fi
        rank=$((rank + 1))
    done
    IFS="${IFS_save}"

    local octant=0
    (( n >= 4 )) && octant=1
    local mask glyph cell_color l
    for (( i=0; i<width; i++ )); do
        if [[ -n "${marker_at[${i}]+x}" ]]; then
            "${styler}" '│' "${marker_at[${i}]}" "${surface}"
            continue
        fi
        mask=0
        for (( l=0; l<n; l++ )); do
            (( i < ${fill[l]:-0} )) && mask=$((mask | (1 << l)))
        done
        if (( octant )); then
            glyph="$(showy_quota_octant_mask_char "${mask}")"
        else
            glyph="$(showy_quota_sextant_mask_char "${mask}")"
        fi
        if (( mask == 0 )); then
            cell_color="${surface}"
        else
            cell_color="${mono_color}"
        fi
        "${styler}" "${glyph}" "${cell_color}" "${surface}"
    done
    "${styler}" '▏' "${bg}" "${surface}"
}

# Superscript form of a family initial for the split sigil (AG -> AGᴳ), or the
# plain letter where no modifier-letter glyph exists. Mirrors render.rs.
showy_quota_superscript() {
    case "$1" in
        A|a) printf 'ᴬ' ;;
        B|b) printf 'ᴮ' ;;
        C|c) printf 'ᶜ' ;;
        D|d) printf 'ᴰ' ;;
        E|e) printf 'ᴱ' ;;
        F|f) printf 'ᶠ' ;;
        G|g) printf 'ᴳ' ;;
        H|h) printf 'ᴴ' ;;
        I|i) printf 'ᴵ' ;;
        J|j) printf 'ᴶ' ;;
        K|k) printf 'ᴷ' ;;
        L|l) printf 'ᴸ' ;;
        M|m) printf 'ᴹ' ;;
        N|n) printf 'ᴺ' ;;
        O|o) printf 'ᴼ' ;;
        P|p) printf 'ᴾ' ;;
        R|r) printf 'ᴿ' ;;
        S|s) printf 'ˢ' ;;
        T|t) printf 'ᵀ' ;;
        U|u) printf 'ᵁ' ;;
        W|w) printf 'ᵂ' ;;
        X|x) printf 'ˣ' ;;
        Z|z) printf 'ᶻ' ;;
        *) printf '%s' "$1" ;;
    esac
}

# Pacing marker cell for a reset window. Returns non-zero when no marker
# can be computed.
# Args: $1 = reset timestamp/description, $2 = window minutes, $3 = width.
showy_quota_elapsed_marker_cell() {
    local reset_at="$1" window_minutes="$2" width="$3"
    [[ -n "${reset_at}" && "${window_minutes}" =~ ^[0-9]+$ ]] || return 1
    (( window_minutes > 0 )) || return 1

    local reset_epoch duration start_epoch now elapsed marker
    reset_epoch=$(showy_quota_reset_epoch "${reset_at}") || return 1
    duration=$((window_minutes * 60))
    start_epoch=$((reset_epoch - duration))
    now=$(showy_quota_now_epoch)
    elapsed=$((now - start_epoch))
    (( elapsed < 0 )) && elapsed=0
    (( elapsed > duration )) && elapsed="${duration}"
    marker=$(( (duration - elapsed) * width / duration ))
    (( marker < 0 )) && marker=0
    (( marker >= width )) && marker=$((width - 1))
    printf '%s\n' "${marker}"
}

# Explicit per-provider mode from SHOWY_QUOTA_PROVIDER_MODES ("p=mode,..."), or
# empty when the provider has no override.
showy_quota_mode_for_provider() {
    local provider="$1" entry k v
    local IFS=,
    for entry in ${SHOWY_QUOTA_PROVIDER_MODES:-}; do
        [[ "${entry}" == *"="* ]] || continue
        k="${entry%%=*}"
        v="${entry#*=}"
        k="${k#"${k%%[![:space:]]*}"}"; k="${k%"${k##*[![:space:]]}"}"
        v="${v#"${v%%[![:space:]]*}"}"; v="${v%"${v##*[![:space:]]}"}"
        if [[ "${k}" == "${provider}" && -n "${v}" ]]; then
            printf '%s' "${v}"
            return 0
        fi
    done
    return 0
}

# Resolve a provider's terminal body. Args: provider, has_tertiary (1/0),
# pooled (1/0 — auto-detected model-pooled provider whose extras carry every
# positional slot). mono3 collapses to dual without a tertiary slot; the family
# bodies (dual2/mono4) pass through and adapt to the pool count at render.
showy_quota_terminal_mode_for_provider() {
    local provider="$1" has_tertiary="${2:-0}" pooled="${3:-0}"
    local requested
    case "${SHOWY_QUOTA_TERMINAL_BAR_MODE:-auto}" in
        dual) requested=dual ;;
        dual2) requested=dual2 ;;
        mono3) requested=mono3 ;;
        mono4) requested=mono4 ;;
        *)
            requested="$(showy_quota_mode_for_provider "${provider}")"
            if [[ -z "${requested}" ]]; then
                if (( pooled )); then requested=dual2; else requested=dual; fi
            fi
            ;;
    esac

    local mode=dual
    case "${requested}" in
        mono3) [[ "${has_tertiary}" == "1" ]] && mode=mono3 || mode=dual ;;
        mono4) if (( ! pooled )) && [[ "${has_tertiary}" == "1" ]]; then mode=mono3; else mode=mono4; fi ;;
        dual2) mode=dual2 ;;
        *) mode=dual ;;
    esac

    printf '%s\n' "${mode}"
}

# Choose the dominant color for a provider record (uses the lowest of all
# windows' remaining-percents).
showy_quota_provider_color_key() {
    local jq_in="$1"
    local lowest
    lowest=$(printf '%s' "${jq_in}" | jq -r '
        def pct(x): [0, ([100, (x | tonumber | floor)] | min)] | max;
        [ .usage.primary, .usage.secondary, .usage.tertiary ]
        | map(select(. != null and (.usedPercent | type == "number")) | (100 - pct(.usedPercent)))
        | (min // 0)
    ')
    showy_quota_color_key "${lowest}"
}
