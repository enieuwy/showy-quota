#!/usr/bin/env bash
# showy-quota - TPM wrapper for the tmux status-line renderer.

set -euo pipefail

CURRENT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
DEFAULT_BAR_BIN="${CURRENT_DIR}/bin/showy-quota-tmux-bar"

showy_quota_tmux_message() {
    local message="$1"
    if command -v tmux >/dev/null 2>&1 && tmux display-message "${message}" >/dev/null 2>&1; then
        return 0
    fi
    printf '%s\n' "${message}" >&2
}

if ! command -v tmux >/dev/null 2>&1; then
    printf '%s\n' 'showy-quota: tmux command not found; cannot install tmux status integration' >&2
    exit 1
fi

showy_quota_tmux_option() {
    local name="$1" default_value="$2" value
    if showy_quota_tmux_option_is_set "${name}"; then
        value="$(tmux show-option -gqv "${name}" 2>/dev/null || true)"
        printf '%s\n' "${value}"
    else
        printf '%s\n' "${default_value}"
    fi
}

showy_quota_tmux_option_is_set() {
    local wanted="$1" option rest
    while IFS=' ' read -r option rest; do
        [[ "${option}" == "${wanted}" ]] && return 0
    done < <(tmux show-options -gq 2>/dev/null || true)
    return 1
}

showy_quota_expand_user_path() {
    local value="$1"
    case "${value}" in
        ~)
            printf '%s\n' "${HOME}"
            ;;
        ~/*)
            printf '%s/%s\n' "${HOME}" "${value#~/}"
            ;;
        *)
            printf '%s\n' "${value}"
            ;;
    esac
}

showy_quota_escape_double_quotes() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    printf '%s\n' "${value}"
}

showy_quota_min_status_length() {
    local option="$1" wanted="$2" current
    [[ "${wanted}" =~ ^[0-9]+$ ]] || return 0
    current="$(tmux show-option -gqv "${option}" 2>/dev/null || printf '0')"
    [[ "${current}" =~ ^[0-9]+$ ]] || current=0
    if (( current < wanted )); then
        tmux set-option -gq "${option}" "${wanted}"
    fi
}

bar_bin="$(showy_quota_tmux_option '@showy-quota-bin' "${DEFAULT_BAR_BIN}")"
[[ -n "${bar_bin}" ]] || bar_bin="${DEFAULT_BAR_BIN}"
bar_bin="$(showy_quota_expand_user_path "${bar_bin}")"
status_side="$(showy_quota_tmux_option '@showy-quota-position' 'right')"
status_length="$(showy_quota_tmux_option '@showy-quota-status-length' '300')"
separator="$(showy_quota_tmux_option '@showy-quota-separator' ' ')"

case "${status_side}" in
    right)
        status_option="status-right"
        status_length_option="status-right-length"
        ;;
    left)
        status_option="status-left"
        status_length_option="status-left-length"
        ;;
    off|none|disabled)
        status_option=""
        status_length_option=""
        ;;
    *)
        tmux display-message "showy-quota: unsupported @showy-quota-position '${status_side}', using right"
        status_option="status-right"
        status_length_option="status-right-length"
        ;;
esac

if [[ -n "${status_option}" ]]; then
    if [[ ! -x "${bar_bin}" ]]; then
        showy_quota_tmux_message "showy-quota: renderer is not executable: ${bar_bin}"
    else
        showy_quota_min_status_length "${status_length_option}" "${status_length}"
        current_status="$(tmux show-option -gqv "${status_option}" 2>/dev/null || true)"
        if [[ "${current_status}" != *"showy-quota-tmux-bar"* && "${current_status}" != *"${bar_bin}"* ]]; then
            escaped_bar_bin="$(showy_quota_escape_double_quotes "${bar_bin}")"
            tmux set-option -gq -a "${status_option}" "${separator}#(\"${escaped_bar_bin}\")"
        fi
    fi
fi

popup_key="$(showy_quota_tmux_option '@showy-quota-popup-key' '')"
case "${popup_key}" in
    ''|off|none|disabled)
        ;;
    *)
        popup_height="$(showy_quota_tmux_option '@showy-quota-popup-height' '36')"
        popup_width="$(showy_quota_tmux_option '@showy-quota-popup-width' '92')"
        popup_interval="$(showy_quota_tmux_option '@showy-quota-popup-interval' '30')"
        popup_title="$(showy_quota_tmux_option '@showy-quota-popup-title' 'CodexBar usage')"
        [[ "${popup_interval}" =~ ^[0-9]+$ ]] || popup_interval=30

        # shellcheck disable=SC2016
        popup_command='config="${XDG_CONFIG_HOME:-$HOME/.config}/showy-quota/config.env"; [ -r "$config" ] && . "$config"; while :; do clear; "${SHOWY_QUOTA_CODEXBAR_BIN:-codexbar}" usage; sleep '"${popup_interval}"'; done'
        tmux bind-key "${popup_key}" display-popup -E -h "${popup_height}" -w "${popup_width}" -T "${popup_title}" "${popup_command}"
        ;;
esac

tmux refresh-client -S 2>/dev/null || true
