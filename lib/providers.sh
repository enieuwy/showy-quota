#!/usr/bin/env bash
# Provider identity from the shared registry. Read once per shell, with no child processes.
# shellcheck disable=SC2034 # Registry arrays are read by scripts sourcing this file.
# The guard is the array itself, never a scalar "loaded" flag: every scalar
# SHOWY_QUOTA_* variable is exported to child processes (showy_quota_export_config,
# the SketchyBar bootstrap), arrays never are, so a child that inherited such a
# flag skipped these declarations and died on its first lookup under `set -u`.
if ! declare -p SHOWY_QUOTA_PROVIDER_SIGILS >/dev/null 2>&1; then
    # -g: lib/common.sh is sometimes sourced inside a function (load_state_libs);
    # a plain declare there would make these local, gone when it returns.
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
fi
