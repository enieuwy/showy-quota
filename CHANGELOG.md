# Changelog

All notable changes to `codexbar-bars` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Initial scaffold: SketchyBar, Zellij, and tmux indicator strips driven by
  `codexbar usage --format json --provider all`.
- Shared cache fetcher `bin/cb-bars-fetch` with flock and last-known-good
  fallback.
- ANSI strip renderer `bin/cb-bars-zellij-bar`.
- tmux markup renderer `bin/cb-bars-tmux-bar`.
- Long-running zjstatus pipe loop `bin/cb-bars-zellij-pipe`.
- SketchyBar item + plugin scripts that source provider icons from
  CodexBar's bundled SVGs (`/Applications/CodexBar.app/Contents/Resources`).
- `make install` / `make uninstall` symlink installer.
- Smoke tests over JSON fixtures.
