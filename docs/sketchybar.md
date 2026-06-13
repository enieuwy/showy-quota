# SketchyBar integration

## What gets added

Per provider in the filtered render set (CodexBar usage JSON from managed
localhost `codexbar serve` or visibly degraded CLI fallback, after
`SHOWY_QUOTA_PROVIDERS` / `SHOWY_QUOTA_PROVIDERS_EXCLUDE` are applied):

- `showy_quota.<provider>.icon` — provider icon (`sketchybar-app-font` when
  mapped, CodexBar SVG/PNG fallback otherwise)
- `showy_quota.<provider>.primary` / `.secondary` / `.tertiary` — native slider
  usage rows
- `showy_quota.<provider>.secondary_marker` / `.tertiary_marker` — pacing
  markers
- `showy_quota.<provider>.slot` — transparent click/spacing item
- `showy_quota.<provider>.label` — countdown label

Plus:

- `showy_quota.trigger`     — invisible item that runs the plugin every
  `SHOWY_QUOTA_SKETCHYBAR_UPDATE_FREQ` seconds (default `10`, matching the
  default Zellij pipe interval).
- `showy_quota_bracket`     — pill background grouping the provider items.
- `showy_quota.degraded`  — trailing `⚠cli` marker when the cache came from CLI fallback.

Provider adds/removals reconcile against that filtered set on the next plugin
tick; no `sketchybar --reload` is required after the initial install.

Provider order is stable across additions/removals. Set
`SHOWY_QUOTA_PROVIDER_ORDER` to rank providers without filtering them; missing
providers are skipped. Set `SHOWY_QUOTA_PROVIDERS` when you want an ordered
allow-list instead.


## Layout state

`bin/showy-quota-state` exposes the filtered provider list for external
SketchyBar layout managers. It does not move SketchyBar items itself; it only
reports CodexBar state:

```json
{
  "available": true,
  "cache": { "source": "serve", "degraded": false },
  "providers": ["codex", "claude"],
  "providerCount": 2,
  "sketchybar": {
    "itemPrefix": "showy_quota",
    "bracket": "showy_quota_bracket",
    "compactProviderThreshold": 5,
    "compactRecommended": false
  }
}
```

Use this when your own SketchyBar config needs to compact, hide, or move
unrelated items around a wide CodexBar provider set. `showy-quota` does not
own cross-item layout policy.

When the filtered provider set changes, the SketchyBar plugin also triggers
`showy_quota_provider_change` with `SHOWY_QUOTA_PROVIDER_COUNT` and
`SHOWY_QUOTA_PROVIDERS` environment values. Configs that do not add/subscribe to
that event are unaffected.

## Pill geometry

The bracket reads `SHOWY_QUOTA_SKETCHYBAR_PILL_RADIUS`,
`SHOWY_QUOTA_SKETCHYBAR_PILL_HEIGHT`, and `SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR`.
Defaults are `14`, `28`, and `0xcc24273a`.

For compatibility with existing sketchybarrc setups, the bootstrap item also
forwards `PILL_RADIUS` / `PILL_HEIGHT` into those envs when the explicit
`SHOWY_QUOTA_SKETCHYBAR_PILL_*` knobs are unset.

## Countdown label

Each provider's countdown label is pinned to a fixed width
(`SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH`, default `32`) so the pill does not
jitter as the remaining-time string changes length (`59m` → `1:00` →
`23:59` → `idle`). The default fits the widest countdown form (`HH:MM`); set
it to `dynamic` to restore auto-sizing.

## Click action

Clicking the usage rows, label, or a non-degraded provider icon runs
`SHOWY_QUOTA_SKETCHYBAR_CLICK` (default: `open -b com.steipete.codexbar`),
which brings the CodexBar app forward. When a provider status is degraded
(`minor`, `maintenance`, `major`, or `critical`) and CodexBar supplies an
HTTP(S) status URL, clicking that provider's icon opens the status page
instead.

## Provider filters

`SHOWY_QUOTA_PROVIDERS` is an ordered allow-list. `SHOWY_QUOTA_PROVIDERS_EXCLUDE`
removes providers from that result afterward, so the exclude list wins on
overlap. When `SHOWY_QUOTA_PROVIDERS` is empty, `SHOWY_QUOTA_PROVIDER_ORDER` ranks
the providers CodexBar currently reports without filtering them.

Examples:

- no filters → every provider CodexBar currently reports, ranked by `SHOWY_QUOTA_PROVIDER_ORDER`
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

Rows are native SketchyBar sliders using `SHOWY_QUOTA_PNG_BAR_W` for width.
Tertiary is hidden when the provider does not expose that window.

## Customizing colors

Set `SHOWY_QUOTA_PALETTE_PRIMARY_*` in `~/.config/showy-quota/config.env` for
the minimal palette surface. Secondary and tertiary rows auto-derive from the
primary palette at `0.55` by default, so the 7d/monthly rows keep the original
dimmed ai-quota look unless you override `SHOWY_QUOTA_PALETTE_SECONDARY_*` or
`SHOWY_QUOTA_PALETTE_TERTIARY_*` directly. `SHOWY_QUOTA_PALETTE_TRACK`,
`SHOWY_QUOTA_PALETTE_ICON_TEXT`, `SHOWY_QUOTA_PALETTE_COUNTDOWN`,
`SHOWY_QUOTA_PALETTE_COUNTDOWN_WARN`, `SHOWY_QUOTA_PALETTE_STALE`, and
`SHOWY_QUOTA_PALETTE_ELAPSED` stay global across rows. Countdown labels use
`SHOWY_QUOTA_PALETTE_COUNTDOWN` unless the reset time is inside
`SHOWY_QUOTA_TIME_WARN_MINUTES`, then they use `SHOWY_QUOTA_PALETTE_COUNTDOWN_WARN`.

Use `showy-quota` to browse named palettes and persist `SHOWY_QUOTA_THEME`
without hand-editing the config file.

## Stale and degraded snapshots

When `${SHOWY_QUOTA_USAGE_FILE}` is older than
`2 × SHOWY_QUOTA_REFRESH_SECONDS`, the plugin turns on the trailing
`showy_quota.stale` item inside `showy_quota_bracket`. The item renders
`SHOWY_QUOTA_STALE_GLYPH` (default `⚠`) in `SHOWY_QUOTA_PALETTE_COUNTDOWN_WARN`.
Provider sliders and countdown labels switch to `SHOWY_QUOTA_PALETTE_STALE`;
provider icons keep their normal status tint, and elapsed marker overlays are
hidden so stale reset timing is not presented as live.

When the shared cache was refreshed from CLI fallback instead of
`codexbar serve`, `showy_quota.degraded` renders `⚠cli` in the same warning
color. Serve recovery clears the marker on the next successful fetch.

## Cache

Only SVG fallback icons are PNG-cached in `${SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE}`
(default `~/.cache/showy-quota/sketchybar`). Native bars and mapped font icons
are not rasterized.
