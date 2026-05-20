# Architecture

`showy-bar` is intentionally tiny. There are three layers:

1. **Data plane.** A single shell script, `bin/showy-bar-fetch`, owns the
   on-disk cache and is the only renderer path that talks to CodexBar. By
   default it probes `codexbar serve` at localhost `/usage`, then falls back to
   invoking `codexbar usage --format json` directly.
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
                │ codexbar usage --format json │  or  │ codexbar serve /usage │
                └──────────────────────────┬─────────────────────────────────┘
                                           │ (slow CLI cold; cheap server hit)
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
                         provider icon PNGs in image cache
```

## Cache contract

- File: `${SHOWY_BAR_CACHE_DIR}/usage.json` (default: `${XDG_CACHE_HOME:-$HOME/.cache}/showy-bar/usage.json`)
- Stamp file: `${SHOWY_BAR_CACHE_DIR}/usage.json.updated-at`
- `flock` path: `${SHOWY_BAR_CACHE_DIR}/usage.lock`
- owner-scoped `mkdir` fallback path: `${SHOWY_BAR_CACHE_DIR}/usage.lock.d`
- Validation: `jq` must accept an array of provider objects. If a usage
  window is present, its `usedPercent` must be numeric before publishing.

The fetcher prints the cache content to stdout regardless of whether it
just refreshed or served stale bytes. Callers must not differentiate; if
they want freshness data they read `--age`.

Freshness is a shared render concern, not a per-provider state. A cache is
stale exactly when `showy_bar_age_seconds "${SHOWY_BAR_USAGE_FILE}"` is greater
than `SHOWY_BAR_REFRESH_SECONDS * 2`. Zellij, tmux, SketchyBar, and
`showy-bar-state` all consume that same rule: renderers show one trailing stale
indicator and grey frozen data, while the state surface reports the boolean and
threshold.

Refreshes try `${SHOWY_BAR_CODEXBAR_SERVE_URL%/}/usage` first with `curl`; the
default base URL is `http://127.0.0.1:8080`. Set
`SHOWY_BAR_CODEXBAR_SERVE_URL=` to skip the HTTP probe. Connection failures,
non-local URLs, missing `curl`, non-array HTTP payloads, or arrays with no
renderable usage providers fall back to the CLI path. The same on-disk
validation and last-known-good semantics still gate publication.

The tmux and Zellij detail panes source showy-bar config when present, then
run `${SHOWY_BAR_CODEXBAR_BIN:-codexbar} usage` directly because they display
CodexBar's text UI, not the compact cache-backed renderer output.

## Failure semantics

| Condition                                     | Outcome                                              |
|-----------------------------------------------|------------------------------------------------------|
| `codexbar` not on PATH and no serve URL       | First run errors; subsequent runs serve stale cache  |
| `codexbar serve` unavailable or not renderable | Fetcher falls back to CLI, then existing stale cache |
| CodexBar CLI returns non-JSON               | Cache is **not** updated; previous value still served|
| CodexBar JSON fails `jq` validation           | Same — preserve last good cache                      |
| Cache file missing and every refresh path fails | Fetcher exits non-zero; renderers print `AI ?`     |
| Cache age > `2 × SHOWY_BAR_REFRESH_SECONDS`     | All render surfaces show one trailing `⚠`, grey frozen quota data, and hide elapsed markers |

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

Provider render order is deterministic. `SHOWY_BAR_PROVIDERS`, when set, is
both an allow-list and the render order. Otherwise `SHOWY_BAR_PROVIDER_ORDER`
ranks providers without filtering them; missing providers are skipped, and
unlisted providers render after the ranked providers sorted by id. The default
rank is `codex,claude,opencode,gemini`.

## Adding a new SketchyBar provider

CodexBar discovers providers; this repo discovers them via the cache
content. So: enable the provider in CodexBar and wait for the next refresh
cycle. Zellij and tmux will render the new provider automatically.
SketchyBar declares/removes provider items on the next plugin tick after the
filtered provider set changes; no reload is required after the initial install.

## External layout managers

`bin/showy-bar-state` is the public bridge for configs that need CodexBar's
filtered provider count without duplicating CodexBar or renderer internals.
It honors `SHOWY_BAR_PROVIDERS` / `SHOWY_BAR_PROVIDERS_EXCLUDE` and emits:

- `available`: whether a valid cache was read.
- `stale`: whether the cache age exceeds `SHOWY_BAR_REFRESH_SECONDS * 2`.
- `cacheAgeSeconds`: seconds since the usage cache mtime, or `null` when absent.
- `staleAfterSeconds`: the numeric stale threshold.
- `providers`: filtered provider ids in render order.
- `providerCount`: `providers | length`.
- `sketchybar.compactRecommended`: `providerCount >=
  SHOWY_BAR_SKETCHYBAR_COMPACT_PROVIDER_COUNT`.

Consumers should treat `available=false` as "leave the current layout alone";
it means no last-known-good cache exists yet.

The SketchyBar plugin triggers `showy_bar_provider_change` when that filtered
provider set changes, so configs can subscribe without polling if they want
immediate layout reconciliation.
