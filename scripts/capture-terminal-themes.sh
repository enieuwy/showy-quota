#!/bin/bash
# Capture real Ghostty previews for the README theme gallery.
#
# The previews use a decorationless Ghostty window so the output starts at the
# terminal grid's top-left cell. Capture is done from a full-screen image and
# cropped from CoreGraphics window bounds so macOS window borders/shadows do not
# leak into the final PNGs.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"

GHOSTTY_APP="${SHOWY_BAR_CAPTURE_GHOSTTY_APP:-/Applications/Ghostty.app}"
GHOSTTY_BIN="${SHOWY_BAR_CAPTURE_GHOSTTY_BIN:-${GHOSTTY_APP}/Contents/MacOS/ghostty}"
BASH_BIN="${SHOWY_BAR_CAPTURE_BASH_BIN:-/opt/homebrew/bin/bash}"
OUT_DIR="${SHOWY_BAR_CAPTURE_OUT_DIR:-${REPO_ROOT}/docs/images/themes}"
OUTPUT_W="${SHOWY_BAR_CAPTURE_OUTPUT_W:-968}"
OUTPUT_H="${SHOWY_BAR_CAPTURE_OUTPUT_H:-48}"
OUTPUT_X="${SHOWY_BAR_CAPTURE_OUTPUT_X:-0}"
OUTPUT_Y="${SHOWY_BAR_CAPTURE_OUTPUT_Y:-0}"
WARMUP_SECONDS="${SHOWY_BAR_CAPTURE_WARMUP_SECONDS:-1.8}"
HOLD_SECONDS="${SHOWY_BAR_CAPTURE_HOLD_SECONDS:-5}"
FONT_SIZE="${SHOWY_BAR_CAPTURE_FONT_SIZE:-18}"
COLUMNS="${SHOWY_BAR_CAPTURE_COLUMNS:-88}"
LINES="${SHOWY_BAR_CAPTURE_LINES:-2}"
WINDOW_X="${SHOWY_BAR_CAPTURE_WINDOW_X:-137}"
WINDOW_Y="${SHOWY_BAR_CAPTURE_WINDOW_Y:-113}"

if [[ ! -x "${GHOSTTY_BIN}" ]]; then
    printf 'showy-bar: Ghostty binary not found: %s\n' "${GHOSTTY_BIN}" >&2
    printf 'showy-bar: set SHOWY_BAR_CAPTURE_GHOSTTY_APP or SHOWY_BAR_CAPTURE_GHOSTTY_BIN to override it\n' >&2
    exit 1
fi
if [[ ! -x "${BASH_BIN}" ]]; then
    BASH_BIN="$(command -v bash || true)"
fi
if [[ -z "${BASH_BIN}" || ! -x "${BASH_BIN}" ]]; then
    printf 'showy-bar: bash is required for terminal captures\n' >&2
    exit 1
fi

for tool in awk magick open screencapture swift; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        printf 'showy-bar: %s is required for terminal captures\n' "${tool}" >&2
        exit 1
    fi
done

mkdir -p -- "${OUT_DIR}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/showy-terminal-capture.XXXXXX")"
active_config=""
cleanup() {
    if [[ -n "${active_config}" ]]; then
        pkill -f "${active_config}" >/dev/null 2>&1 || true
    fi
    rm -rf -- "${TMP_DIR}"
}
trap cleanup EXIT

WINDOW_BOUNDS_SCRIPT="${TMP_DIR}/window-bounds.swift"
cat > "${WINDOW_BOUNDS_SCRIPT}" <<'SWIFT'
import CoreGraphics
import Foundation

if CommandLine.arguments.count < 4 {
    exit(2)
}

let expectedX = Int(CommandLine.arguments[1]) ?? 0
let minWidth = Int(CommandLine.arguments[2]) ?? 0
let minHeight = Int(CommandLine.arguments[3]) ?? 0
let options = CGWindowListOption(arrayLiteral: .optionOnScreenOnly, .excludeDesktopElements)
let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []

for window in windows {
    let owner = (window[kCGWindowOwnerName as String] as? String) ?? ""
    guard owner == "Ghostty" else { continue }
    guard let bounds = window[kCGWindowBounds as String] as? [String: Any],
          let x = bounds["X"] as? NSNumber,
          let y = bounds["Y"] as? NSNumber,
          let width = bounds["Width"] as? NSNumber,
          let height = bounds["Height"] as? NSNumber else { continue }

    if abs(x.intValue - expectedX) <= 2 &&
        width.intValue >= minWidth &&
        height.intValue >= minHeight {
        print("\(x.intValue),\(y.intValue),\(width.intValue),\(height.intValue)")
        exit(0)
    }
}

exit(1)
SWIFT

SCREEN_SCALE_SCRIPT="${TMP_DIR}/screen-scale.swift"
cat > "${SCREEN_SCALE_SCRIPT}" <<'SWIFT'
import AppKit
import Foundation

print(NSScreen.main?.backingScaleFactor ?? 1.0)
SWIFT

ghostty_string() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    printf '"%s"' "${value}"
}

palette_value() {
    local theme="$1" key="$2"
    # shellcheck disable=SC2016
    SHOWY_BAR_THEME="${theme}" \
        SHOWY_BAR_NO_CONFIG=1 \
        "${BASH_BIN}" -c '. ./lib/common.sh; showy_bar_palette "$1"' _ "${key}"
}

write_command_script() {
    local theme="$1" command_file="$2"
    cat > "${command_file}" <<EOF
#!${BASH_BIN}
set -euo pipefail
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:\${PATH:-}"
cd "${REPO_ROOT}"
printf '\\033[?25l\\033[2J\\033[H'
SHOWY_BAR_THEME="${theme}" \\
    SHOWY_BAR_NO_CONFIG=1 \\
    SHOWY_BAR_FORCE_COLOR=1 \\
    SHOWY_BAR_NOW_EPOCH=4070908800 \\
    "${REPO_ROOT}/bin/showy-bar" --preview "${theme}"
sleep "${HOLD_SECONDS}"
EOF
    chmod +x "${command_file}"
}

write_ghostty_config() {
    local bg="$1" fg="$2" command_file="$3" config_file="$4"
    cat > "${config_file}" <<EOF
font-family = "JetBrainsMono Nerd Font Mono"
font-family-bold = "JetBrainsMono Nerd Font Mono"
font-size = ${FONT_SIZE}
font-thicken = false
background = #${bg}
foreground = #${fg}
cursor-style = block
cursor-style-blink = false
mouse-hide-while-typing = true
window-decoration = none
macos-window-shadow = false
window-padding-x = 0
window-padding-y = 0
window-padding-balance = false
window-padding-color = background
window-width = ${COLUMNS}
window-height = ${LINES}
window-position-x = ${WINDOW_X}
window-position-y = ${WINDOW_Y}
window-save-state = never
window-step-resize = true
resize-overlay = never
confirm-close-surface = false
shell-integration = none
command = $(ghostty_string "${command_file}")
EOF
}

screen_scale() {
    swift "${SCREEN_SCALE_SCRIPT}"
}

window_bounds() {
    local min_width="$1" min_height="$2"
    local bounds=""
    local i

    for (( i = 0; i < 60; i++ )); do
        if bounds="$(swift "${WINDOW_BOUNDS_SCRIPT}" "${WINDOW_X}" "${min_width}" "${min_height}")" && [[ -n "${bounds}" ]]; then
            printf '%s\n' "${bounds}"
            return 0
        fi
        sleep 0.05
    done

    return 1
}

wait_capture_window_gone() {
    local i

    for (( i = 0; i < 80; i++ )); do
        if ! swift "${WINDOW_BOUNDS_SCRIPT}" "${WINDOW_X}" 900 80 >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.05
    done

    return 1
}

window_crop_geometry() {
    local bounds="$1" scale="$2"
    local x y width height
    IFS=, read -r x y width height <<< "${bounds}"
    awk \
        -v scale="${scale}" \
        -v x="${x}" \
        -v y="${y}" \
        -v width="${width}" \
        -v height="${height}" \
        'BEGIN {
            printf "%dx%d+%d+%d",
                int(width * scale + 0.5),
                int(height * scale + 0.5),
                int(x * scale + 0.5),
                int(y * scale + 0.5)
        }'
}

capture_theme() {
    local theme="$1"
    local command_file="${TMP_DIR}/${theme}.sh"
    local config_file="${TMP_DIR}/${theme}.ghostty"
    local screen_file="${TMP_DIR}/${theme}-screen.png"
    local raw_file="${TMP_DIR}/${theme}-raw.png"
    local out_file="${OUT_DIR}/${theme}-terminal.png"
    local bg fg bounds scale geometry

    bg="$(palette_value "${theme}" bg)"
    fg="$(palette_value "${theme}" icon_text)"
    write_command_script "${theme}" "${command_file}"
    write_ghostty_config "${bg}" "${fg}" "${command_file}" "${config_file}"
    "${GHOSTTY_BIN}" +validate-config --config-file="${config_file}"

    wait_capture_window_gone >/dev/null 2>&1 || true
    active_config="${config_file}"
    open -na "${GHOSTTY_APP}" --args --config-file="${config_file}"
    sleep "${WARMUP_SECONDS}"

    if ! bounds="$(window_bounds 900 80)"; then
        printf 'showy-bar: Ghostty capture window did not appear for %s\n' "${theme}" >&2
        return 1
    fi

    scale="$(screen_scale)"
    geometry="$(window_crop_geometry "${bounds}" "${scale}")"
    screencapture -x "${screen_file}"
    magick "${screen_file}" -crop "${geometry}" +repage "${raw_file}"
    magick "${raw_file}" -crop "${OUTPUT_W}x${OUTPUT_H}+${OUTPUT_X}+${OUTPUT_Y}" +repage -strip "${out_file}"
    chmod 0644 "${out_file}"
    printf 'captured %s\n' "${out_file}"

    pkill -f "${active_config}" >/dev/null 2>&1 || true
    wait_capture_window_gone >/dev/null 2>&1 || true
    active_config=""
}

main() {
    local theme
    if (( $# > 0 )); then
        for theme in "$@"; do
            [[ -n "${theme}" ]] || continue
            capture_theme "${theme}"
        done
        return 0
    fi

    while IFS= read -r theme; do
        [[ -n "${theme}" ]] || continue
        capture_theme "${theme}"
    done < <("${REPO_ROOT}/bin/showy-bar" --list)
}

main "$@"
