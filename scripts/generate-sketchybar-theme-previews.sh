#!/bin/bash
# Generate README SketchyBar theme previews from real plugin-rendered icons
# and SVG-native rows that mirror the SketchyBar slider layout.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
OUT_DIR="${SHOWY_BAR_SKETCH_PREVIEW_OUT_DIR:-${REPO_ROOT}/docs/images/themes}"
BASH_BIN="${SHOWY_BAR_SKETCH_PREVIEW_BASH_BIN:-/opt/homebrew/bin/bash}"
CODEXBAR_RESOURCES="${SHOWY_BAR_CODEXBAR_RESOURCES:-/Applications/CodexBar.app/Contents/Resources}"
NOW_EPOCH="4070908800"
BAR_W=80
ROW_H=6
ROW_RX=3

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
    SHOWY_BAR_THEME="${theme}" \
        SHOWY_BAR_NO_CONFIG=1 \
        "${BASH_BIN}" -c '. "$1"; showy_bar_palette "$2"' _ "${REPO_ROOT}/lib/common.sh" "${key}"
}

role_color() {
    local theme="$1" role="$2" remaining="$3"
    SHOWY_BAR_THEME="${theme}" \
        SHOWY_BAR_NO_CONFIG=1 \
        "${BASH_BIN}" -c '. "$1"; showy_bar_role_color "$2" "$3"' _ "${REPO_ROOT}/lib/common.sh" "${role}" "${remaining}"
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

elapsed_marker_x() {
    local reset_at="$1" window_minutes="$2" width="${3:-${BAR_W}}"
    local reset_epoch duration start_epoch elapsed marker
    [[ -n "${reset_at}" && "${window_minutes}" =~ ^[0-9]+$ ]] || return 1
    (( window_minutes > 0 )) || return 1

    reset_epoch=$(SHOWY_BAR_NO_CONFIG=1 "${BASH_BIN}" -c '. "$1"; showy_bar_reset_epoch "$2"' _ "${REPO_ROOT}/lib/common.sh" "${reset_at}") || return 1
    duration=$((window_minutes * 60))
    start_epoch=$((reset_epoch - duration))
    elapsed=$((NOW_EPOCH - start_epoch))
    (( elapsed < 0 )) && elapsed=0
    (( elapsed > duration )) && elapsed="${duration}"
    marker=$(( (duration - elapsed) * width / duration ))
    (( marker < 0 )) && marker=0
    (( marker >= width )) && marker=$((width - 1))
    printf '%s\n' "${marker}"
}

draw_usage_row() {
    local x="$1" y="$2" remaining="$3" fill_hex="$4" track_hex="$5"
    local fill_w
    [[ "${remaining}" =~ ^[0-9]+$ ]] || remaining=0
    (( remaining > 100 )) && remaining=100
    fill_w=$((BAR_W * remaining / 100))
    (( remaining > 0 && fill_w == 0 )) && fill_w=1

    printf '<rect x="%s" y="%s" width="%s" height="%s" rx="%s" fill="#%s"/>\n' \
        "${x}" "${y}" "${BAR_W}" "${ROW_H}" "${ROW_RX}" "${track_hex}"
    if (( fill_w > 0 )); then
        printf '<rect x="%s" y="%s" width="%s" height="%s" rx="%s" fill="#%s"/>\n' \
            "${x}" "${y}" "${fill_w}" "${ROW_H}" "${ROW_RX}" "${fill_hex}"
    fi
}

draw_marker() {
    local x="$1" y="$2" marker="$3" elapsed_hex="$4"
    [[ "${marker}" =~ ^[0-9]+$ ]] || return 0
    printf '<rect x="%s" y="%s" width="1" height="%s" fill="#%s"/>\n' \
        "$((x + marker))" "${y}" "${ROW_H}" "${elapsed_hex}"
}

draw_provider() {
    local theme="$1" cache_dir="$2" provider="$3" icon_x="$4" bar_x="$5" label_x="$6" label="$7" label_hex="$8" rem_p="$9"
    local rem_s="${10}" s_reset="${11}" s_window="${12}"
    local icon primary_hex secondary_hex track_hex elapsed_hex marker

    icon="$(png_data_uri "${cache_dir}/icon-v2-${provider}.png")"
    primary_hex="$(role_color "${theme}" primary "${rem_p}")"
    secondary_hex="$(role_color "${theme}" secondary "${rem_s}")"
    track_hex="$(palette_value "${theme}" track)"
    elapsed_hex="$(palette_value "${theme}" elapsed)"
    marker="$(elapsed_marker_x "${s_reset}" "${s_window}" "${BAR_W}" || true)"

    printf '<image href="%s" x="%s" y="3" width="22" height="22" preserveAspectRatio="xMidYMid meet"/>\n' "${icon}" "${icon_x}"
    draw_usage_row "${bar_x}" 7 "${rem_p}" "${primary_hex}" "${track_hex}"
    draw_usage_row "${bar_x}" 15 "${rem_s}" "${secondary_hex}" "${track_hex}"
    draw_marker "${bar_x}" 15 "${marker}" "${elapsed_hex}"
    printf '<text x="%s" y="14" dominant-baseline="middle" font-family="SF Pro Text, SF Pro, -apple-system, BlinkMacSystemFont, Helvetica, Arial, sans-serif" font-size="12" font-weight="600" fill="#%s">%s</text>\n' "${label_x}" "${label_hex}" "${label}"
}

render_svg() {
    local theme="$1" cache_dir="$2" out_file="$3"
    local countdown countdown_warn

    countdown="$(palette_value "${theme}" countdown)"
    countdown_warn="$(palette_value "${theme}" countdown_warn)"

    {
        printf '<svg xmlns="http://www.w3.org/2000/svg" width="324" height="28" viewBox="0 0 324 28" role="img">\n'
        printf '<title>%s SketchyBar rendered preview</title>\n' "${theme}"
        printf '<rect x="0" y="0" width="324" height="28" rx="14" fill="transparent"/>\n'
        draw_provider "${theme}" "${cache_dir}" claude 0 27 115 '3:29' "${countdown}" 83 30 '2099-01-03T00:00:00Z' 10080
        draw_provider "${theme}" "${cache_dir}" codex 167 194 282 '23m' "${countdown_warn}" 8 65 '2099-01-04T12:00:00Z' 10080
        printf '</svg>\n'
    } > "${out_file}"
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
    strip_png_metadata "${cache_dir}/icon-v2-codex.png"

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
