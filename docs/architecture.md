# Architecture

`showy-quota` has two integration families:

1. **Shell integrations for host bars.** tmux, SketchyBar, and the advanced zjstatus path use the existing shell data plane and renderers.
2. **Standalone Zellij plugin.** Zellij's recommended path is a Rust/WASM plugin that fetches CodexBar serve directly and renders the same ANSI-styled terminal strip in-process.

```text
Recommended Zellij path:

codexbar serve /health + /usage
        ▲
        │ start if absent (Zellij background command pane)
        │
showy-quota-zellij.wasm
  ├─ WebAccess request to localhost /health and /usage
  ├─ OpenTerminalsOrPlugins startup of `codexbar serve`
  ├─ RunCommands degraded fallback to `codexbar usage --format json --pretty`
  ├─ in-memory last-known-good data
  └─ ANSI-styled terminal strip rendered directly in a one-line Zellij pane

Shell integrations:

codexbar serve /health + /usage  or  codexbar usage --format json
        ▲
        │ start if absent (managed pidfile)
        │
bin/showy-quota-fetch  ← shared cache + source marker + flock + atomic publish
        ├──► bin/showy-quota-state                 (stable provider/layout state JSON)
        ├──► sketchybar/plugins/showy_quota.sh    (native SketchyBar rows + icons)
        ├──► bin/showy-quota-tmux-bar             (tmux #[…] markup)
        └──► bin/showy-quota-zellij-bar           (advanced zjstatus ANSI segment)
```

## Shell cache contract

The shell data plane is still the reliability boundary for tmux, SketchyBar, and advanced zjstatus composition.

- File: `${SHOWY_QUOTA_CACHE_DIR}/usage.json` (default: `${XDG_CACHE_HOME:-$HOME/.cache}/showy-quota/usage.json`)
- Stamp file: `${SHOWY_QUOTA_CACHE_DIR}/usage.json.updated-at`
- Source file: `${SHOWY_QUOTA_CACHE_DIR}/source` (`serve`, `cli`, or absent/unknown); CLI source is visibly degraded as `⚠cli`.
- `flock` path: `${SHOWY_QUOTA_CACHE_DIR}/usage.lock`
- owner-scoped `mkdir` fallback path: `${SHOWY_QUOTA_CACHE_DIR}/usage.lock.d`
- Validation: `jq` must accept an array of provider objects. If a usage window is present, its `usedPercent` must be numeric before publication.

The fetcher prints the cache content to stdout regardless of whether it just refreshed or served stale bytes. Callers must not differentiate; if they want freshness data they read `--age`.

Freshness is a shared render concern. A shell cache is stale when `showy_quota_age_seconds "${SHOWY_QUOTA_USAGE_FILE}"` is greater than `SHOWY_QUOTA_REFRESH_SECONDS * 2`. Shell renderers show one trailing stale indicator, grey frozen data, and hide elapsed markers; `showy-quota-state` reports the boolean and threshold.

Refreshes prefer `${SHOWY_QUOTA_CODEXBAR_SERVE_URL%/}/usage` with `curl`; the default base URL is `http://127.0.0.1:8080`. Before falling back to CLI, `showy-quota-fetch` probes `/health` and, with `SHOWY_QUOTA_MANAGE_SERVE=1` (default), starts `codexbar serve` on the port implied by `SHOWY_QUOTA_CODEXBAR_SERVE_URL` (default `8080`) in the background with a pidfile. `SHOWY_QUOTA_CODEXBAR_SERVE_PORT` is now a compatibility override; when both are set and disagree, the fetcher logs a warning and prefers the URL port. Set `SHOWY_QUOTA_MANAGE_SERVE=0` to disable managed startup, or `SHOWY_QUOTA_CODEXBAR_SERVE_URL=` to skip HTTP entirely. When an existing cache is still fresh under `SHOWY_QUOTA_REFRESH_SECONDS`, the fetcher may still refresh from `codexbar serve` every `SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS` so bars repaint shortly after the server's own response cache changes. Failed fast HTTP probes keep serving the existing cache; they do not invoke the slower CLI fallback until the normal refresh interval expires.

The tmux and Zellij detail panes source showy-quota config when present, then run `${SHOWY_QUOTA_CODEXBAR_BIN:-codexbar} usage` directly because they display CodexBar's text UI, not the compact cache-backed renderer output.

## Standalone Zellij plugin contract

The plugin does not read the host cache and does not shell out to showy-quota scripts. Its default path is:

```text
web_request("/health") → start `codexbar serve` if needed → web_request("/usage") → parse → filter/order → render ANSI strip
```

It requests `WebAccess`, `OpenTerminalsOrPlugins`, and `RunCommands` up front. If serve cannot be reached, the managed serve command uses the port from `serve_url` unless `serve_port` is set explicitly. CLI fallback runs `codexbar usage --format json --pretty`, renders data with a trailing `⚠cli` marker, and continues probing serve so it can switch back automatically.

The plugin keeps last-known-good JSON in memory for the pane/session. If refreshes fail after a success, it continues rendering the previous data and marks it stale at `2 × interval_seconds`. That preserves the user-visible last-known-good behavior without requiring `FullHdAccess` or a disk cache.

The Rust renderer intentionally duplicates only the terminal strip logic. Golden tests compare its output to `bin/showy-quota-zellij-bar` over the fixture set so tmux/SketchyBar can stay shell-based without visual drift in Zellij.

## Terminal rendering modes

`SHOWY_QUOTA_TERMINAL_BAR_MODE` in shell, and `terminal_bar_mode` in plugin KDL, affect the Zellij/tmux bar body. The default `auto` renderer selects per provider from configuration: providers in `SHOWY_QUOTA_MONO3_PROVIDERS` / `mono3_providers` (default `gemini,antigravity`) render as `mono3` unless excluded; every other provider uses `dual` primary/secondary half-block geometry.

`mono3` reads the tertiary window, packs primary/secondary/tertiary into one U+1FBxx sextant row, and uses one provider-level foreground color. Because the terminal body can draw only one pacing separator, mono3 bases it on `SHOWY_QUOTA_MONO3_MARKER_SOURCE` / `mono3_marker_source` (`primary` by default; `secondary`, `tertiary`, `shared`, and `none` are supported). Stale snapshots hide pacing separators.

Forced `sextant3` keeps the older bottom-most-row cell-color policy and does not draw elapsed markers; forced `dual` and `mono3` apply those bodies to every provider.

## Failure semantics

| Condition | Shell integrations | Standalone Zellij plugin |
|---|---|---|
| `codexbar serve` unavailable | Fetcher starts serve, then falls back to visibly degraded CLI (`⚠cli`) if startup/probe fails | Plugin starts serve via background command pane, then uses visibly degraded CLI (`⚠cli`) if needed |
| CodexBar CLI returns non-JSON | Cache is not updated; previous value still served | Fallback output rejected; previous in-memory value remains |
| CodexBar JSON fails validation | Same — preserve last good cache | Same — preserve last in-memory value |
| No prior valid data | Renderers print `AI ?` or `AI idle` depending on path | Pane shows unavailable/invalid message or `AI idle` |
| Data older than stale threshold | One trailing `⚠`, grey frozen quota data, no elapsed markers | Same visual stale behavior from in-memory age |
| Zellij permission denied | Not applicable to shell/zjstatus feeder | Pane shows `showy-quota: permission denied` |

## Why bash and Rust, not Python/Go

The old `ai-quota` predecessor was Python with a daemon, sidecar, and `--client-defaults` indirection. That stack made sense when it also had to talk to providers. CodexBar removed that need for host bars: bash + `jq` + ImageMagick remains the lowest-friction glue for tmux and SketchyBar.

Zellij is different. A high-value Zellij integration must be a standalone WASM plugin so users can install one artifact and avoid `zjstatus`, feeder loops, and shell-script setup. Zellij officially supports Rust plugins, so the Zellij renderer is Rust. The project is not a full Rust port: SketchyBar and tmux stay on the shell path, with parity enforced by tests.

Go is not used because the Zellij plugin API is Rust-first. TinyGo/community bindings would add WASI/API risk, and a Go CLI plus Rust plugin would split compiled logic across two languages.

## Provider id mapping

CodexBar's JSON `provider` field is the canonical id and matches the filename of its bundled SVG (`ProviderIcon-<id>.svg`). The SketchyBar plugin uses these one-to-one — no remapping table.

The terminal strips render a 2-letter sigil per provider. New CodexBar providers fall back to the first two letters of the id.

Provider render order is deterministic. `SHOWY_QUOTA_PROVIDERS` / plugin `providers`, when set, is both an allow-list and render order. Otherwise `SHOWY_QUOTA_PROVIDER_ORDER` / plugin `provider_order` ranks providers without filtering them; missing providers are skipped, and unlisted providers render after ranked providers sorted by id. The default rank is `codex,claude,opencode,gemini`.

## Adding a new SketchyBar provider

CodexBar discovers providers; this repo discovers them via the cache content. Enable the provider in CodexBar and wait for the next refresh cycle. Zellij/tmux terminal strips render new providers automatically. SketchyBar declares/removes provider items on the next plugin tick after the filtered provider set changes; no reload is required after the initial install.

## External layout managers

`bin/showy-quota-state` is the public bridge for configs that need CodexBar's filtered provider count without duplicating CodexBar or renderer internals. It honors `SHOWY_QUOTA_PROVIDERS` / `SHOWY_QUOTA_PROVIDERS_EXCLUDE` and emits:

- `available`: whether a valid cache was read.
- `stale`: whether cache age exceeds `SHOWY_QUOTA_REFRESH_SECONDS * 2`.
- `cacheAgeSeconds`: seconds since usage cache mtime, or `null` when absent.
- `staleAfterSeconds`: numeric stale threshold.
- `providers`: filtered provider ids in render order.
- `providerCount`: `providers | length`.
- `sketchybar.compactRecommended`: `providerCount >= SHOWY_QUOTA_SKETCHYBAR_COMPACT_PROVIDER_COUNT`.

Consumers should treat `available=false` as "leave the current layout alone"; it means no last-known-good cache exists yet.

The SketchyBar plugin triggers `showy_quota_provider_change` when that filtered provider set changes, so configs can subscribe without polling if they want immediate layout reconciliation.
