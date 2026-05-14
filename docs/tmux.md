# tmux integration

## Output shape

`bin/showy-bar-tmux-bar` emits tmux-format markup, the same per-provider
shape as the Zellij renderer except using `#[fg=#RRGGBB]` / `#[bold]` /
`#[default]` instead of ANSI escape sequences.

When the cache is older than `2 × SHOWY_BAR_REFRESH_SECONDS`, quota colors
remain the last-known values and each countdown is rendered as `?` using
`SHOWY_BAR_PALETTE_COUNTDOWN_WARN`; the secondary `w` hint is suppressed.

## status-right

Append to your existing `status-right` so `showy-bar` cohabits with
whatever else you display:

```tmux
if -F '#{m:*showy-bar-tmux-bar*,#{status-right}}' '' 'set -ag status-right " #(/Users/REPLACE_ME/.local/bin/showy-bar-tmux-bar)"'
```

Use the absolute path to `showy-bar-tmux-bar`; tmux's startup PATH often
does not include `~/.local/bin`. The guard prevents duplicate segments
when `.tmux.conf` is sourced repeatedly.

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

`bind-key '/' display-popup -E -h 36 -w 92 -T "CodexBar usage" 'while :; do clear; codexbar usage; sleep 30; done'`

Hit `<prefix>/` to open. CodexBar's text mode is the detail view.
