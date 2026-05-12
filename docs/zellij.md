# Zellij integration

## Output shape

`bin/showy-bar-zellij-bar` emits a single line of ANSI for the zjstatus
`pipe` widget. Format per provider:

```
<SIGIL>▕<12-cell 5h/7d bar>▏<countdown>
```

| Segment | Meaning |
|---|---|
| **SIGIL** | 2-letter provider abbreviation (`CL`, `CX`, `GE`, …), rendered in the primary-window color pill. |
| **bar** | 12 cells of upper-half blocks (`▀`). Foreground is the primary/5h window; background is the secondary/7d window. The secondary elapsed marker is drawn with `SHOWY_BAR_PALETTE_ELAPSED` in the lower half. |
| **countdown** | Compact like `12m`, `4h`, `4:31`, `2d`, `5w`, or `?` if the provider does not expose a primary reset time. Normal labels use `SHOWY_BAR_PALETTE_COUNTDOWN`; urgent labels use `SHOWY_BAR_PALETTE_COUNTDOWN_WARN`. |

When the cache is older than `2 × SHOWY_BAR_REFRESH_SECONDS`, every
provider chunk is dimmed.

## Pipe vs command widget

The pipe widget is more stable than the `command` widget under WASMI,
which crashes when `std::sync::Mutex::new()` runs on a single-threaded
WASM target. The pipe feeder runs as an external background process,
started by the terminal wrapper script with `ZELLIJ_SESSION_NAME` set, and
re-emits the strip every `SHOWY_BAR_ZELLIJ_PIPE_INTERVAL` seconds.

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

The keybind (`zellij/detail-pane.kdl.fragment`) opens a floating pane
running `while :; do clear; codexbar usage; sleep 30; done`. CodexBar's
text mode is the detail view — there is no custom detail-watch in this repo.
