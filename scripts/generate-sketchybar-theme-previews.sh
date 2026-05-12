#!/bin/bash
# Generate README SketchyBar theme previews from the real plugin PNG renderer.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
OUT_DIR="${SHOWY_BAR_SKETCH_PREVIEW_OUT_DIR:-${REPO_ROOT}/docs/images/themes}"
BASH_BIN="${SHOWY_BAR_SKETCH_PREVIEW_BASH_BIN:-/opt/homebrew/bin/bash}"
CODEXBAR_RESOURCES="${SHOWY_BAR_CODEXBAR_RESOURCES:-/Applications/CodexBar.app/Contents/Resources}"
NOW_EPOCH="4070908800"

if [[ ! -x "${BASH_BIN}" ]]; then
    BASH_BIN="$(command -v bash || true)"
fi
if [[ -z "${BASH_BIN}" || ! -x "${BASH_BIN}" ]]; then
    printf 'showy-bar: bash is required for SketchyBar previews\n' >&2
    exit 1
fi

for tool in base64 jq magick; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        printf 'showy-bar: %s is required for SketchyBar previews\n' "${tool}" >&2
        exit 1
    fi
done

mkdir -p -- "${OUT_DIR}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/showy-sketch-preview.XXXXXX")"
cleanup() { rm -rf -- "${TMP_DIR}"; }
trap cleanup EXIT

fixture_json() {
    cat <<'JSON'
[
  {"provider":"claude","usage":{
    "primary":  {"usedPercent":17,"resetsAt":"2099-01-01T03:29:00Z","windowMinutes":300},
    "secondary":{"usedPercent":70,"resetsAt":"2099-01-03T00:00:00Z","windowMinutes":10080}
  }},
  {"provider":"codex","usage":{
    "primary":  {"usedPercent":92,"resetsAt":"2099-01-01T00:23:00Z","windowMinutes":10080},
    "secondary":{"usedPercent":35,"resetsAt":"2099-01-04T12:00:00Z","windowMinutes":10080}
  }}
]
JSON
}

write_fetch_stub() {
    local path="$1"
    cat > "${path}" <<'EOF'
#!/bin/bash
printf '%s\n' "${SHOWY_BAR_PREVIEW_FIXTURE}"
EOF
    chmod +x "${path}"
}

write_sketchybar_stub() {
    local path="$1"
    cat > "${path}" <<'EOF'
#!/bin/bash
exit 0
EOF
    chmod +x "${path}"
}

palette_value() {
    local theme="$1" key="$2"
    # shellcheck disable=SC2016
    SHOWY_BAR_THEME="${theme}" \
        SHOWY_BAR_NO_CONFIG=1 \
        "${BASH_BIN}" -c '. ./lib/common.sh; showy_bar_palette "$1"' _ "${key}"
}

png_data_uri() {
    local path="$1"
    printf 'data:image/png;base64,%s' "$(base64 < "${path}" | tr -d '\n')"
}

strip_png_metadata() {
    local path="$1" tmp
    tmp="${path}.strip.png"
    magick "${path}" -strip "${tmp}"
    mv -f -- "${tmp}" "${path}"
}

render_svg() {
    local theme="$1" cache_dir="$2" out_file="$3"
    local countdown countdown_warn
    local claude_icon claude_bar codex_icon codex_bar

    countdown="$(palette_value "${theme}" countdown)"
    countdown_warn="$(palette_value "${theme}" countdown_warn)"
    claude_icon="$(png_data_uri "${cache_dir}/icon-v2-claude.png")"
    claude_bar="$(png_data_uri "${cache_dir}/bar-claude.png")"
    codex_icon="$(png_data_uri "${cache_dir}/icon-v2-codex.png")"
    codex_bar="$(png_data_uri "${cache_dir}/bar-codex.png")"

    cat > "${out_file}" <<EOF
<svg xmlns="http://www.w3.org/2000/svg" width="324" height="28" viewBox="0 0 324 28" role="img">
<title>${theme} SketchyBar rendered preview</title>
<rect x="0" y="0" width="324" height="28" rx="14" fill="transparent"/>
<image href="${claude_icon}" x="0" y="3" width="22" height="22" preserveAspectRatio="xMidYMid meet"/>
<image href="${claude_bar}" x="27" y="5" width="80" height="18" preserveAspectRatio="none"/>
<text x="115" y="14" dominant-baseline="middle" font-family="SF Pro Text, SF Pro, -apple-system, BlinkMacSystemFont, Helvetica, Arial, sans-serif" font-size="12" font-weight="600" fill="#${countdown}">3:29</text>
<image href="${codex_icon}" x="167" y="3" width="22" height="22" preserveAspectRatio="xMidYMid meet"/>
<image href="${codex_bar}" x="194" y="5" width="80" height="18" preserveAspectRatio="none"/>
<text x="282" y="14" dominant-baseline="middle" font-family="SF Pro Text, SF Pro, -apple-system, BlinkMacSystemFont, Helvetica, Arial, sans-serif" font-size="12" font-weight="600" fill="#${countdown_warn}">23m</text>
</svg>
EOF
    chmod 0644 "${out_file}"
}

render_theme() {
    local theme="$1"
    local cache_dir="${TMP_DIR}/${theme}/cache"
    local stub_dir="${TMP_DIR}/${theme}/bin"
    local fetch_bin="${TMP_DIR}/${theme}/fetch.sh"
    local svg_out="${OUT_DIR}/${theme}-sketchybar.svg"

    mkdir -p -- "${cache_dir}" "${stub_dir}"
    write_fetch_stub "${fetch_bin}"
    write_sketchybar_stub "${stub_dir}/sketchybar"

    SHOWY_BAR_THEME="${theme}" \
        SHOWY_BAR_NO_CONFIG=1 \
        SHOWY_BAR_NOW_EPOCH="${NOW_EPOCH}" \
        SHOWY_BAR_FETCH_BIN="${fetch_bin}" \
        SHOWY_BAR_PREVIEW_FIXTURE="$(fixture_json)" \
        SHOWY_BAR_SKETCHYBAR_IMAGE_CACHE="${cache_dir}" \
        SHOWY_BAR_CODEXBAR_RESOURCES="${CODEXBAR_RESOURCES}" \
        PATH="${stub_dir}:${PATH}" \
        "${BASH_BIN}" "${REPO_ROOT}/sketchybar/plugins/showy_bar.sh"

    strip_png_metadata "${cache_dir}/icon-v2-claude.png"
    strip_png_metadata "${cache_dir}/bar-claude.png"
    strip_png_metadata "${cache_dir}/icon-v2-codex.png"
    strip_png_metadata "${cache_dir}/bar-codex.png"

    render_svg "${theme}" "${cache_dir}" "${svg_out}"
    printf 'rendered %s\n' "${svg_out}"
}

main() {
    local theme
    if (( $# > 0 )); then
        for theme in "$@"; do
            [[ -n "${theme}" ]] || continue
            render_theme "${theme}"
        done
        return 0
    fi

    while IFS= read -r theme; do
        [[ -n "${theme}" ]] || continue
        render_theme "${theme}"
    done < <("${REPO_ROOT}/bin/showy-bar" --list)
}

main "$@"
