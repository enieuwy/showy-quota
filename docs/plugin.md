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

If serve cannot be started or reached, the plugin discovers enabled providers
through `codexbar config providers --format json --pretty` and falls back to
one `codexbar usage --provider <id> --format json --pretty [--status]` call per
enabled provider. Each provider has its own in-flight flag, last-known-good
slice, and failure backoff, so one hung or failing provider never blocks the
rest. The merged result is marked with `⚠cli`.

## Install prebuilt WASM

Download the release artifact into Zellij's plugin directory:

```sh
mkdir -p ~/.config/zellij/plugins
curl -L \
  -o ~/.config/zellij/plugins/showy-quota-zellij.wasm \
  https://github.com/enieuwy/showy-quota/releases/latest/download/showy-quota-zellij.wasm
```

Then paste `adapters/zellij/layout-pane.kdl.fragment` into your layout, usually inside `default_tab_template` after `children`.

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
        // provider_failure_backoff_seconds 120
        // provider_discovery_backoff_seconds 60
        // build_marker false  // opt-in ⚠ver stale-build warning
    }
}
```

Each tab that includes this pane gets its own plugin instance. That is expected:
every instance probes CodexBar serve on a one-shot timer, asks Zellij to keep a
hidden background command pane for `codexbar serve` if the probe fails, and keeps
its own in-memory last-known-good output.

## Permissions

The default configuration requests these permissions:

```text
WebAccess
OpenTerminalsOrPlugins
RunCommands
```

`WebAccess` is always needed for localhost `/health` and `/usage`.
`OpenTerminalsOrPlugins` is requested only when `manage_serve` is enabled,
because that path starts the managed `codexbar serve` background command pane.
`RunCommands` is requested only when degraded CLI fallback is enabled, for
provider discovery (`codexbar config providers --format json --pretty`) and
one `codexbar usage --provider <id> --format json --pretty` call per enabled
provider.

The grant is **not** invalidated when you cut a new release or rebuild the
`.wasm` — Zellij keys it on the plugin's path, not the binary's contents. A
re-prompt means the grant is missing for that path. On macOS the grant lives in
a cache file that the OS can purge under disk pressure, which is the usual cause
of an "occasional" prompt.

`make install-plugin` pre-grants this for the installed path, so a fresh install
is prompt-free on first launch. The grant is best-effort and never fails the
install. It only covers first launch: a later macOS cache purge still drops the
grant and re-prompts, so re-run `make grant-zellij-permissions` (idempotent) if
the prompt returns. This is an upstream limitation — Zellij stores grants in its
OS cache dir with no relocation override ([zellij#5071](https://github.com/zellij-org/zellij/issues/5071)).

You do not need this repo to silence the prompt. Standalone, pick one:

1. **Accept once.** Reveal floating panes, focus the pending permission pane,
   and press `y`. This is native Zellij and persists until the cache is purged.
2. **Pre-grant by hand.** Add the block below to `permissions.kdl` for the exact
   plugin path used in your layout. Zellij versions differ on whether file
   plugins are keyed by absolute path, expanded `file:` URL, or the literal
   `file:~` URL from the layout, so include all forms. If your dotfile manager
   installs a renamed artifact such as `showy-quota-zellij-chezmoi.wasm`, use
   that filename in every grant entry.
3. **Request less.** Set `manage_serve false` and `cli_fallback "off"` (when you
   run `codexbar serve` yourself); the plugin then only asks for `WebAccess`.

If you already have the repo or the shell tools installed, the bundled helper
just writes the same block for you and is safe to re-run:

```sh
make grant-zellij-permissions
# or, for a renamed/relocated artifact:
make grant-zellij-permissions PLUGIN=~/.config/zellij/plugins/showy-quota-zellij-chezmoi.wasm
# or call the CLI directly:
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

## Upgrading in live sessions

Replacing the `.wasm` on disk does not update running Zellij sessions: each
session caches the compiled module by plugin path, and new tabs reuse that
cached module. Restarting the session always picks up the new build. To
upgrade a live session instead (verified on Zellij 0.44.x):

1. Refresh the session's module cache so new tabs load the new build. A
   plugin `new-pane` with `--skip-plugin-cache` always performs a fresh
   from-disk load and returns its pane id for cleanup:

   ```sh
   pane=$(zellij action new-pane --plugin "file:$HOME/.config/zellij/plugins/showy-quota-zellij.wasm" --skip-plugin-cache --floating --width 60 --height 3)
   zellij action close-pane --pane-id "$pane"
   ```

2. Reload existing strip panes by id with the built-in plugin-manager
   (`zellij action launch-or-focus-plugin plugin-manager --floating`):
   navigate with arrows, `Tab` reloads the selected instance, `Del` closes
   one, `Esc` exits. If its list is empty, open the session-manager once
   (`zellij action launch-or-focus-plugin session-manager --floating`) and
   close it — on 0.44.x the plugin list only populates after a session-list
   query.

Avoid `zellij action start-or-reload-plugin` for strips configured in a
layout: it matches instances by location **and** configuration, its
`--configuration` flag cannot express values containing commas (such as
`provider_order`), and when the server believes a load is already in flight
at that location it silently queues the request and does nothing. Depending
on state it either has no effect or starts a duplicate instance instead of
reloading.

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
|`cli_fallback`|`degraded`|`degraded` or `off`. Degraded fallback appends `degraded_cli_glyph`.|
|`build_marker`|`false`|Opt-in stale-build warning. When `true`, the plugin periodically probes the installed `codexbar --version` and appends `⚠ver` while the running serve's `/health` version differs (detection only — never recycles a session serve). Off by default: the version probe does not run and no marker shows. Needs `cli_fallback` enabled (the probe uses `RunCommands`).|
|`cli_command`|`codexbar`|Command used for provider discovery (`codexbar config providers …`) and per-provider fallback (`codexbar usage --provider <id> …`). Do not point this at `showy-quota-fetch`; the plugin is intentionally self-contained.|
|`cli_interval_seconds`|`120`|Slow cadence while fallback is active; the plugin still probes serve every tick and switches back when it recovers.|
|`provider_failure_backoff_seconds`|`cli_interval_seconds`|Base window to skip a provider after a CLI call fails. The window escalates exponentially per consecutive failure (capped at 30 min), so a provider that keeps wedging is probed rarely.|
|`reset_description_timezone_offset`|`UTC`|Optional fixed offset (`UTC`, `+HH:MM`, or `-HH:MM`) used only when CodexBar provides local-time `resetDescription` text without an ISO `resetsAt`. The WASM plugin cannot infer the host timezone reliably, so set this explicitly if CodexBar emits local-time descriptions and your local zone is not UTC. Prefer ISO timestamps when available; this fallback cannot model DST transitions.|
|`provider_discovery_backoff_seconds`|`60`|How long to skip provider discovery after failure, and how long a successful CodexBar provider inventory is reused before refresh.|
|`providers`|empty|Comma-separated allow-list and render order. When set, it also constrains the per-provider fallback work list.|
|`providers_exclude`|empty|Comma-separated deny-list. Excluded providers are dropped from the per-provider fallback work list before any CLI call.|
|`provider_order`|`codex,claude,copilot,opencode,gemini`|Render order when `providers` is empty. Display order only — not a provider inventory.|
|`bar_width`|`12`|Cells in each provider bar body. Minimum is 8.|
|`terminal_bar_mode`|`auto`|`auto`, `dual`, `dual2`, `mono3`, or `mono4`. `mono4` needs an octant-capable terminal (Ghostty/kitty/WezTerm); `dual2` splits a model-pooled provider into standalone per-pool `dual` widgets (`AGᴳ`/`AGᶜ`, works everywhere).|
|`provider_modes`|`gemini=mono3,cursor=mono3`|Per-provider body in `auto` mode, `provider=mode,…`. Providers without an entry render `dual`, except model pools: a provider whose `extraRateWindows` carry all its positional slots auto-detects as model-pooled and splits into one standalone `dual` per pool (`AGᴳ`/`AGᶜ`); a single pool stays one plain `dual`. Antigravity is the canonical case (OAuth → plain `dual`, the Antigravity IDE → `AGᴳ` + `AGᶜ`). An explicit `provider=dual2` (or `mono4`) forces the pool view and unions positional + extra pools (e.g. Codex + Spark). Pools sharing one billing cycle (same reset+window, e.g. Cursor's Total/Auto/API) stay bright and draw a single pacing marker.|
|`mono_color_mode`|`lowest`|mono3/mono4 chunk color: `lowest` or `primary`.|
|`mono_markers`|`primary`|Comma list of paced window slots (`primary`,`secondary`,`tertiary`,`quaternary`); `none` disables. First marker uses `palette_elapsed`, the rest `palette_elapsed_long`.|
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
    degraded_cli_glyph "⚠cli"
    // reset_description_timezone_offset "-07:00"
}
```

The shell integrations still use `config.env`; the plugin uses KDL so the WASM artifact remains standalone.

## Failure behavior

|Condition|Pane behavior|
|---|---|
|Permission denied|Shows `showy-quota: permission denied`.|
|CodexBar serve unavailable before first success|Starts `codexbar serve`; if startup fails, runs `codexbar config providers` discovery followed by per-provider `codexbar usage --provider <id>` calls. Shows `showy-quota: CodexBar serve unavailable` only when `cli_fallback "off"` is set.|
|CLI fallback active|Merges successful per-provider records into the in-memory payload, marks output with `⚠cli`, and continues probing serve so it can switch back automatically.|
|Serve build behind the installed binary|Opt-in via `build_marker true` (off by default). When enabled and the running serve's `/health` reports a `version` that differs from the installed `codexbar --version`, appends a `⚠ver` marker so you know to restart the serve onto the new build. Detection only — the plugin never kills or recycles a session serve (recycling stays with `showy-quota-fetch`). Requires `cli_fallback` enabled (the on-disk `codexbar --version` probe needs `RunCommands`) and a serve/binary that reports a version (CodexBar invoked by absolute path); otherwise it is a silent no-op.|
|Single provider hangs or errors|A POSIX-sh watchdog terminates the spawned `codexbar usage` after 15s (Zellij has no command-cancel API), so a wedged call — e.g. one blocked on the macOS keychain — cannot leak as a zellij-server child or re-prompt every retry. That provider's slot keeps its previous record, and its backoff escalates so repeated hangs retry rarely. Other providers continue to render normally.|
|Serve fails after a success|Keeps rendering last-known-good output; turns stale at `2 × interval_seconds`; falls back only after repeated failures.|
|Serve returns invalid JSON|Keeps last-known-good output; before first success tries the per-provider degraded fallback.|
|No renderable providers and no prior data|Renders `AI idle`. CodexBar reporting zero enabled providers is the same canonical empty state, not an error.|

## Advanced zjstatus path

If you intentionally compose several widgets inside one zjstatus row, keep using `bin/showy-quota-zellij-pipe` and the advanced fragment in `docs/zellij.md`. That path still requires the shell scripts and `zjstatus.wasm`; it is supported but no longer recommended for a normal showy-quota-only Zellij bar.
