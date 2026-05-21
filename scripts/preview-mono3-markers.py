#!/usr/bin/env python3
"""Side-by-side ANSI preview of four mono3 pacing-marker strategies.

Run from a terminal with a Nerd Font (or any font with Unicode sextant
glyphs) to compare how each strategy looks against representative
fill/marker scenarios.

Strategies:

  1. current (insert)      Existing showy-bar behavior: insert a `│`
                           character at a boundary in [0..WIDTH]. Body
                           width grows to WIDTH+1 when the marker is
                           interior.
  2. bg-overlay            At the marker cell, swap the cell background
                           from `surface` to `elapsed`. The sextant
                           glyph is unchanged. Width stays at WIDTH but
                           the marker is invisible when the cell is
                           fully filled (mask=7, `█`) because that cell
                           has no background sub-pixels.
  3. in-place │            Replace the sextant glyph at the marker cell
                           with `│`. Width stays at WIDTH. Loses the
                           glyph's encoded fill info at that cell; if
                           the cell happened to be a row-transition,
                           the readout drifts by one cell.
  4. hybrid                bg-overlay when mask < 7, in-place `│` when
                           mask == 7. Width stays at WIDTH, marker is
                           always visible, and no fill information is
                           lost (mask==7 cells are by monotonicity
                           beyond every row's transition).
"""

import sys

SEXTANT = {
    0: ' ',
    1: chr(0x1FB02),  # SEXTANT-12
    2: chr(0x1FB0B),  # SEXTANT-34
    3: chr(0x1FB0E),  # SEXTANT-1234
    4: chr(0x1FB2D),  # SEXTANT-56
    5: chr(0x1FB30),  # SEXTANT-1256
    6: chr(0x1FB39),  # SEXTANT-3456
    7: '\u2588',      # FULL BLOCK
}

# Default-palette swatches (share/themes/default.env).
MONO_GOOD = '25be6a'  # primary_good (green)
MONO_BAD  = 'ee5396'  # primary_bad (pink)
SURFACE   = '2a2a2a'  # cell background
ELAPSED   = 'be95ff'  # pacing marker color

RST = '\033[0m'
WIDTH = 12


def fg(h: str) -> str:
    r, g, b = int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)
    return f'\033[38;2;{r};{g};{b}m'


def bg(h: str) -> str:
    r, g, b = int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)
    return f'\033[48;2;{r};{g};{b}m'


def mask_for(i: int, pf: int, sf: int, tf: int) -> int:
    m = 0
    if i < pf:
        m |= 1
    if i < sf:
        m |= 2
    if i < tf:
        m |= 4
    return m


def v1_insert(pf, sf, tf, marker_boundary, mono):
    """Insert `│` at a boundary in [0..WIDTH]; body width = WIDTH+1 when interior."""
    out = []
    for i in range(WIDTH):
        if marker_boundary == i:
            out.append(fg(ELAPSED) + bg(SURFACE) + '│' + RST)
        m = mask_for(i, pf, sf, tf)
        ch = SEXTANT[m]
        c = SURFACE if m == 0 else mono
        out.append(fg(c) + bg(SURFACE) + ch + RST)
    if marker_boundary == WIDTH:
        out.append(fg(ELAPSED) + bg(SURFACE) + '│' + RST)
    return ''.join(out)


def v2_bg_overlay(pf, sf, tf, marker_cell, mono):
    """Swap cell background to `elapsed` at marker_cell; glyph unchanged."""
    out = []
    for i in range(WIDTH):
        m = mask_for(i, pf, sf, tf)
        ch = SEXTANT[m]
        c = SURFACE if m == 0 else mono
        b = ELAPSED if i == marker_cell else SURFACE
        out.append(fg(c) + bg(b) + ch + RST)
    return ''.join(out)


def v3_inplace(pf, sf, tf, marker_cell, mono):
    """Replace glyph with `│` at marker_cell."""
    out = []
    for i in range(WIDTH):
        if i == marker_cell:
            out.append(fg(ELAPSED) + bg(SURFACE) + '│' + RST)
        else:
            m = mask_for(i, pf, sf, tf)
            ch = SEXTANT[m]
            c = SURFACE if m == 0 else mono
            out.append(fg(c) + bg(SURFACE) + ch + RST)
    return ''.join(out)


def v4_hybrid(pf, sf, tf, marker_cell, mono):
    """bg-overlay when mask<7, in-place `│` when mask==7."""
    out = []
    for i in range(WIDTH):
        m = mask_for(i, pf, sf, tf)
        if i == marker_cell:
            if m == 7:
                out.append(fg(ELAPSED) + bg(SURFACE) + '│' + RST)
            else:
                ch = SEXTANT[m]
                c = SURFACE if m == 0 else mono
                out.append(fg(c) + bg(ELAPSED) + ch + RST)
        else:
            ch = SEXTANT[m]
            c = SURFACE if m == 0 else mono
            out.append(fg(c) + bg(SURFACE) + ch + RST)
    return ''.join(out)


# (label, p_fill, s_fill, t_fill, marker_cell, marker_boundary, mono_color)
SCENARIOS = [
    ('A. Fresh window, all rows ~100% (mask=7 everywhere); marker mid-bar',
     12, 12, 12, 6, 6, MONO_GOOD),
    ('B. Typical mid-life: p=8/12 s=10/12 t=4/12; marker at cell 5',
     8, 10, 4, 5, 5, MONO_GOOD),
    ('C. Near-empty primary (p=2/12), others healthy; marker at cell 1 (transition collision)',
     2, 10, 10, 1, 1, MONO_BAD),
    ('D. Marker at far-right (cell 11 / boundary 12)',
     8, 10, 4, 11, 12, MONO_GOOD),
    ('E. Marker at far-left (cell 0 / boundary 0)',
     8, 10, 4, 0, 0, MONO_GOOD),
]


def main() -> int:
    label_w = 24
    for (label, pf, sf, tf, mc, mb, mono) in SCENARIOS:
        print(label)
        print(f'  {"1. current (insert)":<{label_w}} → '
              + v1_insert(pf, sf, tf, mb, mono)
              + f'   width={WIDTH + 1}')
        print()
        print(f'  {"2. bg-overlay":<{label_w}} → '
              + v2_bg_overlay(pf, sf, tf, mc, mono)
              + f'   width={WIDTH}')
        print()
        print(f'  {"3. in-place │":<{label_w}} → '
              + v3_inplace(pf, sf, tf, mc, mono)
              + f'   width={WIDTH}')
        print()
        print(f'  {"4. hybrid":<{label_w}} → '
              + v4_hybrid(pf, sf, tf, mc, mono)
              + f'   width={WIDTH}')
        print()
        print()
    return 0


if __name__ == '__main__':
    sys.exit(main())
