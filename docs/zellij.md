# Zellij integration

The recommended Zellij integration is the standalone `showy-quota-zellij.wasm` plugin. It owns one borderless status pane, starts/probes `codexbar serve`, renders the terminal strip in-process with ANSI styling, and repaints on load plus a one-shot timer.

It does not use `zjstatus`, does not need a feeder loop, does not require the showy-quota shell scripts to be installed, and does not require users to start `codexbar serve` by hand.

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

Then paste `adapters/zellij/layout-pane.kdl.fragment` into your layout, usually inside `default_tab_template` after `children`:

```kdl
layout {
    default_tab_template {
        children
        pane size=1 borderless=true {
            plugin location="file:~/.config/zellij/plugins/showy-quota-zellij.wasm" {
                serve_url "http://127.0.0.1:8080"
                manage_serve true
                serve_command "codexbar"
                interval_seconds 10
                cli_fallback "degraded"
                cli_command "codexbar"
                cli_interval_seconds 120
            }
        }
    }
}
```

The plugin probes `http://127.0.0.1:8080/health` and `/usage` by default. If `/health` is unavailable, it asks Zellij to start `codexbar serve` in a hidden background command pane on the `serve_url` port. If serve still cannot be reached, the plugin runs `codexbar config providers --format json --pretty` to discover enabled providers, then issues one `codexbar usage --provider <id> --format json --pretty [--status]` per enabled provider. Successful records merge into the in-memory payload; failing or hanging providers stamp a per-provider backoff and keep their previous slice. The merged output is marked with `⚠cli` until serve recovers. The plugin is intentionally self-contained — do not point `cli_command` at `showy-quota-fetch`.

## Permissions

Default configuration permissions:

```text
WebAccess
OpenTerminalsOrPlugins
RunCommands
```

`WebAccess` is always needed for localhost `/health` and `/usage`.
`OpenTerminalsOrPlugins` is requested only when `manage_serve` is enabled,
because that path starts the managed serve background command pane.
`RunCommands` is requested only when degraded CLI fallback is enabled, for
`codexbar config providers --format json --pretty` discovery and one
`codexbar usage --provider <id> --format json --pretty` call per enabled
provider.

Zellij can show permission prompts in a floating pane that is hidden by default.
The grant is keyed on the plugin's path, not the binary, so cutting a release or
rebuilding the `.wasm` never invalidates it — a re-prompt means the grant for
that path is missing (on macOS the permissions cache file can be purged by the
OS, which is the usual "occasional" trigger).

You do not need this repo to silence the prompt. Standalone, pick one:

1. **Accept once.** Reveal floating panes, focus the pending permission pane,
   and press `y`. This is native Zellij and persists until the cache is purged.
2. **Pre-grant by hand.** Add the block below to `permissions.kdl` for the exact
   plugin path used in your layout. Zellij versions differ on whether local file
   plugins are keyed by absolute path, expanded `file:` URL, or the literal
   `file:~` URL from the layout, so include all forms. If your dotfile manager
   installs a renamed artifact such as `showy-quota-zellij-chezmoi.wasm`, use
   that filename in every grant entry.
3. **Request less.** Set `manage_serve false` and `cli_fallback "off"` if you
   run `codexbar serve` yourself; then the plugin only asks for `WebAccess`.

If you already have the repo or the shell tools installed, the bundled helper
just writes the same block for you and is safe to re-run:

```sh
make grant-zellij-permissions
# renamed/relocated artifact:
make grant-zellij-permissions PLUGIN=~/.config/zellij/plugins/showy-quota-zellij-chezmoi.wasm
# or the CLI directly:
showy-quota --grant-zellij [/abs/path/to/plugin.wasm]
```

macOS:

```kdl
// ~/Library/Caches/org.Zellij-Contributors.Zellij/permissions.kdl
"file:~/.config/zellij/plugins/showy-quota-zellij.wasm" {
    WebAccess
    OpenTerminalsOrPlugins
    RunCommands
}
"/Users/you/.config/zellij/plugins/showy-quota-zellij.wasm" {
    WebAccess
    OpenTerminalsOrPlugins
    RunCommands
}
"file:/Users/you/.config/zellij/plugins/showy-quota-zellij.wasm" {
    WebAccess
    OpenTerminalsOrPlugins
    RunCommands
}
```

Linux:

```kdl
// ${XDG_CACHE_HOME:-~/.cache}/zellij/permissions.kdl
"file:~/.config/zellij/plugins/showy-quota-zellij.wasm" {
    WebAccess
    OpenTerminalsOrPlugins
    RunCommands
}
"/home/you/.config/zellij/plugins/showy-quota-zellij.wasm" {
    WebAccess
    OpenTerminalsOrPlugins
    RunCommands
}
"file:/home/you/.config/zellij/plugins/showy-quota-zellij.wasm" {
    WebAccess
    OpenTerminalsOrPlugins
    RunCommands
}
```

If you do not pre-grant, reveal floating panes once, focus the pending permission pane, and accept.

## Output shape

The plugin renders the same styled terminal strip geometry as `bin/showy-quota-zellij-bar`:

```text
<SIGIL>▕<12-cell bar body>▏<countdown>
```

| Segment | Meaning |
|---|---|
| **SIGIL** | 2-letter provider abbreviation (`CL`, `CX`, `GE`, …). |
| **bar** | In default `auto` mode, most providers render as `dual` half-block geometry (`▀`): the primary window on the upper half over the secondary on the lower half, each colored by its remaining-quota severity and dimmed when it is a weekly/monthly cap, with a pacing marker on **both** rows. Providers in `provider_modes` (default `gemini=mono3,antigravity=mono3`) render their mapped body: `mono3` packs three windows into one sextant cell-row, `mono4` packs four into one octant cell-row — each with one provider-level color and `mono_markers` pacing separators. `mono4` needs an octant-capable terminal (Ghostty/kitty/WezTerm). Dimming is horizon-based (`SHOWY_QUOTA_DIM_WINDOW_MINUTES`, default weekly), not row-based. |
| **countdown** | Compact like `12m`, `4h`, `4:31`, `2d`, `5w`, or `?` if the provider does not expose a primary reset time. |

Stale snapshots keep the last-known-good data, hide elapsed markers, grey data-bearing colors, and append the stale glyph (`⚠` by default). CLI fallback appends `⚠cli`; stale and degraded can appear together.

```text
fresh:    CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m
stale:    CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m ⚠
fallback: CL▕▀▀▀▀▀▀▀▀▀▀▀▀▏12m ⚠cli
```

## Plugin configuration

The plugin is standalone and does not read `~/.config/showy-quota/config.env`. Configure it in KDL; see [`docs/plugin.md`](plugin.md) for the full table.

Common options:

```kdl
plugin location="file:~/.config/zellij/plugins/showy-quota-zellij.wasm" {
    serve_url "http://127.0.0.1:8080"
    manage_serve true
    serve_command "codexbar"
    interval_seconds 10
    cli_fallback "degraded"
    cli_command "codexbar"
    cli_interval_seconds 120
    // provider_failure_backoff_seconds 120
    // provider_discovery_backoff_seconds 60
    // reset_description_timezone_offset "-07:00"  // fallback for local-time resetDescription text
    providers ""               // comma-separated allow-list and order
    providers_exclude ""       // comma-separated deny-list
    provider_order "codex,claude,copilot,opencode,gemini"
    bar_width 12
    terminal_bar_mode "auto"   // auto, dual, mono3, mono4
    provider_modes "gemini=mono3,antigravity=mono3"
    mono_color_mode "lowest"   // lowest or primary
    mono_markers "primary"     // comma list of slots; "none" disables
}
```

Palette keys mirror the shell env names without `SHOWY_QUOTA_`, lowercased (`palette_primary_good`, `palette_countdown_warn`, `stale_glyph`, etc.).

`mono3`/`mono4` need enough windows to fill their lanes: `mono4` falls back to `mono3` (3 windows) then `dual` (<3), and `mono3` collapses to `dual` without a tertiary window — so Antigravity's two weekly pools render `dual` until its session windows are also reported. `mono4` additionally needs an octant-capable terminal; run `python3 docs/scripts/preview-quad-octants.py` to test yours before enabling it.

## Font requirements

Each provider chunk is wrapped in Powerline-Extra end caps: U+E0B6 (`cap_left`, default ``) and U+E0B4 (`cap_right`, default ``). Any Nerd Font ships these. With a non-Nerd font, configure terminal fallback for Powerline Extra or set both caps to empty strings.

The `dual` body uses Unicode Block Elements (`▀`, `▕`, `▏`). `mono3` requires Unicode Symbols for Legacy Computing sextants (U+1FB00–U+1FB3B; drawn by most terminals incl. Alacritty/iTerm2). `mono4` requires Unicode 16 octants (U+1CD00–U+1CDE5; drawn only by Ghostty, kitty, WezTerm, and libvte-based terminals).

## Detail pane

The keybind (`adapters/zellij/detail-pane.kdl.fragment`) is unchanged. It opens a floating pane that sources `${XDG_CONFIG_HOME:-$HOME/.config}/showy-quota/config.env` when present, then runs CodexBar's text UI:

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
