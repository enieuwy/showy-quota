const MAX_SUBPROCESS_STDOUT_BYTES: usize = 5 * 1024 * 1024;

/// POSIX-sh watchdog that runs an argv command in a separate process session,
/// bounds its captured stdout, and terminates the entire command tree on
/// timeout. Zellij exposes no cancellation API for `run_command`, so CLI
/// fallback work must clean up interactive helpers as well as its direct child.
fn watchdog_script_with_child(timeout_secs: u64, child_script: &str) -> String {
    let capture_limit = MAX_SUBPROCESS_STDOUT_BYTES.saturating_add(1);
    format!(
        r#"__d=$(mktemp -d "${{TMPDIR:-/tmp}}/showy-quota.XXXXXX") || exit 125
umask 077
__fifo="$__d/stdout"
__out="$__d/output"
mkfifo "$__fifo" || {{ rm -rf "$__d"; exit 125; }}
trap 'rm -rf "$__d"' EXIT
__setsid=0
if command -v setsid >/dev/null 2>&1; then
    setsid /bin/sh -c '{child_script}' showy-quota-child "$@" >"$__fifo" 2>/dev/null &
    __p=$!
    __setsid=1
else
    /bin/sh -c '{child_script}' showy-quota-child "$@" >"$__fifo" 2>/dev/null &
    __p=$!
fi
__kill_descendants() {{
    __children=$(pgrep -P "$1" 2>/dev/null || true)
    [ -n "$__children" ] || return 0
    while IFS= read -r __child; do
        [ -n "$__child" ] || continue
        __kill_descendants "$__child" "$2"
        kill "-$2" "$__child" 2>/dev/null || true
    done <<EOF
$__children
EOF
}}
__p_reaped=0
__k_reaped=0
__stop() {{
    # A reaped pid is a FREE pid, so signalling it is not a harmless no-op: the
    # id may already belong to an unrelated same-uid process, and the setsid
    # branch signals a whole process GROUP. Refuse once the child is waited on.
    [ "$__p_reaped" -eq 0 ] || return 0
    __signal=$1
    if [ "$__setsid" -eq 1 ]; then
        kill "-$__signal" -- "-$__p" 2>/dev/null || true
    else
        __kill_descendants "$__p" "$__signal"
        if command -v pkill >/dev/null 2>&1; then
            pkill "-$__signal" -P "$__p" 2>/dev/null || true
        fi
        kill "-$__signal" "$__p" 2>/dev/null || true
    fi
}}
__stop_timer() {{
    # Killing the timer subshell alone orphans its `sleep` grandchild: the
    # subshell's own EXIT trap does not reliably fire when it is signalled
    # mid-`wait`, and an orphaned sleep keeps the inherited stdout write end
    # open, so a caller capturing this script through a pipe would block until
    # the timeout elapsed even though the command already finished.
    [ "$__k_reaped" -eq 0 ] || return 0
    __kill_descendants "$__k" TERM
    kill "$__k" 2>/dev/null || true
    wait "$__k" 2>/dev/null
    __k_reaped=1
}}
(sleep {timeout_secs} & __s=$!; trap 'kill "$__s" 2>/dev/null' EXIT; wait "$__s"; __stop TERM; sleep 2 & __s=$!; wait "$__s"; __stop KILL) &
__k=$!
# Re-arm the EXIT trap now that __k/__stop/__p exist: on unclean shutdown
# (Zellij has no cancellation API and just signals this shell on pane
# close/unload) stop the timer and the whole child tree before removing the
# temp dir. The __p_reaped/__k_reaped guards make this genuinely idempotent:
# once a normal or oversize path has waited on a pid, the trap skips it instead
# of signalling an id the kernel may have handed to someone else.
trap '__stop_timer; __stop TERM; __stop KILL; wait "$__p" 2>/dev/null; rm -rf "$__d"' EXIT
dd if="$__fifo" of="$__out" bs=1 count={capture_limit} 2>/dev/null
__bytes=$(wc -c <"$__out" | tr -d '[:space:]')
if [ "$__bytes" -gt {max_stdout} ]; then
    __stop TERM
    sleep 2
    __stop KILL
    __stop_timer
    wait "$__p" 2>/dev/null
    __p_reaped=1
    exit 125
fi
cat "$__out"
wait "$__p"
__r=$?
__p_reaped=1
__stop_timer
exit "$__r""#,
        max_stdout = MAX_SUBPROCESS_STDOUT_BYTES,
    )
}

pub fn watchdog_script(timeout_secs: u64) -> String {
    watchdog_script_with_child(timeout_secs, r#"exec "$@""#)
}

/// Like `watchdog_script`, but first resolves the binary in `$1` to an absolute
/// path: CodexBar reads its version from the app bundle via `argv[0]`, so a bare
/// command name reports no version. The binary still rides in `$1` (never
/// interpolated), so this is injection-safe like `watchdog_script`.
pub fn version_probe_script(timeout_secs: u64) -> String {
    watchdog_script_with_child(
        timeout_secs,
        r#"case $1 in */*) b=$1;; *) b=$(command -v "$1" 2>/dev/null) || b=$1;; esac; exec "$b" --version"#,
    )
}

/// Wrap a command in the self-terminating watchdog. `$0` is only a label; the
/// real command rides in `"$@"`, so provider ids and the configured binary path
/// are never interpolated into the shell string (injection-safe).
pub fn watchdog_argv<'a>(script: &'a str, command: &[&'a str]) -> Vec<&'a str> {
    let mut argv = vec!["/bin/sh", "-c", script, "showy-quota-watchdog"];
    argv.extend_from_slice(command);
    argv
}

/// Build the argv for a version probe: the binary rides in `$1` (never
/// interpolated into the script); `$0` is only a label.
pub fn version_probe_argv<'a>(script: &'a str, bin: &'a str) -> Vec<&'a str> {
    vec!["/bin/sh", "-c", script, "showy-quota-version", bin]
}
