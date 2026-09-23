# Fish completion for showy-quota, showy-quota-fetch, and showy-quota-state.
function __showy_quota_mode_is -a mode
    set -l tokens (commandline -opc)
    test (count $tokens) -ge 2; and test "$tokens[2]" = "$mode"
end
function __showy_quota_at_root
    test (count (commandline -opc)) -eq 1
end


complete -c showy-quota -f
complete -c showy-quota -n '__showy_quota_at_root; or __showy_quota_mode_is --set' -l set -r -f -a '(showy-quota --list)'
complete -c showy-quota -n '__showy_quota_at_root' -l unset
complete -c showy-quota -n '__showy_quota_at_root' -l current
complete -c showy-quota -n '__showy_quota_at_root' -l list
complete -c showy-quota -n '__showy_quota_at_root; or __showy_quota_mode_is --preview' -l preview -r -f -a '(showy-quota --list)'
complete -c showy-quota -n '__showy_quota_at_root' -l diagnose
complete -c showy-quota -n '__showy_quota_at_root' -l grant-zellij
complete -c showy-quota -n '__showy_quota_at_root' -s h -l help
complete -c showy-quota -n '__showy_quota_at_root' -a 'guard prompt'
complete -c showy-quota -n '__showy_quota_mode_is --diagnose' -l json
complete -c showy-quota -n '__showy_quota_mode_is --diagnose' -l redact
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -l manage-serve
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -l cli-fallback
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -l force
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -l dry-run
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -l check
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -F
complete -c showy-quota -n '__showy_quota_mode_is guard' -l provider -r -f
complete -c showy-quota -n '__showy_quota_mode_is guard' -l window -r -f -a 'primary secondary tertiary worst'
complete -c showy-quota -n '__showy_quota_mode_is guard' -l min-remaining -r -f
complete -c showy-quota -n '__showy_quota_mode_is guard' -l max-used -r -f
complete -c showy-quota -n '__showy_quota_mode_is guard' -l no-fetch
complete -c showy-quota -n '__showy_quota_mode_is guard' -l allow-stale
complete -c showy-quota -n '__showy_quota_mode_is guard' -l wait-max -r -f
complete -c showy-quota -n '__showy_quota_mode_is guard' -l json
complete -c showy-quota -n '__showy_quota_mode_is guard' -l quiet
complete -c showy-quota -n '__showy_quota_mode_is prompt' -l ansi
complete -c showy-quota -n '__showy_quota_mode_is prompt' -l provider -r -f

complete -c showy-quota-fetch -f
complete -c showy-quota-fetch -l refresh
complete -c showy-quota-fetch -l json
complete -c showy-quota-fetch -l cache-only
complete -c showy-quota-fetch -l age
complete -c showy-quota-fetch -l path
complete -c showy-quota-fetch -l stop-serve
complete -c showy-quota-fetch -s h -l help

complete -c showy-quota-state -f
complete -c showy-quota-state -l json
complete -c showy-quota-state -l count
complete -c showy-quota-state -l providers
complete -c showy-quota-state -l no-fetch
complete -c showy-quota-state -s h -l help
