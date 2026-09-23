#!/usr/bin/env bash
# showy-quota — shared strip data helpers for shell integrations that inspect
# provider order/layout without rendering the terminal bar themselves.
#
# Sourced after lib/common.sh.

# Provider id → stable sigil. Unknown ids retain the two-character fallback.
showy_quota_provider_sigil() {
    if [[ -v SHOWY_QUOTA_PROVIDER_SIGILS["$1"] ]]; then
        printf '%s' "${SHOWY_QUOTA_PROVIDER_SIGILS[$1]}"
    else
        printf '%s' "${1:0:2}" | tr '[:lower:]' '[:upper:]'
    fi
}

# Filter and explain providers with the same reason predicate. The ordinary
# render path still runs one jq process and emits only the original records.
showy_quota_filter_records() {
    local mode="$1"
    jq --arg allow "${SHOWY_QUOTA_PROVIDERS:-}" \
        --arg exclude "${SHOWY_QUOTA_PROVIDERS_EXCLUDE:-}" \
        --arg order "${SHOWY_QUOTA_PROVIDER_ORDER:-}" \
        --arg mode "${mode}" '
        def valid_provider_id:
            type == "string"
            and test("^[A-Za-z0-9_.-]+$")
            and . != "."
            and . != ".."
            and (startswith("-") | not);
        def list($raw):
            $raw | split(",")
            | map(gsub("^\\s+|\\s+$"; ""))
            | map(select(length > 0));
        def pos($items; $provider):
            ($items | index($provider)) as $idx
            | if $idx == null then 1000000 else $idx end;
        def reason($allow_list; $exclude_list):
            if (.provider | valid_provider_id | not) then "invalid_id"
            elif (.error // null) != null then "error_record"
            elif ((.usage | type) != "object"
                or ([
                    .usage.primary,
                    .usage.secondary,
                    .usage.tertiary
                ] | any(. != null and (.usedPercent | type == "number")) | not))
                then "no_numeric_usage_window"
            elif (.provider as $p | $exclude_list | index($p)) != null
                then "excluded_by_denylist"
            elif ($allow_list | length) > 0
                and (.provider as $p | $allow_list | index($p)) == null
                then "excluded_by_allowlist"
            else "included" end;
        (list($allow)) as $allow_list
        | (list($exclude)) as $exclude_list
        | (list($order)) as $order_list
        | if $mode == "filtered" then
            [ .[] | select(reason($allow_list; $exclude_list) == "included") ] as $filtered
            | if ($allow_list | length) > 0 then
                $filtered | sort_by([(.provider as $p | pos($allow_list; $p)), .provider])
              elif ($order_list | length) > 0 then
                $filtered | sort_by([(.provider as $p | pos($order_list; $p)), .provider])
              else $filtered end
          else
            [to_entries[] | select(.value | reason($allow_list; $exclude_list) == "included")] as $included
            | (if ($allow_list | length) > 0 then
                $included | sort_by([(.value.provider as $p | pos($allow_list; $p)), .value.provider])
              elif ($order_list | length) > 0 then
                $included | sort_by([(.value.provider as $p | pos($order_list; $p)), .value.provider])
              else $included end) as $ordered
            | (. as $records
            | [ $records | to_entries[] | . as $entry
                | ($entry.value.provider // null) as $provider
                | ($entry.value | reason($allow_list; $exclude_list)) as $why
                | {
                    provider: $provider,
                    reason: $why,
                    sourceIndex: $entry.key,
                    position: (if $why == "included" then
                        ($ordered | map(.key) | index($entry.key))
                      else null end),
                    rankSource: (if ($allow_list | length) > 0 then "allowlist"
                                 elif ($order_list | length) > 0 then "provider_order"
                                 else "cache" end),
                    orderRank: (if ($allow_list | length) > 0 then
                        ($allow_list | index($provider))
                      elif ($order_list | length) > 0 then
                        ($order_list | index($provider))
                      else $entry.key end)
                }
            ])
          end
    '
}

showy_quota_filter_renderable() {
    showy_quota_filter_records filtered
}

showy_quota_explain_providers() {
    showy_quota_filter_records explain
}

