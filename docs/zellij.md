# Zellij integration

## Output shape

`bin/showy-bar-zellij-bar` emits a single line of ANSI for the zjstatus
`pipe` widget. Format per provider:

```
<SIGIL>▕<12-cell bar body>▏<countdown>
```

| Segment | Meaning |
|---|---|
| **SIGIL** | 2-letter provider abbreviation (`CL`, `CX`, `GE`, …), rendered in the provider severity color pill. |
| **bar** | In default `auto` mode, time-tier providers render as `dual`: 12 upper-half blocks (`▀`) where foreground is primary/5h and background is secondary/7d, with the secondary elapsed marker in `SHOWY_BAR_PALETTE_ELAPSED`. Providers listed in `SHOWY_BAR_MONO3_PROVIDERS` (`gemini,antigravity` by default) render as `mono3`: primary, secondary, and tertiary are top/middle/bottom sextant rows with one foreground color, plus one provider-level light `│` pacing separator. The separator is based on the primary row by default. |
| **countdown** | Compact like `12m`, `4h`, `4:31`, `2d`, `5w`, or `?` if the provider does not expose a primary reset time. Normal labels use `SHOWY_BAR_PALETTE_COUNTDOWN`; urgent labels use `SHOWY_BAR_PALETTE_COUNTDOWN_WARN`. |

`SHOWY_BAR_MONO3_PROVIDERS` opts providers into `mono3` in `auto` mode;
`SHOWY_BAR_MONO3_PROVIDERS_EXCLUDE` wins and forces listed providers back to
`dual`. `SHOWY_BAR_MONO3_COLOR_MODE=lowest` colors `mono3` by the lowest
remaining visible row using the primary palette; set it to `primary` to key off
primary only. `SHOWY_BAR_MONO3_MARKER_SOURCE` selects the one mono3 pacing
separator: `primary` (default), `secondary`, `tertiary`, `shared` (only when at
least two rows share one parseable reset/window), or `none`. Stale snapshots
hide mono3 pacing separators. Set
`SHOWY_BAR_TERMINAL_BAR_MODE=dual`, `sextant3`, or `mono3` to force one body
mode for every provider. Forced `sextant3` uses the same top/middle/bottom
geometry as `mono3`, but keeps the bottom-most filled row as the cell color and
omits elapsed markers.

When the cache is older than `2 × SHOWY_BAR_REFRESH_SECONDS`, the strip gets
one trailing `SHOWY_BAR_STALE_GLYPH` (default `⚠`) after the last provider.
The cap glyphs, sigil background, separator, bar fill cells, and countdown
foreground switch to `SHOWY_BAR_PALETTE_STALE`; sigil letters and the strip
background stay unchanged, and elapsed reset markers are hidden. Countdown text
keeps its last computed value when the reset timestamp is usable.

```text
fresh: CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m
stale: CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m ⚠   # data-bearing colors greyed
```

## Font requirements

Each provider chunk is wrapped in Powerline-Extra end caps: U+E0B6
(`SHOWY_BAR_CAP_LEFT`, default ``) and U+E0B4
(`SHOWY_BAR_CAP_RIGHT`, default ``). Any Nerd Font ships these;
with a non-Nerd font configure your terminal to fall back to a
Powerline-Extra font for the U+E0A0–U+E0D4 range, or set either
`SHOWY_BAR_CAP_*` env var to an empty string for a flat edge. Common
alternatives are `` / `` (slant) and `` / ``
(flame).

The `dual` body uses only Unicode Block Elements (`▀`, `▕`, `▏`), which every
monospace font carries. `auto` may use `mono3` for model-class providers, and
the forced `sextant3`/`mono3` bodies require a font with Unicode Symbols for
Legacy Computing U+1FB00–U+1FB3B.

## Pipe vs command widget

The pipe widget is more stable than the `command` widget under WASMI,
which crashes when `std::sync::Mutex::new()` runs on a single-threaded
WASM target. The pipe feeder runs as an external background process:

```sh
ZELLIJ_SESSION_NAME=test showy-bar-zellij-pipe
```

Start one feeder for each Zellij session (usually from the terminal wrapper
that launches the session). `ZELLIJ_SESSION_NAME` targets updates at that
session when the feeder runs outside Zellij. It re-emits the strip every
`SHOWY_BAR_ZELLIJ_PIPE_INTERVAL` seconds (default `10`); SketchyBar uses the
same default cadence to avoid visible countdown drift between surfaces.
The feeder does not watch Zellij session metadata or subscribe to tab events.

New tab-local zjstatus instances start with empty pipe state until the next
feeder tick. For immediate paint after creating a tab or plugin, send a
one-shot update:

```sh
ZELLIJ_SESSION_NAME=test showy-bar-zellij-kick
```

For new-tab bindings outside Zellij, prefer the convenience wrapper:

```sh
showy-bar-zellij-new-tab --layout clean-tab
```

That is equivalent to `zellij action new-tab ...` followed by
`showy-bar-zellij-kick`, and keeps the repaint outside Zellij's pane lifecycle.
Avoid a Zellij `Run "showy-bar-zellij-kick"` keybinding for this path: `Run`
opens a transient pane and is visibly slower than invoking the wrapper from
the terminal emulator or session-launching wrapper.

Mode-bound Zellij `NewTab` keys (for example tab-mode `n` or tmux-mode `c`)
cannot trigger an external kick. If immediate paint matters for those paths,
also bind a direct terminal-emulator key to `showy-bar-zellij-new-tab` (or to
`zellij action new-tab ...` followed by `showy-bar-zellij-kick`).

## Layout snippet

See `zellij/layout-pane.kdl.fragment`. Paste the fragment at layout or tab
scope. It includes only the visible widget pane; it no longer includes a
`floating_panes` block because the feeder runs externally.

The recommended setup uses `clean-tab.kdl`, a simple tab layout without
hidden floating panes, for `NewTab` keybindings.

The plugin line assumes `zjstatus.wasm` exists at
`~/.config/zellij/plugins/zjstatus.wasm`; install zjstatus there or edit
the `plugin location=...` path before using the fragment.

## Detail pane

The keybind (`zellij/detail-pane.kdl.fragment`) opens a floating pane. The
pane sources `${XDG_CONFIG_HOME:-$HOME/.config}/showy-bar/config.env` when it
exists, then runs `while :; do clear;
"${SHOWY_BAR_CODEXBAR_BIN:-codexbar}" usage; sleep 30; done`. CodexBar's text
mode is the detail view — there is no custom detail-watch in this repo.
