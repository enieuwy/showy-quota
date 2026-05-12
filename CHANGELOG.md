# Changelog

All notable changes to `showy-bar` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

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
