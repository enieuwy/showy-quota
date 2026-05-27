# Standalone Zellij plugin

`showy-quota-zellij.wasm` is the recommended Zellij integration. It is a visible one-line Zellij plugin pane that fetches CodexBar usage data directly and renders the quota strip itself as plain text; ANSI color escapes are intentionally disabled because Zellij plugin panes render escape bytes literally.

It does **not** require `zjstatus`, `showy-quota-zellij-pipe`, or any installed showy-quota shell scripts.

## Requirements

- Zellij 0.44.3 or newer.
- CodexBar with `codexbar serve` running on localhost.
- A font that can render the configured caps and bar glyphs. Any Nerd Font covers the defaults.

Default data source:

```text
http://127.0.0.1:8080/usage
```

The optional CLI fallback needs `codexbar` on the Zellij server `PATH` and adds the `RunCommands` permission.

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
        interval_seconds 10
        fallback_command "" // set to "codexbar" to enable CLI fallback
    }
}
```

Each tab that includes this pane gets its own plugin instance. That is expected: every instance fetches from CodexBar serve on a one-shot timer that re-arms after each tick, and keeps its own in-memory last-known-good output.

## Permissions

The default plugin path requests only:

```text
WebAccess
```

If you set `fallback_command "codexbar"`, it also requests:

```text
RunCommands
```

Zellij can show plugin permission prompts in a hidden floating pane. To avoid a blank pane on first launch, pre-grant permissions. Zellij versions differ on whether file plugins are stored by absolute path or `file:` URL, so include both forms when editing the file by hand.

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

## Configuration

The plugin does not read `~/.config/showy-quota/config.env`. Configure it in KDL.

Common options:

|KDL key|Default|Effect|
|---|---:|---|
|`serve_url`|`http://127.0.0.1:8080`|CodexBar serve base URL. The plugin requests `/usage`.|
|`interval_seconds`|`10`|Refresh cadence; timers are one-shot and re-armed after each tick.|
|`fallback_command`|empty|Set to `codexbar` to run `codexbar usage --format json --pretty` if serve fails.|
|`providers`|empty|Comma-separated allow-list and render order.|
|`providers_exclude`|empty|Comma-separated deny-list.|
|`provider_order`|`codex,claude,opencode,gemini`|Render order when `providers` is empty.|
|`bar_width`|`12`|Cells in each provider bar body. Minimum is 8.|
|`terminal_bar_mode`|`auto`|`auto`, `dual`, `mono3`, or `sextant3`.|
|`mono3_providers`|`gemini,antigravity`|Providers that use `mono3` in `auto` mode.|
|`mono3_marker_source`|`primary`|`primary`, `secondary`, `tertiary`, `shared`, or `none`.|
|`mono3_marker_style`|`replace`|`replace` keeps width fixed; `insert` adds a separator cell.|
|`cap_left` / `cap_right`|`` / ``|Provider chunk end caps; set to empty strings for flat edges.|

Provider, threshold, glyph, and geometry keys use the same names as shell env vars without the `SHOWY_QUOTA_` prefix, lowercased. Palette keys are accepted for config parity but only affect shell/zjstatus color output; the standalone plugin emits plain text. Example:

```kdl
plugin location="file:~/.config/zellij/plugins/showy-quota-zellij.wasm" {
    serve_url "http://127.0.0.1:8080"
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
|CodexBar serve unavailable before first success|Shows `showy-quota: CodexBar serve unavailable`.|
|Serve fails after a success|Keeps rendering last-known-good output; turns stale after `2 × interval_seconds`.|
|Serve returns invalid JSON|Keeps last-known-good output; before first success shows the unavailable message.|
|No renderable providers and no prior data|Renders `AI idle`.|
|Fallback command enabled and serve fails|Runs `codexbar usage --format json --pretty`; successful output becomes last-known-good.|

## Advanced zjstatus path

If you intentionally compose several widgets inside one zjstatus row, keep using `bin/showy-quota-zellij-pipe` and the advanced fragment in `docs/zellij.md`. That path still requires the shell scripts and `zjstatus.wasm`; it is supported but no longer recommended for a normal showy-quota-only Zellij bar.
