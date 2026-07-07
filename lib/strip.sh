#!/usr/bin/env bash
# showy-quota — shared strip data helpers for shell integrations that inspect
# provider order/layout without rendering the terminal bar themselves.
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

