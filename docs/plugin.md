# Standalone Zellij plugin

`showy-quota-zellij.wasm` is the recommended Zellij integration. It is a visible one-line Zellij plugin pane that fetches CodexBar usage data directly and renders the quota strip itself with the same ANSI styling as the terminal renderer.

It does **not** require `zjstatus`, `showy-quota-zellij-pipe`, or any installed showy-quota shell scripts.

## Requirements

- Zellij 0.44.3 or newer.
- `codexbar` on the Zellij server `PATH`.
- A font that can render the configured caps and bar glyphs. Any Nerd Font covers the defaults.

The plugin starts `codexbar serve` itself by default. It probes:

```text
http://127.0.0.1:8080/health
http://127.0.0.1:8080/usage
```

If serve cannot be started or reached, the default CLI fallback runs
`codexbar usage --format json --pretty` and marks the strip with `⚠cli`.

## Install prebuilt WASM

Download the release artifact into Zellij's plugin directory:

```sh
mkdir -p ~/.config/zellij/plugins
curl -L \
  -o ~/.config/zellij/plugins/showy-quota-zellij.wasm \
  https://github.com/enieuwy/showy-quota/releases/latest/download/showy-quota-zellij.wasm
```

Then paste `zellij/layout-pane.kdl.fragment` into your layout, usually inside `default_tab_template` after `children`.

## Build from source

```sh
git clone https://github.com/enieuwy/showy-quota
cd showy-quota
rustup target add wasm32-wasip1
make plugin
make install-plugin
```

`make install-plugin` copies the built artifact to:

```text
~/.config/zellij/plugins/showy-quota-zellij.wasm
```

Set `ZELLIJ_PLUGINS=/some/path` to install elsewhere. Set `FORCE=1` to replace an existing different file.

## Layout

```kdl
pane size=1 borderless=true {
    plugin location="file:~/.config/zellij/plugins/showy-quota-zellij.wasm" {
        // Defaults shown.
        serve_url "http://127.0.0.1:8080"
        manage_serve true
        serve_command "codexbar"
        interval_seconds 10
        cli_fallback "degraded"
        cli_command "codexbar"
        cli_interval_seconds 120
    }
}
```

Each tab that includes this pane gets its own plugin instance. That is expected:
every instance probes CodexBar serve on a one-shot timer, asks Zellij to keep a
hidden background command pane for `codexbar serve` if the probe fails, and keeps
its own in-memory last-known-good output.

## Permissions

The plugin requests these permissions up front:

```text
WebAccess
OpenTerminalsOrPlugins
RunCommands
```

`WebAccess` is for localhost `/health` and `/usage`, `OpenTerminalsOrPlugins`
is for the managed `codexbar serve` background command pane, and `RunCommands`
is for the visible degraded CLI fallback.

Zellij can show plugin permission prompts in a hidden floating pane. To avoid a blank pane on first launch, pre-grant permissions. Zellij versions differ on whether file plugins are stored by absolute path, expanded `file:` URL, or the literal `file:~` URL from the layout, so include all forms when editing the file by hand.

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

## Configuration

The plugin does not read `~/.config/showy-quota/config.env`. Configure it in KDL.

Common options:

|KDL key|Default|Effect|
|---|---:|---|
|`serve_url`|`http://127.0.0.1:8080`|CodexBar serve base URL. The plugin requests `/health` and `/usage`.|
|`manage_serve`|`true`|Ask Zellij to start `codexbar serve` in a hidden background command pane when `/health` is unavailable.|
|`serve_command`|`codexbar`|Command used for managed serve startup.|
|`serve_port`|URL port|Port passed to `codexbar serve --port`; defaults to the port in `serve_url` so custom localhost ports stay aligned.|
|`interval_seconds`|`10`|Serve refresh cadence; timers are one-shot and re-armed after each tick.|
|`cli_fallback`|`degraded`|`degraded` or `off`. Degraded fallback appends `⚠cli`.|
|`cli_command`|`codexbar`|Command used for `usage --format json --pretty` fallback.|
|`cli_interval_seconds`|`120`|Slow cadence while fallback is active; the plugin still probes serve every tick and switches back when it recovers.|
|`providers`|empty|Comma-separated allow-list and render order.|
|`providers_exclude`|empty|Comma-separated deny-list.|
|`provider_order`|`codex,claude,opencode,gemini`|Render order when `providers` is empty.|
|`bar_width`|`12`|Cells in each provider bar body. Minimum is 8.|
|`terminal_bar_mode`|`auto`|`auto`, `dual`, `mono3`, or `sextant3`.|
|`mono3_providers`|`gemini,antigravity`|Providers that use `mono3` in `auto` mode.|
|`mono3_marker_source`|`primary`|`primary`, `secondary`, `tertiary`, `shared`, or `none`.|
|`mono3_marker_style`|`replace`|`replace` keeps width fixed; `insert` adds a separator cell.|
|`cap_left` / `cap_right`|`` / ``|Provider chunk end caps; set to empty strings for flat edges.|

Provider, threshold, glyph, geometry, and palette keys use the same names as shell env vars without the `SHOWY_QUOTA_` prefix, lowercased. Example:

```kdl
plugin location="file:~/.config/zellij/plugins/showy-quota-zellij.wasm" {
    serve_url "http://127.0.0.1:8080"
    manage_serve true
    cli_fallback "degraded"
    providers "codex,claude,gemini"
    good_min_remaining 35
    time_warn_minutes 45
    stale_glyph "⚠"
}
```

The shell integrations still use `config.env`; the plugin uses KDL so the WASM artifact remains standalone.

## Failure behavior

|Condition|Pane behavior|
|---|---|
|Permission denied|Shows `showy-quota: permission denied`.|
|CodexBar serve unavailable before first success|Starts `codexbar serve`; if startup fails, uses CLI fallback or shows `showy-quota: CodexBar serve unavailable` when fallback is off.|
|CLI fallback active|Renders fetched quota data with trailing `⚠cli`; continues probing serve and removes the marker after recovery.|
|Serve fails after a success|Keeps rendering last-known-good output; turns stale at `2 × interval_seconds`; falls back only after repeated failures.|
|Serve returns invalid JSON|Keeps last-known-good output; before first success tries the degraded CLI fallback.|
|No renderable providers and no prior data|Renders `AI idle`.|

## Advanced zjstatus path

If you intentionally compose several widgets inside one zjstatus row, keep using `bin/showy-quota-zellij-pipe` and the advanced fragment in `docs/zellij.md`. That path still requires the shell scripts and `zjstatus.wasm`; it is supported but no longer recommended for a normal showy-quota-only Zellij bar.
