#!/usr/bin/env bash
# Provider identity from the shared registry. Read once per shell, with no child processes.
# shellcheck disable=SC2034 # Registry arrays are read by scripts sourcing this file.
if [[ -z "${SHOWY_QUOTA_PROVIDER_REGISTRY_LOADED:-}" ]]; then
    # -g: lib/common.sh is sometimes sourced inside a function (load_state_libs);
    # a plain declare there would make these local while the guard stays global.
    declare -gA SHOWY_QUOTA_PROVIDER_SIGILS=()
    declare -gA SHOWY_QUOTA_PROVIDER_FONT_ICONS=()
    SHOWY_QUOTA_PROVIDER_DEFAULT_ORDER=""
    while IFS=$'\t' read -r provider_id provider_sigil provider_rank provider_font; do
        [[ -n "${provider_id}" && "${provider_id}" != \#* ]] || continue
        SHOWY_QUOTA_PROVIDER_SIGILS["${provider_id}"]="${provider_sigil}"
        if [[ "${provider_font}" != "-" ]]; then
            SHOWY_QUOTA_PROVIDER_FONT_ICONS["${provider_id}"]="${provider_font}"
        fi
        if [[ "${provider_rank}" != "-" ]]; then
            SHOWY_QUOTA_PROVIDER_DEFAULT_ORDER+="${SHOWY_QUOTA_PROVIDER_DEFAULT_ORDER:+,}${provider_id}"
        fi
    done < "${BASH_SOURCE[0]%/*}/../share/providers.tsv"
    SHOWY_QUOTA_PROVIDER_REGISTRY_LOADED=1
fi
