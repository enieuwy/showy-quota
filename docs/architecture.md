# Architecture

`codexbar-bars` is intentionally tiny. There are three layers:

1. **Data plane.** A single shell script, `bin/cb-bars-fetch`, that owns the
   on-disk cache and is the only place that ever invokes `codexbar`.
2. **Renderers.** Three independent scripts that read the cache and emit
   format-specific output:
   - `bin/cb-bars-zellij-bar` → ANSI strip
   - `bin/cb-bars-tmux-bar` → tmux markup
   - `sketchybar/plugins/cb_bars.sh` → SketchyBar item updates + PNGs
3. **Presentation.** SketchyBar items, Zellij layout/keybind, tmux
   status-right snippet — declared once, never re-evaluated by this repo.

```
                ┌─────────────────────────────────────────────┐
                │   codexbar usage --format json --pretty     │
                └──────────────────────────┬──────────────────┘
                                           │ (slow; 1–10 s cold)
                                           ▼
                ┌─────────────────────────────────────────────┐
                │   bin/cb-bars-fetch (flock + atomic write)  │
                │   ~/.cache/codexbar-bars/usage.json         │
                └──────────────────────────┬──────────────────┘
                                           │ (cheap; pure JSON read)
                ┌──────────────────────────┼──────────────────┐
                ▼                          ▼                  ▼
   sketchybar/plugins/cb_bars.sh   cb-bars-zellij-bar   cb-bars-tmux-bar
        │                                │                  │
        ▼                                ▼                  ▼
   sketchybar --set …                 ANSI strip        tmux #[…] markup
   PNGs in image cache               (zjstatus pipe)   (status-right)
```

## Cache contract

- File: `${XDG_CACHE_HOME:-$HOME/.cache}/codexbar-bars/usage.json`
- Stamp file: same path with `.updated-at` suffix.
- Lock file: same path with `.lock` suffix; either an `flock`-held
  descriptor or a `mkdir`-based lease.
- Validation: `jq -e 'type == "array"'` before publishing.

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
| Cache age > `2 × CB_BARS_REFRESH_SECONDS`     | Renderers dim every provider in the strip            |

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
`cb_bars_provider_sigil` (`lib/strip.sh`). New CodexBar providers fall
back to the first two letters of the id.

## Adding a new SketchyBar provider

CodexBar discovers providers; this repo discovers them via the cache
content. So: enable the provider in CodexBar, wait for the next refresh
cycle, and a new icon/bar/label triple appears automatically. No code
change required.
