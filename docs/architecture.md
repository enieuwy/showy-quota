# Architecture

`showy-bar` is intentionally tiny. There are three layers:

1. **Data plane.** A single shell script, `bin/showy-bar-fetch`, that owns the
   on-disk cache and is the only place that ever invokes `codexbar`.
2. **Renderers / state surfaces.** Independent scripts that read the cache and
   emit format-specific output or stable integration state:
   - `bin/showy-bar-state` → provider/layout state JSON for external coordinators
   - `bin/showy-bar-zellij-bar` → ANSI strip
   - `bin/showy-bar-tmux-bar` → tmux markup
   - `sketchybar/plugins/showy_bar.sh` → SketchyBar item updates + PNGs
3. **Presentation.** SketchyBar items, Zellij layout/keybind, tmux
   status-right snippet, and optional external layout managers.

```
                ┌─────────────────────────────────────────────┐
                │       codexbar usage --format json        │
                └──────────────────────────┬──────────────────┘
                                           │ (slow; 1–10 s cold)
                                           ▼
                ┌─────────────────────────────────────────────┐
                │   bin/showy-bar-fetch (flock + atomic write)  │
                │   ~/.cache/showy-bar/usage.json         │
                └──────────────────────────┬──────────────────┘
                                           │ (cheap; pure JSON read)
                ┌──────────────┬───────────┴──────────┬──────────────────┐
                ▼              ▼                      ▼                  ▼
        showy-bar-state   sketchybar/plugins/   showy-bar-zellij-bar   showy-bar-tmux-bar
         state JSON          showy_bar.sh          ANSI strip        tmux #[…] markup
                               │                (zjstatus pipe)   (status-right)
                               ▼
                         sketchybar --set …
                         PNGs in image cache
```

## Cache contract

- File: `${XDG_CACHE_HOME:-$HOME/.cache}/showy-bar/usage.json`
- Stamp file: same path with `.updated-at` suffix.
- Lock file: same path with `.lock` suffix; either an `flock`-held
  descriptor or an owner-scoped `mkdir` lock.
- Validation: `jq` must accept an array of provider objects. If a usage
  window is present, its `usedPercent` must be numeric before publishing.

The fetcher prints the cache content to stdout regardless of whether it
just refreshed or served stale bytes. Callers must not differentiate; if
they want freshness data they read `--age`.

## Failure semantics

| Condition                                     | Outcome                                              |
|-----------------------------------------------|------------------------------------------------------|
| `codexbar` not on PATH                        | First run errors; subsequent runs serve stale cache  |
| `codexbar` returns non-JSON                   | Cache is **not** updated; previous value still served|
| `codexbar` JSON fails `jq` validation         | Same — preserve last good cache                      |
| Cache file missing **and** `codexbar` fails   | Fetcher exits non-zero; renderers print `AI ?`       |
| Cache age > `2 × SHOWY_BAR_REFRESH_SECONDS`     | Zellij + tmux dim every provider; SketchyBar unchanged |

## Why bash and not Python/Go/Rust

The ai-quota predecessor was Python with a daemon, sidecar, and
`--client-defaults` indirection. That stack made sense when ai-quota also
had to *talk to providers*. CodexBar removed that need. Bash + `jq` +
ImageMagick is enough to stitch JSON to bars and is the lowest possible
install footprint.

## Provider id mapping

CodexBar's JSON `provider` field is the canonical id and matches the
filename of its bundled SVG (`ProviderIcon-<id>.svg`). The SketchyBar
plugin uses these one-to-one — no remapping table.

The Zellij and tmux strips render a 2-letter sigil per provider via
`showy_bar_provider_sigil` (`lib/strip.sh`). New CodexBar providers fall
back to the first two letters of the id.

## Adding a new SketchyBar provider

CodexBar discovers providers; this repo discovers them via the cache
content. So: enable the provider in CodexBar and wait for the next refresh
cycle. Zellij and tmux will render the new provider automatically. For
SketchyBar, reload the bar after the new provider appears in the cache so
the icon/bar/label item triple is declared. No code change required.

## External layout managers

`bin/showy-bar-state` is the public bridge for configs that need CodexBar's
filtered provider count without duplicating CodexBar or renderer internals.
It honors `SHOWY_BAR_PROVIDERS` / `SHOWY_BAR_PROVIDERS_EXCLUDE` and emits:

- `available`: whether a valid cache was read.
- `providers`: filtered provider ids in render order.
- `providerCount`: `providers | length`.
- `sketchybar.compactRecommended`: `providerCount >=
  SHOWY_BAR_SKETCHYBAR_COMPACT_PROVIDER_COUNT`.

Consumers should treat `available=false` as "leave the current layout alone";
it means no last-known-good cache exists yet.

The SketchyBar plugin triggers `showy_bar_provider_change` when that filtered
provider set changes, so configs can subscribe without polling if they want
immediate layout reconciliation.
