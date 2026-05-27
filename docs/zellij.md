# Zellij integration

The recommended Zellij integration is the standalone `showy-quota-zellij.wasm` plugin. It owns one borderless status pane, fetches CodexBar usage through `codexbar serve`, renders the terminal strip in-process without ANSI escapes, and repaints on load plus a one-shot timer.

It does not use `zjstatus`, does not need a feeder loop, and does not require the showy-quota shell scripts to be installed.

## Quick install

```sh
mkdir -p ~/.config/zellij/plugins
curl -L \
  -o ~/.config/zellij/plugins/showy-quota-zellij.wasm \
  https://github.com/enieuwy/showy-quota/releases/latest/download/showy-quota-zellij.wasm
```

Or build from source:

```sh
make plugin
make install-plugin
```

Then paste `zellij/layout-pane.kdl.fragment` into your layout, usually inside `default_tab_template` after `children`:

```kdl
layout {
    default_tab_template {
        children
        pane size=1 borderless=true {
            plugin location="file:~/.config/zellij/plugins/showy-quota-zellij.wasm" {
                serve_url "http://127.0.0.1:8080"
                interval_seconds 10
                fallback_command ""
            }
        }
    }
}
```

Start `codexbar serve` before launching Zellij. The plugin requests `http://127.0.0.1:8080/usage` by default.

## Permissions

Default permission:

```text
WebAccess
```

If `fallback_command "codexbar"` is set, the plugin also requests:

```text
RunCommands
```

Zellij can show permission prompts in a floating pane that is hidden by default. Pre-grant permissions to avoid a blank first launch. Zellij versions differ on whether local file plugins are keyed by absolute path or `file:` URL, so include both forms when editing `permissions.kdl` by hand.

macOS:

```kdl
// ~/Library/Caches/org.Zellij-Contributors.Zellij/permissions.kdl
"/Users/you/.config/zellij/plugins/showy-quota-zellij.wasm" {
    WebAccess
    // RunCommands only if fallback_command is enabled
}
"file:/Users/you/.config/zellij/plugins/showy-quota-zellij.wasm" {
    WebAccess
    // RunCommands only if fallback_command is enabled
}
```

Linux:

```kdl
// ${XDG_CACHE_HOME:-~/.cache}/zellij/permissions.kdl
"/home/you/.config/zellij/plugins/showy-quota-zellij.wasm" {
    WebAccess
    // RunCommands only if fallback_command is enabled
}
"file:/home/you/.config/zellij/plugins/showy-quota-zellij.wasm" {
    WebAccess
    // RunCommands only if fallback_command is enabled
}
```

If you do not pre-grant, reveal floating panes once, focus the pending permission pane, and accept.

## Output shape

The plugin renders the same terminal strip geometry as `bin/showy-quota-zellij-bar`, without ANSI color escapes:

```text
<SIGIL>▕<12-cell bar body>▏<countdown>
```

| Segment | Meaning |
|---|---|
| **SIGIL** | 2-letter provider abbreviation (`CL`, `CX`, `GE`, …). |
| **bar** | In default `auto` mode, time-tier providers render as `dual` half-block geometry (`▀`) for primary/5h over secondary/7d quota. Providers listed in `mono3_providers` (`gemini,antigravity` by default) render as `mono3`: primary, secondary, and tertiary are packed into top/middle/bottom sextant rows, plus one provider-level `│` pacing separator. Color roles still configure shell/zjstatus output; the standalone plugin uses the same thresholds and geometry but emits no color escapes. |
| **countdown** | Compact like `12m`, `4h`, `4:31`, `2d`, `5w`, or `?` if the provider does not expose a primary reset time. |

Stale snapshots keep the last-known-good data, hide elapsed markers, and append the stale glyph (`⚠` by default). The shell and advanced zjstatus renderers additionally grey data-bearing colors; the standalone plugin intentionally emits plain text because Zellij plugin panes render escape bytes literally.

```text
fresh: CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m
stale: CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m ⚠
```

## Plugin configuration

The plugin is standalone and does not read `~/.config/showy-quota/config.env`. Configure it in KDL; see [`docs/plugin.md`](plugin.md) for the full table.

Common options:

```kdl
plugin location="file:~/.config/zellij/plugins/showy-quota-zellij.wasm" {
    serve_url "http://127.0.0.1:8080"
    interval_seconds 10
    fallback_command ""        // set to "codexbar" to enable CLI fallback
    providers ""               // comma-separated allow-list and order
    providers_exclude ""       // comma-separated deny-list
    provider_order "codex,claude,opencode,gemini"
    bar_width 12
    terminal_bar_mode "auto"   // auto, dual, mono3, sextant3
    mono3_providers "gemini,antigravity"
    mono3_marker_source "primary"
    mono3_marker_style "replace"
}
```

Palette keys mirror the shell env names without `SHOWY_QUOTA_`, lowercased (`palette_primary_good`, `palette_countdown_warn`, `stale_glyph`, etc.).

## Font requirements

Each provider chunk is wrapped in Powerline-Extra end caps: U+E0B6 (`cap_left`, default ``) and U+E0B4 (`cap_right`, default ``). Any Nerd Font ships these. With a non-Nerd font, configure terminal fallback for Powerline Extra or set both caps to empty strings.

The `dual` body uses Unicode Block Elements (`▀`, `▕`, `▏`). The `mono3` and `sextant3` bodies require Unicode Symbols for Legacy Computing U+1FB00–U+1FB3B.

## Detail pane

The keybind (`zellij/detail-pane.kdl.fragment`) is unchanged. It opens a floating pane that sources `${XDG_CONFIG_HOME:-$HOME/.config}/showy-quota/config.env` when present, then runs CodexBar's text UI:

```sh
while :; do clear; "${SHOWY_QUOTA_CODEXBAR_BIN:-codexbar}" usage; sleep 30; done
```

That detail view is intentionally separate from the standalone plugin.

## Advanced: embedding in a zjstatus segment list

Use this only when you already own a multi-widget zjstatus row and want showy-quota as one segment beside other zjstatus consumers such as [`zjstatus-hints`](https://github.com/b0o/zjstatus-hints).

This path still requires:

- `zjstatus.wasm`
- installed showy-quota shell scripts
- one `showy-quota-zellij-pipe` feeder per Zellij session

Example zjstatus pane:

```kdl
pane size=1 borderless=true {
    plugin location="file:~/.config/zellij/plugins/zjstatus.wasm" {
        pipe_showy_quota_format      "{output}"
        pipe_showy_quota_rendermode  "raw"
        pipe_zjstatus_hints_format   "{output}"

        format_left  "{pipe_showy_quota}"
        format_right "{pipe_zjstatus_hints}"
        format_center ""
        format_space ""
    }
}
```

Start the feeder for the session:

```sh
ZELLIJ_SESSION_NAME=test showy-quota-zellij-pipe
```

The advanced path is deliberately not the default because every new tab gets a fresh zjstatus plugin instance with empty in-memory pipe state until a feeder tick reaches it. The standalone plugin avoids that class of failure by rendering directly in each tab's pane.

### Permission gotcha for `load_plugins` companions

Companion plugins loaded via `load_plugins` can also prompt in hidden floating panes. For `zjstatus-hints`, pre-grant the permissions it requests:

```kdl
"/Users/you/.config/zellij/plugins/zjstatus-hints.wasm" {
    ReadApplicationState
    MessageAndLaunchOtherPlugins
}
```

This is separate from showy-quota's standalone plugin permission grant.
