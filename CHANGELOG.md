# Changelog

All notable changes to `showy-bar` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.1.0] — 2026-05-17

### Added
- Initial scaffold: SketchyBar, Zellij, and tmux indicator strips driven by
  `codexbar usage --format json`.
- Shared cache fetcher `bin/showy-bar-fetch` with flock and last-known-good
  fallback.
- Stable state surface `bin/showy-bar-state` for external layout managers.
- SketchyBar `showy_bar_provider_change` event trigger on provider-set changes.
- ANSI strip renderer `bin/showy-bar-zellij-bar`.
- tmux markup renderer `bin/showy-bar-tmux-bar`.
- Long-running zjstatus pipe loop `bin/showy-bar-zellij-pipe`.
- SketchyBar item + plugin scripts that source provider icons from
  CodexBar's bundled SVGs (`/Applications/CodexBar.app/Contents/Resources`).
- `make install` / `make uninstall` symlink installer.
- Smoke tests over JSON fixtures.
- Built-in `default` palette theme matching the original ai-quota/showy-bar
  colors.
- Renamed the former `example` palette to `catppuccin-mocha-blue` and made its
  Catppuccin Mocha colors explicit.
- Renamed countdown and fallback-icon palette knobs to user-facing role names:
  `SHOWY_BAR_PALETTE_COUNTDOWN`, `SHOWY_BAR_PALETTE_COUNTDOWN_WARN`, and
  `SHOWY_BAR_PALETTE_ICON_TEXT`.
- `lib/common.sh`: explicit Bash 4 guard with a friendly error pointing macOS
  users at Homebrew bash.
- `bin/showy-bar --diagnose` / `make diagnose`: prints tool paths, config
  state, cache age, provider state, and active env knobs for bug reports.
- `make doctor`: validates `bash` 4+, `jq`, and `codexbar` before installing.
  `make install` now depends on `doctor`; bypass with `make install-bin` if
  you really must.

### Fixed
- `lib/common.sh`: `showy_bar_log` now treats `SHOWY_BAR_DEBUG=0` as off (was
  any non-empty value).
- `bin/showy-bar-zellij-bar`: `SHOWY_BAR_FORCE_COLOR=0` no longer force-enables
  color; only `=1` does.
- `bin/showy-bar-zellij-pipe`: validate `SHOWY_BAR_ZELLIJ_PIPE_INTERVAL`
  numeric input to avoid feeder-killing `sleep` failures under `set -euo pipefail`.

### Security
- `bin/showy-bar-fetch`: cache dir and files now persist as `0700`/`0600`
  instead of the user's default umask. CodexBar usage JSON stays user-only.
