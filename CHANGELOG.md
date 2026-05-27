# Changelog

All notable changes to `showy-quota` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Standalone `showy-quota-zellij.wasm` plugin for Zellij. It fetches CodexBar
  serve directly, renders the quota strip in-process, and removes the normal
  Zellij dependency on `zjstatus`, feeder loops, and showy-quota shell scripts.
- Rust renderer parity tests that compare plugin output against the existing
  shell Zellij renderer over JSON fixtures.
- `make plugin` / `make install-plugin` and a release workflow that attaches
  the prebuilt Zellij WASM artifact to `v*` releases.

### Changed
- Zellij docs now make the standalone plugin primary and move the zjstatus pipe
  feeder to the advanced composition path.
- Renamed project from `showy-bar` to `showy-quota`. All binaries, env vars,
  config paths, SketchyBar/zjstatus widget names, cache paths, and docs use
  `showy-quota` / `showy_quota` / `SHOWY_QUOTA` consistently. The Zellij
  pipe widget is now `pipe_showy_quota`, the SketchyBar item prefix is
  `showy_quota`, and the config directory is `~/.config/showy-quota/`. Git
  remote updated to `enieuwy/showy-quota`.

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
