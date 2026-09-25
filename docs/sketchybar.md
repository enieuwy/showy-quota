# SketchyBar integration

## What gets added

Per provider in the filtered render set (CodexBar usage JSON from managed
localhost `codexbar serve` or visibly degraded CLI fallback, after
`SHOWY_QUOTA_PROVIDERS` / `SHOWY_QUOTA_PROVIDERS_EXCLUDE` are applied):

- `showy_quota.<provider>.icon` — provider icon (`sketchybar-app-font` when
  mapped, CodexBar SVG/PNG fallback otherwise)
- `showy_quota.<provider>.primary` / `.secondary` / `.tertiary` / `.quaternary` —
  native slider usage rows (2–4, adaptive)
- `showy_quota.<provider>.primary_marker` / `.secondary_marker` / `.tertiary_marker` / `.quaternary_marker`
  — per-window pacing markers (every present window is paced, except pools
  sharing one billing cycle, which show only the primary marker)
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

All per-tick compute happens in the native renderer. Each tick the plugin
runs `showy-quota-render --emit sketchybar-frame --from-cache` once. The
renderer reads the cache, the live item list, and the notch plan, and prints
the final `sketchybar --set` arguments: remaining percentages, elapsed
markers, countdown labels, colors, icons, click scripts, and stale and
shared-cycle handling. It also says when the items must be declared again. The
plugin runs `showy-quota-fetch` only when the cache is missing or unusable
(synchronously) or older than the refresh interval (in the background). The
shell plugin declares the items, rasterizes provider icons, and runs
`sketchybar`. The render binary ships with `make install-bin` / release
tarballs; without it the plugin clears its items and logs a hint. Rebuild it
(`make render-bin`) together with the plugin: the plugin needs the
`sketchybar-*` emit modes.

## Tick cost

The renderer compares each provider's arguments with the ones the plugin sent
last (`${SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE}/frame.txt`) and emits only the
providers that changed, which the plugin sends in one `sketchybar` call. A
tick where nothing changed sends nothing and skips the notch re-plan.
Countdown labels change at most once a minute, so on the default 10 s timer
most ticks send nothing.

- The comparison covers the final arguments, so a changed setting (palette,
  glyphs, widths, icon mode, click action) re-sends every provider.
- One `sketchybar --query bar` per tick checks that the items still exist and
  still follow `showy_quota.trigger`. Missing or misplaced items, or a changed
  provider set, rebuild every item in one `sketchybar` call.
- If another script changes these items, the plugin restores them the next
  time their arguments change, or at once after `sketchybar --reload`.


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

Only those three envs are read. Earlier versions also forwarded bare
`PILL_RADIUS` / `PILL_HEIGHT` from `sketchybarrc`; that forwarding is gone,
because the unprefixed names collide with other SketchyBar components and
produced silent layout changes. Rename any such setting to the
`SHOWY_QUOTA_SKETCHYBAR_PILL_*` form.

## Countdown label

Each provider's countdown label is pinned to a fixed width
(`SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH`, default `32`) so the pill does not
jitter as the remaining-time string changes length (`59m` → `1:00` →
`23:59` → `idle`). The default fits the widest countdown form (`HH:MM`); set
it to `dynamic` to restore auto-sizing.

## Notch placement

Set `SHOWY_QUOTA_SKETCHYBAR_PLACEMENT=notch` (default `left`) and reload
SketchyBar. Providers keep their order left to right; any provider that would
run under the notch moves right of it (`position=e`). The one bracket covers
both sides, so on the notched display the pill appears to pass behind the
notch.

Whenever a tick sends arguments, the plugin measures the bar in one batched
`sketchybar --query` and the renderer plans the split
(`showy-quota-render --emit sketchybar-layout`):

1. The notch gap comes from two invisible 1pt anchors,
   `showy_quota.notch_q` and `showy_quota.notch_e`, so it is exactly the gap
   SketchyBar reserves (`notch_width`). No notch-size table is involved.
2. The left side ends at the notch (or at an earlier `q`/`center` item). The
   right side ends at the first `right`/`center` item, less the width of any
   of your own `e` items, which share that flow.
3. The planner keeps as many providers on the left as fit, and moves the rest
   right. If the right side is still short of room, countdown labels hide on
   every provider. If that is not enough, the providers that fit nowhere
   collapse into a `+N` item (`showy_quota.overflow`). No item is placed under
   the notch.

Positions change with `--set position=`, so a re-split never tears the pill
down. The split is also re-planned on `front_app_switched`, `display_change`,
`system_woke`, and `showy_quota_layout`. Apart from `system_woke`, which
renders too, these events only re-plan: they do not read the cache or send
rows. An event that finds a render in flight is not dropped: it leaves a note,
and that render or the next tick re-plans. Trigger `showy_quota_layout` from
your own config after you show or hide an item beside the pill:

```bash
sketchybar --trigger showy_quota_layout
```

Limits:

- SketchyBar reserves `notch_width` (a bar setting, default `200`), not the
  measured notch. If you set it narrower than the real notch, items can sit
  under it.
- The notch gap exists only on the built-in display. On a display without a
  notch, nothing moves. When the bar shows on several displays, the plan
  follows the display with the widest notch gap, so an external display shows
  the split pill with an empty middle.

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
|                          row 1                                   |   ← primary
+------------------------------------------------------------------+
|                          row 2                                   |   ← secondary
+------------------------------------------------------------------+
|                          row 3                                   |   ← tertiary (when present)
+------------------------------------------------------------------+
|                          row 4                                   |   ← quaternary (model pools)
+------------------------------------------------------------------+
```

Rows are native SketchyBar sliders using `SHOWY_QUOTA_PNG_BAR_W` for width, and
the stack adapts from 2 to 4 rows. A time-tiered provider shows its
primary/secondary/tertiary windows (5h, weekly, …), tertiary hidden when absent.
A model-pooled provider whose `extraRateWindows` carry every positional slot
(e.g. Antigravity) shows its pool windows instead, family-grouped — Antigravity
renders four: Gemini 5h/weekly then Claude+GPT 5h/weekly.
If CodexBar transiently marks one family's windows `usageKnown:false`
(placeholder, not a real measurement — e.g. the Claude/GPT pool during a
collection hiccup), those lanes stay drawn as empty tracks with no pacing
marker rather than collapsing, so a momentarily-thin family does not vanish
from the stack (parity with the Zellij `AGᶜ` lane).

A provider CodexBar cannot read at all (an expired login, a network failure)
keeps its place in the bar as an error label with no rows: the icon and the
label draw in the warning color, every slider stays off, and its click action
still reaches the provider's status page when CodexBar publishes one.
A provider whose own slice went stale renders grey with no pacing markers,
exactly like a wholly stale bar, while the providers beside it keep their own
colors.

## Customizing colors

Set `SHOWY_QUOTA_PALETTE_PRIMARY_*` in `~/.config/showy-quota/config.env` for
the minimal palette surface. Each usage row is colored by its remaining-quota
severity against the primary palette, then dimmed when its window is a
long-horizon cap — `windowMinutes` at or beyond `SHOWY_QUOTA_DIM_WINDOW_MINUTES`
(default `10080`, i.e. weekly/monthly). The dim color is the primary palette
scaled by `SHOWY_QUOTA_PALETTE_DIM_SCALE` (default `0.55`) unless you set an
explicit `SHOWY_QUOTA_PALETTE_DIM_*` override, so weekly/monthly rows keep the
dimmed ai-quota look while 5h/daily rows stay bright. Pools that share one
billing cycle (identical reset and `windowMinutes`, e.g. Cursor's Total/Auto/API)
are an exception: every row stays bright and only the primary pacing marker is
drawn, since the others would land on the same column. `SHOWY_QUOTA_PALETTE_TRACK`,
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

## Provider icons and `SHOWY_QUOTA_CODEXBAR_RESOURCES`

Provider icons are rasterized from `${SHOWY_QUOTA_CODEXBAR_RESOURCES}/ProviderIcon-<id>.svg`
(default the CodexBar app bundle's `Resources`). `rsvg-convert` (librsvg) is
preferred when installed, because ImageMagick's internal `MSVG:` decoder cannot
rasterize stroke-only paths (`fill="none"`) and silently produces a fully
transparent image for roughly a third of CodexBar's provider SVGs. Without
`rsvg-convert` those providers fall back to the drawn two-letter sigil icon
rather than rendering an invisible slot.

The drawn sigil is annotated with a concrete font file, because a Homebrew
ImageMagick has no fontconfig delegate and an empty `magick -list font`, which
makes a bare font name fail. Candidates are tried in order:
`${SHOWY_QUOTA_SKETCHYBAR_ICON_FONT_FILE}`, `/System/Library/Fonts/SFNS.ttf`,
`/System/Library/Fonts/Helvetica.ttc`,
`/System/Library/Fonts/Supplemental/Arial.ttf`. When none is readable the
fallback degrades to a plain disc so the slot stays visible and clickable.

Point `SHOWY_QUOTA_CODEXBAR_RESOURCES` only at a directory you trust: a
malicious SVG can otherwise instruct the renderer to fetch remote resources. As
defense in depth the plugin runs `magick` under a bundled restrictive policy
(`adapters/sketchybar/imagemagick/policy.xml`, injected via
`MAGICK_CONFIGURE_PATH`) that blocks the network coders and delegate execution,
so SVG `href` fetches (SSRF) are denied regardless of the system ImageMagick
policy. `rsvg-convert` is invoked directly rather than as an ImageMagick
delegate, so that ban still holds; librsvg refuses remote hrefs on its own.
