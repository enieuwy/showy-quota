# tmux integration

## Output shape

`bin/cb-bars-tmux-bar` emits tmux-format markup, the same per-provider
shape as the Zellij renderer except using `#[fg=#RRGGBB]` / `#[bold]` /
`#[default]` instead of ANSI escape sequences.

## status-right

Append to your existing `status-right` so `codexbar-bars` cohabits with
whatever else you display:

```tmux
set -ag status-right ' #(cb-bars-tmux-bar)'
```

`set -ag` (append) matters — `set -g` would drop your other widgets.

## Refresh interval

tmux invokes the script on its own schedule (default 15 s). The script
itself reads from the shared cache, so it is fast (≤ 50 ms typical).
Tighten the cache TTL with `CB_BARS_REFRESH_SECONDS` if you want the
strip to track provider state more aggressively.

## PATH gotchas

`#(...)` runs under the tmux server's PATH at startup time, not your
interactive shell's. If `tmux source ~/.tmux.conf` works but a fresh
`tmux` session shows nothing, ensure `~/.local/bin` is on the PATH that
exists when the server starts (e.g. set it in `/etc/launchd.conf` on
macOS, or via `tmux set-environment -g PATH ...`).

## Detail popup

`bind-key '/' display-popup -E -h 36 -w 92 -T "CodexBar usage" 'watch -n 30 codexbar usage'`

Hit `<prefix>/` to open. CodexBar's text mode is the detail view.
