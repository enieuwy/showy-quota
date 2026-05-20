# tmux integration

## Output shape

`bin/showy-bar-tmux-bar` emits tmux-format markup with the same visible
provider chunks as the Zellij strip:

```text
<SIGIL>▕<12-cell primary/secondary half-block bar>▏<countdown>
```

The bar uses upper-half blocks (`▀`) so each full-height terminal cell carries
two windows: foreground is the primary window, background is the secondary
window. The secondary elapsed marker is drawn with `SHOWY_BAR_PALETTE_ELAPSED`
in the lower half. Colors are emitted as tmux `#[fg=#RRGGBB,bg=#RRGGBB]`
markup; the markup is longer than the visible strip but does not consume
status-line columns.

When the cache is older than `2 × SHOWY_BAR_REFRESH_SECONDS`, tmux gets one
trailing `SHOWY_BAR_STALE_GLYPH` (default `⚠`) after the last provider. The
cap glyphs, sigil background, separator, bar fill cells, and countdown
foreground use `SHOWY_BAR_PALETTE_STALE`; sigil letters and the strip
background stay unchanged, and elapsed markers are hidden.

```text
fresh: #[…]CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m
stale: #[…]CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m #[…]⚠   # data-bearing colors greyed
```

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
Tighten the cache TTL with `SHOWY_BAR_REFRESH_SECONDS` if you want the
strip to track provider state more aggressively.

## PATH gotchas

`#(...)` runs under the tmux server's PATH at startup time, not your
interactive shell's. If `tmux source ~/.tmux.conf` works but a fresh
`tmux` session shows nothing, ensure `~/.local/bin` is on the PATH that
exists when the server starts (e.g. set it in `/etc/launchd.conf` on
macOS, or via `tmux set-environment -g PATH ...`).

## Detail popup

`bind-key '/' display-popup -E -h 36 -w 92 -T "CodexBar usage" 'config="${XDG_CONFIG_HOME:-$HOME/.config}/showy-bar/config.env"; [ -r "$config" ] && . "$config"; while :; do clear; "${SHOWY_BAR_CODEXBAR_BIN:-codexbar}" usage; sleep 30; done'`

Hit `<prefix>/` to open. CodexBar's text mode is the detail view.
