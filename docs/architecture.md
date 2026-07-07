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
  ├─ RunCommands provider discovery via `codexbar config providers --format json`
  ├─ RunCommands per-provider degraded fallback to `codexbar usage --provider <id> --format json --pretty`
  ├─ in-memory last-known-good data, kept per provider so one failure does not blow away the others
  └─ ANSI-styled terminal strip rendered directly in a one-line Zellij pane

Shell integrations:

codexbar serve /health + /usage  or  codexbar usage --format json
        ▲
        │ start if absent (managed pidfile)
        │
bin/showy-quota-fetch  ← shared cache + source marker + flock + atomic publish
        ├──► bin/showy-quota-state                 (stable provider/layout state JSON)
        ├──► adapters/sketchybar/plugins/showy_quota.sh    (native SketchyBar rows + icons)
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
- Corrupt cache quarantine: if the existing usage cache fails validation before
  a fetcher-owned refresh path runs, it is moved to
  `usage.json.corrupt.<epoch>.<pid>` and old quarantine files are pruned
  (`SHOWY_QUOTA_CORRUPT_CACHE_RETENTION`, default `3`).

The fetcher prints the cache content to stdout regardless of whether it just refreshed or served stale bytes. Callers must not differentiate; if they want freshness data they read `--age`. During non-forced lock contention, a caller with an existing valid cache may emit that snapshot immediately while the lock holder refreshes. Forced refresh callers wait for the holder and retry recovery first, but still fall back to an existing valid cache if no refreshed cache is published; this preserves the fetcher's last-known-good output contract.

Freshness is a shared render concern. A shell cache is stale when `showy_quota_age_seconds "${SHOWY_QUOTA_USAGE_FILE}"` is greater than `SHOWY_QUOTA_REFRESH_SECONDS * 2`. Shell renderers show one trailing stale indicator, grey frozen data, and hide elapsed markers; `showy-quota-state` reports the boolean and threshold.

Refreshes prefer `${SHOWY_QUOTA_CODEXBAR_SERVE_URL%/}/usage` with `curl`; the default base URL is `http://127.0.0.1:8080`. The `/health` probe uses `SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS` (default `10`) for fast liveness detection, while the `/usage` probe uses the larger `SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS` (default `30`): a healthy `codexbar serve` bounds collection per provider and can take up to ~0.8x its request deadline (~24s by default) to return the healthy providers when a slow one degrades to an error row, so reusing the short health timeout here would abandon that usable partial response and fall back to visibly degraded CLI data whenever any provider is briefly slow. Before falling back to CLI, `showy-quota-fetch` probes `/health` and, with `SHOWY_QUOTA_MANAGE_SERVE=1` (default), starts `codexbar serve` on the port implied by `SHOWY_QUOTA_CODEXBAR_SERVE_URL` (default `8080`) in the background with a pidfile. `SHOWY_QUOTA_CODEXBAR_SERVE_PORT` is now a compatibility override; when both are set and disagree, the fetcher logs a warning and prefers the URL port. Set `SHOWY_QUOTA_MANAGE_SERVE=0` to disable managed startup, or `SHOWY_QUOTA_CODEXBAR_SERVE_URL=` to skip HTTP entirely. When an existing cache is still fresh under `SHOWY_QUOTA_REFRESH_SECONDS`, the fetcher may still refresh from `codexbar serve` every `SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS` so bars repaint shortly after the server's own response cache changes. Failed HTTP probes keep serving the existing cache; they do not invoke the slower CLI fallback until the normal refresh interval expires.

When a healthy `codexbar serve` reports a build `version` on `/health`, `showy-quota-fetch` reuses it only if that build matches the installed `codexbar --version`. Both the `/health` value and `codexbar --version` are reduced to a comparable version token (the first `v?`-digit field, `v` stripped) — the same normalization glean's stale-serve detector uses — so a `CodexBar`-prefixed `/health` value is not mistaken for a stale build, and a transient bare `CodexBar` yields no token (reuse, never recycle). On a real mismatch (e.g. a CodexBar update left a stale in-memory binary serving the port) it recycles the serve — terminating a managed serve through its pidfile, or freeing the configured port of a foreign serve after verifying each `lsof` listener PID is actually a CodexBar serve (command basename plus a `serve` argument) — and starts a fresh build. Listeners that fail verification are never signaled, and there is no name-based `pkill` fallback: when the port cannot be safely freed, the stale serve is reused. A serve whose `/health` omits `version` is reused unchanged, so the gate is a no-op for builds that predate the field. Recycling only happens with `SHOWY_QUOTA_MANAGE_SERVE=1`, runs under the existing fetch lock, and falls back to reuse when no recycle mechanism is available.
The configured `codexbar` binary is resolved to an absolute path before `--version` and before launching a managed serve, because CodexBar reads its version from the app bundle via `argv[0]` — invoked by a bare command name it reports no version (in `--version` and serve `/health`), which would otherwise leave the gate inert and make showy-quota's own recycled serves omit `/health.version`.

The tmux and Zellij detail panes source showy-quota config when present, then run `${SHOWY_QUOTA_CODEXBAR_BIN:-codexbar} usage` directly because they display CodexBar's text UI, not the compact cache-backed renderer output.

## Standalone Zellij plugin contract

The plugin does not read the host cache and does not shell out to showy-quota scripts. Its default path is:

```text
web_request("/health") → start `codexbar serve` if needed → web_request("/usage") → parse → filter/order → render ANSI strip
```

It always requests `WebAccess`, then requests `OpenTerminalsOrPlugins` only when `manage_serve` is enabled and `RunCommands` only when degraded CLI fallback is enabled. If serve cannot be reached, the managed serve command uses the port from `serve_url` unless `serve_port` is set explicitly. CLI fallback is provider-aware: the plugin first runs `codexbar config providers --format json --pretty` once per discovery window, then issues one `codexbar usage --provider <id> --format json --pretty [--status]` per enabled provider. Successful per-provider records are merged into the in-memory payload incrementally, and one provider's failure or hang only stamps that provider's backoff — every other provider continues to render its last-known-good slice marked with `⚠cli`. Serve health probing continues so the plugin can switch back automatically.

The plugin keeps last-known-good JSON in memory for the pane/session. If refreshes fail after a success, it continues rendering the previous data and marks it stale at `2 × interval_seconds`. That preserves the user-visible last-known-good behavior without requiring `FullHdAccess` or a disk cache.

The Rust renderer intentionally duplicates only the terminal strip logic. Golden tests compare its output to `bin/showy-quota-zellij-bar` over the fixture set so tmux/SketchyBar can stay shell-based without visual drift in Zellij.

## Terminal rendering modes

`SHOWY_QUOTA_TERMINAL_BAR_MODE` (shell) / `terminal_bar_mode` (plugin KDL) sets the Zellij/tmux bar body: `auto` (default), `dual`, `dual2`, `mono3`, or `mono4`. In `auto`, each provider's body comes from the `SHOWY_QUOTA_PROVIDER_MODES` / `provider_modes` map (default `gemini=mono3,cursor=mono3`); providers without an entry render `dual`, except model pools — a provider whose `extraRateWindows` carry all its positional slots auto-detects as model-pooled and splits into one standalone `dual` per pool (`AGᴳ`, `AGᶜ`); a single pool stays one plain `dual`. `mono4` is opt-in only; an explicit `provider=dual2`/`mono4` forces the pool view and never happens automatically for `mono4`.

`mono3` packs primary/secondary/tertiary into one U+1FB00 sextant row; `mono4` packs up to four windows into one U+1CD00 octant row. Both use a single provider-level foreground color (`SHOWY_QUOTA_MONO_COLOR_MODE` / `mono_color_mode`: `lowest` (default) or `primary`), dimmed only when every present window is a long-horizon cap. Pacing markers are the `SHOWY_QUOTA_MONO_MARKERS` / `mono_markers` list of window slots (`primary`, `secondary`, `tertiary`, `quaternary`; default `primary`; `none` disables); the first marker uses `palette_elapsed`, the rest `palette_elapsed_long`. More than two markers crowd an 8–12 cell bar. Stale snapshots hide markers.

`mono4`'s windows are assembled generically from `usage.primary/secondary/tertiary` plus `usage.extraRateWindows` (distinct windows, slots first, deduped) — e.g. Antigravity's Gemini and Claude+GPT session/weekly pools. It requires an octant-capable terminal:

| body | glyphs | renders in |
|---|---|---|
| `dual` | half-blocks (U+2580) | every terminal |
| `mono3` | sextants (U+1FB00) | most, incl. Alacritty, iTerm2 |
| `mono4` | octants (U+1CD00, Unicode 16) | Ghostty, kitty, WezTerm, libvte only |
| `dual2` | half-blocks (U+2580) | every terminal |

Run `python3 docs/scripts/preview-quad-octants.py` to test a terminal and preview `mono4` before enabling it; octants render as tofu where unsupported.

`dual2` splits a model-pooled provider into one standalone `dual` per pool (`AGᴳ` Gemini, `AGᶜ` Claude+GPT), each rendered by the normal `dual` path (half-blocks, every terminal). It pairs `usage.extraRateWindows` by family (session+weekly), unions any positional pool not carried by the extras (e.g. Codex + Spark), and a single pool stays one plain `dual`. Force per provider via `PROVIDER_MODES=<provider>=dual2`.

Window slots are semantic in every mode: a provider is renderable when any of its primary/secondary/tertiary windows reports a numeric `usedPercent`, and each window only ever renders in its own row, marker, and color role. A missing window leaves its row empty rather than shifting later windows up; a missing primary additionally renders an `idle` countdown label because there is no primary reset to count down (a provider may report `usage.primary: null` with live secondary/tertiary windows).

Color and pacing follow each window's **horizon**, not its row position. A window is dimmed — its severity color scaled by `SHOWY_QUOTA_PALETTE_DIM_SCALE` / `palette_dim_scale` (default `0.55`), or an explicit `SHOWY_QUOTA_PALETTE_DIM_*` override — when its `windowMinutes` is at or beyond `SHOWY_QUOTA_DIM_WINDOW_MINUTES` / `dim_window_minutes` (default `10080`, i.e. weekly or monthly). Shorter live tiers (5h, daily) stay at full brightness, and windows without a known `windowMinutes` are treated as bright. So a time-tiered provider (Codex/Claude: 5h + weekly) shows a bright 5h row over a dimmed weekly row; Antigravity's split Gemini and Claude+GPT pools each show a bright 5h row over a dimmed weekly row; uniform daily pools (Gemini) dim none. The `dual` body draws a pacing marker on each row; the mono bodies draw only the configured marker slots.

Pools that share one billing cycle — identical `resetsAt` and `windowMinutes` across at least two present slots — are an exception. They are parallel usage *categories* within a single budget (e.g. Cursor's Total/Auto/API on one 30-day cycle), not a live tier over a longer cap, so they render at full brightness regardless of horizon and draw a single pacing marker (the others would land on the identical column). This is why `cursor` ships as `mono3` by default.

The stacked modes collapse to the densest body the data supports: `mono4` needs four assembled windows (else it falls back to `mono3`, then `dual`); `mono3` needs a tertiary slot (else `dual`). Model-pooled Antigravity carries session+weekly windows per pool, so `auto` splits it into `AGᴳ` + `AGᶜ`; if a stacked body is forced, missing lanes still collapse the body rather than leaving empty rows, matching SketchyBar dropping an absent row.

### Bar configuration reference

| Env (shell) / KDL key | Default | Meaning |
|---|---|---|
| `SHOWY_QUOTA_TERMINAL_BAR_MODE` / `terminal_bar_mode` | `auto` | `auto`, `dual`, `dual2`, `mono3`, `mono4` |
| `SHOWY_QUOTA_PROVIDER_MODES` / `provider_modes` | `gemini=mono3,cursor=mono3` | per-provider body in `auto`; model pools (extras carry all slots) split into standalone `dual` widgets per pool; `provider=mode,…` overrides |
| `SHOWY_QUOTA_MONO_COLOR_MODE` / `mono_color_mode` | `lowest` | mono3/mono4 chunk color: `lowest` or `primary` |
| `SHOWY_QUOTA_MONO_MARKERS` / `mono_markers` | `primary` | comma list of paced slots; `none` disables |
| `SHOWY_QUOTA_PALETTE_ELAPSED` / `palette_elapsed` | `be95ff` | first pacing marker color |
| `SHOWY_QUOTA_PALETTE_ELAPSED_LONG` / `palette_elapsed_long` | `3ddbd9` | second+ pacing marker color |
| `SHOWY_QUOTA_DIM_WINDOW_MINUTES` / `dim_window_minutes` | `10080` | windowMinutes at/above which a window dims (weekly) |
| `SHOWY_QUOTA_PALETTE_DIM_SCALE` / `palette_dim_scale` | `0.55` | brightness scale for dimmed (long-horizon) windows |
| `SHOWY_QUOTA_ZELLIJ_BAR_WIDTH` / `bar_width` | `12` | bar cell width (min 8) |

## Failure semantics

| Condition | Shell integrations | Standalone Zellij plugin |
|---|---|---|
| `codexbar serve` unavailable | Fetcher starts serve, then falls back to provider-aware CLI (`⚠cli`) if startup/probe fails | Plugin starts serve via background command pane, then falls back to provider-discovery + per-provider `codexbar usage --provider <id>` calls (`⚠cli`) if needed |
| CodexBar CLI returns non-JSON | Provider call recorded as failed; cache otherwise untouched | Per-provider result rejected; the provider's previous in-memory slice (if any) remains |
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

Provider render order is deterministic. `SHOWY_QUOTA_PROVIDERS` / plugin `providers`, when set, is both an allow-list and render order. Otherwise `SHOWY_QUOTA_PROVIDER_ORDER` / plugin `provider_order` ranks providers without filtering them; missing providers are skipped, and unlisted providers render after ranked providers sorted by id. The default rank is `codex,claude,copilot,opencode,gemini`.

## Adding a new SketchyBar provider

CodexBar discovers providers; this repo discovers them via the cache content. Enable the provider in CodexBar and wait for the next refresh cycle. Zellij/tmux terminal strips render new providers automatically. SketchyBar declares/removes provider items on the next plugin tick after the filtered provider set changes; no reload is required after the initial install.

## External layout managers

`bin/showy-quota-state` is the public bridge for configs that need CodexBar's filtered provider/layout state without duplicating CodexBar or renderer internals. It honors `SHOWY_QUOTA_PROVIDERS` / `SHOWY_QUOTA_PROVIDERS_EXCLUDE`, preserves renderer order, and emits:

| Field | Meaning |
|---|---|
| `available` | Whether a valid cache was read. |
| `stale` | Whether cache age exceeds `SHOWY_QUOTA_REFRESH_SECONDS * 2`. |
| `cache.source`, `cache.degraded` | Cache source marker (`serve`, `cli`, or `unknown`) and whether CLI fallback is visible. |
| `cacheAgeSeconds` | Seconds since usage cache mtime, or `null` when absent. |
| `staleAfterSeconds` | Numeric stale threshold. |
| `providers[]` | Filtered provider id strings in render order (for example, `"codex"`). This is the stable flat list external layout managers use for item reconciliation. |
| `providerMetrics[]` | Filtered provider metrics in the same render order. Each element is `{ "provider": "codex", "windows": { "primary": W|null, "secondary": W|null, "tertiary": W|null }, "extraRateWindows": [E...] }`. |
| `providerMetrics[].windows.*` | Positional window slots; missing or non-numeric `usedPercent` slots are `null` and never shifted up. `W` contains `usedPercent`, `remainingPercent`, `resetsAt`, `resetDescription`, `windowMinutes`, and `minutesUntilReset`. |
| `providerMetrics[].extraRateWindows[]` | Extra rate windows from CodexBar. `E` adds `title` and `usageKnown` to the same usage fields as `W`; unknown usage keeps usage fields `null`. |
| `providerCount` | `providers | length`. |
| `sketchybar.compactRecommended` | `providerCount >= SHOWY_QUOTA_SKETCHYBAR_COMPACT_PROVIDER_COUNT`. |

Consumers should treat `available=false` as "leave the current layout alone"; it means no last-known-good cache exists yet.

The SketchyBar plugin triggers `showy_quota_provider_change` when that filtered provider set changes, so configs can subscribe without polling if they want immediate layout reconciliation.
