# SketchyBar integration

## What gets added

Per provider returned by `codexbar usage --format json`:

- `cb_bars.<provider>.icon`  — provider PNG (rendered from CodexBar's SVG)
- `cb_bars.<provider>.bar`   — multi-segment usage bar PNG
- `cb_bars.<provider>.label` — countdown label

Plus:

- `cb_bars.trigger`     — invisible item that runs the plugin every
  `CB_BARS_SKETCHYBAR_UPDATE_FREQ` seconds.
- `cb_bars_bracket`     — pill background grouping the triple.

## Pill geometry

The bracket reads `PILL_RADIUS` and `PILL_HEIGHT` from your existing
sketchybarrc env, falling back to 14/28. So `codexbar-bars` cohabits
visually with whatever other bracket pills you use.

## Click action

Clicking any of the three items runs `CB_BARS_SKETCHYBAR_CLICK` (default:
`open -b com.steipete.CodexBar`), which brings the CodexBar app forward.
CodexBar's own menu serves as the detail UI.

## PNG bar layout

```
+-------------------------------- 80 px ---------------------------+
|                          primary (5h)                            |   ← row 1
+------------------------------------------------------------------+
|                          secondary (7d)                          |   ← row 2
+------------------------------------------------------------------+
|                          tertiary (varies)                       |   ← row 3 (only when present)
+------------------------------------------------------------------+
```

When a provider has only primary + secondary (most common), the image is
18 px tall; with tertiary it's 22 px.

## Customizing colors

Set `CB_BARS_PALETTE_GOOD/WARN/BAD/UNKNOWN/TRACK/TEXT` in
`~/.config/codexbar-bars/config.env`. All values are 6-char hex (no `#`).

## Cache

PNGs go to `${CB_BARS_SKETCHYBAR_IMAGE_CACHE}` (default
`~/.cache/codexbar-bars/sketchybar`). They are byte-compared on each
refresh; only changed images are written.

## Caveats

- The plugin does not dim or annotate when the cache is stale. Zellij and
  tmux do; SketchyBar relies on CodexBar's own menu for incident hints.
- New providers added in CodexBar after sketchybarrc was sourced require
  `sketchybar --reload` to appear, because items are declared once when
  this file is sourced. The plugin's timer only updates already-declared
  items.
