# Bash completion for showy-quota, showy-quota-fetch, and showy-quota-state.
_showy_quota_complete() {
    local current="${COMP_WORDS[COMP_CWORD]}" previous="" command_name="${COMP_WORDS[0]##*/}" mode=""
    local name words i
    local -a themes=()
    local guard_words='--provider --window --min-remaining --max-used --no-fetch --allow-stale --wait-max --json --quiet -h --help'
    COMPREPLY=()
    if (( COMP_CWORD > 0 )); then
        previous="${COMP_WORDS[COMP_CWORD-1]}"
    fi

    case "${command_name}" in
        showy-quota-fetch)
            mapfile -t COMPREPLY < <(compgen -W '--refresh --json --cache-only --age --path --stop-serve --serve-status --restart-serve -h --help' -- "${current}")
            return ;;
        showy-quota-state)
            mapfile -t COMPREPLY < <(compgen -W '--json --count --providers --explain --no-fetch -h --help' -- "${current}")
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
    case "${mode}" in
        guard|run|next-reset|pick)
            if [[ "${mode}" == 'run' ]]; then
                for (( i = 2; i < COMP_CWORD; i++ )); do
                    [[ "${COMP_WORDS[i]}" == '--' ]] || continue
                    # Everything after -- is the gated command.
                    if (( COMP_CWORD == i + 1 )); then
                        mapfile -t COMPREPLY < <(compgen -c -- "${current}")
                    else
                        mapfile -t COMPREPLY < <(compgen -f -- "${current}")
                        compopt -o filenames 2>/dev/null || true
                    fi
                    return
                done
            fi
            case "${previous}" in
                --window)
                    mapfile -t COMPREPLY < <(compgen -W 'primary secondary tertiary worst' -- "${current}")
                    return ;;
                --format)
                    mapfile -t COMPREPLY < <(compgen -W 'id json' -- "${current}")
                    return ;;
                --provider|--providers|--min-remaining|--max-used|--wait-max) return ;;
            esac
            case "${mode}" in
                guard) words="${guard_words}" ;;
                run) words="${guard_words} --" ;;
                next-reset) words='--provider --window --json --epoch --seconds --no-fetch -h --help' ;;
                pick) words='--min-remaining --window --format --no-fetch --providers -h --help' ;;
            esac
            mapfile -t COMPREPLY < <(compgen -W "${words}" -- "${current}") ;;
        prompt)
            [[ "${previous}" == '--provider' || "${previous}" == '--format' ]] && return
            mapfile -t COMPREPLY < <(compgen -W '--ansi --provider --format -h --help' -- "${current}") ;;
        serve)
            if (( COMP_CWORD == 2 )); then
                mapfile -t COMPREPLY < <(compgen -W 'status restart stop -h --help' -- "${current}")
            elif [[ "${COMP_WORDS[2]}" == 'status' ]]; then
                mapfile -t COMPREPLY < <(compgen -W '--json' -- "${current}")
            fi ;;
        --diagnose|--check-config)
            mapfile -t COMPREPLY < <(compgen -W '--json --redact' -- "${current}") ;;
        --grant-zellij)
            if [[ "${current}" == -* ]]; then
                mapfile -t COMPREPLY < <(compgen -W '--manage-serve --cli-fallback --force --dry-run --check' -- "${current}")
            else
                mapfile -t COMPREPLY < <(compgen -f -- "${current}")
                compopt -o filenames 2>/dev/null || true
            fi ;;
        *)
            if (( COMP_CWORD == 1 )); then
                mapfile -t COMPREPLY < <(compgen -W '--set --unset --current --list --preview --diagnose --check-config --grant-zellij guard prompt serve next-reset run pick refresh -h --help' -- "${current}")
            fi ;;
    esac
}
complete -F _showy_quota_complete showy-quota showy-quota-fetch showy-quota-state
