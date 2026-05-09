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

# ── stub codexbar that just prints whatever fixture we point it at ──

stub_dir="${TMP}/bin"
mkdir -p "${stub_dir}"
cat > "${stub_dir}/codexbar" <<'EOF'
#!/bin/sh
[ -n "${CB_BARS_TEST_FIXTURE:-}" ] || exit 1
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

# ── schema drift / edge JSON ────────────────────────────────────────

printf '\nschema drift\n'

# 1. Float usedPercent must not crash bash arithmetic.
out=$(run_renderer cb-bars-zellij-bar codexbar-realistic.json)
assert_contains "float usedPercent renders codex"      "CX" "${out}"
assert_contains "float usedPercent renders claude"     "CL" "${out}"

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
cat "${FIXTURE_DIR}/codexbar-mixed.json"
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
    for _ in 1 2 3 4; do
        (
            CB_BARS_NO_CONFIG=1 \
            CB_BARS_CACHE_DIR="${cache}" \
            CB_BARS_CODEXBAR_BIN="${slow_dir}/codexbar" \
            CB_BARS_FORCE_NO_FLOCK="${force_no_flock}" \
            CB_BARS_TEST_COUNTER="${counter}" \
            "${REPO_ROOT}/bin/cb-bars-fetch" >/dev/null 2>&1
        ) &
    done
    wait
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
# ── summary ──────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "${PASSED}" "${FAILED}"
if (( FAILED > 0 )); then
    printf 'failing tests:\n' >&2
    for f in "${FAILURES[@]}"; do printf '  - %s\n' "${f}" >&2; done
    exit 1
fi
