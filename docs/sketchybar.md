# SketchyBar integration

## What gets added

Per provider in the filtered render set (`codexbar usage --format json`
after `SHOWY_BAR_PROVIDERS` / `SHOWY_BAR_PROVIDERS_EXCLUDE` are applied):

- `showy_bar.<provider>.icon`  — provider PNG (rendered from CodexBar's SVG)
- `showy_bar.<provider>.bar`   — multi-segment usage bar PNG
- `showy_bar.<provider>.label` — countdown label

Plus:

- `showy_bar.trigger`     — invisible item that runs the plugin every
  `SHOWY_BAR_SKETCHYBAR_UPDATE_FREQ` seconds.
- `showy_bar_bracket`     — pill background grouping the triple.

Provider adds/removals reconcile against that filtered set on the next plugin
tick; no `sketchybar --reload` is required after the initial install.


## Layout state

`bin/showy-bar-state` exposes the filtered provider list for external
SketchyBar layout managers. It does not move SketchyBar items itself; it only
reports CodexBar state:

```json
{
  "available": true,
  "providers": ["codex", "claude"],
  "providerCount": 2,
  "sketchybar": {
    "itemPrefix": "showy_bar",
    "bracket": "showy_bar_bracket",
    "compactProviderThreshold": 5,
    "compactRecommended": false
  }
}
```

Use this when your own SketchyBar config needs to compact, hide, or move
unrelated items around a wide CodexBar provider set. `showy-bar` does not
own cross-item layout policy.

When the filtered provider set changes, the SketchyBar plugin also triggers
`showy_bar_provider_change` with `SHOWY_BAR_PROVIDER_COUNT` and
`SHOWY_BAR_PROVIDERS` environment values. Configs that do not add/subscribe to
that event are unaffected.

## Pill geometry

The bracket reads `SHOWY_BAR_SKETCHYBAR_PILL_RADIUS`,
`SHOWY_BAR_SKETCHYBAR_PILL_HEIGHT`, and `SHOWY_BAR_SKETCHYBAR_PILL_COLOR`.
Defaults are `14`, `28`, and `0xcc24273a`.

For compatibility with existing sketchybarrc setups, the bootstrap item also
forwards `PILL_RADIUS` / `PILL_HEIGHT` into those envs when the explicit
`SHOWY_BAR_SKETCHYBAR_PILL_*` knobs are unset.

## Click action

Clicking the usage bar, label, or a non-degraded provider icon runs
`SHOWY_BAR_SKETCHYBAR_CLICK` (default: `open -b com.steipete.codexbar`),
which brings the CodexBar app forward. When a provider status is degraded
(`minor`, `maintenance`, `major`, or `critical`) and CodexBar supplies an
HTTP(S) status URL, clicking that provider's icon opens the status page
instead.

## Provider filters

`SHOWY_BAR_PROVIDERS` is an allow-list. `SHOWY_BAR_PROVIDERS_EXCLUDE` removes
providers from that result afterward, so the exclude list wins on overlap.

Examples:

- empty / empty → every provider CodexBar currently reports
- include only → only those providers
- exclude only → everything except those providers
- include + exclude → the include set minus the exclude set

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

Set `SHOWY_BAR_PALETTE_PRIMARY_*` in `~/.config/showy-bar/config.env` for
the minimal palette surface. Secondary and tertiary rows auto-derive from the
primary palette at `0.55` by default, so the 7d/monthly rows keep the original
dimmed ai-quota look unless you override `SHOWY_BAR_PALETTE_SECONDARY_*` or
`SHOWY_BAR_PALETTE_TERTIARY_*` directly. `SHOWY_BAR_PALETTE_TRACK`,
`SHOWY_BAR_PALETTE_ICON_TEXT`, `SHOWY_BAR_PALETTE_COUNTDOWN`,
`SHOWY_BAR_PALETTE_COUNTDOWN_WARN`, and `SHOWY_BAR_PALETTE_ELAPSED`
stay global across rows. Countdown labels use `SHOWY_BAR_PALETTE_COUNTDOWN`
unless the reset time is inside `SHOWY_BAR_TIME_WARN_MINUTES`, then they use
`SHOWY_BAR_PALETTE_COUNTDOWN_WARN`.

Use `showy-bar` to browse named palettes and persist `SHOWY_BAR_THEME`
without hand-editing the config file.

## Cache

PNGs go to `${SHOWY_BAR_SKETCHYBAR_IMAGE_CACHE}` (default
`~/.cache/showy-bar/sketchybar`). They are byte-compared on each
refresh; only changed images are written.

## Caveats

- The plugin does not dim or annotate when the cache is stale. Zellij and
  tmux do; SketchyBar relies on CodexBar's own menu for incident hints.
