# tmux integration

## Output shape

`bin/showy-bar-tmux-bar` emits tmux-format markup with the same visible
provider chunks as the Zellij strip:

```text
<SIGIL>▕<12-cell bar body>▏<countdown>
```

Default `SHOWY_BAR_TERMINAL_BAR_MODE=auto` renders each provider according to
its configured terminal body. Time-tier providers use `dual`: upper-half blocks
(`▀`) where foreground is primary and background is secondary, with the
secondary elapsed marker in `SHOWY_BAR_PALETTE_ELAPSED`. Providers listed in
`SHOWY_BAR_MONO3_PROVIDERS` (`gemini,antigravity` by default) use `mono3`:
primary, secondary, and tertiary are top/middle/bottom sextant rows with one
provider-level foreground color and one fixed light `│` pacing separator. The
separator is based on the primary row by default, so the bar body is one
terminal cell wider only when that selected row has a parseable reset/window.

Colors are emitted as tmux `#[fg=#RRGGBB,bg=#RRGGBB]` markup; the markup is
longer than the visible strip but does not consume status-line columns.
`SHOWY_BAR_MONO3_PROVIDERS` opts providers into `mono3` in `auto` mode;
`SHOWY_BAR_MONO3_PROVIDERS_EXCLUDE` wins and forces listed providers back to
`dual`. `SHOWY_BAR_MONO3_COLOR_MODE=lowest` colors `mono3` by the lowest
remaining visible row using the primary palette; set it to `primary` to key off
primary only. `SHOWY_BAR_MONO3_MARKER_SOURCE` selects the one mono3 pacing
separator: `primary` (default), `secondary`, `tertiary`, `shared` (only when at
least two rows share one parseable reset/window), or `none`. Stale snapshots
hide mono3 pacing separators. Set `SHOWY_BAR_TERMINAL_BAR_MODE=dual`, `sextant3`,
or `mono3` to force one body mode for every provider. Forced `sextant3` uses
the same top/middle/bottom geometry as `mono3`, but keeps the bottom-most filled
row as the cell color and omits elapsed markers.

When the cache is older than `2 × SHOWY_BAR_REFRESH_SECONDS`, tmux gets one
trailing `SHOWY_BAR_STALE_GLYPH` (default `⚠`) after the last provider. The
cap glyphs, sigil background, separator, bar fill cells, and countdown
foreground use `SHOWY_BAR_PALETTE_STALE`; sigil letters and the strip
background stay unchanged, and elapsed markers are hidden.

```text
fresh: #[…]CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m
stale: #[…]CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m #[…]⚠   # data-bearing colors greyed
```

## Font requirements

Each provider chunk is wrapped in Powerline-Extra end caps: U+E0B6
(`SHOWY_BAR_CAP_LEFT`, default ``) and U+E0B4 (`SHOWY_BAR_CAP_RIGHT`, default
``). Any Nerd Font ships these; with a non-Nerd font configure a fallback for
the U+E0A0–U+E0D4 range, or set either `SHOWY_BAR_CAP_*` env var to an empty
string for a flat edge.

The `dual` body uses common Unicode Block Elements (`▀`, `▕`, `▏`). `auto`
renders providers in `SHOWY_BAR_MONO3_PROVIDERS` as `mono3`, and forced
`sextant3`/`mono3` bodies require Unicode Symbols for Legacy Computing
U+1FB00–U+1FB3B. If your tmux font cannot render those sextants, use a font with
that range, remove the provider from `SHOWY_BAR_MONO3_PROVIDERS`, or force
`SHOWY_BAR_TERMINAL_BAR_MODE=dual`.


## status-right

Append to your existing `status-right` so `showy-bar` cohabits with
whatever else you display:

```tmux
set -g status-right-length 300
if -F '#{m:*showy-bar-tmux-bar*,#{status-right}}' '' 'set -ag status-right " #(/Users/REPLACE_ME/.local/bin/showy-bar-tmux-bar)"'
```

Use the absolute path to `showy-bar-tmux-bar`; tmux's startup PATH often
does not include `~/.local/bin`. The guard prevents duplicate segments
when `.tmux.conf` is sourced repeatedly.

For a clean standalone preview, remove tmux's default green status styling:

```tmux
set -g status-right-length 300
set -g status-style 'fg=default,bg=default'
set -g status-left ''
set -g window-status-format ''
set -g window-status-current-format ''
```

## Refresh interval

tmux invokes the script on its own schedule (default 15 s). The script
itself reads from the shared cache, so it is fast (≤ 50 ms typical).
With `codexbar serve` running, the shared cache refreshes from the local HTTP
endpoint every `SHOWY_BAR_CODEXBAR_SERVE_REFRESH_SECONDS` by default. Tighten
`SHOWY_BAR_REFRESH_SECONDS` only if you intentionally want the slower CLI
fallback to run more often too.

## PATH gotchas

`#(...)` runs under the tmux server's PATH at startup time, not your
interactive shell's. If `tmux source ~/.tmux.conf` works but a fresh
`tmux` session shows nothing, ensure `~/.local/bin` is on the PATH that
exists when the server starts (e.g. set it in `/etc/launchd.conf` on
macOS, or via `tmux set-environment -g PATH ...`).

## Detail popup

`bind-key '/' display-popup -E -h 36 -w 92 -T "CodexBar usage" 'config="${XDG_CONFIG_HOME:-$HOME/.config}/showy-bar/config.env"; [ -r "$config" ] && . "$config"; while :; do clear; "${SHOWY_BAR_CODEXBAR_BIN:-codexbar}" usage; sleep 30; done'`

Hit `<prefix>/` to open. CodexBar's text mode is the detail view.
