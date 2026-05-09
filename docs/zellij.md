# Zellij integration

## Output shape

`bin/cb-bars-zellij-bar` emits a single line of ANSI for the zjstatus
`pipe` widget. Format per provider:

```
<SIGIL> <8-cell-bar> <countdown> [w]
```

- **SIGIL**: 2-letter provider abbreviation (`CL`, `CX`, `GE`, …).
- **bar**: 8 cells of `█`/`░` colored by remaining-percent.
- **countdown**: compact like `12m`, `4h31m`, `2d`, `5w`, or `?` if the
  provider does not expose a primary reset time.
- **w** suffix: shown when the secondary window's remaining-percent is
  worse than primary; colored by the secondary band.

When the cache is older than `2 × CB_BARS_REFRESH_SECONDS`, every
provider chunk is dimmed.

## Pipe vs command widget

The pipe widget is more stable than the `command` widget under WASMI,
which crashes when `std::sync::Mutex::new()` runs on a single-threaded
WASM target. `bin/cb-bars-zellij-pipe` runs as a hidden 1%×1% pane and
re-emits the strip every `CB_BARS_ZELLIJ_PIPE_INTERVAL` seconds.

## Layout snippet

See `zellij/layout-pane.kdl.fragment`. Place the visible widget pane
where you want the strip; place the hidden command pane anywhere
(typically at the end of the layout).

## Detail pane

The keybind (`zellij/detail-pane.kdl.fragment`) opens a floating pane
running `watch -n 30 codexbar usage`. CodexBar's text mode is the detail
view — there is no custom detail-watch in this repo.
