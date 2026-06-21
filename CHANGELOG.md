# Changelog

All notable changes to `showy-quota` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Fixed
- `bin/showy-quota-fetch` now matches the managed `codexbar serve` process by
  splitting the `ps` command output with `read -ra` instead of an unquoted
  expansion. The previous `for token in ${command}` glob-expanded the command
  string against the working directory, so a cwd containing a file matching a
  glob character in the command line could misclassify a healthy serve and
  trigger an unnecessary restart (or miss a running serve).
- `bin/showy-quota-fetch`'s cache publisher now checks each `mv` when promoting
  the temporary `usage`/`usage-stamp`/`source` files and removes the leftover
  temporaries on failure instead of leaking them into the cache directory.
- The Rust renderer's pacing-marker math now computes in `u64` before narrowing
  to `usize`, so a large `windowMinutes` no longer truncates or overflows on the
  32-bit `usize` of the `wasm32` Zellij plugin (it already matched the shell's
  64-bit arithmetic on native targets).
- Palette dim-scaling widens to `u128` before multiplying, so a pathological
  scale factor can no longer overflow (debug panic / release wrap); channels
  clamp to `0xff` as before.

## [0.3.0] — 2026-06-20

### Fixed
- The SketchyBar countdown label no longer truncates the widest `HH:MM`
  reset times (e.g. Gemini's `23:59` lost its last digit). The pinned
  `SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH` default is now `32` (was `27`), which
  fits the longest countdown form; shorter strings stay jitter-free as before.
- The Zellij/tmux `mono3` body now collapses to the two-lane
  `dual` body for providers without a tertiary window, matching the SketchyBar
  renderer which already drops the absent tertiary row, so a provider whose
  third positional slot is empty no longer shows a blank bottom lane.
- The standalone Zellij plugin no longer floods macOS with keychain prompts
  (and no longer leaks `codexbar usage --provider …` processes as zellij-server
  children) when `codexbar serve` cannot reach a provider that blocks on an
  interactive prompt — e.g. the login-keychain "Claude Code-credentials" dialog.
  Per-provider CLI fallback and provider discovery now run through a POSIX-sh
  watchdog that terminates the spawned command after
  `PROVIDER_COMMAND_TIMEOUT_SECONDS` (15s); Zellij has no command-cancel API, so
  a wedged command previously survived forever and a fresh prompt queued on
  every retry. Per-provider failure backoff now escalates exponentially (base
  doubling per consecutive failure, capped at 30 min), so a persistently
  blocking provider is probed rarely instead of every tick while its
  last-known-good slice keeps rendering as degraded.
- `bin/showy-quota-fetch` no longer restarts the managed `codexbar serve` when
  `/usage` merely times out (curl exit 28). A serve whose `/health` is OK but
  whose `/usage` blocks on provider collection (keychain) is alive, not
  unhealthy; restarting it only spawned a fresh serve that re-triggered the same
  blocking collection and prompt. It now records a serve failure and falls back
  without churning serve.
- Per-provider CLI fallback now carries forward each provider's last-known-good
  record from the existing cache when its fresh fetch fails or is in backoff, so
  a transiently blocking provider (claude, cursor) renders stale instead of
  vanishing from the bars. Carry-forward is gated on at least one fresh success,
  so a cycle where every provider fails still preserves the prior cache and lets
  it age to stale rather than republishing it under a fresh timestamp.

### Changed
- Model-pooled providers now adapt to the data with no configuration: in `auto`
  mode a provider whose `usage.extraRateWindows` carry every present positional
  slot is auto-detected and **split into one standalone `dual` provider per
  pool** — `AGᴳ` (Gemini), `AGᶜ` (Claude+GPT) — each a normal dual widget
  (semantic-colored sigil, full-width bar, pacing marker). A single pool stays
  one plain `dual`. This fixes Antigravity, whose pools depend on CodexBar's
  auth method: OAuth reports only Gemini (one pool → `AG`); the Antigravity IDE
  reports Gemini plus Claude+GPT (two pools → `AGᴳ` + `AGᶜ`). The split reuses the
  existing `dual` renderer on every surface — no combined-widget code.
  `SHOWY_QUOTA_PROVIDER_MODES` / `provider_modes` force it per provider
  (`provider=dual2`), unioning a positional pool with extra pools (e.g. Codex +
  Spark → `CXᶜ` + `CXˢ`), or select the opt-in octant body (`provider=mono4`,
  terminal support required). The SketchyBar plugin shows a pooled provider's
  windows as adaptive 2–4 native slider rows.
- `make install-plugin` now pre-grants the standalone Zellij plugin's
  permissions for the installed path, so a fresh install is prompt-free on
  first launch (previously it only printed a reminder to run
  `make grant-zellij-permissions`). The grant is best-effort and never fails
  the install. It covers first launch only: a later macOS cache purge still
  drops the grant, so re-run `make grant-zellij-permissions` if the prompt
  returns — an upstream Zellij limitation, not a regression.
- The SketchyBar bar slot default (`SHOWY_QUOTA_SKETCHYBAR_BAR_WIDTH`) is now
  `SHOWY_QUOTA_PNG_BAR_W + 3` (was `+ 4`), trimming 1px of dead space between
  each provider's bars and its countdown. This offsets the wider countdown
  label above so per-provider width stays effectively unchanged; the 80px bars
  are untouched.
- Bar dimming and pacing are now driven by each usage window's **horizon**
  instead of its row position. A window is dimmed only when its `windowMinutes`
  is at or beyond `SHOWY_QUOTA_DIM_WINDOW_MINUTES` (default `10080`, i.e.
  weekly/monthly), so a 5h live tier stays bright while its weekly/monthly cap
  dims — regardless of which slot it occupies. Time-tiered providers (Codex,
  Claude) keep a bright 5h row over a dimmed weekly row; model-pooled providers
  (Antigravity) keep each pool's bright 5h row over its dimmed weekly; Gemini's uniform-daily pools dim
  none. The `dual` body now draws a pacing marker on **both** rows, and the
  SketchyBar plugin gained a `primary_marker` so every pool is paced (was
  secondary/tertiary only).
- The Zellij/tmux marker is now a configurable list: `SHOWY_QUOTA_MONO_MARKERS`
  (default `primary`) names which window slots get a pacing separator (`none`
  disables), replacing the single `SHOWY_QUOTA_MONO3_MARKER_SOURCE`. The first
  marker uses `palette_elapsed`, the rest a distinct `palette_elapsed_long`.
- `auto` mode now reads the per-provider `SHOWY_QUOTA_PROVIDER_MODES` map
  (default `gemini=mono3,cursor=mono3`) instead of the `mono3_providers`
  allow/deny lists; providers without an entry render `dual`, except
  auto-detected model pools (see above). `mono4` is opt-in via this map and is
  never chosen automatically.
- Pools that share one billing cycle — identical `resetsAt` and `windowMinutes`
  across at least two present windows — now render at full brightness and draw a
  single pacing marker in every renderer (Zellij, tmux, SketchyBar). These are
  parallel usage *categories* within one budget (e.g. Cursor's Total/Auto/API on
  one 30-day cycle), not a live tier over a longer cap, so the horizon-based
  dimming and per-row markers that suit Codex/Claude are suppressed for them.
  `cursor` now ships as `mono3` by default so all three pools are visible (the
  `dual` body dropped the API pool); the SketchyBar rows and tmux/Zellij bodies
  stop showing redundant dimming and duplicate markers for it.

### Added
- `SHOWY_QUOTA_PALETTE_DIM_SCALE` / `palette_dim_scale` (default `0.55`) and
  `SHOWY_QUOTA_PALETTE_DIM_{GOOD,WARN,BAD,UNKNOWN}` / `palette_dim_*` set the
  color of dimmed (long-horizon) windows, plus `SHOWY_QUOTA_DIM_WINDOW_MINUTES`
  / `dim_window_minutes` (default `10080`) sets the weekly/monthly dim threshold.
- New `mono4` terminal body: four per-pool windows packed into one Unicode 16
  octant cell (`U+1CD00`), the 4-lane sibling of `mono3`'s sextants. mono4's
  windows are assembled from `usage.extraRateWindows` (e.g. Antigravity's Gemini
  and Claude+GPT session/weekly pools) and collapse to `mono3`/`dual` when fewer
  than four are available. Requires an octant-capable terminal (Ghostty, kitty,
  WezTerm); `docs/scripts/preview-quad-octants.py` tests a terminal and previews it.
- `SHOWY_QUOTA_PALETTE_ELAPSED_LONG` (default `3ddbd9`) colors the second and
  later pacing markers; `SHOWY_QUOTA_MONO_MARKERS` (list) and
  `SHOWY_QUOTA_PROVIDER_MODES` (map) configure markers and per-provider bodies.
- New `dual2` terminal body: splits a model-pooled provider into one standalone
  `dual` per pool (`AGᴳ` Gemini, `AGᶜ` Claude+GPT) from `usage.extraRateWindows`,
  each rendered by the normal half-block `dual` path (every terminal, unlike
  `mono4`). Auto-detected in `auto` when a provider's extras carry all its
  positional slots; force per provider via
  `SHOWY_QUOTA_PROVIDER_MODES=<provider>=dual2`; a single pool stays one plain
  `dual`.

### Removed
- The row-position palette knobs `SHOWY_QUOTA_PALETTE_SECONDARY_*`,
  `SHOWY_QUOTA_PALETTE_TERTIARY_*`, `SHOWY_QUOTA_PALETTE_SECONDARY_SCALE`, and
  `SHOWY_QUOTA_PALETTE_TERTIARY_SCALE` (and their KDL equivalents). Dimming is
  now horizon-based via `SHOWY_QUOTA_PALETTE_DIM_*` / `SHOWY_QUOTA_PALETTE_DIM_SCALE`;
  move any secondary/tertiary color or scale overrides to the `DIM` keys.
- The `sextant3` terminal mode (its per-column-color variant of `mono3` was a
  forced-only niche `auto` never selected). Use `mono3` (or `mono4`).
- `SHOWY_QUOTA_MONO3_PROVIDERS` / `_EXCLUDE` (use `SHOWY_QUOTA_PROVIDER_MODES`),
  `SHOWY_QUOTA_MONO3_MARKER_SOURCE` (use `SHOWY_QUOTA_MONO_MARKERS`),
  `SHOWY_QUOTA_MONO3_COLOR_MODE` (renamed `SHOWY_QUOTA_MONO_COLOR_MODE`), and
  `SHOWY_QUOTA_MONO3_MARKER_STYLE` (markers now always replace a cell; the
  `insert` style and the `shared` marker source are gone).

## [0.2.5] — 2026-06-12

### Changed
- The SketchyBar countdown label now reserves a fixed width
  (`SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH`, default `27`) so provider rows and
  the pill no longer jitter as the remaining-time string changes length.

### Fixed
- Shell and standalone Zellij renderers now keep providers whose primary quota
  window is absent when secondary or tertiary quota windows are still valid.
  This restores Antigravity, whose CodexBar payload currently reports
  `usage.primary: null` with usable secondary/tertiary windows.
- Usage window slots are now semantic across every renderer (SketchyBar,
  tmux, Zellij shell strip, and the standalone Zellij plugin): primary,
  secondary, and tertiary windows always map to their documented top, middle,
  and bottom rows. Previously missing windows were compacted away, so a
  provider without a primary window rendered its secondary window in the
  primary row and borrowed its reset for the countdown. A missing primary now
  renders an empty top row with an `idle` label, while present windows stay
  in their own rows, markers, and color roles.
- `bin/showy-quota-fetch` now treats a successful empty CodexBar provider
  inventory as canonical empty state, publishes `[]`, and bypasses stale serve
  or CLI backoff data instead of preserving disabled providers.
- The standalone Zellij plugin now validates `codexbar serve /usage` provider
  sets against the discovered canonical inventory before publishing. Any
  mismatch falls back to per-provider CLI data or an empty payload, preventing
  stale disabled providers from rendering after CodexBar config changes.

## [0.2.4] — 2026-06-07

### Added
- `showy-quota --grant-zellij [path]` and `make grant-zellij-permissions`
  can pre-grant the standalone plugin's Zellij permissions by writing
  `permissions.kdl` for the installed plugin path. This is optional setup
  convenience for source/shell users; the release WASM remains standalone and
  can still be used by accepting Zellij's native prompt or adding the documented
  KDL block by hand. The helper is idempotent, preserves other plugins' grants,
  and re-heals after a macOS cache purge. `make install-plugin` now prints the
  command as an optional follow-up.

### Changed
- Zellij plugin permission requests now match enabled runtime paths: `WebAccess`
  is always requested, while `OpenTerminalsOrPlugins` is skipped when
  `manage_serve` is disabled and `RunCommands` is skipped when degraded CLI
  fallback is off.
- Shell cache refreshes now allow local `codexbar serve /usage` requests up to
  10 seconds by default, preventing slow provider collection from being
  misclassified as CLI fallback and shown as `⚠cli` in SketchyBar/Zellij.

## [0.2.3] — 2026-06-05

### Fixed
- `bin/showy-quota-fetch`: avoid Bash here-strings and heredocs in runtime
  fetch paths so Homebrew Bash 5.3 no longer hangs while validating CodexBar
  payloads. This lets SketchyBar refresh the shared cache again instead of
  showing stale warnings while the standalone Zellij plugin remains fresh.

## [0.2.2] — 2026-06-02

### Fixed
- `bin/showy-quota-fetch`: preserve serve-backed last-known-good usage data
  during transient `/usage` failures, retry serve normally while the serve cache
  is active, and fall back to CLI only after repeated serve-unavailable failures.
- SketchyBar: render stale/degraded markers from the same cache snapshot as the
  quota payload, avoiding background-refresh races that could show stale `cli`
  markers.

## [0.2.1] — 2026-06-02

### Added
- `bin/showy-quota --diagnose --json` emits stable machine-readable diagnostics
  (paths, tool availability, config/cache state, env knobs, and a CodexBar
  probe) alongside the existing human-readable output.
- Configurable degraded CLI glyph via the `degraded_cli_glyph` plugin/KDL key
  and `SHOWY_QUOTA_DEGRADED_CLI_GLYPH`.
- `reset_description_timezone_offset` (KDL) /
  `SHOWY_QUOTA_RESET_DESCRIPTION_TIMEZONE_OFFSET` to make local-time
  `resetDescription` countdowns deterministic under WASM, where the host
  timezone cannot be inferred. ISO `resetsAt` timestamps remain preferred.
- Corrupt cache quarantine: an invalid `usage.json` is moved aside as
  `usage.json.corrupt.<epoch>.<pid>` with bounded retention
  (`SHOWY_QUOTA_CORRUPT_CACHE_RETENTION`, default `3`) instead of being
  silently rechecked.
- CI now runs `cargo clippy --workspace --all-targets -- -D warnings` and a
  Linux `cargo audit` dependency advisory scan, plus `cargo fmt --check`.

### Changed
- Moved host-specific SketchyBar, tmux, and Zellij fragments under
  `adapters/` to keep the repository root focused on shared code and metadata.
- Shell renderers no longer block on lock contention when a valid cache already
  exists: non-forced callers emit the current snapshot immediately while the
  lock holder refreshes. Forced refreshes still wait and retry.
- `make doctor` distinguishes `codexbar` CLI vs serve-only data sources and
  reports optional-tool availability without failing valid setups.
- The TPM `showy-quota.tmux` wrapper now preflights `tmux` and verifies the
  renderer is executable before wiring a `#(...)` status command.
- Palette helpers (shell and Rust) accept a leading `#` and normalize hex to
  lowercase.
- Smaller Zellij WASM artifact via a release profile (`lto`,
  `opt-level = "z"`, `codegen-units = 1`, `panic = "abort"`, stripped
  debuginfo).
- SketchyBar icon cache now keys on palette inputs and cleans up temporary
  icon files.

### Fixed
- The Zellij plugin no longer requests a repaint when an event leaves the
  rendered strip unchanged, while still repainting on real output changes and
  synchronous timer-driven state transitions.
- `bin/showy-quota-fetch` closes stdin for spawned CodexBar subprocesses so a
  hung command cannot block on terminal input.
- `crates/showy-quota-zellij-core` reset-description day rollover uses checked
  date arithmetic instead of a panicking add.
- `make install*` targets mark source scripts executable; `make uninstall`
  removes the copied plugin only when it matches the current artifact.

## [0.2.0] — 2026-06-01

### Added
- Standalone `showy-quota-zellij.wasm` plugin for Zellij. It fetches CodexBar
  serve directly, renders the quota strip in-process, and removes the normal
  Zellij dependency on `zjstatus`, feeder loops, and showy-quota shell scripts.
- TPM-compatible `showy-quota.tmux` wrapper that wires the existing tmux
  renderer into `status-right` and can optionally bind the detail popup.
- Rust renderer parity tests that compare plugin output against the existing
  shell Zellij renderer over JSON fixtures.
- `make plugin` / `make install-plugin` and a release workflow that attaches
  the prebuilt Zellij WASM artifact to `v*` releases.
- Provider-aware per-provider fallback in both `bin/showy-quota-fetch` and the
  self-contained Zellij plugin, so individual provider refresh failures no
  longer collapse the whole quota strip.

### Changed
- Zellij docs now make the standalone plugin primary and move the zjstatus pipe
  feeder to the advanced composition path.
- Renamed project from `showy-bar` to `showy-quota`. All binaries, env vars,
  config paths, SketchyBar/zjstatus widget names, cache paths, and docs use
  `showy-quota` / `showy_quota` / `SHOWY_QUOTA` consistently. The Zellij
  pipe widget is now `pipe_showy_quota`, the SketchyBar item prefix is
  `showy_quota`, and the config directory is `~/.config/showy-quota/`. Git
  remote updated to `enieuwy/showy-quota`.
- Provider discovery now uses CodexBar `config providers` as the canonical
  inventory for fallback while preserving `SHOWY_QUOTA_PROVIDERS`,
  `SHOWY_QUOTA_PROVIDERS_EXCLUDE`, and `SHOWY_QUOTA_PROVIDER_ORDER` filters.
- Fallback refreshes now use per-provider backoff and preserve stale cache
  entries when a provider is temporarily unavailable.

### Fixed
- Canonical empty provider inventories now publish an empty usage payload
  instead of being treated as refresh failures.

### Removed
- `showy-quota-zellij-kick` and `showy-quota-zellij-new-tab`; the standalone
  plugin paints on load and timer events, so the recommended path no longer
  needs manual repaint wrappers.

## [0.1.0] — 2026-05-17

### Added
- Initial scaffold: SketchyBar, Zellij, and tmux indicator strips driven by
  `codexbar usage --format json`.
- Shared cache fetcher `bin/showy-quota-fetch` with flock and last-known-good
  fallback.
- Stable state surface `bin/showy-quota-state` for external layout managers.
- SketchyBar `showy_quota_provider_change` event trigger on provider-set changes.
- ANSI strip renderer `bin/showy-quota-zellij-bar`.
- tmux markup renderer `bin/showy-quota-tmux-bar`.
- Long-running zjstatus pipe loop `bin/showy-quota-zellij-pipe`.
- SketchyBar item + plugin scripts that source provider icons from
  CodexBar's bundled SVGs (`/Applications/CodexBar.app/Contents/Resources`).
- `make install` / `make uninstall` symlink installer.
- Smoke tests over JSON fixtures.
- Built-in `default` palette theme matching the original ai-quota/showy-quota
  colors.
- Renamed the former `example` palette to `catppuccin-mocha-blue` and made its
  Catppuccin Mocha colors explicit.
- Renamed countdown and fallback-icon palette knobs to user-facing role names:
  `SHOWY_QUOTA_PALETTE_COUNTDOWN`, `SHOWY_QUOTA_PALETTE_COUNTDOWN_WARN`, and
  `SHOWY_QUOTA_PALETTE_ICON_TEXT`.
- `lib/common.sh`: explicit Bash 4 guard with a friendly error pointing macOS
  users at Homebrew bash.
- `bin/showy-quota --diagnose` / `make diagnose`: prints tool paths, config
  state, cache age, provider state, and active env knobs for bug reports.
- `make doctor`: validates `bash` 4+, `jq`, and `codexbar` before installing.
  `make install` now depends on `doctor`; bypass with `make install-bin` if
  you really must.

### Fixed
- `lib/common.sh`: `showy_quota_log` now treats `SHOWY_QUOTA_DEBUG=0` as off (was
  any non-empty value).
- `bin/showy-quota-zellij-bar`: `SHOWY_QUOTA_FORCE_COLOR=0` no longer force-enables
  color; only `=1` does.
- `bin/showy-quota-zellij-pipe`: validate `SHOWY_QUOTA_ZELLIJ_PIPE_INTERVAL`
  numeric input to avoid feeder-killing `sleep` failures under `set -euo pipefail`.

### Security
- `bin/showy-quota-fetch`: cache dir and files now persist as `0700`/`0600`
  instead of the user's default umask. CodexBar usage JSON stays user-only.

[Unreleased]: https://github.com/enieuwy/showy-quota/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/enieuwy/showy-quota/compare/v0.2.5...v0.3.0
[0.2.5]: https://github.com/enieuwy/showy-quota/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/enieuwy/showy-quota/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/enieuwy/showy-quota/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/enieuwy/showy-quota/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/enieuwy/showy-quota/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/enieuwy/showy-quota/releases/tag/v0.2.0
