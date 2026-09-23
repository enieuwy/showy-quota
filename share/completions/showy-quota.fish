# Fish completion for showy-quota, showy-quota-fetch, and showy-quota-state.
function __showy_quota_mode_is -a mode
    set -l tokens (commandline -opc)
    test (count $tokens) -ge 2; and test "$tokens[2]" = "$mode"
end
function __showy_quota_at_root
    test (count (commandline -opc)) -eq 1
end
function __showy_quota_serve_action
    set -l tokens (commandline -opc)
    test (count $tokens) -eq 2; and test "$tokens[2]" = serve
end
function __showy_quota_serve_status
    set -l tokens (commandline -opc)
    test (count $tokens) -ge 3; and test "$tokens[2]" = serve; and test "$tokens[3]" = status
end


complete -c showy-quota -f
complete -c showy-quota -n '__showy_quota_at_root; or __showy_quota_mode_is --set' -l set -r -f -a '(showy-quota --list)'
complete -c showy-quota -n '__showy_quota_at_root' -l unset
complete -c showy-quota -n '__showy_quota_at_root' -l current
complete -c showy-quota -n '__showy_quota_at_root' -l list
complete -c showy-quota -n '__showy_quota_at_root; or __showy_quota_mode_is --preview' -l preview -r -f -a '(showy-quota --list)'
complete -c showy-quota -n '__showy_quota_at_root' -l diagnose
complete -c showy-quota -n '__showy_quota_at_root' -l check-config
complete -c showy-quota -n '__showy_quota_at_root' -l grant-zellij
complete -c showy-quota -n '__showy_quota_at_root' -s h -l help
complete -c showy-quota -n '__showy_quota_at_root' -a 'guard prompt serve next-reset run pick refresh'
for mode in --diagnose --check-config
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l json
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l redact
end
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -l manage-serve
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -l cli-fallback
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -l force
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -l dry-run
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -l check
complete -c showy-quota -n '__showy_quota_mode_is --grant-zellij' -F
for mode in guard run
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l provider -r -f
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l min-remaining -r -f
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l max-used -r -f
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l allow-stale
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l wait-max -r -f
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l quiet
end
for mode in guard run next-reset pick
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l window -r -f -a 'primary secondary tertiary worst'
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l no-fetch
end
for mode in guard run next-reset
    complete -c showy-quota -n "__showy_quota_mode_is $mode" -l json
end
complete -c showy-quota -n '__showy_quota_mode_is next-reset' -l provider -r -f
complete -c showy-quota -n '__showy_quota_mode_is next-reset' -l epoch
complete -c showy-quota -n '__showy_quota_mode_is next-reset' -l seconds
complete -c showy-quota -n '__showy_quota_mode_is pick' -l min-remaining -r -f
complete -c showy-quota -n '__showy_quota_mode_is pick' -l format -r -f -a 'id json'
complete -c showy-quota -n '__showy_quota_mode_is pick' -l providers -r -f
complete -c showy-quota -n '__showy_quota_mode_is prompt' -l ansi
complete -c showy-quota -n '__showy_quota_mode_is prompt' -l provider -r -f
complete -c showy-quota -n '__showy_quota_mode_is prompt' -l format -r -f
complete -c showy-quota -n '__showy_quota_serve_action' -a 'status restart stop'
complete -c showy-quota -n '__showy_quota_serve_status' -l json

complete -c showy-quota-fetch -f
complete -c showy-quota-fetch -l refresh
complete -c showy-quota-fetch -l json
complete -c showy-quota-fetch -l cache-only
complete -c showy-quota-fetch -l age
complete -c showy-quota-fetch -l path
complete -c showy-quota-fetch -l stop-serve
complete -c showy-quota-fetch -l serve-status
complete -c showy-quota-fetch -l restart-serve
complete -c showy-quota-fetch -s h -l help

complete -c showy-quota-state -f
complete -c showy-quota-state -l json
complete -c showy-quota-state -l count
complete -c showy-quota-state -l providers
complete -c showy-quota-state -l explain
complete -c showy-quota-state -l no-fetch
complete -c showy-quota-state -s h -l help
