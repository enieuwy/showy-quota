# SketchyBar integration

## What gets added

Per provider in the filtered render set (`codexbar usage --format json`
after `SHOWY_BAR_PROVIDERS` / `SHOWY_BAR_PROVIDERS_EXCLUDE` are applied):

- `showy_bar.<provider>.icon` — provider icon (`sketchybar-app-font` when
  mapped, CodexBar SVG/PNG fallback otherwise)
- `showy_bar.<provider>.primary` / `.secondary` / `.tertiary` — native slider
  usage rows
- `showy_bar.<provider>.secondary_marker` / `.tertiary_marker` — pacing
  markers
- `showy_bar.<provider>.slot` — transparent click/spacing item
- `showy_bar.<provider>.label` — countdown label

Plus:

- `showy_bar.trigger`     — invisible item that runs the plugin every
  `SHOWY_BAR_SKETCHYBAR_UPDATE_FREQ` seconds.
- `showy_bar_bracket`     — pill background grouping the provider items.

Provider adds/removals reconcile against that filtered set on the next plugin
tick; no `sketchybar --reload` is required after the initial install.

Provider order is stable across additions/removals. Set
`SHOWY_BAR_PROVIDER_ORDER` to rank providers without filtering them; missing
providers are skipped. Set `SHOWY_BAR_PROVIDERS` when you want an ordered
allow-list instead.


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

Clicking the usage rows, label, or a non-degraded provider icon runs
`SHOWY_BAR_SKETCHYBAR_CLICK` (default: `open -b com.steipete.codexbar`),
which brings the CodexBar app forward. When a provider status is degraded
(`minor`, `maintenance`, `major`, or `critical`) and CodexBar supplies an
HTTP(S) status URL, clicking that provider's icon opens the status page
instead.

## Provider filters

`SHOWY_BAR_PROVIDERS` is an ordered allow-list. `SHOWY_BAR_PROVIDERS_EXCLUDE`
removes providers from that result afterward, so the exclude list wins on
overlap. When `SHOWY_BAR_PROVIDERS` is empty, `SHOWY_BAR_PROVIDER_ORDER` ranks
the providers CodexBar currently reports without filtering them.

Examples:

- no filters → every provider CodexBar currently reports, ranked by `SHOWY_BAR_PROVIDER_ORDER`
- include only → only those providers, in include-list order
- exclude only → everything except those providers, still ranked by provider order
- include + exclude → the include set minus the exclude set

## Native bar layout

```
+-------------------------------- 80 px ---------------------------+
|                          primary (5h)                            |   ← row 1
+------------------------------------------------------------------+
|                          secondary (7d)                          |   ← row 2
+------------------------------------------------------------------+
|                          tertiary (varies)                       |   ← row 3 (only when present)
+------------------------------------------------------------------+
```

Rows are native SketchyBar sliders using `SHOWY_BAR_PNG_BAR_W` for width.
Tertiary is hidden when the provider does not expose that window.

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

Only SVG fallback icons are PNG-cached in `${SHOWY_BAR_SKETCHYBAR_IMAGE_CACHE}`
(default `~/.cache/showy-bar/sketchybar`). Native bars and mapped font icons
are not rasterized.

## Caveats

- The plugin does not dim when the cache is stale. Zellij and tmux do.
