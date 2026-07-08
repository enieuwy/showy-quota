# tmux integration

## Output shape

`bin/showy-quota-tmux-bar` is a thin shell driver around the native
`showy-quota-render` binary. It emits tmux-format markup with the same visible
provider chunks as the Zellij strip:

```text
<SIGIL>▕<12-cell bar body>▏<countdown>
```

Default `SHOWY_QUOTA_TERMINAL_BAR_MODE=auto` renders each provider according to
its configured terminal body. Most providers use `dual`: upper-half blocks
(`▀`) where foreground is the primary window and background is the secondary,
each colored by its remaining-quota severity and dimmed when it is a
weekly/monthly cap, with a pacing marker on both rows in
`SHOWY_QUOTA_PALETTE_ELAPSED`. Providers listed in
`SHOWY_QUOTA_PROVIDER_MODES` (default `gemini=mono3,cursor=mono3`) render
their mapped body: `mono3` packs three windows into one sextant cell-row;
`mono4` packs four into one octant cell-row. Both use one provider-level
foreground color and the `SHOWY_QUOTA_MONO_MARKERS` pacing separators.

Colors are emitted as tmux `#[fg=#RRGGBB,bg=#RRGGBB]` markup; the markup is
longer than the visible strip but does not consume status-line columns.
`SHOWY_QUOTA_PROVIDER_MODES` maps providers to a body in `auto` mode
(`provider=mode,…`; unmapped providers render `dual`).
`SHOWY_QUOTA_MONO_COLOR_MODE=lowest` (default) colors mono3/mono4 by the lowest
remaining present window using the primary palette; set it to `primary` to key
off the primary window. `SHOWY_QUOTA_MONO_MARKERS` is a comma list of paced
window slots (`primary`, `secondary`, `tertiary`, `quaternary`; default
`primary`; `none` disables); the first marker uses `SHOWY_QUOTA_PALETTE_ELAPSED`,
the rest `SHOWY_QUOTA_PALETTE_ELAPSED_LONG`. Stale snapshots hide pacing
separators. Set `SHOWY_QUOTA_TERMINAL_BAR_MODE=dual`, `dual2`, `mono3`, or
`mono4` to force a terminal body mode; `mono4` needs an octant-capable
terminal (Ghostty/kitty/WezTerm), while `dual2` splits model-pooled providers
into standalone per-pool `dual` widgets (`AGᴳ` + `AGᶜ`) and renders everywhere.
`mono4` collapses to `mono3` (3 windows) then `dual` (<3), and `mono3`
collapses to `dual` without a tertiary window. (Antigravity is auto-split into
`AGᴳ` + `AGᶜ` by pool detection, independent of those collapse rules.)
Providers whose slots share one billing cycle (identical
reset and `windowMinutes`, e.g. Cursor's Total/Auto/API) stay at full brightness
and draw a single pacing marker instead of dimming every row and repeating the
identical one.

When the cache is older than `2 × SHOWY_QUOTA_REFRESH_SECONDS`, tmux gets one
trailing `SHOWY_QUOTA_STALE_GLYPH` (default `⚠`) after the last provider. The
cap glyphs, sigil background, separator, bar fill cells, and countdown
foreground use `SHOWY_QUOTA_PALETTE_STALE`; sigil letters and the strip
background stay unchanged, and elapsed markers are hidden. When the shared cache
came from CLI fallback instead of `codexbar serve`, tmux appends `⚠cli` in the
warning color.

```text
fresh:    #[…]CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m
stale:    #[…]CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m #[…]⚠     # data-bearing colors greyed
fallback: #[…]CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m #[…]⚠cli
```

`make install-copy` installs `showy-quota-render` beside the copied tmux driver
when you use a release tarball, and `make install-bin` builds/links it for
source installs. If you keep a custom renderer build somewhere else, set
`SHOWY_QUOTA_RENDER_BIN=/absolute/path/to/showy-quota-render`; otherwise the
driver looks for a `showy-quota-render` sibling next to the driver script,
then on PATH, then in the repo's `target/release/` directory.

## Font requirements

Each provider chunk is wrapped in Powerline-Extra end caps: U+E0B6
(`SHOWY_QUOTA_CAP_LEFT`, default ``) and U+E0B4 (`SHOWY_QUOTA_CAP_RIGHT`, default
``). Any Nerd Font ships these; with a non-Nerd font configure a fallback for
the U+E0A0–U+E0D4 range, or set either `SHOWY_QUOTA_CAP_*` env var to an empty
string for a flat edge.

The `dual` body uses common Unicode Block Elements (`▀`, `▕`, `▏`). `mono3`
requires Unicode Symbols for Legacy Computing sextants (U+1FB00–U+1FB3B; drawn
by most terminals). `mono4` requires Unicode 16 octants (U+1CD00–U+1CDE5; drawn
only by Ghostty, kitty, WezTerm, and libvte-based terminals). If your tmux
terminal cannot render a body's glyphs, map the provider to a different body via
`SHOWY_QUOTA_PROVIDER_MODES` or force `SHOWY_QUOTA_TERMINAL_BAR_MODE=dual`.


## TPM plugin

TPM users can let the repo wire tmux itself:

```tmux
set -g @plugin 'enieuwy/showy-quota'

# Optional: bind the detail popup. Pick any prefix-relative key you prefer.
set -g @showy-quota-popup-key '/'
```

The root `showy-quota.tmux` file is a thin wrapper around
`bin/showy-quota-tmux-bar`. TPM clones the repo, executes the wrapper, and the
wrapper appends the existing renderer to `status-right`. It does not replace
the renderer or create a tmux-specific data path.

Supported TPM options:

```tmux
set -g @showy-quota-position 'right'          # right, left, or off
set -g @showy-quota-bin '~/.local/bin/showy-quota-tmux-bar'
set -g @showy-quota-status-length '300'
set -g @showy-quota-separator ' '
set -g @showy-quota-popup-key '/'             # empty by default
set -g @showy-quota-popup-height '36'
set -g @showy-quota-popup-width '92'
set -g @showy-quota-popup-interval '30'
```

Set `@showy-quota-popup-key` only if you want the plugin to bind a key; tmux's
default `<prefix>/` binding is otherwise left alone.

## status-right

Append to your existing `status-right` so `showy-quota` cohabits with
whatever else you display:

```tmux
set -g status-right-length 300
if -F '#{m:*showy-quota-tmux-bar*,#{status-right}}' '' 'set -ag status-right " #(/Users/REPLACE_ME/.local/bin/showy-quota-tmux-bar)"'
```

Use the absolute path to `showy-quota-tmux-bar`; tmux's startup PATH often
does not include `~/.local/bin`. The driver also needs `showy-quota-render`
(installed by `make install-copy` from release tarballs, by `make install-bin`
from source, or supplied with
`SHOWY_QUOTA_RENDER_BIN=/absolute/path/to/showy-quota-render`). The guard
prevents duplicate segments when `.tmux.conf` is sourced repeatedly.

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
With `codexbar serve` reachable, the shared cache re-reads `/usage` every
`SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS` (default: half of
`SHOWY_QUOTA_REFRESH_SECONDS`, i.e. 60 s). If serve is missing,
`showy-quota-fetch` starts it when `SHOWY_QUOTA_MANAGE_SERVE=1` (default),
telling it to collect once per `SHOWY_QUOTA_REFRESH_SECONDS`. Tighten
`SHOWY_QUOTA_REFRESH_SECONDS` if you want fresher data; the serve cadences
follow it, and it also bounds how often the slower CLI fallback runs.

## PATH gotchas

`#(...)` runs under the tmux server's PATH at startup time, not your
interactive shell's. If `tmux source ~/.tmux.conf` works but a fresh
`tmux` session shows nothing, ensure `~/.local/bin` is on the PATH that
exists when the server starts (e.g. set it in `/etc/launchd.conf` on
macOS, or via `tmux set-environment -g PATH ...`).
The TPM wrapper now checks that `tmux` is callable and that the configured
`@showy-quota-bin` renderer is executable before appending a `#(...)` status
command. If the renderer path is wrong, it displays a tmux message instead of
installing a broken status segment.

## Detail popup

`bind-key '/' display-popup -E -h 36 -w 92 -T "CodexBar usage" 'config="${XDG_CONFIG_HOME:-$HOME/.config}/showy-quota/config.env"; [ -r "$config" ] && . "$config"; while :; do clear; "${SHOWY_QUOTA_CODEXBAR_BIN:-codexbar}" usage; sleep 30; done'`

Hit `<prefix>/` to open. CodexBar's text mode is the detail view.
