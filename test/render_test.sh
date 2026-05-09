#!/usr/bin/env bash
# codexbar-bars — smoke tests for renderers.
#
# Each test runs a renderer against a JSON fixture, with a stub `codexbar`
# binary that just prints the fixture, and asserts the output meets a
# minimal shape. Failures print context and abort the suite.
#
# Usage: test/render_test.sh

set -uo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
FIXTURE_DIR="${REPO_ROOT}/test/fixtures"

TMP=$(mktemp -d -t cb-bars-test.XXXXXX)
trap 'rm -rf "${TMP}"' EXIT

# ── stub codexbar that validates fetcher argv and prints the fixture ──

stub_dir="${TMP}/bin"
mkdir -p "${stub_dir}"
cat > "${stub_dir}/codexbar" <<'EOF'
#!/bin/sh
[ -n "${CB_BARS_TEST_FIXTURE:-}" ] || exit 1
[ "${1:-}" = "usage" ] || exit 90
shift
saw_format=0
saw_provider=0
saw_pretty=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --format)
            shift
            [ "${1:-}" = "json" ] || exit 91
            saw_format=1
            ;;
        --provider)
            shift
            [ "${1:-}" = "all" ] || exit 92
            saw_provider=1
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
[ "${saw_format}" = "1" ] && [ "${saw_provider}" = "1" ] && [ "${saw_pretty}" = "1" ] || exit 94
cat "${CB_BARS_TEST_FIXTURE}"
EOF
chmod +x "${stub_dir}/codexbar"

# Stub sketchybar so the plugin's --set calls do not error.
cat > "${stub_dir}/sketchybar" <<'EOF'
#!/bin/sh
echo "sketchybar $*" >> "${CB_BARS_TEST_LOG:-/dev/null}"
EOF
chmod +x "${stub_dir}/sketchybar"

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

run_renderer() {
    local renderer="$1" fixture="$2"
    local cache; cache=$(mk_cache)
    local out
    out=$(
        PATH="${stub_dir}:${PATH}" \
        CB_BARS_NO_CONFIG=1 \
        CB_BARS_CACHE_DIR="${cache}" \
        CB_BARS_TEST_FIXTURE="${FIXTURE_DIR}/${fixture}" \
        CB_BARS_FORCE_COLOR=1 \
        "${REPO_ROOT}/bin/${renderer}" 2>&1
    )
    printf '%s' "${out}"
}

assert_contains() {
    local label="$1" needle="$2" haystack="$3"
    if printf '%s' "${haystack}" | grep -F -q -- "${needle}"; then
        ok "${label}"
    else
        fail "${label}" "expected to contain: ${needle}"
        printf '    got: %s\n' "${haystack}" >&2
    fi
}

assert_not_contains() {
    local label="$1" needle="$2" haystack="$3"
    if printf '%s' "${haystack}" | grep -F -q -- "${needle}"; then
        fail "${label}" "expected NOT to contain: ${needle}"
    else
        ok "${label}"
    fi
}

# ── zellij renderer ──────────────────────────────────────────────────

printf 'zellij renderer\n'

out=$(run_renderer cb-bars-zellij-bar codexbar-mixed.json)
assert_contains "renders CL sigil for claude"          "CL"  "${out}"
assert_contains "renders CX sigil for codex"           "CX"  "${out}"
assert_contains "renders GE sigil for gemini"          "GE"  "${out}"
assert_not_contains "skips errored provider (cursor)"  "CR"  "${out}"

out=$(run_renderer cb-bars-zellij-bar codexbar-empty.json)
assert_contains "empty fixture renders 'AI idle'"      "AI idle" "${out}"

out=$(run_renderer cb-bars-zellij-bar codexbar-error-only.json)
assert_contains "all-error fixture renders 'AI idle'"  "AI idle" "${out}"

out=$(run_renderer cb-bars-zellij-bar codexbar-low.json)
# Bad-palette ed8796 = decimal RGB 237;135;150 inside the truecolor escape.
assert_contains "low-remaining fixture uses BAD palette" "237;135;150" "${out}"

# ── tmux renderer ────────────────────────────────────────────────────

printf '\ntmux renderer\n'

out=$(run_renderer cb-bars-tmux-bar codexbar-mixed.json)
assert_contains "tmux markup uses #[bold]"             "#[bold]" "${out}"
assert_contains "tmux markup names claude sigil"       "CL"      "${out}"
assert_contains "tmux markup uses #[default] reset"    "#[default]" "${out}"

out=$(run_renderer cb-bars-tmux-bar codexbar-empty.json)
assert_contains "tmux empty fixture renders 'AI idle'" "AI idle" "${out}"

install_bin="${TMP}/install/bin"
mkdir -p "${install_bin}"
ln -s "${REPO_ROOT}/bin/cb-bars-tmux-bar" "${install_bin}/cb-bars-tmux-bar"
out=$(
    PATH="${stub_dir}:${PATH}" \
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="$(mk_cache)" \
    CB_BARS_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${install_bin}/cb-bars-tmux-bar"
)
assert_contains "installed symlink resolves repo lib" "CL" "${out}"

# ── filter ───────────────────────────────────────────────────────────

printf '\nprovider filter\n'

cache=$(mk_cache)
out=$(
    PATH="${stub_dir}:${PATH}" \
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    CB_BARS_PROVIDERS=claude \
    NO_COLOR=1 \
    "${REPO_ROOT}/bin/cb-bars-zellij-bar"
)
assert_contains "filter restricts to claude"           "CL" "${out}"
assert_not_contains "filter excludes codex"            "CX" "${out}"
assert_not_contains "filter excludes gemini"           "GE" "${out}"

# ── sketchybar item declaration (without sketchybar daemon) ───────────

printf '\nsketchybar item declaration\n'

cache=$(mk_cache)
log="${TMP}/sb-items.log"
PATH="${stub_dir}:${PATH}" \
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    CB_BARS_TEST_LOG="${log}" \
    "${REPO_ROOT}/sketchybar/items/cb_bars.sh"
item_log="$(< "${log}")"
assert_contains "icon item reserves visible width" "cb_bars.claude.icon" "${item_log}"
assert_contains "icon item sets width" "width=24" "${item_log}"
assert_contains "bar item sets width" "width=84" "${item_log}"
assert_contains "bar item enables background image drawing" "background.image.drawing=on" "${item_log}"

# ── sketchybar plugin (without sketchybar daemon) ────────────────────

printf '\nsketchybar plugin (PNG generation)\n'

cache=$(mk_cache)
log="${TMP}/sb.log"
PATH="${stub_dir}:${PATH}" \
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_SKETCHYBAR_IMAGE_CACHE="${cache}/sb" \
    CB_BARS_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    CB_BARS_TEST_LOG="${log}" \
    "${REPO_ROOT}/sketchybar/plugins/cb_bars.sh"

if [[ -s "${cache}/sb/bar-claude.png" ]]; then ok "claude bar PNG generated"
else fail "claude bar PNG generated"; fi
if [[ -s "${cache}/sb/bar-codex.png" ]]; then ok "codex bar PNG generated"
else fail "codex bar PNG generated"; fi
if [[ -s "${log}" ]]; then ok "sketchybar received --set commands"
else fail "sketchybar received --set commands"; fi
if grep -q 'label.color=0xff' "${log}" 2>/dev/null; then
    ok "label.color is well-formed (0xffRRGGBB)"
else
    fail "label.color is well-formed"
fi
if grep -q 'width=84' "${log}" 2>/dev/null; then
    ok "plugin repairs bar item width"
else
    fail "plugin repairs bar item width"
fi

cache=$(mk_cache)
log="${TMP}/sb-filter.log"
PATH="${stub_dir}:${PATH}" \
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_SKETCHYBAR_IMAGE_CACHE="${cache}/sb" \
    CB_BARS_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    CB_BARS_TEST_LOG="${log}" \
    CB_BARS_PROVIDERS=claude \
    "${REPO_ROOT}/sketchybar/plugins/cb_bars.sh"
assert_contains "sketchybar filter includes claude" "cb_bars.claude.label" "$(< "${log}")"
assert_not_contains "sketchybar filter excludes codex" "cb_bars.codex.label" "$(< "${log}")"

# ── schema drift / edge JSON ────────────────────────────────────────

printf '\nschema drift\n'

# 1. Float usedPercent must not crash bash arithmetic.
out=$(run_renderer cb-bars-zellij-bar codexbar-realistic.json)
assert_contains "float usedPercent renders codex"      "CX" "${out}"
assert_contains "float usedPercent renders claude"     "CL" "${out}"
assert_contains "float usedPercent uses GOOD palette" "166;218;149" "${out}"

out=$(run_renderer cb-bars-tmux-bar codexbar-realistic.json)
assert_contains "tmux float usedPercent renders codex" "CX" "${out}"

# 2. Provider with usage.primary but no resetsAt must render '?' not crash.
out=$(run_renderer cb-bars-zellij-bar codexbar-no-reset.json)
assert_contains "no-reset fixture still renders codex" "CX" "${out}"
assert_contains "no-reset fixture shows '?' countdown" "?"  "${out}"

# 3. Non-array JSON must be rejected by the fetcher (refresh path).
printf '\ncache fetcher\n'

cache=$(mk_cache)
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    "${REPO_ROOT}/bin/cb-bars-fetch" 2>&1
) || rc=$?
if (( rc != 0 )) && ! [[ -f "${cache}/usage.json" ]]; then
    ok "fetcher rejects non-array JSON"
else
    fail "fetcher rejects non-array JSON" "rc=${rc}; cache exists: $([[ -f ${cache}/usage.json ]] && echo yes)"
fi

# 3b. Fresh but invalid cache must not be emitted as success.
cache=$(mk_cache)
printf '%s\n' '[{"provider":"codex","usage":{"primary":{}}}]' > "${cache}/usage.json"
rc=0
out=$(
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_CODEXBAR_BIN="${missing_bin:-${TMP}/no-such-codexbar}" \
    "${REPO_ROOT}/bin/cb-bars-fetch" 2>/dev/null
) || rc=$?
if (( rc != 0 )) && [[ -z "${out}" ]]; then
    ok "fetcher rejects invalid fresh cache"
else
    fail "fetcher rejects invalid fresh cache" "rc=${rc}; out=${out}"
fi

bad_provider="${TMP}/bad-provider.json"
printf '%s\n' '[{"provider":"bad/id","usage":{"primary":{"usedPercent":12}}}]' > "${bad_provider}"
cache=$(mk_cache)
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_TEST_FIXTURE="${bad_provider}" \
    "${REPO_ROOT}/bin/cb-bars-fetch" 2>/dev/null
) || rc=$?
if (( rc != 0 )) && [[ -z "${out}" ]] && ! [[ -f "${cache}/usage.json" ]]; then
    ok "fetcher rejects unsafe provider ids"
else
    fail "fetcher rejects unsafe provider ids" "rc=${rc}; out=${out}"
fi

# 4. Missing codexbar binary, no cache → fetcher fails with diagnostic.
missing_bin="${TMP}/no-such-codexbar"
cache=$(mk_cache)
rc=0
out=$(
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_CODEXBAR_BIN="${missing_bin}" \
    "${REPO_ROOT}/bin/cb-bars-fetch" 2>&1
) || rc=$?
if (( rc != 0 )); then
    ok "fetcher fails when codexbar missing and cache empty"
else
    fail "fetcher fails when codexbar missing and cache empty"
fi

# 5. Missing codexbar binary, but stale cache exists → serve stale.
cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
rc=0
out=$(
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_CODEXBAR_BIN="${missing_bin}" \
    "${REPO_ROOT}/bin/cb-bars-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array"' >/dev/null 2>&1; then
    ok "fetcher serves stale cache when codexbar disappears"
else
    fail "fetcher serves stale cache when codexbar disappears" "rc=${rc}"
fi

# 6. Stale-cache dimming kicks in for terminal renderers.
printf '\nstale cache dimming\n'

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
# Backdate cache to 1988 so age is decades, well beyond 2 * default REFRESH_SECONDS.
touch -t 198801010000 "${cache}/usage.json"
# Use a bogus codexbar bin so fetch cannot refresh the backdated cache.
ansi_dim=$'\x1b[2m'
out=$(
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    CB_BARS_FORCE_COLOR=1 \
    "${REPO_ROOT}/bin/cb-bars-zellij-bar"
)
assert_contains "zellij dims when cache is stale"      "${ansi_dim}" "${out}"

# 7. Concurrent fetch — only one codexbar invocation across simultaneous
#    callers. We exercise both lock paths via CB_BARS_FORCE_NO_FLOCK.
printf '\nconcurrent fetch\n'

slow_dir="${TMP}/slow"
mkdir -p "${slow_dir}"
cat > "${slow_dir}/codexbar" <<EOF
#!/bin/sh
[ -n "\${CB_BARS_TEST_COUNTER:-}" ] && printf 'x' >> "\${CB_BARS_TEST_COUNTER}"
sleep 1
cat "\${CB_BARS_TEST_FIXTURE:-${FIXTURE_DIR}/codexbar-mixed.json}"
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
            CB_BARS_NO_CONFIG=1 \
            CB_BARS_CACHE_DIR="${cache}" \
            CB_BARS_CODEXBAR_BIN="${slow_dir}/codexbar" \
            CB_BARS_FORCE_NO_FLOCK="${force_no_flock}" \
            CB_BARS_TEST_COUNTER="${counter}" \
            "${REPO_ROOT}/bin/cb-bars-fetch" > "${out_file}" 2>/dev/null
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
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_CODEXBAR_BIN="${slow_dir}/codexbar" \
    CB_BARS_FORCE_NO_FLOCK=1 \
    CB_BARS_TEST_COUNTER="${counter}" \
    "${REPO_ROOT}/bin/cb-bars-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array"' >/dev/null 2>&1; then
    ok "mkdir path: recovers dead owner lock"
else
    fail "mkdir path: recovers dead owner lock" "rc=${rc}"
fi

printf '\nforced refresh lock wait\n'
cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-low.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
counter="${cache}/cb-call-count"
: > "${counter}"
pids=()
outputs=()
for idx in 1 2 3 4; do
    out_file="${cache}/refresh.${idx}.json"
    outputs+=("${out_file}")
    (
        CB_BARS_NO_CONFIG=1 \
        CB_BARS_CACHE_DIR="${cache}" \
        CB_BARS_CODEXBAR_BIN="${slow_dir}/codexbar" \
        CB_BARS_TEST_COUNTER="${counter}" \
        CB_BARS_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
        "${REPO_ROOT}/bin/cb-bars-fetch" --refresh > "${out_file}" 2>/dev/null
    ) &
    pids+=("$!")
done

all_fresh=1
for idx in "${!pids[@]}"; do
    if wait "${pids[$idx]}" && grep -F -q 'futureUnknownTopLevelField' "${outputs[$idx]}"; then
        :
    else
        all_fresh=0
    fi
done
if (( all_fresh )); then
    ok "forced refresh callers wait for refreshed cache"
else
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
counter="${cache}/timeout-call-count"
: > "${counter}"
(
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_CODEXBAR_BIN="${slow_dir}/codexbar" \
    CB_BARS_FORCE_NO_FLOCK=1 \
    CB_BARS_TEST_COUNTER="${counter}" \
    CB_BARS_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/cb-bars-fetch" --refresh >/dev/null 2>/dev/null
) &
holder_pid=$!
sleep 0.2
rc=0
out=$(
    CB_BARS_NO_CONFIG=1 \
    CB_BARS_CACHE_DIR="${cache}" \
    CB_BARS_CODEXBAR_BIN="${slow_dir}/codexbar" \
    CB_BARS_FORCE_NO_FLOCK=1 \
    CB_BARS_LOCK_WAIT_TENTHS=1 \
    CB_BARS_TEST_COUNTER="${counter}" \
    CB_BARS_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/cb-bars-fetch" --refresh 2>/dev/null
) || rc=$?
wait "${holder_pid}" || true
if (( rc != 0 )) && [[ -z "${out}" ]]; then
    ok "forced refresh timeout does not emit stale cache"
else
    fail "forced refresh timeout does not emit stale cache" "rc=${rc}; out=${out}"
fi
# ── summary ──────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "${PASSED}" "${FAILED}"
if (( FAILED > 0 )); then
    printf 'failing tests:\n' >&2
    for f in "${FAILURES[@]}"; do printf '  - %s\n' "${f}" >&2; done
    exit 1
fi
