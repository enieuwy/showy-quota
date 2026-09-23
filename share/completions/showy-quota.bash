# Bash completion for showy-quota, showy-quota-fetch, and showy-quota-state.
_showy_quota_complete() {
    local current="${COMP_WORDS[COMP_CWORD]}" previous="" command_name="${COMP_WORDS[0]##*/}" mode=""
    local name
    local -a themes=()
    COMPREPLY=()
    if (( COMP_CWORD > 0 )); then
        previous="${COMP_WORDS[COMP_CWORD-1]}"
    fi

    case "${command_name}" in
        showy-quota-fetch)
            mapfile -t COMPREPLY < <(compgen -W '--refresh --json --cache-only --age --path --stop-serve -h --help' -- "${current}")
            return ;;
        showy-quota-state)
            mapfile -t COMPREPLY < <(compgen -W '--json --count --providers --no-fetch -h --help' -- "${current}")
            return ;;
    esac

    mode="${COMP_WORDS[1]:-}"
    if [[ "${previous}" == '--set' || "${previous}" == '--preview' ]]; then
        while IFS= read -r name; do
            # --list is a local theme-directory scan, one plain name per line.
            [[ "${name}" =~ ^[[:alnum:]._-]+$ ]] && themes+=("${name}")
        done < <(showy-quota --list 2>/dev/null)
        mapfile -t COMPREPLY < <(compgen -W "${themes[*]}" -- "${current}")
        return
    fi
    if [[ "${mode}" == 'guard' ]]; then
        case "${previous}" in
            --window)
                mapfile -t COMPREPLY < <(compgen -W 'primary secondary tertiary worst' -- "${current}")
                return ;;
            --provider|--min-remaining|--max-used|--wait-max) return ;;
        esac
        mapfile -t COMPREPLY < <(compgen -W '--provider --window --min-remaining --max-used --no-fetch --allow-stale --wait-max --json --quiet -h --help' -- "${current}")
    elif [[ "${mode}" == 'prompt' ]]; then
        [[ "${previous}" == '--provider' ]] && return
        mapfile -t COMPREPLY < <(compgen -W '--ansi --provider -h --help' -- "${current}")
    elif [[ "${mode}" == '--diagnose' ]]; then
        mapfile -t COMPREPLY < <(compgen -W '--json --redact' -- "${current}")
    elif [[ "${mode}" == '--grant-zellij' ]]; then
        if [[ "${current}" == -* ]]; then
            mapfile -t COMPREPLY < <(compgen -W '--manage-serve --cli-fallback --force --dry-run --check' -- "${current}")
        else
            mapfile -t COMPREPLY < <(compgen -f -- "${current}")
            compopt -o filenames 2>/dev/null || true
        fi
    elif (( COMP_CWORD == 1 )); then
        mapfile -t COMPREPLY < <(compgen -W '--set --unset --current --list --preview --diagnose --grant-zellij guard prompt -h --help' -- "${current}")
    fi
}
complete -F _showy_quota_complete showy-quota showy-quota-fetch showy-quota-state
