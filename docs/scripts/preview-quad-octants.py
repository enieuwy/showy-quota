#!/usr/bin/env python3
"""Preview a 4-lane "quad" octant bar for Antigravity's four windows.

Run this in the terminal you actually watch your bars in:

    python3 docs/scripts/preview-quad-octants.py

Four vertically-stacked lanes in one monospace cell require Unicode 16
OCTANT glyphs (U+1CD00..U+1CDE5, the 2x4 mosaics). These are drawn by
Ghostty, kitty, WezTerm and libvte terminals, but NOT by Alacritty 0.16
or iTerm2 today (their built-in legacy-computing fonts stop at the
Unicode 13 sextants). The SUPPORT CHECK section below is the real test:
if the octant ramp shows tofu/boxes while the sextant ramp shows proper
mosaics, this terminal cannot render the quad and you want the
two-dual-per-family fallback (also shown) instead.

Nothing here is wired into showy-quota; it is a standalone visual probe.
"""
from __future__ import annotations

import os

# ── repo default palette (matches lib/common.sh) ────────────────────────────
GOOD = (0x25, 0xBE, 0x6A)
WARN = (0xF0, 0xAF, 0x00)
BAD = (0xEE, 0x53, 0x96)
SURFACE = (0x2A, 0x2A, 0x2A)
ELAPSED = (0xBE, 0x95, 0xFF)       # pace marker for a live/short window (bright)
ELAPSED_LONG = (0x3D, 0xDB, 0xD9)  # pace marker for a weekly/monthly window (distinct hue)
DIM_SCALE = 0.55


def dim(rgb: tuple[int, int, int]) -> tuple[int, int, int]:
    return tuple(int(c * DIM_SCALE) for c in rgb)  # type: ignore[return-value]


def severity(remaining: int) -> tuple[int, int, int]:
    return GOOD if remaining >= 40 else WARN if remaining >= 15 else BAD


def horizon_color(remaining: int, is_long: bool) -> tuple[int, int, int]:
    """A window's color: severity palette, dimmed when it is a weekly/monthly cap."""
    color = severity(remaining)
    return dim(color) if is_long else color


def fg(rgb: tuple[int, int, int]) -> str:
    return f"\033[38;2;{rgb[0]};{rgb[1]};{rgb[2]}m"


def bg(rgb: tuple[int, int, int]) -> str:
    return f"\033[48;2;{rgb[0]};{rgb[1]};{rgb[2]}m"


RESET = "\033[0m"

# ── octant lane table ───────────────────────────────────────────────────────
# Index is a 4-bit lane mask: bit0 = top lane ... bit3 = bottom lane.
# Each entry is the glyph whose 2x4 mosaic fills exactly those full-width rows.
# Most are Unicode 16 octants; four are pre-existing quarter/half/full blocks.
LANE = [
    chr(c)
    for c in (
        0x0020,   # 0000  (empty)
        0x1FB82,  # 0001  top 1/4            UPPER ONE QUARTER BLOCK
        0x1CD06,  # 0010  row 2              BLOCK OCTANT-34
        0x2580,   # 0011  top half           UPPER HALF BLOCK
        0x1CD27,  # 0100  row 3              BLOCK OCTANT-56
        0x1CD2A,  # 0101  rows 1,3           BLOCK OCTANT-1256
        0x1CD33,  # 0110  rows 2,3           BLOCK OCTANT-3456
        0x1FB85,  # 0111  top 3/4            UPPER THREE QUARTERS BLOCK
        0x2582,   # 1000  bottom 1/4         LOWER ONE QUARTER BLOCK
        0x1CDAE,  # 1001  rows 1,4           BLOCK OCTANT-1278
        0x1CDB7,  # 1010  rows 2,4           BLOCK OCTANT-3478
        0x1CDBA,  # 1011  rows 1,2,4         BLOCK OCTANT-123478
        0x2584,   # 1100  bottom half        LOWER HALF BLOCK
        0x1CDDD,  # 1101  rows 1,3,4         BLOCK OCTANT-125678
        0x2586,   # 1110  bottom 3/4         LOWER THREE QUARTERS BLOCK
        0x2588,   # 1111  full               FULL BLOCK
    )
]


def filled(remaining: int, width: int) -> int:
    return round(max(0, min(100, remaining)) / 100 * width)


def render_mono4(
    lanes: list[dict],
    width: int = 24,
    markers: list[tuple[int, tuple[int, int, int]]] | None = None,
    color_mode: str = "lowest",
) -> str:
    """Four windows (top->bottom) packed into one octant row, mono3-style.

    One provider color for the fills (mono3's rule, extended to four lanes), but
    up to TWO pacing separators: a bright one for a live/short window and a
    second, distinct color for the secondary model's weekly (7d+) window. Later
    markers win on a column collision.
    """
    fills = [filled(l["rem"], width) for l in lanes]
    rem = lanes[0]["rem"] if color_mode == "primary" else min(l["rem"] for l in lanes)
    all_long = all(l["long"] for l in lanes)
    color = horizon_color(rem, all_long)
    marker_at = {col: mc for col, mc in (markers or [])}
    cells = []
    for col in range(width):
        if col in marker_at:
            cells.append(f"{fg(marker_at[col])}{bg(SURFACE)}\u2502{RESET}")
            continue
        mask = sum(1 << lane for lane in range(4) if col < fills[lane])
        cells.append(f"{fg(color)}{bg(SURFACE)}{LANE[mask]}{RESET}")
    return "".join(cells)


def render_dual(top: dict, bottom: dict, width: int = 12) -> str:
    """Two windows in one half-block row (universal glyphs, renders everywhere)."""
    tf, bf = filled(top["rem"], width), filled(bottom["rem"], width)
    tc = horizon_color(top["rem"], top["long"])
    bc = horizon_color(bottom["rem"], bottom["long"])
    cells = []
    for i in range(width):
        t = tc if i < tf else SURFACE
        b = bc if i < bf else SURFACE
        cells.append(f"{fg(t)}{bg(b)}\u2580{RESET}")
    return "".join(cells)


def ramp(start: int, count: int) -> str:
    return "".join(chr(start + i) for i in range(count))


def detect_terminal() -> str:
    if os.environ.get("GHOSTTY_RESOURCES_DIR"):
        return "Ghostty (octants supported)"
    if os.environ.get("KITTY_WINDOW_ID"):
        return "kitty (octants supported)"
    if os.environ.get("WEZTERM_PANE"):
        return "WezTerm (octants supported)"
    if os.environ.get("ALACRITTY_WINDOW_ID"):
        return "Alacritty (NO octant support as of 0.16 -- expect tofu)"
    prog = os.environ.get("TERM_PROGRAM", "")
    return prog or os.environ.get("TERM", "unknown")


def main() -> None:
    bar_w = 28
    # Antigravity's four windows, top -> bottom. Sample levels chosen to be
    # visibly distinct; Gemini weekly is shown exhausted (its real state today).
    lanes = [
        {"name": "Gemini Session  (5h)", "rem": 65, "long": False},
        {"name": "Gemini Weekly       ", "rem": 0, "long": True},
        {"name": "Claude+GPT Session  ", "rem": 90, "long": False},
        {"name": "Claude+GPT Weekly   ", "rem": 18, "long": True},
    ]

    print(f"\nterminal: {detect_terminal()}\n")

    print("── SUPPORT CHECK (do these glyphs render, or show tofu/boxes?) ──")
    print(f"  half-blocks (always work) : {ramp(0x2596, 10)}  \u2580\u2584\u2588")
    print(f"  sextants U+1FB00 (work in Alacritty) : {ramp(0x1FB00, 28)}")
    print(f"  octants  U+1CD00 (need Ghostty/kitty): {ramp(0x1CD00, 32)}")
    print(f"  quad lane glyphs (mask 0..15)        : {''.join(LANE)}")

    print("\n── mono4: Antigravity's 4 windows in ONE octant cell-row ──")
    print("   one provider fill color (mono3's rule) + TWO pacing markers:")
    # Illustrative pace columns: each model family's weekly reset position.
    mark_gemini = (round(bar_w * 0.30), ELAPSED)       # primary model pace (bright)
    mark_3p = (round(bar_w * 0.62), ELAPSED_LONG)      # secondary model weekly (distinct)
    markers = [mark_gemini, mark_3p]
    lowest = render_mono4(lanes, bar_w, markers, "lowest")
    primary = render_mono4(lanes, bar_w, markers, "primary")
    print(f"     color=lowest (most urgent):  AG\u2595{lowest}\u258f")
    print(f"     color=primary window:        AG\u2595{primary}\u258f")
    print(
        f"     markers:  {fg(ELAPSED)}\u2502{RESET} Gemini pace      "
        f"{fg(ELAPSED_LONG)}\u2502{RESET} Claude+GPT weekly pace"
    )
    print("   lanes top->bottom (fill levels distinct; fill color shared):")
    for lane in lanes:
        sw = horizon_color(lane["rem"], lane["long"])
        tag = "weekly cap" if lane["long"] else "live 5h "
        print(f"     {fg(sw)}\u2588\u2588{RESET} {lane['name']}  {lane['rem']:3d}%  {tag}")

    print("\n── FALLBACK (non-octant terminals): two dual sub-bars per family ──")
    gem = render_dual(lanes[0], lanes[1], 12)
    cgp = render_dual(lanes[2], lanes[3], 12)
    print(f"  AG \u1d33\u2595{gem}\u258f \u1d9c\u2595{cgp}\u258f   (each: 5h over weekly; per-window color + pacing)")

    print(
        "\nIf the octant rows are boxes/blank, this terminal can't draw mono4 --\n"
        "use mono3/dual or the two-dual fallback (or run these bars in Ghostty).\n"
    )


if __name__ == "__main__":
    main()
