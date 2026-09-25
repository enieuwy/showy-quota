#!/usr/bin/env bash
# showy-quota — smoke tests for renderers.
#
# Each test runs a renderer against a JSON fixture, with a stub `codexbar`
# binary that just prints the fixture, and asserts the output meets a
# minimal shape. Failures print context and abort the suite.
#
# Usage: test/render_test.sh
# shellcheck disable=SC2030,SC2031


# Homebrew Bash 5.3 on macOS can hang while materializing here-doc/here-string
# payloads around 512 bytes. Bash 4.4 compatibility mode avoids that runtime
# regression while keeping the suite on a Bash 4-compatible feature set.
if [[ "${BASH_VERSION:-}" == 5.3.* && -z "${BASH_COMPAT:-}" ]]; then
    export BASH_COMPAT=4.4
fi
set -uo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
FIXTURE_DIR="${REPO_ROOT}/test/fixtures"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/showy-quota-test.XXXXXX")
trap 'rm -rf "${TMP}"' EXIT
export SHOWY_QUOTA_MANAGE_SERVE=0
export SHOWY_QUOTA_CODEXBAR_SERVE_URL=

# Colour must never be inherited from the developer's shell. The renderer
# disables colour when NO_COLOR is set OR when TERM is exactly "dumb"
# (showy-quota-render.rs:334-335), and SHOWY_QUOTA_FORCE_COLOR=1 overrides both
# in the other direction. An agent harness or editor terminal that sets any of
# them silently flips the ANSI that colour-sensitive assertions parse, so the
# suite passes locally and fails in CI, which sets none. Normalise to the CI
# state — colour enabled, nothing forced — and let each test state the colour
# it expects.
unset NO_COLOR CLICOLOR CLICOLOR_FORCE FORCE_COLOR SHOWY_QUOTA_FORCE_COLOR
export TERM=xterm-256color

# The shell reset-description parser delegates month and meridiem parsing to
# `date`, which is locale-sensitive: under fa_IR or fr_FR an English "12 PM"
# parses differently or not at all, while the Python expectation helper stays
# locale-independent. Pin the locale so the two sides cannot disagree because
# of the host, and pin the timezone so a countdown assertion cannot depend on
# where the machine is.
export LC_ALL=C
export TZ=UTC

# Config sourcing refuses a group/other-writable file, so the permissions of
# the fixtures this suite writes decide whether those tests pass. A shell with
# a permissive umask (0000 is common in agent and CI harnesses) produces 0666
# config files and fails them for a reason that has nothing to do with the
# code. Pin the umask for the same reason the colour and locale state above is
# pinned: the suite must not read the shell that launched it.
umask 022


# Build the native renderer only when the suite cannot use the repository
# fallback path. In CI this is normally already present via the preceding cargo
# phase or make target, so the guard stays a cheap file check.
RENDER_BIN="${REPO_ROOT}/target/release/showy-quota-render"
if [[ ! -x "${RENDER_BIN}" ]]; then
    if command -v cargo >/dev/null 2>&1; then
        if ! (cd "${REPO_ROOT}" && cargo build --release -p showy-quota-zellij-core); then
            printf 'failed to build showy-quota-render\n' >&2
            exit 1
        fi
    else
        printf 'showy-quota-render missing and cargo is unavailable; run make render-bin\n' >&2
        exit 1
    fi
fi
# ── stub codexbar that validates fetcher argv and prints the fixture ──

stub_dir="${TMP}/bin"
mkdir -p "${stub_dir}"
cat > "${stub_dir}/codexbar" <<'EOF'
#!/bin/sh
# Provider inventory request: derive enabled-true entries from the fixture.
if [ "${1:-}" = "config" ] && [ "${2:-}" = "providers" ]; then
    [ -n "${SHOWY_QUOTA_TEST_FIXTURE:-}" ] || exit 1
    shift 2
    saw_format=0
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --format) shift; [ "${1:-}" = "json" ] || exit 11; saw_format=1 ;;
            --json|--json-only) saw_format=1 ;;
            --pretty) ;;
            *) exit 12 ;;
        esac
        shift
    done
    [ "${saw_format}" = "1" ] || exit 13
    jq '
        if type == "array" then
            [ .[] | {provider: .provider, enabled: true} ]
        else
            error("fixture is not an array")
        end
    ' < "${SHOWY_QUOTA_TEST_FIXTURE}" || exit 14
    exit 0
fi

# Per-provider usage: filter the fixture to the requested provider id.
[ -n "${SHOWY_QUOTA_TEST_FIXTURE:-}" ] || exit 1
[ "${1:-}" = "usage" ] || exit 90
shift
saw_format=0
saw_pretty=0
saw_provider=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --format)
            shift
            [ "${1:-}" = "json" ] || exit 91
            saw_format=1
            ;;
        --provider)
            shift
            saw_provider="${1:-}"
            ;;
        --pretty)
            saw_pretty=1
            ;;
        --status)
            ;;
        *)
            exit 93
            ;;
    esac
    shift
done
[ "${saw_format}" = "1" ] && [ "${saw_pretty}" = "1" ] || exit 94
# The fetcher must always isolate provider calls; reject any aggregate call.
[ -n "${saw_provider}" ] || exit 95
jq --arg p "${saw_provider}" '
    if type == "array" then [ .[] | select(.provider == $p) ]
    else error("fixture is not an array") end
' < "${SHOWY_QUOTA_TEST_FIXTURE}"
EOF
chmod +x "${stub_dir}/codexbar"

cat > "${stub_dir}/curl" <<'EOF'
#!/bin/sh
[ -n "${SHOWY_QUOTA_TEST_SERVE_FIXTURE:-}" ] || exit 88
url=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --fail|--silent|--show-error)
            ;;
        --max-time)
            shift
            case "${1:-}" in
                ""|*[!0-9.]*)
                    exit 89
                    ;;
            esac
            if [ -n "${SHOWY_QUOTA_TEST_CURL_MAX_TIME_FILE:-}" ]; then
                printf '%s\n' "$1" > "${SHOWY_QUOTA_TEST_CURL_MAX_TIME_FILE}"
            fi
            ;;
        --max-filesize)
            shift
            case "${1:-}" in
                ""|*[!0-9]*)
                    exit 89
                    ;;
            esac
            ;;
        http://*)
            url="$1"
            ;;
        *)
            exit 90
            ;;
    esac
    shift
done
if [ "${url}" = "${SHOWY_QUOTA_TEST_SERVE_URL%/}/health" ]; then
    printf '{}'
    exit 0
fi
[ "${url}" = "${SHOWY_QUOTA_TEST_SERVE_URL%/}/usage" ] || exit 91
cat "${SHOWY_QUOTA_TEST_SERVE_FIXTURE}"
EOF
chmod +x "${stub_dir}/curl"

# Stub sketchybar with enough statefulness for plugin lifecycle tests. Each
# declared item is a file in the state dir; `.order` keeps SketchyBar's item
# list order so `--query bar` can answer the way the daemon does. A seeded
# `bar` file overrides that answer.
cat > "${stub_dir}/sketchybar" <<'EOF'
#!/bin/sh
log="${SHOWY_QUOTA_TEST_LOG:-/dev/null}"
state_dir="${SHOWY_QUOTA_TEST_STATE_DIR:-}"
[ -n "${state_dir}" ] && mkdir -p "${state_dir}"
echo "sketchybar $*" >> "${log}"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --query)
            # One invocation may batch several queries, as the notch planner
            # does; a missing item fails the whole call like the daemon.
            shift
            item="${1:-}"
            [ -n "${state_dir}" ] || exit 1
            if [ "${item}" = "bar" ]; then
                if [ -e "${state_dir}/bar" ]; then
                    cat "${state_dir}/bar"
                else
                    printf '{"items":['
                    [ -f "${state_dir}/.order" ] && sed 's/.*/"&"/' "${state_dir}/.order" | paste -sd, -
                    printf ']}\n'
                fi
            elif [ "${item}" = "displays" ]; then
                printf '[]\n'
            elif [ -e "${state_dir}/${item}" ]; then
                printf '{"name":"%s","geometry":{"position":"left","drawing":"on"}}\n' "${item}"
            else
                exit 1
            fi
            shift
            ;;
        --add)
            shift
            kind="${1:-}"
            shift
            name="${1:-}"
            if [ -n "${state_dir}" ] && [ -n "${name}" ]; then
                : > "${state_dir}/${name}"
                grep -qxF "${name}" "${state_dir}/.order" 2>/dev/null \
                    || printf '%s\n' "${name}" >> "${state_dir}/.order"
            fi
            shift
            if [ "${kind}" = "item" ] && [ "$#" -gt 0 ]; then
                shift
            elif [ "${kind}" = "bracket" ]; then
                while [ "$#" -gt 0 ] && [ "${1#--}" = "$1" ]; do
                    shift
                done
            fi
            ;;
        --remove)
            shift
            name="${1:-}"
            if [ -n "${state_dir}" ] && [ -n "${name}" ]; then
                rm -f "${state_dir}/${name}"
                if [ -f "${state_dir}/.order" ]; then
                    grep -vxF "${name}" "${state_dir}/.order" > "${state_dir}/.order.tmp"
                    mv "${state_dir}/.order.tmp" "${state_dir}/.order"
                fi
            fi
            shift
            ;;
        --set)
            shift
            [ "$#" -gt 0 ] && shift
            while [ "$#" -gt 0 ] && [ "${1#--}" = "$1" ]; do
                shift
            done
            ;;
        *)
            shift
            ;;
    esac
done
exit 0
EOF
chmod +x "${stub_dir}/sketchybar"

cat > "${stub_dir}/zellij" <<'EOF'
#!/bin/sh
log="${SHOWY_QUOTA_TEST_ZELLIJ_LOG:-/dev/null}"
printf '%s\n' "$*" >> "${log}"
exit 0
EOF
chmod +x "${stub_dir}/zellij"

PASSED=0
FAILED=0
FAILURES=()

ok()    { PASSED=$((PASSED + 1)); printf '  ✓ %s\n' "$1"; }
fail()  {
    FAILED=$((FAILED + 1))
    FAILURES+=("$1")
    printf '  ✗ %s\n' "$1" >&2
    [[ -n "${2:-}" ]] && printf '    %s\n' "$2" >&2
}

# Fresh cache dir per test so stale state never leaks.
mk_cache() { mktemp -d "${TMP}/cache.XXXXXX"; }

fixture_path() {
    local fixture="$1"
    if [[ "${fixture}" == /* ]]; then
        printf '%s' "${fixture}"
    else
        printf '%s' "${FIXTURE_DIR}/${fixture}"
    fi
}

seed_usage_cache() {
    local cache="$1" fixture="$2" source="${3:-serve}"
    local fixture_file
    fixture_file=$(fixture_path "${fixture}")
    jq -c --arg source "${source}" \
        '{schema: "showy-quota/cache@1", source: $source, providers: .}' \
        "${fixture_file}" > "${cache}/usage.json"
}

# Wrap an ALREADY-SEEDED "${cache}/usage.json" (bare array, e.g. from a plain
# `cp` fixture copy) into a cache envelope carrying the given source, in
# place. Mirrors what the real fetcher's single-rename publish produces.
seed_usage_source() {
    local cache="$1" source="$2" tmp
    tmp=$(mktemp "${cache}/usage-seed.XXXXXX")
    jq -c --arg source "${source}" \
        '{schema: "showy-quota/cache@1", source: $source, providers: .}' \
        "${cache}/usage.json" > "${tmp}"
    mv -- "${tmp}" "${cache}/usage.json"
}

# Read the `source` field back out of a cache dir's envelope the way tests
# assert on it: the raw value, not the serve/cli/unknown-normalized one
# `showy_quota_cache_source` returns. "missing" when there is no usable
# envelope, distinguishable from any real source value.
cache_source_value() {
    local cache="$1"
    [[ -r "${cache}/usage.json" ]] || { printf 'missing'; return; }
    jq -r 'if type == "object" and has("source") then .source else "missing" end' \
        "${cache}/usage.json" 2>/dev/null
}
pid_start_epoch() {
    python3 - "$1" <<'PY'
import datetime
import subprocess
import sys

pid = sys.argv[1]
out = subprocess.check_output(["ps", "-p", pid, "-o", "lstart="], text=True).strip()
for fmt in ("%a %b %d %H:%M:%S %Y", "%a %b %e %H:%M:%S %Y"):
    try:
        print(int(datetime.datetime.strptime(out, fmt).timestamp()))
        raise SystemExit(0)
    except ValueError:
        pass
raise SystemExit(1)
PY
}

unused_tcp_port() {
    python3 <<'PY'
import contextlib
import socket

with contextlib.closing(socket.socket(socket.AF_INET, socket.SOCK_STREAM)) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

run_renderer() {
    local renderer="$1" fixture="$2"
    shift 2
    local cache; cache=$(mk_cache)
    local out
    seed_usage_cache "${cache}" "${fixture}" serve
    out=$(
        env \
            PATH="${stub_dir}:${PATH}" \
            SHOWY_QUOTA_NO_CONFIG=1 \
            SHOWY_QUOTA_CACHE_DIR="${cache}" \
            SHOWY_QUOTA_TEST_FIXTURE="$(fixture_path "${fixture}")" \
            SHOWY_QUOTA_DEGRADED_CLI=0 \
            SHOWY_QUOTA_FORCE_COLOR=1 \
            "$@" \
            "${REPO_ROOT}/bin/${renderer}" 2>&1
    )
    printf '%s' "${out}"
}

run_renderer_json() {
    local renderer="$1" fixture="$2"
    shift 2
    local cache; cache=$(mk_cache)
    local fixture_file out
    fixture_file=$(fixture_path "${fixture}")
    out=$(
        env \
            PATH="${stub_dir}:${PATH}" \
            SHOWY_QUOTA_NO_CONFIG=1 \
            SHOWY_QUOTA_CACHE_DIR="${cache}" \
            SHOWY_QUOTA_DEGRADED_CLI=0 \
            SHOWY_QUOTA_FORCE_COLOR=1 \
            "$@" \
            "${REPO_ROOT}/bin/${renderer}" --json "${fixture_file}" 2>&1
    )
    printf '%s' "${out}"
}

run_state() {
    local fixture="$1"
    shift
    local cache; cache=$(mk_cache)
    local fixture_file
    fixture_file=$(fixture_path "${fixture}")
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_TEST_FIXTURE="${fixture_file}" \
        "$@" \
        "${REPO_ROOT}/bin/showy-quota-state"
}

run_state_with_usage_file() {
    local fixture="$1" usage_file="$2"
    shift 2
    run_state "${fixture}" SHOWY_QUOTA_USAGE_FILE="${usage_file}" "$@"
}

run_theme() {
    local xdg="$1"
    shift
    env \
        PATH="${stub_dir}:${PATH}" \
        XDG_CONFIG_HOME="${xdg}" \
        "${REPO_ROOT}/bin/showy-quota" "$@"
}


run_sketchybar_items() {
    local fixture="$1" cache="$2" log="$3"
    shift 3
    local fixture_file
    fixture_file=$(fixture_path "${fixture}")
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE="${cache}/sb" \
        SHOWY_QUOTA_TEST_FIXTURE="${fixture_file}" \
        SHOWY_QUOTA_TEST_LOG="${log}" \
        SHOWY_QUOTA_TEST_STATE_DIR="${cache}/sb-state" \
        SHOWY_QUOTA_DEGRADED_CLI=0 \
        "$@" \
        "${REPO_ROOT}/adapters/sketchybar/items/showy_quota.sh"
}

run_sketchybar_plugin() {
    local fixture="$1" cache="$2" log="$3"
    shift 3
    local fixture_file
    fixture_file=$(fixture_path "${fixture}")
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE="${cache}/sb" \
        SHOWY_QUOTA_TEST_FIXTURE="${fixture_file}" \
        SHOWY_QUOTA_TEST_LOG="${log}" \
        SHOWY_QUOTA_DEGRADED_CLI=0 \
        "$@" \
        SHOWY_QUOTA_TEST_STATE_DIR="${cache}/sb-state" \
        "${REPO_ROOT}/adapters/sketchybar/plugins/showy_quota.sh"
}

run_sketchybar_plugin_without_magick() {
    local fixture="$1" cache="$2" log="$3"
    shift 3
    local fixture_file no_magick_path tool tool_path
    fixture_file=$(fixture_path "${fixture}")
    no_magick_path="${TMP}/no-magick-bin"
    mkdir -p "${no_magick_path}"
    # grep and paste back the stateful sketchybar stub's item list.
    for tool in bash jq readlink dirname mkdir mktemp mv rm rmdir date stat sed tr cat python3 grep paste; do
        if [[ "${tool}" == "bash" && -x /opt/homebrew/bin/bash ]]; then
            tool_path=/opt/homebrew/bin/bash
        else
            tool_path=$(command -v "${tool}") || {
                fail "no-magick helper can find required tool ${tool}"
                return 1
            }
        fi
        ln -sf "${tool_path}" "${no_magick_path}/${tool}"
    done
    ln -sf "${stub_dir}/codexbar" "${no_magick_path}/codexbar"
    ln -sf "${stub_dir}/sketchybar" "${no_magick_path}/sketchybar"
    if PATH="${no_magick_path}" command -v magick >/dev/null 2>&1; then
        fail "no-magick helper excludes magick from PATH"
        return 1
    fi
    env \
        PATH="${no_magick_path}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE="${cache}/sb" \
        SHOWY_QUOTA_TEST_FIXTURE="${fixture_file}" \
        SHOWY_QUOTA_TEST_LOG="${log}" \
        SHOWY_QUOTA_DEGRADED_CLI=0 \
        "$@" \
        SHOWY_QUOTA_TEST_STATE_DIR="${cache}/sb-state" \
        "${REPO_ROOT}/adapters/sketchybar/plugins/showy_quota.sh"
}

seed_sketchybar_state() {
    local cache="$1"
    shift
    mkdir -p "${cache}/sb"
    : > "${cache}/sb/providers.txt"
    local pid
    for pid in "$@"; do
        printf '%s\n' "${pid}" >> "${cache}/sb/providers.txt"
    done
}
# Declare items the way the stub's `--add` does: a state file per item plus
# its line in `.order`, SketchyBar's item list.
seed_sketchybar_live_items() {
    local cache="$1"
    shift
    mkdir -p "${cache}/sb-state"
    local pid role name
    local -a names=()
    for pid in "$@"; do
        for role in icon primary secondary tertiary quaternary secondary_marker \
            tertiary_marker quaternary_marker primary_marker slot label; do
            names+=("showy_quota.${pid}.${role}")
        done
    done
    if (($# > 0)); then
        names+=(showy_quota.overflow showy_quota.stale showy_quota.degraded showy_quota_bracket)
    fi
    for name in "${names[@]}"; do
        : > "${cache}/sb-state/${name}"
        printf '%s\n' "${name}" >> "${cache}/sb-state/.order"
    done
}

process_state() {
    local pid="$1"
    local state=""
    if state=$(ps -o state= -p "${pid}" 2>/dev/null) \
        || state=$(ps -o stat= -p "${pid}" 2>/dev/null); then
        state="${state#"${state%%[![:space:]]*}"}"
        state="${state%%[[:space:]]*}"
        printf '%s\n' "${state}"
        return 0
    fi
    return 1
}

wait_for_state_prefix() {
    local pid="$1" prefix="$2"
    local state
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
        state=$(process_state "${pid}" || true)
        [[ "${state}" == "${prefix}"* ]] && return 0
        sleep 0.1
    done
    return 1
}

run_with_test_timeout() {
    local timeout_seconds="$1"
    shift
    python3 -c '
import os
import signal
import subprocess
import sys

timeout = float(sys.argv[1])
argv = sys.argv[2:]

proc = subprocess.Popen(
    argv,
    stdout=sys.stdout.buffer,
    stderr=sys.stderr.buffer,
    stdin=subprocess.DEVNULL,
    start_new_session=True,
)
try:
    sys.exit(proc.wait(timeout=timeout))
except subprocess.TimeoutExpired:
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        proc.wait(timeout=1)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait()
    sys.exit(124)
' "${timeout_seconds}" "$@"
}



assert_contains() {
    local label="$1" needle="$2" haystack="$3"
    if [[ "${haystack}" == *"${needle}"* ]]; then
        ok "${label}"
    else
        fail "${label}" "expected to contain: ${needle}"
        printf '    got: %s\n' "${haystack}" >&2
    fi
}

assert_not_contains() {
    local label="$1" needle="$2" haystack="$3"
    if [[ "${haystack}" == *"${needle}"* ]]; then
        fail "${label}" "expected NOT to contain: ${needle}"
    else
        ok "${label}"
    fi
}

assert_equals() {
    local label="$1" expected="$2" actual="$3"
    if [[ "${actual}" == "${expected}" ]]; then
        ok "${label}"
    else
        fail "${label}" "expected: ${expected}"
        printf '    got: %s\n' "${actual}" >&2
    fi
}

strip_tmux_markup() {
    printf '%s' "$1" | sed -E 's/#\[[^]]*\]//g'
}

run_common_eval() {
    local code="$1"
    shift
    # shellcheck disable=SC2016
    env \
        SHOWY_QUOTA_TEST_CODE="${code}" \
        SHOWY_QUOTA_TEST_REPO_ROOT="${REPO_ROOT}" \
        "$@" \
        bash -lc '
            set -euo pipefail
            . "${SHOWY_QUOTA_TEST_REPO_ROOT}/lib/common.sh"
            eval "${SHOWY_QUOTA_TEST_CODE}"
        '
}

run_strip_filter() {
    local payload="$1"
    # shellcheck disable=SC2016  # body runs in a sub-bash; $-vars expand there
    env \
        SHOWY_QUOTA_TEST_PAYLOAD="${payload}" \
        SHOWY_QUOTA_TEST_REPO_ROOT="${REPO_ROOT}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        bash -lc '
            set -euo pipefail
            . "${SHOWY_QUOTA_TEST_REPO_ROOT}/lib/common.sh"
            . "${SHOWY_QUOTA_TEST_REPO_ROOT}/lib/strip.sh"
            printf "%s" "${SHOWY_QUOTA_TEST_PAYLOAD}" | showy_quota_filter_renderable
        '
}

expected_reset_description_epoch() {
    local now="$1" desc="$2"
    python3 - "${now}" "${desc}" <<'PY'
import datetime
import sys

now = int(sys.argv[1])
desc = sys.argv[2]
today = datetime.datetime.fromtimestamp(now).date()
candidate = datetime.datetime.strptime(f"{today.isoformat()} {desc}", "%Y-%m-%d %I:%M %p")
epoch = int(candidate.timestamp())
if epoch < now:
    epoch += 86400
print(epoch)
PY
}



hex_to_rgb_csv() {
    local hex="$1"
    printf '%d,%d,%d' $((16#${hex:0:2})) $((16#${hex:2:2})) $((16#${hex:4:2}))
}

# ── palette helpers ───────────────────────────────────────────────────
printf 'palette helpers\n'

out=$(run_common_eval 'showy_quota_scale_hex 25be6a 0.55' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "scale helper matches legacy 0.55 green" "14683a" "${out}"

out=$(run_common_eval 'showy_quota_scale_hex "#25be6a" 0.55' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "scale helper accepts leading hash" "14683a" "${out}"

rc=0
out=$(run_common_eval 'showy_quota_scale_hex 123 0.55' SHOWY_QUOTA_NO_CONFIG=1 2>&1) || rc=$?
assert_equals "scale helper rejects malformed hex" "1" "${rc}"

rc=0
out=$(run_common_eval 'showy_quota_scale_hex 25be6a abc' SHOWY_QUOTA_NO_CONFIG=1 2>&1) || rc=$?
assert_equals "scale helper rejects non-numeric scale" "1" "${rc}"

out=$(run_common_eval 'showy_quota_primary_palette good' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_PALETTE_PRIMARY_GOOD="#25BE6A")
assert_equals "primary palette normalizes leading hash" "25be6a" "${out}"

out=$(run_common_eval 'showy_quota_primary_palette good' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "primary palette returns canonical primary color" "25be6a" "${out}"

out=$(run_common_eval 'showy_quota_dim_palette good' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "dim palette auto-derives from primary scale" "14683a" "${out}"

out=$(run_common_eval 'showy_quota_dim_palette good' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_PALETTE_DIM_GOOD=112233)
assert_equals "dim palette honors explicit override" "112233" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s|%s" "$(showy_quota_window_color 50 0)" "$(showy_quota_window_color 50 1)"' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "window color dims long-horizon windows only" "25be6a|14683a" "${out}"

out=$(run_common_eval 'showy_quota_dim_palette good' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_PALETTE_DIM_SCALE=0.75)
assert_equals "dim scale knob recomputes derived palette" "1b8e4f" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s|%s|%s|%s|%s" "$(showy_quota_is_long_window 300)" "$(showy_quota_is_long_window 1440)" "$(showy_quota_is_long_window 10080)" "$(showy_quota_is_long_window 43200)" "$(showy_quota_is_long_window "")"' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "is_long_window flags weekly and monthly horizons" "0|0|1|1|0" "${out}"

out=$(run_common_eval 'showy_quota_is_long_window 1440' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_DIM_WINDOW_MINUTES=1440)
assert_equals "dim window minutes threshold is configurable" "1" "${out}"

# ── numeric config validation ─────────────────────────────────────────
printf '\nnumeric config validation\n'

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s|%s|%s" "$(showy_quota_color_key 10)" "$(showy_quota_color_key 20)" "$(showy_quota_color_key 50)"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_GOOD_MIN_REMAINING='evil; rm -rf /' SHOWY_QUOTA_WARN_MIN_REMAINING=oops)
assert_equals "malformed coloring thresholds fall back to defaults" "bad|warn|good" "${out}"

out=$(run_common_eval 'showy_quota_color_key 50' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_GOOD_MIN_REMAINING=80)
assert_equals "valid coloring threshold is still honored" "warn" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s|%s|%s|%s" "$(showy_quota_uint 42 7)" "$(showy_quota_uint abc 7)" "$(showy_quota_uint 99999 7 100)" "$(showy_quota_uint 09 7)"' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "uint helper validates, clamps, and decimal-normalizes" "42|7|100|9" "${out}"

# SHOWY_QUOTA_NOW_EPOCH pins the clock for every freshness and stale decision,
# so a malformed override must fall back to the real clock rather than being
# honoured as-is. A silently accepted junk value would poison cache-age and
# stale rendering everywhere.
# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "$(showy_quota_now_epoch)"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_NOW_EPOCH=4070908800)
assert_equals "now_epoch honors a valid pinned override" "4070908800" "${out}"
# Bracket the call with the real clock instead of a floor: a parser that
# returned one fixed epoch above 1600000000 for every junk value would satisfy
# a threshold while still poisoning every freshness decision. The window is
# generous enough for a slow subshell and still excludes a constant.
for bad_now in abc -1 ' 123 ' '' 1e9 '12.5' '0x10'; do
    now_before=$(date +%s)
    # shellcheck disable=SC2016
    out=$(run_common_eval 'printf "%s" "$(showy_quota_now_epoch)"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_NOW_EPOCH="${bad_now}")
    now_after=$(date +%s)
    if [[ "${out}" =~ ^[0-9]+$ ]] && (( out >= now_before && out <= now_after )); then
        ok "now_epoch falls back to the real clock for malformed '${bad_now}'"
    else
        fail "now_epoch falls back to the real clock for malformed '${bad_now}'" \
            "got '${out}', expected the clock between ${now_before} and ${now_after}"
    fi
done

out=$(run_renderer showy-quota-zellij-bar codexbar-mixed.json SHOWY_QUOTA_PNG_BAR_W=09 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "leading-zero numeric config renders without octal abort" "CL" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_LOCK_WAIT_TENTHS}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_LOCK_WAIT_TENTHS=99999999)
assert_equals "lock wait clamps to ceiling" "36000" "${out}"

out=$(run_common_eval 'showy_quota_primary_palette good' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_THEME=catppuccin-mocha-blue)
assert_equals "built-in Catppuccin Mocha Blue theme overrides the primary palette" "89b4fa" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s|%s|%s|%s|%s|%s|%s" "$(showy_quota_primary_palette good)" "$(showy_quota_primary_palette warn)" "$(showy_quota_palette bg)" "$(showy_quota_palette icon_text)" "$(showy_quota_palette countdown)" "$(showy_quota_palette countdown_warn)" "$(showy_quota_palette elapsed)"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_THEME=default)
assert_equals "built-in default theme exposes original palette" "25be6a|f0af00|161616|f2f4f8|7b8496|ee5396|be95ff" "${out}"

theme_xdg="${TMP}/xdg-theme"
mkdir -p "${theme_xdg}/showy-quota"
printf '%s\n' \
    'SHOWY_QUOTA_THEME=catppuccin-mocha-blue' \
    'SHOWY_QUOTA_PALETTE_PRIMARY_GOOD=010203' \
    'SHOWY_QUOTA_PALETTE_ICON_TEXT=020304' \
    'SHOWY_QUOTA_PALETTE_COUNTDOWN=030405' \
    'SHOWY_QUOTA_PALETTE_COUNTDOWN_WARN=040506' \
    > "${theme_xdg}/showy-quota/config.env"
out=$(run_common_eval 'showy_quota_primary_palette good' SHOWY_QUOTA_NO_CONFIG= XDG_CONFIG_HOME="${theme_xdg}")
assert_equals "config env overrides themed primary palette" "010203" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s|%s|%s" "$(showy_quota_palette icon_text)" "$(showy_quota_palette countdown)" "$(showy_quota_palette countdown_warn)"' SHOWY_QUOTA_NO_CONFIG= XDG_CONFIG_HOME="${theme_xdg}")
assert_equals "config env overrides themed text role palettes" "020304|030405|040506" "${out}"

# ── regular-file source guard ─────────────────────────────────────────
printf '\nregular-file source guard\n'

fifo_config_xdg="${TMP}/xdg-fifo-config"
mkdir -p "${fifo_config_xdg}/showy-quota"
mkfifo "${fifo_config_xdg}/showy-quota/config.env"
rc=0
out=$(
    run_with_test_timeout 2 \
        env \
        PATH="${stub_dir}:${PATH}" \
        XDG_CONFIG_HOME="${fifo_config_xdg}" \
        SHOWY_QUOTA_CACHE_DIR="$(mk_cache)" \
        SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
        SHOWY_QUOTA_DEGRADED_CLI=0 \
        SHOWY_QUOTA_FORCE_COLOR=0 \
        NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar" 2>&1
) || rc=$?
assert_equals "config FIFO source guard completes" "0" "${rc}"
assert_contains "config FIFO source guard still renders" "CL" "${out}"

fifo_theme_xdg="${TMP}/xdg-fifo-theme"
mkdir -p "${fifo_theme_xdg}/showy-quota/themes"
printf '%s\n' 'SHOWY_QUOTA_THEME=default' > "${fifo_theme_xdg}/showy-quota/config.env"
mkfifo "${fifo_theme_xdg}/showy-quota/themes/default.env"
rc=0
out=$(
    run_with_test_timeout 2 \
        env \
        PATH="${stub_dir}:${PATH}" \
        XDG_CONFIG_HOME="${fifo_theme_xdg}" \
        SHOWY_QUOTA_CACHE_DIR="$(mk_cache)" \
        SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
        SHOWY_QUOTA_DEGRADED_CLI=0 \
        SHOWY_QUOTA_FORCE_COLOR=0 \
        NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar" 2>&1
) || rc=$?
assert_equals "theme FIFO source guard completes" "0" "${rc}"
assert_contains "theme FIFO source guard still renders" "CL" "${out}"

# ── executable & glyph config validation ──────────────────────────────
printf '\nexecutable & glyph config validation\n'

# Injection-shaped *_BIN values (whitespace / shell metacharacters) are rejected
# back to their defaults so a poisoned env/config.env entry cannot be exec'd.
# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_CODEXBAR_BIN}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CODEXBAR_BIN='/bin/sh -c evil')
assert_equals "injection-shaped codexbar bin degrades to default" "codexbar" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_ZELLIJ_BIN}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_ZELLIJ_BIN='zellij; rm -rf ~')
assert_equals "injection-shaped zellij bin degrades to default" "zellij" "${out}"

# A clean but currently-missing path is preserved: exec just fails into the
# renderer's normal fallback (existence is not required).
# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_CODEXBAR_BIN}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CODEXBAR_BIN=/tmp/showy-quota/no-such-codexbar)
assert_equals "clean missing codexbar path is preserved" "/tmp/showy-quota/no-such-codexbar" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'showy_quota_valid_bin "/usr/local/bin/codexbar"; printf "|"; showy_quota_valid_bin "a b" || printf "rej"' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "valid_bin accepts path and rejects whitespace" "/usr/local/bin/codexbar|rej" "${out}"

# Status glyphs carrying control characters fall back to defaults; clean ones stay.
# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_STALE_GLYPH}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_STALE_GLYPH=$'\x1bboom')
assert_equals "control-char stale glyph degrades to default" "⚠" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_DEGRADED_CLI_GLYPH}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_DEGRADED_CLI_GLYPH='!!')
assert_equals "clean custom degraded glyph preserved" "!!" "${out}"

# A theme name that escapes the themes dir is ignored (no arbitrary .env source);
# defaults are kept instead of aborting the renderer.
# shellcheck disable=SC2016
out=$(run_common_eval 'showy_quota_primary_palette good' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_THEME='../catppuccin-mocha-blue')
assert_equals "traversal theme name is ignored (defaults kept)" "25be6a" "${out}"

# CodexBar date fields are length-capped before reaching date/gdate -d.
# shellcheck disable=SC2016
out=$(run_common_eval 'r=$(showy_quota_reset_epoch 2099-01-15T00:00:00Z); [[ "$r" =~ ^[0-9]+$ ]] && printf digits || printf no' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "reset_epoch accepts a short ISO timestamp" "digits" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'showy_quota_reset_epoch "$(printf "9%.0s" {1..100})" >/dev/null && printf accept || printf reject' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "reset_epoch rejects an overlong date string" "reject" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'showy_quota_reset_description_epoch "Resets $(printf "x%.0s" {1..100})" >/dev/null && printf accept || printf reject' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "reset_description_epoch rejects an overlong fragment" "reject" "${out}"

# ── pinned time regressions ───────────────────────────────────────────
printf '\npinned time regressions\n'

reset_now_a=4070908800
reset_now_b=$((reset_now_a + 86400))
expected_reset_a=$(expected_reset_description_epoch "${reset_now_a}" "11:59 PM")
expected_reset_b=$(expected_reset_description_epoch "${reset_now_b}" "11:59 PM")
actual_reset_a=$(run_common_eval 'showy_quota_reset_description_epoch "Resets 11:59 PM"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_NOW_EPOCH="${reset_now_a}")
actual_reset_b=$(run_common_eval 'showy_quota_reset_description_epoch "Resets 11:59 PM"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_NOW_EPOCH="${reset_now_b}")
if (( actual_reset_a / 60 == expected_reset_a / 60 && actual_reset_b / 60 == expected_reset_b / 60 && actual_reset_b - actual_reset_a == 86400 )); then
    ok "reset_description_epoch honors pinned now"
else
    fail "reset_description_epoch honors pinned now" "expected minutes ${expected_reset_a}|${expected_reset_b}; got ${actual_reset_a}|${actual_reset_b}"
fi

# The 12 AM / 12 PM arms of the meridiem conversion. The shell delegates them
# to `date`'s %I:%M %p, the Rust core hand-rolls them in parse_time_12h, and the
# two must agree — an off-by-12h here silently shifts a provider's countdown by
# half a day. Pinned to noon so 12 AM is behind now (rolls to the next day) and
# 12 PM is ahead of now (same day), which makes the two values impossible to
# confuse.
meridiem_now=$((4070908800 + 43200))
expected_midnight=$(expected_reset_description_epoch "${meridiem_now}" "12:00 AM")
expected_noon=$(expected_reset_description_epoch "${meridiem_now}" "12:00 PM")
actual_midnight=$(run_common_eval 'showy_quota_reset_description_epoch "Resets 12:00 AM"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_NOW_EPOCH="${meridiem_now}")
actual_noon=$(run_common_eval 'showy_quota_reset_description_epoch "Resets 12:00 PM"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_NOW_EPOCH="${meridiem_now}")
if (( actual_midnight / 60 == expected_midnight / 60 )); then
    ok "reset_description_epoch reads 12 AM as midnight"
else
    fail "reset_description_epoch reads 12 AM as midnight" "expected ${expected_midnight}; got ${actual_midnight}"
fi
if (( actual_noon / 60 == expected_noon / 60 )); then
    ok "reset_description_epoch reads 12 PM as noon"
else
    fail "reset_description_epoch reads 12 PM as noon" "expected ${expected_noon}; got ${actual_noon}"
fi
# A collapsed meridiem arm would map both spellings onto the same hour.
if (( actual_noon != actual_midnight )); then
    ok "reset_description_epoch keeps 12 AM and 12 PM distinct"
else
    fail "reset_description_epoch keeps 12 AM and 12 PM distinct" "both resolved to ${actual_noon}"
fi

marker_fixture="${TMP}/codexbar-marker-pinned.json"
printf '%s\n' \
    '[{"provider":"codex","usage":{"primary":{"usedPercent":50,"windowMinutes":60,"resetsAt":"2099-01-01T01:00:00Z"},"secondary":null,"tertiary":null}}]' \
    > "${marker_fixture}"
marker_reset_epoch=4070912400
marker_now_a=4070910600
marker_now_b=4070911500
marker_x_a=$(( (marker_reset_epoch - marker_now_a) * 80 / (60 * 60) ))
marker_x_b=$(( (marker_reset_epoch - marker_now_b) * 80 / (60 * 60) ))
expected_marker_pct_a=$(( (marker_x_a * 100 + (80 - 1) / 2) / (80 - 1) ))
expected_marker_pct_b=$(( (marker_x_b * 100 + (80 - 1) / 2) / (80 - 1) ))
cache=$(mk_cache)
log="${TMP}/sb-marker-a.log"
run_sketchybar_plugin_without_magick "${marker_fixture}" "${cache}" "${log}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font SHOWY_QUOTA_NOW_EPOCH="${marker_now_a}" SHOWY_QUOTA_REFRESH_SECONDS=9999999999
marker_log_a="$(< "${log}")"
cache=$(mk_cache)
log="${TMP}/sb-marker-b.log"
run_sketchybar_plugin_without_magick "${marker_fixture}" "${cache}" "${log}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font SHOWY_QUOTA_NOW_EPOCH="${marker_now_b}" SHOWY_QUOTA_REFRESH_SECONDS=9999999999
marker_log_b="$(< "${log}")"
if [[ "${marker_log_a}" == *"showy_quota.codex.primary_marker drawing=on slider.percentage=${expected_marker_pct_a}"* \
      && "${marker_log_b}" == *"showy_quota.codex.primary_marker drawing=on slider.percentage=${expected_marker_pct_b}"* \
      && "${expected_marker_pct_a}" != "${expected_marker_pct_b}" ]]; then
    ok "sketchybar elapsed marker honors pinned now"
else
    fail "sketchybar elapsed marker honors pinned now" "expected ${expected_marker_pct_a}|${expected_marker_pct_b}"
fi

# SketchyBar string knobs are clamped to a known-good shape.
# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR='0xff112233 drawing=on')
assert_equals "malformed pill color falls back to default" "0xcc24273a" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR=0xAA00FF80)
assert_equals "valid pill color is honored" "0xAA00FF80" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT=$'evil\nlabel=x')
assert_equals "control-char provider icon font falls back to default" "sketchybar-app-font:Regular:14.0" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT='SF Pro:Bold:13.0')
assert_equals "valid provider icon font with spaces is honored" "SF Pro:Bold:13.0" "${out}"

# Zellij pipe identifiers are restricted to safe tokens.
# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_ZELLIJ_WIDGET}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_ZELLIJ_WIDGET='foo::bar')
assert_equals "widget with :: falls back to default" "pipe_showy_quota" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_ZELLIJ_WIDGET}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_ZELLIJ_WIDGET='my.widget-1')
assert_equals "valid widget name is honored" "my.widget-1" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_ZELLIJ_PIPE_NAME}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_ZELLIJ_PIPE_NAME='bad name')
assert_equals "pipe name with space falls back to default" "showy-quota" "${out}"

long_zellij_identifier="$(printf 'x%.0s' {1..129})"
# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_ZELLIJ_WIDGET}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_ZELLIJ_WIDGET="${long_zellij_identifier}")
assert_equals "overlong widget falls back to default" "pipe_showy_quota" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s" "${SHOWY_QUOTA_ZELLIJ_PIPE_NAME}"' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_ZELLIJ_PIPE_NAME="${long_zellij_identifier}")
assert_equals "overlong pipe name falls back to default" "showy-quota" "${out}"

# ── countdown formatting ──────────────────────────────────────────────
printf '\ncountdown formatting\n'

out=$(run_common_eval 'showy_quota_format_countdown ""' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "countdown empty is unknown" "?" "${out}"

out=$(run_common_eval 'showy_quota_format_countdown 0' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "countdown zero is now" "now" "${out}"

out=$(run_common_eval 'showy_quota_format_countdown 12' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "countdown under one hour keeps minutes" "12m" "${out}"

out=$(run_common_eval 'showy_quota_format_countdown 180' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "countdown whole hours stays compact" "3h" "${out}"

out=$(run_common_eval 'showy_quota_format_countdown 225' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "countdown mixed hours uses clock form" "3:45" "${out}"

out=$(run_common_eval 'showy_quota_format_countdown 725' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "countdown clock form pads minutes" "12:05" "${out}"

out=$(run_common_eval 'showy_quota_format_countdown 2880' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "countdown days unchanged" "2d" "${out}"

out=$(run_common_eval 'showy_quota_primary_label 12 88 "2099-01-01T00:12:00Z"' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "primary label keeps live countdown behavior" "12m" "${out}"

out=$(run_common_eval 'showy_quota_primary_label "" 100 ""' SHOWY_QUOTA_NO_CONFIG=1)
assert_equals "primary label keeps live idle behavior" "idle" "${out}"


# ── theme CLI ─────────────────────────────────────────────────────────
printf '\nshowy-quota cli\n'

theme_cli_xdg=$(mktemp -d "${TMP}/xdg-theme-list.XXXXXX")
mkdir -p "${theme_cli_xdg}/showy-quota/themes"
printf '%s\n' ": \"\${SHOWY_QUOTA_PALETTE_PRIMARY_GOOD:=010203}\"" > "${theme_cli_xdg}/showy-quota/themes/catppuccin-mocha-blue.env"
printf '%s\n' ": \"\${SHOWY_QUOTA_PALETTE_PRIMARY_GOOD:=040506}\"" > "${theme_cli_xdg}/showy-quota/themes/foo.env"
out=$(run_theme "${theme_cli_xdg}" --list)
assert_equals "theme list merges sorted unique names" $'carbonfox\ncatppuccin-frappe\ncatppuccin-latte\ncatppuccin-macchiato\ncatppuccin-mocha\ncatppuccin-mocha-blue\ndefault\ndracula\nfoo\ngruvbox-dark\nmonochrome\nnord\ntokyonight' "${out}"

theme_current_xdg=$(mktemp -d "${TMP}/xdg-theme-current.XXXXXX")
out=$(run_theme "${theme_current_xdg}" --current)
assert_equals "theme current is none without config" "(none)" "${out}"

theme_export_xdg=$(mktemp -d "${TMP}/xdg-theme-export.XXXXXX")
mkdir -p "${theme_export_xdg}/showy-quota"
printf '%s\n' 'export SHOWY_QUOTA_THEME="default"' > "${theme_export_xdg}/showy-quota/config.env"
out=$(run_theme "${theme_export_xdg}" --current)
assert_equals "theme current reads exported assignment" "default" "${out}"

theme_set_xdg=$(mktemp -d "${TMP}/xdg-theme-set.XXXXXX")
theme_config="${theme_set_xdg}/showy-quota/config.env"
run_theme "${theme_set_xdg}" --set default
assert_equals "theme set creates config line" "SHOWY_QUOTA_THEME=default" "$(< "${theme_config}")"

theme_replace_xdg=$(mktemp -d "${TMP}/xdg-theme-replace.XXXXXX")
mkdir -p "${theme_replace_xdg}/showy-quota"
theme_config="${theme_replace_xdg}/showy-quota/config.env"
printf '%s\n' \
    'FOO=1' \
    '# SHOWY_QUOTA_THEME=old-comment' \
    'export SHOWY_QUOTA_THEME=old-active' \
    '    SHOWY_QUOTA_THEME=catppuccin-latte' \
    'BAR=2' \
    > "${theme_config}"
run_theme "${theme_replace_xdg}" --set default
assert_equals "theme set preserves config and coalesces active lines" $'FOO=1\n# SHOWY_QUOTA_THEME=old-comment\nexport SHOWY_QUOTA_THEME=default\nBAR=2' "$(< "${theme_config}")"

theme_orphan_lock_xdg=$(mktemp -d "${TMP}/xdg-theme-orphan-lock.XXXXXX")
mkdir -p "${theme_orphan_lock_xdg}/showy-quota"
theme_config="${theme_orphan_lock_xdg}/showy-quota/config.env"
mkdir -p "${theme_config}.lock"
touch -t 202001010000 "${theme_config}.lock"
run_theme "${theme_orphan_lock_xdg}" --set default
assert_equals "theme set reclaims stale ownerless write lock" "SHOWY_QUOTA_THEME=default" "$(< "${theme_config}")"

theme_fresh_lock_xdg=$(mktemp -d "${TMP}/xdg-theme-fresh-lock.XXXXXX")
mkdir -p "${theme_fresh_lock_xdg}/showy-quota"
theme_config="${theme_fresh_lock_xdg}/showy-quota/config.env"
mkdir -p "${theme_config}.lock"
printf '%s\n' "$$" > "${theme_config}.lock/owner.$$"
theme_fresh_lock_rc=0
run_theme "${theme_fresh_lock_xdg}" --set default 2>/dev/null || theme_fresh_lock_rc=$?
assert_equals "theme set times out on live-owner write lock" "1" "${theme_fresh_lock_rc}"

theme_unset_xdg=$(mktemp -d "${TMP}/xdg-theme-unset.XXXXXX")
mkdir -p "${theme_unset_xdg}/showy-quota"
theme_config="${theme_unset_xdg}/showy-quota/config.env"
printf '%s\n' \
    'FOO=1' \
    '# SHOWY_QUOTA_THEME=old-comment' \
    'export SHOWY_QUOTA_THEME=catppuccin-mocha' \
    'SHOWY_QUOTA_THEME=default' \
    'BAR=2' \
    > "${theme_config}"
run_theme "${theme_unset_xdg}" --unset
assert_equals "theme unset removes every active line" $'FOO=1\n# SHOWY_QUOTA_THEME=old-comment\nBAR=2' "$(< "${theme_config}")"

theme_bogus_xdg=$(mktemp -d "${TMP}/xdg-theme-bogus.XXXXXX")
mkdir -p "${theme_bogus_xdg}/showy-quota"
theme_config="${theme_bogus_xdg}/showy-quota/config.env"
printf '%s\n' 'FOO=1' > "${theme_config}"
theme_before="$(< "${theme_config}")"
rc=0
out=$(run_theme "${theme_bogus_xdg}" --set bogus 2>&1) || rc=$?
theme_after="$(< "${theme_config}")"
if (( rc != 0 )) && [[ "${theme_after}" == "${theme_before}" ]]; then
    ok "bogus theme fails without modifying config"
else
    fail "bogus theme fails without modifying config" "rc=${rc}; before=${theme_before}; after=${theme_after}; out=${out}"
fi

theme_preview_xdg=$(mktemp -d "${TMP}/xdg-theme-preview.XXXXXX")
out=$(run_theme "${theme_preview_xdg}" --preview catppuccin-mocha-blue)
assert_contains "theme preview uses Catppuccin Mocha Blue primary good RGB" "137;180;250m" "${out}"
assert_contains "theme preview uses derived secondary RGB" "75;99;137m" "${out}"
assert_contains "theme preview uses fixed clock countdown" "3:29" "${out}"
assert_contains "theme preview uses urgent minute countdown" "23m" "${out}"
assert_contains "theme preview uses Catppuccin Mocha countdown RGB" "166;173;200m" "${out}"
assert_not_contains "theme preview avoids stale day label" "7d" "${out}"
assert_contains "theme preview reuses zellij strip shape" "CL" "${out}"
assert_contains "theme preview includes zellij powerline cap" "" "${out}"
assert_contains "theme preview includes zellij half-block stack" "▀" "${out}"
assert_not_contains "theme preview omits provider sigil padding" " CL " "${out}"
assert_not_contains "theme preview omits countdown padding" " 3:29 " "${out}"
assert_not_contains "theme preview omits old weekly hint glyph" " w " "${out}"

out=$(run_theme "${theme_preview_xdg}" --preview dracula)
assert_contains "theme preview uses Dracula primary purple RGB" "189;147;249m" "${out}"

out=$(run_theme "${theme_preview_xdg}" --preview nord)
assert_contains "theme preview uses Nord frost primary RGB" "136;192;208m" "${out}"

out=$(run_theme "${theme_preview_xdg}" --preview default)
assert_contains "theme preview default uses original good RGB" "37;190;106m" "${out}"
assert_contains "theme preview default uses ai-quota countdown RGB" "123;132;150m" "${out}"

theme_fallback_xdg=$(mktemp -d "${TMP}/xdg-theme-fallback.XXXXXX")
out=$(
    env \
        PATH="${stub_dir}:${PATH}" \
        XDG_CONFIG_HOME="${theme_fallback_xdg}" \
        SHOWY_QUOTA_TEST_NO_FZF=1 \
        "${REPO_ROOT}/bin/showy-quota"
)
assert_contains "theme fallback prints current state" "Current theme: (none)" "${out}"
assert_contains "theme fallback prints available theme" "catppuccin-mocha-blue" "${out}"
assert_contains "theme fallback prints built-in default theme" "default" "${out}"
assert_contains "theme fallback prints set hint" "showy-quota --set <name>" "${out}"
if [[ ! -e "${theme_fallback_xdg}/showy-quota/config.env" ]]; then
    ok "theme fallback writes nothing"
else
    fail "theme fallback writes nothing"
fi

diag_xdg=$(mktemp -d "${TMP}/xdg-diagnose.XXXXXX")
diag_cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${diag_cache}/usage.json"
seed_usage_source "${diag_cache}" "cli"
out=$(
    env \
        PATH="${stub_dir}:${PATH}" \
        XDG_CONFIG_HOME="${diag_xdg}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${diag_cache}" \
        SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
        SHOWY_QUOTA_THEME=default \
        "${REPO_ROOT}/bin/showy-quota" --diagnose --json
)
if printf '%s' "${out}" | jq -e '.paths.showyQuota and .tools.required.jq.available and .cache.path and .state.available == true and .env.SHOWY_QUOTA_THEME == "default" and .codexbarProbe.cachedProviderCount == 4' >/dev/null; then
    ok "diagnose json exposes stable machine-readable state"
else
    fail "diagnose json exposes stable machine-readable state" "${out}"
fi

# --diagnose output is written to be pasted into bug reports, so --redact must
# strip what only describes this machine (absolute paths, URL hosts) while
# keeping what a maintainer needs (basenames, ports, versions, counts).
redact_out=$(
    env \
        PATH="${stub_dir}:${PATH}" \
        XDG_CONFIG_HOME="${diag_xdg}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${diag_cache}" \
        SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='http://127.0.0.1:9999' \
        SHOWY_QUOTA_THEME=default \
        "${REPO_ROOT}/bin/showy-quota" --diagnose --json --redact
)
if printf '%s' "${redact_out}" | jq -e '
        .redacted == true
        and (.paths.repoRoot | startswith("<path>/"))
        and (.paths.cache | startswith("<path>/"))
        and .env.SHOWY_QUOTA_CODEXBAR_SERVE_URL == "http://<host>:9999"
        and .env.SHOWY_QUOTA_THEME == "default"
        and .codexbarProbe.cachedProviderCount == 4
    ' >/dev/null; then
    ok "diagnose --redact hides local paths and URL hosts but keeps port, theme and counts"
else
    fail "diagnose --redact hides local paths and URL hosts but keeps port, theme and counts" "${redact_out}"
fi
# No absolute path may survive anywhere in a redacted paste.
if printf '%s' "${redact_out}" | jq -r '[paths(type == "string") as $p | getpath($p)] | map(select(startswith("/"))) | length' | grep -qx 0; then
    ok "diagnose --redact leaves no absolute path in the payload"
else
    fail "diagnose --redact leaves no absolute path in the payload" \
        "$(printf '%s' "${redact_out}" | jq -c '[paths(type == "string") as $p | getpath($p)] | map(select(startswith("/")))')"
fi
# The default stays unredacted: bug reports need the real layout by default.
if printf '%s' "${out}" | jq -e '.redacted == false and (.paths.repoRoot | startswith("/"))' >/dev/null; then
    ok "diagnose json is unredacted unless --redact is passed"
else
    fail "diagnose json is unredacted unless --redact is passed" "${out}"
fi

# Every value that reaches a paste must be masked, not just the ones the
# redactor was first written for. Five distinct code paths leaked a real path
# or host while the payload still reported `"redacted": true`, which tells the
# user the paste is safe: the JSON codexbar command, the text cache-probe line,
# the state payload (provider error messages arrive from CodexBar and quote the
# URL or path they failed against), the theme value, and `manage serve`. Each
# form below is a shape that escaped an earlier pass, so the table doubles as
# the list of regressions to defend: absolute, `~user`, bare `~user`,
# relative, IPv6-bracketed, and userinfo-bearing.
redact_probe_cache=$(mk_cache)
for redact_case in \
    '/Users/alice/private/token.json|alice' \
    '/Users/alice x/private/token.json|alice' \
    '/Users/alice%corp/private/token.json|alice' \
    '/Users/alice:corp/private/token.json|alice' \
    '/home/alice/private/token.json|alice' \
    '~alice/private/token.json|alice' \
    '~alice|alice' \
    '../alice/private/token.json|alice' \
    'https://secret.example.test:8443/private|secret.example' \
    'https://[2001:db8::1]:8443/private|db8' \
    'http://user:pw@secret.example.test/x|secret.example'
do
    redact_value="${redact_case%%|*}"
    redact_needle="${redact_case##*|}"
    printf '[{"provider":"cursor","error":{"code":1,"kind":"provider","message":"failed at %s"}}]\n' \
        "${redact_value}" > "${redact_probe_cache}/usage.json"
    seed_usage_source "${redact_probe_cache}" "cli"
    for redact_mode in "--diagnose --redact" "--diagnose --json --redact"; do
        # shellcheck disable=SC2086
        redact_seen=$(
            env \
                PATH="${stub_dir}:${PATH}" \
                XDG_CONFIG_HOME="${diag_xdg}" \
                SHOWY_QUOTA_NO_CONFIG=1 \
                SHOWY_QUOTA_CACHE_DIR="${redact_probe_cache}" \
                SHOWY_QUOTA_MANAGE_SERVE=0 \
                SHOWY_QUOTA_THEME="${redact_value}" \
                "${REPO_ROOT}/bin/showy-quota" ${redact_mode} 2>&1 | grep -c "${redact_needle}" || true
        )
        assert_equals "diagnose ${redact_mode} masks ${redact_value}" "0" "${redact_seen}"
    done
done

# Masking must not cost the value a maintainer needs: the port survives even
# when the authority ends at a query rather than a path.
assert_equals "diagnose --redact keeps the port when a query follows the host" \
    "http://<host>:8080" \
    "$(env PATH="${stub_dir}:${PATH}" XDG_CONFIG_HOME="${diag_xdg}" SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${redact_probe_cache}" SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='http://secret.example:8080?token=1' \
        "${REPO_ROOT}/bin/showy-quota" --diagnose --redact 2>&1 \
        | sed -n 's/.*SHOWY_QUOTA_CODEXBAR_SERVE_URL=\(.*\)/\1/p' | head -1)"

# A bare command name is not a path and stays readable, or the flag would
# destroy the field it exists to report.
assert_equals "diagnose --redact keeps a bare command name" "codexbar" \
    "$(env PATH="${stub_dir}:${PATH}" XDG_CONFIG_HOME="${diag_xdg}" SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${redact_probe_cache}" SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CODEXBAR_BIN=codexbar \
        "${REPO_ROOT}/bin/showy-quota" --diagnose --json --redact 2>/dev/null \
        | jq -r '.codexbarProbe.command')"

# ── grant zellij permissions ──────────────────────────────────────────
printf '\ngrant zellij permissions\n'

grant_home=$(mktemp -d "${TMP}/grant-home.XXXXXX")
grant_perms="${grant_home}/permissions.kdl"
grant_plugin="${grant_home}/.config/zellij/plugins/showy-quota-zellij-chezmoi.wasm"
mkdir -p "$(dirname "${grant_plugin}")"
: > "${grant_plugin}"

# Pre-seed an unrelated plugin block to prove merge-safety.
printf '%s\n' \
    '"/other/plugin.wasm" {' \
    '    ReadApplicationState' \
    '}' > "${grant_perms}"

run_grant() {
    env \
        PATH="${stub_dir}:${PATH}" \
        HOME="${grant_home}" \
        SHOWY_QUOTA_ZELLIJ_PERMISSIONS_FILE="${grant_override_perms:-${grant_perms}}" \
        "${REPO_ROOT}/bin/showy-quota" --grant-zellij "$@"
}

# Read-only modes must report every key without creating the cache directory.
grant_absent_file="${grant_home}/absent-cache/permissions.kdl"
grant_absent_dir="${grant_home}/absent-cache"
grant_check_rc=0
grant_check_output=$(grant_override_perms="${grant_absent_file}" run_grant --check "${grant_plugin}") || grant_check_rc=$?
assert_equals "grant --check fails when permissions file is absent" "1" "${grant_check_rc}"
assert_contains "grant --check reports missing bare key" "${grant_plugin}: missing" "${grant_check_output}"
assert_contains "grant --check reports missing file: URL key" "file:${grant_plugin}: missing" "${grant_check_output}"
assert_contains "grant --check reports missing file:~ key" "file:~/.config/zellij/plugins/showy-quota-zellij-chezmoi.wasm: missing" "${grant_check_output}"
grant_dry_rc=0
grant_dry_output=$(grant_override_perms="${grant_absent_file}" run_grant --dry-run "${grant_plugin}") || grant_dry_rc=$?
assert_equals "grant --dry-run succeeds with absent permissions file" "0" "${grant_dry_rc}"
assert_contains "grant --dry-run names the permissions file" "${grant_absent_file}" "${grant_dry_output}"
assert_contains "grant --dry-run names the plugin" "${grant_plugin}" "${grant_dry_output}"
assert_contains "grant --dry-run shows expected permissions" "WebAccess" "${grant_dry_output}"
assert_contains "grant --dry-run reports missing keys" "${grant_plugin}: missing" "${grant_dry_output}"
if [[ ! -e "${grant_absent_dir}" ]]; then
    ok "grant read-only modes do not create the permissions directory"
else
    fail "grant read-only modes do not create the permissions directory"
fi
grant_conflict_rc=0
run_grant --dry-run --check "${grant_plugin}" >/dev/null 2>&1 || grant_conflict_rc=$?
assert_equals "grant rejects --dry-run with --check as usage error" "2" "${grant_conflict_rc}"

grant_before_checksum=$(cksum < "${grant_perms}")
case "$(uname -s)" in
    Darwin) grant_before_mtime=$(stat -f %m "${grant_perms}") ;;
    *) grant_before_mtime=$(stat -c %Y "${grant_perms}") ;;
esac
run_grant --dry-run "${grant_plugin}" >/dev/null
assert_equals "grant --dry-run preserves existing permissions bytes" "${grant_before_checksum}" "$(cksum < "${grant_perms}")"
case "$(uname -s)" in
    Darwin) grant_after_mtime=$(stat -f %m "${grant_perms}") ;;
    *) grant_after_mtime=$(stat -c %Y "${grant_perms}") ;;
esac
assert_equals "grant --dry-run preserves existing permissions mtime" "${grant_before_mtime}" "${grant_after_mtime}"
grant_check_rc=0
grant_check_output=$(run_grant --check "${grant_plugin}") || grant_check_rc=$?
assert_equals "grant --check fails when only an unrelated key exists" "1" "${grant_check_rc}"
assert_contains "grant --check excludes unrelated grants" "${grant_plugin}: missing" "${grant_check_output}"

grant_single_perms="${grant_home}/single-permissions.kdl"
printf '"file:~/.config/zellij/plugins/showy-quota-zellij-chezmoi.wasm" {\n    WebAccess\n}\n' > "${grant_single_perms}"
grant_check_rc=0
grant_check_output=$(grant_override_perms="${grant_single_perms}" run_grant --check "${grant_plugin}") || grant_check_rc=$?
assert_equals "grant --check accepts one matching key" "0" "${grant_check_rc}"
assert_contains "grant --check reports matching file:~ key" "file:~/.config/zellij/plugins/showy-quota-zellij-chezmoi.wasm: present (WebAccess)" "${grant_check_output}"
assert_contains "grant --check reports other keys missing" "${grant_plugin}: missing" "${grant_check_output}"

run_grant "${grant_plugin}" >/dev/null
grant_content="$(< "${grant_perms}")"
assert_contains "grant writes bare absolute path key" "\"${grant_plugin}\" {" "${grant_content}"
assert_contains "grant writes file: url key" "\"file:${grant_plugin}\" {" "${grant_content}"
assert_contains "grant writes home-relative file:~ key" '"file:~/.config/zellij/plugins/showy-quota-zellij-chezmoi.wasm" {' "${grant_content}"
assert_contains "grant requests WebAccess by default" "WebAccess" "${grant_content}"
assert_contains "grant preserves unrelated plugin block" '"/other/plugin.wasm" {' "${grant_content}"
assert_contains "grant preserves unrelated permission" "ReadApplicationState" "${grant_content}"
assert_not_contains "grant omits RunCommands by default" "RunCommands" "${grant_content}"

grant_check_rc=0
grant_check_output=$(run_grant --check "${grant_plugin}") || grant_check_rc=$?
assert_equals "grant --check succeeds with matching grant" "0" "${grant_check_rc}"
assert_contains "grant --check reports granted permission" "${grant_plugin}: present (WebAccess)" "${grant_check_output}"
grant_check_rc=0
grant_check_output=$(run_grant --check --cli-fallback "${grant_plugin}") || grant_check_rc=$?
assert_equals "grant --check fails when RunCommands is absent" "1" "${grant_check_rc}"
assert_contains "grant --check reports partial grant" "${grant_plugin}: present (WebAccess)" "${grant_check_output}"

# Re-running must not duplicate our own blocks (idempotent + self-healing).
run_grant "${grant_plugin}" >/dev/null
grant_dupes=$(grep -c -F "\"${grant_plugin}\" {" "${grant_perms}" || true)
assert_equals "grant is idempotent for the bare key block" "1" "${grant_dupes}"
assert_contains "idempotent re-run keeps unrelated block" '"/other/plugin.wasm" {' "$(< "${grant_perms}")"

# Opt-in flags widen the grant to the capabilities the plugin actually uses.
run_grant --manage-serve --cli-fallback "${grant_plugin}" >/dev/null
grant_optin="$(< "${grant_perms}")"
assert_contains "grant --cli-fallback adds RunCommands" "RunCommands" "${grant_optin}"
assert_contains "grant --manage-serve adds OpenTerminalsOrPlugins" "OpenTerminalsOrPlugins" "${grant_optin}"
grant_check_rc=0
run_grant --check --manage-serve --cli-fallback "${grant_plugin}" >/dev/null || grant_check_rc=$?
assert_equals "grant --check accepts all requested permissions" "0" "${grant_check_rc}"
# Restore the default (minimal) grant for the remaining assertions.
run_grant "${grant_plugin}" >/dev/null

# A missing plugin file is refused (blocks the pre-seed-then-swap attack) unless
# --force is passed for intentional pre-provisioning.
grant_missing="${grant_home}/nope/ghost.wasm"
grant_rc=0
run_grant "${grant_missing}" >/dev/null 2>&1 || grant_rc=$?
assert_equals "grant refuses absent plugin without --force" "1" "${grant_rc}"
if grep -qF "\"${grant_missing}\" {" "${grant_perms}"; then
    fail "grant does not write absent plugin without --force"
else
    ok "grant does not write absent plugin without --force"
fi
grant_force_rc=0
run_grant --force "${grant_missing}" >/dev/null 2>&1 || grant_force_rc=$?
assert_equals "grant --force pre-provisions absent plugin" "0" "${grant_force_rc}"
assert_contains "grant --force writes key for absent plugin" "\"${grant_missing}\" {" "$(< "${grant_perms}")"

# No-arg grant targets the default installed wasm under ZELLIJ_PLUGINS.
grant_default_perms="${grant_home}/default-permissions.kdl"
: > "${grant_home}/.config/zellij/plugins/showy-quota-zellij.wasm"
env \
    PATH="${stub_dir}:${PATH}" \
    HOME="${grant_home}" \
    ZELLIJ_PLUGINS="${grant_home}/.config/zellij/plugins" \
    SHOWY_QUOTA_ZELLIJ_PERMISSIONS_FILE="${grant_default_perms}" \
    "${REPO_ROOT}/bin/showy-quota" --grant-zellij >/dev/null
assert_contains "no-arg grant targets default plugin name" "/showy-quota-zellij.wasm\" {" "$(< "${grant_default_perms}")"

# A plugin path containing a KDL-significant character is rejected (no injection).
grant_inject_rc=0
run_grant '/tmp/x" { RunCommands }; "y.wasm' >/dev/null 2>&1 || grant_inject_rc=$?
if (( grant_inject_rc != 0 )); then
    ok "grant rejects quote in plugin path"
else
    fail "grant rejects quote in plugin path" "rc=${grant_inject_rc}"
fi

# A non-.kdl permissions-file override is rejected before any write.
grant_badperms_rc=0
env \
    PATH="${stub_dir}:${PATH}" \
    HOME="${grant_home}" \
    SHOWY_QUOTA_ZELLIJ_PERMISSIONS_FILE="${grant_home}/evil.txt" \
    "${REPO_ROOT}/bin/showy-quota" --grant-zellij "${grant_plugin}" >/dev/null 2>&1 || grant_badperms_rc=$?
if (( grant_badperms_rc != 0 )) && [[ ! -e "${grant_home}/evil.txt" ]]; then
    ok "grant rejects non-.kdl permissions file override"
else
    fail "grant rejects non-.kdl permissions file override" "rc=${grant_badperms_rc}; file exists: $([[ -e ${grant_home}/evil.txt ]] && echo yes)"
fi

# ── zellij renderer ──────────────────────────────────────────────────
printf 'zellij renderer\n'

sextant_fixture="${TMP}/codexbar-sextant.json"
printf '%s\n' \
    '[' \
    '{"provider":"claude","usage":{"primary":{"usedPercent":25},"secondary":{"usedPercent":50,"windowMinutes":100,"resetsAt":"2099-01-01T01:40:00Z"},"tertiary":{"usedPercent":75}}}' \
    ']' > "${sextant_fixture}"

mono_fixture="${TMP}/codexbar-mono.json"
printf '%s\n' \
    '[' \
    '{"provider":"gemini","usage":{"primary":{"usedPercent":25,"windowMinutes":100,"resetsAt":"2099-01-01T01:40:00Z"},"secondary":{"usedPercent":50,"windowMinutes":200,"resetsAt":"2099-01-01T03:20:00Z"},"tertiary":{"usedPercent":75,"windowMinutes":300,"resetsAt":"2099-01-01T05:00:00Z"}}}' \
    ']' > "${mono_fixture}"

mono_claude_fixture="${TMP}/codexbar-mono-claude.json"
printf '%s\n' \
    '[' \
    '{"provider":"claude","usage":{"primary":{"usedPercent":25,"windowMinutes":100,"resetsAt":"2099-01-01T01:40:00Z"},"secondary":{"usedPercent":50,"windowMinutes":200,"resetsAt":"2099-01-01T03:20:00Z"},"tertiary":{"usedPercent":75,"windowMinutes":300,"resetsAt":"2099-01-01T05:00:00Z"}}}' \
    ']' > "${mono_claude_fixture}"

mixed_error_fixture="${TMP}/codexbar-mixed-error.json"
jq '
    [.[] | select(.error == null)]
    + [{"provider":"cursor","error":{"code":1,"kind":"provider","message":"No Cursor session found."}}]
' "$(fixture_path codexbar-mixed.json)" > "${mixed_error_fixture}"

sanitized_error_fixture="${TMP}/codexbar-error-sanitize.json"
# Padding that exceeds the 160-char truncation cap without forming a long
# alphanumeric run (which the secret-redaction pass collapses): short
# space-separated tokens keep this exercising truncation, not redaction.
sanitized_padding=""
while (( ${#sanitized_padding} < 200 )); do
    sanitized_padding+="word "
done
sanitized_raw_message=$'Token\x01bad  '"${sanitized_padding}"
jq -n --arg message "${sanitized_raw_message}" \
    '[{"provider":"cursor","error":{"code":1,"kind":"provider","message":$message}}]' \
    > "${sanitized_error_fixture}"





out=$(run_renderer showy-quota-zellij-bar codexbar-mixed.json)
assert_contains "renders CL sigil for claude"          "CL"  "${out}"
assert_contains "renders CX sigil for codex"           "CX"  "${out}"
assert_contains "renders GE sigil for gemini"          "GE"  "${out}"
assert_contains "renders CR sigil for errored cursor"   "CR"  "${out}"
assert_contains "renders provider error label"          "⚠err" "${out}"
assert_not_contains "strip hides raw cursor error text" "No Cursor session found." "${out}"
assert_contains "zellij weekly hint uses derived secondary color" "20;104;58m" "${out}"
assert_contains "zellij uses ai-quota powerline left cap" "" "${out}"
assert_contains "zellij uses ai-quota half-block cells" "▀" "${out}"
assert_not_contains "zellij no longer emits old block bar" "████" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-mixed.json SHOWY_QUOTA_DEGRADED_CLI=1 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij degraded CLI marker is visible" "⚠cli" "${out}"

out=$(run_renderer showy-quota-zellij-bar "${sextant_fixture}" SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij default terminal mode keeps half-block cells" "▀" "${out}"
assert_not_contains "zellij default terminal mode is not a stacked sextant body" "🬎" "${out}"

out=$(run_renderer showy-quota-zellij-bar "${sextant_fixture}" SHOWY_QUOTA_TERMINAL_BAR_MODE=mono3 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij forced mono3 renders three stacked row geometry" "CL▕██🬎🬎🬂🬂  ▏" "${out}"
assert_not_contains "zellij forced mono3 omits half-block cells" "▀" "${out}"

out=$(run_renderer showy-quota-zellij-bar "${mono_fixture}" SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070912400 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij auto mono3 uses primary marker by default" "GE▕██🬎│🬂🬂  ▏" "${out}"
assert_not_contains "zellij auto mono3 omits half-block cells" "▀" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-antigravity-quad.json SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij antigravity auto-detect splits into AGᴳ" "AGᴳ▕▀▀▀▀▀▀▀▀▀▀▀▀▏" "${out}"
assert_contains "zellij antigravity auto-detect splits into AGᶜ" "AGᶜ▕▀▀▀▀▀▀▀▀▀▀▀▀▏" "${out}"

# Antigravity via OAuth reports only the Gemini pool (one family); the bar
# auto-detects the pool and adapts to a single plain dual (no family tag),
# where AGY's two pools render as dual2 above — same detection, different shape.
out=$(run_renderer showy-quota-zellij-bar codexbar-antigravity-oauth.json SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij antigravity one pool renders plain dual" "AG▕▀▀▀▀▀▀▀▀▀▀▀▀▏" "${out}"
assert_not_contains "zellij antigravity one pool omits family tag" "G▀" "${out}"

out=$(run_renderer showy-quota-zellij-bar "${mono_fixture}" SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070914800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij auto mono3 primary boundary zero starts after left separator" "GE▕│█🬎🬎🬂🬂  ▏" "${out}"

out=$(run_renderer showy-quota-zellij-bar "${mono_fixture}" SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij auto mono3 primary boundary width replaces last cell" "GE▕██🬎🬎🬂🬂 │▏" "${out}"

out=$(run_renderer showy-quota-zellij-bar "${mono_fixture}" SHOWY_QUOTA_MONO_MARKERS=secondary SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070912400 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij auto mono3 marker source can use secondary" "GE▕██🬎🬎🬂│  ▏" "${out}"

out=$(run_renderer showy-quota-zellij-bar "${mono_fixture}" SHOWY_QUOTA_MONO_MARKERS=none SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070912400 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij auto mono3 marker source can be disabled" "GE▕██🬎🬎🬂🬂  ▏" "${out}"
assert_not_contains "zellij auto mono3 disabled marker omits separator" "│" "${out}"


out=$(run_renderer showy-quota-zellij-bar "${mono_claude_fixture}" SHOWY_QUOTA_PROVIDER_MODES=claude=mono3 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070912400 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij provider mono3 override uses mono marker path" "CL▕██🬎│🬂🬂  ▏" "${out}"
assert_not_contains "zellij provider mono3 override omits half-block cells" "▀" "${out}"

out=$(run_renderer showy-quota-zellij-bar "${mono_fixture}" SHOWY_QUOTA_PROVIDER_MODES=gemini=dual SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070912400 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij provider dual override forces dual" "▀" "${out}"
assert_not_contains "zellij provider dual override suppresses separator" "│" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-no-tertiary.json SHOWY_QUOTA_TERMINAL_BAR_MODE=mono3 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij mono3 collapses to dual when tertiary absent" "AG▕▀▀▀▀▀▀▀▀▏" "${out}"
assert_not_contains "zellij mono3 collapse omits sextant cells" "🬂" "${out}"
assert_not_contains "zellij mono3 collapse omits shared separator" "│" "${out}"

# ── Cursor shared-cycle pools ───────────────────────────────────────
# Cursor reports Total/Auto/API as parallel pools sharing one billing cycle
# (identical resetsAt + 30-day window): categories within one monthly budget,
# not a live tier over a longer cap. Every row stays bright (no long-horizon
# dimming) and only the primary pacing marker is drawn. The default
# provider_modes map renders cursor as mono3.
out=$(run_renderer showy-quota-zellij-bar codexbar-cursor.json SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij cursor defaults to mono3 with one marker" "CR▕███│██🬰🬭▏2w" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-cursor.json SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800)
assert_contains "zellij cursor shared-cycle stays bright" "38;2;37;190;106" "${out}"
assert_not_contains "zellij cursor shared-cycle not dimmed" "38;2;20;104;58" "${out}"

# Forced dual: both rows bright, only the primary pacing marker (foreground);
# the redundant secondary marker (same column, background) is suppressed.
out=$(run_renderer showy-quota-zellij-bar codexbar-cursor.json SHOWY_QUOTA_TERMINAL_BAR_MODE=dual SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800)
assert_contains "zellij cursor dual draws primary pacing marker" "38;2;190;149;255" "${out}"
assert_not_contains "zellij cursor dual drops redundant secondary marker" "48;2;190;149;255" "${out}"

# tmux mirror: bright fills, single marker, no dimmed cap color (#14683a).
out=$(run_renderer showy-quota-tmux-bar codexbar-cursor.json SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800)
assert_contains "tmux cursor shared-cycle stays bright" "fg=#25be6a" "${out}"
assert_not_contains "tmux cursor shared-cycle not dimmed" "#14683a" "${out}"
assert_contains "tmux cursor single pacing marker" "fg=#be95ff" "${out}"

# mono4: four per-pool windows packed into one octant row (model-pooled provider).
out=$(run_renderer showy-quota-zellij-bar codexbar-antigravity-quad.json SHOWY_QUOTA_PROVIDER_MODES=antigravity=mono4 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij mono4 packs four windows into octant row" "AG▕𜷝𜷝𜴪𜴪𜴪𜴪𜴪│𜴧𜴧  ▏" "${out}"
assert_not_contains "zellij mono4 omits half-block dual cells" "▀" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-antigravity-quad.json SHOWY_QUOTA_PROVIDER_MODES=antigravity=mono4 SHOWY_QUOTA_MONO_MARKERS=primary,tertiary SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij mono4 draws two configured pacing markers" "AG▕𜷝𜷝𜴪𜴪𜴪𜴪𜴪│𜴧│  ▏" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-no-tertiary.json SHOWY_QUOTA_PROVIDER_MODES=antigravity=mono4 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij mono4 collapses to dual without four windows" "AG▕▀▀▀▀▀▀▀▀▏" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-codex-mono4-four.json SHOWY_QUOTA_PROVIDER_MODES=codex=mono4 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij forced mono4 keeps three positional plus one extra as four lanes" "CX▕█🮅🮅▀▀▀🮂│🮂   ▏" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-codex-mono4-three.json SHOWY_QUOTA_PROVIDER_MODES=codex=mono4 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij forced mono4 collapses three assembled windows to mono3" "CX▕███🬎🬎🬎🬂│🬂   ▏" "${out}"
assert_not_contains "zellij forced mono4 three-window fallback omits octant-only cells" "🮅" "${out}"

# dual2: a model-pooled provider splits into one standalone per-family dual each
# (AGᴳ, AGᶜ), rendered through the normal dual path — half-blocks, every terminal.
out=$(run_renderer showy-quota-zellij-bar codexbar-antigravity-quad.json SHOWY_QUOTA_PROVIDER_MODES=antigravity=dual2 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij dual2 splits into AGᴳ dual" "AGᴳ▕▀▀▀▀▀▀▀▀▀▀▀▀▏" "${out}"
assert_contains "zellij dual2 splits into AGᶜ dual" "AGᶜ▕▀▀▀▀▀▀▀▀▀▀▀▀▏" "${out}"
assert_not_contains "zellij dual2 uses only half-blocks" "🬂" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-no-tertiary.json SHOWY_QUOTA_PROVIDER_MODES=antigravity=dual2 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij dual2 falls back to dual without family windows" "AG▕▀▀▀▀▀▀▀▀▏" "${out}"

# Q4: a non-pooled provider can be manually pooled. Codex's main pool lives in
# the positional slots and its Spark pool in the extras; auto-detection leaves
# it a plain dual, but an explicit dual2 unions both into per-family sub-bars.
out=$(run_renderer showy-quota-zellij-bar codexbar-codex-spark.json SHOWY_QUOTA_PROVIDERS=codex SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij codex auto stays positional dual (extras are a separate pool)" "CX▕▀▀▀▀▀▀▀▀▀▀▀▀▏" "${out}"
out=$(run_renderer showy-quota-zellij-bar codexbar-codex-spark.json SHOWY_QUOTA_PROVIDER_MODES=codex=dual2 SHOWY_QUOTA_PROVIDERS=codex SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij codex manual dual2 splits main into CXᶜ" "CXᶜ▕▀▀▀▀▀▀▀▀▀▀▀▀▏" "${out}"
assert_contains "zellij codex manual dual2 splits spark into CXˢ" "CXˢ▕▀▀▀▀▀▀▀▀▀▀▀▀▏" "${out}"

# Codex after OpenAI temporarily removed the 5h limit: usage.primary is null,
# so the weekly cap left-compacts into the primary row and drives the countdown
# (a real reset, not an empty top + idle). With only one live window it renders
# as a single full-height bar (no empty second row). Its Spark weekly shares the
# reset key but must NOT make Codex look model-pooled (no CXˢ/CXᶜ split).
out=$(run_renderer showy-quota-zellij-bar codexbar-codex-no-5h.json SHOWY_QUOTA_PROVIDERS=codex SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij codex null primary promotes weekly as one full bar" "CX▕████████████▏6d" "${out}"
assert_not_contains "zellij codex single window uses no half-block" "▀" "${out}"
assert_not_contains "zellij codex null primary is not split by family (Spark)" "CXˢ" "${out}"
assert_not_contains "zellij codex null primary is not split by family (main)" "CXᶜ" "${out}"
# The single full-height bar is the dim-good long-horizon fill over the surface.
out=$(run_renderer showy-quota-zellij-bar codexbar-codex-no-5h.json SHOWY_QUOTA_PROVIDERS=codex SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=12 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_FORCE_COLOR=1)
assert_contains "zellij codex promoted weekly fills a full-height bar" $'38;2;20;104;58m\x1b[48;2;42;42;42m█' "${out}"

# Horizon model: in a time-tiered provider the short (5h) window stays bright
# and the long (weekly) window dims, and BOTH rows show a pacing marker.
out=$(run_renderer showy-quota-zellij-bar codexbar-mixed.json SHOWY_QUOTA_PROVIDERS=claude SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070919600 SHOWY_QUOTA_FORCE_COLOR=1)
assert_contains "zellij dual keeps short (5h) window bright" "38;2;37;190;106" "${out}"
assert_contains "zellij dual dims long (weekly) window" "48;2;20;104;58" "${out}"
assert_contains "zellij dual paces the primary row" "38;2;190;149;255" "${out}"
assert_contains "zellij dual paces the secondary row" "48;2;190;149;255" "${out}"

# Horizon model: a weekly-only provider (Antigravity pools) dims its filled
# window and never uses a bright primary-good fill for a weekly window.
out=$(run_renderer showy-quota-zellij-bar codexbar-no-tertiary.json SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_FORCE_COLOR=1)
assert_contains "zellij dual dims weekly-only secondary fill" "48;2;132;96;0" "${out}"
assert_not_contains "zellij dual weekly window is not bright-good fill" "48;2;37;190;106" "${out}"


out=$(run_renderer showy-quota-zellij-bar codexbar-empty.json)
assert_contains "empty provider array still renders 'AI none'" "AI none" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-error-only.json)
assert_contains "zellij all-error fixture renders cursor sigil" "CR" "${out}"
assert_contains "zellij all-error fixture renders factory sigil" "FA" "${out}"
assert_contains "zellij all-error fixture renders provider error label" "⚠err" "${out}"
assert_not_contains "zellij all-error fixture does not render 'AI idle'" "AI idle" "${out}"
assert_not_contains "zellij all-error strip hides cursor error text" "No Cursor session found." "${out}"
assert_not_contains "zellij all-error strip hides factory error text" "Safari cookies not readable." "${out}"

out=$(run_renderer showy-quota-zellij-bar "${mixed_error_fixture}" SHOWY_QUOTA_PROVIDER_ORDER=codex,cursor,claude,gemini NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "zellij mixed error fixture keeps codex chunk" "CX▕" "${out}"
assert_contains "zellij mixed error fixture keeps claude chunk" "CL▕" "${out}"
assert_contains "zellij mixed error fixture keeps gemini chunk" "GE▕" "${out}"
assert_contains "zellij mixed error fixture renders cursor error chunk" "CR    ⚠err    " "${out}"
if [[ "${out}" == *CX*"CR    ⚠err    "*CL*GE* ]]; then
    ok "zellij mixed error fixture follows provider order"
else
    fail "zellij mixed error fixture follows provider order" "${out}"
fi

out=$(run_renderer showy-quota-zellij-bar codexbar-error-only.json SHOWY_QUOTA_ERROR_GLYPH='!' NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "custom error glyph renders bang error label" "!err" "${out}"
assert_not_contains "custom error glyph replaces warning label" "⚠err" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-error-only.json SHOWY_QUOTA_PROVIDERS_EXCLUDE=cursor NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_not_contains "exclude drops cursor error chunk from strip" "CR" "${out}"
assert_contains "exclude keeps factory error chunk in strip" "FA    ⚠err    " "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-empty.json SHOWY_QUOTA_DEGRADED_CLI=1 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "empty degraded fixture renders trailing CLI marker" "AI none ⚠cli" "${out}"

idle_cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-empty.json" "${idle_cache}/usage.json"
seed_usage_source "${idle_cache}" "cli"
touch -t 198801010000 "${idle_cache}/usage.json"
out=$(
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${idle_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-zellij-idle" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_FORCE_COLOR=0 \
        NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_contains "stale degraded idle cache renders both markers" "AI none ⚠ ⚠cli" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-low.json)
# Bad-palette ee5396 = decimal RGB 238;83;150 inside the truecolor escape.
assert_contains "low-remaining fixture uses BAD palette" "238;83;150" "${out}"

json_cache=$(mk_cache)
out=$(
    env \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${json_cache}" \
        SHOWY_QUOTA_FETCH_BIN="${TMP}/missing-fetch" \
        SHOWY_QUOTA_FORCE_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar" --json - < "$(fixture_path codexbar-mixed.json)"
)
assert_contains "zellij --json stdin renders without fetch" "CL" "${out}"
ansi_dim=$'\x1b[2m'
assert_not_contains "zellij --json stdin skips stale dimming" "${ansi_dim}" "${out}"

printf '{"schema":"showy-quota/cache@1","source":"cli","providers":[]}' > "${json_cache}/usage.json"
out=$(
    env \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${json_cache}" \
        SHOWY_QUOTA_FETCH_BIN="${TMP}/missing-fetch" \
        SHOWY_QUOTA_FORCE_COLOR=0 \
        NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar" --json "$(fixture_path codexbar-mixed.json)"
)
assert_not_contains "zellij --json file ignores live cache degraded marker" "⚠cli" "${out}"

# --json must reject a readable file that is not valid quota JSON (e.g. a
# non-array payload or an arbitrary file like /etc/passwd) rather than render it.
json_bad_rc=0
env SHOWY_QUOTA_NO_CONFIG=1 "${REPO_ROOT}/bin/showy-quota-zellij-bar" --json "$(fixture_path codexbar-non-array.json)" >/dev/null 2>&1 || json_bad_rc=$?
if (( json_bad_rc != 0 )); then
    ok "zellij --json rejects non-quota JSON file"
else
    fail "zellij --json rejects non-quota JSON file" "rc=${json_bad_rc}"
fi

printf '\ndriver cache freshness contract\n'
driver_fetch_stub="${TMP}/driver-fetch-no-persist"
cat > "${driver_fetch_stub}" <<'EOF'
#!/bin/sh
exit 1
EOF
chmod +x "${driver_fetch_stub}"

driver_cache=$(mk_cache)
seed_usage_cache "${driver_cache}" codexbar-empty.json serve
out=$(
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${driver_cache}" \
        SHOWY_QUOTA_FETCH_BIN="${driver_fetch_stub}" \
        SHOWY_QUOTA_FORCE_COLOR=0 \
        NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_contains "driver fresh cache renders seeded data" "AI none" "${out}"
assert_not_contains "driver fresh cache omits stale marker" "⚠" "${out}"

driver_stale_cache=$(mk_cache)
seed_usage_cache "${driver_stale_cache}" codexbar-empty.json serve
touch -t 202001010000 "${driver_stale_cache}/usage.json"
out=$(
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${driver_stale_cache}" \
        SHOWY_QUOTA_FETCH_BIN="${driver_fetch_stub}" \
        SHOWY_QUOTA_FORCE_COLOR=0 \
        NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_contains "driver stale cache shows stale marker" "AI none ⚠" "${out}"

driver_degraded_cache=$(mk_cache)
seed_usage_cache "${driver_degraded_cache}" codexbar-empty.json cli
out=$(
    env -u SHOWY_QUOTA_DEGRADED_CLI \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${driver_degraded_cache}" \
        SHOWY_QUOTA_FETCH_BIN="${driver_fetch_stub}" \
        SHOWY_QUOTA_FORCE_COLOR=0 \
        NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_contains "driver cli source derives degraded marker" "AI none ⚠cli" "${out}"

out=$(
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${driver_degraded_cache}" \
        SHOWY_QUOTA_FETCH_BIN="${driver_fetch_stub}" \
        SHOWY_QUOTA_DEGRADED_CLI=0 \
        SHOWY_QUOTA_FORCE_COLOR=0 \
        NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_not_contains "driver degraded CLI zero disables marker" "⚠cli" "${out}"

driver_explicit_degraded_cache=$(mk_cache)
seed_usage_cache "${driver_explicit_degraded_cache}" codexbar-empty.json serve
out=$(
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${driver_explicit_degraded_cache}" \
        SHOWY_QUOTA_FETCH_BIN="${driver_fetch_stub}" \
        SHOWY_QUOTA_DEGRADED_CLI=1 \
        SHOWY_QUOTA_FORCE_COLOR=0 \
        NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_contains "driver degraded CLI one forces marker" "AI none ⚠cli" "${out}"

driver_missing_cache=$(mk_cache)
out=$(
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${driver_missing_cache}" \
        SHOWY_QUOTA_FETCH_BIN="${driver_fetch_stub}" \
        SHOWY_QUOTA_FORCE_COLOR=0 \
        NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_equals "driver missing cache falls back to neutral segment" "AI ?" "${out}"

printf '\nrender CLI integration\n'
if (cd "${REPO_ROOT}" && cargo test -p showy-quota-zellij-core --test render_cli); then
    ok "render CLI matches in-process renderer"
else
    fail "render CLI matches in-process renderer"
fi

# ── tmux renderer ────────────────────────────────────────────────────

printf '\ntmux renderer\n'

out=$(run_renderer showy-quota-tmux-bar codexbar-mixed.json)
assert_contains "tmux markup applies bold style"        "bold]" "${out}"
assert_contains "tmux markup names claude sigil"       "CL"      "${out}"
assert_contains "tmux markup uses #[default] reset"    "#[default]" "${out}"
assert_contains "tmux uses zellij powerline left cap"  "" "${out}"
assert_contains "tmux uses half-block primary/secondary cells" "▀" "${out}"
assert_contains "tmux uses derived secondary background color" "bg=#14683a" "${out}"
assert_not_contains "tmux no longer emits weekly hint glyph" "]w" "${out}"

out=$(run_renderer showy-quota-tmux-bar codexbar-mixed.json SHOWY_QUOTA_DEGRADED_CLI=1)
visible=$(strip_tmux_markup "${out}")
assert_contains "tmux degraded CLI marker is visible" "⚠cli" "${visible}"

out=$(run_renderer showy-quota-tmux-bar "${sextant_fixture}" SHOWY_QUOTA_TERMINAL_BAR_MODE=mono3 SHOWY_QUOTA_TMUX_BAR_WIDTH=8)
visible=$(strip_tmux_markup "${out}")
assert_contains "tmux forced mono3 renders three stacked row geometry" "CL▕██🬎🬎🬂🬂  ▏" "${visible}"
assert_not_contains "tmux forced mono3 omits half-block cells" "▀" "${visible}"
assert_contains "tmux forced mono3 colors cells with single chunk color" "fg=#f0af00,bg=#2a2a2a]█" "${out}"
assert_not_contains "tmux forced mono3 omits elapsed markers" "be95ff" "${out}"

out=$(run_renderer showy-quota-tmux-bar "${mono_fixture}" SHOWY_QUOTA_TMUX_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070912400)
visible=$(strip_tmux_markup "${out}")
assert_contains "tmux auto mono3 visible output preserves separator geometry" "GE▕██🬎│🬂🬂  ▏" "${visible}"
assert_not_contains "tmux auto mono3 omits half-block cells" "▀" "${visible}"
assert_contains "tmux auto mono3 colors separator with elapsed palette" "fg=#be95ff,bg=#2a2a2a]│" "${out}"
assert_contains "tmux auto mono3 uses one primary-palette foreground" "fg=#f0af00,bg=#2a2a2a]█" "${out}"
assert_contains "tmux auto mono3 colors combined sextants with primary foreground" "fg=#f0af00,bg=#2a2a2a]🬎" "${out}"
assert_not_contains "tmux auto mono3 does not use bottom-role all-row color" "fg=#846000,bg=#2a2a2a]█" "${out}"
assert_not_contains "tmux auto mono3 does not use role color for combined sextants" "fg=#846000,bg=#2a2a2a]🬎" "${out}"
out=$(run_renderer showy-quota-tmux-bar "${mono_fixture}" SHOWY_QUOTA_TMUX_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800)
visible=$(strip_tmux_markup "${out}")
assert_contains "tmux auto mono3 primary boundary width replaces last cell" "GE▕██🬎🬎🬂🬂 │▏" "${visible}"


out=$(run_renderer showy-quota-tmux-bar "${mono_claude_fixture}" SHOWY_QUOTA_PROVIDER_MODES=claude=mono3 SHOWY_QUOTA_TMUX_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070912400)
visible=$(strip_tmux_markup "${out}")
assert_contains "tmux provider mono3 override uses mono marker path" "CL▕██🬎│🬂🬂  ▏" "${visible}"
assert_contains "tmux provider mono3 override colors separator with elapsed palette" "fg=#be95ff,bg=#2a2a2a]│" "${out}"

out=$(run_renderer showy-quota-tmux-bar codexbar-no-tertiary.json SHOWY_QUOTA_TERMINAL_BAR_MODE=mono3 SHOWY_QUOTA_TMUX_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 SHOWY_QUOTA_NOW_EPOCH=4070908800)
visible=$(strip_tmux_markup "${out}")
assert_contains "tmux mono3 collapses to dual when tertiary absent" "AG▕▀▀▀▀▀▀▀▀▏" "${visible}"
assert_not_contains "tmux mono3 collapse omits sextant cells" "🬂" "${visible}"
assert_not_contains "tmux mono3 collapse omits shared separator" "│" "${visible}"


out=$(run_renderer showy-quota-tmux-bar codexbar-empty.json)
assert_contains "tmux empty fixture renders 'AI none'" "AI none" "${out}"

out=$(run_renderer_json showy-quota-tmux-bar codexbar-error-only.json)
visible=$(strip_tmux_markup "${out}")
assert_contains "tmux --json all-error fixture renders cursor sigil" "CR" "${visible}"
assert_contains "tmux --json all-error fixture renders factory sigil" "FA" "${visible}"
assert_contains "tmux --json all-error fixture renders provider error label" "⚠err" "${visible}"
assert_not_contains "tmux --json all-error fixture does not render 'AI idle'" "AI idle" "${visible}"
assert_not_contains "tmux --json all-error strip hides cursor error text" "No Cursor session found." "${visible}"
assert_not_contains "tmux --json all-error strip hides factory error text" "Safari cookies not readable." "${visible}"

out=$(run_renderer showy-quota-tmux-bar "${mixed_error_fixture}" SHOWY_QUOTA_PROVIDER_ORDER=codex,cursor,claude,gemini)
visible=$(strip_tmux_markup "${out}")
assert_contains "tmux mixed error fixture keeps codex chunk" "CX▕" "${visible}"
assert_contains "tmux mixed error fixture keeps claude chunk" "CL▕" "${visible}"
assert_contains "tmux mixed error fixture keeps gemini chunk" "GE▕" "${visible}"
assert_contains "tmux mixed error fixture renders cursor error chunk" "CR    ⚠err    " "${visible}"
if [[ "${visible}" == *CX*"CR    ⚠err    "*CL*GE* ]]; then
    ok "tmux mixed error fixture follows provider order"
else
    fail "tmux mixed error fixture follows provider order" "${visible}"
fi

install_bin="${TMP}/install/bin"
mkdir -p "${install_bin}"
ln -s "${REPO_ROOT}/bin/showy-quota-tmux-bar" "${install_bin}/showy-quota-tmux-bar"
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="$(mk_cache)" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${install_bin}/showy-quota-tmux-bar"
)
assert_contains "installed symlink resolves repo lib" "CL" "${out}"

tmux_stub_dir="${TMP}/tmux-wrapper-bin"
mkdir -p "${tmux_stub_dir}"
tmux_log="${TMP}/tmux-wrapper.log"
tmux_missing_bar="${TMP}/missing-showy-quota-tmux-bar"
cat > "${tmux_stub_dir}/tmux" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "tmux $*" >> "${SHOWY_QUOTA_TEST_TMUX_LOG}"
case "$1" in
    show-options)
        printf '%s\n' '@showy-quota-bin'
        ;;
    show-option)
        if [[ "${*: -1}" == "@showy-quota-bin" ]]; then
            printf '%s\n' "${SHOWY_QUOTA_TEST_TMUX_BAR_BIN}"
        else
            printf '0\n'
        fi
        ;;
    display-message|set-option|bind-key|refresh-client)
        ;;
esac
EOF
chmod +x "${tmux_stub_dir}/tmux"
out=$(
    env \
        PATH="${tmux_stub_dir}:${PATH}" \
        SHOWY_QUOTA_TEST_TMUX_LOG="${tmux_log}" \
        SHOWY_QUOTA_TEST_TMUX_BAR_BIN="${tmux_missing_bar}" \
        "${REPO_ROOT}/showy-quota.tmux" 2>&1
)
tmux_wrapper_log="$(< "${tmux_log}")"
assert_contains "tmux wrapper warns when renderer is not executable" "display-message showy-quota: renderer is not executable: ${tmux_missing_bar}" "${tmux_wrapper_log}"
assert_not_contains "tmux wrapper does not append broken status command" "#(\"${tmux_missing_bar}\")" "${tmux_wrapper_log}"

# ── tmux wrapper injection guard ──────────────────────────────────────
printf '\ntmux wrapper injection guard\n'

tmux_injection_stub_dir="${TMP}/tmux-injection-bin"
mkdir -p "${tmux_injection_stub_dir}"
cat > "${tmux_injection_stub_dir}/tmux" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "tmux $*" >> "${SHOWY_QUOTA_TEST_TMUX_LOG}"
option="${*: -1}"
case "$1" in
    show-options)
        case "${SHOWY_QUOTA_TEST_TMUX_MODE}" in
            escape)
                printf '%s\n' '@showy-quota-separator' '@showy-quota-popup-key' '@showy-quota-popup-title'
                ;;
            bad-bin)
                printf '%s\n' '@showy-quota-bin'
                ;;
        esac
        ;;
    show-option)
        case "${option}" in
            @showy-quota-separator) printf '%s\n' 'x#(id)' ;;
            @showy-quota-popup-key) printf '%s\n' 'Q' ;;
            @showy-quota-popup-title) printf '%s\n' 'x#(id)' ;;
            @showy-quota-bin) printf '%s\n' '$(id)' ;;
            status-right|status-left) printf '\n' ;;
            *) printf '0\n' ;;
        esac
        ;;
    display-message|set-option|bind-key|refresh-client)
        ;;
esac
EOF
chmod +x "${tmux_injection_stub_dir}/tmux"

tmux_escape_log="${TMP}/tmux-wrapper-escape.log"
: > "${tmux_escape_log}"
out=$(
    env \
        PATH="${tmux_injection_stub_dir}:${PATH}" \
        SHOWY_QUOTA_TEST_TMUX_LOG="${tmux_escape_log}" \
        SHOWY_QUOTA_TEST_TMUX_MODE=escape \
        "${REPO_ROOT}/showy-quota.tmux" 2>&1
)
tmux_wrapper_log="$(< "${tmux_escape_log}")"
assert_contains "tmux wrapper escapes separator format command" "set-option -gq -a status-right x##(id)#(" "${tmux_wrapper_log}"
assert_not_contains "tmux wrapper does not keep raw separator format command" "set-option -gq -a status-right x#(id)#(" "${tmux_wrapper_log}"
assert_contains "tmux wrapper escapes popup title format command" "bind-key Q display-popup -E -h 36 -w 92 -T x##(id)" "${tmux_wrapper_log}"
assert_not_contains "tmux wrapper does not keep raw popup title format command" " -T x#(id) " "${tmux_wrapper_log}"

tmux_bad_bin_log="${TMP}/tmux-wrapper-bad-bin.log"
: > "${tmux_bad_bin_log}"
out=$(
    env \
        PATH="${tmux_injection_stub_dir}:${PATH}" \
        SHOWY_QUOTA_TEST_TMUX_LOG="${tmux_bad_bin_log}" \
        SHOWY_QUOTA_TEST_TMUX_MODE=bad-bin \
        "${REPO_ROOT}/showy-quota.tmux" 2>&1
)
tmux_wrapper_log="$(< "${tmux_bad_bin_log}")"
# shellcheck disable=SC2016  # literal $(id) is the injection payload being asserted
assert_contains "tmux wrapper rejects command-substitution renderer path" 'display-message showy-quota: renderer path contains shell metacharacters: $(id)' "${tmux_wrapper_log}"
# shellcheck disable=SC2016  # literal $(id) is the injection payload being asserted
assert_not_contains "tmux wrapper does not embed command-substitution renderer path" 'set-option -gq -a status-right $(id)#(' "${tmux_wrapper_log}"

# ── filter ───────────────────────────────────────────────────────────

printf '\nprovider filter\n'

out=$(run_renderer showy-quota-zellij-bar codexbar-mixed.json SHOWY_QUOTA_PROVIDERS=claude NO_COLOR=1)
assert_contains "filter restricts to claude"           "CL" "${out}"
assert_not_contains "filter excludes codex"            "CX" "${out}"
assert_not_contains "filter excludes gemini"           "GE" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-mixed.json SHOWY_QUOTA_PROVIDERS_EXCLUDE=codex NO_COLOR=1)
assert_contains "exclude-only keeps claude"            "CL" "${out}"
assert_not_contains "exclude-only drops codex"         "CX" "${out}"
assert_contains "exclude-only keeps gemini"            "GE" "${out}"

out=$(run_renderer showy-quota-tmux-bar codexbar-mixed.json SHOWY_QUOTA_PROVIDERS_EXCLUDE=codex)
assert_not_contains "tmux exclude-only drops codex"    "CX" "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-mixed.json SHOWY_QUOTA_PROVIDERS='claude,codex' SHOWY_QUOTA_PROVIDERS_EXCLUDE=codex NO_COLOR=1)
assert_contains "include+exclude keeps claude"         "CL" "${out}"
assert_not_contains "include+exclude drops codex"      "CX" "${out}"
assert_not_contains "include+exclude drops gemini"     "GE" "${out}"

order_fixture="${TMP}/codexbar-order.json"
printf '%s\n' \
    '[' \
    '{"provider":"gemini","usage":{"primary":{"usedPercent":0}}},' \
    '{"provider":"claude","usage":{"primary":{"usedPercent":10}}},' \
    '{"provider":"opencode","usage":{"primary":{"usedPercent":20}}},' \
    '{"provider":"codex","usage":{"primary":{"usedPercent":30}}}' \
    ']' > "${order_fixture}"
out=$(run_state "${order_fixture}")
assert_equals "default provider order ignores source order" "codex,claude,opencode,gemini" "$(printf '%s' "${out}" | jq -r '.providers | join(",")')"

out=$(run_state codexbar-mixed.json SHOWY_QUOTA_PROVIDER_ORDER=gemini,claude)
assert_equals "provider order skips missing providers without filtering" "gemini,claude,codex" "$(printf '%s' "${out}" | jq -r '.providers | join(",")')"

out=$(run_state codexbar-mixed.json SHOWY_QUOTA_PROVIDER_ORDER=codex,claude,gemini SHOWY_QUOTA_PROVIDERS=gemini,claude)
assert_equals "allow-list order overrides provider order" "gemini,claude" "$(printf '%s' "${out}" | jq -r '.providers | join(",")')"

# ── provider id validator hardening ───────────────────────────────────
printf '\nprovider id validator hardening\n'

unsafe_json_results=()
unsafe_filter_results=()
for unsafe_provider_id in "." ".." "-foo"; do
    unsafe_provider_file="${TMP}/codexbar-unsafe-${unsafe_provider_id//[^A-Za-z0-9]/_}.json"
    printf '[{"provider":"%s","usage":{"primary":{"usedPercent":1}}}]\n' "${unsafe_provider_id}" > "${unsafe_provider_file}"
    # shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
    unsafe_json_results+=("$(run_common_eval 'showy_quota_json_valid "${SHOWY_QUOTA_TEST_FILE}" && printf valid || printf reject' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_FILE="${unsafe_provider_file}")")
    unsafe_filter_results+=("$(run_strip_filter "$(< "${unsafe_provider_file}")" | jq -r 'length')")
done
assert_equals "json_valid rejects unsafe provider ids" "reject,reject,reject" "$(IFS=,; printf '%s' "${unsafe_json_results[*]}")"
assert_equals "filter_renderable rejects unsafe provider ids" "0,0,0" "$(IFS=,; printf '%s' "${unsafe_filter_results[*]}")"

safe_provider_file="${TMP}/codexbar-safe-provider-id.json"
printf '%s\n' '[{"provider":"codex","usage":{"primary":{"usedPercent":1}}}]' > "${safe_provider_file}"
out=$(run_renderer showy-quota-zellij-bar "${safe_provider_file}" NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "provider id validator still renders normal id" "CX" "${out}"

# ── serde payload validator parity ────────────────────────────────────
printf '\nserde payload validator parity\n'

validator_case_names=(
    numeric-status-indicator
    object-status-url
    numeric-extra-title
    string-extra-usage-known
    numeric-window-resets-at
    string-window-minutes
    fractional-window-minutes
)
validator_case_payloads=(
    '[{"provider":"codex","status":{"indicator":1},"usage":{"primary":{"usedPercent":1}}}]'
    '[{"provider":"codex","status":{"url":{}},"usage":{"primary":{"usedPercent":1}}}]'
    '[{"provider":"codex","usage":{"primary":{"usedPercent":1},"extraRateWindows":[{"title":1,"window":{"usedPercent":1}}]}}]'
    '[{"provider":"codex","usage":{"primary":{"usedPercent":1},"extraRateWindows":[{"usageKnown":"false","window":{"usedPercent":1}}]}}]'
    '[{"provider":"codex","usage":{"primary":{"usedPercent":1,"resetsAt":123}}}]'
    '[{"provider":"codex","usage":{"primary":{"usedPercent":1,"windowMinutes":"300"}}}]'
    '[{"provider":"codex","usage":{"primary":{"usedPercent":1,"windowMinutes":300.5}}}]'
)
validator_common_results=()
validator_fetch_results=()
validator_serve_url="http://127.0.0.1:18080"
for validator_case_index in "${!validator_case_names[@]}"; do
    validator_case_file="${TMP}/codexbar-${validator_case_names[validator_case_index]}.json"
    printf '%s\n' "${validator_case_payloads[validator_case_index]}" > "${validator_case_file}"
    # shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
    validator_common_results+=("$(run_common_eval 'showy_quota_json_valid "${SHOWY_QUOTA_TEST_FILE}" && printf valid || printf reject' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_FILE="${validator_case_file}")")

    validator_cache=$(mk_cache)
    validator_rc=0
    validator_out=$(
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${validator_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-validator" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL="${validator_serve_url}" \
        SHOWY_QUOTA_TEST_SERVE_URL="${validator_serve_url}" \
        SHOWY_QUOTA_TEST_SERVE_FIXTURE="${validator_case_file}" \
        "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
    ) || validator_rc=$?
    if (( validator_rc != 0 )) && [[ -z "${validator_out}" ]] && [[ ! -e "${validator_cache}/usage.json" ]]; then
        validator_fetch_results+=("reject")
    else
        validator_fetch_results+=("publish")
    fi
done
assert_equals "json validator rejects serde-incompatible typed fields" "reject,reject,reject,reject,reject,reject,reject" "$(IFS=,; printf '%s' "${validator_common_results[*]}")"
assert_equals "serve validator rejects serde-incompatible typed fields before publication" "reject,reject,reject,reject,reject,reject,reject" "$(IFS=,; printf '%s' "${validator_fetch_results[*]}")"

validator_nullable_file="${TMP}/codexbar-nullable-optional-fields.json"
printf '%s\n' \
    '[{"provider":"codex","status":{"indicator":null},"usage":{"primary":{"usedPercent":null,"resetsAt":null,"windowMinutes":null},"extraRateWindows":[{"id":null,"usageKnown":null,"window":{"remainingPercent":null,"resetDescription":null}}]}}]' \
    > "${validator_nullable_file}"
# shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
out=$(run_common_eval 'showy_quota_json_valid "${SHOWY_QUOTA_TEST_FILE}" && printf valid || printf reject' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_FILE="${validator_nullable_file}")
assert_equals "json validator accepts null and absent serde optionals" "valid" "${out}"

mixed_validity_file="${TMP}/codexbar-mixed-validity.json"
printf '%s\n' \
    '[{"provider":"codex","usage":{"primary":{"usedPercent":1}}},{"provider":"bad/id","usage":{"primary":{"usedPercent":1}}}]' \
    > "${mixed_validity_file}"
# shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
out=$(run_common_eval 'showy_quota_json_valid "${SHOWY_QUOTA_TEST_FILE}" && printf valid || printf reject' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_FILE="${mixed_validity_file}")
assert_equals "json validator accepts a well-formed record beside a malformed sibling" "valid" "${out}"

# ── state surface ─────────────────────────────────────────────────────
printf '\ncodexbar state\n'

out=$(run_state codexbar-mixed.json SHOWY_QUOTA_NOW_EPOCH=4070908800)
assert_equals "state marks cache available" "true" "$(printf '%s' "${out}" | jq -r '.available')"
assert_equals "state provider count honors renderable filter" "3" "$(printf '%s' "${out}" | jq -r '.providerCount')"
assert_equals "state provider order matches render order" "codex,claude,gemini" "$(printf '%s' "${out}" | jq -r '.providers | join(",")')"
assert_equals "state providerMetrics includes renderable and errored providers" "4" "$(printf '%s' "${out}" | jq -r '.providerMetrics | length')"
assert_equals "state claude primary window exposes normalized usage" "17|83|300|340" "$(printf '%s' "${out}" | jq -r '.providerMetrics[] | select(.provider == "claude") | .windows.primary | [.usedPercent, .remainingPercent, .windowMinutes, .minutesUntilReset] | map(tostring) | join("|")')"
assert_equals "state missing tertiary window stays null" "true" "$(printf '%s' "${out}" | jq -r '.providerMetrics[] | select(.provider == "codex") | .windows.tertiary == null')"
assert_equals "state renderable provider error field stays null" "true" "$(printf '%s' "${out}" | jq -r '.providerMetrics[] | select(.provider == "codex") | .error == null')"
assert_equals "state providers stay string array" "true" "$(printf '%s' "${out}" | jq -r '.providers | type == "array" and all(.[]; type == "string")')"
assert_equals "state compact recommendation defaults below threshold" "false" "$(printf '%s' "${out}" | jq -r '.sketchybar.compactRecommended')"
assert_equals "state exposes degraded CLI source" "cli" "$(printf '%s' "${out}" | jq -r '.cache.source')"
assert_equals "state exposes degraded flag" "true" "$(printf '%s' "${out}" | jq -r '.cache.degraded')"

# The core metrics emitter now applies the strict provider-id predicate here.
# This intentionally tightens state providerMetrics versus the old loose jq path,
# which accepted ".", "..", and leading-dash provider ids.
state_strict_ids_fixture="${TMP}/codexbar-strict-provider-metrics.json"
cat > "${state_strict_ids_fixture}" <<'EOF'
[
  {
    "provider": "codex",
    "source": "openai-web",
    "usage": {
      "primary": { "usedPercent": 42, "windowMinutes": 300 },
      "secondary": null,
      "tertiary": null
    }
  },
  {
    "provider": ".",
    "source": "test",
    "usage": {
      "primary": { "usedPercent": 11, "windowMinutes": 300 },
      "secondary": null,
      "tertiary": null
    }
  },
  {
    "provider": "..",
    "source": "test",
    "usage": {
      "primary": { "usedPercent": 12, "windowMinutes": 300 },
      "secondary": null,
      "tertiary": null
    }
  },
  {
    "provider": "-evil",
    "source": "test",
    "usage": {
      "primary": { "usedPercent": 13, "windowMinutes": 300 },
      "secondary": null,
      "tertiary": null
    }
  }
]
EOF
state_strict_ids_cache=$(mk_cache)
cp "${state_strict_ids_fixture}" "${state_strict_ids_cache}/usage.json"
seed_usage_source "${state_strict_ids_cache}" "cli"
state_strict_ids_fetch="${TMP}/state-strict-provider-metrics-fetch"
cat > "${state_strict_ids_fetch}" <<EOF
#!/bin/sh
jq -c '.providers // .' "${state_strict_ids_cache}/usage.json"
EOF
chmod +x "${state_strict_ids_fetch}"
state_strict_ids_json=$(
    env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_NOW_EPOCH=4070908800 \
        SHOWY_QUOTA_CACHE_DIR="${state_strict_ids_cache}" \
        SHOWY_QUOTA_FETCH_BIN="${state_strict_ids_fetch}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-state-strict-ids" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json
)
assert_equals "state providerMetrics strict ids keep valid provider" "true" "$(printf '%s' "${state_strict_ids_json}" | jq -r 'any(.providerMetrics[]; .provider == "codex")')"
assert_equals "state providerMetrics strict ids reject dot provider" "false" "$(printf '%s' "${state_strict_ids_json}" | jq -r 'any(.providerMetrics[]; .provider == ".")')"
assert_equals "state providerMetrics strict ids reject dotdot provider" "false" "$(printf '%s' "${state_strict_ids_json}" | jq -r 'any(.providerMetrics[]; .provider == "..")')"
assert_equals "state providerMetrics strict ids reject leading-dash provider" "false" "$(printf '%s' "${state_strict_ids_json}" | jq -r 'any(.providerMetrics[]; .provider == "-evil")')"

render_metrics_smoke=$(
    env SHOWY_QUOTA_NOW_EPOCH=4070908800 \
        "${RENDER_BIN}" --emit metrics --json - < "${FIXTURE_DIR}/codexbar-mixed.json"
)
assert_equals "render metrics CLI emits state providerMetrics contract" "array|string|number|true" "$(printf '%s' "${render_metrics_smoke}" | jq -r '[(type), (.[0].provider | type), (.[0].windows.primary.usedPercent | type), (.[0].error == null)] | map(tostring) | join("|")')"

# Rows are a structured transport for surfaces that stack chunks: the band
# travels in `severity`/`color`, so the text must stay plain unless `--ansi`
# asks for escapes. A consumer that strips control bytes would otherwise render
# the escape bodies as literal text.
render_rows_smoke=$(
    env SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_FORCE_COLOR=1 \
        "${RENDER_BIN}" --emit rows --json - < "${FIXTURE_DIR}/codexbar-antigravity-quad.json"
)
assert_equals "render rows CLI splits pooled families into one row each" "AGᴳ,AGᶜ" "$(printf '%s' "${render_rows_smoke}" | jq -r 'map(.sigil) | join(",")')"
assert_equals "render rows CLI reports band and colour per row" "good|25be6a" "$(printf '%s' "${render_rows_smoke}" | jq -r '.[1] | [.severity, .color] | join("|")')"
assert_equals "render rows CLI text stays plain even when colour is forced" "true" "$(printf '%s' "${render_rows_smoke}" | jq -r 'all(.[]; (.text | contains("\u001b")) | not)')"

render_rows_ansi=$(
    env SHOWY_QUOTA_NOW_EPOCH=4070908800 \
        "${RENDER_BIN}" --emit rows --ansi --json - < "${FIXTURE_DIR}/codexbar-antigravity-quad.json"
)
assert_equals "render rows CLI honours explicit --ansi" "true" "$(printf '%s' "${render_rows_ansi}" | jq -r 'any(.[]; .text | contains("\u001b"))')"

# The vertical view is the same data on the other axis: one line per quota
# window instead of one line per provider, so windows the strip packs into
# half blocks/sextants each keep their own bar, percent and countdown.
#
# Every assertion below reads the plate as plain text, so colour is pinned off
# here rather than inherited from the environment. The coloured path is covered
# by the `--emit rows --ansi` test above.
render_vertical() {
    env NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0 SHOWY_QUOTA_NOW_EPOCH=4070908800 "$@" \
        "${RENDER_BIN}" --emit vertical --json -
}

render_vertical_quad=$(render_vertical < "${FIXTURE_DIR}/codexbar-antigravity-quad.json")
assert_equals "render vertical CLI gives every pooled window its own line" "4" "$(printf '%s\n' "${render_vertical_quad}" | grep -c '▕')"
assert_equals "render vertical CLI labels each window horizon outside the plate" "5h,7d,5h,7d" "$(printf '%s\n' "${render_vertical_quad}" | grep '▕' | sed -E 's/ *▕.*//; s/.* //' | paste -sd, -)"
assert_equals "render vertical CLI separates provider blocks with one blank line" "1" "$(printf '%s\n' "${render_vertical_quad}" | grep -c '^$')"
# Codepoint counts, not bytes: the family superscript and the Powerline cap are
# multibyte, so awk's byte length would report a misalignment that is not there.
assert_equals "render vertical CLI aligns bars under a wide family chip" "1" "$(printf '%s\n' "${render_vertical_quad}" | grep '▕' | jq -Rn '[inputs | sub("▕.*"; "") | length] | unique | length')"

# Cursor's Total/Auto/API share one reset, horizon and usage but are three
# separate pools: the vertical view must not collapse them into one line, and a
# 30d/31d cycle is labelled as the month it is rather than a raw day count.
render_vertical_cursor=$(render_vertical < "${FIXTURE_DIR}/codexbar-cursor.json")
assert_equals "render vertical CLI keeps same-cycle pools on separate ordinal lines" "1mo¹,1mo²,1mo³" "$(printf '%s\n' "${render_vertical_cursor}" | sed -E 's/ *▕.*//; s/.* //' | paste -sd, -)"
assert_equals "render vertical CLI reports each pool's own percentage" "88%,75%,100%" "$(printf '%s\n' "${render_vertical_cursor}" | grep -oE '[0-9]+%' | paste -sd, -)"

# `urgency` flattens the provider blocks so the window closest to running out is
# the first line. A tie keeps provider order, not sigil order: sorting the sigil
# put CL before CX under the default codex,claude order, contradicting the
# documented behaviour, and no assertion covered a cross-provider tie.
assert_equals "render vertical CLI urgency order leads with the lowest remaining" "75%,88%,100%" "$(render_vertical SHOWY_QUOTA_VERTICAL_SORT=urgency < "${FIXTURE_DIR}/codexbar-cursor.json" | grep -oE '[0-9]+%' | paste -sd, -)"
assert_equals "render vertical CLI urgency order drops the block separators" "0" "$(render_vertical SHOWY_QUOTA_VERTICAL_SORT=urgency < "${FIXTURE_DIR}/codexbar-antigravity-quad.json" | grep -c '^$')"
vertical_tie_fixture="${TMP}/codexbar-vertical-tie.json"
printf '%s\n' '[{"provider":"codex","usage":{"primary":{"usedPercent":50,"windowMinutes":300,"resetsAt":"2099-01-01T00:12:00Z"},"secondary":null,"tertiary":null}},{"provider":"claude","usage":{"primary":{"usedPercent":50,"windowMinutes":300,"resetsAt":"2099-01-01T00:12:00Z"},"secondary":null,"tertiary":null}}]' > "${vertical_tie_fixture}"
assert_equals "render vertical CLI urgency tie keeps provider order" "CX,CL" \
    "$(render_vertical SHOWY_QUOTA_VERTICAL_SORT=urgency < "${vertical_tie_fixture}" | sed -E 's/^[^A-Z]*([A-Z]+).*/\1/' | paste -sd, -)"
assert_equals "render vertical CLI urgency tie follows a configured provider order" "CL,CX" \
    "$(render_vertical SHOWY_QUOTA_VERTICAL_SORT=urgency SHOWY_QUOTA_PROVIDER_ORDER=claude,codex < "${vertical_tie_fixture}" | sed -E 's/^[^A-Z]*([A-Z]+).*/\1/' | paste -sd, -)"

# The clock answers "when" for rows that all read the same relative countdown, so
# it is on by default; `0` trades it back for six columns.
assert_equals "render vertical CLI appends the local reset clock by default" "3" "$(render_vertical SHOWY_QUOTA_RESET_DESCRIPTION_TIMEZONE_OFFSET=0 < "${FIXTURE_DIR}/codexbar-cursor.json" | grep -cE ' [0-9]{2}:[0-9]{2}$')"
assert_equals "render vertical CLI drops the reset clock on request" "0" "$(render_vertical SHOWY_QUOTA_VERTICAL_RESET_CLOCK=0 SHOWY_QUOTA_RESET_DESCRIPTION_TIMEZONE_OFFSET=0 < "${FIXTURE_DIR}/codexbar-cursor.json" | grep -cE ' [0-9]{2}:[0-9]{2}$')"
# A default line stays inside an SSH session on a phone. Its exact width depends
# on the widest sigil and horizon label in the snapshot (a model-pooled provider
# carries a family tag, so `AGG` costs every row a column); what is fixed is that
# the clock is six of those columns.
render_vertical_widest=$(render_vertical < "${FIXTURE_DIR}/codexbar-realistic.json" | jq -Rrn '[inputs | length] | max')
assert_equals "render vertical CLI keeps a default line inside a phone's width" "ok" "$([[ ${render_vertical_widest} -le 46 ]] && echo ok || echo "too wide: ${render_vertical_widest}")"
render_vertical_clocked=$(printf '%s\n' "${render_vertical_cursor}" | head -1 | jq -Rrn 'input | length')
render_vertical_plain=$(render_vertical SHOWY_QUOTA_VERTICAL_RESET_CLOCK=0 < "${FIXTURE_DIR}/codexbar-cursor.json" | head -1 | jq -Rrn 'input | length')
assert_equals "render vertical CLI spends exactly six columns on the clock" "6" "$((render_vertical_clocked - render_vertical_plain))"

assert_equals "render vertical CLI honours the vertical bar width" "8" "$(render_vertical SHOWY_QUOTA_VERTICAL_BAR_WIDTH=8 < "${FIXTURE_DIR}/codexbar-cursor.json" | head -1 | sed -E 's/.*▕(.*)▏.*/\1/' | jq -Rn 'input | length')"
assert_equals "render vertical CLI renders idle when nothing is renderable" "AI none" "$(render_vertical < "${FIXTURE_DIR}/codexbar-empty.json")"

out=$(run_state codexbar-error-only.json)
assert_equals "state error-only providers stay renderable-only empty" "[]" "$(printf '%s' "${out}" | jq -c '.providers')"
assert_equals "state error-only provider count stays renderable-only zero" "0" "$(printf '%s' "${out}" | jq -r '.providerCount')"
assert_equals "state error-only providerMetrics includes errored providers" "cursor,factory" "$(printf '%s' "${out}" | jq -r '.providerMetrics | map(.provider) | join(",")')"
assert_equals "state cursor error is auth bucket with sanitized message" "auth|No Cursor session found." "$(printf '%s' "${out}" | jq -r '.providerMetrics[] | select(.provider == "cursor") | [.error.kind, .error.message] | join("|")')"
assert_equals "state factory error is cookies bucket with sanitized message" "cookies|Safari cookies not readable." "$(printf '%s' "${out}" | jq -r '.providerMetrics[] | select(.provider == "factory") | [.error.kind, .error.message] | join("|")')"
assert_equals "state errored providers expose null windows and no extras" "true" "$(printf '%s' "${out}" | jq -r 'all(.providerMetrics[]; .windows.primary == null and .windows.secondary == null and .windows.tertiary == null and (.extraRateWindows | length == 0))')"

out=$(run_state codexbar-error-only.json SHOWY_QUOTA_PROVIDERS_EXCLUDE=cursor)
assert_equals "state exclude drops errored providerMetrics" "factory" "$(printf '%s' "${out}" | jq -r '.providerMetrics | map(.provider) | join(",")')"
assert_equals "state exclude keeps renderable providers empty" "[]" "$(printf '%s' "${out}" | jq -c '.providers')"

out=$(run_state "${sanitized_error_fixture}")
assert_equals "state sanitizes and truncates provider error message" "160|true|true" "$(printf '%s' "${out}" | jq -r '.providerMetrics[] | select(.provider == "cursor") | .error.message | [(length | tostring), (contains("\u0001") | not | tostring), (startswith("Tokenbad ") | tostring)] | join("|")')"

# Provider error messages are redacted before export so pasted --diagnose/state
# output cannot leak auth/session material (emails, bearer/token-shaped runs).
redact_error_fixture="${TMP}/codexbar-error-redact.json"
jq -n '[{"provider":"cursor","error":{"code":1,"kind":"provider","message":"auth failed for alice@example.test token 0123456789abcdef0123456789abcdef01234567"}}]' \
    > "${redact_error_fixture}"
redact_out=$(run_state "${redact_error_fixture}" | jq -r '.providerMetrics[] | select(.provider == "cursor") | .error.message')
assert_not_contains "state redacts email in provider error message" "alice@example.test" "${redact_out}"
assert_not_contains "state redacts token in provider error message" "0123456789abcdef0123456789abcdef01234567" "${redact_out}"

out=$(run_state codexbar-antigravity-quad.json SHOWY_QUOTA_NOW_EPOCH=4070908800)
assert_equals "state extraRateWindows exposes known window metrics" "Gemini Session|true|35|65|300|180" "$(printf '%s' "${out}" | jq -r '.providerMetrics[] | select(.provider == "antigravity") | .extraRateWindows[0] | [.title, .usageKnown, .usedPercent, .remainingPercent, .windowMinutes, .minutesUntilReset] | map(tostring) | join("|")')"

out=$(run_state codexbar-antigravity-degraded-family.json SHOWY_QUOTA_NOW_EPOCH=4070908800)
assert_equals "state unknown extraRateWindows keep usage fields null" "Claude + GPT Session|false|true" "$(printf '%s' "${out}" | jq -r '.providerMetrics[] | select(.provider == "antigravity") | .extraRateWindows[] | select(.title == "Claude + GPT Session") | [.title, .usageKnown, ([.usedPercent, .remainingPercent, .resetsAt, .resetDescription, .windowMinutes, .minutesUntilReset] | all(. == null))] | map(tostring) | join("|")')"

state_usage_cache=$(mk_cache)
state_usage_file="${state_usage_cache}/usage-explicit.json"
out=$(run_state_with_usage_file codexbar-mixed.json "${state_usage_file}")
assert_equals "state marks fresh cache not stale" "false" "$(printf '%s' "${out}" | jq -r '.stale')"
assert_equals "state exposes cache age" "true" "$(printf '%s' "${out}" | jq -r '.cacheAgeSeconds | type == "number"')"
assert_equals "state exposes stale-after threshold" "240" "$(printf '%s' "${out}" | jq -r '.staleAfterSeconds')"

state_usage_cache=$(mk_cache)
state_usage_file="${state_usage_cache}/usage-explicit.json"
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${state_usage_file}"
touch -t 198801010000 "${state_usage_file}"
out=$(run_state_with_usage_file codexbar-mixed.json "${state_usage_file}" SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-state" SHOWY_QUOTA_CODEXBAR_SERVE_URL='')
assert_equals "state marks stale cache stale" "true" "$(printf '%s' "${out}" | jq -r '.stale')"

out=$(run_state codexbar-mixed.json SHOWY_QUOTA_SKETCHYBAR_COMPACT_PROVIDER_COUNT=3)
assert_equals "state compact threshold is configurable" "true" "$(printf '%s' "${out}" | jq -r '.sketchybar.compactRecommended')"

out=$(run_state codexbar-mixed.json SHOWY_QUOTA_SKETCHYBAR_COMPACT_PROVIDER_COUNT=03)
assert_equals "state compact threshold accepts leading zeroes" "3" "$(printf '%s' "${out}" | jq -r '.sketchybar.compactProviderThreshold')"
assert_equals "state leading-zero compact threshold drives recommendation" "true" "$(printf '%s' "${out}" | jq -r '.sketchybar.compactRecommended')"

out=$(run_state codexbar-mixed.json SHOWY_QUOTA_PROVIDERS_EXCLUDE=codex)
assert_equals "state honors provider excludes" "claude,gemini" "$(printf '%s' "${out}" | jq -r '.providers | join(",")')"

state_missing_cache=$(mk_cache)
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${state_missing_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-state" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-state"
)
assert_equals "state reports unavailable without cache" "false" "$(printf '%s' "${out}" | jq -r '.available')"
assert_equals "state keeps unavailable provider count empty" "0" "$(printf '%s' "${out}" | jq -r '.providerCount')"
assert_equals "state cache age null when absent" "null" "$(printf '%s' "${out}" | jq -r '.cacheAgeSeconds')"

# ── sketchybar bootstrap (without sketchybar daemon) ────────────────────

printf '\nsketchybar bootstrap\n'

cache=$(mk_cache)
log="${TMP}/sb-items.log"
run_sketchybar_items codexbar-mixed.json "${cache}" "${log}"
item_log="$(< "${log}")"
assert_contains "bootstrap declares trigger item" "showy_quota.trigger drawing=off updates=on" "${item_log}"
assert_contains "bootstrap defaults trigger cadence to zellij interval" "update_freq=10" "${item_log}"
assert_contains "bootstrap synchronously adds provider items" "--add item showy_quota.claude.icon left" "${item_log}"
assert_contains "bootstrap adds native primary slider" "--add slider showy_quota.claude.primary left 80" "${item_log}"
assert_contains "bootstrap adds native marker overlay" "--add slider showy_quota.claude.secondary_marker left 80" "${item_log}"
assert_contains "bootstrap recreates bracket immediately" "--add bracket showy_quota_bracket" "${item_log}"
assert_contains "bootstrap declares stale indicator" "--add item showy_quota.stale left" "${item_log}"
assert_contains "bootstrap declares degraded indicator" "--add item showy_quota.degraded left" "${item_log}"
assert_contains "bootstrap places indicators rightmost in bracket" "showy_quota.cursor.label showy_quota.overflow showy_quota.stale showy_quota.degraded --set showy_quota_bracket" "${item_log}"
assert_contains "bootstrap preserves icon width" "width=22" "${item_log}"
assert_contains "bootstrap preserves native bar slot width" "showy_quota.claude.slot icon.drawing=off" "${item_log}"
assert_contains "bootstrap preserves native bar width" "width=83" "${item_log}"

cache=$(mk_cache)
log="${TMP}/sb-items-pill.log"
run_sketchybar_items codexbar-mixed.json "${cache}" "${log}" SHOWY_QUOTA_SKETCHYBAR_PILL_RADIUS=6 SHOWY_QUOTA_SKETCHYBAR_PILL_HEIGHT=18
item_log="$(< "${log}")"
assert_contains "bootstrap honors prefixed pill radius" "background.corner_radius=6" "${item_log}"
assert_contains "bootstrap honors prefixed pill height" "background.height=18" "${item_log}"

# Bare PILL_RADIUS / PILL_HEIGHT collide with other sketchybarrc components, so
# the bootstrap deliberately does not forward them. The defaults must win.
cache=$(mk_cache)
log="${TMP}/sb-items-pill-bare.log"
run_sketchybar_items codexbar-mixed.json "${cache}" "${log}" PILL_RADIUS=6 PILL_HEIGHT=18
item_log="$(< "${log}")"
assert_contains "bootstrap ignores bare PILL_RADIUS" "background.corner_radius=14" "${item_log}"
assert_contains "bootstrap ignores bare PILL_HEIGHT" "background.height=28" "${item_log}"

cache=$(mk_cache)
seed_sketchybar_state "${cache}" codex claude gemini
log="${TMP}/sb-items-stale.log"
run_sketchybar_items codexbar-mixed.json "${cache}" "${log}"
item_log="$(< "${log}")"
assert_contains "bootstrap ignores stale provider state" "--add item showy_quota.gemini.icon left" "${item_log}"

cache=$(mk_cache)
seed_sketchybar_state "${cache}" codex claude gemini
log="${TMP}/sb-items-empty.log"
run_sketchybar_items codexbar-mixed.json "${cache}" "${log}" SHOWY_QUOTA_PROVIDERS_EXCLUDE='claude,codex,gemini'
item_log="$(< "${log}")"
assert_contains "bootstrap removes stale native provider items when desired set is empty" "--remove showy_quota.gemini.icon --remove showy_quota.gemini.primary --remove showy_quota.gemini.secondary --remove showy_quota.gemini.tertiary" "${item_log}"
assert_contains "bootstrap removes stale native marker items when desired set is empty" "--remove showy_quota.gemini.secondary_marker --remove showy_quota.gemini.tertiary_marker --remove showy_quota.gemini.quaternary_marker --remove showy_quota.gemini.primary_marker --remove showy_quota.gemini.slot --remove showy_quota.gemini.label" "${item_log}"
assert_contains "bootstrap removes stale bracket when desired set is empty" "--remove showy_quota_bracket" "${item_log}"

cache=$(mk_cache)
log="${TMP}/sb-items-click.log"
# shellcheck disable=SC2030,SC2031
(
    PATH="${stub_dir}:${PATH}"
    export SHOWY_QUOTA_NO_CONFIG=1
    export SHOWY_QUOTA_CACHE_DIR="${cache}"
    export SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE="${cache}/sb"
    export SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json"
    export SHOWY_QUOTA_TEST_LOG="${log}"
    SHOWY_QUOTA_SKETCHYBAR_CLICK='custom-click'
    . "${REPO_ROOT}/adapters/sketchybar/items/showy_quota.sh"
)
item_log="$(< "${log}")"
assert_contains "bootstrap exports non-exported click override" "click_script=custom-click" "${item_log}"

# ── sketchybar plugin (without sketchybar daemon) ───────────────────────

printf '\nsketchybar plugin (native sliders)\n'

cache=$(mk_cache)
log="${TMP}/sb.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}"

if [[ -s "${log}" ]]; then ok "sketchybar received --set commands"
else fail "sketchybar received --set commands"; fi
if grep -q 'label.color=0xff' "${log}" 2>/dev/null; then
    ok "label.color is well-formed (0xffRRGGBB)"
else
    fail "label.color is well-formed"
fi
assert_contains "plugin uses countdown label color" "label.color=0xff7b8496" "$(< "${log}")"
if grep -q 'width=83' "${log}" 2>/dev/null; then
    ok "plugin repairs native bar slot width"
else
    fail "plugin repairs native bar slot width"
fi
plugin_log="$(< "${log}")"
assert_contains "plugin keeps stale indicator off on fresh cache" "--set showy_quota.stale drawing=off" "${plugin_log}"
assert_contains "plugin keeps degraded indicator off on fresh cache" "--set showy_quota.degraded drawing=off" "${plugin_log}"
assert_contains "plugin updates native primary row percentage" "--set showy_quota.claude.primary drawing=on slider.percentage=83" "${plugin_log}"
assert_contains "plugin updates native secondary row percentage" "--set showy_quota.claude.secondary drawing=on slider.percentage=81" "${plugin_log}"
assert_contains "plugin hides missing tertiary row" "--set showy_quota.claude.tertiary drawing=off" "${plugin_log}"
assert_contains "plugin pins countdown label width" "label.width=32 label.align=left" "${plugin_log}"
assert_contains "plugin updates native tertiary row when present" "--set showy_quota.gemini.tertiary drawing=on slider.percentage=100" "${plugin_log}"
assert_contains "plugin uses derived secondary row color" "showy_quota.claude.secondary drawing=on slider.percentage=81 slider.highlight_color=0xff14683a" "${plugin_log}"
assert_contains "plugin uses derived tertiary row color" "showy_quota.gemini.tertiary drawing=on slider.percentage=100 slider.highlight_color=0xff25be6a" "${plugin_log}"
assert_contains "plugin uses native track color" "slider.background.color=0xff3a3a4a" "${plugin_log}"
assert_contains "plugin draws elapsed marker overlay" "--set showy_quota.claude.secondary_marker drawing=on slider.percentage=100" "${plugin_log}"
assert_contains "plugin uses elapsed marker color" "slider.knob.background.color=0xffbe95ff" "${plugin_log}"
assert_contains "plugin positions two-row primary above center" "showy_quota.claude.primary drawing=on slider.percentage=83 slider.highlight_color=0xff25be6a slider.background.color=0xff3a3a4a slider.background.height=6 slider.background.corner_radius=3 slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset=4" "${plugin_log}"
assert_contains "plugin positions two-row secondary below center" "showy_quota.claude.secondary drawing=on slider.percentage=81 slider.highlight_color=0xff14683a slider.background.color=0xff3a3a4a slider.background.height=6 slider.background.corner_radius=3 slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset=-4" "${plugin_log}"
assert_contains "plugin positions tertiary below three-row stack" "showy_quota.gemini.tertiary drawing=on slider.percentage=100 slider.highlight_color=0xff25be6a slider.background.color=0xff3a3a4a slider.background.height=6 slider.background.corner_radius=3 slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset=-7" "${plugin_log}"
assert_contains "plugin click reset keeps slider rows stable" "sketchybar --set 'showy_quota.claude.primary' slider.percentage=83" "${plugin_log}"
assert_not_contains "plugin no longer logs provider bar PNGs" "bar-claude.png" "${plugin_log}"
if compgen -G "${cache}/sb/bar-*.png" >/dev/null; then
    fail "plugin no longer writes provider bar PNGs" "$(compgen -G "${cache}/sb/bar-*.png")"
else
    ok "plugin no longer writes provider bar PNGs"
fi

secondary_only_fixture="${TMP}/codexbar-secondary-only-sketchybar.json"
printf '%s\n' \
    '[' \
    '{"provider":"antigravity","usage":{"primary":null,"secondary":{"usedPercent":0},"tertiary":{"usedPercent":25}}}' \
    ']' > "${secondary_only_fixture}"
cache=$(mk_cache)
log="${TMP}/sb-secondary-only.log"
run_sketchybar_plugin "${secondary_only_fixture}" "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_contains "plugin promotes live secondary into the primary row" "--set showy_quota.antigravity.primary drawing=on slider.percentage=100" "${plugin_log}"
assert_contains "plugin promotes tertiary into the middle row" "--set showy_quota.antigravity.secondary drawing=on slider.percentage=75" "${plugin_log}"
assert_contains "plugin leaves the vacated bottom row undrawn" "showy_quota.antigravity.tertiary drawing=off" "${plugin_log}"
assert_contains "plugin labels a promoted no-reset full window idle" "showy_quota.antigravity.label drawing=on label=idle" "${plugin_log}"

# Single live window (Codex once OpenAI drops the 5h limit): one centered
# full-height row, and the vacated secondary row is not drawn (no empty track).
cache=$(mk_cache)
log="${TMP}/sb-single-window.log"
run_sketchybar_plugin codexbar-codex-no-5h.json "${cache}" "${log}" SHOWY_QUOTA_PROVIDERS=codex SHOWY_QUOTA_NOW_EPOCH=4070908800
plugin_log="$(< "${log}")"
assert_contains "plugin centers a single-window primary row" "--set showy_quota.codex.primary drawing=on slider.percentage=100" "${plugin_log}"
assert_contains "plugin gives the single-window row centered geometry" "slider.knob.drawing=off background.color=0x00000000 background.height=0 padding_left=0 padding_right=0 width=0 y_offset=0 click_script" "${plugin_log}"
assert_contains "plugin leaves the single-window secondary row undrawn" "--set showy_quota.codex.secondary drawing=off" "${plugin_log}"

# Model-pooled provider (Antigravity): SketchyBar auto-detects the pools (extras
# carry every positional slot) and shows all four windows as family-grouped rows
# (Gemini 5h/weekly, Claude+GPT 5h/weekly), not the cross-family positional slots.
cache=$(mk_cache)
log="${TMP}/sb-pooled.log"
run_sketchybar_plugin codexbar-antigravity-quad.json "${cache}" "${log}" SHOWY_QUOTA_NOW_EPOCH=4070908800
plugin_log="$(< "${log}")"
assert_contains "plugin pooled provider draws gemini session row" "--set showy_quota.antigravity.primary drawing=on slider.percentage=65" "${plugin_log}"
assert_contains "plugin pooled provider draws gemini weekly row" "--set showy_quota.antigravity.secondary drawing=on slider.percentage=0" "${plugin_log}"
assert_contains "plugin pooled provider draws claude session row" "--set showy_quota.antigravity.tertiary drawing=on slider.percentage=90" "${plugin_log}"
assert_contains "plugin pooled provider draws claude weekly row in fourth lane" "--set showy_quota.antigravity.quaternary drawing=on slider.percentage=18" "${plugin_log}"

# ── sketchybar shared-cycle fourth lane ───────────────────────────────
printf '\nsketchybar shared-cycle fourth lane\n'

shared_fourth_fixture="${TMP}/codexbar-shared-fourth-diff.json"
printf '%s\n' \
    '[' \
    '{"provider":"antigravity","usage":{' \
    '"primary":{"usedPercent":10,"windowMinutes":10080,"resetsAt":"2099-01-08T00:00:00Z"},' \
    '"secondary":{"usedPercent":20,"windowMinutes":10080,"resetsAt":"2099-01-08T00:00:00Z"},' \
    '"tertiary":{"usedPercent":30,"windowMinutes":10080,"resetsAt":"2099-01-08T00:00:00Z"},' \
    '"extraRateWindows":[' \
    '{"title":"One","window":{"usedPercent":10,"windowMinutes":10080,"resetsAt":"2099-01-08T00:00:00Z"}},' \
    '{"title":"Two","window":{"usedPercent":20,"windowMinutes":10080,"resetsAt":"2099-01-08T00:00:00Z"}},' \
    '{"title":"Three","window":{"usedPercent":30,"windowMinutes":10080,"resetsAt":"2099-01-08T00:00:00Z"}},' \
    '{"title":"Four","window":{"usedPercent":20,"windowMinutes":10080,"resetsAt":"2099-01-07T00:00:00Z"}}' \
    ']}}' \
    ']' > "${shared_fourth_fixture}"
shared_fourth_now=4070908800
shared_fourth_reset=$((shared_fourth_now + 6 * 86400))
shared_fourth_marker_x=$(( (shared_fourth_reset - shared_fourth_now) * 80 / (10080 * 60) ))
shared_fourth_marker_pct=$(( (shared_fourth_marker_x * 100 + (80 - 1) / 2) / (80 - 1) ))
cache=$(mk_cache)
log="${TMP}/sb-shared-fourth-diff.log"
run_sketchybar_plugin "${shared_fourth_fixture}" "${cache}" "${log}" SHOWY_QUOTA_NOW_EPOCH="${shared_fourth_now}" SHOWY_QUOTA_REFRESH_SECONDS=9999999999
plugin_log="$(< "${log}")"
assert_contains "plugin shared-cycle checks differing fourth reset for dimming" "showy_quota.antigravity.quaternary drawing=on slider.percentage=80 slider.highlight_color=0xff14683a" "${plugin_log}"
assert_contains "plugin shared-cycle checks differing fourth reset for marker" "showy_quota.antigravity.quaternary_marker drawing=on slider.percentage=${shared_fourth_marker_pct}" "${plugin_log}"

# Regression: when CodexBar transiently marks a pooled family's windows
# `usageKnown:false` (e.g. Antigravity's Claude/GPT pool during a collection
# hiccup), the lanes must stay drawn as empty tracks rather than collapsing —
# otherwise the whole Claude family vanishes and SketchyBar drops to the two
# Gemini bars. Mirrors the Zellij renderer, which keeps the AGᶜ lane.
cache=$(mk_cache)
log="${TMP}/sb-degraded-family.log"
run_sketchybar_plugin codexbar-antigravity-degraded-family.json "${cache}" "${log}" SHOWY_QUOTA_NOW_EPOCH=4070908800
plugin_log="$(< "${log}")"
assert_contains "plugin degraded-family keeps gemini session row" "--set showy_quota.antigravity.primary drawing=on slider.percentage=65" "${plugin_log}"
assert_contains "plugin degraded-family keeps gemini weekly row" "--set showy_quota.antigravity.secondary drawing=on slider.percentage=0" "${plugin_log}"
assert_contains "plugin degraded-family keeps unmeasured claude session lane present" "--set showy_quota.antigravity.tertiary drawing=on slider.percentage=0" "${plugin_log}"
assert_contains "plugin degraded-family keeps unmeasured claude weekly lane present" "--set showy_quota.antigravity.quaternary drawing=on slider.percentage=0" "${plugin_log}"

cache=$(mk_cache)
log="${TMP}/sb-degraded.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}" SHOWY_QUOTA_DEGRADED_CLI=1
plugin_log="$(< "${log}")"
assert_contains "plugin shows degraded CLI indicator" "--set showy_quota.degraded drawing=on label=⚠cli label.color=0xffee5396" "${plugin_log}"
cache=$(mk_cache)
log="${TMP}/sb-degraded-empty.log"
run_sketchybar_plugin codexbar-empty.json "${cache}" "${log}" SHOWY_QUOTA_DEGRADED_CLI=1
plugin_log="$(< "${log}")"
assert_contains "plugin shows degraded CLI indicator when provider set is empty" "--set showy_quota.degraded drawing=on label=⚠cli label.color=0xffee5396" "${plugin_log}"
if [[ -e "${cache}/sb-state/showy_quota.degraded" ]]; then
    ok "plugin keeps degraded indicator item when provider set is empty"
else
    fail "plugin keeps degraded indicator item when provider set is empty"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
seed_usage_source "${cache}" "serve"
race_fetch="${cache}/race-fetch"
cat > "${race_fetch}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--cache-only" ]]; then
    jq -c '.providers // .' "${SHOWY_QUOTA_CACHE_DIR}/usage.json"
    exit 0
fi
jq -c '.source = "cli"' "${SHOWY_QUOTA_CACHE_DIR}/usage.json" > "${SHOWY_QUOTA_CACHE_DIR}/usage.json.race-tmp"
mv -- "${SHOWY_QUOTA_CACHE_DIR}/usage.json.race-tmp" "${SHOWY_QUOTA_CACHE_DIR}/usage.json"
exit 0
EOF
chmod +x "${race_fetch}"
log="${TMP}/sb-source-race.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}" SHOWY_QUOTA_DEGRADED_CLI= SHOWY_QUOTA_FETCH_BIN="${race_fetch}"
plugin_log="$(< "${log}")"
assert_contains "plugin samples degraded source before background refresh" "--set showy_quota.degraded drawing=off" "${plugin_log}"


cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
log="${TMP}/sb-stale.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}" SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-plugin" SHOWY_QUOTA_CODEXBAR_SERVE_URL=''
plugin_log="$(< "${log}")"
assert_contains "plugin shows stale indicator" "--set showy_quota.stale drawing=on label=⚠ label.color=0xffee5396" "${plugin_log}"
assert_contains "plugin keeps degraded indicator off on stale cache" "--set showy_quota.degraded drawing=off" "${plugin_log}"
assert_contains "plugin greys stale provider primary" "--set showy_quota.claude.primary drawing=on slider.percentage=83 slider.highlight_color=0xff6c7086" "${plugin_log}"
assert_contains "plugin greys stale provider label" "label.color=0xff6c7086" "${plugin_log}"
assert_contains "plugin hides stale marker" "--set showy_quota.claude.secondary_marker drawing=off" "${plugin_log}"

cache=$(mk_cache)
log="${TMP}/sb-no-magick.log"
run_sketchybar_plugin_without_magick codexbar-mixed.json "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_contains "plugin updates native bars without magick" "--set showy_quota.claude.primary drawing=on slider.percentage=83" "${plugin_log}"
assert_contains "plugin hides icons when magick is unavailable" "--set showy_quota.claude.icon drawing=off click_script=open -b com.steipete.codexbar" "${plugin_log}"
if compgen -G "${cache}/sb/bar-*.png" >/dev/null; then
    fail "plugin does not rasterize bars without magick" "$(compgen -G "${cache}/sb/bar-*.png")"
else
    ok "plugin does not rasterize bars without magick"
fi

cache=$(mk_cache)
log="${TMP}/sb-font-icons-no-magick.log"
run_sketchybar_plugin_without_magick codexbar-mixed.json "${cache}" "${log}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font
plugin_log="$(< "${log}")"
assert_contains "plugin can draw provider icons from app font without magick" "--set showy_quota.claude.icon drawing=on icon.drawing=on icon=:claude: icon.font=sketchybar-app-font:Regular:14.0" "${plugin_log}"
assert_contains "plugin maps codex provider to app font icon" "showy_quota.codex.icon drawing=on icon.drawing=on icon=:codex:" "${plugin_log}"
assert_contains "plugin maps gemini provider to app font icon" "showy_quota.gemini.icon drawing=on icon.drawing=on icon=:gemini:" "${plugin_log}"
assert_contains "plugin widens font icon item to make a real native bar gap" "showy_quota.claude.icon drawing=on icon.drawing=on icon=:claude: icon.font=sketchybar-app-font:Regular:14.0 icon.color=0xfff2f4f8 icon.align=center icon.width=22 icon.padding_left=0 icon.padding_right=0 label.drawing=off background.image.drawing=off background.color=0x00000000 background.height=0 padding_left=5 padding_right=0 width=24" "${plugin_log}"
assert_not_contains "font icon mode avoids provider PNG cache paths" "icon-v5-" "${plugin_log}"

copilot_fixture="${TMP}/codexbar-copilot.json"
printf '%s\n' '[{"provider":"copilot","usage":{"primary":{"usedPercent":0},"secondary":{"usedPercent":0}}}]' > "${copilot_fixture}"
cache=$(mk_cache)
log="${TMP}/sb-copilot-font-no-magick.log"
run_sketchybar_plugin_without_magick "${copilot_fixture}" "${cache}" "${log}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font
copilot_font_log="$(< "${log}")"
assert_not_contains "font icon mode does not use sketchybar-app-font copilot glyph" "showy_quota.copilot.icon drawing=on icon.drawing=on icon=:copilot:" "${copilot_font_log}"
assert_contains "copilot has no pointer-like font fallback without magick" "--set showy_quota.copilot.icon drawing=off click_script=open -b com.steipete.codexbar" "${copilot_font_log}"


cache=$(mk_cache)
log="${TMP}/sb-font-status-no-magick.log"
run_sketchybar_plugin_without_magick codexbar-status-major.json "${cache}" "${log}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font
font_status_log="$(< "${log}")"
assert_contains "font icon mode colors degraded providers without magick" "showy_quota.codex.icon drawing=on icon.drawing=on icon=:codex: icon.font=sketchybar-app-font:Regular:14.0 icon.color=0xffee5396" "${font_status_log}"
assert_contains "font icon mode preserves degraded provider status click without magick" "click_script=open 'https://status.openai.com/'" "${font_status_log}"
assert_not_contains "font icon mode skips status PNG for mapped provider without magick" "icon-v5-codex-" "${font_status_log}"

# ── sketchybar status URL guard ───────────────────────────────────────
printf '\nsketchybar status URL guard\n'

status_good_fixture="${TMP}/codexbar-status-good-url.json"
printf '%s\n' \
    '[{"provider":"codex","status":{"indicator":"major","url":"https://status.example.com"},"usage":{"primary":{"usedPercent":6,"windowMinutes":300}}}]' \
    > "${status_good_fixture}"
cache=$(mk_cache)
log="${TMP}/sb-status-good-url.log"
run_sketchybar_plugin_without_magick "${status_good_fixture}" "${cache}" "${log}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font SHOWY_QUOTA_SKETCHYBAR_CLICK=true
status_url_log="$(< "${log}")"
assert_contains "status URL guard allows normal https click" "click_script=open 'https://status.example.com'" "${status_url_log}"

status_loopback_fixture="${TMP}/codexbar-status-loopback-url.json"
printf '%s\n' \
    '[{"provider":"codex","status":{"indicator":"major","url":"http://127.0.0.1:8080"},"usage":{"primary":{"usedPercent":6,"windowMinutes":300}}}]' \
    > "${status_loopback_fixture}"
cache=$(mk_cache)
log="${TMP}/sb-status-loopback-url.log"
run_sketchybar_plugin_without_magick "${status_loopback_fixture}" "${cache}" "${log}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font SHOWY_QUOTA_SKETCHYBAR_CLICK=true
status_url_log="$(< "${log}")"
assert_not_contains "status URL guard rejects loopback status click" "click_script=open 'http://127.0.0.1:8080'" "${status_url_log}"
assert_contains "status URL guard falls back for rejected URL" "click_script=true" "${status_url_log}"

# When magick is present, the plugin points MAGICK_CONFIGURE_PATH at the bundled
# restrictive policy (SSRF defense) before any magick invocation.
mp_bin="${TMP}/magick-policy-bin"
mkdir -p "${mp_bin}"
for tool in bash jq readlink dirname mkdir mktemp mv rm rmdir date stat sed tr cat python3; do
    if [[ "${tool}" == "bash" && -x /opt/homebrew/bin/bash ]]; then
        ln -sf /opt/homebrew/bin/bash "${mp_bin}/bash"
    else
        ln -sf "$(command -v "${tool}")" "${mp_bin}/${tool}"
    fi
done
ln -sf "${stub_dir}/codexbar" "${mp_bin}/codexbar"
ln -sf "${stub_dir}/sketchybar" "${mp_bin}/sketchybar"
mp_env_log="${TMP}/magick-env.log"
cat > "${mp_bin}/magick" <<EOF
#!/bin/sh
printf '%s\n' "\${MAGICK_CONFIGURE_PATH:-UNSET}" >> "${mp_env_log}"
for a in "\$@"; do last="\$a"; done
case "\${last}" in
    info:) printf '0 0 0 ' ;;
    PNG32:*) : > "\${last#PNG32:}" 2>/dev/null || true ;;
    /*) : > "\${last}" 2>/dev/null || true ;;
esac
exit 0
EOF
chmod +x "${mp_bin}/magick"
: > "${mp_env_log}"
cache=$(mk_cache)
env \
    PATH="${mp_bin}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE="${cache}/sb" \
    SHOWY_QUOTA_TEST_FIXTURE="$(fixture_path codexbar-mixed.json)" \
    SHOWY_QUOTA_TEST_LOG="${TMP}/sb-magick-policy.log" \
    SHOWY_QUOTA_DEGRADED_CLI=0 \
    SHOWY_QUOTA_TEST_STATE_DIR="${cache}/sb-state" \
    "${REPO_ROOT}/adapters/sketchybar/plugins/showy_quota.sh" >/dev/null 2>&1 || true
if [[ -s "${mp_env_log}" ]] && grep -qF "${REPO_ROOT}/adapters/sketchybar/imagemagick" "${mp_env_log}"; then
    ok "plugin runs magick under the bundled restrictive policy"
else
    fail "plugin runs magick under the bundled restrictive policy" "env log: $(cat "${mp_env_log}" 2>/dev/null)"
fi

if grep -qF '<policy domain="delegate" rights="none" pattern="*" />' "${REPO_ROOT}/adapters/sketchybar/imagemagick/policy.xml"; then
    ok "ImageMagick policy disables external delegates"
else
    fail "ImageMagick policy disables external delegates"
fi

if command -v magick >/dev/null 2>&1; then
    policy_dir="${REPO_ROOT}/adapters/sketchybar/imagemagick"
    policy_listing="$(MAGICK_CONFIGURE_PATH="${policy_dir}" magick -list policy 2>/dev/null || true)"
    if [[ "${policy_listing}" == *"${policy_dir}/policy.xml"* && "${policy_listing}" == *"Policy: Delegate"* && "${policy_listing}" == *"pattern: *"* ]]; then
        ok "ImageMagick loads bundled delegate-deny policy"
    else
        fail "ImageMagick loads bundled delegate-deny policy" "${policy_listing}"
    fi

    cache=$(mk_cache)
    log="${TMP}/sb-status.log"
    run_sketchybar_plugin codexbar-status-major.json "${cache}" "${log}"
    status_icon_path=$(compgen -G "${cache}/sb/icon-v5-codex-*-major.png" | sort | head -n 1 || true)
    if [[ -s "${status_icon_path}" ]]; then
        ok "plugin generates status-tinted icon"
    else
        fail "plugin generates status-tinted icon"
    fi
    status_log="$(< "${log}")"
    assert_contains "plugin uses status-tinted icon" "${status_icon_path}" "${status_log}"
    assert_contains "plugin routes degraded status icon to provider status page" "click_script=open 'https://status.openai.com/'" "${status_log}"


    opencode_fixture="${TMP}/codexbar-opencode.json"
    printf '%s\n' '[{"provider":"opencode","usage":{"primary":{"usedPercent":12,"windowMinutes":300,"resetsAt":"2099-01-01T05:40:00Z"}}}]' > "${opencode_fixture}"
    resource_dir="${TMP}/opencode-resources"
    mkdir -p "${resource_dir}"
    printf '%s\n' '<svg width="100" height="100" viewBox="0 0 100 100" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill-rule="evenodd" clip-rule="evenodd" d="M80 88H20V12H80V88ZM35 27H65V72H35V27Z" fill="#211E1E"/></svg>' > "${resource_dir}/ProviderIcon-opencode.svg"

    policy_svg_png="${TMP}/policy-opencode.png"
    if MAGICK_CONFIGURE_PATH="${policy_dir}" magick -background none -density 300 "MSVG:${resource_dir}/ProviderIcon-opencode.svg" \
            -resize 64x64 "PNG32:${policy_svg_png}" >/dev/null 2>&1 && [[ -s "${policy_svg_png}" ]]; then
        ok "restrictive ImageMagick policy still rasterizes provider SVGs"
    else
        fail "restrictive ImageMagick policy still rasterizes provider SVGs"
    fi

    policy_http_server="${TMP}/policy-http-server.py"
    policy_http_port_file="${TMP}/policy-http-port"
    policy_http_log="${TMP}/policy-http-requests.log"
    cat > "${policy_http_server}" <<'PY'
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file, request_log = sys.argv[1], sys.argv[2]

class Handler(BaseHTTPRequestHandler):
    def _record(self):
        with open(request_log, "a", encoding="utf-8") as fh:
            fh.write(f"{self.command} {self.path}\n")
        self.send_response(404)
        self.end_headers()

    def do_GET(self):
        self._record()

    def do_HEAD(self):
        self._record()

    def log_message(self, fmt, *args):
        pass

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w", encoding="utf-8") as fh:
    fh.write(str(server.server_address[1]))
server.serve_forever()
PY
    : > "${policy_http_log}"
    python3 "${policy_http_server}" "${policy_http_port_file}" "${policy_http_log}" &
    policy_http_pid=$!
    for _ in {1..50}; do
        [[ -s "${policy_http_port_file}" ]] && break
        sleep 0.1
    done
    if [[ ! -s "${policy_http_port_file}" ]]; then
        kill "${policy_http_pid}" 2>/dev/null || true
        wait "${policy_http_pid}" 2>/dev/null || true
        fail "restricted policy test HTTP server starts"
    else
        ok "restricted policy test HTTP server starts"
        policy_http_port="$(< "${policy_http_port_file}")"
        policy_hostile_svg="${TMP}/policy-hostile.svg"
        policy_hostile_png="${TMP}/policy-hostile.png"
        cat > "${policy_hostile_svg}" <<EOF
<svg width="64" height="64" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
  <rect width="64" height="64" fill="#211e1e"/>
  <image width="32" height="32" href="http://127.0.0.1:${policy_http_port}/href.png" xlink:href="http://127.0.0.1:${policy_http_port}/xlink.png"/>
</svg>
EOF
        MAGICK_CONFIGURE_PATH="${policy_dir}" magick -background none -density 300 "MSVG:${policy_hostile_svg}" \
            -resize 64x64 "PNG32:${policy_hostile_png}" >/dev/null 2>&1 || true
        kill "${policy_http_pid}" 2>/dev/null || true
        wait "${policy_http_pid}" 2>/dev/null || true
        if [[ ! -s "${policy_http_log}" ]]; then
            ok "hostile SVG under restrictive ImageMagick policy does not fetch external resources"
        else
            fail "hostile SVG under restrictive ImageMagick policy does not fetch external resources" "$(cat "${policy_http_log}")"
        fi
    fi
    cache=$(mk_cache)
    log="${TMP}/sb-opencode.log"
    run_sketchybar_plugin "${opencode_fixture}" "${cache}" "${log}" SHOWY_QUOTA_CODEXBAR_RESOURCES="${resource_dir}"
    opencode_icon_path=$(compgen -G "${cache}/sb/icon-v5-opencode-*.png" | sort | head -n 1 || true)
    if [[ -s "${opencode_icon_path}" ]]; then
        ok "plugin generates tinted dark icon"
    else
        fail "plugin generates tinted dark icon"
    fi
    opencode_mean=$(magick "${opencode_icon_path}" -background black -alpha remove -format '%[fx:(mean.r+mean.g+mean.b)/3]' info: 2>/dev/null || true)
    if awk -v mean="${opencode_mean:-0}" 'BEGIN { exit !(mean > 0.25) }'; then
        ok "plugin tints near-black monochrome icons to text color"
    else
        fail "plugin tints near-black monochrome icons to text color" "mean=${opencode_mean}"
    fi

    cache=$(mk_cache)
    log="${TMP}/sb-opencode-font-fallback.log"
    run_sketchybar_plugin "${opencode_fixture}" "${cache}" "${log}" SHOWY_QUOTA_CODEXBAR_RESOURCES="${resource_dir}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font
    font_fallback_log="$(< "${log}")"
    assert_contains "font icon mode falls back to SVG for unmapped opencode" "showy_quota.opencode.icon drawing=on icon.drawing=off label.drawing=off background.image=${cache}/sb/icon-v5-opencode-" "${font_fallback_log}"
    assert_not_contains "font icon mode avoids generic code glyph for opencode" "showy_quota.opencode.icon drawing=on icon.drawing=on icon=:code:" "${font_fallback_log}"

    copilot_resource_dir="${TMP}/copilot-resources"
    mkdir -p "${copilot_resource_dir}"
    printf '%s\n' '<svg width="100" height="100" viewBox="0 0 100 100" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M50 10C72 10 90 28 90 50C90 72 72 90 50 90C28 90 10 72 10 50C10 28 28 10 50 10Z" fill="#ffffff"/></svg>' > "${copilot_resource_dir}/ProviderIcon-copilot.svg"
    cache=$(mk_cache)
    log="${TMP}/sb-copilot-svg-fallback.log"
    run_sketchybar_plugin "${copilot_fixture}" "${cache}" "${log}" SHOWY_QUOTA_CODEXBAR_RESOURCES="${copilot_resource_dir}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font
    copilot_svg_log="$(< "${log}")"
    assert_contains "font icon mode falls back to CodexBar SVG for copilot" "showy_quota.copilot.icon drawing=on icon.drawing=off label.drawing=off background.image=${cache}/sb/icon-v5-copilot-" "${copilot_svg_log}"
    assert_not_contains "font icon mode avoids pointer-like copilot app-font glyph" "showy_quota.copilot.icon drawing=on icon.drawing=on icon=:copilot:" "${copilot_svg_log}"

    # ImageMagick's internal MSVG decoder rasterizes stroke-only paths to a
    # fully transparent image and still exits 0, so "magick succeeded" is not
    # proof of a visible icon. Roughly a third of CodexBar's provider SVGs are
    # shaped like this one.
    stroke_fixture="${TMP}/codexbar-stroke-only.json"
    printf '%s\n' '[{"provider":"opencode","usage":{"primary":{"usedPercent":12,"windowMinutes":300,"resetsAt":"2099-01-01T05:40:00Z"}}}]' > "${stroke_fixture}"
    stroke_resource_dir="${TMP}/stroke-only-resources"
    mkdir -p "${stroke_resource_dir}"
    printf '%s\n' '<svg width="100" height="100" viewBox="0 0 100 100" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M38 38H27C20.925 38 16 33.075 16 27C16 20.925 20.925 16 27 16C33.075 16 38 20.925 38 27V73C38 79.075 33.075 84 27 84" stroke="white" stroke-width="9" stroke-linecap="round"/></svg>' > "${stroke_resource_dir}/ProviderIcon-opencode.svg"

    cache=$(mk_cache)
    log="${TMP}/sb-stroke-only.log"
    run_sketchybar_plugin "${stroke_fixture}" "${cache}" "${log}" SHOWY_QUOTA_CODEXBAR_RESOURCES="${stroke_resource_dir}"
    stroke_icon_path=$(compgen -G "${cache}/sb/icon-v5-opencode-*.png" | sort | head -n 1 || true)
    stroke_alpha=$(magick "${stroke_icon_path}" -format '%[fx:maxima.a]' info: 2>/dev/null || true)
    if [[ -s "${stroke_icon_path}" && -n "${stroke_alpha}" && "${stroke_alpha}" != "0" ]]; then
        ok "stroke-only provider SVG never publishes an invisible icon"
    else
        fail "stroke-only provider SVG never publishes an invisible icon" \
            "path=${stroke_icon_path} maxima.a=${stroke_alpha}"
    fi

    # Without librsvg the MSVG path cannot draw the icon at all, so the drawn
    # two-letter sigil must take over rather than leaving an empty slot.
    no_rsvg_dir="${TMP}/no-rsvg-bin"
    mkdir -p "${no_rsvg_dir}"
    printf '%s\n' '#!/bin/sh' 'exit 127' > "${no_rsvg_dir}/rsvg-convert"
    chmod +x "${no_rsvg_dir}/rsvg-convert"
    cache=$(mk_cache)
    log="${TMP}/sb-stroke-only-no-rsvg.log"
    run_sketchybar_plugin "${stroke_fixture}" "${cache}" "${log}" \
        PATH="${no_rsvg_dir}:${stub_dir}:${PATH}" \
        SHOWY_QUOTA_CODEXBAR_RESOURCES="${stroke_resource_dir}"
    sigil_icon_path=$(compgen -G "${cache}/sb/icon-v5-opencode-*.png" | sort | head -n 1 || true)
    sigil_alpha=$(magick "${sigil_icon_path}" -format '%[fx:maxima.a]' info: 2>/dev/null || true)
    if [[ -s "${sigil_icon_path}" && -n "${sigil_alpha}" && "${sigil_alpha}" != "0" ]]; then
        ok "unrenderable provider SVG falls back to the drawn sigil icon"
    else
        fail "unrenderable provider SVG falls back to the drawn sigil icon" \
            "path=${sigil_icon_path} maxima.a=${sigil_alpha}"
    fi
else
    ok "plugin skips ImageMagick icon tests when magick is unavailable"
fi

cache=$(mk_cache)
log="${TMP}/sb-idle.log"
run_sketchybar_plugin codexbar-idle-no-reset.json "${cache}" "${log}"
assert_contains "plugin uses idle label when reset missing at 100%" "label=idle" "$(< "${log}")"

urgent_fixture="${TMP}/codexbar-urgent.json"
printf '%s\n' '[{"provider":"claude","usage":{"primary":{"usedPercent":12,"windowMinutes":300,"resetsAt":"2099-01-01T00:10:00Z"}}}]' > "${urgent_fixture}"
cache=$(mk_cache)
log="${TMP}/sb-urgent.log"
run_sketchybar_plugin "${urgent_fixture}" "${cache}" "${log}" SHOWY_QUOTA_NOW_EPOCH=4070908800
assert_contains "plugin uses bad label color for urgent countdown" "label.color=0xffee5396" "$(< "${log}")"

printf '\nsketchybar plugin (lifecycle diff)\n'

cache=$(mk_cache)
seed_sketchybar_live_items "${cache}" claude codex
seed_sketchybar_state "${cache}" codex claude
log="${TMP}/sb-add.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_contains "plugin adds newly visible provider" "--add item showy_quota.gemini.label left" "${plugin_log}"
# When the desired set changes, the plugin tears down and re-adds every
# provider so SketchyBar lays them out in desired_providers order. The
# previous incremental path appended the new provider at the end, which
# placed late-arriving providers (e.g. antigravity opening mid-session)
# to the right of providers that sort before them.
# The stub's `.order` is SketchyBar's item list after the run.
add_codex_before_gemini=$(grep -n -x -F 'showy_quota.codex.label' "${cache}/sb-state/.order" | head -n1 | cut -d: -f1)
add_gemini_label=$(grep -n -x -F 'showy_quota.gemini.label' "${cache}/sb-state/.order" | head -n1 | cut -d: -f1)
if [[ -n "${add_codex_before_gemini}" && -n "${add_gemini_label}" \
    && "${add_codex_before_gemini}" -lt "${add_gemini_label}" ]]; then
    ok "plugin re-adds declared providers ahead of new providers in sort order"
else
    fail "plugin re-adds declared providers ahead of new providers in sort order" \
        "codex line=${add_codex_before_gemini} gemini line=${add_gemini_label}"
fi
assert_contains "plugin rebuilds bracket with added native provider" "showy_quota.gemini.icon showy_quota.gemini.primary showy_quota.gemini.secondary showy_quota.gemini.tertiary showy_quota.gemini.quaternary showy_quota.gemini.secondary_marker showy_quota.gemini.tertiary_marker showy_quota.gemini.quaternary_marker showy_quota.gemini.primary_marker showy_quota.gemini.slot showy_quota.gemini.label showy_quota.cursor.icon showy_quota.cursor.primary showy_quota.cursor.secondary showy_quota.cursor.tertiary showy_quota.cursor.quaternary showy_quota.cursor.secondary_marker showy_quota.cursor.tertiary_marker showy_quota.cursor.quaternary_marker showy_quota.cursor.primary_marker showy_quota.cursor.slot showy_quota.cursor.label showy_quota.overflow showy_quota.stale showy_quota.degraded --set showy_quota_bracket" "${plugin_log}"
assert_contains "plugin draws the errored provider label" "--set showy_quota.cursor.label drawing=on label=" "${plugin_log}"
# The error-row contract is label-only: every slider stays off, the icon uses
# the warning tint (error rows carry no incident indicator), and the row keeps
# a click action. The label-only assertion above passes with a normal-tint
# icon, so pin the rest here.
assert_contains "plugin keeps the errored primary lane off" "--set showy_quota.cursor.primary drawing=off slider.percentage=0" "${plugin_log}"
assert_contains "plugin keeps the errored secondary lane off" "--set showy_quota.cursor.secondary drawing=off" "${plugin_log}"
assert_contains "plugin keeps the errored tertiary lane off" "--set showy_quota.cursor.tertiary drawing=off" "${plugin_log}"
assert_contains "plugin keeps the errored quaternary lane off" "--set showy_quota.cursor.quaternary drawing=off" "${plugin_log}"
# The lifecycle run uses the default PNG icon path: without an incident the
# fallback glyph renders untinted, so the warning color arrives through the
# generated PNG cache key, not the font-icon color field. The plugin draws a
# PNG only when ImageMagick is installed; without it the icon is off, as in
# the other PNG icon cases guarded above.
if command -v magick >/dev/null 2>&1; then
    assert_contains "plugin tints the errored icon with the warning color" "icon-v5-cursor-" "${plugin_log}"
    assert_contains "plugin keys the errored icon on the error tint" "-error.png" "${plugin_log}"
fi
error_font_cache=$(mk_cache)
seed_usage_cache "${error_font_cache}" codexbar-error-only.json serve
seed_sketchybar_state "${error_font_cache}" cursor factory
seed_sketchybar_live_items "${error_font_cache}" cursor factory
error_font_log_pre="${TMP}/sb-error-font-pre.log"
run_sketchybar_plugin codexbar-error-only.json "${error_font_cache}" "${error_font_log_pre}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font SHOWY_QUOTA_SKETCHYBAR_FORCE_REDECLARE=1
error_font_log="${TMP}/sb-error-font.log"
run_sketchybar_plugin codexbar-error-only.json "${error_font_cache}" "${error_font_log}" SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE=font
# Steady-state runs only refresh labels, never icons: the icon tint lives on
# the redeclare-run log, where the full row is drawn.
error_font_log_content="$(< "${error_font_log_pre}")"
error_warn_argb_check="0xff$(run_common_eval 'showy_quota_palette countdown_warn' SHOWY_QUOTA_NO_CONFIG=1)"
assert_contains "plugin tints the errored font icon with the warning color" "showy_quota.cursor.icon drawing=on icon.drawing=on icon=:cursor: icon.font=sketchybar-app-font:Regular:14.0 icon.color=${error_warn_argb_check}" "${error_font_log_content}"
assert_contains "plugin triggers provider-change event" "--trigger showy_quota_provider_change SHOWY_QUOTA_PROVIDER_COUNT=4 SHOWY_QUOTA_PROVIDERS=codex,claude,gemini,cursor" "${plugin_log}"

cache=$(mk_cache)
seed_sketchybar_state "${cache}" codex claude gemini
log="${TMP}/sb-redeclare.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_contains "plugin redeclares missing live items" "--add item showy_quota.claude.icon left" "${plugin_log}"
assert_contains "plugin redeclares missing bracket when state matches" "--add bracket showy_quota_bracket" "${plugin_log}"

drop_fixture="${TMP}/codexbar-no-gemini.json"
jq '[ .[] | select(.provider != "gemini") ]' "${FIXTURE_DIR}/codexbar-mixed.json" > "${drop_fixture}"
cache=$(mk_cache)
seed_sketchybar_state "${cache}" codex claude gemini
log="${TMP}/sb-remove.log"
run_sketchybar_plugin "${drop_fixture}" "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_contains "plugin removes dropped provider native rows" "--remove showy_quota.gemini.icon --remove showy_quota.gemini.primary --remove showy_quota.gemini.secondary --remove showy_quota.gemini.tertiary" "${plugin_log}"
assert_contains "plugin removes dropped provider native markers" "--remove showy_quota.gemini.secondary_marker --remove showy_quota.gemini.tertiary_marker --remove showy_quota.gemini.quaternary_marker --remove showy_quota.gemini.primary_marker --remove showy_quota.gemini.slot --remove showy_quota.gemini.label" "${plugin_log}"

cache=$(mk_cache)
seed_sketchybar_state "${cache}" codex claude gemini cursor
seed_sketchybar_live_items "${cache}" codex claude gemini cursor
log="${TMP}/sb-unchanged.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_not_contains "plugin unchanged set skips item adds" "--add item showy_quota." "${plugin_log}"
assert_not_contains "plugin unchanged set skips bracket rebuild" "--add bracket showy_quota_bracket" "${plugin_log}"
assert_not_contains "plugin unchanged set skips provider removals" "--remove showy_quota." "${plugin_log}"
assert_not_contains "plugin unchanged set skips bracket removal" "--remove showy_quota_bracket" "${plugin_log}"
assert_contains "plugin unchanged set still updates providers" "--set showy_quota.claude.label" "${plugin_log}"

# Regression: a plugin run that outlives `sketchybar --reload` can re-add
# provider items before the rc re-adds front_app, so the pill drew over the
# front-app icon. Items listed before showy_quota.trigger force a redeclare,
# which re-adds them after it; the same live set in order stays untouched.
for order in misplaced ordered; do
    cache=$(mk_cache)
    seed_sketchybar_state "${cache}" codex claude gemini cursor
    seed_sketchybar_live_items "${cache}" codex claude gemini cursor
    # The live item list holds every seeded item after the trigger; the
    # misplaced case lists codex's icon ahead of front_app and the trigger.
    if [[ "${order}" == misplaced ]]; then
        bar_prefix='["showy_quota.codex.icon","front_app","showy_quota.trigger"]'
    else
        bar_prefix='["front_app","showy_quota.trigger"]'
    fi
    jq -R -s -c --argjson prefix "${bar_prefix}" \
        '{items: ($prefix + ((split("\n") | map(select(length > 0))) - $prefix))}' \
        "${cache}/sb-state/.order" > "${cache}/sb-state/bar"
    log="${TMP}/sb-order-${order}.log"
    run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}"
    plugin_log="$(< "${log}")"
    if [[ "${order}" == misplaced ]]; then
        assert_contains "plugin redeclares items listed before its trigger" \
            "--add item showy_quota.codex.icon left" "${plugin_log}"
    else
        assert_not_contains "plugin keeps items that follow its trigger" \
            "--add item showy_quota." "${plugin_log}"
    fi
done

# Frame diff: the plugin sends only rows SketchyBar does not already show.
frame_cache=$(mk_cache)
run_sketchybar_plugin codexbar-mixed.json "${frame_cache}" "${TMP}/sb-frame-first.log"
log="${TMP}/sb-frame-same.log"
run_sketchybar_plugin codexbar-mixed.json "${frame_cache}" "${log}"
assert_not_contains "plugin sends nothing when every row is unchanged" \
    "--set showy_quota." "$(< "${log}")"

frame_changed_fixture="${TMP}/codexbar-frame-changed.json"
jq 'map(if .provider == "claude" then .usage.primary.usedPercent = 50 else . end)' \
    "${FIXTURE_DIR}/codexbar-mixed.json" > "${frame_changed_fixture}"
seed_usage_cache "${frame_cache}" "${frame_changed_fixture}" serve
log="${TMP}/sb-frame-changed.log"
run_sketchybar_plugin "${frame_changed_fixture}" "${frame_cache}" "${log}"
plugin_log="$(< "${log}")"
assert_contains "plugin re-sends the changed row" \
    "--set showy_quota.claude.primary drawing=on slider.percentage=50" "${plugin_log}"
assert_not_contains "plugin leaves unchanged rows alone" "--set showy_quota.codex." "${plugin_log}"

# A setting outside the rows (here the pacing marker colour) re-sends them all.
log="${TMP}/sb-frame-context.log"
run_sketchybar_plugin "${frame_changed_fixture}" "${frame_cache}" "${log}" \
    SHOWY_QUOTA_PALETTE_ELAPSED=ff0000
plugin_log="$(< "${log}")"
assert_contains "plugin re-sends every row when a row setting changes" \
    "--set showy_quota.codex.label" "${plugin_log}"
assert_contains "plugin applies the changed row setting" \
    "slider.knob.background.color=0xffff0000" "${plugin_log}"

# SketchyBar restarted without the bootstrap: the items are gone although the
# frame file still matches, so the plugin redeclares and re-sends every row.
rm -rf -- "${frame_cache}/sb-state"
log="${TMP}/sb-frame-restart.log"
run_sketchybar_plugin "${frame_changed_fixture}" "${frame_cache}" "${log}" \
    SHOWY_QUOTA_PALETTE_ELAPSED=ff0000
plugin_log="$(< "${log}")"
assert_contains "plugin redeclares items SketchyBar lost" \
    "--add item showy_quota.codex.icon left" "${plugin_log}"
assert_contains "plugin re-sends rows after a redeclare" \
    "--set showy_quota.codex.label" "${plugin_log}"

# Layout events re-plan the notch split only; they never re-send rows. The
# planner needs the two notch anchors, so seed them for this event alone.
for anchor in showy_quota.notch_q showy_quota.notch_e; do
    : > "${frame_cache}/sb-state/${anchor}"
    printf '%s\n' "${anchor}" >> "${frame_cache}/sb-state/.order"
done
log="${TMP}/sb-layout-event.log"
run_sketchybar_plugin "${frame_changed_fixture}" "${frame_cache}" "${log}" \
    SHOWY_QUOTA_PALETTE_ELAPSED=ff0000 SHOWY_QUOTA_SKETCHYBAR_PLACEMENT=notch \
    SENDER=front_app_switched
plugin_log="$(< "${log}")"
assert_contains "layout event re-plans the notch split" \
    "--set /^showy_quota\\.codex\\.[^.]*$/ position=left" "${plugin_log}"
assert_not_contains "layout event sends no rows" "--set showy_quota.codex.label" "${plugin_log}"
for anchor in showy_quota.notch_q showy_quota.notch_e; do
    rm -f "${frame_cache}/sb-state/${anchor}"
    grep -vxF "${anchor}" "${frame_cache}/sb-state/.order" > "${frame_cache}/sb-state/.order.tmp"
    mv "${frame_cache}/sb-state/.order.tmp" "${frame_cache}/sb-state/.order"
done
log="${TMP}/sb-layout-event-left.log"
: > "${log}"
run_sketchybar_plugin "${frame_changed_fixture}" "${frame_cache}" "${log}" \
    SHOWY_QUOTA_PALETTE_ELAPSED=ff0000 SENDER=front_app_switched
assert_equals "layout event does nothing without notch placement" "" "$(< "${log}")"

# A layout event that finds a render in flight must not be lost: it leaves a
# note, and the next render re-plans even though no row changed. The first
# notch render declares the anchors, so the later one has nothing to send;
# it also leaves its own post-redeclare note, which a second render consumes.
run_sketchybar_plugin "${frame_changed_fixture}" "${frame_cache}" "${TMP}/sb-layout-notch.log" \
    SHOWY_QUOTA_PALETTE_ELAPSED=ff0000 SHOWY_QUOTA_SKETCHYBAR_PLACEMENT=notch
if [[ -e "${frame_cache}/sb/layout.pending" ]]; then
    ok "redeclare asks the next tick to re-plan the notch split"
else
    fail "redeclare asks the next tick to re-plan the notch split"
fi
run_sketchybar_plugin "${frame_changed_fixture}" "${frame_cache}" "${TMP}/sb-layout-notch-2.log" \
    SHOWY_QUOTA_PALETTE_ELAPSED=ff0000 SHOWY_QUOTA_SKETCHYBAR_PLACEMENT=notch
if [[ -e "${frame_cache}/sb/layout.pending" ]]; then
    fail "the tick after a redeclare consumes the re-plan note"
else
    ok "the tick after a redeclare consumes the re-plan note"
fi
sleep 30 &
lock_holder=$!
mkdir -p "${frame_cache}/sb/render.lock"
printf '%s\t%s\n' "${lock_holder}" "$(LC_ALL=C ps -p "${lock_holder}" -o lstart=)" \
    > "${frame_cache}/sb/render.lock/owner.pid"
log="${TMP}/sb-layout-busy.log"
: > "${log}"
run_sketchybar_plugin "${frame_changed_fixture}" "${frame_cache}" "${log}" \
    SHOWY_QUOTA_PALETTE_ELAPSED=ff0000 SHOWY_QUOTA_SKETCHYBAR_PLACEMENT=notch \
    SENDER=front_app_switched
assert_equals "layout event skips while a render holds the lock" "" "$(< "${log}")"
kill "${lock_holder}" 2>/dev/null || true
wait "${lock_holder}" 2>/dev/null || true
rm -rf -- "${frame_cache}/sb/render.lock"
if [[ -e "${frame_cache}/sb/layout.pending" ]]; then
    ok "layout event leaves a note for the render in flight"
else
    fail "layout event leaves a note for the render in flight"
fi
log="${TMP}/sb-layout-pending.log"
run_sketchybar_plugin "${frame_changed_fixture}" "${frame_cache}" "${log}" \
    SHOWY_QUOTA_PALETTE_ELAPSED=ff0000 SHOWY_QUOTA_SKETCHYBAR_PLACEMENT=notch
plugin_log="$(< "${log}")"
assert_not_contains "pending-layout render has no row to send" \
    "--set showy_quota.codex.label" "${plugin_log}"
assert_contains "render re-plans a layout the busy event left pending" \
    "--set /^showy_quota\\.codex\\.[^.]*$/ position=left" "${plugin_log}"
if [[ -e "${frame_cache}/sb/layout.pending" ]]; then
    fail "render clears the pending layout note"
else
    ok "render clears the pending layout note"
fi

# Regression: a newly-visible provider that sorts before an existing one
# (the antigravity-opened-mid-session case) must end up to the *left* of
# the later-sorting provider on the bar, not appended at the end.
late_arrival_fixture="${TMP}/codexbar-antigravity-late.json"
jq '. + [{provider:"antigravity",usage:{primary:{usedPercent:10,windowMinutes:300,resetsAt:"2099-01-01T05:40:00Z"}}},
        {provider:"opencodego",  usage:{primary:{usedPercent:20,windowMinutes:300,resetsAt:"2099-01-01T05:40:00Z"}}}]' \
    "${FIXTURE_DIR}/codexbar-mixed.json" > "${late_arrival_fixture}"

cache=$(mk_cache)
# Seed state as if opencodego was already declared (in its existing slot)
# but antigravity is the new arrival.
seed_sketchybar_state      "${cache}" codex claude gemini opencodego
seed_sketchybar_live_items "${cache}" codex claude gemini opencodego
log="${TMP}/sb-late-arrival.log"
run_sketchybar_plugin "${late_arrival_fixture}" "${cache}" "${log}"
plugin_log="$(< "${log}")"

add_antigravity=$(grep -n -x -F 'showy_quota.antigravity.label' "${cache}/sb-state/.order" | head -n1 | cut -d: -f1)
add_opencodego=$(grep -n -x -F 'showy_quota.opencodego.label' "${cache}/sb-state/.order" | head -n1 | cut -d: -f1)
if [[ -n "${add_antigravity}" && -n "${add_opencodego}" \
    && "${add_antigravity}" -lt "${add_opencodego}" ]]; then
    ok "late-arriving provider lands ahead of later-sorting peer"
else
    fail "late-arriving provider lands ahead of later-sorting peer" \
        "antigravity line=${add_antigravity} opencodego line=${add_opencodego}"
fi

cache=$(mk_cache)
log="${TMP}/sb-filter.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}" SHOWY_QUOTA_PROVIDERS_EXCLUDE=codex
plugin_log="$(< "${log}")"
assert_contains "sketchybar exclude-only keeps claude" "showy_quota.claude.label" "${plugin_log}"
assert_not_contains "sketchybar exclude-only drops codex" "showy_quota.codex.label" "${plugin_log}"
assert_contains "sketchybar exclude-only keeps gemini" "showy_quota.gemini.label" "${plugin_log}"

cache=$(mk_cache)
log="${TMP}/sb-filter-overlap.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}" SHOWY_QUOTA_PROVIDERS='claude,codex' SHOWY_QUOTA_PROVIDERS_EXCLUDE=codex
plugin_log="$(< "${log}")"
assert_contains "sketchybar include+exclude keeps claude" "showy_quota.claude.label" "${plugin_log}"
assert_not_contains "sketchybar include+exclude drops codex" "showy_quota.codex.label" "${plugin_log}"
assert_not_contains "sketchybar include+exclude drops gemini" "showy_quota.gemini.label" "${plugin_log}"
# ── schema drift / edge JSON ────────────────────────────────────────

printf '\nschema drift\n'

# 1. Float usedPercent must not crash bash arithmetic.
out=$(run_renderer showy-quota-zellij-bar codexbar-realistic.json)
assert_contains "float usedPercent renders codex"      "CX" "${out}"
assert_contains "float usedPercent renders claude"     "CL" "${out}"
assert_contains "float usedPercent uses GOOD palette" "37;190;106" "${out}"

out=$(run_renderer showy-quota-tmux-bar codexbar-realistic.json)
assert_contains "tmux float usedPercent renders codex" "CX" "${out}"

# 2. Provider with usage.primary but no resetsAt must render '?' not crash.
out=$(run_renderer showy-quota-zellij-bar codexbar-no-reset.json)
assert_contains "no-reset fixture still renders codex" "CX" "${out}"
assert_contains "no-reset fixture shows '?' countdown" "?"  "${out}"

out=$(run_renderer showy-quota-zellij-bar codexbar-reset-description.json)
assert_contains "resetDescription fixture renders codex" "CX" "${out}"
assert_not_contains "resetDescription fixture avoids '?' countdown" "?" "${out}"


out=$(run_renderer showy-quota-zellij-bar codexbar-idle-no-reset.json)
assert_contains "idle-no-reset fixture renders claude" "CL" "${out}"
assert_contains "idle-no-reset fixture shows idle label" "idle" "${out}"

secondary_only_fixture="${TMP}/codexbar-secondary-only.json"
printf '%s\n' \
    '[' \
    '{"provider":"antigravity","usage":{"primary":null,"secondary":{"usedPercent":0},"tertiary":{"usedPercent":25}}}' \
    ']' > "${secondary_only_fixture}"
out=$(run_renderer showy-quota-zellij-bar "${secondary_only_fixture}" SHOWY_QUOTA_PROVIDER_MODES=antigravity=mono3 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 NO_COLOR=1 SHOWY_QUOTA_FORCE_COLOR=0)
assert_contains "secondary-only provider renders in zellij" "AG" "${out}"
assert_contains "secondary-only provider shows idle label" "idle" "${out}"
# Promotion leaves two live windows, so forced mono3 downgrades to a plain dual
# (half-blocks, no sextants) with the promoted secondary on the top row.
assert_contains "secondary-only provider promotes to a plain dual" "AG▕▀▀▀▀▀▀▀▀▏idle" "${out}"
assert_not_contains "secondary-only promotion leaves no sextant top row" "🬹" "${out}"
assert_not_contains "secondary-only promotion leaves no sextant bottom row" "🬋" "${out}"
out=$(run_renderer showy-quota-tmux-bar "${secondary_only_fixture}" SHOWY_QUOTA_PROVIDER_MODES=antigravity=mono3 SHOWY_QUOTA_TMUX_BAR_WIDTH=8 SHOWY_QUOTA_REFRESH_SECONDS=9999999999)
visible=$(strip_tmux_markup "${out}")
assert_contains "secondary-only provider renders in tmux" "AG" "${visible}"
assert_contains "secondary-only provider promotes to a plain dual in tmux" "AG▕▀▀▀▀▀▀▀▀▏" "${visible}"
out=$(run_state "${secondary_only_fixture}")
assert_equals "secondary-only provider is renderable state" "antigravity" "$(printf '%s' "${out}" | jq -r '.providers | join(",")')"
# 3. Non-array JSON must be rejected by the fetcher (refresh path).
printf '\ncache fetcher\n'

cache=$(mk_cache)
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>&1
) || rc=$?
if (( rc != 0 )) && ! [[ -f "${cache}/usage.json" ]]; then
    ok "fetcher rejects non-array JSON"
else
    fail "fetcher rejects non-array JSON" "rc=${rc}; cache exists: $([[ -f ${cache}/usage.json ]] && echo yes)"
fi

# 3b. Fresh but invalid cache must not be emitted as success.
cache=$(mk_cache)
printf '%s\n' '[{"provider":"codex","usage":{"primary":{"usedPercent":"invalid"}}}]' > "${cache}/usage.json"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin:-${TMP}/no-such-codexbar}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
corrupt_files=("${cache}"/usage.json.corrupt.*)
if (( rc != 0 )) && [[ -z "${out}" ]] && [[ ! -e "${cache}/usage.json" ]] && [[ -s "${corrupt_files[0]}" ]]; then
    ok "fetcher rejects and quarantines invalid fresh cache"
else
    fail "fetcher rejects and quarantines invalid fresh cache" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
printf '%s\n' '[{"provider":"codex","usage":{"primary":{}}}]' > "${cache}/usage.json"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    "${REPO_ROOT}/bin/showy-quota-fetch" --stop-serve 2>/dev/null
) || rc=$?
if (( rc == 0 )) && [[ -f "${cache}/usage.json" ]] && [[ -z "${out}" ]]; then
    ok "fetcher stop-serve does not quarantine usage cache"
else
    fail "fetcher stop-serve does not quarantine usage cache" "rc=${rc}; out=${out}; cache_exists=$([[ -f ${cache}/usage.json ]] && echo yes || echo no)"
fi

cache=$(mk_cache)
printf '%s\n' '[{"provider":"codex","usage":{"primary":{"usedPercent":"invalid"}}}]' > "${cache}/usage.json"
race_log="${cache}/race.log"
(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin:-${TMP}/no-such-codexbar}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_DEBUG=1 \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>>"${race_log}" >/dev/null || true
) &
(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin:-${TMP}/no-such-codexbar}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_DEBUG=1 \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>>"${race_log}" >/dev/null || true
) &
wait
corrupt_files=("${cache}"/usage.json.corrupt.*)
if [[ -s "${corrupt_files[0]}" ]] && ! grep -q 'quarantine failed' "${race_log}" 2>/dev/null; then
    ok "fetcher tolerates concurrent corrupt cache quarantine"
else
    race_details=""
    [[ -r "${race_log}" ]] && race_details="$(< "${race_log}")"
    fail "fetcher tolerates concurrent corrupt cache quarantine" "${race_details}"
fi

cache=$(mk_cache)
for idx in 1 2 3 4 5; do
    printf '%s\n' 'invalid' > "${cache}/usage.json.corrupt.000${idx}.${idx}"
done
printf '%s\n' '[{"provider":"codex","usage":{"primary":{"usedPercent":"invalid"}}}]' > "${cache}/usage.json"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin:-${TMP}/no-such-codexbar}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_CORRUPT_CACHE_RETENTION=3 \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
corrupt_files=("${cache}"/usage.json.corrupt.*)
if (( rc != 0 )) && (( ${#corrupt_files[@]} == 3 )); then
    ok "fetcher bounds corrupt cache quarantine retention"
else
    fail "fetcher bounds corrupt cache quarantine retention" "rc=${rc}; corrupt_count=${#corrupt_files[@]}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-realistic.json" "${cache}/usage.json"
cache_only_marker="${cache}/refresh-attempted"
cache_only_bin="${cache}/codexbar-refresh-attempt"
cat > "${cache_only_bin}" <<EOF
#!/usr/bin/env bash
printf x > '${cache_only_marker}'
exit 99
EOF
chmod +x "${cache_only_bin}"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_CODEXBAR_BIN="${cache_only_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-fetch" --cache-only 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1 && [[ ! -e "${cache_only_marker}" ]]; then
    ok "fetcher cache-only emits valid cache without refreshing"
else
    fail "fetcher cache-only emits valid cache without refreshing" "rc=${rc}; out=${out}"
fi


bad_provider="${TMP}/bad-provider.json"
printf '%s\n' '[{"provider":"bad/id","usage":{"primary":{"usedPercent":12}}}]' > "${bad_provider}"
cache=$(mk_cache)
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_TEST_FIXTURE="${bad_provider}" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc != 0 )) && [[ -z "${out}" ]] && ! [[ -f "${cache}/usage.json" ]]; then
    ok "fetcher rejects unsafe provider ids"
else
    fail "fetcher rejects unsafe provider ids" "rc=${rc}; out=${out}"
fi

dotdot_provider="${TMP}/dotdot-provider.json"
printf '%s\n' '[{"provider":"..","usage":{"primary":{"usedPercent":12}}},{"provider":"codex","usage":{"primary":{"usedPercent":30}}}]' > "${dotdot_provider}"
cache=$(mk_cache)
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_TEST_FIXTURE="${dotdot_provider}" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
# '..' is a path component that would escape the failure-stamp dir. A mismatch
# between enabled-record count (2) and valid-id count (1: only codex) is invalid
# inventory: discovery fails and the fetcher refuses to publish, rather than
# silently dropping '..' or emitting a canonical-empty cache.
if (( rc != 0 )) && [[ -z "${out}" ]] && ! [[ -f "${cache}/usage.json" ]] \
    && [[ -r "${cache}/config-providers-failed-at" ]]; then
    ok "fetcher rejects '..' provider id as invalid inventory (no path traversal)"
else
    fail "fetcher rejects '..' provider id as invalid inventory (no path traversal)" "rc=${rc}; out=${out}"
fi

# 4. Missing codexbar binary, no cache → fetcher fails with diagnostic.
missing_bin="${TMP}/no-such-codexbar"
cache=$(mk_cache)
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>&1
) || rc=$?
if (( rc != 0 )); then
    ok "fetcher fails when codexbar missing and cache empty"
else
    fail "fetcher fails when codexbar missing and cache empty"
fi

cache=$(mk_cache)
rc=0
serve_url="http://127.0.0.1:8080"
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc != 0 )) && [[ -z "${out}" ]] && ! [[ -f "${cache}/usage.json" ]]; then
    ok "fetcher empty serve URL disables default HTTP probe"
else
    fail "fetcher empty serve URL disables default HTTP probe" "rc=${rc}; out=${out}"
fi

# 4b. codexbar serve can refresh the cache without invoking the CLI binary.
cache=$(mk_cache)
rc=0
serve_url="http://127.0.0.1:18080"
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_TEST_CURL_MAX_TIME_FILE="${cache}/curl-max-time" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1; then
    ok "fetcher reads codexbar serve usage endpoint"
else
    fail "fetcher reads codexbar serve usage endpoint" "rc=${rc}; out=${out}"
fi
assert_equals "fetcher records serve cache source" "serve" "$(cache_source_value "${cache}")"
assert_equals "fetcher uses production-safe default usage probe timeout" "30" "$(< "${cache}/curl-max-time")"

# A pathological serve timeout is clamped so curl --max-time stays bounded.
clamp_cache=$(mk_cache)
env \
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${clamp_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_TEST_CURL_MAX_TIME_FILE="${clamp_cache}/curl-max-time" \
    SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS=999999999 \
    "${REPO_ROOT}/bin/showy-quota-fetch" >/dev/null 2>&1 || true
assert_equals "fetcher clamps a pathological usage probe timeout" "60" "$(< "${clamp_cache}/curl-max-time")"

zero_timeout_cache=$(mk_cache)
env \
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${zero_timeout_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_TEST_CURL_MAX_TIME_FILE="${zero_timeout_cache}/curl-max-time" \
    SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS=0 \
    "${REPO_ROOT}/bin/showy-quota-fetch" >/dev/null 2>&1 || true
assert_equals "fetcher rejects zero usage probe timeout back to default" "30" "$(< "${zero_timeout_cache}/curl-max-time")"

large_payload_fixture="${TMP}/codexbar-large-payload.json"
python3 -c '
import json
import sys

payload = [
    {
        "provider": "codex",
        "usage": {"primary": {"usedPercent": 12}},
        "pad": "x" * 600,
    },
    {
        "provider": "claude",
        "usage": {"primary": {"usedPercent": 34}},
        "pad": "y" * 601,
    },
]
with open(sys.argv[1], "w", encoding="utf-8") as fh:
    json.dump(payload, fh, separators=(",", ":"))
' "${large_payload_fixture}"

cache=$(mk_cache)
rc=0
serve_url="http://127.0.0.1:18080"
out=$(
    run_with_test_timeout 5 env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
        SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
        SHOWY_QUOTA_TEST_SERVE_FIXTURE="${large_payload_fixture}" \
        "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and (length == 2) and any(.[]; .provider == "codex" and (.pad | length == 600)) and any(.[]; .provider == "claude" and (.pad | length == 601))' >/dev/null 2>&1; then
    ok "fetcher validates large serve payload without hanging"
else
    fail "fetcher validates large serve payload without hanging" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
rc=0
out=$(
    run_with_test_timeout 5 env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_TEST_FIXTURE="${large_payload_fixture}" \
        "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and (length == 2) and any(.[]; .provider == "codex" and (.pad | length == 600)) and any(.[]; .provider == "claude" and (.pad | length == 601))' >/dev/null 2>&1; then
    ok "fetcher validates large provider payload without hanging"
else
    fail "fetcher validates large provider payload without hanging" "rc=${rc}; out=${out}"
fi

managed_bin_dir="${TMP}/managed-bin"
mkdir -p "${managed_bin_dir}"
cat > "${managed_bin_dir}/codexbar" <<'EOF'
#!/usr/bin/env bash
set -eu
if [[ -n "${SHOWY_QUOTA_TEST_MANAGED_ARGS_FILE:-}" ]]; then
    printf '%s\n' "$*" >> "${SHOWY_QUOTA_TEST_MANAGED_ARGS_FILE}"
fi
if [[ "${1:-}" == "serve" ]]; then
    shift
    port=""
    while (($#)); do
        case "$1" in
            --port)
                shift
                port="${1:-}"
                ;;
            --refresh-interval)
                shift
                ;;
        esac
        shift
    done
    [[ -n "${port}" ]] || exit 91

    count=1
    if [[ -n "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE:-}" ]]; then
        count=0
        if [[ -r "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE}" ]]; then
            IFS= read -r count < "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE}" || count=0
        fi
        count=$((count + 1))
        printf '%s\n' "${count}" > "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE}"
    fi

    if [[ "${SHOWY_QUOTA_TEST_MANAGED_MODE:-healthy}" == "restart-once" && "${count}" -eq 1 ]]; then
        while true; do
            sleep 1
        done
    fi

    if [[ "${SHOWY_QUOTA_TEST_MANAGED_MODE:-healthy}" == "health-ok-usage-bad-once" && "${count}" -eq 1 ]]; then
        python3 - "${port}" <<'PY' &
import http.server
import sys

port = int(sys.argv[1])

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"{}")
            return
        if self.path == "/usage":
            body = b"[]"
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.end_headers()

    def log_message(self, _format, *args):
        return

http.server.ThreadingHTTPServer.allow_reuse_address = True
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
        child=$!
        trap 'kill "${child}" 2>/dev/null || true' EXIT TERM INT
        wait "${child}"
        exit $?
    fi

    if [[ "${SHOWY_QUOTA_TEST_MANAGED_MODE:-healthy}" == "health-ok-usage-hang" ]]; then
        python3 - "${port}" <<'PY' &
import http.server
import sys
import time

port = int(sys.argv[1])

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"{}")
            return
        if self.path == "/usage":
            time.sleep(30)
            return
        self.send_response(404)
        self.end_headers()

    def log_message(self, _format, *args):
        return

http.server.ThreadingHTTPServer.allow_reuse_address = True
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
        child=$!
        trap 'kill "${child}" 2>/dev/null || true' EXIT TERM INT
        wait "${child}"
        exit $?
    fi

    python3 - "${port}" "${SHOWY_QUOTA_TEST_SERVE_FIXTURE}" <<'PY' &
import http.server
import sys

port = int(sys.argv[1])
fixture = sys.argv[2]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"{}")
            return
        if self.path == "/usage":
            with open(fixture, "rb") as fh:
                body = fh.read()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.end_headers()

    def log_message(self, _format, *args):
        return

http.server.ThreadingHTTPServer.allow_reuse_address = True
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
    child=$!
    trap 'kill "${child}" 2>/dev/null || true' EXIT TERM INT
    wait "${child}"
    exit $?
fi
if [[ "${1:-}" == "usage" ]]; then
    cat "${SHOWY_QUOTA_TEST_FIXTURE}"
    exit 0
fi
exit 90
EOF
chmod +x "${managed_bin_dir}/codexbar"

cache=$(mk_cache)
managed_port=$(unused_tcp_port)
managed_url="http://127.0.0.1:${managed_port}"
rc=0
out=$(
    PATH="${managed_bin_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_MANAGE_SERVE=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS=019 \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-low.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${cache}" SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" "${REPO_ROOT}/bin/showy-quota-fetch" --stop-serve >/dev/null 2>/dev/null || true
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1 && [[ "$(cache_source_value "${cache}")" == "serve" ]]; then
    ok "fetcher auto-starts managed codexbar serve before CLI fallback"
else
    fail "fetcher auto-starts managed codexbar serve before CLI fallback" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
managed_port=$(unused_tcp_port)
managed_url="http://127.0.0.1:${managed_port}"
managed_args_log="${TMP}/managed-args.log"
rm -f "${managed_args_log}"
rc=0
out=$(
    PATH="${managed_bin_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_MANAGE_SERVE=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS=50 \
    SHOWY_QUOTA_TEST_MANAGED_ARGS_FILE="${managed_args_log}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-low.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${cache}" SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" "${REPO_ROOT}/bin/showy-quota-fetch" --stop-serve >/dev/null 2>/dev/null || true
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1; then
    assert_contains "fetcher derives managed serve port from serve URL" "--port ${managed_port}" "$(< "${managed_args_log}")"
else
    fail "fetcher derives managed serve port from serve URL" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
managed_port=$(unused_tcp_port)
managed_url="http://127.0.0.1:${managed_port}"
managed_count_file="${TMP}/managed-count.log"
rm -f "${managed_count_file}"
rc=0
out=$(
    PATH="${managed_bin_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_MANAGE_SERVE=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS=20 \
    SHOWY_QUOTA_TEST_MANAGED_MODE=restart-once \
    SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE="${managed_count_file}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-low.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${cache}" SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" "${REPO_ROOT}/bin/showy-quota-fetch" --stop-serve >/dev/null 2>/dev/null || true
if (( rc == 0 )) && [[ "$(< "${managed_count_file}")" == "2" ]] && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1; then
    ok "fetcher restarts unhealthy managed serve once"
else
    count_value="missing"
    [[ -r "${managed_count_file}" ]] && count_value=$(< "${managed_count_file}")
    fail "fetcher restarts unhealthy managed serve once" "rc=${rc}; out=${out}; count=${count_value}"
fi

cache=$(mk_cache)
managed_port=$(unused_tcp_port)
managed_url="http://127.0.0.1:${managed_port}"
managed_count_file="${TMP}/managed-usage-bad-count.log"
rm -f "${managed_count_file}"
rc=0
out=$(
    PATH="${managed_bin_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_MANAGE_SERVE=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS=50 \
    SHOWY_QUOTA_CODEXBAR_SERVE_FAILURES_BEFORE_RESTART=1 \
    SHOWY_QUOTA_TEST_MANAGED_MODE=health-ok-usage-bad-once \
    SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE="${managed_count_file}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-low.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${cache}" SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" "${REPO_ROOT}/bin/showy-quota-fetch" --stop-serve >/dev/null 2>/dev/null || true
if (( rc == 0 )) && [[ "$(< "${managed_count_file}")" == "2" ]] && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1 && [[ "$(cache_source_value "${cache}")" == "serve" ]]; then
    ok "fetcher restarts managed serve when health is OK but usage is not publishable"
else
    count_value="missing"
    [[ -r "${managed_count_file}" ]] && count_value=$(< "${managed_count_file}")
    fail "fetcher restarts managed serve when health is OK but usage is not publishable" "rc=${rc}; out=${out}; count=${count_value}"
fi

cache=$(mk_cache)
managed_port=$(unused_tcp_port)
managed_url="http://127.0.0.1:${managed_port}"
managed_count_file="${TMP}/managed-usage-hang-count.log"
rm -f "${managed_count_file}"
rc=0
out=$(
    PATH="${managed_bin_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_MANAGE_SERVE=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS=50 \
    SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS=1 \
    SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS=1 \
    SHOWY_QUOTA_CODEXBAR_SERVE_FAILURES_BEFORE_RESTART=1 \
    SHOWY_QUOTA_PROVIDERS=claude \
    SHOWY_QUOTA_TEST_MANAGED_MODE=health-ok-usage-hang \
    SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE="${managed_count_file}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-low.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${cache}" SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" "${REPO_ROOT}/bin/showy-quota-fetch" --stop-serve >/dev/null 2>/dev/null || true
if [[ "$(< "${managed_count_file}")" == "1" ]] && [[ "$(cache_source_value "${cache}")" == "cli" ]] && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "claude")' >/dev/null 2>&1; then
    ok "fetcher does not restart managed serve when usage times out"
else
    count_value="missing"
    [[ -r "${managed_count_file}" ]] && count_value=$(< "${managed_count_file}")
    fail "fetcher does not restart managed serve when usage times out" "rc=${rc}; out=${out}; count=${count_value}; source=$(cache_source_value "${cache}")"
fi


cache=$(mk_cache)
sleep 30 &
unrelated_pid=$!
unrelated_start=$(pid_start_epoch "${unrelated_pid}") || unrelated_start=0
printf '%s:%s:%s\n' "${unrelated_pid}" "${unrelated_start}" "sleep" > "${cache}/codexbar-serve.pid"
SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" \
    "${REPO_ROOT}/bin/showy-quota-fetch" --stop-serve >/dev/null 2>/dev/null || true
if kill -0 "${unrelated_pid}" 2>/dev/null; then
    ok "fetcher stop-serve ignores unrelated pidfile process"
else
    fail "fetcher stop-serve ignores unrelated pidfile process"
fi
if [[ ! -e "${cache}/codexbar-serve.pid" ]]; then
    ok "fetcher stop-serve removes stale pidfile"
else
    fail "fetcher stop-serve removes stale pidfile" "$(< "${cache}/codexbar-serve.pid")"
fi
kill "${unrelated_pid}" 2>/dev/null || true
wait "${unrelated_pid}" 2>/dev/null || true

cache=$(mk_cache)
bash -c 'exec -a codexbar sleep 30' &
unrelated_codexbar_pid=$!
unrelated_codexbar_start=$(pid_start_epoch "${unrelated_codexbar_pid}") || unrelated_codexbar_start=0
printf '%s:%s:%s\n' "${unrelated_codexbar_pid}" "${unrelated_codexbar_start}" "codexbar" > "${cache}/codexbar-serve.pid"
SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" \
    "${REPO_ROOT}/bin/showy-quota-fetch" --stop-serve >/dev/null 2>/dev/null || true
if kill -0 "${unrelated_codexbar_pid}" 2>/dev/null; then
    ok "fetcher stop-serve ignores non-serve codexbar pidfile process"
else
    fail "fetcher stop-serve ignores non-serve codexbar pidfile process"
fi
if [[ ! -e "${cache}/codexbar-serve.pid" ]]; then
    ok "fetcher stop-serve removes non-serve codexbar pidfile"
else
    fail "fetcher stop-serve removes non-serve codexbar pidfile" "$(< "${cache}/codexbar-serve.pid")"
fi
kill "${unrelated_codexbar_pid}" 2>/dev/null || true
wait "${unrelated_codexbar_pid}" 2>/dev/null || true

# ── codexbar serve build-version gate (showy-quota-serve-stale-gate) ─────
# A serve must be reused only while its /health build matches the installed
# binary; a mismatch (e.g. after a CodexBar update) must recycle it.
version_bin_dir="${TMP}/codexbar-version-bin"
mkdir -p "${version_bin_dir}"
cat > "${version_bin_dir}/codexbar" <<'EOF'
#!/usr/bin/env bash
set -eu
if [[ "${1:-}" == "--version" ]]; then
    printf 'CodexBar %s\n' "${SHOWY_QUOTA_TEST_ONDISK_VERSION:-0.0.0}"
    exit 0
fi
if [[ "${1:-}" == "serve" ]]; then
    shift
    port=""
    while (($#)); do
        case "$1" in
            --port)
                shift
                port="${1:-}"
                ;;
            --refresh-interval)
                shift
                ;;
        esac
        shift
    done
    [[ -n "${port}" ]] || exit 91

    count=1
    if [[ -n "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE:-}" ]]; then
        count=0
        if [[ -r "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE}" ]]; then
            IFS= read -r count < "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE}" || count=0
        fi
        count=$((count + 1))
        printf '%s\n' "${count}" > "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE}"
    fi

    python3 - "${port}" "${SHOWY_QUOTA_TEST_SERVE_FIXTURE}" "${SHOWY_QUOTA_TEST_SERVE_VERSION:-}" <<'PY' &
import http.server
import json
import sys

port = int(sys.argv[1])
fixture = sys.argv[2]
version = sys.argv[3]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            payload = {"status": "ok"}
            if version:
                payload["version"] = version
            body = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path == "/usage":
            with open(fixture, "rb") as fh:
                body = fh.read()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.end_headers()

    def log_message(self, _format, *args):
        return

http.server.ThreadingHTTPServer.allow_reuse_address = True
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
    child=$!
    trap 'kill "${child}" 2>/dev/null || true' EXIT TERM INT
    wait "${child}"
    exit $?
fi
exit 90
EOF
chmod +x "${version_bin_dir}/codexbar"

version_gate_fetch() {
    local _cache="$1" _url="$2" _count="$3" _sver="$4" _over="$5" _manage="$6"
    shift 6
    PATH="${version_bin_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_MANAGE_SERVE="${_manage}" \
        SHOWY_QUOTA_CACHE_DIR="${_cache}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL="${_url}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS=50 \
        SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE="${_count}" \
        SHOWY_QUOTA_TEST_SERVE_VERSION="${_sver}" \
        SHOWY_QUOTA_TEST_ONDISK_VERSION="${_over}" \
        SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
        "${REPO_ROOT}/bin/showy-quota-fetch" "$@"
}

version_gate_stop() {
    SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="$1" \
        SHOWY_QUOTA_CODEXBAR_BIN="${version_bin_dir}/codexbar" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL="$2" \
        "${REPO_ROOT}/bin/showy-quota-fetch" --stop-serve >/dev/null 2>/dev/null || true
}

# Matching build: the healthy serve is reused, never recycled (count stays 1).
cache=$(mk_cache)
managed_port=$(unused_tcp_port)
managed_url="http://127.0.0.1:${managed_port}"
count_file="${TMP}/version-match-count.log"
rm -f "${count_file}"
version_gate_fetch "${cache}" "${managed_url}" "${count_file}" 9.9.9 9.9.9 1 --refresh >/dev/null 2>/dev/null || true
rc=0
out=$(version_gate_fetch "${cache}" "${managed_url}" "${count_file}" 9.9.9 9.9.9 1 --refresh 2>/dev/null) || rc=$?
count_value="missing"; [[ -r "${count_file}" ]] && count_value=$(< "${count_file}")
source_value=$(cache_source_value "${cache}")
version_gate_stop "${cache}" "${managed_url}"
if (( rc == 0 )) && [[ "${count_value}" == "1" ]] && [[ "${source_value}" == "serve" ]] \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1; then
    ok "fetcher reuses healthy serve when build version matches on-disk"
else
    fail "fetcher reuses healthy serve when build version matches on-disk" "rc=${rc}; count=${count_value}; source=${source_value}; out=${out}"
fi

# Stale build: running serve reports an older version than on-disk -> recycle
# and start a fresh serve (count becomes 2).
cache=$(mk_cache)
managed_port=$(unused_tcp_port)
managed_url="http://127.0.0.1:${managed_port}"
count_file="${TMP}/version-mismatch-count.log"
rm -f "${count_file}"
version_gate_fetch "${cache}" "${managed_url}" "${count_file}" 1.0.0 1.0.0 1 --refresh >/dev/null 2>/dev/null || true
rc=0
out=$(version_gate_fetch "${cache}" "${managed_url}" "${count_file}" 2.0.0 2.0.0 1 --refresh 2>/dev/null) || rc=$?
count_value="missing"; [[ -r "${count_file}" ]] && count_value=$(< "${count_file}")
source_value=$(cache_source_value "${cache}")
version_gate_stop "${cache}" "${managed_url}"
if (( rc == 0 )) && [[ "${count_value}" == "2" ]] && [[ "${source_value}" == "serve" ]] \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1; then
    ok "fetcher recycles stale serve when build version differs from on-disk"
else
    fail "fetcher recycles stale serve when build version differs from on-disk" "rc=${rc}; count=${count_value}; source=${source_value}; out=${out}"
fi

# Management disabled: a version mismatch must not recycle (count stays 1); the
# stale serve is still reused rather than killed without a restart path.
cache=$(mk_cache)
managed_port=$(unused_tcp_port)
managed_url="http://127.0.0.1:${managed_port}"
count_file="${TMP}/version-unmanaged-count.log"
rm -f "${count_file}"
version_gate_fetch "${cache}" "${managed_url}" "${count_file}" 1.0.0 1.0.0 1 --refresh >/dev/null 2>/dev/null || true
rc=0
out=$(version_gate_fetch "${cache}" "${managed_url}" "${count_file}" 1.0.0 2.0.0 0 --refresh 2>/dev/null) || rc=$?
count_value="missing"; [[ -r "${count_file}" ]] && count_value=$(< "${count_file}")
source_value=$(cache_source_value "${cache}")
version_gate_stop "${cache}" "${managed_url}"
if (( rc == 0 )) && [[ "${count_value}" == "1" ]] && [[ "${source_value}" == "serve" ]] \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1; then
    ok "fetcher does not recycle stale serve when serve management is disabled"
else
    fail "fetcher does not recycle stale serve when serve management is disabled" "rc=${rc}; count=${count_value}; source=${source_value}; out=${out}"
fi

# Cross-tool normalization: a /health version carrying the "CodexBar " prefix
# must normalize the same as `codexbar --version`, so a current serve is reused,
# not recycled (the divergence flagged by the glean stale-serve detector).
cache=$(mk_cache)
managed_port=$(unused_tcp_port)
managed_url="http://127.0.0.1:${managed_port}"
count_file="${TMP}/version-normalize-count.log"
rm -f "${count_file}"
version_gate_fetch "${cache}" "${managed_url}" "${count_file}" "CodexBar 9.9.9" 9.9.9 1 --refresh >/dev/null 2>/dev/null || true
rc=0
out=$(version_gate_fetch "${cache}" "${managed_url}" "${count_file}" "CodexBar 9.9.9" 9.9.9 1 --refresh 2>/dev/null) || rc=$?
count_value="missing"; [[ -r "${count_file}" ]] && count_value=$(< "${count_file}")
source_value=$(cache_source_value "${cache}")
version_gate_stop "${cache}" "${managed_url}"
if (( rc == 0 )) && [[ "${count_value}" == "1" ]] && [[ "${source_value}" == "serve" ]] \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1; then
    ok "fetcher normalizes prefixed /health version and reuses current serve"
else
    fail "fetcher normalizes prefixed /health version and reuses current serve" "rc=${rc}; count=${count_value}; source=${source_value}; out=${out}"
fi

# Regression: a bare-configured codexbar bin must resolve to an absolute path so
# `--version` and the spawned serve's /health report a version. CodexBar reads
# its version from the app bundle via argv[0], so a bare invocation yields none
# and the gate would be silently inert. This fake mimics that: it reports a
# version only when invoked with a path in argv[0]. The on-disk version moves
# 1.1.1 -> 2.2.2 between fetches, so a working resolver recycles (count == 2);
# a broken one (bare argv[0], no version on either side) would reuse (count 1).
argv0_bin_dir="${TMP}/codexbar-argv0-bin"
mkdir -p "${argv0_bin_dir}"
cat > "${argv0_bin_dir}/codexbar" <<'EOF'
#!/usr/bin/env bash
set -eu
has_path=0
case "$0" in */*) has_path=1 ;; esac
if [[ "${1:-}" == "--version" ]]; then
    if (( has_path )); then
        printf 'CodexBar %s\n' "${SHOWY_QUOTA_TEST_ONDISK_VERSION:-0.0.0}"
    else
        printf 'CodexBar\n'
    fi
    exit 0
fi
if [[ "${1:-}" == "serve" ]]; then
    shift
    port=""
    while (($#)); do
        case "$1" in
            --port)
                shift
                port="${1:-}"
                ;;
            --refresh-interval)
                shift
                ;;
        esac
        shift
    done
    [[ -n "${port}" ]] || exit 91

    count=1
    if [[ -n "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE:-}" ]]; then
        count=0
        if [[ -r "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE}" ]]; then
            IFS= read -r count < "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE}" || count=0
        fi
        count=$((count + 1))
        printf '%s\n' "${count}" > "${SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE}"
    fi

    serve_version=""
    if (( has_path )); then
        serve_version="${SHOWY_QUOTA_TEST_SERVE_VERSION:-}"
    fi

    python3 - "${port}" "${SHOWY_QUOTA_TEST_SERVE_FIXTURE}" "${serve_version}" <<'PY' &
import http.server
import json
import sys

port = int(sys.argv[1])
fixture = sys.argv[2]
version = sys.argv[3]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            payload = {"status": "ok"}
            if version:
                payload["version"] = version
            body = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path == "/usage":
            with open(fixture, "rb") as fh:
                body = fh.read()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.end_headers()

    def log_message(self, _format, *args):
        return

http.server.ThreadingHTTPServer.allow_reuse_address = True
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
    child=$!
    trap 'kill "${child}" 2>/dev/null || true' EXIT TERM INT
    wait "${child}"
    exit $?
fi
exit 90
EOF
chmod +x "${argv0_bin_dir}/codexbar"

argv0_gate_fetch() {
    local _cache="$1" _url="$2" _count="$3" _sver="$4" _over="$5"
    shift 5
    PATH="${argv0_bin_dir}:${PATH}" \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_MANAGE_SERVE=1 \
        SHOWY_QUOTA_CACHE_DIR="${_cache}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL="${_url}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS=50 \
        SHOWY_QUOTA_TEST_MANAGED_COUNT_FILE="${_count}" \
        SHOWY_QUOTA_TEST_SERVE_VERSION="${_sver}" \
        SHOWY_QUOTA_TEST_ONDISK_VERSION="${_over}" \
        SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
        "${REPO_ROOT}/bin/showy-quota-fetch" "$@"
}

cache=$(mk_cache)
managed_port=$(unused_tcp_port)
managed_url="http://127.0.0.1:${managed_port}"
count_file="${TMP}/argv0-gate-count.log"
rm -f "${count_file}"
argv0_gate_fetch "${cache}" "${managed_url}" "${count_file}" 1.1.1 1.1.1 --refresh >/dev/null 2>/dev/null || true
rc=0
out=$(argv0_gate_fetch "${cache}" "${managed_url}" "${count_file}" 2.2.2 2.2.2 --refresh 2>/dev/null) || rc=$?
count_value="missing"; [[ -r "${count_file}" ]] && count_value=$(< "${count_file}")
source_value=$(cache_source_value "${cache}")
SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${cache}" SHOWY_QUOTA_CODEXBAR_BIN="${argv0_bin_dir}/codexbar" SHOWY_QUOTA_CODEXBAR_SERVE_URL="${managed_url}" "${REPO_ROOT}/bin/showy-quota-fetch" --stop-serve >/dev/null 2>/dev/null || true
if (( rc == 0 )) && [[ "${count_value}" == "2" ]] && [[ "${source_value}" == "serve" ]] \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1; then
    ok "fetcher resolves bare codexbar bin to absolute so the version gate engages"
else
    fail "fetcher resolves bare codexbar bin to absolute so the version gate engages" "rc=${rc}; count=${count_value}; source=${source_value}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-low.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "claude") and all(.[]; .provider != "codex")' >/dev/null 2>&1; then
    ok "fetcher refreshes fresh cache from codexbar serve cadence"
else
    fail "fetcher refreshes fresh cache from codexbar serve cadence" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS=08 \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-low.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex") and any(.provider == "gemini")' >/dev/null 2>&1; then
    ok "fetcher treats leading-zero serve cadence as decimal"
else
    fail "fetcher treats leading-zero serve cadence as decimal" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex") and any(.provider == "gemini")' >/dev/null 2>&1; then
    ok "fetcher skips CLI fallback after failed fast serve probe"
else
    fail "fetcher skips CLI fallback after failed fast serve probe" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
seed_usage_source "${cache}" "serve"
python3 - "${cache}/usage.json" <<'PY'
import os
import sys

os.utime(sys.argv[1], (1000, 1000))
PY
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_NOW_EPOCH=1015 \
    SHOWY_QUOTA_REFRESH_SECONDS=10 \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
source_value="missing"
source_value="$(cache_source_value "${cache}")"
if (( rc == 0 )) \
    && [[ "${source_value}" == "serve" ]] \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "cursor")' >/dev/null 2>&1
then
    ok "fetcher preserves serve cache before stale CLI fallback"
else
    fail "fetcher preserves serve cache before stale CLI fallback" "rc=${rc}; source=${source_value}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
seed_usage_source "${cache}" "serve"
python3 - "${cache}/usage.json" <<'PY'
import os
import sys

os.utime(sys.argv[1], (1000, 1000))
PY
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_NOW_EPOCH=1300 \
    SHOWY_QUOTA_REFRESH_SECONDS=10 \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
source_value="missing"
source_value="$(cache_source_value "${cache}")"
failure_count="missing"
[[ -r "${cache}/serve-failed-count" ]] && failure_count="$(< "${cache}/serve-failed-count")"
if (( rc == 0 )) \
    && [[ "${source_value}" == "serve" ]] \
    && [[ "${failure_count}" == "1" ]] \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "cursor")' >/dev/null 2>&1
then
    ok "fetcher preserves stale serve cache before CLI failure threshold"
else
    fail "fetcher preserves stale serve cache before CLI failure threshold" "rc=${rc}; source=${source_value}; failures=${failure_count}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
seed_usage_source "${cache}" "serve"
printf '%s\n' "1299" > "${cache}/serve-failed-at"
printf '%s\n' "1" > "${cache}/serve-failed-count"
python3 - "${cache}/usage.json" <<'PY'
import os
import sys

os.utime(sys.argv[1], (1000, 1000))
PY
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_NOW_EPOCH=1300 \
    SHOWY_QUOTA_REFRESH_SECONDS=10 \
    SHOWY_QUOTA_CODEXBAR_SERVE_FAILURE_BACKOFF_SECONDS=60 \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
source_value="missing"
source_value="$(cache_source_value "${cache}")"
if (( rc == 0 )) \
    && [[ "${source_value}" == "serve" ]] \
    && [[ ! -e "${cache}/serve-failed-count" ]] \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex") and all(.[]; .provider != "cursor")' >/dev/null 2>&1
then
    ok "fetcher bypasses serve failure backoff for stale serve cache"
else
    fail "fetcher bypasses serve failure backoff for stale serve cache" "rc=${rc}; source=${source_value}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
seed_usage_source "${cache}" "serve"
printf '%s\n' "2" > "${cache}/serve-failed-count"
python3 - "${cache}/usage.json" <<'PY'
import os
import sys

os.utime(sys.argv[1], (1000, 1000))
PY
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_NOW_EPOCH=1300 \
    SHOWY_QUOTA_REFRESH_SECONDS=10 \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
source_value="missing"
source_value="$(cache_source_value "${cache}")"
failure_count="missing"
[[ -r "${cache}/serve-failed-count" ]] && failure_count="$(< "${cache}/serve-failed-count")"
if (( rc == 0 )) \
    && [[ "${source_value}" == "cli" ]] \
    && [[ "${failure_count}" == "3" ]] \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex") and all(.[]; .provider != "cursor")' >/dev/null 2>&1
then
    ok "fetcher falls back to CLI after repeated serve usage failures"
else
    fail "fetcher falls back to CLI after repeated serve usage failures" "rc=${rc}; source=${source_value}; failures=${failure_count}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
seed_usage_source "${cache}" "serve"
printf '%s\n' "2" > "${cache}/serve-failed-count"
python3 - "${cache}/usage.json" <<'PY'
import os
import sys

os.utime(sys.argv[1], (1000, 1000))
PY
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_NOW_EPOCH=1300 \
    SHOWY_QUOTA_REFRESH_SECONDS=10 \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="http://127.0.0.1:18081" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
source_value="missing"
source_value="$(cache_source_value "${cache}")"
failure_count="missing"
[[ -r "${cache}/serve-failed-count" ]] && failure_count="$(< "${cache}/serve-failed-count")"
if (( rc == 0 )) \
    && [[ "${source_value}" == "cli" ]] \
    && [[ "${failure_count}" == "3" ]] \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex") and all(.[]; .provider != "cursor")' >/dev/null 2>&1
then
    ok "fetcher falls back to CLI after repeated unavailable serve failures"
else
    fail "fetcher falls back to CLI after repeated unavailable serve failures" "rc=${rc}; source=${source_value}; failures=${failure_count}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && [[ -r "${cache}/serve-failed-at" ]] && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex") and any(.provider == "gemini")' >/dev/null 2>&1; then
    ok "fetcher records failed fast serve probe backoff"
else
    fail "fetcher records failed fast serve probe backoff" "rc=${rc}; out=${out}"
fi
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-low.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex") and any(.provider == "gemini") and any(.provider == "cursor")' >/dev/null 2>&1; then
    ok "fetcher backs off repeated fast serve probes"
else
    fail "fetcher backs off repeated fast serve probes" "rc=${rc}; out=${out}"
fi
cp "${FIXTURE_DIR}/codexbar-low.json" "${cache}/usage.json"
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-low.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-low.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" --refresh 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and length == 1 and .[0].provider == "claude"' >/dev/null 2>&1; then
    ok "fetcher forced refresh bypasses serve backoff"
else
    fail "fetcher forced refresh bypasses serve backoff" "rc=${rc}; out=${out}"
fi


# 4c. A non-CodexBar service on the default port must not block CLI fallback.
cache=$(mk_cache)
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "claude")' >/dev/null 2>&1; then
    ok "fetcher falls back when serve returns non-array JSON"
else
    fail "fetcher falls back when serve returns non-array JSON" "rc=${rc}; out=${out}"
fi
assert_equals "fetcher records CLI degraded cache source" "cli" "$(cache_source_value "${cache}")"

cache=$(mk_cache)
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-empty.json" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1; then
    ok "fetcher falls back when serve returns no renderable providers"
else
    fail "fetcher falls back when serve returns no renderable providers" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${bad_provider}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "claude")' >/dev/null 2>&1; then
    ok "fetcher falls back when serve fails publish validation"
else
    fail "fetcher falls back when serve fails publish validation" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
rc=0
userinfo_url="http://127.0.0.1:18080@example.com"
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${userinfo_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${userinfo_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc != 0 )) && [[ -z "${out}" ]] && ! [[ -f "${cache}/usage.json" ]]; then
    ok "fetcher rejects serve URL userinfo host spoofing"
else
    fail "fetcher rejects serve URL userinfo host spoofing" "rc=${rc}; out=${out}"
fi

# 5. Missing codexbar binary, but stale cache exists → serve stale.
cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${missing_bin}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array"' >/dev/null 2>&1; then
    ok "fetcher serves stale cache when codexbar disappears"
else
    fail "fetcher serves stale cache when codexbar disappears" "rc=${rc}"
fi


timeout_cli_dir="${TMP}/timeout-cli"
mkdir -p "${timeout_cli_dir}"
cat > "${timeout_cli_dir}/codexbar" <<'EOF'
#!/usr/bin/env bash
set -eu
printf 'x' >> "${SHOWY_QUOTA_TEST_COUNTER}"
sleep 5
cat "${SHOWY_QUOTA_TEST_FIXTURE}"
EOF
chmod +x "${timeout_cli_dir}/codexbar"
cat > "${timeout_cli_dir}/timeout" <<'EOF'
#!/usr/bin/env bash
set -eu
seconds="$1"
shift
printf '%s\n' "${seconds}" > "${SHOWY_QUOTA_TEST_TIMEOUT_ARGS_FILE}"
"$@" &
pid=$!
sleep 0.1
if kill -0 "${pid}" 2>/dev/null; then
    kill "${pid}" 2>/dev/null || true
    wait "${pid}" 2>/dev/null || true
    exit 124
fi
wait "${pid}"
EOF
chmod +x "${timeout_cli_dir}/timeout"

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-low.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
expected_stale=$(< "${cache}/usage.json")
timeout_counter="${cache}/timeout-cli-call-count"
timeout_args="${cache}/timeout-cli-seconds"
: > "${timeout_counter}"
rc=0
out=$(
    PATH="${timeout_cli_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${timeout_cli_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_CODEXBAR_CLI_TIMEOUT_SECONDS=1 \
    SHOWY_QUOTA_CODEXBAR_CONFIG_PROVIDERS_TIMEOUT_SECONDS=1 \
    SHOWY_QUOTA_TEST_COUNTER="${timeout_counter}" \
    SHOWY_QUOTA_TEST_TIMEOUT_ARGS_FILE="${timeout_args}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && [[ "${out}" == "${expected_stale}" ]] && [[ -s "${timeout_counter}" ]] && [[ -r "${cache}/cli-failed-at" ]]; then
    ok "fetcher bounds hanging CLI fallback and emits stale cache"
else
    timeout_value="missing"
    [[ -r "${timeout_args}" ]] && timeout_value=$(< "${timeout_args}")
    fail "fetcher bounds hanging CLI fallback and emits stale cache" "rc=${rc}; out=${out}; timeout=${timeout_value}"
fi
calls_before=$(< "${timeout_counter}")
rc=0
out=$(
    PATH="${timeout_cli_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${timeout_cli_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_CODEXBAR_CLI_TIMEOUT_SECONDS=1 \
    SHOWY_QUOTA_CODEXBAR_CONFIG_PROVIDERS_TIMEOUT_SECONDS=1 \
    SHOWY_QUOTA_TEST_COUNTER="${timeout_counter}" \
    SHOWY_QUOTA_TEST_TIMEOUT_ARGS_FILE="${timeout_args}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
calls_after=$(< "${timeout_counter}")
if (( rc == 0 )) && [[ "${out}" == "${expected_stale}" ]] && [[ "${calls_after}" == "${calls_before}" ]]; then
    ok "fetcher backs off repeated hanging CLI fallback"
else
    fail "fetcher backs off repeated hanging CLI fallback" "rc=${rc}; before=${#calls_before}; after=${#calls_after}; out=${out}"
fi

malformed_cli_dir="${TMP}/malformed-cli"
mkdir -p "${malformed_cli_dir}"
cat > "${malformed_cli_dir}/codexbar" <<'EOF'
#!/usr/bin/env bash
set -eu
printf 'x' >> "${SHOWY_QUOTA_TEST_COUNTER}"
cat "${SHOWY_QUOTA_TEST_FIXTURE}"
EOF
chmod +x "${malformed_cli_dir}/codexbar"
cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-low.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
expected_stale=$(< "${cache}/usage.json")
malformed_counter="${cache}/malformed-cli-call-count"
: > "${malformed_counter}"
rc=0
out=$(
    PATH="${malformed_cli_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${malformed_cli_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_COUNTER="${malformed_counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
malformed_calls_before=$(< "${malformed_counter}")
if (( rc == 0 )) && [[ "${out}" == "${expected_stale}" ]] && [[ -s "${malformed_counter}" ]] && [[ -r "${cache}/cli-failed-at" ]]; then
    ok "fetcher records CLI backoff for unusable zero-exit output"
else
    fail "fetcher records CLI backoff for unusable zero-exit output" "rc=${rc}; calls=${#malformed_calls_before}; out=${out}"
fi
rc=0
out=$(
    PATH="${malformed_cli_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${malformed_cli_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_COUNTER="${malformed_counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
malformed_calls_after=$(< "${malformed_counter}")
if (( rc == 0 )) && [[ "${out}" == "${expected_stale}" ]] && [[ "${malformed_calls_after}" == "${malformed_calls_before}" ]]; then
    ok "fetcher backs off repeated unusable zero-exit CLI output"
else
    fail "fetcher backs off repeated unusable zero-exit CLI output" "rc=${rc}; before=${#malformed_calls_before}; after=${#malformed_calls_after}; out=${out}"
fi


# 5b. Provider-aware fallback: discovery, isolation, and last-known-good
#     preservation when every provider call fails.
printf '\nprovider-aware fallback\n'

# Provider discovery: only enabled providers are queried.
provider_aware_dir="${TMP}/provider-aware"
mkdir -p "${provider_aware_dir}"
cat > "${provider_aware_dir}/codexbar" <<EOF
#!/bin/sh
# Provider inventory: instant, separately counted.
counter="\${SHOWY_QUOTA_TEST_COUNTER:-}"
config_counter="\${SHOWY_QUOTA_TEST_CONFIG_COUNTER:-}"
if [ "\${1:-}" = "config" ] && [ "\${2:-}" = "providers" ]; then
    [ -n "\${config_counter}" ] && printf 'x' >> "\${config_counter}"
    disabled="\${SHOWY_QUOTA_TEST_DISABLE_PROVIDER:-}"
    jq --arg disabled "\${disabled}" \
        '[.[] | {provider, enabled: (.provider != \$disabled)}]' \
        < "\${SHOWY_QUOTA_TEST_PROVIDERS_FIXTURE:-${FIXTURE_DIR}/codexbar-mixed.json}"
    exit 0
fi
# Per-provider usage: counted once per invocation. Optional per-provider hang
# (sleep) and per-provider failure (non-zero exit).
[ -n "\${counter}" ] && printf 'x' >> "\${counter}"
provider=""
while [ "\$#" -gt 0 ]; do
    case "\$1" in
        --provider) shift; provider="\${1:-}" ;;
    esac
    shift
done
if [ -n "\${provider}" ] && [ "\${provider}" = "\${SHOWY_QUOTA_TEST_HANG_PROVIDER:-}" ]; then
    sleep 5
    exit 1
fi
if [ -n "\${provider}" ] && [ "\${provider}" = "\${SHOWY_QUOTA_TEST_FAIL_PROVIDER:-}" ]; then
    exit 7
fi
jq --arg p "\${provider}" '[.[] | select(.provider == \$p)]' \
    < "\${SHOWY_QUOTA_TEST_FIXTURE:-${FIXTURE_DIR}/codexbar-mixed.json}"
EOF
chmod +x "${provider_aware_dir}/codexbar"

# Discovery: per-provider invocations restricted to enabled providers from
# `codexbar config providers`. With "cursor" disabled, the fetcher must not
# attempt a per-provider call for it.
cache=$(mk_cache)
usage_counter="${cache}/per-provider-call-count"
: > "${usage_counter}"
config_counter="${cache}/config-providers-call-count"
: > "${config_counter}"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${provider_aware_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_TEST_COUNTER="${usage_counter}" \
    SHOWY_QUOTA_TEST_CONFIG_COUNTER="${config_counter}" \
    SHOWY_QUOTA_TEST_DISABLE_PROVIDER=cursor \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
config_calls=$(wc -c < "${config_counter}" | tr -d ' ')
usage_calls=$(wc -c < "${usage_counter}" | tr -d ' ')
if (( rc == 0 )) \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "claude") and any(.provider == "codex") and any(.provider == "gemini") and (all(.[]; .provider != "cursor"))' >/dev/null 2>&1 \
    && (( config_calls == 1 )) \
    && (( usage_calls == 3 )); then
    ok "fetcher per-provider CLI fallback honors codexbar config discovery"
else
    fail "fetcher per-provider CLI fallback honors codexbar config discovery" "rc=${rc}; config=${config_calls}; usage=${usage_calls}; out=${out:0:80}"
fi

# SHOWY_QUOTA_PROVIDERS is an ordered allow-list for fallback queries. It
# should take precedence over the display-only provider order when set.
cache=$(mk_cache)
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${provider_aware_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_PROVIDER_ORDER='codex,claude,gemini' \
    SHOWY_QUOTA_PROVIDERS='gemini,claude' \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) \
    && [[ "$(printf '%s' "${out}" | jq -r '[.[].provider] | join(",")')" == "gemini,claude" ]]; then
    ok "fetcher honors SHOWY_QUOTA_PROVIDERS order during fallback"
else
    fail "fetcher honors SHOWY_QUOTA_PROVIDERS order during fallback" "rc=${rc}; out=${out:0:120}"
fi

# Provider isolation: one hanging provider must not prevent the others from
# publishing. We bound the hang with the per-CLI hard timeout; the surviving
# providers still merge into a publishable payload.
cache=$(mk_cache)
usage_counter="${cache}/isolation-call-count"
: > "${usage_counter}"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${provider_aware_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_CODEXBAR_CLI_TIMEOUT_SECONDS=1 \
    SHOWY_QUOTA_TEST_COUNTER="${usage_counter}" \
    SHOWY_QUOTA_TEST_HANG_PROVIDER=codex \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "claude") and any(.provider == "gemini") and (all(.[]; .provider != "codex"))' >/dev/null 2>&1 \
    && [[ -r "${cache}/provider-failures/codex" ]]; then
    ok "fetcher isolates one hanging provider from surviving providers"
else
    fail "fetcher isolates one hanging provider from surviving providers" "rc=${rc}; out=${out:0:120}"
fi
# The failure stamp must name the reason, not just the time. A hung provider
# is killed by the hard timeout, so its stamp records rc=124: that is what
# separates "CodexBar hung" from "CodexBar refused fast" in a post-mortem of a
# background refresh, whose stderr the SketchyBar plugin discards.
stamp_body=$(cat "${cache}/provider-failures/codex" 2>/dev/null)
stamp_epoch=$(printf '%s\n' "${stamp_body}" | sed -n '1p')
if [[ "${stamp_epoch}" =~ ^[0-9]+$ ]]; then
    ok "failure stamp keeps a bare epoch on its first line"
else
    fail "failure stamp keeps a bare epoch on its first line" "body=${stamp_body}"
fi
assert_contains "failure stamp records the timeout exit code" "rc=124" "${stamp_body}"

# SHOWY_QUOTA_LOG_FILE is the only way a background cycle can explain itself:
# `start_background_refresh` runs the fetcher with stderr on /dev/null, so the
# DEBUG stderr path cannot be observed there.
log_sink_cache=$(mk_cache)
log_sink_file="${TMP}/log-sink.log"
log_sink_counter="${log_sink_cache}/log-sink-count"
: > "${log_sink_counter}"
rc=0
(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${log_sink_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${provider_aware_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_CODEXBAR_CLI_TIMEOUT_SECONDS=1 \
    SHOWY_QUOTA_LOG_FILE="${log_sink_file}" \
    SHOWY_QUOTA_TEST_COUNTER="${log_sink_counter}" \
    SHOWY_QUOTA_TEST_HANG_PROVIDER=codex \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" >/dev/null 2>/dev/null
) || rc=$?
log_sink_body=$(cat "${log_sink_file}" 2>/dev/null)
assert_contains "log sink records the failing provider and its exit code" \
    "codexbar usage --provider codex failed (rc=124)" "${log_sink_body}"
if printf '%s\n' "${log_sink_body}" | sed -n '1p' | grep -q '^[0-9][0-9]* \[showy-quota:[0-9][0-9]*\] '; then
    ok "log sink stamps each line with an epoch and a pid"
else
    fail "log sink stamps each line with an epoch and a pid" "first=$(printf '%s\n' "${log_sink_body}" | sed -n '1p')"
fi

# The knob is opt-in: a refresh with SHOWY_QUOTA_LOG_FILE unset must not
# append to the sink an earlier run created.
no_sink_cache=$(mk_cache)
no_sink_counter="${no_sink_cache}/no-sink-count"
: > "${no_sink_counter}"
log_sink_size_before=$(wc -c < "${log_sink_file}" | tr -d ' ')
(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${no_sink_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${provider_aware_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_CODEXBAR_CLI_TIMEOUT_SECONDS=1 \
    SHOWY_QUOTA_TEST_COUNTER="${no_sink_counter}" \
    SHOWY_QUOTA_TEST_HANG_PROVIDER=codex \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" >/dev/null 2>/dev/null
) || true
log_sink_size_after=$(wc -c < "${log_sink_file}" | tr -d ' ')
assert_equals "log sink stays opt-in when SHOWY_QUOTA_LOG_FILE is unset" \
    "${log_sink_size_before}" "${log_sink_size_after}"


# Provider-level backoff: once a provider has a fresh failure stamp, a
# subsequent refresh skips it without bumping the per-provider counter. We
# pin the backoff seconds explicitly so this is independent of REFRESH_SECONDS.
: > "${usage_counter}"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${provider_aware_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_CODEXBAR_CLI_TIMEOUT_SECONDS=1 \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_PROVIDER_FAILURE_BACKOFF_SECONDS=3600 \
    SHOWY_QUOTA_TEST_COUNTER="${usage_counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
# Expect 3 calls: claude, gemini, cursor; codex is in backoff and skipped.
backoff_calls=$(wc -c < "${usage_counter}" | tr -d ' ')
if (( rc == 0 )) \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "claude") and any(.provider == "gemini") and (all(.[]; .provider != "codex"))' >/dev/null 2>&1 \
    && (( backoff_calls == 3 )); then
    ok "fetcher honors per-provider backoff on subsequent refresh"
else
    fail "fetcher honors per-provider backoff on subsequent refresh" "rc=${rc}; calls=${backoff_calls}; out=${out:0:160}"
fi

# No successful providers + existing cache → preserve last-known-good rather
# than publishing an empty array. Every per-provider call returns an
# unparseable error, but the previous valid cache must still be emitted.
cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
expected_cache=$(< "${cache}/usage.json")
touch -t 198801010000 "${cache}/usage.json"
usage_counter="${cache}/all-fail-call-count"
: > "${usage_counter}"
failing_dir="${TMP}/all-fail"
mkdir -p "${failing_dir}"
cat > "${failing_dir}/codexbar" <<EOF
#!/bin/sh
counter="\${SHOWY_QUOTA_TEST_COUNTER:-}"
if [ "\${1:-}" = "config" ] && [ "\${2:-}" = "providers" ]; then
    cat "\${SHOWY_QUOTA_TEST_FIXTURE:-${FIXTURE_DIR}/codexbar-mixed.json}" \
        | jq '[.[] | {provider, enabled: true}]'
    exit 0
fi
[ -n "\${counter}" ] && printf 'x' >> "\${counter}"
# Every per-provider call exits non-zero so no provider succeeds.
exit 7
EOF
chmod +x "${failing_dir}/codexbar"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${failing_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_COUNTER="${usage_counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) \
    && [[ "${out}" == "${expected_cache}" ]] \
    && [[ -r "${cache}/cli-failed-at" ]]; then
    ok "fetcher preserves stale cache when every provider fallback fails"
else
    fail "fetcher preserves stale cache when every provider fallback fails" "rc=${rc}; cache_unchanged=$([[ \"${out}\" == \"${expected_cache}\" ]] && echo yes || echo no)"
fi

# A fresh error-only record keeps the previous cache's usage beside the new
# error (with its original providerMeta time), and a later success drops the
# preserved slice. Exercises `merge_error_records_with_last_known_usage`.
error_usage_dir="${TMP}/error-keeps-usage"
mkdir -p "${error_usage_dir}"
cat > "${error_usage_dir}/codexbar" <<EOF
#!/bin/sh
if [ "\${1:-}" = "config" ] && [ "\${2:-}" = "providers" ]; then
    printf '[{"provider":"claude","enabled":true},{"provider":"codex","enabled":true}]'
    exit 0
fi
provider=""
while [ "\$#" -gt 0 ]; do
    case "\$1" in
        --provider) shift; provider="\${1:-}" ;;
    esac
    shift
done
case "\${SHOWY_QUOTA_TEST_SCENE:-ok}:\${provider}" in
    claude-error:claude)
        printf '[{"provider":"claude","error":{"message":"Not signed in"}}]'
        ;;
    claude-fresh:claude)
        printf '[{"provider":"claude","usage":{"primary":{"usedPercent":55,"resetsAt":"2026-09-30T01:29:34Z","windowMinutes":300}}}]'
        ;;
    *)
        jq --arg p "\${provider}" '[.[] | select(.provider == \$p)]' \
            < "${FIXTURE_DIR}/codexbar-mixed.json"
        ;;
esac
EOF
chmod +x "${error_usage_dir}/codexbar"
error_usage_cache=$(mk_cache)
fetch_error_usage() {
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${error_usage_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${error_usage_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_PROVIDERS='claude,codex' \
    SHOWY_QUOTA_PROVIDER_FAILURE_BACKOFF_SECONDS=0 \
    SHOWY_QUOTA_TEST_SCENE="$1" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
}
rc=0
fetch_error_usage ok > "${TMP}/error-usage-1.json" || rc=$?
meta_before=$(jq -r '.providerMeta.claude.updatedAt' "${error_usage_cache}/usage.json" 2>/dev/null)
rc=0
fetch_error_usage claude-error > "${TMP}/error-usage-2.json" || rc=$?
out_error=$(< "${TMP}/error-usage-2.json")
meta_during=$(jq -r '.providerMeta.claude.updatedAt' "${error_usage_cache}/usage.json" 2>/dev/null)
rc=0
fetch_error_usage claude-fresh > "${TMP}/error-usage-3.json" || rc=$?
out_fresh=$(< "${TMP}/error-usage-3.json")
meta_after=$(jq -r '.providerMeta.claude.updatedAt' "${error_usage_cache}/usage.json" 2>/dev/null)
if (( rc == 0 )) \
    && printf '%s' "${out_error}" | jq -e '.[] | select(.provider == "claude") | .error.message == "Not signed in" and .usage.primary.usedPercent == 17' >/dev/null 2>&1 \
    && [[ -n "${meta_before}" && "${meta_during}" == "${meta_before}" ]] \
    && printf '%s' "${out_fresh}" | jq -e '.[] | select(.provider == "claude") | (has("error") | not) and .usage.primary.usedPercent == 55' >/dev/null 2>&1 \
    && [[ "${meta_after}" -ge "${meta_during}" ]]; then
    ok "fetcher keeps last-known usage beside a fresh error and drops it on success"
else
    fail "fetcher keeps last-known usage beside a fresh error and drops it on success" "rc=${rc}; error_out=${out_error:0:160}; fresh_out=${out_fresh:0:160}; meta=${meta_before}/${meta_during}/${meta_after}"
fi


# Config discovery failure + stale cache with no allow-list overlap should
# fall through to the explicit allow-list rather than giving up on refresh.
config_fail_dir="${TMP}/config-fail"
mkdir -p "${config_fail_dir}"
cat > "${config_fail_dir}/codexbar" <<EOF
#!/bin/sh
if [ "\${1:-}" = "config" ] && [ "\${2:-}" = "providers" ]; then
    exit 7
fi
provider=""
while [ "\$#" -gt 0 ]; do
    case "\$1" in
        --provider) shift; provider="\${1:-}" ;;
    esac
    shift
done
jq --arg p "\${provider}" '[.[] | select(.provider == \$p)]' \
    < "\${SHOWY_QUOTA_TEST_FIXTURE:-${FIXTURE_DIR}/codexbar-mixed.json}"
EOF
chmod +x "${config_fail_dir}/codexbar"
cache=$(mk_cache)
jq '[.[] | select(.provider == "codex")]' "${FIXTURE_DIR}/codexbar-mixed.json" > "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${config_fail_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_PROVIDERS='gemini' \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) \
    && [[ "$(printf '%s' "${out}" | jq -r '[.[].provider] | join(",")')" == "gemini" ]]; then
    ok "fetcher falls through to allow-list when cache filters empty"
else
    fail "fetcher falls through to allow-list when cache filters empty" "rc=${rc}; out=${out:0:120}"
fi

# Canonical empty inventory: when `codexbar config providers` reports no
# enabled providers, the fetcher must publish `[]` so renderers can show idle
# instead of stale/blank output.
empty_inv_dir="${TMP}/empty-inv"
mkdir -p "${empty_inv_dir}"
cat > "${empty_inv_dir}/codexbar" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "config" ] && [ "${2:-}" = "providers" ]; then
    printf '[]'
    exit 0
fi
# Should never be called when the inventory is empty.
exit 42
EOF
chmod +x "${empty_inv_dir}/codexbar"
cache=$(mk_cache)
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${empty_inv_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && [[ "${out}" == "[]" ]] && [[ "$(cache_source_value "${cache}")" == "cli" ]]; then
    ok "fetcher publishes empty cache when codexbar reports no enabled providers"
else
    fail "fetcher publishes empty cache when codexbar reports no enabled providers" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
seed_usage_source "${cache}" "cli"
printf '%s\n' "$(date +%s)" > "${cache}/cli-failed-at"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${empty_inv_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_CODEXBAR_CLI_FAILURE_BACKOFF_SECONDS=3600 \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && [[ "${out}" == "[]" ]] && [[ "$(cache_source_value "${cache}")" == "cli" ]] && ! [[ -e "${cache}/cli-failed-at" ]]; then
    ok "fetcher publishes empty inventory before CLI backoff"
else
    fail "fetcher publishes empty inventory before CLI backoff" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${empty_inv_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && [[ "${out}" == "[]" ]] && [[ "$(cache_source_value "${cache}")" == "cli" ]]; then
    ok "fetcher rejects stale serve payload when inventory is empty"
else
    fail "fetcher rejects stale serve payload when inventory is empty" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${empty_inv_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-empty.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && [[ "${out}" == "[]" ]] && [[ "$(cache_source_value "${cache}")" == "cli" ]]; then
    ok "fetcher rejects empty serve payload when inventory is empty"
else
    fail "fetcher rejects empty serve payload when inventory is empty" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${empty_inv_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && [[ "${out}" == "[]" ]] && [[ "$(cache_source_value "${cache}")" == "cli" ]]; then
    ok "fetcher rejects invalid serve payload when inventory is empty"
else
    fail "fetcher rejects invalid serve payload when inventory is empty" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${empty_inv_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_url}" \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    SHOWY_QUOTA_TEST_SERVE_URL="http://127.0.0.1:18081" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && [[ "${out}" == "[]" ]] && [[ "$(cache_source_value "${cache}")" == "cli" ]]; then
    ok "fetcher skips unreachable serve probe when inventory is empty"
else
    fail "fetcher skips unreachable serve probe when inventory is empty" "rc=${rc}; out=${out}"
fi

# All-invalid inventory: a discovery response containing only unsafe provider
# ids must be treated as a discovery failure, never silently published as
# canonical empty (would mask CodexBar misconfiguration).
bad_inv_dir="${TMP}/bad-inv"
mkdir -p "${bad_inv_dir}"
cat > "${bad_inv_dir}/codexbar" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "config" ] && [ "${2:-}" = "providers" ]; then
    printf '%s' '[{"provider":"bad/id","enabled":true}]'
    exit 0
fi
exit 99
EOF
chmod +x "${bad_inv_dir}/codexbar"
cache=$(mk_cache)
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${bad_inv_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc != 0 )) && [[ -z "${out}" ]] && ! [[ -f "${cache}/usage.json" ]] \
    && [[ -r "${cache}/config-providers-failed-at" ]]; then
    ok "fetcher treats all-invalid provider inventory as discovery failure"
else
    fail "fetcher treats all-invalid provider inventory as discovery failure" "rc=${rc}; out=${out}"
fi

# Empty/null/missing provider ids in enabled records are invalid inventory,
# not canonical empty inventory.
bad_empty_inv_dir="${TMP}/bad-empty-inv"
mkdir -p "${bad_empty_inv_dir}"
cat > "${bad_empty_inv_dir}/codexbar" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "config" ] && [ "${2:-}" = "providers" ]; then
    printf '%s' "${SHOWY_QUOTA_TEST_PROVIDERS_PAYLOAD}"
    exit 0
fi
exit 99
EOF
chmod +x "${bad_empty_inv_dir}/codexbar"
for bad_payload in \
    '[{"provider":"","enabled":true}]' \
    '[{"provider":null,"enabled":true}]' \
    '[{"enabled":true}]'
do
    cache=$(mk_cache)
    rc=0
    out=$(
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${bad_empty_inv_dir}/codexbar" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_TEST_PROVIDERS_PAYLOAD="${bad_payload}" \
        "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
    ) || rc=$?
    if (( rc != 0 )) && [[ -z "${out}" ]] && ! [[ -f "${cache}/usage.json" ]] \
        && [[ -r "${cache}/config-providers-failed-at" ]]; then
        ok "fetcher treats enabled provider payload ${bad_payload} as invalid inventory"
    else
        fail "fetcher treats enabled provider payload ${bad_payload} as invalid inventory" "rc=${rc}; out=${out}"
    fi
done

# SHOWY_QUOTA_PROVIDERS_EXCLUDE drops providers from the discovered inventory
# before any per-provider CLI call.

cache=$(mk_cache)
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${bad_empty_inv_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_TEST_PROVIDERS_PAYLOAD='[{"provider":"codex","enabled":true},{"enabled":true}]' \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc != 0 )) && [[ -z "${out}" ]] && ! [[ -f "${cache}/usage.json" ]] \
    && [[ -r "${cache}/config-providers-failed-at" ]]; then
    ok "fetcher treats mixed valid and malformed enabled providers as invalid inventory"
else
    fail "fetcher treats mixed valid and malformed enabled providers as invalid inventory" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
usage_counter="${cache}/exclude-call-count"
: > "${usage_counter}"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${provider_aware_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_PROVIDERS_EXCLUDE='codex,cursor' \
    SHOWY_QUOTA_TEST_COUNTER="${usage_counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
excl_calls=$(wc -c < "${usage_counter}" | tr -d ' ')
if (( rc == 0 )) \
    && printf '%s' "${out}" | jq -e 'type == "array" and any(.provider == "claude") and any(.provider == "gemini") and (all(.[]; .provider != "codex" and .provider != "cursor"))' >/dev/null 2>&1 \
    && (( excl_calls == 2 )); then
    ok "fetcher exclude list prunes providers before per-provider CLI calls"
else
    fail "fetcher exclude list prunes providers before per-provider CLI calls" "rc=${rc}; calls=${excl_calls}; out=${out:0:120}"
fi

# 6. Stale-cache rendering marks one frozen-snapshot indicator and greys data.
printf '\nstale cache rendering\n'

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
# Backdate cache to 1988 so age is decades, well beyond 2 * default REFRESH_SECONDS.
touch -t 198801010000 "${cache}/usage.json"
# Use a bogus codexbar bin so fetch cannot refresh the backdated cache.
ansi_dim=$'\x1b[2m'
stale_rgb=$(hex_to_rgb_csv "$(run_common_eval 'showy_quota_palette stale' SHOWY_QUOTA_NO_CONFIG=1)")
stale_sgr="${stale_rgb//,/;}"
surface_rgb=$(hex_to_rgb_csv "$(run_common_eval 'showy_quota_palette surface' SHOWY_QUOTA_NO_CONFIG=1)")
surface_sgr="${surface_rgb//,/;}"
bg_rgb=$(hex_to_rgb_csv "$(run_common_eval 'showy_quota_palette bg' SHOWY_QUOTA_NO_CONFIG=1)")
bg_sgr="${bg_rgb//,/;}"
countdown_warn_rgb=$(hex_to_rgb_csv "$(run_common_eval 'showy_quota_palette countdown_warn' SHOWY_QUOTA_NO_CONFIG=1)")
countdown_warn_sgr="${countdown_warn_rgb//,/;}"
printf -v stale_half_escape '\033[38;2;%sm\033[48;2;%sm▀' "${stale_sgr}" "${stale_sgr}"
printf -v stale_sextant_escape '\033[38;2;%sm\033[48;2;%sm🬎' "${stale_sgr}" "${surface_sgr}"
printf -v stale_sigil_bg_escape '\033[48;2;%smCL' "${stale_sgr}"
printf -v stale_countdown_escape '\033[38;2;%sm\033[48;2;%sm12m' "${stale_sgr}" "${surface_sgr}"
printf -v stale_separator_escape '\033[38;2;%sm\033[48;2;%sm▕' "${bg_sgr}" "${stale_sgr}"
printf -v trailing_stale_escape ' \033[1m\033[38;2;%sm\033[48;2;%sm⚠' "${countdown_warn_sgr}" "${bg_sgr}"
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_FORCE_COLOR=1 \
    SHOWY_QUOTA_NOW_EPOCH=4070928480 \
    "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_contains "zellij shows trailing stale indicator" "${trailing_stale_escape}" "${out}"
assert_contains "zellij greys stale half-block cells" "${stale_half_escape}" "${out}"
assert_contains "zellij greys stale sigil background" "${stale_sigil_bg_escape}" "${out}"
assert_contains "zellij keeps separator on stale background" "${stale_separator_escape}" "${out}"
assert_contains "zellij greys stale countdown" "${stale_countdown_escape}" "${out}"
assert_not_contains "zellij does not dim stale cache" "${ansi_dim}" "${out}"
assert_contains "zellij stale cache preserves valid countdown text" "12m" "${out}"

# Forced mono3 needs a provider with all three windows; a provider without a
# tertiary window collapses to dual.
sextant_stale_cache=$(mk_cache)
cp "${mono_fixture}" "${sextant_stale_cache}/usage.json"
touch -t 198801010000 "${sextant_stale_cache}/usage.json"
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${sextant_stale_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_FORCE_COLOR=1 \
    SHOWY_QUOTA_NOW_EPOCH=4070928480 \
    SHOWY_QUOTA_TERMINAL_BAR_MODE=mono3 \
    "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_contains "zellij mono3 stale shows trailing stale indicator" "${trailing_stale_escape}" "${out}"
assert_contains "zellij mono3 greys stale sextant cells" "${stale_sextant_escape}" "${out}"
assert_contains "zellij mono3 keeps separator on stale background" "${stale_separator_escape}" "${out}"
assert_not_contains "zellij mono3 stale omits half-block cells" "▀" "${out}"
assert_not_contains "zellij mono3 stale has no elapsed marker" "190;149;255" "${out}"

out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_NOW_EPOCH=4070928480 \
    "${REPO_ROOT}/bin/showy-quota-tmux-bar"
)
assert_not_contains "tmux does not dim stale cache" "#[dim]" "${out}"
assert_contains "tmux shows trailing stale indicator" "#[fg=#161616,bg=#161616] #[default]#[fg=#ee5396,bg=#161616,bold]⚠" "${out}"
assert_contains "tmux keeps separator on stale background" "#[fg=#161616,bg=#6c7086]▕" "${out}"
assert_contains "tmux greys stale countdown" "#[fg=#6c7086,bg=#2a2a2a,bold]12m" "${out}"
assert_not_contains "tmux stale cache has no weekly hint" "]w" "${out}"

out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${sextant_stale_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_NOW_EPOCH=4070928480 \
    SHOWY_QUOTA_TERMINAL_BAR_MODE=mono3 \
    "${REPO_ROOT}/bin/showy-quota-tmux-bar"
)
assert_contains "tmux mono3 stale shows trailing stale indicator" "#[fg=#161616,bg=#161616] #[default]#[fg=#ee5396,bg=#161616,bold]⚠" "${out}"
assert_contains "tmux mono3 greys stale sextant cells" "#[fg=#6c7086,bg=#2a2a2a]🬎" "${out}"
assert_contains "tmux mono3 keeps separator on stale background" "#[fg=#161616,bg=#6c7086]▕" "${out}"
assert_not_contains "tmux mono3 stale omits half-block cells" "▀" "${out}"
assert_not_contains "tmux mono3 stale has no elapsed marker" "be95ff" "${out}"

mono_stale_cache=$(mk_cache)
cp "${mono_fixture}" "${mono_stale_cache}/usage.json"
touch -t 198801010000 "${mono_stale_cache}/usage.json"
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${mono_stale_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_FORCE_COLOR=1 \
    SHOWY_QUOTA_NOW_EPOCH=4070912400 \
    SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 \
    "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_contains "zellij mono3 stale shows trailing stale indicator" "${trailing_stale_escape}" "${out}"
assert_contains "zellij mono3 stale greys sextant cells" "${stale_sextant_escape}" "${out}"
assert_not_contains "zellij mono3 stale omits half-block cells" "▀" "${out}"
assert_not_contains "zellij mono3 stale omits shared separator" "│" "${out}"
assert_not_contains "zellij mono3 stale has no elapsed color" "190;149;255" "${out}"

out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${mono_stale_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_NOW_EPOCH=4070912400 \
    SHOWY_QUOTA_TMUX_BAR_WIDTH=8 \
    "${REPO_ROOT}/bin/showy-quota-tmux-bar"
)
assert_contains "tmux mono3 stale shows trailing stale indicator" "#[fg=#161616,bg=#161616] #[default]#[fg=#ee5396,bg=#161616,bold]⚠" "${out}"
assert_contains "tmux mono3 stale greys sextant cells" "#[fg=#6c7086,bg=#2a2a2a]🬎" "${out}"
assert_not_contains "tmux mono3 stale omits half-block cells" "▀" "${out}"
assert_not_contains "tmux mono3 stale omits shared separator" "│" "${out}"
assert_not_contains "tmux mono3 stale has no elapsed color" "be95ff" "${out}"


stale_past_fixture="${TMP}/codexbar-stale-past.json"
printf '%s\n' '[{"provider":"claude","usage":{"primary":{"usedPercent":17,"windowMinutes":300,"resetsAt":"1988-01-01T00:00:00Z"}}}]' > "${stale_past_fixture}"
cache=$(mk_cache)
cp "${stale_past_fixture}" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_FORCE_COLOR=1 \
    "${REPO_ROOT}/bin/showy-quota-zellij-bar"
)
assert_contains "zellij stale absolute reset shows unknown countdown" "?" "${out}"
assert_not_contains "zellij stale absolute reset does not show now" "now" "${out}"


# 7. Concurrent fetch — only one codexbar invocation across simultaneous
#    callers. We exercise both lock paths via SHOWY_QUOTA_FORCE_NO_FLOCK.
printf '\nconcurrent fetch\n'

slow_dir="${TMP}/slow"
mkdir -p "${slow_dir}"
cat > "${slow_dir}/codexbar" <<EOF
#!/bin/sh
# Provider inventory: instant response derived from the fixture, no counter.
fixture="\${SHOWY_QUOTA_TEST_FIXTURE:-${FIXTURE_DIR}/codexbar-mixed.json}"
if [ "\${1:-}" = "config" ] && [ "\${2:-}" = "providers" ]; then
    jq '[.[] | {provider, enabled: true}]' < "\${fixture}"
    exit 0
fi
# Per-provider usage call: count the invocation, then sleep before returning.
[ -n "\${SHOWY_QUOTA_TEST_COUNTER:-}" ] && printf 'x' >> "\${SHOWY_QUOTA_TEST_COUNTER}"
sleep 1
provider=""
while [ "\$#" -gt 0 ]; do
    case "\$1" in
        --provider) shift; provider="\${1:-}" ;;
    esac
    shift
done
if [ -n "\${provider}" ]; then
    jq --arg p "\${provider}" '[.[] | select(.provider == \$p)]' < "\${fixture}"
else
    cat "\${fixture}"
fi
EOF
chmod +x "${slow_dir}/codexbar"

for path_label in flock mkdir; do
    if [[ "${path_label}" == "flock" ]]; then
        force_no_flock=""
    else
        force_no_flock=1
    fi
    cache=$(mk_cache)
    counter="${cache}/cb-call-count"
    : > "${counter}"
    pids=()
    outputs=()
    for idx in 1 2 3 4; do
        out_file="${cache}/out.${idx}.json"
        outputs+=("${out_file}")
        (
            SHOWY_QUOTA_NO_CONFIG=1 \
            SHOWY_QUOTA_CACHE_DIR="${cache}" \
            SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
            SHOWY_QUOTA_FORCE_NO_FLOCK="${force_no_flock}" \
            SHOWY_QUOTA_TEST_COUNTER="${counter}" \
            SHOWY_QUOTA_PROVIDERS=claude \
            "${REPO_ROOT}/bin/showy-quota-fetch" > "${out_file}" 2>/dev/null
        ) &
        pids+=("$!")
    done

    all_callers_ok=1
    for idx in "${!pids[@]}"; do
        if wait "${pids[$idx]}" && jq -e 'type == "array"' "${outputs[$idx]}" >/dev/null 2>&1; then
            :
        else
            all_callers_ok=0
        fi
    done
    if (( all_callers_ok )); then
        ok "${path_label} path: every concurrent caller gets valid JSON"
    else
        fail "${path_label} path: every concurrent caller gets valid JSON"
    fi

    calls=$(wc -c < "${counter}" | tr -d ' ')
    if (( calls == 1 )); then
        ok "${path_label} path: codexbar invoked exactly once across 4 callers"
    else
        fail "${path_label} path: codexbar invoked exactly once across 4 callers" "got ${calls} calls"
    fi
    if [[ -s "${cache}/usage.json" ]]; then
        ok "${path_label} path: cache populated"
    else
        fail "${path_label} path: cache populated"
    fi
done

cache=$(mk_cache)
counter="${cache}/recovered-lock-call-count"
: > "${counter}"
mkdir "${cache}/usage.lock.d"
printf '%s\n' 999999 > "${cache}/usage.lock.d/owner.pid"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
    SHOWY_QUOTA_TEST_COUNTER="${counter}" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array"' >/dev/null 2>&1; then
    ok "mkdir path: recovers dead owner lock"
else
    fail "mkdir path: recovers dead owner lock" "rc=${rc}"
fi

cache=$(mk_cache)
counter="${cache}/recovered-stopped-lock-call-count"
: > "${counter}"
sleep 30 &
owner_pid=$!
kill -STOP "${owner_pid}" 2>/dev/null || true
mkdir "${cache}/usage.lock.d"
printf '%s\n' "${owner_pid}" > "${cache}/usage.lock.d/owner.pid"
rc=0
if wait_for_state_prefix "${owner_pid}" T; then
    out=$(
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
        SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
        SHOWY_QUOTA_TEST_COUNTER="${counter}" \
        "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
    ) || rc=$?
else
    rc=99
    out=""
fi
kill -KILL "${owner_pid}" 2>/dev/null || true
wait "${owner_pid}" 2>/dev/null || true
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array"' >/dev/null 2>&1; then
    ok "mkdir path: recovers stopped owner lock"
else
    fail "mkdir path: recovers stopped owner lock" "rc=${rc}"
fi

cache=$(mk_cache)
counter="${cache}/recovered-aged-lock-call-count"
: > "${counter}"
sleep 30 &
owner_pid=$!
mkdir "${cache}/usage.lock.d"
printf '%s\n' "${owner_pid}" > "${cache}/usage.lock.d/owner.pid"
touch -t 198801010000 "${cache}/usage.lock.d"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
    SHOWY_QUOTA_TEST_COUNTER="${counter}" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
kill -KILL "${owner_pid}" 2>/dev/null || true
wait "${owner_pid}" 2>/dev/null || true
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array"' >/dev/null 2>&1; then
    ok "mkdir path: recovers aged live owner lock"
else
    fail "mkdir path: recovers aged live owner lock" "rc=${rc}"
fi

for owner_case in empty malformed; do
    cache=$(mk_cache)
    counter="${cache}/recovered-${owner_case}-lock-call-count"
    : > "${counter}"
    mkdir "${cache}/usage.lock.d"
    if [[ "${owner_case}" == "empty" ]]; then
        : > "${cache}/usage.lock.d/owner.pid"
    else
        printf '%s\n' not-a-pid > "${cache}/usage.lock.d/owner.pid"
    fi
    touch -t 198801010000 "${cache}/usage.lock.d"
    rc=0
    out=$(
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
        SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
        SHOWY_QUOTA_LOCK_WAIT_TENTHS=1 \
        SHOWY_QUOTA_TEST_COUNTER="${counter}" \
        "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
    ) || rc=$?
    if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array"' >/dev/null 2>&1; then
        ok "mkdir path: recovers ${owner_case} owner lock"
    else
        fail "mkdir path: recovers ${owner_case} owner lock" "rc=${rc}"
    fi
done

cache=$(mk_cache)
counter="${cache}/retry-empty-lock-call-count"
: > "${counter}"
mkdir "${cache}/usage.lock.d"
: > "${cache}/usage.lock.d/owner.pid"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
    SHOWY_QUOTA_LOCK_WAIT_TENTHS=10 \
    SHOWY_QUOTA_TEST_COUNTER="${counter}" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array"' >/dev/null 2>&1; then
    ok "mkdir path: retries empty owner lock after wait"
else
    fail "mkdir path: retries empty owner lock after wait" "rc=${rc}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-low.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
expected_stale=$(< "${cache}/usage.json")
counter="${cache}/retry-stale-empty-lock-call-count"
: > "${counter}"
mkdir "${cache}/usage.lock.d"
: > "${cache}/usage.lock.d/owner.pid"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
    SHOWY_QUOTA_LOCK_WAIT_TENTHS=50 \
    SHOWY_QUOTA_TEST_COUNTER="${counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_PROVIDERS=codex \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && [[ "${out}" == "${expected_stale}" ]]; then
    ok "mkdir path: stale valid cache skips empty owner lock wait"
else
    fail "mkdir path: stale valid cache skips empty owner lock wait" "rc=${rc}; out=${out}"
fi


cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-low.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
counter="${cache}/nonforced-valid-lock-call-count"
expected_stale=$(< "${cache}/usage.json")
: > "${counter}"
(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
    SHOWY_QUOTA_TEST_COUNTER="${counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_PROVIDERS=codex \
    "${REPO_ROOT}/bin/showy-quota-fetch" --refresh >/dev/null 2>/dev/null
) &
holder_pid=$!
sleep 0.2
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
    SHOWY_QUOTA_LOCK_WAIT_TENTHS=20 \
    SHOWY_QUOTA_TEST_COUNTER="${counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
wait "${holder_pid}" || true
if (( rc == 0 )) && [[ "${out}" == "${expected_stale}" ]]; then
    ok "non-forced valid cache skips mkdir lock wait"
else
    fail "non-forced valid cache skips mkdir lock wait" "rc=${rc}; out=${out}"
fi
printf '\nforced refresh lock wait\n'
cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-low.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
counter="${cache}/cb-call-count"
: > "${counter}"
pids=()
outputs=()
errors=()
for idx in 1 2 3 4; do
    out_file="${cache}/refresh.${idx}.json"
    err_file="${cache}/refresh.${idx}.err"
    outputs+=("${out_file}")
    errors+=("${err_file}")
    (
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
        SHOWY_QUOTA_DEBUG=1 \
        SHOWY_QUOTA_LOCK_WAIT_TENTHS=300 \
        SHOWY_QUOTA_TEST_COUNTER="${counter}" \
        SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
        SHOWY_QUOTA_PROVIDERS=codex \
        "${REPO_ROOT}/bin/showy-quota-fetch" --refresh > "${out_file}" 2> "${err_file}"
    ) &
    pids+=("$!")
done

all_fresh=1
bad_idx=()
for idx in "${!pids[@]}"; do
    if wait "${pids[$idx]}" && grep -F -q 'futureUnknownTopLevelField' "${outputs[$idx]}"; then
        :
    else
        all_fresh=0
        bad_idx+=("${idx}")
    fi
done
if (( all_fresh )); then
    ok "forced refresh callers wait for refreshed cache"
else
    for idx in "${bad_idx[@]}"; do
        out_file="${outputs[$idx]}"
        err_file="${errors[$idx]}"
        out_size=$(wc -c < "${out_file}" 2>/dev/null | tr -d ' ' || printf '?')
        printf '  caller %s: out_size=%s first_byte=%q\n' \
            "$((idx + 1))" "${out_size}" \
            "$(head -c1 "${out_file}" 2>/dev/null || printf '')" >&2
        if [[ -s "${err_file}" ]]; then
            printf '  caller %s stderr (last 20 lines):\n' "$((idx + 1))" >&2
            tail -n 20 "${err_file}" | sed 's/^/    /' >&2
        fi
    done
    fail "forced refresh callers wait for refreshed cache"
fi
calls=$(wc -c < "${counter}" | tr -d ' ')
if (( calls == 1 )); then
    ok "forced refresh invokes codexbar once across 4 callers"
else
    fail "forced refresh invokes codexbar once across 4 callers" "got ${calls} calls"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-low.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
counter="${cache}/forced-refresh-empty-lock-call-count"
: > "${counter}"
mkdir "${cache}/usage.lock.d"
: > "${cache}/usage.lock.d/owner.pid"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
    SHOWY_QUOTA_LOCK_WAIT_TENTHS=10 \
    SHOWY_QUOTA_TEST_COUNTER="${counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_PROVIDERS=codex \
    "${REPO_ROOT}/bin/showy-quota-fetch" --refresh 2>/dev/null
) || rc=$?
if (( rc == 0 )) && grep -F -q 'futureUnknownTopLevelField' <<< "${out}"; then
    ok "forced refresh retries empty owner lock before stale fallback"
else
    fail "forced refresh retries empty owner lock before stale fallback" "rc=${rc}; out=${out}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-low.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
counter="${cache}/timeout-call-count"
expected_stale=$(< "${cache}/usage.json")
: > "${counter}"
timeout_dir="${TMP}/slow-refresh"
mkdir -p "${timeout_dir}"
cat > "${timeout_dir}/codexbar" <<'EOF'
#!/bin/sh
[ -n "${SHOWY_QUOTA_TEST_COUNTER:-}" ] && printf 'x' >> "${SHOWY_QUOTA_TEST_COUNTER}"
sleep 5
cat "${SHOWY_QUOTA_TEST_FIXTURE:-${FIXTURE_DIR}/codexbar-mixed.json}"
EOF
chmod +x "${timeout_dir}/codexbar"
(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${timeout_dir}/codexbar" \
    SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
    SHOWY_QUOTA_TEST_COUNTER="${counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" --refresh >/dev/null 2>/dev/null
) &
holder_pid=$!
sleep 0.2
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${timeout_dir}/codexbar" \
    SHOWY_QUOTA_FORCE_NO_FLOCK=1 \
    SHOWY_QUOTA_LOCK_WAIT_TENTHS=0 \
    SHOWY_QUOTA_TEST_COUNTER="${counter}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-quota-fetch" --refresh 2>/dev/null
) || rc=$?
wait "${holder_pid}" || true
if (( rc == 0 )) && [[ "${out}" == "${expected_stale}" ]]; then
    ok "forced refresh timeout falls back to stale cache"
else
    fail "forced refresh timeout falls back to stale cache" "rc=${rc}; out=${out}"
fi
# ── marker overflow guard (showy-quota-marker-overflow) ─────────────────
# An absurd windowMinutes (2^62) previously wrapped 64-bit duration to 0 and
# crashed the elapsed-marker math with a divide-by-zero. Pin the guard through
# the zellij driver/native renderer path: the strip must render successfully,
# with the provider visible, but with no elapsed marker column.
printf '\nmarker overflow guard\n'

marker_overflow_fixture="${TMP}/marker-overflow.json"
cat > "${marker_overflow_fixture}" <<'JSON'
[{"provider":"claude","usage":{"primary":{"usedPercent":50,"windowMinutes":4611686018427387904,"resetsAt":"2026-07-06T20:00:00Z"},"secondary":{"usedPercent":50,"windowMinutes":100,"resetsAt":"2099-01-01T01:40:00Z"},"tertiary":{"usedPercent":50,"windowMinutes":100,"resetsAt":"2099-01-01T01:40:00Z"}}}]
JSON
marker_rc=0
marker_out=$(
    env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TERMINAL_BAR_MODE=mono3 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_FORCE_COLOR=0 NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar" --json - < "${marker_overflow_fixture}" 2>&1
) || marker_rc=$?
assert_equals "overflow windowMinutes renders successfully" "0" "${marker_rc}"
assert_contains "overflow windowMinutes keeps provider visible" "CL" "${marker_out}"
assert_not_contains "overflow windowMinutes yields no elapsed marker" "│" "${marker_out}"

marker_sane_fixture="${TMP}/marker-sane.json"
cat > "${marker_sane_fixture}" <<'JSON'
[{"provider":"claude","usage":{"primary":{"usedPercent":50,"windowMinutes":100,"resetsAt":"2099-01-01T01:40:00Z"},"secondary":{"usedPercent":50,"windowMinutes":100,"resetsAt":"2099-01-01T01:40:00Z"},"tertiary":{"usedPercent":50,"windowMinutes":100,"resetsAt":"2099-01-01T01:40:00Z"}}}]
JSON
marker_out=$(
    env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TERMINAL_BAR_MODE=mono3 SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=8 SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_FORCE_COLOR=0 NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar" --json - < "${marker_sane_fixture}" 2>&1
) || marker_out=""
assert_contains "sane windowMinutes still renders elapsed marker" "│" "${marker_out}"

# ── cache age with future mtime (showy-quota-future-mtime) ──────────────
# A future-dated cache file (clock skew, restored backup, hand-touched file)
# must be treated as aged, not pinned-fresh forever: showy_quota_age_seconds
# reports the absolute distance |now - mtime|, and showy_quota_cache_stale_for
# flags it stale. A freshly-written file still reads as a small age.
printf '\ncache age with future mtime\n'

future_cache="${TMP}/future-mtime-cache.json"
: > "${future_cache}"
touch -t 401201010000 "${future_cache}"
future_age=$(run_common_eval "showy_quota_age_seconds \"${future_cache}\"" SHOWY_QUOTA_NO_CONFIG=1)
if [[ "${future_age}" =~ ^[0-9]+$ ]] && (( future_age > 1000000000 )); then
    ok "future-dated cache reports a large positive age (absolute distance)"
else
    fail "future-dated cache reports a large positive age (absolute distance)" "age=${future_age}"
fi

future_stale_rc=0
run_common_eval "showy_quota_cache_stale_for \"${future_cache}\"" SHOWY_QUOTA_NO_CONFIG=1 || future_stale_rc=$?
assert_equals "future-dated cache is flagged stale" "0" "${future_stale_rc}"

fresh_cache="${TMP}/fresh-mtime-cache.json"
: > "${fresh_cache}"
fresh_age=$(run_common_eval "showy_quota_age_seconds \"${fresh_cache}\"" SHOWY_QUOTA_NO_CONFIG=1)
if [[ "${fresh_age}" =~ ^[0-9]+$ ]] && (( fresh_age < 60 )); then
    ok "freshly-written cache reports a small age"
else
    fail "freshly-written cache reports a small age" "age=${fresh_age}"
fi

# ── future-dated ownerless render lock (showy-quota-future-render-lock) ──
# An ownerless render.lock dir with a far-future mtime yields a negative age
# that would never satisfy the ownerless-reclaim threshold and would wedge
# SketchyBar rendering forever. It must be reclaimed by absolute distance
# (|age| >= 30) so rendering proceeds (sketchybar receives --set) without the
# "render lock is ownerless; skipping" bail-out.
printf '\nfuture-dated render lock\n'

cache=$(mk_cache)
future_lock_dir="${cache}/sb/render.lock"
mkdir -p "${future_lock_dir}"
touch -t 401201010000 "${future_lock_dir}"
log="${TMP}/sb-future-lock.log"
future_lock_err="${TMP}/sb-future-lock.err"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}" SHOWY_QUOTA_DEBUG=1 2>"${future_lock_err}"
plugin_log="$(< "${log}")"
assert_contains "future-dated ownerless render lock is reclaimed and rendering proceeds" "--set" "${plugin_log}"
assert_not_contains "future-dated ownerless render lock does not wedge rendering" "render lock is ownerless; skipping" "$(< "${future_lock_err}")"

# ── dot provider discovery preserves cache (showy-quota-dot-provider) ────
# `codexbar config providers` reporting a single '.' provider is invalid
# inventory (valid_provider_id rejects '.'), not a canonical-empty inventory:
# enabled-record count (1) != valid-id count (0) => discovery fails and the
# fetcher must fall back to the existing cache rather than clobbering it with
# the canonical-empty `[]` payload.
printf '\ndot provider discovery\n'

dot_inv_dir="${TMP}/dot-inv"
mkdir -p "${dot_inv_dir}"
cat > "${dot_inv_dir}/codexbar" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "config" ] && [ "${2:-}" = "providers" ]; then
    printf '%s' '[{"provider":".","enabled":true}]'
    exit 0
fi
exit 99
EOF
chmod +x "${dot_inv_dir}/codexbar"

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
rc=0
out=$(
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${dot_inv_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_REFRESH_SECONDS=0 \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
dot_cache_after="missing"
[[ -r "${cache}/usage.json" ]] && dot_cache_after="$(< "${cache}/usage.json")"
assert_equals "dot-only inventory fetch succeeds (falls back to cache)" "0" "${rc}"
if [[ "${dot_cache_after}" != "[]" ]] \
    && printf '%s' "${dot_cache_after}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1; then
    ok "dot-only inventory preserves last-known-good cache (never publishes [])"
else
    fail "dot-only inventory preserves last-known-good cache (never publishes [])" "cache=${dot_cache_after:0:80}"
fi

# ── serve recycle refuses foreign listeners (showy-quota-ba81839102504781) ──
# A stale-build serve on our port that is NOT a codexbar serve (a foreign
# process merely holding the port) must never be signaled: serve_recycle_stale
# only kills lsof listeners it verifies as a codexbar serve. The fetcher keeps
# using the stale serve rather than killing an unrelated process — the removed
# blind `pkill -f 'codexbar serve'` fallback could kill other sessions' serves.
printf '\nserve recycle refuses foreign listeners\n'

recycle_bin_dir="${TMP}/serve-recycle-foreign-bin"
mkdir -p "${recycle_bin_dir}"
cat > "${recycle_bin_dir}/codexbar" <<'EOF'
#!/usr/bin/env bash
set -eu
if [[ "${1:-}" == "--version" ]]; then
    printf 'CodexBar %s\n' "${SHOWY_QUOTA_TEST_ONDISK_VERSION:-2.0.0}"
    exit 0
fi
# A `serve` (or any other) invocation must NOT grab the port; the foreign
# listener owns it. Exit non-zero so a reintroduced start path fails loudly.
exit 90
EOF
chmod +x "${recycle_bin_dir}/codexbar"

recycle_cache=$(mk_cache)
recycle_usage="${TMP}/serve-recycle-usage.json"
cp "${FIXTURE_DIR}/codexbar-realistic.json" "${recycle_usage}"
recycle_port=$(unused_tcp_port)
recycle_url="http://127.0.0.1:${recycle_port}"
# A plain python HTTP server: its command's first-word basename is the python
# interpreter, never the codexbar bin basename, so serve_listener_is_codexbar
# rejects it and serve_recycle_stale must leave it untouched.
python3 - "${recycle_port}" "${recycle_usage}" <<'PY' &
import http.server
import json
import sys

port = int(sys.argv[1])
fixture = sys.argv[2]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            body = json.dumps({"status": "ok", "version": "1.0.0"}).encode()
        elif self.path == "/usage":
            with open(fixture, "rb") as fh:
                body = fh.read()
        else:
            self.send_response(404)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *args):
        return

http.server.ThreadingHTTPServer.allow_reuse_address = True
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
recycle_listener_pid=$!

recycle_ready=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do
    if curl -fsS --max-time 1 "${recycle_url}/health" >/dev/null 2>&1; then
        recycle_ready=1
        break
    fi
    sleep 0.1
done
if (( recycle_ready )); then
    ok "foreign listener is up on the serve port"
else
    fail "foreign listener is up on the serve port" "port=${recycle_port}"
fi

# Stale build (serve /health 1.0.0 vs on-disk 2.0.0) forces the recycle path.
# The fetch rc is irrelevant here; the contract is that the foreign listener
# survives.
run_with_test_timeout 15 env \
    PATH="${recycle_bin_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_MANAGE_SERVE=1 \
    SHOWY_QUOTA_CACHE_DIR="${recycle_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${recycle_bin_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${recycle_url}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS=10 \
    "${REPO_ROOT}/bin/showy-quota-fetch" --refresh >/dev/null 2>&1 || true

recycle_listener_state="dead"
kill -0 "${recycle_listener_pid}" 2>/dev/null && recycle_listener_state="alive"
kill "${recycle_listener_pid}" 2>/dev/null || true
wait "${recycle_listener_pid}" 2>/dev/null || true
assert_equals "serve recycle leaves an unverified foreign listener alive" "alive" "${recycle_listener_state}"

# ── automation: quota guard ─────────────────────────────────────────────
# guard evaluates showy-quota-state provider metrics for CI/automation. Each
# case seeds a fresh cache from a fixture and disables codexbar (missing bin +
# empty SERVE_URL, MANAGE_SERVE off), so the internal refresh is a no-op and the
# seeded metrics are evaluated as-is. The huge refresh knob keeps the
# just-written cache non-stale. codexbar-mixed: codex primary 95% remaining
# (5% used), codex secondary 41% remaining (59% used).
printf '\nquota guard\n'

run_guard() {
    local fixture="$1"; shift
    local cache; cache=$(mk_cache)
    cp "$(fixture_path "${fixture}")" "${cache}/usage.json"
    seed_usage_source "${cache}" "cli"
    env \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-guard" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
        "${REPO_ROOT}/bin/showy-quota" guard "$@"
}

# Pass: codex/primary has 95% remaining, well above a 5% floor.
rc=0
out=$(run_guard codexbar-mixed.json --provider codex --window primary --min-remaining 5 2>&1) || rc=$?
assert_equals "guard passes when remaining clears the floor" "0" "${rc}"
assert_contains "guard pass names codex/primary window" "codex/primary" "${out}"

# Breach: 95% remaining is below a 96% floor; human line names codex/primary.
rc=0
out=$(run_guard codexbar-mixed.json --provider codex --window primary --min-remaining 96 2>&1) || rc=$?
assert_equals "guard breaches when remaining is below the floor" "1" "${rc}"
assert_contains "guard breach names codex/primary window" "codex/primary" "${out}"
assert_contains "guard breach human line reads BREACH" "BREACH" "${out}"

# Inclusive-pass boundary: remaining == floor passes (catches a <= regression).
rc=0
out=$(run_guard codexbar-mixed.json --provider codex --window primary --min-remaining 95 2>&1) || rc=$?
assert_equals "guard boundary remaining == floor passes (inclusive)" "0" "${rc}"

# --json breach object exposes the decision fields.
rc=0
guard_json=$(run_guard codexbar-mixed.json --provider codex --window primary --min-remaining 96 --json) || rc=$?
assert_equals "guard --json breach exit code is 1" "1" "${rc}"
assert_equals "guard --json breach exit field" "1" "$(printf '%s' "${guard_json}" | jq -r '.exit')"
assert_equals "guard --json breach ok is false" "false" "$(printf '%s' "${guard_json}" | jq -r '.ok')"
assert_equals "guard --json breach reason is breach" "breach" "$(printf '%s' "${guard_json}" | jq -r '.reason')"
assert_equals "guard --json breach worst provider" "codex" "$(printf '%s' "${guard_json}" | jq -r '.worst.provider')"
assert_equals "guard --json breach worst window" "primary" "$(printf '%s' "${guard_json}" | jq -r '.worst.window')"

# Default worst window: codex primary (95) vs secondary (41) => the
# least-remaining slot (secondary) is chosen.
guard_json=$(run_guard codexbar-mixed.json --provider codex --json)
assert_equals "guard default window picks the least-remaining slot" "secondary" "$(printf '%s' "${guard_json}" | jq -r '.worst.window')"
assert_equals "guard default worst reports the low remaining" "41" "$(printf '%s' "${guard_json}" | jq -r '.worst.remainingPercent')"

# Default across all present providers: three are evaluated and the overall
# worst is codex/secondary (41), still passing the default 10% floor.
guard_json=$(run_guard codexbar-mixed.json --json)
assert_equals "guard default evaluates every present provider" "3" "$(printf '%s' "${guard_json}" | jq -r '.evaluated')"
assert_equals "guard default worst provider across providers" "codex" "$(printf '%s' "${guard_json}" | jq -r '.worst.provider')"

# Unknown provider is unusable (exit 2).
rc=0
out=$(run_guard codexbar-mixed.json --provider nope 2>&1) || rc=$?
assert_equals "guard unknown provider exits 2" "2" "${rc}"
assert_contains "guard unknown provider names the reason" "unknown-provider" "${out}"

# Errored provider is unusable (exit 2) with a provider-error reason.
rc=0
out=$(run_guard codexbar-error-only.json --provider cursor 2>&1) || rc=$?
assert_equals "guard errored provider exits 2" "2" "${rc}"
assert_contains "guard errored provider reports provider-error" "provider-error" "${out}"

# Mutually-exclusive thresholds are a usage error (exit 3).
rc=0
out=$(run_guard codexbar-mixed.json --min-remaining 10 --max-used 20 2>&1) || rc=$?
assert_equals "guard rejects both thresholds with usage exit 3" "3" "${rc}"
assert_contains "guard both-thresholds error explains the conflict" "mutually exclusive" "${out}"

# Oversized digit strings must fail before Bash converts them to machine integers.
rc=0
out=$(run_guard codexbar-mixed.json --no-fetch --min-remaining 18446744073709551616 2>&1) || rc=$?
assert_equals "guard rejects an overflowing remaining threshold with usage exit 3" "3" "${rc}"
assert_contains "guard reports the invalid remaining threshold" "invalid --min-remaining" "${out}"
rc=0
out=$(run_guard codexbar-mixed.json --no-fetch --max-used 18446744073709551616 2>&1) || rc=$?
assert_equals "guard rejects an overflowing used threshold with usage exit 3" "3" "${rc}"
rc=0
out=$(run_guard codexbar-mixed.json --no-fetch --wait-max 18446744073709551616 2>&1) || rc=$?
assert_equals "guard rejects an overflowing wait budget with usage exit 3" "3" "${rc}"

# --wait-max 0 never sleeps: an immediate breach exits 1 straight away
# (timing-based waits are intentionally not exercised).
rc=0
out=$(run_guard codexbar-mixed.json --provider codex --window primary --min-remaining 96 --wait-max 0 2>&1) || rc=$?
assert_equals "guard --wait-max 0 immediate breach exits 1" "1" "${rc}"

# --no-fetch reads an aged cache as-is and must not collect from codexbar.
# --allow-stale makes the cached decision evaluable while a marker-writing
# codexbar stub proves that neither guard nor state starts a provider refresh.
guard_no_fetch_cache=$(mk_cache)
seed_usage_cache "${guard_no_fetch_cache}" codexbar-mixed.json cli
touch -t 198801010000 "${guard_no_fetch_cache}/usage.json"
guard_no_fetch_marker="${TMP}/guard-no-fetch.marker"
guard_no_fetch_stub="${TMP}/guard-no-fetch-codexbar-stub"
cat > "${guard_no_fetch_stub}" <<EOF
#!/bin/sh
: > "${guard_no_fetch_marker}"
exit 1
EOF
chmod +x "${guard_no_fetch_stub}"
rm -f "${guard_no_fetch_marker}"
rc=0
out=$(
    env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${guard_no_fetch_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${guard_no_fetch_stub}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        "${REPO_ROOT}/bin/showy-quota" guard --provider codex --window primary \
            --min-remaining 5 --no-fetch --allow-stale 2>&1
) || rc=$?
assert_equals "guard --no-fetch evaluates the cached quota" "0" "${rc}"
assert_contains "guard --no-fetch reports the cached window" "codex/primary" "${out}"
if [[ -e "${guard_no_fetch_marker}" ]]; then
    fail "guard --no-fetch runs no provider collection" "marker present"
else
    ok "guard --no-fetch runs no provider collection"
fi

# Stale cache: an old-mtime cache under the default refresh window is unusable
# without --allow-stale, and evaluable with it. (Old mtime + missing codexbar =>
# the fetch cannot refresh, so the stale cache is what gets evaluated.)
guard_stale_cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${guard_stale_cache}/usage.json"
seed_usage_source "${guard_stale_cache}" "cli"
touch -t 198801010000 "${guard_stale_cache}/usage.json"
rc=0
out=$(
    env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${guard_stale_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-guard-stale" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        "${REPO_ROOT}/bin/showy-quota" guard --provider codex --window primary --min-remaining 5 2>&1
) || rc=$?
assert_equals "guard stale cache without --allow-stale exits 2" "2" "${rc}"
assert_contains "guard stale cache reports the stale reason" "stale" "${out}"
rc=0
out=$(
    env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${guard_stale_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-guard-stale" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        "${REPO_ROOT}/bin/showy-quota" guard --provider codex --window primary --min-remaining 5 --allow-stale 2>&1
) || rc=$?
assert_equals "guard stale cache with --allow-stale evaluates (exit 0)" "0" "${rc}"

# ── automation: quota prompt segment ────────────────────────────────────
# prompt prints "<SIGIL> <used>% <countdown>" for the worst-remaining window,
# delegating cache-only rendering to the native `showy-quota-render --emit prompt
# --from-cache` path. NOW_EPOCH pins the 2099-dated resets to deterministic
# countdowns; the huge refresh knob keeps the seeded healthy cache non-stale.
printf '\nquota prompt segment\n'

run_prompt() {
    local cache="$1"; shift
    env \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-prompt" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_NOW_EPOCH=4070908800 \
        SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
        "${REPO_ROOT}/bin/showy-quota" prompt "$@"
}

prompt_cache=$(mk_cache)
seed_usage_cache "${prompt_cache}" codexbar-mixed.json serve

# Worst window on the mixed fixture is codex/secondary: 59% used, ~7 days out.
rc=0
out=$(run_prompt "${prompt_cache}") || rc=$?
assert_equals "prompt exits 0 on a healthy cache" "0" "${rc}"
assert_equals "prompt renders the worst window segment" "CX 59% 7d" "${out}"

# --provider narrows to claude: its worst window is secondary (19% used, ~5d).
assert_equals "prompt --provider selects the requested provider" "CL 19% 5d" "$(run_prompt "${prompt_cache}" --provider claude)"

# Empty cache degrades to the neutral segment, still exit 0.
prompt_empty_cache=$(mk_cache)
rc=0
out=$(run_prompt "${prompt_empty_cache}") || rc=$?
assert_equals "prompt empty cache exits 0" "0" "${rc}"
assert_equals "prompt empty cache prints the neutral segment" "AI ?" "${out}"

# --ansi wraps the segment in a CSI severity color; NO_COLOR suppresses it.
prompt_ansi_bytes=$(
    env -u NO_COLOR SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${prompt_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-prompt" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
        "${REPO_ROOT}/bin/showy-quota" prompt --ansi | od -An -tx1 | tr -d ' \n'
)
assert_contains "prompt --ansi emits a CSI escape" "1b5b" "${prompt_ansi_bytes}"
prompt_noansi_bytes=$(
    env NO_COLOR=1 SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${prompt_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-prompt" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
        "${REPO_ROOT}/bin/showy-quota" prompt --ansi | od -An -tx1 | tr -d ' \n'
)
assert_not_contains "prompt --ansi with NO_COLOR emits no escape" "1b5b" "${prompt_noansi_bytes}"

prompt_stale_cache=$(mk_cache)
seed_usage_cache "${prompt_stale_cache}" codexbar-mixed.json serve
touch -t 202001010000 "${prompt_stale_cache}/usage.json"
out=$(
    env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${prompt_stale_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-prompt" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_NOW_EPOCH=4070908800 \
        "${REPO_ROOT}/bin/showy-quota" prompt
)
assert_contains "prompt stale cache appends stale glyph" "⚠" "${out}"

# prompt must never refresh: point codexbar at a marker-writing stub and confirm
# the default (cache-only) path never executes it.
prompt_marker_cache=$(mk_cache)
seed_usage_cache "${prompt_marker_cache}" codexbar-mixed.json serve
prompt_marker="${TMP}/prompt-no-refresh.marker"
prompt_stub="${TMP}/prompt-codexbar-stub"
cat > "${prompt_stub}" <<EOF
#!/bin/sh
: > "${prompt_marker}"
exit 1
EOF
chmod +x "${prompt_stub}"
rm -f "${prompt_marker}"
out=$(
    env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${prompt_marker_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${prompt_stub}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
        "${REPO_ROOT}/bin/showy-quota" prompt
)
assert_contains "prompt cache-only path still renders" "CX" "${out}"
if [[ -e "${prompt_marker}" ]]; then
    fail "prompt does not refresh the cache (no codexbar collection)" "marker present"
else
    ok "prompt does not refresh the cache (no codexbar collection)"
fi

# ── state --no-fetch (cache-only) ───────────────────────────────────────
# --no-fetch reports the cache as-is (fetch --cache-only), never collecting from
# codexbar. Seed an aged cache, prove the state surface reflects it, and prove
# no provider collection runs via the same marker-stub technique.
printf '\nstate --no-fetch\n'

state_nf_cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${state_nf_cache}/usage.json"
seed_usage_source "${state_nf_cache}" "cli"
touch -t 198801010000 "${state_nf_cache}/usage.json"
state_nf_marker="${TMP}/state-no-fetch.marker"
state_nf_stub="${TMP}/state-codexbar-stub"
cat > "${state_nf_stub}" <<EOF
#!/bin/sh
: > "${state_nf_marker}"
exit 1
EOF
chmod +x "${state_nf_stub}"
rm -f "${state_nf_marker}"
state_nf_json=$(
    env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${state_nf_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${state_nf_stub}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json
)
assert_equals "state --no-fetch reports the seeded cache as available" "true" "$(printf '%s' "${state_nf_json}" | jq -r '.available')"
assert_equals "state --no-fetch exposes cache source" "cli" "$(printf '%s' "${state_nf_json}" | jq -r '.cache.source')"
assert_equals "state --no-fetch exposes cache degraded flag" "true" "$(printf '%s' "${state_nf_json}" | jq -r '.cache.degraded')"
assert_equals "state --no-fetch exposes numeric cache age" "number" "$(printf '%s' "${state_nf_json}" | jq -r '.cacheAgeSeconds | type')"
assert_equals "state --no-fetch surfaces the cached providers" "codex,claude,gemini" "$(printf '%s' "${state_nf_json}" | jq -r '.providers | join(",")')"
if [[ -e "${state_nf_marker}" ]]; then
    fail "state --no-fetch runs no provider collection" "marker present"
else
    ok "state --no-fetch runs no provider collection"
fi

# ── agent-CLI statusline adapter ─────────────────────────────────────────
# The adapter drains stdin, wraps showy-quota-zellij-bar with statusline
# defaults (width 8, Powerline caps off), resolves the bar via SHOWY_QUOTA_BAR_BIN
# and degrades to "AI ?" when no bar can be found. It never hard-fails.
printf '\nagent-CLI statusline adapter\n'

STATUSLINE_BIN="${REPO_ROOT}/adapters/agent-cli/showy-quota-statusline"

run_statusline() {
    local cache; cache=$(mk_cache)
    cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
    seed_usage_source "${cache}" "cli"
    printf '%s' '{"session_id":"showy-quota-test"}' | env \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-statusline" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
        SHOWY_QUOTA_NOW_EPOCH=4070908800 \
        SHOWY_QUOTA_DEGRADED_CLI=0 \
        SHOWY_QUOTA_FORCE_COLOR=1 \
        "$@" \
        "${STATUSLINE_BIN}"
}

# Piped stdin JSON is drained and the seeded cache renders a real strip.
rc=0
out=$(run_statusline) || rc=$?
assert_equals "statusline exits 0 with a seeded cache" "0" "${rc}"
assert_contains "statusline renders the provider strip from stdin+cache" "CX" "${out}"

# Powerline end caps (U+E0B6 = ee82b6, U+E0B4 = ee82b4) are inherited from the
# bar's defaults (on, like every other strip) and dropped by
# SHOWY_QUOTA_STATUSLINE_CAPS=0 for plain-font hosts.
statusline_default_bytes=$(run_statusline | od -An -tx1 | tr -d ' \n')
assert_contains "statusline inherits the left powerline cap by default" "ee82b6" "${statusline_default_bytes}"
assert_contains "statusline inherits the right powerline cap by default" "ee82b4" "${statusline_default_bytes}"
statusline_caps_bytes=$(run_statusline SHOWY_QUOTA_STATUSLINE_CAPS=0 | od -An -tx1 | tr -d ' \n')
assert_not_contains "statusline caps=0 drops the left powerline cap" "ee82b6" "${statusline_caps_bytes}"
assert_not_contains "statusline caps=0 drops the right powerline cap" "ee82b4" "${statusline_caps_bytes}"

# Width/caps env contract, isolated from the renderer via a reporting bar stub
# that echoes the env the adapter hands it (proving SHOWY_QUOTA_BAR_BIN resolution).
statusline_stub="${TMP}/statusline-bar-stub"
cat > "${statusline_stub}" <<'EOF'
#!/bin/sh
cat >/dev/null 2>&1 || true
printf 'width=%s capL=[%s] capR=[%s]\n' "${SHOWY_QUOTA_ZELLIJ_BAR_WIDTH:-unset}" "${SHOWY_QUOTA_CAP_LEFT-UNSET}" "${SHOWY_QUOTA_CAP_RIGHT-UNSET}"
EOF
chmod +x "${statusline_stub}"
run_statusline_stub() {
    printf '%s' '{}' | env \
        SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_BAR_BIN="${statusline_stub}" \
        "$@" \
        "${STATUSLINE_BIN}"
}
assert_equals "statusline width knob overrides the default" "width=5 capL=[UNSET] capR=[UNSET]" "$(run_statusline_stub SHOWY_QUOTA_STATUSLINE_WIDTH=5)"
assert_equals "statusline explicit bar width wins over the knob" "width=11 capL=[UNSET] capR=[UNSET]" "$(run_statusline_stub SHOWY_QUOTA_ZELLIJ_BAR_WIDTH=11 SHOWY_QUOTA_STATUSLINE_WIDTH=5)"
assert_equals "statusline default leaves the bar cap defaults untouched" "width=8 capL=[UNSET] capR=[UNSET]" "$(run_statusline_stub)"
assert_equals "statusline caps=0 blanks the caps for plain-font hosts" "width=8 capL=[] capR=[]" "$(run_statusline_stub SHOWY_QUOTA_STATUSLINE_CAPS=0)"

# Unresolvable bar => neutral "AI ?" segment, exit 0. Isolate a copy of the
# adapter from its sibling bin/ and clear PATH so no bar can be found.
statusline_iso_dir="${TMP}/statusline-isolated"
mkdir -p "${statusline_iso_dir}"
cp "${STATUSLINE_BIN}" "${statusline_iso_dir}/showy-quota-statusline"
chmod +x "${statusline_iso_dir}/showy-quota-statusline"
rc=0
out=$(printf '%s' '{}' | env -i PATH="/usr/bin:/bin" SHOWY_QUOTA_BAR_BIN=/nonexistent "${statusline_iso_dir}/showy-quota-statusline") || rc=$?
assert_equals "statusline missing bar exits 0" "0" "${rc}"
assert_equals "statusline missing bar prints the neutral segment" "AI ?" "${out}"

# ── statusline forwards argv (showy-quota-5a0c988086e20528 / showy-quota-68a579f4c1af8889) ──
# The wrapper's final `exec` previously called the bar binary with a fixed,
# empty argv, so flags passed to the statusline adapter — `-h`/`--help`
# included — were silently swallowed instead of reaching the renderer.
# `exec "${BAR_BIN}" "$@"` must forward every argument through untouched.
printf '\nstatusline argv forwarding\n'

rc=0
out=$(printf '%s' '{}' | env SHOWY_QUOTA_NO_CONFIG=1 "${STATUSLINE_BIN}" --help) || rc=$?
assert_equals "statusline --help exits 0 through the wrapper" "0" "${rc}"
assert_contains "statusline --help forwards through to the renderer's usage text" "Usage: showy-quota-zellij-bar" "${out}"

# ── guard --wait-max never exceeds the caller's budget (showy-quota-b0578f246f2cb64b) ──
# The wait branch previously slept (secondsUntilReset + 30) whenever that
# fell inside the gate (secondsUntilReset <= wait_max), so a near-immediate
# reset next to a small --wait-max could overrun the caller's stated budget
# by up to 30s. Pin a fixture whose only window resets a few seconds from
# now (minutesUntilReset floors to 0 either way) and assert the full guard
# invocation — forced refresh, retry, and all — completes inside the budget
# plus a small process-overhead allowance, well under the old 30s overrun.
# run_with_test_timeout bounds the case so a misbehaving branch cannot hang.
printf '\nguard --wait-max budget\n'

wait_max_reset_at=$(python3 -c '
import datetime
now = datetime.datetime.now(datetime.timezone.utc)
print((now + datetime.timedelta(seconds=20)).strftime("%Y-%m-%dT%H:%M:%SZ"))
')

wait_max_fixture="${TMP}/wait-max-budget.json"
cat > "${wait_max_fixture}" <<JSON
[
  {
    "provider": "waitmax",
    "source": "api",
    "usage": {
      "primary":   { "usedPercent": 100, "windowMinutes": 300, "resetsAt": "${wait_max_reset_at}" },
      "secondary": null,
      "tertiary":  null
    }
  }
]
JSON

wait_max_cache=$(mk_cache)
cp "${wait_max_fixture}" "${wait_max_cache}/usage.json"
seed_usage_source "${wait_max_cache}" "cli"

wait_max_budget=5
wait_max_start=$(date +%s)
rc=0
out=$(
    run_with_test_timeout 20 env \
        SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${wait_max_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${TMP}/no-such-codexbar-wait-max" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
        "${REPO_ROOT}/bin/showy-quota" guard --provider waitmax --window primary \
            --min-remaining 10 --wait-max "${wait_max_budget}" 2>&1
) || rc=$?
wait_max_elapsed=$(( $(date +%s) - wait_max_start ))

assert_equals "guard --wait-max budget run breaches again after the retry (no hang/crash)" "1" "${rc}"
if (( wait_max_elapsed <= wait_max_budget + 5 )); then
    ok "guard --wait-max ${wait_max_budget} stays within budget (elapsed ${wait_max_elapsed}s, old grace would have taken ~30s)"
else
    fail "guard --wait-max ${wait_max_budget} stays within budget" "elapsed ${wait_max_elapsed}s exceeds budget+overhead; out=${out}"
fi

# ── canonical boolean parsing (showy_quota_bool) ────────────────────────
printf '\ncanonical boolean parsing\n'

bool_true_values=("true" "TRUE" " yes " "on" "1")
bool_true_results=()
for bool_value in "${bool_true_values[@]}"; do
    # shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
    bool_true_results+=("$(run_common_eval 'showy_quota_bool "${SHOWY_QUOTA_TEST_VALUE}" 0 && printf true || printf false' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_VALUE="${bool_value}")")
done
assert_equals "showy_quota_bool recognizes true spellings" "true,true,true,true,true" "$(IFS=,; printf '%s' "${bool_true_results[*]}")"

bool_false_values=("false" "no" "off" "0")
bool_false_results=()
for bool_value in "${bool_false_values[@]}"; do
    # shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
    bool_false_results+=("$(run_common_eval 'showy_quota_bool "${SHOWY_QUOTA_TEST_VALUE}" 1 && printf true || printf false' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_VALUE="${bool_value}")")
done
assert_equals "showy_quota_bool recognizes false spellings" "false,false,false,false" "$(IFS=,; printf '%s' "${bool_false_results[*]}")"

# shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
out=$(run_common_eval 'showy_quota_bool "${SHOWY_QUOTA_TEST_VALUE}" 1 && printf true || printf false' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_VALUE=)
assert_equals "showy_quota_bool falls back to default=true on empty value" "true" "${out}"
# shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
out=$(run_common_eval 'showy_quota_bool "${SHOWY_QUOTA_TEST_VALUE}" 0 && printf true || printf false' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_VALUE=)
assert_equals "showy_quota_bool falls back to default=false on empty value" "false" "${out}"
# shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
out=$(run_common_eval 'showy_quota_bool "${SHOWY_QUOTA_TEST_VALUE}" 1 && printf true || printf false' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_VALUE=garbage)
assert_equals "showy_quota_bool falls back to default=true on garbage" "true" "${out}"
# shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
out=$(run_common_eval 'showy_quota_bool "${SHOWY_QUOTA_TEST_VALUE}" 0 && printf true || printf false' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_VALUE=garbage)
assert_equals "showy_quota_bool falls back to default=false on garbage" "false" "${out}"

# ── provider id length parity (fetcher validation path) ─────────────────
printf '\nprovider id length parity\n'

provider_id_64=$(printf '%64s' '' | tr ' ' 'p')
provider_id_65=$(printf '%65s' '' | tr ' ' 'p')
# shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
out=$(run_common_eval 'showy_quota_valid_provider_id "${SHOWY_QUOTA_TEST_ID}" && printf valid || printf reject' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_ID="${provider_id_64}")
assert_equals "64-char provider id accepted by the shared fetcher validator" "valid" "${out}"
# shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
out=$(run_common_eval 'showy_quota_valid_provider_id "${SHOWY_QUOTA_TEST_ID}" && printf valid || printf reject' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_TEST_ID="${provider_id_65}")
assert_equals "65-char provider id rejected by the shared fetcher validator" "reject" "${out}"

# ── malformed palette hex degrades instead of dying (showy-quota-00685b1ec34e8659) ──
# showy_quota_primary_palette/dim_palette/palette previously normalized their
# hex inputs with the FATAL showy_quota_normalize_hex, so one bad
# SHOWY_QUOTA_PALETTE_* override killed every shell render. They now degrade
# to that knob's documented default (warning to stderr) and exit 0.
printf '\nmalformed palette hex degrades instead of dying\n'

hex_fallback_err="${TMP}/hex-fallback.err"

rc=0
out=$(run_common_eval 'showy_quota_primary_palette good' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_PALETTE_PRIMARY_GOOD=nothex 2>"${hex_fallback_err}") || rc=$?
assert_equals "malformed primary override exits 0 instead of dying" "0" "${rc}"
assert_equals "malformed primary override degrades to the documented default" "25be6a" "${out}"
assert_contains "malformed primary override warns to stderr" "invalid palette hex" "$(cat "${hex_fallback_err}")"

rc=0
out=$(run_common_eval 'showy_quota_dim_palette good' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_PALETTE_PRIMARY_GOOD=nothex 2>"${hex_fallback_err}") || rc=$?
assert_equals "malformed primary feeding dim palette exits 0 instead of dying" "0" "${rc}"
assert_equals "malformed primary feeding dim palette degrades before scale_hex" "14683a" "${out}"
assert_contains "malformed primary feeding dim palette warns to stderr" "invalid palette hex" "$(cat "${hex_fallback_err}")"

rc=0
out=$(run_common_eval 'showy_quota_dim_palette good' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_PALETTE_DIM_GOOD=nothex 2>"${hex_fallback_err}") || rc=$?
assert_equals "malformed dim override exits 0 instead of dying" "0" "${rc}"
assert_equals "malformed dim override degrades to the unset-override computed scale" "14683a" "${out}"
assert_contains "malformed dim override warns to stderr" "invalid palette hex" "$(cat "${hex_fallback_err}")"

rc=0
out=$(run_common_eval 'showy_quota_palette bg' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_PALETTE_BG=nothex 2>"${hex_fallback_err}") || rc=$?
assert_equals "malformed global palette token exits 0 instead of dying" "0" "${rc}"
assert_equals "malformed global palette token degrades to its documented default" "161616" "${out}"
assert_contains "malformed global palette token warns to stderr" "invalid palette hex" "$(cat "${hex_fallback_err}")"

# shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
rc=0
out=$(run_common_eval 'showy_quota_primary_palette good; showy_quota_primary_palette good' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_PALETTE_PRIMARY_GOOD=nothex 2>"${hex_fallback_err}") || rc=$?
assert_equals "malformed primary override fallback is memoised across repeated calls" "25be6a25be6a" "${out}"
assert_equals "malformed primary override warns only once (memoisation)" "1" "$(grep -c "invalid palette hex" "${hex_fallback_err}")"

# A full render depends on showy_quota_window_color (the shared severity/dim
# color lookup every renderer calls) to compute a row's color; previously a
# malformed override there hard-killed the render (exit 1) instead of
# degrading. Exercise it directly, unwrapped, exactly as production callers
# invoke it in `set -euo pipefail` scripts.
rc=0
out=$(run_common_eval 'showy_quota_window_color 50 0' SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_PALETTE_PRIMARY_GOOD=nothex 2>"${hex_fallback_err}") || rc=$?
assert_equals "full render color lookup survives a malformed palette override (previously exit 1)" "0" "${rc}"
assert_equals "full render color lookup still returns the documented default" "25be6a" "${out}"

# The real SketchyBar plugin renders end-to-end with the same malformed
# override in its environment, proving the fix holds for a production
# renderer, not just the helper functions in isolation.
sb_hex_fixture=$(mk_cache)
sb_hex_log="${TMP}/sb-hex-fallback.log"
run_sketchybar_plugin codexbar-mixed.json "${sb_hex_fixture}" "${sb_hex_log}" SHOWY_QUOTA_PALETTE_PRIMARY_WARN=nothex
sb_hex_rc=$?
assert_equals "sketchybar plugin full render survives a malformed palette override" "0" "${sb_hex_rc}"

printf '\ncache envelope (payload+source atomicity)\n'

# The fetcher must publish payload and source metadata in ONE envelope
# object, via a single atomic rename, so a reader can never observe a NEW
# payload paired with STALE (or absent) source metadata. No separate
# `source` file may exist afterward.
envelope_publish_cache=$(mk_cache)
rc=0
envelope_publish_out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${envelope_publish_cache}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-fetch" 2>/dev/null
) || rc=$?
envelope_publish_raw="$(< "${envelope_publish_cache}/usage.json")"
if (( rc == 0 )) \
    && printf '%s' "${envelope_publish_out}" | jq -e 'type == "array" and any(.provider == "codex")' >/dev/null 2>&1 \
    && printf '%s' "${envelope_publish_raw}" | jq -e '
        type == "object"
        and .schema == "showy-quota/cache@2"
        and (has("source") and (.source | type) == "string")
        and (has("providers") and (.providers | type) == "array")
        and (has("providerMeta") and (.providerMeta | type) == "object")
    ' >/dev/null 2>&1 \
    && [[ ! -e "${envelope_publish_cache}/source" ]]; then
    ok "fetcher publishes one envelope with source+providers and no legacy source file"
else
    fail "fetcher publishes one envelope with source+providers and no legacy source file" \
        "rc=${rc}; out=${envelope_publish_out}; usage.json=${envelope_publish_raw}"
fi

# A successful publish must leave no staging files behind. The previous
# provider-metadata stage leaked one `usage.prev-meta.*` file per publish.
envelope_leftovers=$(compgen -G "${envelope_publish_cache}/usage.prev-meta.*"; compgen -G "${envelope_publish_cache}/usage-stamp.*"; compgen -G "${envelope_publish_cache}/usage.??????") || true
assert_equals "fetcher publish leaves no staging temp files in the cache dir" "" "${envelope_leftovers}"

# showy-quota-state reads the source marker out of the envelope, not a
# separate file.
state_source_cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${state_source_cache}/usage.json"
seed_usage_source "${state_source_cache}" "serve"
state_source_json=$(
    SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
    SHOWY_QUOTA_CACHE_DIR="${state_source_cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json
)
assert_equals "state reports the envelope's source marker" "serve" \
    "$(printf '%s' "${state_source_json}" | jq -r '.cache.source')"

# A pre-existing legacy bare-array cache (written by a prior version) is
# still rendered, with source reported as "unknown", and self-heals into an
# envelope on the next successful refresh.
legacy_cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${legacy_cache}/usage.json"
legacy_json=$(
    SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
    SHOWY_QUOTA_CACHE_DIR="${legacy_cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json
)
assert_equals "legacy bare-array cache still renders as available" "true" \
    "$(printf '%s' "${legacy_json}" | jq -r '.available')"
assert_equals "legacy bare-array cache reports source unknown" "unknown" \
    "$(printf '%s' "${legacy_json}" | jq -r '.cache.source')"

rc=0
legacy_refresh_out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${legacy_cache}" \
    SHOWY_QUOTA_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    "${REPO_ROOT}/bin/showy-quota-fetch" --refresh 2>/dev/null
) || rc=$?
legacy_after_refresh="$(< "${legacy_cache}/usage.json")"
if (( rc == 0 )) && printf '%s' "${legacy_after_refresh}" | jq -e 'type == "object" and has("source") and has("providers")' >/dev/null 2>&1; then
    ok "legacy bare-array cache upgrades to an envelope after one refresh"
else
    fail "legacy bare-array cache upgrades to an envelope after one refresh" \
        "rc=${rc}; out=${legacy_refresh_out}; usage.json=${legacy_after_refresh}"
fi

# showy_quota_json_valid accepts either shape, an empty provider array is a
# real published state, and mixed valid/invalid records stay tolerant;
# anything that is neither an array nor an envelope is rejected.
json_valid_case() {
    local desc="$1" content="$2" expect="$3" f
    f="${TMP}/json-valid-case.json"
    printf '%s' "${content}" > "${f}"
    # shellcheck disable=SC2016  # eval body; $-vars expand in run_common_eval's sub-shell
    assert_equals "${desc}" "${expect}" "$(
        run_common_eval 'showy_quota_json_valid "${SHOWY_QUOTA_TEST_JSON_VALID_FILE}" && printf ok || printf bad' \
            SHOWY_QUOTA_TEST_JSON_VALID_FILE="${f}"
    )"
}
json_valid_case "showy_quota_json_valid accepts envelope with empty providers array" '{"providers":[]}' ok
json_valid_case "showy_quota_json_valid accepts bare empty array" '[]' ok
json_valid_case "showy_quota_json_valid accepts mixed valid+invalid envelope" \
    '{"providers":[{"provider":"codex"},{"provider":"bad id"}]}' ok
json_valid_case "showy_quota_json_valid rejects non-array non-envelope" '42' bad

# Contract-level proof the race is closed: the published usage.json ALONE
# determines the reported source. Creating or deleting a stray legacy
# `source` file (as a pre-upgrade install might still have lying around)
# must have NO effect, because there is no second file left to race against.
race_closed_cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${race_closed_cache}/usage.json"
seed_usage_source "${race_closed_cache}" "serve"
state_source_for_cache() {
    SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_MANAGE_SERVE=0 \
        SHOWY_QUOTA_CACHE_DIR="${race_closed_cache}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json | jq -r '.cache.source'
}
race_source_before="$(state_source_for_cache)"
printf 'cli\n' > "${race_closed_cache}/source"
race_source_with_stray_file="$(state_source_for_cache)"
rm -f -- "${race_closed_cache}/source"
race_source_after_delete="$(state_source_for_cache)"
if [[ "${race_source_before}" == "serve" \
    && "${race_source_with_stray_file}" == "serve" \
    && "${race_source_after_delete}" == "serve" ]]; then
    ok "cache.source is immune to a stray legacy source file (race closed at the contract level)"
else
    fail "cache.source is immune to a stray legacy source file (race closed at the contract level)" \
        "before=${race_source_before}; with_stray_file=${race_source_with_stray_file}; after_delete=${race_source_after_delete}"
fi


# ── per-provider cache freshness ─────────────────────────────────────

printf '\nper-provider cache freshness\n'

# A provider whose own CLI fetch fails is carried forward from the previous
# cache. The publish refreshes the file's mtime, so without per-slice
# metadata that preserved record would claim to be as new as the fetch that
# republished it.
freshness_dir="${TMP}/per-provider-freshness"
mkdir -p "${freshness_dir}"
cat > "${freshness_dir}/codexbar" <<EOF
#!/bin/sh
if [ "\${1:-}" = "config" ] && [ "\${2:-}" = "providers" ]; then
    jq '[.[] | {provider, enabled: true}]' < "${FIXTURE_DIR}/codexbar-mixed.json"
    exit 0
fi
provider=""
while [ "\$#" -gt 0 ]; do
    case "\$1" in --provider) shift; provider="\${1:-}" ;; esac
    shift
done
if [ "\${provider}" = "\${SHOWY_QUOTA_TEST_FAIL_PROVIDER:-}" ]; then exit 7; fi
jq --arg p "\${provider}" '[.[] | select(.provider == \$p)]' < "${FIXTURE_DIR}/codexbar-mixed.json"
EOF
chmod +x "${freshness_dir}/codexbar"

freshness_cache=$(mk_cache)
freshness_now=$(date +%s)
freshness_then=$(( freshness_now - 1000 ))
run_freshness_fetch() {
    env SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${freshness_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN="${freshness_dir}/codexbar" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_REFRESH_SECONDS=60 \
        "$@" \
        "${REPO_ROOT}/bin/showy-quota-fetch" --refresh >/dev/null 2>&1
}
run_freshness_fetch SHOWY_QUOTA_NOW_EPOCH="${freshness_then}"
run_freshness_fetch SHOWY_QUOTA_NOW_EPOCH="${freshness_now}" SHOWY_QUOTA_TEST_FAIL_PROVIDER=gemini

freshness_meta=$(jq -c '.providerMeta' "${freshness_cache}/usage.json" 2>/dev/null)
assert_equals "fresh provider slices carry the publish timestamp" "${freshness_now}" \
    "$(printf '%s' "${freshness_meta}" | jq -r '.codex.updatedAt')"
assert_equals "a carried-forward slice keeps the timestamp of the fetch that measured it" \
    "${freshness_then}" "$(printf '%s' "${freshness_meta}" | jq -r '.gemini.updatedAt')"

freshness_state=$(
    env SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${freshness_cache}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_REFRESH_SECONDS=60 \
        SHOWY_QUOTA_NOW_EPOCH="${freshness_now}" \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json
)
assert_equals "the cache as a whole is reported fresh" "false" \
    "$(printf '%s' "${freshness_state}" | jq -r '.stale')"
assert_equals "state marks the carried-forward provider stale" "true" \
    "$(printf '%s' "${freshness_state}" | jq -r '.providerFreshness.gemini.stale')"
assert_equals "state leaves the freshly fetched providers alone" "false" \
    "$(printf '%s' "${freshness_state}" | jq -r '.providerFreshness.codex.stale')"
assert_equals "state reports the carried-forward slice's age" "1000" \
    "$(printf '%s' "${freshness_state}" | jq -r '.providerFreshness.gemini.ageSeconds')"

# The renderer reads the same metadata straight from the cache, so the strip
# greys exactly one chunk while the rest keep their bands.
freshness_args=$(
    env SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${freshness_cache}" \
        SHOWY_QUOTA_REFRESH_SECONDS=60 \
        SHOWY_QUOTA_NOW_EPOCH="${freshness_now}" \
        "${RENDER_BIN}" --emit sketchybar-frame --from-cache 2>/dev/null \
        | awk -F'\037' '$1 == "set"' | tr '\037' '\n'
)
# The primary slider's highlight in the `sketchybar --set` arguments.
primary_highlight() {
    printf '%s\n' "${freshness_args}" | awk -v item="showy_quota.$1.primary" '
        $0 == item { found = 1; next }
        found && $0 == "--set" { exit }
        found && sub(/^slider\.highlight_color=/, "") { print; exit }'
}
freshness_gemini_row=$(primary_highlight gemini)
freshness_codex_row=$(primary_highlight codex)
assert_equals "the carried-forward provider renders in the stale colour" "0xff6c7086" \
    "${freshness_gemini_row}"
# A missing row must not pass as "kept its color": the awk lookup yields an
# empty string when the renderer drops the provider entirely.
if [[ -z "${freshness_codex_row}" ]]; then
    fail "a fresh provider keeps its own band beside a stale one" "codex row missing from sketchybar rows"
elif [[ "${freshness_codex_row}" == "0xff6c7086" ]]; then
    fail "a fresh provider keeps its own band beside a stale one" "codex highlight=${freshness_codex_row}"
else
    ok "a fresh provider keeps its own band beside a stale one"
fi
empty_reason_for_state() {
    local fixture="$1"
    shift
    # Seed the cache directly: a refresh applies the provider allow-list at
    # fetch time (`apply_provider_filters`), so the "your filters hide live
    # data" case is only observable when the cache already holds providers.
    local cache
    cache=$(mk_cache)
    cp "$(fixture_path "${fixture}")" "${cache}/usage.json"
    env SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${cache}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        "$@" \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json | jq -r '.emptyReason'
}
legacy_freshness_cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${legacy_freshness_cache}/usage.json"
seed_usage_source "${legacy_freshness_cache}" "serve"
legacy_freshness_state=$(
    env SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${legacy_freshness_cache}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_REFRESH_SECONDS=9999999 \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json
)
assert_equals "a legacy envelope still reports every provider" "claude,codex,cursor,gemini" \
    "$(printf '%s' "${legacy_freshness_state}" | jq -r '[.providerFreshness | keys[]] | sort | join(",")')"
assert_equals "a legacy envelope reports no per-provider staleness" "false" \
    "$(printf '%s' "${legacy_freshness_state}" | jq -r '[.providerFreshness[] | .stale] | any')"

# ── empty-state reasons ──────────────────────────────────────────────

printf '\nempty-state reasons\n'

assert_equals "an empty CodexBar inventory is reported as no-providers" "no-providers" \
    "$(empty_reason_for_state codexbar-empty.json)"

assert_equals "a filter that removes every provider is reported as filtered" "filtered" \
    "$(empty_reason_for_state codexbar-mixed.json SHOWY_QUOTA_PROVIDERS=nothing-matches)"
assert_equals "a rendered strip reports no empty reason" "null" \
    "$(empty_reason_for_state codexbar-mixed.json)"
assert_equals "error-only providers still count as rendered" "null" \
    "$(empty_reason_for_state codexbar-error-only.json)"
assert_equals "a missing cache is reported as unavailable" "unavailable" \
    "$(SHOWY_QUOTA_CACHE_DIR="${TMP}/sq-no-such-cache" SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CODEXBAR_SERVE_URL='' "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json | jq -r '.emptyReason')"
idle_reason_cache=$(mk_cache)
printf '[{"provider":"codex","usage":{"primary":{"usedPercent":null}}}]' > "${idle_reason_cache}/usage.json"
assert_equals "unused quota is reported as idle" "idle" \
    "$(SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${idle_reason_cache}" SHOWY_QUOTA_CODEXBAR_SERVE_URL='' "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json | jq -r '.emptyReason')"
# Muse Code while its server withholds quota: no error and no window object.
# It draws greyed in place, so the strip is not empty.
printf '[{"provider":"muse","error":null,"usage":{"primary":null,"secondary":null,"tertiary":null}}]' > "${idle_reason_cache}/usage.json"
assert_equals "withheld quota counts as a rendered provider" "null" \
    "$(SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${idle_reason_cache}" SHOWY_QUOTA_CODEXBAR_SERVE_URL='' "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json | jq -r '.emptyReason')"

# The strip label and the state token must agree; they are derived
# independently (Rust renderer vs jq) and drift would send users hunting the
# wrong problem.
empty_label_cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-empty.json" "${empty_label_cache}/usage.json"
empty_label_strip=$(
    env SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${empty_label_cache}" \
        "${RENDER_BIN}" --from-cache 2>/dev/null
)
if [[ "${empty_label_strip}" == *"AI none"* ]]; then
    ok "the strip says AI none when CodexBar has no providers"
else
    fail "the strip says AI none when CodexBar has no providers" "strip=${empty_label_strip}"
fi

filtered_label_cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${filtered_label_cache}/usage.json"
filtered_label_strip=$(
    env SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${filtered_label_cache}" \
        SHOWY_QUOTA_PROVIDERS=nothing-matches \
        "${RENDER_BIN}" --from-cache 2>/dev/null
)
if [[ "${filtered_label_strip}" == *"AI filtered"* ]]; then
    ok "the strip says AI filtered when the user's own filters emptied it"
else
    fail "the strip says AI filtered when the user's own filters emptied it" "strip=${filtered_label_strip}"
fi

printf '\nmanaged serve controls\n'

serve_control_cache=$(mk_cache)
serve_control_stub="${TMP}/serve-control-stub"
mkdir -p "${serve_control_stub}"
cat > "${serve_control_stub}/curl" <<'EOF'
#!/usr/bin/env bash
url="${*: -1}"
printf '%s\n' "${url}" >> "${SHOWY_QUOTA_TEST_STATUS_CURL_LOG}"
[[ "${url}" == "${SHOWY_QUOTA_TEST_SERVE_URL}/health" ]] || exit 91
printf '{"version":"CodexBar v9.8.7"}\n'
EOF
cat > "${serve_control_stub}/ps" <<'EOF'
#!/usr/bin/env bash
if [[ "${2:-}" == "${SHOWY_QUOTA_TEST_STATUS_PID:-none}" ]]; then
    case "${4:-}" in
        lstart=) printf 'Mon Jan  1 00:00:00 2024\n'; exit 0 ;;
        command=) printf 'codexbar serve --port 18183\n'; exit 0 ;;
    esac
fi
exec /bin/ps "$@"
EOF
cat > "${serve_control_stub}/lsof" <<'EOF'
#!/usr/bin/env bash
[[ "$*" == *'tcp:18183'* ]] || exit 91
printf '%s\n' "${SHOWY_QUOTA_TEST_STATUS_LISTENER_PID:-99999999}"
EOF
chmod +x "${serve_control_stub}/curl" "${serve_control_stub}/ps" "${serve_control_stub}/lsof"
serve_control_url='http://127.0.0.1:18183'
serve_control_log="${TMP}/serve-control-curl.log"
: > "${serve_control_log}"
serve_control_status() {
    env TZ=UTC PATH="${serve_control_stub}:${PATH}" SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${serve_control_cache}" \
        SHOWY_QUOTA_CODEXBAR_BIN=codexbar \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_control_url}" \
        SHOWY_QUOTA_TEST_SERVE_URL="${serve_control_url}" \
        SHOWY_QUOTA_TEST_STATUS_CURL_LOG="${serve_control_log}" \
        SHOWY_QUOTA_NOW_EPOCH=1704067300 \
        "$@" "${REPO_ROOT}/bin/showy-quota" serve status --json
}
serve_control_rc=0
serve_control_out=$(serve_control_status env) || serve_control_rc=$?
if (( serve_control_rc == 1 )) && printf '%s' "${serve_control_out}" |
    jq -e '.healthy == false and .managed == {pidfileState:"missing",pid:null,alive:false,owned:false}
        and .serve.localOnly == true and .serve.port == 18183
        and .serve.healthReachable == true and .serve.version == "9.8.7"
        and .failure.count == 0 and .cache.ageSeconds == null' >/dev/null; then
    ok "serve status reports a reachable unmanaged responder without creating a pidfile"
else
    fail "serve status reports a reachable unmanaged responder without creating a pidfile" "rc=${serve_control_rc}; ${serve_control_out}"
fi
if [[ ! -e "${serve_control_cache}/codexbar-serve.pid" ]] && [[ "$(< "${serve_control_log}")" != *'/usage'* ]]; then
    ok "serve status does not modify the pidfile or request /usage"
else
    fail "serve status does not modify the pidfile or request /usage"
fi
serve_control_absent="${TMP}/serve-control-absent"
serve_control_rc=0
serve_control_out=$(env PATH="${serve_control_stub}:${PATH}" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${serve_control_absent}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_control_url}" \
    SHOWY_QUOTA_TEST_SERVE_URL="${serve_control_url}" \
    SHOWY_QUOTA_TEST_STATUS_CURL_LOG="${serve_control_log}" \
    "${REPO_ROOT}/bin/showy-quota" serve status) || serve_control_rc=$?
if (( serve_control_rc == 1 )) && [[ ! -e "${serve_control_absent}" ]] \
    && [[ "${serve_control_out}" == *'managed serve: missing'* ]] \
    && [[ "${serve_control_out}" == *'/health: true'* ]]; then
    ok "serve status text reports missing management without creating a cache directory"
else
    fail "serve status text reports missing management without creating a cache directory" "rc=${serve_control_rc}; ${serve_control_out}"
fi
seed_usage_cache "${serve_control_cache}" codexbar-empty.json cli
printf '%s\n' '99999999:1704067200:codexbar' > "${serve_control_cache}/codexbar-serve.pid"
printf '4\n' > "${serve_control_cache}/serve-failed-count"
printf '1704067285\n' > "${serve_control_cache}/serve-failed-at"
serve_control_rc=0
serve_control_out=$(serve_control_status env) || serve_control_rc=$?
if (( serve_control_rc == 1 )) && printf '%s' "${serve_control_out}" |
    jq -e '.managed.pidfileState == "stale" and .managed.pid == 99999999
        and .managed.alive == false and .managed.owned == false
        and .failure == {count:4,lastFailedAt:1704067285,backoffSeconds:60,backoffRemainingSeconds:45}
        and .cache.source == "cli" and (.cache.ageSeconds | type == "number")' >/dev/null \
    && [[ -f "${serve_control_cache}/codexbar-serve.pid" && -f "${serve_control_cache}/serve-failed-at" ]]; then
    ok "serve status keeps stale pidfile and backoff stamps while reporting cache freshness"
else
    fail "serve status keeps stale pidfile and backoff stamps while reporting cache freshness" "rc=${serve_control_rc}; ${serve_control_out}"
fi
sleep 30 &
serve_control_pid=$!
printf '%s:1704067200:codexbar\n' "${serve_control_pid}" > "${serve_control_cache}/codexbar-serve.pid"
serve_control_rc=0
serve_control_out=$(serve_control_status env SHOWY_QUOTA_TEST_STATUS_PID="${serve_control_pid}" \
    SHOWY_QUOTA_TEST_STATUS_LISTENER_PID="${serve_control_pid}") || serve_control_rc=$?
if (( serve_control_rc == 0 )) && printf '%s' "${serve_control_out}" |
    jq -e --argjson pid "${serve_control_pid}" '.healthy and .managed.pidfileState == "owned"
        and .managed.pid == $pid and .managed.alive and .managed.owned' >/dev/null; then
    ok "serve status checks an alive pid against its recorded identity"
else
    fail "serve status checks an alive pid against its recorded identity" "rc=${serve_control_rc}; ${serve_control_out}"
fi
serve_control_rc=0
serve_control_out=$(serve_control_status env SHOWY_QUOTA_TEST_STATUS_PID="${serve_control_pid}") || serve_control_rc=$?
if (( serve_control_rc == 1 )) && printf '%s' "${serve_control_out}" |
    jq -e --argjson pid "${serve_control_pid}" '.healthy == false and .managed.pidfileState == "stale"
        and .managed.pid == $pid and .managed.alive and .managed.owned == false
        and (.managed.reason | contains("does not own the configured port"))
        and .serve.healthReachable == true' >/dev/null \
    && [[ "$(< "${serve_control_cache}/codexbar-serve.pid")" == "${serve_control_pid}:1704067200:codexbar" ]]; then
    ok "serve status does not claim a matching pid when a foreign process owns the port"
else
    fail "serve status does not claim a matching pid when a foreign process owns the port" "rc=${serve_control_rc}; ${serve_control_out}"
fi
kill "${serve_control_pid}" 2>/dev/null || true
wait "${serve_control_pid}" 2>/dev/null || true
printf '%s\n' '99999999:1704067200:codexbar' > "${serve_control_cache}/codexbar-serve.pid"
serve_control_rc=0
serve_control_out=$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${serve_control_cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_control_url}" "${REPO_ROOT}/bin/showy-quota" serve stop 2>&1) || serve_control_rc=$?
if (( serve_control_rc == 0 )) && [[ ! -e "${serve_control_cache}/codexbar-serve.pid" ]]; then
    ok "serve stop removes a stale pidfile without signaling a foreign process"
else
    fail "serve stop removes a stale pidfile without signaling a foreign process" "rc=${serve_control_rc}; ${serve_control_out}"
fi
for serve_control_refusal in unmanaged nonlocal; do
    serve_control_rc=0
    if [[ "${serve_control_refusal}" == unmanaged ]]; then
        serve_control_out=$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${serve_control_cache}" \
            SHOWY_QUOTA_MANAGE_SERVE=0 SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_control_url}" \
            "${REPO_ROOT}/bin/showy-quota" serve restart 2>&1) || serve_control_rc=$?
        serve_control_needle='disabled'
    else
        serve_control_out=$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${serve_control_cache}" \
            SHOWY_QUOTA_MANAGE_SERVE=1 SHOWY_QUOTA_CODEXBAR_SERVE_URL='http://example.test:18183' \
            "${REPO_ROOT}/bin/showy-quota" serve restart 2>&1) || serve_control_rc=$?
        serve_control_needle='local loopback'
    fi
    if (( serve_control_rc == 1 )) && [[ "${serve_control_out}" == *"${serve_control_needle}"* ]]; then
        ok "serve restart refuses ${serve_control_refusal} mode"
    else
        fail "serve restart refuses ${serve_control_refusal} mode" "rc=${serve_control_rc}; ${serve_control_out}"
    fi
done
serve_control_restart_cache=$(mk_cache)
serve_control_restart_port=$(unused_tcp_port)
serve_control_restart_url="http://127.0.0.1:${serve_control_restart_port}"
serve_control_args="${TMP}/serve-control-args.log"
serve_control_rc=0
serve_control_out=$(env PATH="${managed_bin_dir}:${PATH}" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${serve_control_restart_cache}" SHOWY_QUOTA_MANAGE_SERVE=1 \
    SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_control_restart_url}" \
    SHOWY_QUOTA_REFRESH_SECONDS=147 \
    SHOWY_QUOTA_TEST_MANAGED_ARGS_FILE="${serve_control_args}" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota" serve restart 2>&1) || serve_control_rc=$?
if (( serve_control_rc == 0 )) && [[ -s "${serve_control_restart_cache}/codexbar-serve.pid" ]] \
    && [[ "$(< "${serve_control_args}")" == *"serve --port ${serve_control_restart_port} --refresh-interval 147"* ]]; then
    ok "serve restart starts a stub on the configured port with the refresh interval"
else
    fail "serve restart starts a stub on the configured port with the refresh interval" "rc=${serve_control_rc}; ${serve_control_out}"
fi
serve_control_rc=0
env PATH="${managed_bin_dir}:${PATH}" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${serve_control_restart_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_control_restart_url}" \
    "${REPO_ROOT}/bin/showy-quota" serve stop >/dev/null 2>&1 || serve_control_rc=$?
if (( serve_control_rc == 0 )) && [[ ! -e "${serve_control_restart_cache}/codexbar-serve.pid" ]]; then
    ok "serve stop terminates the isolated managed stub"
else
    fail "serve stop terminates the isolated managed stub" "rc=${serve_control_rc}"
fi
serve_control_wait_cache=$(mk_cache)
serve_control_wait_port=$(unused_tcp_port)
serve_control_wait_url="http://127.0.0.1:${serve_control_wait_port}"
serve_control_wait_lock="${serve_control_wait_cache}/usage.lock.d"
mkdir "${serve_control_wait_lock}"
printf '%s\n' "$$" > "${serve_control_wait_lock}/owner.pid"
serve_control_wait_stub="${TMP}/serve-control-wait-stub"
mkdir "${serve_control_wait_stub}"
cat > "${serve_control_wait_stub}/sleep" <<'EOF'
#!/usr/bin/env bash
if [[ ! -e "${SHOWY_QUOTA_TEST_RELEASE_MARKER}" && -f "${SHOWY_QUOTA_TEST_RELEASE_LOCK}/owner.pid" ]]; then
    : > "${SHOWY_QUOTA_TEST_RELEASE_MARKER}"
    rm -f -- "${SHOWY_QUOTA_TEST_RELEASE_LOCK}/owner.pid"
    rmdir -- "${SHOWY_QUOTA_TEST_RELEASE_LOCK}"
fi
exec /bin/sleep "$@"
EOF
chmod +x "${serve_control_wait_stub}/sleep"
serve_control_rc=0
serve_control_out=$(env PATH="${serve_control_wait_stub}:${managed_bin_dir}:${PATH}" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${serve_control_wait_cache}" SHOWY_QUOTA_MANAGE_SERVE=1 \
    SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_control_wait_url}" \
    SHOWY_QUOTA_LOCK_WAIT_TENTHS=20 SHOWY_QUOTA_TEST_RELEASE_LOCK="${serve_control_wait_lock}" \
    SHOWY_QUOTA_TEST_RELEASE_MARKER="${serve_control_wait_cache}/lock-released" \
    SHOWY_QUOTA_TEST_SERVE_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${REPO_ROOT}/bin/showy-quota" serve restart 2>&1) || serve_control_rc=$?
if (( serve_control_rc == 0 )) && [[ "${serve_control_out}" == *'managed codexbar serve restarted'* ]] \
    && [[ -s "${serve_control_wait_cache}/codexbar-serve.pid" ]] \
    && [[ -f "${serve_control_wait_cache}/lock-released" ]] && [[ ! -e "${serve_control_wait_lock}" ]]; then
    ok "serve restart waits for a held refresh lock before starting the managed stub"
else
    fail "serve restart waits for a held refresh lock before starting the managed stub" "rc=${serve_control_rc}; ${serve_control_out}"
fi
serve_control_rc=0
serve_control_stop_err="${TMP}/serve-control-wait-stop.err"
env PATH="${managed_bin_dir}:${PATH}" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${serve_control_wait_cache}" \
    SHOWY_QUOTA_CODEXBAR_BIN="${managed_bin_dir}/codexbar" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL="${serve_control_wait_url}" \
    "${REPO_ROOT}/bin/showy-quota" serve stop >/dev/null 2>"${serve_control_stop_err}" || serve_control_rc=$?
if (( serve_control_rc != 0 )) || [[ -e "${serve_control_wait_cache}/codexbar-serve.pid" ]]; then
    fail "serve stop cleans up the managed stub after lock contention" \
        "rc=${serve_control_rc}; stderr=$(< "${serve_control_stop_err}")"
fi


# ── automation primitives: reset, guarded run, provider pick, refresh ──
printf '\nquota automation primitives\n'
automation_cache=$(mk_cache)
automation_fixture="${TMP}/automation-reset.json"
printf '%s\n' '[{"provider":"codex","usage":{"primary":{"usedPercent":95,"resetsAt":"2099-01-01T00:02:30Z"}}},{"provider":"claude","usage":{"primary":{"usedPercent":40,"resetsAt":"2099-01-01T00:04:00Z"}}}]' > "${automation_fixture}"
seed_usage_cache "${automation_cache}" "${automation_fixture}" serve
automation_out=$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${automation_cache}" \
    SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
    "${REPO_ROOT}/bin/showy-quota" next-reset --provider codex --window primary --no-fetch)
assert_equals "next-reset prints exact fixture seconds" "150" "${automation_out}"
automation_out=$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${automation_cache}" \
    SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
    "${REPO_ROOT}/bin/showy-quota" next-reset --provider codex --window primary --epoch --no-fetch)
assert_equals "next-reset prints absolute fixture epoch" "4070908950" "${automation_out}"
automation_rc=0
automation_out=$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${automation_cache}" \
    SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
    "${REPO_ROOT}/bin/showy-quota" next-reset --provider missing --no-fetch) || automation_rc=$?
assert_equals "next-reset exits 1 for an unknown provider" "1" "${automation_rc}"
automation_rc=0
automation_out=$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${automation_cache}" \
    SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
    "${REPO_ROOT}/bin/showy-quota" run --no-fetch --provider claude --window primary \
        --min-remaining 50 -- sh -c 'printf child-ok; exit 17') || automation_rc=$?
assert_equals "run executes a passing child" "child-ok" "${automation_out}"
assert_equals "run forwards its child exit code" "17" "${automation_rc}"
automation_marker="${TMP}/run-blocked-marker"
automation_rc=0
# shellcheck disable=SC2016  # The child shell must expand its own "$1".
env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${automation_cache}" \
    SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
    "${REPO_ROOT}/bin/showy-quota" run --no-fetch --provider codex --window primary \
        --min-remaining 10 --wait-max 0 -- sh -c 'touch "$1"' sh "${automation_marker}" \
        >/dev/null 2>&1 || automation_rc=$?
assert_equals "run refuses a breach without reset budget" "1" "${automation_rc}"
if [[ -e "${automation_marker}" ]]; then
    fail "run never launches a child after a breach"
else
    ok "run never launches a child after a breach"
fi
# The next provider can become the bottleneck after one reset. Simulate the
# sleeps without waiting for the wall clock; each retry must follow a new reset.
automation_step1="${TMP}/automation-step1.json"
automation_step2="${TMP}/automation-step2.json"
jq '.providers[0].usage.primary.usedPercent = 10
    | .providers[1].usage.primary.usedPercent = 95
    | .providers[1].usage.primary.resetsAt = "2099-01-01T00:00:40Z"' \
    "${automation_cache}/usage.json" > "${automation_step1}"
jq '.providers[0].usage.primary.usedPercent = 10
    | .providers[1].usage.primary.usedPercent = 10' \
    "${automation_step1}" > "${automation_step2}"
automation_sleep_dir="${TMP}/automation-sleep"
mkdir -p "${automation_sleep_dir}"
cat > "${automation_sleep_dir}/sleep" <<'EOF'
#!/bin/sh
count=$(cat "${AUTO_SLEEP_COUNT}" 2>/dev/null || printf 0)
count=$((count + 1))
printf '%s' "${count}" > "${AUTO_SLEEP_COUNT}"
if [ "${count}" -eq 1 ]; then
    cp "${AUTO_STEP1}" "${AUTO_CACHE}/usage.json"
else
    cp "${AUTO_STEP2}" "${AUTO_CACHE}/usage.json"
fi
EOF
chmod +x "${automation_sleep_dir}/sleep"
automation_rc=0
automation_out=$(env PATH="${automation_sleep_dir}:${PATH}" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${automation_cache}" SHOWY_QUOTA_NOW_EPOCH=4070908800 \
    SHOWY_QUOTA_REFRESH_SECONDS=9999999999 AUTO_CACHE="${automation_cache}" \
    AUTO_STEP1="${automation_step1}" AUTO_STEP2="${automation_step2}" \
    AUTO_SLEEP_COUNT="${TMP}/automation-sleep-count" \
    "${REPO_ROOT}/bin/showy-quota" run --no-fetch --provider codex,claude \
        --window primary --min-remaining 20 --wait-max 300 -- sh -c 'printf ready') || automation_rc=$?
assert_equals "run passes after two distinct resets" "0" "${automation_rc}"
assert_equals "run executes after the second reset" "ready" "${automation_out}"
assert_equals "run sleeps once for each reset" "2" "$(< "${TMP}/automation-sleep-count")"
# Restore the initial fixture for the provider-routing cases below.
seed_usage_cache "${automation_cache}" "${automation_fixture}" serve

automation_out=$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${automation_cache}" \
    SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
    "${REPO_ROOT}/bin/showy-quota" pick --no-fetch)
assert_equals "pick chooses maximum remaining on worst windows" "claude" "${automation_out}"
automation_rc=0
env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${automation_cache}" \
    SHOWY_QUOTA_NOW_EPOCH=4070908800 SHOWY_QUOTA_REFRESH_SECONDS=9999999999 \
    "${REPO_ROOT}/bin/showy-quota" pick --no-fetch --min-remaining 61 >/dev/null 2>&1 || automation_rc=$?
assert_equals "pick exits 1 above available floor" "1" "${automation_rc}"
automation_rc=0
automation_out=$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${automation_cache}" \
    "${REPO_ROOT}/bin/showy-quota" pick --no-fetch --min-remaining 18446744073709551616 2>&1) || automation_rc=$?
assert_equals "pick rejects an overflowing floor with usage exit 2" "2" "${automation_rc}"
assert_contains "pick reports the invalid floor" "invalid floor" "${automation_out}"
automation_rc=0
env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${automation_cache}" \
    "${REPO_ROOT}/bin/showy-quota" run --no-fetch --wait-max 18446744073709551616 -- true \
    >/dev/null 2>&1 || automation_rc=$?
assert_equals "run rejects an overflowing wait budget with usage exit 2" "2" "${automation_rc}"
refresh_path="${TMP}/refresh-path"
mkdir -p "${refresh_path}"
ln -s "$(command -v bash)" "${refresh_path}/bash"
refresh_fetch="${TMP}/refresh-fetch"
printf '#!/bin/sh\nprintf fetched > "%s"\n' "${TMP}/refresh-fetched" > "${refresh_fetch}"
chmod +x "${refresh_fetch}"
automation_rc=0
env PATH="${refresh_path}:/usr/bin:/bin" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${automation_cache}" SHOWY_QUOTA_FETCH_BIN="${refresh_fetch}" \
    "${REPO_ROOT}/bin/showy-quota" refresh >/dev/null 2>&1 || automation_rc=$?
assert_equals "refresh succeeds without host surfaces" "0" "${automation_rc}"
assert_equals "refresh runs forced fetch without host surfaces" "fetched" "$(< "${TMP}/refresh-fetched")"
cat > "${refresh_path}/tmux" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "${REFRESH_TMUX_LOG}"
case "$1" in
    has-session) [ "${REFRESH_TMUX_SERVER}" = 1 ] ;;
    show-options)
        case "$3" in
            status-right) printf '%s\n' "${REFRESH_TMUX_RIGHT}" ;;
            status-left) printf '%s\n' "${REFRESH_TMUX_LEFT}" ;;
        esac ;;
    list-clients) printf '/dev/pts/1\n' ;;
esac
EOF
chmod +x "${refresh_path}/tmux"
refresh_tmux_log="${TMP}/refresh-tmux.log"
: > "${refresh_tmux_log}"
env PATH="${refresh_path}:/usr/bin:/bin" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${automation_cache}" SHOWY_QUOTA_FETCH_BIN="${refresh_fetch}" \
    REFRESH_TMUX_LOG="${refresh_tmux_log}" REFRESH_TMUX_SERVER=1 \
    REFRESH_TMUX_RIGHT='#{hostname}' REFRESH_TMUX_LEFT='plain text' \
    "${REPO_ROOT}/bin/showy-quota" refresh >/dev/null
assert_not_contains "refresh leaves unrelated tmux status alone" "refresh-client" "$(< "${refresh_tmux_log}")"
: > "${refresh_tmux_log}"
env PATH="${refresh_path}:/usr/bin:/bin" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${automation_cache}" SHOWY_QUOTA_FETCH_BIN="${refresh_fetch}" \
    REFRESH_TMUX_LOG="${refresh_tmux_log}" REFRESH_TMUX_SERVER=1 \
    REFRESH_TMUX_RIGHT='#{hostname}' REFRESH_TMUX_LEFT='#("/tmp/showy-quota-tmux-bar")' \
    "${REPO_ROOT}/bin/showy-quota" refresh >/dev/null
assert_contains "refresh nudges a configured showy-quota tmux status" \
    'refresh-client -S -t /dev/pts/1' "$(< "${refresh_tmux_log}")"
: > "${refresh_tmux_log}"
env PATH="${refresh_path}:/usr/bin:/bin" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${automation_cache}" SHOWY_QUOTA_FETCH_BIN="${refresh_fetch}" \
    REFRESH_TMUX_LOG="${refresh_tmux_log}" REFRESH_TMUX_SERVER=0 \
    REFRESH_TMUX_RIGHT='showy-quota-tmux-bar' REFRESH_TMUX_LEFT='' \
    "${REPO_ROOT}/bin/showy-quota" refresh >/dev/null
assert_not_contains "refresh does not nudge tmux without a server" "refresh-client" "$(< "${refresh_tmux_log}")"

# ── configuration issue checker ────────────────────────────────────────
printf '\nconfiguration issue checker\n'
check_xdg="${TMP}/config-issues"
check_cache=$(mk_cache)
mkdir -p "${check_xdg}/showy-quota"
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${check_cache}/usage.json"
printf '%s\n' 'SHOWY_QUOTA_REFRESH_SECONDS=oops' 'SHOWY_QUOTA_PALETTE_TYPO=fff' \
    > "${check_xdg}/showy-quota/config.env"
check_rc=0
check_out=$(env XDG_CONFIG_HOME="${check_xdg}" SHOWY_QUOTA_NO_CONFIG= \
    SHOWY_QUOTA_CACHE_DIR="${check_cache}" "${REPO_ROOT}/bin/showy-quota" --check-config --json) || check_rc=$?
if (( check_rc == 1 )) && printf '%s' "${check_out}" | jq -e '
    .count == 2
    and any(.issues[]; .key == "SHOWY_QUOTA_REFRESH_SECONDS" and .raw == "oops"
        and .effective == "120" and .reason == "not_integer")
    and any(.issues[]; .key == "SHOWY_QUOTA_PALETTE_TYPO" and .raw == "fff"
        and .effective == null and .reason == "unknown_key")
' >/dev/null; then
    ok "config checker shows the numeric fallback and ignored unknown knob"
else
    fail "config checker shows the numeric fallback and ignored unknown knob" "rc=${check_rc}; ${check_out}"
fi
check_diag=$(env XDG_CONFIG_HOME="${check_xdg}" SHOWY_QUOTA_NO_CONFIG= \
    SHOWY_QUOTA_CACHE_DIR="${check_cache}" SHOWY_QUOTA_CODEXBAR_SERVE_URL= \
    "${REPO_ROOT}/bin/showy-quota" --diagnose --json)
if printf '%s' "${check_diag}" | jq -e '
    .configIssues.count == 2
    and any(.configIssues.issues[]; .key == "SHOWY_QUOTA_REFRESH_SECONDS" and .effective == "120")
' >/dev/null; then
    ok "diagnose JSON includes the config issue list"
else
    fail "diagnose JSON includes the config issue list" "${check_diag}"
fi
check_text=$(env XDG_CONFIG_HOME="${check_xdg}" SHOWY_QUOTA_NO_CONFIG= \
    SHOWY_QUOTA_CACHE_DIR="${check_cache}" SHOWY_QUOTA_CODEXBAR_SERVE_URL= \
    "${REPO_ROOT}/bin/showy-quota" --diagnose)
if [[ "${check_text}" == *"config issues: 2"* && "${check_text}" == *"effective=120"* ]]; then
    ok "diagnose text includes issue count and fallback"
else
    fail "diagnose text includes issue count and fallback" "${check_text}"
fi
printf '%s\n' 'SHOWY_QUOTA_CODEXBAR_BIN="/private/config-check-secret/bin; bad"' \
    'SHOWY_QUOTA_MANAGE_SERVE=perhaps' 'SHOWY_QUOTA_PALETTE_BG=badhex' \
    'SHOWY_QUOTA_PNG_BAR_W=99999' > "${check_xdg}/showy-quota/config.env"
check_rc=0
check_out=$(env XDG_CONFIG_HOME="${check_xdg}" SHOWY_QUOTA_NO_CONFIG= \
    SHOWY_QUOTA_CACHE_DIR="${check_cache}" "${REPO_ROOT}/bin/showy-quota" --check-config --json --redact) || check_rc=$?
if (( check_rc == 1 )) && printf '%s' "${check_out}" | jq -e '
    .count >= 4
    and any(.issues[]; .key == "SHOWY_QUOTA_CODEXBAR_BIN" and .reason == "invalid_value" and .effective == "codexbar")
    and any(.issues[]; .key == "SHOWY_QUOTA_MANAGE_SERVE" and .reason == "invalid_boolean" and .effective == "1")
    and any(.issues[]; .key == "SHOWY_QUOTA_PALETTE_BG" and .reason == "invalid_color" and .effective == "161616")
    and any(.issues[]; .key == "SHOWY_QUOTA_PNG_BAR_W" and .reason == "above_max" and .effective == "4096")
    and ([.issues[].raw] | all(.[]; contains("config-check-secret") | not))
' >/dev/null; then
    ok "config checker records clamped values and redacts path-shaped raw values"
else
    fail "config checker records clamped values and redacts path-shaped raw values" "rc=${check_rc}; ${check_out}"
fi
printf '%s\n' 'SHOWY_QUOTA_THEME=missing' > "${check_xdg}/showy-quota/config.env"
check_rc=0
check_out=$(env XDG_CONFIG_HOME="${check_xdg}" SHOWY_QUOTA_NO_CONFIG= \
    SHOWY_QUOTA_CACHE_DIR="${check_cache}" "${REPO_ROOT}/bin/showy-quota" --check-config --json 2>/dev/null) || check_rc=$?
if (( check_rc == 1 )) && printf '%s' "${check_out}" | jq -e '
    .count == 1 and any(.issues[]; .key == "SHOWY_QUOTA_THEME"
        and .raw == "missing" and .reason == "missing_theme")
' >/dev/null; then
    ok "config checker reports a missing theme and exits 1"
else
    fail "config checker reports a missing theme and exits 1" "rc=${check_rc}; ${check_out}"
fi
check_diag=$(env XDG_CONFIG_HOME="${check_xdg}" SHOWY_QUOTA_NO_CONFIG= \
    SHOWY_QUOTA_CACHE_DIR="${check_cache}" SHOWY_QUOTA_CODEXBAR_SERVE_URL= \
    "${REPO_ROOT}/bin/showy-quota" --diagnose --json 2>/dev/null)
assert_equals "diagnose reports a missing theme" "missing_theme" \
    "$(printf '%s' "${check_diag}" | jq -r '.configIssues.issues[] | select(.key == "SHOWY_QUOTA_THEME") | .reason')"
printf '%s\n' 'SHOWY_QUOTA_REFRESH_SECONDS=120' 'SHOWY_QUOTA_THEME=default' \
    > "${check_xdg}/showy-quota/config.env"
check_rc=0
check_out=$(env XDG_CONFIG_HOME="${check_xdg}" SHOWY_QUOTA_NO_CONFIG= \
    SHOWY_QUOTA_CACHE_DIR="${check_cache}" "${REPO_ROOT}/bin/showy-quota" --check-config --json) || check_rc=$?
if (( check_rc == 0 )) && printf '%s' "${check_out}" | jq -e '.count == 0 and .issues == []' >/dev/null; then
    ok "clean config, including a theme, exits successfully"
else
    fail "clean config, including a theme, exits successfully" "rc=${check_rc}; ${check_out}"
fi
check_render_config=$(env XDG_CONFIG_HOME="${check_xdg}" SHOWY_QUOTA_NO_CONFIG= \
    SHOWY_QUOTA_CACHE_DIR="${check_cache}" SHOWY_QUOTA_FETCH_BIN=/usr/bin/true \
    SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 "${REPO_ROOT}/bin/showy-quota-zellij-bar")
check_render_default=$(env XDG_CONFIG_HOME="${check_xdg}" SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${check_cache}" SHOWY_QUOTA_FETCH_BIN=/usr/bin/true \
    SHOWY_QUOTA_NOW_EPOCH=4070908800 NO_COLOR=1 "${REPO_ROOT}/bin/showy-quota-zellij-bar")
assert_equals "config checker leaves hot-path rendering unchanged" "${check_render_default}" "${check_render_config}"

# ── versioned state and provider explanation ────────────────────────
printf '\nversioned state and provider explanation\n'

state_schema="${REPO_ROOT}/share/schema/showy-quota-state.schema.json"
state_schema_cache=$(mk_cache)
if python3 -c 'import jsonschema' >/dev/null 2>&1; then
    printf '  validator: python3 jsonschema (draft 2020-12)\n'
    state_schema_validator=python
else
    printf '  validator: jq structural fallback (python3 jsonschema unavailable)\n'
    state_schema_validator=jq
fi
schema_checked=0
for state_fixture in "${FIXTURE_DIR}"/codexbar-*.json; do
    # The state bridge accepts only cache payloads that the fetcher accepts.
    if ! jq -e 'type == "array"' "${state_fixture}" >/dev/null; then
        continue
    fi
    cp "${state_fixture}" "${state_schema_cache}/usage.json"
    state_schema_output="${TMP}/state-schema-output.json"
    if ! env SHOWY_QUOTA_NO_CONFIG=1 \
        SHOWY_QUOTA_CACHE_DIR="${state_schema_cache}" \
        SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
        SHOWY_QUOTA_RENDER_BIN="${RENDER_BIN}" \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json > "${state_schema_output}"; then
        fail "state accepts $(basename "${state_fixture}")" "state command failed"
        continue
    fi
    if [[ "${state_schema_validator}" == python ]]; then
        if python3 -c 'import json,sys,jsonschema; schema=json.load(open(sys.argv[1])); state=json.load(open(sys.argv[2])); jsonschema.Draft202012Validator.check_schema(schema); jsonschema.Draft202012Validator(schema).validate(state)' \
            "${state_schema}" "${state_schema_output}" 2>/dev/null; then
            schema_checked=$((schema_checked + 1))
        else
            fail "schema validates $(basename "${state_fixture}")" "jsonschema rejected state"
        fi
    elif jq -e --slurpfile schema "${state_schema}" '
        (. as $state | all($schema[0].required[]; . as $key | $state | has($key)))
        and .schemaVersion == 1
        and (.available | type == "boolean")
        and (.stale | type == "boolean")
        and (.cache | type == "object" and (.source | IN("serve","cli","unknown")) and (.degraded | type == "boolean"))
        and (.cacheAgeSeconds | type | IN("number","null"))
        and (.staleAfterSeconds | type == "number")
        and (.providers | type == "array" and all(.[]; type == "string"))
        and (.providerCount | type == "number")
        and (.providerMetrics | type == "array" and all(.[];
            (.provider | type == "string")
            and (.windows | type == "object" and
                ([.primary,.secondary,.tertiary] | all(.[];
                    . == null or (type == "object" and
                        ([.usedPercent,.remainingPercent] | all(.[]; type == "number"))))))
            and (.extraRateWindows | type == "array" and all(.[]; type == "object" and has("usageKnown")))
            and (.error == null or (.error | type == "object" and has("kind") and has("message")))))
        and (.providerFreshness | type == "object" and all(.[]; type == "object" and has("source") and has("updatedAt") and has("ageSeconds") and has("stale")))
        and (.emptyReason | IN(null,"unavailable","no-providers","filtered","idle"))
        and (.sketchybar | type == "object" and (.compactRecommended | type == "boolean")
            and (.compactProviderThreshold | type == "number")
            and (.itemPrefix | type == "string") and (.bracket | type == "string"))
    ' "${state_schema_output}" >/dev/null; then
        schema_checked=$((schema_checked + 1))
    else
        fail "schema structure validates $(basename "${state_fixture}")" "jq rejected state"
    fi
done
if (( schema_checked > 0 )); then
    ok "schema validates ${schema_checked} accepted fixture outputs (${state_schema_validator})"
else
    fail "schema validates accepted fixture outputs" "no fixture passed"
fi

explain_cache=$(mk_cache)
cat > "${explain_cache}/usage.json" <<'EOF'
[
 {"provider":"codex","usage":{"primary":{"usedPercent":25}}},
 {"provider":"claude","usage":{"primary":{"usedPercent":40}}},
 {"provider":"gemini","usage":{"primary":{"usedPercent":null}}},
 {"provider":"cursor","error":{"message":"session missing"}},
 {"provider":"bad/id","usage":{"primary":{"usedPercent":5}}},
 {"provider":"opencode","usage":{"primary":{"usedPercent":30}}}
]
EOF
explain_json=$(env SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${explain_cache}" \
    SHOWY_QUOTA_PROVIDERS='claude,codex' \
    SHOWY_QUOTA_PROVIDERS_EXCLUDE='claude' \
    "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --explain --json)
assert_equals "explain gives every raw provider one stable reason" \
    "included,excluded_by_denylist,no_numeric_usage_window,error_record,invalid_id,excluded_by_allowlist" \
    "$(printf '%s' "${explain_json}" | jq -r 'map(.reason) | join(",")')"
assert_equals "explain exposes allow-list rank and final position" "allowlist|1|0" \
    "$(printf '%s' "${explain_json}" | jq -r '.[] | select(.provider == "codex") | [.rankSource,.orderRank,.position] | join("|")')"
assert_equals "explain excludes denied providers from positions" "null" \
    "$(printf '%s' "${explain_json}" | jq -r '.[] | select(.provider == "claude") | .position')"
assert_equals "explain works with --json before --explain" "included" \
    "$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${explain_cache}" \
        SHOWY_QUOTA_PROVIDERS='codex' \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --json --explain \
        | jq -r '.[0].reason')"
assert_equals "state count is unchanged by the explain path" "1" \
    "$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${explain_cache}" \
        SHOWY_QUOTA_PROVIDERS='claude,codex' SHOWY_QUOTA_PROVIDERS_EXCLUDE='claude' \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --count)"
assert_equals "state providers are unchanged by the explain path" "codex" \
    "$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${explain_cache}" \
        SHOWY_QUOTA_PROVIDERS='claude,codex' SHOWY_QUOTA_PROVIDERS_EXCLUDE='claude' \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --providers)"

explain_diagnose=$(env SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${explain_cache}" \
    SHOWY_QUOTA_CODEXBAR_SERVE_URL='' \
    SHOWY_QUOTA_CODEXBAR_BIN=/bin/false \
    SHOWY_QUOTA_REFRESH_SECONDS=999999 \
    "${REPO_ROOT}/bin/showy-quota" --diagnose --json)
assert_equals "diagnose JSON versions its own object and embedded state" "1|1" \
    "$(printf '%s' "${explain_diagnose}" | jq -r '[.schemaVersion,.state.schemaVersion] | join("|")')"
assert_equals "diagnose JSON explains cached provider decisions" "6|invalid_id" \
    "$(printf '%s' "${explain_diagnose}" | jq -r '[.providerDecisions | length, (.[4].reason)] | join("|")')"
cat > "${explain_cache}/usage.json" <<'EOF'
[
 {"provider":"codex","usage":{"primary":{"usedPercent":25}}},
 {"provider":"gemini","usage":3}
]
EOF
explain_json=$(env SHOWY_QUOTA_NO_CONFIG=1 \
    SHOWY_QUOTA_CACHE_DIR="${explain_cache}" \
    "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --explain --json)
assert_equals "explain assigns a numeric usage field the no-window reason" \
    "included,no_numeric_usage_window" \
    "$(printf '%s' "${explain_json}" | jq -r 'map(.reason) | join(",")')"
assert_equals "state render filter excludes numeric usage but keeps the valid provider" "codex" \
    "$(env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_CACHE_DIR="${explain_cache}" \
        "${REPO_ROOT}/bin/showy-quota-state" --no-fetch --providers)"

printf '\nOpt-in freshness and severity\n'
freshness_cache=$(mk_cache)
freshness_fixture="${TMP}/freshness-fixture.json"
jq 'map(if .provider == "codex" then .usage.primary.usedPercent = 70
        elif .provider == "gemini" then .usage.primary.usedPercent = 95
        else . end)' "${FIXTURE_DIR}/codexbar-mixed.json" > "${freshness_fixture}"
seed_usage_cache "${freshness_cache}" "${freshness_fixture}" serve
freshness_epoch=$(stat -f %m "${freshness_cache}/usage.json" 2>/dev/null) ||
    freshness_epoch=$(stat -c %Y "${freshness_cache}/usage.json")
freshness_bar() {
    env SHOWY_QUOTA_NO_CONFIG=1 XDG_CONFIG_HOME="${TMP}/freshness-config" \
        SHOWY_QUOTA_CACHE_DIR="${freshness_cache}" SHOWY_QUOTA_FETCH_BIN=/usr/bin/true \
        SHOWY_QUOTA_RENDER_BIN="${RENDER_BIN}" SHOWY_QUOTA_FRESHNESS="$1" \
        SHOWY_QUOTA_SEVERITY_GLYPHS=on SHOWY_QUOTA_REFRESH_SECONDS=120 \
        SHOWY_QUOTA_NOW_EPOCH="$2" NO_COLOR=1 \
        "${REPO_ROOT}/bin/showy-quota-zellij-bar"
}
freshness_out=$(freshness_bar age+source "$((freshness_epoch + 42))")
if [[ "${freshness_out}" == *"42s serve" && "${freshness_out}" != *$'\033'* ]]; then
    ok "cache freshness adds age and serve source without color"
else
    fail "cache freshness adds age and serve source without color" "${freshness_out}"
fi
freshness_out=$(freshness_bar age "$((freshness_epoch + 239))")
if [[ "${freshness_out}" == *"3m" ]]; then
    ok "freshness age remains visible just before stale horizon"
else
    fail "freshness age remains visible just before stale horizon" "${freshness_out}"
fi
freshness_out=$(freshness_bar age+source "$((freshness_epoch + 241))")
if [[ "${freshness_out}" == *"⚠" && "${freshness_out}" != *"4m serve" ]]; then
    ok "stale marker wins over opt-in freshness suffix"
else
    fail "stale marker wins over opt-in freshness suffix" "${freshness_out}"
fi
seed_usage_cache "${freshness_cache}" "${freshness_fixture}" cli
freshness_epoch=$(stat -f %m "${freshness_cache}/usage.json" 2>/dev/null) ||
    freshness_epoch=$(stat -c %Y "${freshness_cache}/usage.json")
freshness_out=$(freshness_bar source "$((freshness_epoch + 42))")
if [[ "${freshness_out}" == *"cli ⚠cli" ]]; then
    ok "cache source suffix shows CLI alongside existing degraded marker"
else
    fail "cache source suffix shows CLI alongside existing degraded marker" "${freshness_out}"
fi
freshness_out=$(freshness_bar off "$((freshness_epoch + 42))")
if [[ "${freshness_out}" == *'+'* && "${freshness_out}" == *'!'* &&
      "${freshness_out}" == *'x'* && "${freshness_out}" != *"42s"* ]]; then
    ok "non-color quota bands use distinct ASCII severity cues"
else
    fail "non-color quota bands use distinct ASCII severity cues" "${freshness_out}"
fi

printf '\nplain-text template emit\n'
template_sample=$(
    env SHOWY_QUOTA_NO_CONFIG=1 SHOWY_QUOTA_NOW_EPOCH=4070908800 \
        "${RENDER_BIN}" --emit template --format '{provider}/{window}: {sigil} {used}% {remaining}% {class} {countdown}' \
        --provider codex --json - < "${FIXTURE_DIR}/codexbar-mixed.json"
)
assert_equals "template expands the worst window of a selected provider" \
    "codex/secondary: CX 59% 41% good 7d" "${template_sample}"
template_prompt=$(SHOWY_QUOTA_RENDER_BIN="${RENDER_BIN}" run_prompt "${prompt_cache}" --format '{sigil} {used}% {countdown}{stale}')
assert_equals "the default template reproduces the fixed prompt" "CX 59% 7d" "${template_prompt}"
template_error='' template_rc=0
template_error=$(
    env SHOWY_QUOTA_NO_CONFIG=1 "${RENDER_BIN}" --emit template --format '{unknownField}' \
        --json - < "${FIXTURE_DIR}/codexbar-mixed.json" 2>&1
) || template_rc=$?
assert_equals "unknown template field exits with usage status" "2" "${template_rc}"
if [[ "${template_error}" == *"unknownField"* ]]; then
    ok "unknown template field names the field"
else
    fail "unknown template field names the field" "${template_error}"
fi

# ── sketchybar notch placement planner ───────────────────────────────

# Synthetic batched query: bar, displays, then items. Anchors mark the notch
# gap at [q_x+1, e_x+1); providers are laid out left-to-right from x=51 with
# the live geometry (102pt icon+slot, then 3pt + 32pt label, 8pt between
# providers). With labels=off the labels are hidden and providers sit 110pt
# apart.
notch_snapshot() {
    jq -n -c --argjson n "$1" --argjson rx "$2" --argjson qx "$3" --argjson ex "$4" \
        --arg labels "${5:-on}" '
        def it($name; $pos; $x; $w; $on):
            {name: $name, geometry: {position: $pos, drawing: (if $on then "on" else "off" end)},
             bounding_rects: {"display-1": (if $on then {origin: [$x, 0], size: [$w, 32]}
                                           else {origin: [-9999, -9999], size: [$w, 32]} end)}};
        ($labels == "on") as $on
        | (if $on then 145 else 110 end) as $pitch
        | {padding_right: 18, items: []},
          [{"arrangement-id": 1, frame: {x: 0, y: 0, w: 1728, h: 1117}}],
          it("showy_quota.notch_q"; "q"; $qx; 1; true),
          it("showy_quota.notch_e"; "e"; $ex; 1; true),
          it("clock"; "right"; $rx; 60; true),
          (range(0; $n) as $i | (51 + $pitch * $i) as $x
              | it("showy_quota.p\($i + 1).icon"; "left"; $x; 22; true),
                it("showy_quota.p\($i + 1).slot"; "left"; $x + 19; 83; true),
                it("showy_quota.p\($i + 1).label"; "left"; $x + 105; 32; $on))'
}

# The planner's geometry cases live in the Rust unit tests
# (crates/showy-quota-zellij-core/src/sketchybar_notch.rs). These pin the
# plugin-facing contract of `--emit sketchybar-layout`: a batched query reply
# on stdin becomes a stored plan plus the arguments that apply it.
run_layout() {
    local plan="$1" providers="$2"
    env SHOWY_QUOTA_NO_CONFIG=1 "${RENDER_BIN}" --emit sketchybar-layout \
        --plan "${plan}" --layout-providers "${providers}"
}
layout_plan="${TMP}/notch-layout.json"
layout_out=$(notch_snapshot 6 1324 763 964 | run_layout "${layout_plan}" p1,p2,p3,p4,p5,p6)
assert_contains "layout mode moves providers that would cross the notch to its right" \
    $'--set\x1f/^showy_quota\\.p5\\.[^.]*$/\x1fposition=e' "${layout_out}"
assert_contains "layout mode keeps the providers that fit on the left" \
    $'--set\x1f/^showy_quota\\.p4\\.[^.]*$/\x1fposition=left' "${layout_out}"
assert_contains "layout mode reports a plan that draws nothing new" $'layout\x1fok\x1f0' "${layout_out}"
assert_equals "layout mode stores the plan it applied" '["p5","p6"]' "$(jq -c '.right' "${layout_plan}")"

# The stored plan hid the labels; this one shows them again, so the plugin
# must re-render the rows it had shrunk.
jq -c '.compact = true' "${layout_plan}" > "${layout_plan}.tmp" && mv "${layout_plan}.tmp" "${layout_plan}"
layout_out=$(notch_snapshot 6 1324 763 964 | run_layout "${layout_plan}" p1,p2,p3,p4,p5,p6)
assert_contains "layout mode asks for a re-render when labels come back" $'layout\x1fok\x1f1' "${layout_out}"

layout_none="${TMP}/notch-layout-none.json"
layout_out=$(printf '' | run_layout "${layout_none}" p1)
assert_equals "layout mode reports a dropped SketchyBar reply" $'layout\x1fnoreply' "${layout_out}"
if [[ -e "${layout_none}" ]]; then
    fail "layout mode keeps the stored plan when the reply was dropped"
else
    ok "layout mode keeps the stored plan when the reply was dropped"
fi


# ── summary ──────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "${PASSED}" "${FAILED}"
if (( FAILED > 0 )); then
    printf 'failing tests:\n' >&2
    for f in "${FAILURES[@]}"; do printf '  - %s\n' "${f}" >&2; done
    exit 1
fi
