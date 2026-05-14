#!/usr/bin/env bash
# showy-bar — smoke tests for renderers.
#
# Each test runs a renderer against a JSON fixture, with a stub `codexbar`
# binary that just prints the fixture, and asserts the output meets a
# minimal shape. Failures print context and abort the suite.
#
# Usage: test/render_test.sh
# shellcheck disable=SC2030,SC2031

set -uo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
FIXTURE_DIR="${REPO_ROOT}/test/fixtures"

TMP=$(mktemp -d -t showy-bar-test.XXXXXX)
trap 'rm -rf "${TMP}"' EXIT

# ── stub codexbar that validates fetcher argv and prints the fixture ──

stub_dir="${TMP}/bin"
mkdir -p "${stub_dir}"
cat > "${stub_dir}/codexbar" <<'EOF'
#!/bin/sh
[ -n "${SHOWY_BAR_TEST_FIXTURE:-}" ] || exit 1
[ "${1:-}" = "usage" ] || exit 90
shift
saw_format=0
saw_pretty=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --format)
            shift
            [ "${1:-}" = "json" ] || exit 91
            saw_format=1
            ;;
        --provider)
            exit 92
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
cat "${SHOWY_BAR_TEST_FIXTURE}"
EOF
chmod +x "${stub_dir}/codexbar"

# Stub sketchybar with enough statefulness for plugin lifecycle tests.
cat > "${stub_dir}/sketchybar" <<'EOF'
#!/bin/sh
log="${SHOWY_BAR_TEST_LOG:-/dev/null}"
state_dir="${SHOWY_BAR_TEST_STATE_DIR:-}"
[ -n "${state_dir}" ] && mkdir -p "${state_dir}"
echo "sketchybar $*" >> "${log}"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --query)
            shift
            item="${1:-}"
            if [ -n "${state_dir}" ] && [ -e "${state_dir}/${item}" ]; then
                printf '{}\n'
                exit 0
            fi
            exit 1
            ;;
        --add)
            shift
            kind="${1:-}"
            shift
            name="${1:-}"
            if [ -n "${state_dir}" ] && [ -n "${name}" ]; then
                : > "${state_dir}/${name}"
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

run_renderer() {
    local renderer="$1" fixture="$2"
    shift 2
    local cache; cache=$(mk_cache)
    local fixture_file out
    fixture_file=$(fixture_path "${fixture}")
    out=$(
        env \
            PATH="${stub_dir}:${PATH}" \
            SHOWY_BAR_NO_CONFIG=1 \
            SHOWY_BAR_CACHE_DIR="${cache}" \
            SHOWY_BAR_TEST_FIXTURE="${fixture_file}" \
            SHOWY_BAR_FORCE_COLOR=1 \
            "$@" \
            "${REPO_ROOT}/bin/${renderer}" 2>&1
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
        SHOWY_BAR_NO_CONFIG=1 \
        SHOWY_BAR_CACHE_DIR="${cache}" \
        SHOWY_BAR_TEST_FIXTURE="${fixture_file}" \
        "$@" \
        "${REPO_ROOT}/bin/showy-bar-state"
}

run_theme() {
    local xdg="$1"
    shift
    env \
        PATH="${stub_dir}:${PATH}" \
        XDG_CONFIG_HOME="${xdg}" \
        "${REPO_ROOT}/bin/showy-bar" "$@"
}


run_sketchybar_items() {
    local fixture="$1" cache="$2" log="$3"
    shift 3
    local fixture_file
    fixture_file=$(fixture_path "${fixture}")
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_BAR_NO_CONFIG=1 \
        SHOWY_BAR_CACHE_DIR="${cache}" \
        SHOWY_BAR_SKETCHYBAR_IMAGE_CACHE="${cache}/sb" \
        SHOWY_BAR_TEST_FIXTURE="${fixture_file}" \
        SHOWY_BAR_TEST_LOG="${log}" \
        SHOWY_BAR_TEST_STATE_DIR="${cache}/sb-state" \
        "$@" \
        "${REPO_ROOT}/sketchybar/items/showy_bar.sh"
}

run_sketchybar_plugin() {
    local fixture="$1" cache="$2" log="$3"
    shift 3
    local fixture_file
    fixture_file=$(fixture_path "${fixture}")
    env \
        PATH="${stub_dir}:${PATH}" \
        SHOWY_BAR_NO_CONFIG=1 \
        SHOWY_BAR_CACHE_DIR="${cache}" \
        SHOWY_BAR_SKETCHYBAR_IMAGE_CACHE="${cache}/sb" \
        SHOWY_BAR_TEST_FIXTURE="${fixture_file}" \
        SHOWY_BAR_TEST_LOG="${log}" \
        "$@" \
        SHOWY_BAR_TEST_STATE_DIR="${cache}/sb-state" \
        "${REPO_ROOT}/sketchybar/plugins/showy_bar.sh"
}

run_sketchybar_plugin_without_magick() {
    local fixture="$1" cache="$2" log="$3"
    shift 3
    local fixture_file no_magick_path bash_path jq_path
    fixture_file=$(fixture_path "${fixture}")
    no_magick_path="${TMP}/no-magick-bin"
    mkdir -p "${no_magick_path}"
    bash_path=$(command -v bash)
    [[ -x /opt/homebrew/bin/bash ]] && bash_path=/opt/homebrew/bin/bash
    jq_path=$(command -v jq)
    ln -sf "${bash_path}" "${no_magick_path}/bash"
    ln -sf "${jq_path}" "${no_magick_path}/jq"
    ln -sf "${stub_dir}/codexbar" "${no_magick_path}/codexbar"
    ln -sf "${stub_dir}/sketchybar" "${no_magick_path}/sketchybar"
    env \
        PATH="${no_magick_path}:/usr/bin:/bin:/usr/sbin:/sbin" \
        SHOWY_BAR_NO_CONFIG=1 \
        SHOWY_BAR_CACHE_DIR="${cache}" \
        SHOWY_BAR_SKETCHYBAR_IMAGE_CACHE="${cache}/sb" \
        SHOWY_BAR_TEST_FIXTURE="${fixture_file}" \
        SHOWY_BAR_TEST_LOG="${log}" \
        "$@" \
        SHOWY_BAR_TEST_STATE_DIR="${cache}/sb-state" \
        "${REPO_ROOT}/sketchybar/plugins/showy_bar.sh"
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
seed_sketchybar_live_items() {
    local cache="$1"
    shift
    mkdir -p "${cache}/sb-state"
    local pid
    for pid in "$@"; do
        : > "${cache}/sb-state/showy_bar.${pid}.icon"
        : > "${cache}/sb-state/showy_bar.${pid}.primary"
        : > "${cache}/sb-state/showy_bar.${pid}.secondary"
        : > "${cache}/sb-state/showy_bar.${pid}.tertiary"
        : > "${cache}/sb-state/showy_bar.${pid}.secondary_marker"
        : > "${cache}/sb-state/showy_bar.${pid}.tertiary_marker"
        : > "${cache}/sb-state/showy_bar.${pid}.slot"
        : > "${cache}/sb-state/showy_bar.${pid}.label"
    done
    if (($# > 0)); then
        : > "${cache}/sb-state/showy_bar_bracket"
    fi
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
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        state=$(process_state "${pid}" || true)
        [[ "${state}" == "${prefix}"* ]] && return 0
        sleep 0.1
    done
    return 1
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

assert_equals() {
    local label="$1" expected="$2" actual="$3"
    if [[ "${actual}" == "${expected}" ]]; then
        ok "${label}"
    else
        fail "${label}" "expected: ${expected}"
        printf '    got: %s\n' "${actual}" >&2
    fi
}

run_common_eval() {
    local code="$1"
    shift
    # shellcheck disable=SC2016
    env \
        SHOWY_BAR_TEST_CODE="${code}" \
        SHOWY_BAR_TEST_REPO_ROOT="${REPO_ROOT}" \
        "$@" \
        bash -lc '
            set -euo pipefail
            . "${SHOWY_BAR_TEST_REPO_ROOT}/lib/common.sh"
            eval "${SHOWY_BAR_TEST_CODE}"
        '
}

hex_to_rgb_csv() {
    local hex="$1"
    printf '%d,%d,%d' $((16#${hex:0:2})) $((16#${hex:2:2})) $((16#${hex:4:2}))
}

# ── palette helpers ───────────────────────────────────────────────────
printf 'palette helpers\n'

out=$(run_common_eval 'showy_bar_scale_hex 25be6a 0.55' SHOWY_BAR_NO_CONFIG=1)
assert_equals "scale helper matches legacy 0.55 green" "14683a" "${out}"

out=$(run_common_eval 'showy_bar_role_palette primary good' SHOWY_BAR_NO_CONFIG=1)
assert_equals "primary role palette returns canonical primary color" "25be6a" "${out}"

out=$(run_common_eval 'showy_bar_role_palette secondary good' SHOWY_BAR_NO_CONFIG=1)
assert_equals "secondary role palette auto-derives from primary" "14683a" "${out}"

out=$(run_common_eval 'showy_bar_role_palette secondary good' SHOWY_BAR_NO_CONFIG=1 SHOWY_BAR_PALETTE_SECONDARY_GOOD=112233)
assert_equals "secondary role palette honors explicit override" "112233" "${out}"

out=$(run_common_eval 'showy_bar_role_palette tertiary good' SHOWY_BAR_NO_CONFIG=1)
assert_equals "tertiary role palette auto-derives from primary" "14683a" "${out}"

out=$(run_common_eval 'showy_bar_role_palette tertiary good' SHOWY_BAR_NO_CONFIG=1 SHOWY_BAR_PALETTE_TERTIARY_GOOD=445566)
assert_equals "tertiary role palette honors explicit override" "445566" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s|%s|%s" "$(showy_bar_role_color primary 50)" "$(showy_bar_role_color secondary 50)" "$(showy_bar_role_color tertiary 50)"' SHOWY_BAR_NO_CONFIG=1)
assert_equals "role color helper dispatches by role" "25be6a|14683a|14683a" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s|%s" "$(showy_bar_role_palette secondary good)" "$(showy_bar_role_palette tertiary good)"' SHOWY_BAR_NO_CONFIG=1 SHOWY_BAR_PALETTE_SECONDARY_SCALE=0.75 SHOWY_BAR_PALETTE_TERTIARY_SCALE=0.25)
assert_equals "role scale knobs recompute derived palettes" "1b8e4f|092f1a" "${out}"

out=$(run_common_eval 'showy_bar_role_palette primary good' SHOWY_BAR_NO_CONFIG=1 SHOWY_BAR_THEME=catppuccin-mocha-blue)
assert_equals "built-in Catppuccin Mocha Blue theme overrides the primary palette" "89b4fa" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s|%s|%s|%s|%s|%s|%s" "$(showy_bar_role_palette primary good)" "$(showy_bar_role_palette primary warn)" "$(showy_bar_palette bg)" "$(showy_bar_palette icon_text)" "$(showy_bar_palette countdown)" "$(showy_bar_palette countdown_warn)" "$(showy_bar_palette elapsed)"' SHOWY_BAR_NO_CONFIG=1 SHOWY_BAR_THEME=default)
assert_equals "built-in default theme exposes original palette" "25be6a|f0af00|161616|f2f4f8|7b8496|ee5396|be95ff" "${out}"

theme_xdg="${TMP}/xdg-theme"
mkdir -p "${theme_xdg}/showy-bar"
printf '%s\n' \
    'SHOWY_BAR_THEME=catppuccin-mocha-blue' \
    'SHOWY_BAR_PALETTE_PRIMARY_GOOD=010203' \
    'SHOWY_BAR_PALETTE_ICON_TEXT=020304' \
    'SHOWY_BAR_PALETTE_COUNTDOWN=030405' \
    'SHOWY_BAR_PALETTE_COUNTDOWN_WARN=040506' \
    > "${theme_xdg}/showy-bar/config.env"
out=$(run_common_eval 'showy_bar_role_palette primary good' SHOWY_BAR_NO_CONFIG= XDG_CONFIG_HOME="${theme_xdg}")
assert_equals "config env overrides themed primary palette" "010203" "${out}"

# shellcheck disable=SC2016
out=$(run_common_eval 'printf "%s|%s|%s" "$(showy_bar_palette icon_text)" "$(showy_bar_palette countdown)" "$(showy_bar_palette countdown_warn)"' SHOWY_BAR_NO_CONFIG= XDG_CONFIG_HOME="${theme_xdg}")
assert_equals "config env overrides themed text role palettes" "020304|030405|040506" "${out}"

# ── countdown formatting ──────────────────────────────────────────────
printf '\ncountdown formatting\n'

out=$(run_common_eval 'showy_bar_format_countdown ""' SHOWY_BAR_NO_CONFIG=1)
assert_equals "countdown empty is unknown" "?" "${out}"

out=$(run_common_eval 'showy_bar_format_countdown 0' SHOWY_BAR_NO_CONFIG=1)
assert_equals "countdown zero is now" "now" "${out}"

out=$(run_common_eval 'showy_bar_format_countdown 12' SHOWY_BAR_NO_CONFIG=1)
assert_equals "countdown under one hour keeps minutes" "12m" "${out}"

out=$(run_common_eval 'showy_bar_format_countdown 180' SHOWY_BAR_NO_CONFIG=1)
assert_equals "countdown whole hours stays compact" "3h" "${out}"

out=$(run_common_eval 'showy_bar_format_countdown 225' SHOWY_BAR_NO_CONFIG=1)
assert_equals "countdown mixed hours uses clock form" "3:45" "${out}"

out=$(run_common_eval 'showy_bar_format_countdown 725' SHOWY_BAR_NO_CONFIG=1)
assert_equals "countdown clock form pads minutes" "12:05" "${out}"

out=$(run_common_eval 'showy_bar_format_countdown 2880' SHOWY_BAR_NO_CONFIG=1)
assert_equals "countdown days unchanged" "2d" "${out}"

out=$(run_common_eval 'showy_bar_primary_label 12 88 "2099-01-01T00:12:00Z" 0' SHOWY_BAR_NO_CONFIG=1)
assert_equals "primary label keeps live countdown behavior" "12m" "${out}"

out=$(run_common_eval 'showy_bar_primary_label "" 100 "" 0' SHOWY_BAR_NO_CONFIG=1)
assert_equals "primary label keeps live idle behavior" "idle" "${out}"

out=$(run_common_eval 'showy_bar_primary_label 12 88 "2099-01-01T00:12:00Z" 1' SHOWY_BAR_NO_CONFIG=1)
assert_equals "primary label marks stale cache unknown" "?" "${out}"


# ── theme CLI ─────────────────────────────────────────────────────────
printf '\nshowy-bar cli\n'

theme_cli_xdg=$(mktemp -d "${TMP}/xdg-theme-list.XXXXXX")
mkdir -p "${theme_cli_xdg}/showy-bar/themes"
printf '%s\n' ": \"\${SHOWY_BAR_PALETTE_PRIMARY_GOOD:=010203}\"" > "${theme_cli_xdg}/showy-bar/themes/catppuccin-mocha-blue.env"
printf '%s\n' ": \"\${SHOWY_BAR_PALETTE_PRIMARY_GOOD:=040506}\"" > "${theme_cli_xdg}/showy-bar/themes/foo.env"
out=$(run_theme "${theme_cli_xdg}" --list)
assert_equals "theme list merges sorted unique names" $'carbonfox\ncatppuccin-frappe\ncatppuccin-latte\ncatppuccin-macchiato\ncatppuccin-mocha\ncatppuccin-mocha-blue\ndefault\ndracula\nfoo\ngruvbox-dark\nnord\ntokyonight' "${out}"

theme_current_xdg=$(mktemp -d "${TMP}/xdg-theme-current.XXXXXX")
out=$(run_theme "${theme_current_xdg}" --current)
assert_equals "theme current is none without config" "(none)" "${out}"

theme_set_xdg=$(mktemp -d "${TMP}/xdg-theme-set.XXXXXX")
theme_config="${theme_set_xdg}/showy-bar/config.env"
run_theme "${theme_set_xdg}" --set default
assert_equals "theme set creates config line" "SHOWY_BAR_THEME=default" "$(< "${theme_config}")"

theme_replace_xdg=$(mktemp -d "${TMP}/xdg-theme-replace.XXXXXX")
mkdir -p "${theme_replace_xdg}/showy-bar"
theme_config="${theme_replace_xdg}/showy-bar/config.env"
printf '%s\n' \
    'FOO=1' \
    '# SHOWY_BAR_THEME=old-comment' \
    '    SHOWY_BAR_THEME=old-active' \
    'BAR=2' \
    > "${theme_config}"
run_theme "${theme_replace_xdg}" --set default
assert_equals "theme set preserves config and replaces active line" $'FOO=1\n# SHOWY_BAR_THEME=old-comment\n    SHOWY_BAR_THEME=default\nBAR=2' "$(< "${theme_config}")"

theme_unset_xdg=$(mktemp -d "${TMP}/xdg-theme-unset.XXXXXX")
mkdir -p "${theme_unset_xdg}/showy-bar"
theme_config="${theme_unset_xdg}/showy-bar/config.env"
printf '%s\n' \
    'FOO=1' \
    '# SHOWY_BAR_THEME=old-comment' \
    'SHOWY_BAR_THEME=default' \
    'BAR=2' \
    > "${theme_config}"
run_theme "${theme_unset_xdg}" --unset
assert_equals "theme unset removes only active line" $'FOO=1\n# SHOWY_BAR_THEME=old-comment\nBAR=2' "$(< "${theme_config}")"

theme_bogus_xdg=$(mktemp -d "${TMP}/xdg-theme-bogus.XXXXXX")
mkdir -p "${theme_bogus_xdg}/showy-bar"
theme_config="${theme_bogus_xdg}/showy-bar/config.env"
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
        SHOWY_BAR_TEST_NO_FZF=1 \
        "${REPO_ROOT}/bin/showy-bar"
)
assert_contains "theme fallback prints current state" "Current theme: (none)" "${out}"
assert_contains "theme fallback prints available theme" "catppuccin-mocha-blue" "${out}"
assert_contains "theme fallback prints built-in default theme" "default" "${out}"
assert_contains "theme fallback prints set hint" "showy-bar --set <name>" "${out}"
if [[ ! -e "${theme_fallback_xdg}/showy-bar/config.env" ]]; then
    ok "theme fallback writes nothing"
else
    fail "theme fallback writes nothing"
fi

# ── zellij renderer ──────────────────────────────────────────────────

printf 'zellij renderer\n'

out=$(run_renderer showy-bar-zellij-bar codexbar-mixed.json)
assert_contains "renders CL sigil for claude"          "CL"  "${out}"
assert_contains "renders CX sigil for codex"           "CX"  "${out}"
assert_contains "renders GE sigil for gemini"          "GE"  "${out}"
assert_not_contains "skips errored provider (cursor)"  "CR"  "${out}"
assert_contains "zellij weekly hint uses derived secondary color" "20;104;58m" "${out}"
assert_contains "zellij uses ai-quota powerline left cap" "" "${out}"
assert_contains "zellij uses ai-quota half-block cells" "▀" "${out}"
assert_not_contains "zellij no longer emits old block bar" "████" "${out}"

out=$(run_renderer showy-bar-zellij-bar codexbar-empty.json)
assert_contains "empty fixture renders 'AI idle'"      "AI idle" "${out}"

out=$(run_renderer showy-bar-zellij-bar codexbar-error-only.json)
assert_contains "all-error fixture renders 'AI idle'"  "AI idle" "${out}"

out=$(run_renderer showy-bar-zellij-bar codexbar-low.json)
# Bad-palette ee5396 = decimal RGB 238;83;150 inside the truecolor escape.
assert_contains "low-remaining fixture uses BAD palette" "238;83;150" "${out}"

json_cache=$(mk_cache)
out=$(
    env \
        SHOWY_BAR_NO_CONFIG=1 \
        SHOWY_BAR_CACHE_DIR="${json_cache}" \
        SHOWY_BAR_FETCH_BIN="${TMP}/missing-fetch" \
        SHOWY_BAR_FORCE_COLOR=1 \
        "${REPO_ROOT}/bin/showy-bar-zellij-bar" --json - < "$(fixture_path codexbar-mixed.json)"
)
assert_contains "zellij --json stdin renders without fetch" "CL" "${out}"
ansi_dim=$'\x1b[2m'
assert_not_contains "zellij --json stdin skips stale dimming" "${ansi_dim}" "${out}"

# ── tmux renderer ────────────────────────────────────────────────────

printf '\ntmux renderer\n'

out=$(run_renderer showy-bar-tmux-bar codexbar-mixed.json)
assert_contains "tmux markup uses #[bold]"             "#[bold]" "${out}"
assert_contains "tmux markup names claude sigil"       "CL"      "${out}"
assert_contains "tmux markup uses #[default] reset"    "#[default]" "${out}"
assert_contains "tmux weekly hint uses derived secondary color" "#[fg=#14683a]w" "${out}"

out=$(run_renderer showy-bar-tmux-bar codexbar-empty.json)
assert_contains "tmux empty fixture renders 'AI idle'" "AI idle" "${out}"

install_bin="${TMP}/install/bin"
mkdir -p "${install_bin}"
ln -s "${REPO_ROOT}/bin/showy-bar-tmux-bar" "${install_bin}/showy-bar-tmux-bar"
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="$(mk_cache)" \
    SHOWY_BAR_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json" \
    "${install_bin}/showy-bar-tmux-bar"
)
assert_contains "installed symlink resolves repo lib" "CL" "${out}"

# ── filter ───────────────────────────────────────────────────────────

printf '\nprovider filter\n'

out=$(run_renderer showy-bar-zellij-bar codexbar-mixed.json SHOWY_BAR_PROVIDERS=claude NO_COLOR=1)
assert_contains "filter restricts to claude"           "CL" "${out}"
assert_not_contains "filter excludes codex"            "CX" "${out}"
assert_not_contains "filter excludes gemini"           "GE" "${out}"

out=$(run_renderer showy-bar-zellij-bar codexbar-mixed.json SHOWY_BAR_PROVIDERS_EXCLUDE=codex NO_COLOR=1)
assert_contains "exclude-only keeps claude"            "CL" "${out}"
assert_not_contains "exclude-only drops codex"         "CX" "${out}"
assert_contains "exclude-only keeps gemini"            "GE" "${out}"

out=$(run_renderer showy-bar-tmux-bar codexbar-mixed.json SHOWY_BAR_PROVIDERS_EXCLUDE=codex)
assert_not_contains "tmux exclude-only drops codex"    "CX" "${out}"

out=$(run_renderer showy-bar-zellij-bar codexbar-mixed.json SHOWY_BAR_PROVIDERS='claude,codex' SHOWY_BAR_PROVIDERS_EXCLUDE=codex NO_COLOR=1)
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

out=$(run_state codexbar-mixed.json SHOWY_BAR_PROVIDER_ORDER=gemini,claude)
assert_equals "provider order skips missing providers without filtering" "gemini,claude,codex" "$(printf '%s' "${out}" | jq -r '.providers | join(",")')"

out=$(run_state codexbar-mixed.json SHOWY_BAR_PROVIDER_ORDER=codex,claude,gemini SHOWY_BAR_PROVIDERS=gemini,claude)
assert_equals "allow-list order overrides provider order" "gemini,claude" "$(printf '%s' "${out}" | jq -r '.providers | join(",")')"

# ── state surface ─────────────────────────────────────────────────────
printf '\ncodexbar state\n'

out=$(run_state codexbar-mixed.json)
assert_equals "state marks cache available" "true" "$(printf '%s' "${out}" | jq -r '.available')"
assert_equals "state provider count honors renderable filter" "3" "$(printf '%s' "${out}" | jq -r '.providerCount')"
assert_equals "state provider order matches render order" "codex,claude,gemini" "$(printf '%s' "${out}" | jq -r '.providers | join(",")')"
assert_equals "state compact recommendation defaults below threshold" "false" "$(printf '%s' "${out}" | jq -r '.sketchybar.compactRecommended')"

out=$(run_state codexbar-mixed.json SHOWY_BAR_SKETCHYBAR_COMPACT_PROVIDER_COUNT=3)
assert_equals "state compact threshold is configurable" "true" "$(printf '%s' "${out}" | jq -r '.sketchybar.compactRecommended')"

out=$(run_state codexbar-mixed.json SHOWY_BAR_PROVIDERS_EXCLUDE=codex)
assert_equals "state honors provider excludes" "claude,gemini" "$(printf '%s' "${out}" | jq -r '.providers | join(",")')"

state_missing_cache=$(mk_cache)
out=$(
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${state_missing_cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${TMP}/no-such-codexbar-state" \
    "${REPO_ROOT}/bin/showy-bar-state"
)
assert_equals "state reports unavailable without cache" "false" "$(printf '%s' "${out}" | jq -r '.available')"
assert_equals "state keeps unavailable provider count empty" "0" "$(printf '%s' "${out}" | jq -r '.providerCount')"

# ── sketchybar bootstrap (without sketchybar daemon) ────────────────────

printf '\nsketchybar bootstrap\n'

cache=$(mk_cache)
log="${TMP}/sb-items.log"
run_sketchybar_items codexbar-mixed.json "${cache}" "${log}"
item_log="$(< "${log}")"
assert_contains "bootstrap declares trigger item" "showy_bar.trigger drawing=off updates=on" "${item_log}"
assert_contains "bootstrap synchronously adds provider items" "--add item showy_bar.claude.icon left" "${item_log}"
assert_contains "bootstrap adds native primary slider" "--add slider showy_bar.claude.primary left 80" "${item_log}"
assert_contains "bootstrap adds native marker overlay" "--add slider showy_bar.claude.secondary_marker left 80" "${item_log}"
assert_contains "bootstrap recreates bracket immediately" "--add bracket showy_bar_bracket" "${item_log}"
assert_contains "bootstrap preserves icon width" "width=22" "${item_log}"
assert_contains "bootstrap preserves native bar slot width" "showy_bar.claude.slot icon.drawing=off" "${item_log}"
assert_contains "bootstrap preserves native bar width" "width=84" "${item_log}"

cache=$(mk_cache)
seed_sketchybar_state "${cache}" claude codex gemini
log="${TMP}/sb-items-stale.log"
run_sketchybar_items codexbar-mixed.json "${cache}" "${log}"
item_log="$(< "${log}")"
assert_contains "bootstrap ignores stale provider state" "--add item showy_bar.gemini.icon left" "${item_log}"

cache=$(mk_cache)
seed_sketchybar_state "${cache}" codex claude gemini
log="${TMP}/sb-items-empty.log"
run_sketchybar_items codexbar-mixed.json "${cache}" "${log}" SHOWY_BAR_PROVIDERS_EXCLUDE='claude,codex,gemini'
item_log="$(< "${log}")"
assert_contains "bootstrap removes stale legacy bar item when desired set is empty" "--remove showy_bar.gemini.bar" "${item_log}"
assert_contains "bootstrap removes stale native provider items when desired set is empty" "--remove showy_bar.gemini.primary --remove showy_bar.gemini.secondary --remove showy_bar.gemini.tertiary" "${item_log}"
assert_contains "bootstrap removes stale native marker items when desired set is empty" "--remove showy_bar.gemini.secondary_marker --remove showy_bar.gemini.tertiary_marker --remove showy_bar.gemini.slot --remove showy_bar.gemini.label" "${item_log}"
assert_contains "bootstrap removes stale bracket when desired set is empty" "--remove showy_bar_bracket" "${item_log}"

cache=$(mk_cache)
log="${TMP}/sb-items-click.log"
# shellcheck disable=SC2030,SC2031
(
    PATH="${stub_dir}:${PATH}"
    export SHOWY_BAR_NO_CONFIG=1
    export SHOWY_BAR_CACHE_DIR="${cache}"
    export SHOWY_BAR_SKETCHYBAR_IMAGE_CACHE="${cache}/sb"
    export SHOWY_BAR_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-mixed.json"
    export SHOWY_BAR_TEST_LOG="${log}"
    SHOWY_BAR_SKETCHYBAR_CLICK='custom-click'
    . "${REPO_ROOT}/sketchybar/items/showy_bar.sh"
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
if grep -q 'width=84' "${log}" 2>/dev/null; then
    ok "plugin repairs native bar slot width"
else
    fail "plugin repairs native bar slot width"
fi
plugin_log="$(< "${log}")"
assert_contains "plugin updates native primary row percentage" "--set showy_bar.claude.primary drawing=on slider.percentage=83" "${plugin_log}"
assert_contains "plugin updates native secondary row percentage" "--set showy_bar.claude.secondary drawing=on slider.percentage=81" "${plugin_log}"
assert_contains "plugin hides missing tertiary row" "--set showy_bar.claude.tertiary drawing=off" "${plugin_log}"
assert_contains "plugin updates native tertiary row when present" "--set showy_bar.gemini.tertiary drawing=on slider.percentage=100" "${plugin_log}"
assert_contains "plugin uses derived secondary row color" "showy_bar.claude.secondary drawing=on slider.percentage=81 slider.highlight_color=0xff14683a" "${plugin_log}"
assert_contains "plugin uses derived tertiary row color" "showy_bar.gemini.tertiary drawing=on slider.percentage=100 slider.highlight_color=0xff14683a" "${plugin_log}"
assert_contains "plugin uses native track color" "slider.background.color=0xff3a3a4a" "${plugin_log}"
assert_contains "plugin draws elapsed marker overlay" "--set showy_bar.claude.secondary_marker drawing=on slider.percentage=100" "${plugin_log}"
assert_contains "plugin uses elapsed marker color" "slider.knob.background.color=0xffbe95ff" "${plugin_log}"
assert_contains "plugin uses two-row primary y offset" "showy_bar.claude.primary drawing=on slider.percentage=83" "${plugin_log}"
assert_contains "plugin positions two-row primary above center" "y_offset=4 click_script=command -v sketchybar" "${plugin_log}"
assert_contains "plugin positions two-row secondary below center" "showy_bar.claude.secondary drawing=on slider.percentage=81" "${plugin_log}"
assert_contains "plugin uses three-row tertiary y offset" "showy_bar.gemini.tertiary drawing=on slider.percentage=100" "${plugin_log}"
assert_contains "plugin positions tertiary below three-row stack" "y_offset=-7 click_script=command -v sketchybar" "${plugin_log}"
assert_contains "plugin click reset keeps slider rows stable" "sketchybar --set 'showy_bar.claude.primary' slider.percentage=83" "${plugin_log}"
assert_not_contains "plugin no longer writes provider bar PNGs" "bar-claude.png" "${plugin_log}"

cache=$(mk_cache)
log="${TMP}/sb-no-magick.log"
run_sketchybar_plugin_without_magick codexbar-mixed.json "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_contains "plugin updates native bars without magick" "--set showy_bar.claude.primary drawing=on slider.percentage=83" "${plugin_log}"
assert_contains "plugin hides icons when magick is unavailable" "--set showy_bar.claude.icon drawing=off click_script=open -b com.steipete.codexbar" "${plugin_log}"
if [[ ! -e "${cache}/sb/bar-claude.png" ]]; then
    ok "plugin does not rasterize bars without magick"
else
    fail "plugin does not rasterize bars without magick"
fi

cache=$(mk_cache)
log="${TMP}/sb-font-icons-no-magick.log"
run_sketchybar_plugin_without_magick codexbar-mixed.json "${cache}" "${log}" SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_MODE=font
plugin_log="$(< "${log}")"
assert_contains "plugin can draw provider icons from app font without magick" "--set showy_bar.claude.icon drawing=on icon.drawing=on icon=:claude: icon.font=sketchybar-app-font:Regular:14.0" "${plugin_log}"
assert_contains "plugin maps codex provider to app font icon" "showy_bar.codex.icon drawing=on icon.drawing=on icon=:codex:" "${plugin_log}"
assert_contains "plugin maps gemini provider to app font icon" "showy_bar.gemini.icon drawing=on icon.drawing=on icon=:gemini:" "${plugin_log}"
assert_contains "plugin widens font icon item to make a real native bar gap" "showy_bar.claude.icon drawing=on icon.drawing=on icon=:claude: icon.font=sketchybar-app-font:Regular:14.0 icon.color=0xfff2f4f8 icon.align=center icon.width=22 icon.padding_left=0 icon.padding_right=0 label.drawing=off background.image.drawing=off background.color=0x00000000 background.height=0 padding_left=5 padding_right=0 width=24" "${plugin_log}"
assert_not_contains "font icon mode avoids provider PNG cache paths" "icon-v2-" "${plugin_log}"

if command -v magick >/dev/null 2>&1; then
    cache=$(mk_cache)
    log="${TMP}/sb-status.log"
    run_sketchybar_plugin codexbar-status-major.json "${cache}" "${log}"
    if [[ -s "${cache}/sb/icon-v2-codex-major.png" ]]; then
        ok "plugin generates status-tinted icon"
    else
        fail "plugin generates status-tinted icon"
    fi
    status_log="$(< "${log}")"
    assert_contains "plugin uses status-tinted icon" "icon-v2-codex-major.png" "${status_log}"
    assert_contains "plugin routes degraded status icon to provider status page" "click_script=open 'https://status.openai.com/'" "${status_log}"

    cache=$(mk_cache)
    log="${TMP}/sb-font-status.log"
    run_sketchybar_plugin codexbar-status-major.json "${cache}" "${log}" SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_MODE=font
    font_status_log="$(< "${log}")"
    assert_contains "font icon mode colors degraded providers like PNG tinting" "showy_bar.codex.icon drawing=on icon.drawing=on icon=:codex: icon.font=sketchybar-app-font:Regular:14.0 icon.color=0xffee5396" "${font_status_log}"
    assert_contains "font icon mode preserves degraded provider status click" "click_script=open 'https://status.openai.com/'" "${font_status_log}"
    assert_not_contains "font icon mode skips status PNG for mapped provider" "icon-v2-codex-major.png" "${font_status_log}"

    opencode_fixture="${TMP}/codexbar-opencode.json"
    printf '%s\n' '[{"provider":"opencode","usage":{"primary":{"usedPercent":12,"windowMinutes":300,"resetsAt":"2099-01-01T05:40:00Z"}}}]' > "${opencode_fixture}"
    resource_dir="${TMP}/opencode-resources"
    mkdir -p "${resource_dir}"
    printf '%s\n' '<svg width="100" height="100" viewBox="0 0 100 100" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill-rule="evenodd" clip-rule="evenodd" d="M80 88H20V12H80V88ZM35 27H65V72H35V27Z" fill="#211E1E"/></svg>' > "${resource_dir}/ProviderIcon-opencode.svg"
    cache=$(mk_cache)
    log="${TMP}/sb-opencode.log"
    run_sketchybar_plugin "${opencode_fixture}" "${cache}" "${log}" SHOWY_BAR_CODEXBAR_RESOURCES="${resource_dir}"
    if [[ -s "${cache}/sb/icon-v2-opencode.png" ]]; then
        ok "plugin generates tinted dark icon"
    else
        fail "plugin generates tinted dark icon"
    fi
    opencode_mean=$(magick "${cache}/sb/icon-v2-opencode.png" -background black -alpha remove -format '%[fx:(mean.r+mean.g+mean.b)/3]' info: 2>/dev/null || true)
    if awk -v mean="${opencode_mean:-0}" 'BEGIN { exit !(mean > 0.25) }'; then
        ok "plugin tints near-black monochrome icons to text color"
    else
        fail "plugin tints near-black monochrome icons to text color" "mean=${opencode_mean}"
    fi

    cache=$(mk_cache)
    log="${TMP}/sb-opencode-font-fallback.log"
    run_sketchybar_plugin "${opencode_fixture}" "${cache}" "${log}" SHOWY_BAR_CODEXBAR_RESOURCES="${resource_dir}" SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_MODE=font
    font_fallback_log="$(< "${log}")"
    assert_contains "font icon mode falls back to SVG for unmapped opencode" "showy_bar.opencode.icon drawing=on icon.drawing=off label.drawing=off background.image=${cache}/sb/icon-v2-opencode.png" "${font_fallback_log}"
    assert_not_contains "font icon mode avoids generic code glyph for opencode" "showy_bar.opencode.icon drawing=on icon.drawing=on icon=:code:" "${font_fallback_log}"
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
run_sketchybar_plugin "${urgent_fixture}" "${cache}" "${log}" SHOWY_BAR_NOW_EPOCH=4070908800
assert_contains "plugin uses bad label color for urgent countdown" "label.color=0xffee5396" "$(< "${log}")"

printf '\nsketchybar plugin (lifecycle diff)\n'

cache=$(mk_cache)
seed_sketchybar_live_items "${cache}" claude codex
seed_sketchybar_state "${cache}" codex claude
log="${TMP}/sb-add.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_contains "plugin adds newly visible provider" "--add item showy_bar.gemini.label left" "${plugin_log}"
assert_not_contains "plugin does not re-add declared providers" "--add item showy_bar.codex.label left" "${plugin_log}"
assert_contains "plugin rebuilds bracket with added native provider" "showy_bar.gemini.icon showy_bar.gemini.primary showy_bar.gemini.secondary showy_bar.gemini.tertiary showy_bar.gemini.secondary_marker showy_bar.gemini.tertiary_marker showy_bar.gemini.slot showy_bar.gemini.label --set showy_bar_bracket" "${plugin_log}"
assert_contains "plugin triggers provider-change event" "--trigger showy_bar_provider_change SHOWY_BAR_PROVIDER_COUNT=3 SHOWY_BAR_PROVIDERS=codex,claude,gemini" "${plugin_log}"

cache=$(mk_cache)
seed_sketchybar_state "${cache}" codex claude gemini
log="${TMP}/sb-redeclare.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_contains "plugin redeclares missing live items" "--add item showy_bar.claude.icon left" "${plugin_log}"
assert_contains "plugin redeclares missing bracket when state matches" "--add bracket showy_bar_bracket" "${plugin_log}"

drop_fixture="${TMP}/codexbar-no-gemini.json"
jq '[ .[] | select(.provider != "gemini") ]' "${FIXTURE_DIR}/codexbar-mixed.json" > "${drop_fixture}"
cache=$(mk_cache)
seed_sketchybar_state "${cache}" codex claude gemini
log="${TMP}/sb-remove.log"
run_sketchybar_plugin "${drop_fixture}" "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_contains "plugin removes dropped provider legacy bar" "--remove showy_bar.gemini.icon --remove showy_bar.gemini.bar" "${plugin_log}"
assert_contains "plugin removes dropped provider native rows" "--remove showy_bar.gemini.primary --remove showy_bar.gemini.secondary --remove showy_bar.gemini.tertiary" "${plugin_log}"
assert_contains "plugin removes dropped provider native markers" "--remove showy_bar.gemini.secondary_marker --remove showy_bar.gemini.tertiary_marker --remove showy_bar.gemini.slot --remove showy_bar.gemini.label" "${plugin_log}"

cache=$(mk_cache)
seed_sketchybar_state "${cache}" codex claude gemini
seed_sketchybar_live_items "${cache}" codex claude gemini
log="${TMP}/sb-unchanged.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}"
plugin_log="$(< "${log}")"
assert_not_contains "plugin unchanged set skips item adds" "--add item showy_bar." "${plugin_log}"
assert_not_contains "plugin unchanged set skips bracket rebuild" "--add bracket showy_bar_bracket" "${plugin_log}"
assert_not_contains "plugin unchanged set skips provider removals" "--remove showy_bar." "${plugin_log}"
assert_not_contains "plugin unchanged set skips bracket removal" "--remove showy_bar_bracket" "${plugin_log}"
assert_contains "plugin unchanged set still updates providers" "--set showy_bar.claude.label" "${plugin_log}"

cache=$(mk_cache)
log="${TMP}/sb-filter.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}" SHOWY_BAR_PROVIDERS_EXCLUDE=codex
plugin_log="$(< "${log}")"
assert_contains "sketchybar exclude-only keeps claude" "showy_bar.claude.label" "${plugin_log}"
assert_not_contains "sketchybar exclude-only drops codex" "showy_bar.codex.label" "${plugin_log}"
assert_contains "sketchybar exclude-only keeps gemini" "showy_bar.gemini.label" "${plugin_log}"

cache=$(mk_cache)
log="${TMP}/sb-filter-overlap.log"
run_sketchybar_plugin codexbar-mixed.json "${cache}" "${log}" SHOWY_BAR_PROVIDERS='claude,codex' SHOWY_BAR_PROVIDERS_EXCLUDE=codex
plugin_log="$(< "${log}")"
assert_contains "sketchybar include+exclude keeps claude" "showy_bar.claude.label" "${plugin_log}"
assert_not_contains "sketchybar include+exclude drops codex" "showy_bar.codex.label" "${plugin_log}"
assert_not_contains "sketchybar include+exclude drops gemini" "showy_bar.gemini.label" "${plugin_log}"
# ── schema drift / edge JSON ────────────────────────────────────────

printf '\nschema drift\n'

# 1. Float usedPercent must not crash bash arithmetic.
out=$(run_renderer showy-bar-zellij-bar codexbar-realistic.json)
assert_contains "float usedPercent renders codex"      "CX" "${out}"
assert_contains "float usedPercent renders claude"     "CL" "${out}"
assert_contains "float usedPercent uses GOOD palette" "37;190;106" "${out}"

out=$(run_renderer showy-bar-tmux-bar codexbar-realistic.json)
assert_contains "tmux float usedPercent renders codex" "CX" "${out}"

# 2. Provider with usage.primary but no resetsAt must render '?' not crash.
out=$(run_renderer showy-bar-zellij-bar codexbar-no-reset.json)
assert_contains "no-reset fixture still renders codex" "CX" "${out}"
assert_contains "no-reset fixture shows '?' countdown" "?"  "${out}"

out=$(run_renderer showy-bar-zellij-bar codexbar-reset-description.json)
assert_contains "resetDescription fixture renders codex" "CX" "${out}"
assert_not_contains "resetDescription fixture avoids '?' countdown" "?" "${out}"


out=$(run_renderer showy-bar-zellij-bar codexbar-idle-no-reset.json)
assert_contains "idle-no-reset fixture renders claude" "CL" "${out}"
assert_contains "idle-no-reset fixture shows idle label" "idle" "${out}"
# 3. Non-array JSON must be rejected by the fetcher (refresh path).
printf '\ncache fetcher\n'

cache=$(mk_cache)
rc=0
out=$(
    PATH="${stub_dir}:${PATH}" \
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-non-array.json" \
    "${REPO_ROOT}/bin/showy-bar-fetch" 2>&1
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
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${missing_bin:-${TMP}/no-such-codexbar}" \
    "${REPO_ROOT}/bin/showy-bar-fetch" 2>/dev/null
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
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_TEST_FIXTURE="${bad_provider}" \
    "${REPO_ROOT}/bin/showy-bar-fetch" 2>/dev/null
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
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${missing_bin}" \
    "${REPO_ROOT}/bin/showy-bar-fetch" 2>&1
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
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${missing_bin}" \
    "${REPO_ROOT}/bin/showy-bar-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array"' >/dev/null 2>&1; then
    ok "fetcher serves stale cache when codexbar disappears"
else
    fail "fetcher serves stale cache when codexbar disappears" "rc=${rc}"
fi

# 6. Stale-cache rendering marks countdowns unknown without dimming quota color.
printf '\nstale cache rendering\n'

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-mixed.json" "${cache}/usage.json"
# Backdate cache to 1988 so age is decades, well beyond 2 * default REFRESH_SECONDS.
touch -t 198801010000 "${cache}/usage.json"
# Use a bogus codexbar bin so fetch cannot refresh the backdated cache.
ansi_dim=$'\x1b[2m'
countdown_warn_rgb=$(hex_to_rgb_csv "$(run_common_eval 'showy_bar_palette countdown_warn' SHOWY_BAR_NO_CONFIG=1)")
countdown_warn_sgr="${countdown_warn_rgb//,/;}"
surface_rgb=$(hex_to_rgb_csv "$(run_common_eval 'showy_bar_palette surface' SHOWY_BAR_NO_CONFIG=1)")
surface_sgr="${surface_rgb//,/;}"
printf -v stale_countdown_escape '\033[38;2;%sm\033[48;2;%sm?' "${countdown_warn_sgr}" "${surface_sgr}"
out=$(
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    SHOWY_BAR_FORCE_COLOR=1 \
    "${REPO_ROOT}/bin/showy-bar-zellij-bar"
)
assert_not_contains "zellij does not dim stale cache" "${ansi_dim}" "${out}"
assert_contains "zellij stale countdown uses warn palette" "${stale_countdown_escape}" "${out}"
question_stripped="${out//\?/}"
question_count=$(( ${#out} - ${#question_stripped} ))
if (( question_count == 3 )); then
    ok "zellij marks every stale provider countdown unknown"
else
    fail "zellij marks every stale provider countdown unknown" "question_count=${question_count}"
fi

out=$(
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    "${REPO_ROOT}/bin/showy-bar-tmux-bar"
)
assert_not_contains "tmux does not dim stale cache" "#[dim]" "${out}"
assert_contains "tmux stale countdown uses warn palette" "#[fg=#ee5396]?" "${out}"
assert_not_contains "tmux stale cache suppresses weekly hint" "]w" "${out}"

stale_past_fixture="${TMP}/codexbar-stale-past.json"
printf '%s\n' '[{"provider":"claude","usage":{"primary":{"usedPercent":17,"windowMinutes":300,"resetsAt":"1988-01-01T00:00:00Z"}}}]' > "${stale_past_fixture}"
cache=$(mk_cache)
cp "${stale_past_fixture}" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
out=$(
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${TMP}/no-such-codexbar" \
    SHOWY_BAR_FORCE_COLOR=1 \
    "${REPO_ROOT}/bin/showy-bar-zellij-bar"
)
assert_contains "zellij stale absolute reset shows unknown countdown" "?" "${out}"
assert_not_contains "zellij stale absolute reset does not show now" "now" "${out}"

# 7. Concurrent fetch — only one codexbar invocation across simultaneous
#    callers. We exercise both lock paths via SHOWY_BAR_FORCE_NO_FLOCK.
printf '\nconcurrent fetch\n'

slow_dir="${TMP}/slow"
mkdir -p "${slow_dir}"
cat > "${slow_dir}/codexbar" <<EOF
#!/bin/sh
[ -n "\${SHOWY_BAR_TEST_COUNTER:-}" ] && printf 'x' >> "\${SHOWY_BAR_TEST_COUNTER}"
sleep 1
cat "\${SHOWY_BAR_TEST_FIXTURE:-${FIXTURE_DIR}/codexbar-mixed.json}"
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
            SHOWY_BAR_NO_CONFIG=1 \
            SHOWY_BAR_CACHE_DIR="${cache}" \
            SHOWY_BAR_CODEXBAR_BIN="${slow_dir}/codexbar" \
            SHOWY_BAR_FORCE_NO_FLOCK="${force_no_flock}" \
            SHOWY_BAR_TEST_COUNTER="${counter}" \
            "${REPO_ROOT}/bin/showy-bar-fetch" > "${out_file}" 2>/dev/null
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
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_BAR_FORCE_NO_FLOCK=1 \
    SHOWY_BAR_TEST_COUNTER="${counter}" \
    "${REPO_ROOT}/bin/showy-bar-fetch" 2>/dev/null
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
        SHOWY_BAR_NO_CONFIG=1 \
        SHOWY_BAR_CACHE_DIR="${cache}" \
        SHOWY_BAR_CODEXBAR_BIN="${slow_dir}/codexbar" \
        SHOWY_BAR_FORCE_NO_FLOCK=1 \
        SHOWY_BAR_TEST_COUNTER="${counter}" \
        "${REPO_ROOT}/bin/showy-bar-fetch" 2>/dev/null
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
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_BAR_FORCE_NO_FLOCK=1 \
    SHOWY_BAR_TEST_COUNTER="${counter}" \
    "${REPO_ROOT}/bin/showy-bar-fetch" 2>/dev/null
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
        SHOWY_BAR_NO_CONFIG=1 \
        SHOWY_BAR_CACHE_DIR="${cache}" \
        SHOWY_BAR_CODEXBAR_BIN="${slow_dir}/codexbar" \
        SHOWY_BAR_FORCE_NO_FLOCK=1 \
        SHOWY_BAR_LOCK_WAIT_TENTHS=1 \
        SHOWY_BAR_TEST_COUNTER="${counter}" \
        "${REPO_ROOT}/bin/showy-bar-fetch" 2>/dev/null
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
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_BAR_FORCE_NO_FLOCK=1 \
    SHOWY_BAR_LOCK_WAIT_TENTHS=10 \
    SHOWY_BAR_TEST_COUNTER="${counter}" \
    "${REPO_ROOT}/bin/showy-bar-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && printf '%s' "${out}" | jq -e 'type == "array"' >/dev/null 2>&1; then
    ok "mkdir path: retries empty owner lock after wait"
else
    fail "mkdir path: retries empty owner lock after wait" "rc=${rc}"
fi

cache=$(mk_cache)
cp "${FIXTURE_DIR}/codexbar-low.json" "${cache}/usage.json"
touch -t 198801010000 "${cache}/usage.json"
counter="${cache}/retry-stale-empty-lock-call-count"
: > "${counter}"
mkdir "${cache}/usage.lock.d"
: > "${cache}/usage.lock.d/owner.pid"
rc=0
out=$(
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_BAR_FORCE_NO_FLOCK=1 \
    SHOWY_BAR_LOCK_WAIT_TENTHS=10 \
    SHOWY_BAR_TEST_COUNTER="${counter}" \
    SHOWY_BAR_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-bar-fetch" 2>/dev/null
) || rc=$?
if (( rc == 0 )) && grep -F -q 'futureUnknownTopLevelField' <<< "${out}"; then
    ok "mkdir path: retries stale empty owner lock before fallback"
else
    fail "mkdir path: retries stale empty owner lock before fallback" "rc=${rc}; out=${out}"
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
        SHOWY_BAR_NO_CONFIG=1 \
        SHOWY_BAR_CACHE_DIR="${cache}" \
        SHOWY_BAR_CODEXBAR_BIN="${slow_dir}/codexbar" \
        SHOWY_BAR_TEST_COUNTER="${counter}" \
        SHOWY_BAR_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
        "${REPO_ROOT}/bin/showy-bar-fetch" --refresh > "${out_file}" 2>/dev/null
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
counter="${cache}/forced-refresh-empty-lock-call-count"
: > "${counter}"
mkdir "${cache}/usage.lock.d"
: > "${cache}/usage.lock.d/owner.pid"
rc=0
out=$(
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${slow_dir}/codexbar" \
    SHOWY_BAR_FORCE_NO_FLOCK=1 \
    SHOWY_BAR_LOCK_WAIT_TENTHS=10 \
    SHOWY_BAR_TEST_COUNTER="${counter}" \
    SHOWY_BAR_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-bar-fetch" --refresh 2>/dev/null
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
[ -n "${SHOWY_BAR_TEST_COUNTER:-}" ] && printf 'x' >> "${SHOWY_BAR_TEST_COUNTER}"
sleep 5
cat "${SHOWY_BAR_TEST_FIXTURE:-${FIXTURE_DIR}/codexbar-mixed.json}"
EOF
chmod +x "${timeout_dir}/codexbar"
(
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${timeout_dir}/codexbar" \
    SHOWY_BAR_FORCE_NO_FLOCK=1 \
    SHOWY_BAR_TEST_COUNTER="${counter}" \
    SHOWY_BAR_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-bar-fetch" --refresh >/dev/null 2>/dev/null
) &
holder_pid=$!
sleep 0.2
rc=0
out=$(
    SHOWY_BAR_NO_CONFIG=1 \
    SHOWY_BAR_CACHE_DIR="${cache}" \
    SHOWY_BAR_CODEXBAR_BIN="${timeout_dir}/codexbar" \
    SHOWY_BAR_FORCE_NO_FLOCK=1 \
    SHOWY_BAR_LOCK_WAIT_TENTHS=0 \
    SHOWY_BAR_TEST_COUNTER="${counter}" \
    SHOWY_BAR_TEST_FIXTURE="${FIXTURE_DIR}/codexbar-realistic.json" \
    "${REPO_ROOT}/bin/showy-bar-fetch" --refresh 2>/dev/null
) || rc=$?
wait "${holder_pid}" || true
if (( rc == 0 )) && [[ "${out}" == "${expected_stale}" ]]; then
    ok "forced refresh timeout falls back to stale cache"
else
    fail "forced refresh timeout falls back to stale cache" "rc=${rc}; out=${out}"
fi
# ── summary ──────────────────────────────────────────────────────────

printf '\n%d passed, %d failed\n' "${PASSED}" "${FAILED}"
if (( FAILED > 0 )); then
    printf 'failing tests:\n' >&2
    for f in "${FAILURES[@]}"; do printf '  - %s\n' "${f}" >&2; done
    exit 1
fi
