# Architecture

`showy-quota` has two integration families:

1. **Shell integrations for host bars.** tmux and the advanced zjstatus path use the existing shell data plane plus a shared native terminal-strip renderer; SketchyBar keeps its native shell row/icon renderer.
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
bin/showy-quota-fetch  ← shared cache envelope + flock + atomic publish
        ├──► bin/showy-quota-state                 (stable provider/layout state JSON)
        ├──► adapters/sketchybar/plugins/showy_quota.sh    (native SketchyBar rows + icons)
        ├──► bin/showy-quota-tmux-bar             (thin driver → native tmux #[…] renderer)
        └──► bin/showy-quota-zellij-bar           (thin driver → native ANSI renderer for advanced zjstatus)
```

## Shell cache contract

The shell data plane is still the reliability boundary for tmux, SketchyBar, and advanced zjstatus composition.

- File: `${SHOWY_QUOTA_CACHE_DIR}/usage.json` (default: `${XDG_CACHE_HOME:-$HOME/.cache}/showy-quota/usage.json`) — a cache envelope object `{"schema":"showy-quota/cache@2","source":"serve"|"cli"|"unknown","providerMeta":{...},"providers":[...]}`, where `providers` is the verbatim CodexBar usage array. `providerMeta` maps each published provider id to `{"source":..., "updatedAt": <epoch>}`: the fetch that actually measured that slice, not the publish that republished it (see "Per-provider freshness" below). A legacy bare top-level array (pre-envelope cache) is still read, with `source` treated as `unknown`; it is upgraded to an envelope on the next successful refresh.
- Stamp file: `${SHOWY_QUOTA_CACHE_DIR}/usage.json.updated-at`
- Payload and source commit via a **single atomic `rename(2)`**: the fetcher builds the whole envelope (payload + source) in one temp file and publishes it with one `mv`, so a reader can never observe a NEW payload paired with STALE (or missing) source metadata — there is no second file whose independent commit order could reopen that race. The generation stamp still commits last, since `cache_payload_marker` is derived from the published payload's on-disk identity (inode/mtime/size) and can only be minted once that identity is stable. CLI source is visibly degraded as `⚠cli`.
- `flock` path: `${SHOWY_QUOTA_CACHE_DIR}/usage.lock`
- owner-scoped `mkdir` fallback path: `${SHOWY_QUOTA_CACHE_DIR}/usage.lock.d`
- Validation: `jq` must accept either shape — a bare array of provider objects, or an envelope whose `providers` field is one. If a usage window is present, its `usedPercent` must be numeric before publication.
- Corrupt cache quarantine: if the existing usage cache fails validation before
  a fetcher-owned refresh path runs, it is moved to
  `usage.json.corrupt.<epoch>.<pid>` and old quarantine files are pruned
  (`SHOWY_QUOTA_CORRUPT_CACHE_RETENTION`, default `3`).

The fetcher prints the cache's bare provider array to stdout — never the envelope wrapper — regardless of whether it just refreshed or served stale bytes; this is unchanged for every caller. Callers must not differentiate; if they want freshness data they read `--age`, and if they want the source marker they read `cache.source` from `showy-quota-state` or call `showy_quota_cache_source`. During non-forced lock contention, a caller with an existing valid cache may emit that snapshot immediately while the lock holder refreshes. Forced refresh callers wait for the holder and retry recovery first, but still fall back to an existing valid cache if no refreshed cache is published; this preserves the fetcher's last-known-good output contract.

Freshness is a shared render concern. A shell cache is stale when `showy_quota_age_seconds "${SHOWY_QUOTA_USAGE_FILE}"` is greater than `SHOWY_QUOTA_REFRESH_SECONDS * 2`. Shell bar drivers pass stale/degraded flags to the native renderer so tmux and advanced zjstatus show one trailing stale indicator, grey frozen data, and hide elapsed markers; `showy-quota-state` reports the boolean and threshold.

### Per-provider freshness

Per-provider CLI fallback can preserve one provider's slice while every other
provider refreshes: without per-slice metadata, the carried-forward record
inherits the publish's mtime and presents stale data as freshly fetched.
Every publish therefore records `providerMeta` — `source` and `updatedAt` per
provider id, with `updatedAt` set to the current time for freshly measured
slices and to the *previous envelope's* values for carried-forward slices.
The Rust cache reader (`CacheFreshness`) and `showy-quota-state`'s
`providerFreshness` map both derive each provider's age from that timestamp,
and each stale provider renders exactly like a wholly stale strip (grey
chunk, no pacing marker) while its fresh neighbors keep their colors. A
`serve` source with a `cli`-sourced slice still shows the degraded marker.
Providers without metadata inherit the file's freshness rather than
inventing their own.

Refreshes prefer `${SHOWY_QUOTA_CODEXBAR_SERVE_URL%/}/usage` with `curl`; the default base URL is `http://127.0.0.1:8080`. The `/health` probe uses `SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS` (default `10`) for fast liveness detection, while the `/usage` probe uses the larger `SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS` (default `30`): a healthy `codexbar serve` bounds collection per provider and can take up to ~0.8x its request deadline (~24s by default) to return the healthy providers when a slow one degrades to an error row, so reusing the short health timeout here would abandon that usable partial response and fall back to visibly degraded CLI data whenever any provider is briefly slow. Before falling back to CLI, `showy-quota-fetch` probes `/health` and, with `SHOWY_QUOTA_MANAGE_SERVE=1` (default), starts `codexbar serve` on the port implied by `SHOWY_QUOTA_CODEXBAR_SERVE_URL` (default `8080`) in the background with a pidfile. `SHOWY_QUOTA_CODEXBAR_SERVE_PORT` is now a compatibility override; when both are set and disagree, the fetcher logs a warning and prefers the URL port. Set `SHOWY_QUOTA_MANAGE_SERVE=0` to disable managed startup, or `SHOWY_QUOTA_CODEXBAR_SERVE_URL=` to skip HTTP entirely. When an existing cache is still fresh under `SHOWY_QUOTA_REFRESH_SECONDS`, the fetcher may still refresh from `codexbar serve` every `SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS` so bars repaint shortly after the server's own response cache changes. Failed HTTP probes keep serving the existing cache; they do not invoke the slower CLI fallback until the normal refresh interval expires.

Refresh cadence is deliberately derived from one knob. `SHOWY_QUOTA_REFRESH_SECONDS` (default `120`) is the freshness contract: a managed `codexbar serve` is started with `--refresh-interval` equal to it (so serve never collects more often than the contract promises), and the `/usage` poll re-reads at half of it (`SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS`, default `60`). Worst-case displayed data age is therefore ~1.5x the contract — inside the 2x stale horizon — and quota windows move on hour scales, so oversampling buys nothing except battery drain. `SHOWY_QUOTA_MANAGE_SERVE=1` stays the default on purpose: a resident serve performs the same collection work the CLI fallback would, minus a full CodexBar process launch per refresh, and degrades per provider instead of failing the whole snapshot. Both cadence knobs accept explicit overrides.

When a healthy `codexbar serve` reports a build `version` on `/health`, `showy-quota-fetch` reuses it only if that build matches the installed `codexbar --version`. Both the `/health` value and `codexbar --version` are reduced to a comparable version token (the first `v?`-digit field, `v` stripped) — the same normalization glean's stale-serve detector uses — so a `CodexBar`-prefixed `/health` value is not mistaken for a stale build, and a transient bare `CodexBar` yields no token (reuse, never recycle). On a real mismatch (e.g. a CodexBar update left a stale in-memory binary serving the port) it recycles the serve — terminating a managed serve through its pidfile, or freeing the configured port of a foreign serve after verifying each `lsof` listener PID is actually a CodexBar serve (command basename plus a `serve` argument) — and starts a fresh build. Listeners that fail verification are never signaled, and there is no name-based `pkill` fallback: when the port cannot be safely freed, the stale serve is reused. A serve whose `/health` omits `version` is reused unchanged, so the gate is a no-op for builds that predate the field. Recycling only happens with `SHOWY_QUOTA_MANAGE_SERVE=1`, runs under the existing fetch lock, and falls back to reuse when no recycle mechanism is available.
The configured `codexbar` binary is resolved to an absolute path before `--version` and before launching a managed serve, because CodexBar reads its version from the app bundle via `argv[0]` — invoked by a bare command name it reports no version (in `--version` and serve `/health`), which would otherwise leave the gate inert and make showy-quota's own recycled serves omit `/health.version`.

### Managed serve controls

`showy-quota serve status [--json]` inspects the shell fetcher's pidfile, configured local URL and port, `/health` response and version, failure count and backoff, and cache source and age. It never starts a process, changes the cache directory, removes a stale pidfile, or requests `/usage`. Status exits 0 only when the recorded managed process passes the pid, start-time, binary, and port checks and `/health` responds; it exits 1 otherwise. The JSON object groups fields under `managed`, `serve`, `failure`, and `cache`, with a top-level `healthy` boolean. A reachable foreign serve does not count as a healthy **managed** serve.

`showy-quota serve restart` requires `SHOWY_QUOTA_MANAGE_SERVE=1` and a loopback URL. It takes the shared refresh lock, stops only a verified managed process, starts the configured CodexBar binary with the URL's port and refresh interval, and waits for `/health`. It refuses to claim a restart when a foreign responder still owns the port. `showy-quota serve stop` removes a stale pidfile or stops only the identity-checked managed process under the same lock. Neither command fetches `/usage`; the normal cold refresh still owns quota collection. The standalone Zellij plugin manages its own process and does not use these shell controls.

The tmux and Zellij detail panes source showy-quota config when present, then run `${SHOWY_QUOTA_CODEXBAR_BIN:-codexbar} usage` directly because they display CodexBar's text UI, not the compact cache-backed renderer output.

## Standalone Zellij plugin contract

The plugin does not read the host cache and does not shell out to showy-quota scripts. Its default path is:

```text
web_request("/health") → start `codexbar serve` if needed → web_request("/usage") → parse → filter/order → render ANSI strip
```

It always requests `WebAccess`, then requests `OpenTerminalsOrPlugins` only when `manage_serve` is enabled and `RunCommands` only when degraded CLI fallback is enabled. If serve cannot be reached, the managed serve command uses the port from `serve_url` unless `serve_port` is set explicitly. CLI fallback is provider-aware: the plugin first runs `codexbar config providers --format json --pretty` once per discovery window, then issues one `codexbar usage --provider <id> --format json --pretty [--status]` per enabled provider. Successful per-provider records are merged into the in-memory payload incrementally, and one provider's failure or hang only stamps that provider's backoff — every other provider continues to render its last-known-good slice marked with `⚠cli`. Serve health probing continues so the plugin can switch back automatically.

The plugin keeps last-known-good JSON in memory for the pane/session. If refreshes fail after a success, it continues rendering the previous data and marks it stale at `2 × interval_seconds`. That preserves the user-visible last-known-good behavior without requiring `FullHdAccess` or a disk cache.

Hot-path compute is centralized in the native `showy-quota-render` binary. The tmux and advanced zjstatus shell bars only warm the cache and call `--from-cache`; the standalone Zellij plugin uses the same Rust rendering core in-process; the prompt segment comes from `--emit prompt`; and the SketchyBar plugin gets its whole tick from `--emit sketchybar-frame`: the final `sketchybar --set` arguments (rows, markers, labels, colors, icons, click scripts, stale/shared-cycle handling), diffed against the last frame sent, plus the decision to redeclare items. The notch split comes from `--emit sketchybar-layout`. SketchyBar's shell keeps only host integration: item declaration/teardown, icon rasterization, and the `sketchybar` calls.

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

`dual2` splits a model-pooled provider into one standalone `dual` per pool (`AGᴳ` Gemini, `AGᶜ` Claude+GPT), each rendered by the normal `dual` path (half-blocks, every terminal). It pairs `usage.extraRateWindows` by family (session+weekly), unions any positional pool not carried by the extras, and a single pool stays one plain `dual`. Auto-detection treats a provider as model-pooled only when its extras carry *more* pools than the positional slots expose, so a coincidental `windowMinutes`/`resetsAt` collision between a positional slot and one extra (e.g. Codex's main weekly and its Spark weekly) is not mistaken for pooling. Force per provider via `PROVIDER_MODES=<provider>=dual2`.

Window slots are semantic in every mode: a provider is renderable when any of its primary/secondary/tertiary windows reports a numeric `usedPercent`, and each present window renders in its own row, marker, and color role. A gap keeps later windows in place (a missing secondary never pulls the tertiary up), with one exception: when the **primary** slot is absent, the present windows left-compact into the leading slots so the live window drives the primary row and countdown. This is how Codex renders after OpenAI temporarily removed the 5h limit (`usage.primary: null`): the weekly cap promotes into the primary row with its real reset countdown instead of an empty top row and an `idle` label. A provider left with exactly **one** live window renders as a single full-height bar — a solid `█` body in the terminals, one centered native row on SketchyBar — with no empty second row or stranded lane, because one limit is one bar. A promoted window with no reset that is still full reads `idle`, and a provider with no present window at all is `idle`. Promotion is a render-only view — `showy-quota-state` still reports the raw positional windows untouched.

Color and pacing follow each window's **horizon**, not its row position. A window is dimmed — its severity color scaled by `SHOWY_QUOTA_PALETTE_DIM_SCALE` / `palette_dim_scale` (default `0.55`), or an explicit `SHOWY_QUOTA_PALETTE_DIM_*` override — when its `windowMinutes` is at or beyond `SHOWY_QUOTA_DIM_WINDOW_MINUTES` / `dim_window_minutes` (default `10080`, i.e. weekly or monthly). Shorter live tiers (5h, daily) stay at full brightness, and windows without a known `windowMinutes` are treated as bright. So a time-tiered provider (Codex/Claude: 5h + weekly) shows a bright 5h row over a dimmed weekly row; Antigravity's split Gemini and Claude+GPT pools each show a bright 5h row over a dimmed weekly row; uniform daily pools (Gemini) dim none. The `dual` body draws a pacing marker on each row; the mono bodies draw only the configured marker slots.

Pools that share one billing cycle — identical `resetsAt` and `windowMinutes` across at least two present slots — are an exception. They are parallel usage *categories* within a single budget (e.g. Cursor's Total/Auto/API on one 30-day cycle), not a live tier over a longer cap, so they render at full brightness regardless of horizon and draw a single pacing marker (the others would land on the identical column). This is why `cursor` ships as `mono3` by default.

The stacked modes collapse to the densest body the data supports: `mono4` needs four assembled windows (else it falls back to `mono3`, then `dual`); `mono3` needs a tertiary slot (else `dual`). Model-pooled Antigravity carries session+weekly windows per pool, so `auto` splits it into `AGᴳ` + `AGᶜ`; if a stacked body is forced, missing lanes still collapse the body rather than leaving empty rows, matching SketchyBar dropping an absent row.

### Plain-text templates

`showy-quota-render --emit template --format '{sigil} {remaining}% {countdown}' --from-cache` expands the format once per visible provider. Each expansion uses that provider's window with the **lowest remaining percentage**, including known extra-rate windows. Equal values keep the first window in primary, secondary, tertiary, then extra order. `--join SEP` joins expansions; the default separator is one space. An empty set prints an empty line. `showy-quota prompt --format '{provider}: {used}%'` selects the **single worst window across all providers** instead. Without `--format`, the prompt keeps its existing output (`{sigil} {used}% {countdown}{stale}`).

| Field | Text |
|---|---|
| `{provider}` / `{sigil}` | Provider ID / display sigil |
| `{used}` / `{remaining}` | Integer percentages without `%` |
| `{countdown}` | Reset countdown; empty when unknown |
| `{class}` | Configured severity: `good`, `warn`, or `bad` |
| `{window}` | `primary`, `secondary`, `tertiary`, or an extra-rate window title (`extra` if unnamed) |
| `{stale}` | A space and the configured stale glyph when the cache is stale; empty otherwise |

Use `{{` and `}}` to print literal braces. Unknown or unclosed fields fail with exit code 2 and name the field. Each expansion removes trailing ASCII spaces, so the default prompt does not leave a space when the reset time is unknown. Template output has no ANSI codes. The existing prompt `--ansi` option still controls color for a custom prompt format.

### Vertical view

`showy-quota-render --emit vertical` renders the same data on the other axis: one line per quota window instead of one line per provider. It exists for surfaces that own rows rather than columns — an SSH session on a phone, a tall sidebar pane — where the strip's whole reason for packing windows into one line disappears.

Every mosaic trade-off inverts with it. A window gets a full-height `█` bar, its horizon label, its own remaining percent and its own countdown, so nothing is encoded in half-block/sextant/octant sub-rows and no octant-capable terminal is required. Bodies (`dual`/`mono3`/`mono4`/`dual2`) and `SHOWY_QUOTA_TERMINAL_BAR_MODE` therefore do not apply. `SHOWY_QUOTA_VERTICAL_BAR_WIDTH` (default `16`, min 8) sets the body width; a default line is 43–45 columns — the widest sigil and horizon label in the snapshot set the rest, since a model-pooled provider's family tag (`AGᴳ`) costs every row a column — so it does not wrap on a phone.

A line is `⟨chip⟩ ⟨horizon⟩ ▕⟨bar⟩▏ ⟨percent⟩ ⟨countdown⟩ ⟨clock⟩`. The sigil chip is a coloured pill on the first line of each provider block only; continuation lines hold its width but stay on the page background, because a tinted letterless stub reads as the leading cells of the bar. The label sits *outside* the plate for the same reason — sharing the track's background hides where measurement begins. The percentage never inherits a dimmed band: it is the row's primary reading.

Three strip conventions are deliberately dropped, because each exists to compress information this view has room to state outright:

- **No dimming.** Dim says "weekly/monthly cap" in a body with no room to write it. Here the horizon is printed in its own column, so dim would restate a literal label at the cost of contrast on the glyphs that matter.
- **Pacing markers are ticks, not cells.** The marker is `│` drawn *over* the track (the cell keeps its fill state as background), so it can never be mistaken for usage or punch a hole in a full bar. One marker per line means one `palette_elapsed`; `palette_elapsed_long` and `mono_markers` do not apply.
- **Months, not calendar days.** A 30d and a 31d cycle are both monthly, so a roughly four-week horizon is labelled `1mo` rather than a raw day count that invites a meaningless comparison. Longer horizons round to whole months (`2mo`, `3mo`).

Windows are the strip's own inputs — positional slots plus `extraRateWindows` with known usage — with three selection rules. Positional slots are never deduplicated against each other, because Cursor's Total/Auto/API report one identical reset, horizon *and* usage yet are three distinct pools. Extras are dropped when they only republish a kept window (same horizon, reset and usage), which is how a pool CodexBar publishes both ways draws once; usage is part of that identity because distinct pools legitimately share a reset (Claude's weekly cap and its `Fable only` pool). A dropped extra still hands its **title** to the slot it republished, so Antigravity's two weekly slots render `7dᴳ`/`7dᶜ` instead of losing the only information that distinguishes them.

Labels are earned, not decorative: a window whose horizon is unique renders bare (`5h`, `7d`, `1mo`). A named window sharing a horizon takes the strip's existing superscript family tag (`7dᶠ`), and a nameless slot falls back to its slot ordinal (`1mo¹`, `1mo²`, `1mo³`) — but only when another nameless slot shares that horizon, so a lone slot beside a named window stays bare.

`SHOWY_QUOTA_VERTICAL_SORT` chooses the order. `provider` (default) keeps CodexBar's provider blocks with one blank line between them, which is what makes the grouping parse at a glance. `urgency` flattens the blocks so the window closest to running out is the first line, sorted by remaining then time-to-reset with ties falling back to CodexBar's own slot order; every line then carries its own chip and no separators are drawn.

`SHOWY_QUOTA_VERTICAL_RESET_CLOCK` appends each window's local reset time (`11:54`), answering *when* for rows that all read `1d`. It is **on** by default: the six columns it costs are columns this view has, and a clocked line still fits an SSH session on a phone. Set it to `0` to trade the wall time back for the width. The clock uses `SHOWY_QUOTA_RESET_DESCRIPTION_TIMEZONE_OFFSET` when set, otherwise the host's local offset, so it agrees with the countdown beside it.

Shared-cycle brightness and severity bands follow the strip. A stale snapshot suppresses pacing markers exactly as the strip does, but keeps every countdown — the countdown is the reading, not the pacing, and its stale colour already says the snapshot is old. Strip-level `stale`/`degraded_cli` glyphs move to their own trailing line, since a vertical view has no shared line to trail them on.

Cadence is the caller's: the renderer prints one frame. A live panel is a loop that warms the cache (`showy-quota-fetch`, which stays inside `SHOWY_QUOTA_REFRESH_SECONDS`) and redraws in place.

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
| `SHOWY_QUOTA_VERTICAL_BAR_WIDTH` / `vertical_bar_width` | `16` | `--emit vertical` bar cell width (min 8) |
| `SHOWY_QUOTA_VERTICAL_SORT` / `vertical_sort` | `provider` | `--emit vertical` order: `provider` blocks or `urgency` |
| `SHOWY_QUOTA_VERTICAL_RESET_CLOCK` / `vertical_reset_clock` | `1` | append each window's local reset clock (6 columns) |

## Failure semantics

| Condition | Shell integrations | Standalone Zellij plugin |
|---|---|---|
| `codexbar serve` unavailable | Fetcher starts serve, then falls back to provider-aware CLI (`⚠cli`) if startup/probe fails | Plugin starts serve via background command pane, then falls back to provider-discovery + per-provider `codexbar usage --provider <id>` calls (`⚠cli`) if needed |
| CodexBar CLI returns non-JSON | Provider call recorded as failed; cache otherwise untouched | Per-provider result rejected; the provider's previous in-memory slice (if any) remains |
| CodexBar JSON fails validation | Same — preserve last good cache | Same — preserve last in-memory value |
| No prior valid data | Renderers print `AI ?` or `AI idle` depending on path | Pane shows unavailable/invalid message or `AI idle` |
| Data older than stale threshold | One trailing `⚠`, grey frozen quota data, no elapsed markers | Same visual stale behavior from in-memory age |
| Zellij permission denied | Not applicable to shell/zjstatus feeder | Pane shows `showy-quota: permission denied` |

### Diagnosing a failed refresh

The SketchyBar plugin refreshes with `( fetch ) &` and stderr on `/dev/null`, so
`SHOWY_QUOTA_DEBUG=1` cannot be observed in a background cycle. Two records
survive it instead:

- `SHOWY_QUOTA_LOG_FILE=<path>` appends every `showy_quota_log` line, each
  prefixed with an epoch and the pid. It is opt-in, writes only on the cold
  fetch path, and never gates on `SHOWY_QUOTA_DEBUG`.
- `<cache>/provider-failures/<id>` holds the epoch on line 1 (the only line the
  backoff check reads) and `rc=<code>` on line 2. `rc=124` is the hard timeout,
  `rc=125` the output cap, `rc=unrenderable` a payload with no usable window,
  and any other code comes straight from `codexbar`.

A provider whose collection needs host privileges — the browser-cookie
providers — can fail only in the background cycle, because the responsible
process there is the bar, not your shell. A macOS upgrade resets those grants.

## Why bash and Rust, not Python/Go

The old `ai-quota` predecessor was Python with a daemon, sidecar, and `--client-defaults` indirection. That stack made sense when it also had to talk to providers. CodexBar removed that need for host bars: bash + `jq` remains the lowest-friction glue for the cold fetch path and host integration, while every hot render/compute path (terminal strips, prompt segment, providerMetrics, SketchyBar rows) is small and deterministic enough to centralize in Rust.

Zellij is different at the integration boundary. A high-value Zellij integration must be a standalone WASM plugin so users can install one artifact and avoid `zjstatus`, feeder loops, and shell-script setup. Zellij officially supports Rust plugins, so the standalone pane uses Rust in-process; tmux, advanced zjstatus, and SketchyBar keep shell drivers but delegate all row/strip compute to `showy-quota-render`. The shell cache remains the host-bar reliability boundary.

Go is not used because the Zellij plugin API is Rust-first. TinyGo/community bindings would add WASI/API risk, and a Go CLI plus Rust plugin would split compiled logic across two languages.

## Provider id mapping

CodexBar's JSON `provider` field is the canonical id and matches the filename of its bundled SVG (`ProviderIcon-<id>.svg`). The SketchyBar plugin uses these one-to-one — no remapping table.

`share/providers.tsv` holds the stable two-letter sigils, default display ranks, and optional SketchyBar app-font icon names. Shell reads it once with bash builtins; Rust embeds it in the core. Add or change a provider's display identity there, not in each renderer. Unknown CodexBar providers retain the first-two-letters sigil fallback. CodexBar still owns discovery, live status, and SVG icons.

Provider render order is deterministic. `SHOWY_QUOTA_PROVIDERS` / plugin `providers`, when set, is both an allow-list and render order. Otherwise `SHOWY_QUOTA_PROVIDER_ORDER` / plugin `provider_order` ranks providers without filtering them; missing providers are skipped, and unlisted providers render after ranked providers sorted by id. The default rank comes from `share/providers.tsv` and remains `codex,claude,copilot,opencode,gemini`.

## Adding a new SketchyBar provider

CodexBar discovers providers; this repo discovers them via the cache content. Enable the provider in CodexBar and wait for the next refresh cycle. Zellij/tmux terminal strips render new providers automatically. SketchyBar declares/removes provider items on the next plugin tick after the filtered provider set changes; no reload is required after the initial install.

## External layout managers

`bin/showy-quota-state` is the public bridge for configs that need CodexBar's filtered provider/layout state without duplicating CodexBar or renderer internals. It honors `SHOWY_QUOTA_PROVIDERS` / `SHOWY_QUOTA_PROVIDERS_EXCLUDE`, preserves renderer order, and emits:

The state JSON declares `schemaVersion: 1`. The draft 2020-12 contract is at `share/schema/showy-quota-state.schema.json`. Consumers should check the version before reading fields. Version 1 keeps existing field names and nesting. New optional fields may appear without a version change; removing fields, renaming them, or changing their types needs a new version. `showy-quota --diagnose --json` carries its own top-level `schemaVersion: 1` and embeds the state as `state` without changing it. Moving `sketchybar` into an `adapters` namespace is future breaking work, not part of version 1.

| Field | Meaning |
|---|---|
| `schemaVersion` | Integer state contract version; currently `1`. |
| `available` | Whether a valid cache was read. |
| `stale` | Whether cache age exceeds `SHOWY_QUOTA_REFRESH_SECONDS * 2`. |
| `cache.source`, `cache.degraded` | Cache source marker (`serve`, `cli`, or `unknown`) and whether CLI fallback is visible. |
| `cacheAgeSeconds` | Seconds since usage cache mtime, or `null` when absent. |
| `staleAfterSeconds` | Numeric stale threshold. |
| `providers[]` | Filtered provider id strings in render order (for example, `"codex"`). This is the stable flat list external layout managers use for item reconciliation. |
| `providerMetrics[]` | Filtered provider metrics in render order, plus valid errored providers after the same allow/exclude/order filters. Renderable entries are `{ "provider": "codex", "windows": { "primary": W\|null, "secondary": W\|null, "tertiary": W\|null }, "extraRateWindows": [E...], "error": null }`; errored entries use the same window keys set to `null`, `extraRateWindows: []`, and `error: { "kind": K, "message": M }`. A provider counts as renderable with a numeric `usedPercent` on a positional window; extra rate windows alone do not count, matching the strip's `AI idle`. |
| `providerMetrics[].windows.*` | Positional window slots; missing or non-numeric `usedPercent` slots are `null` and never shifted up. `W` contains `usedPercent`, `remainingPercent`, `resetsAt`, `resetDescription`, `windowMinutes`, and `minutesUntilReset`. |
| `providerMetrics[].extraRateWindows[]` | Extra rate windows from CodexBar. `E` adds `title` and `usageKnown` to the same usage fields as `W`; unknown usage keeps usage fields `null`. |
| `providerMetrics[].error` | `null` for renderable providers. For non-renderable provider errors, `kind` is bucketed by case-insensitive message substrings (`auth`, `login`, `session`, or `token` → `auth`; `cookie` → `cookies`; `timeout`, `connect`, `network`, or `refused` → `network`; otherwise `unknown`) and `message` is sanitized for state JSON/diagnose output. |
| `providerFreshness` | Map from rendered provider id to `{source, updatedAt, ageSeconds, stale}`, derived from the envelope's `providerMeta` plus the file mtime. Everything the per-provider freshness section describes, keyed by what the strip actually drew. |
| `emptyReason` | Why the surface is empty, when it is: `unavailable` (no cache), `no-providers` (CodexBar published an empty inventory), `filtered` (allow/exclude lists removed every provider), `idle` (providers exist and are unused), or `null` (something rendered). The strip prints the matching label (`AI idle`, `AI none`, `AI filtered`). |
| `sketchybar.compactRecommended` | `providerCount >= SHOWY_QUOTA_SKETCHYBAR_COMPACT_PROVIDER_COUNT`. |

`showy-quota-state --explain` lists one decision for every raw cache record. Add `--json` for a JSON array of `{provider, reason, sourceIndex, position, rankSource, orderRank}`. `provider` can be `null` or another invalid raw value. `sourceIndex` is the zero-based cache slot. `position` is the zero-based filtered order, or `null` when excluded. `rankSource` names `allowlist`, `provider_order`, or `cache`; `orderRank` is the zero-based rank in that source, or `null` for an unlisted provider. Reasons are `included`, `excluded_by_allowlist`, `excluded_by_denylist`, `invalid_id`, `error_record`, and `no_numeric_usage_window`. Invalid ids and error records take priority over filters; deny-list wins over allow-list. The text form prints provider, reason, position, rank source, and order rank as tab-separated values. This path inspects the cache; it does not fetch provider usage or add fields to the default state JSON.

The diagnose text includes a cached provider-decision section. The diagnose JSON includes the same decisions in `providerDecisions`; it keeps the embedded state unchanged. The fixture test uses Python `jsonschema` with draft 2020-12 when the module exists, or a `jq` structural check otherwise. CI installs Bash and `jq`, but does not install `jsonschema`.

`providerMetrics` is computed by the native renderer (`showy-quota-render --emit metrics`, the same binary the bar drivers use), which `showy-quota-state` invokes with the raw CodexBar payload on stdin; the shell no longer parses reset times or window math, so the metrics are deterministic across platforms (no BSD/GNU `date` divergence). Provider ids in `providerMetrics` are validated with the same strict predicate as the rest of the pipeline — `.`, `..`, and leading-dash ids are rejected, not merely regex-matched. The flat `providers[]` list, `available`, `stale`, cache, and `sketchybar` fields remain shell-owned.

Consumers should treat `available=false` as "leave the current layout alone"; it means no last-known-good cache exists yet.

The SketchyBar plugin triggers `showy_quota_provider_change` when that filtered provider set changes, so configs can subscribe without polling if they want immediate layout reconciliation.
