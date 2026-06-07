# Changelog

All notable changes to `showy-quota` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]
### Changed
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

[Unreleased]: https://github.com/enieuwy/showy-quota/compare/v0.2.3...HEAD
[0.2.3]: https://github.com/enieuwy/showy-quota/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/enieuwy/showy-quota/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/enieuwy/showy-quota/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/enieuwy/showy-quota/releases/tag/v0.2.0
