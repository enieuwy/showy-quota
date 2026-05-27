# Roadmap

## Current Zellij direction

The project now has one recommended Zellij path: a visible standalone Rust/WASM plugin, `showy-quota-zellij.wasm`.

User-facing promise:

- one WASM artifact;
- one borderless pane in the Zellij layout;
- no `zjstatus` dependency;
- no external feeder loop;
- no kick/new-tab wrapper;
- direct CodexBar serve fetch with explicit `WebAccess` permission;
- optional `codexbar` CLI fallback behind `RunCommands`.

The shell path remains supported for tmux, SketchyBar, and advanced zjstatus composition.

## Shipped shape

```text
CodexBar serve /usage
        │
        ▼
showy-quota-zellij.wasm
  ├─ Zellij lifecycle: load / update / render
  ├─ WebAccess request to localhost /usage
  ├─ optional RunCommands fallback to `codexbar usage --format json --pretty`
  ├─ minimal CodexBar JSON model
  ├─ provider filtering/order + terminal geometry/threshold config subset
  ├─ in-memory last-known-good state
  └─ plain terminal strip renderer equivalent to bin/showy-quota-zellij-bar geometry
```

The plugin is intentionally not a full Rust port. It ports only the Zellij terminal renderer and the CodexBar JSON subset required for that renderer. `bin/showy-quota-fetch`, `bin/showy-quota-tmux-bar`, and SketchyBar scripts remain the primary implementations for their surfaces.

## Rejected alternatives

### Headless zjstatus companion plugin

Rejected as the primary path. It would improve first paint for users already committed to zjstatus, but it would still require zjstatus and still depend on tab-local pipe state. The standalone plugin is more valuable to new users.

### Thin shell-out Zellij plugin

Rejected. A plugin that runs `showy-quota-zellij-bar` removes the feeder but still requires users to clone/install the shell scripts. It fails the single-artifact install goal.

### Rust renderer plus existing `showy-quota-fetch`

Rejected as the shipped path for the same reason: it still depends on shell helpers and `jq`. The standalone plugin preserves fetcher semantics where relevant, but not the implementation.

### Full Rust core for all surfaces

Deferred. The terminal renderer is small enough to port, but SketchyBar includes item lifecycle, sliders, brackets, status icon tinting, ImageMagick fallback PNGs, click scripts, provider-set change events, and macOS layout behavior. Porting that now creates more risk than user value.

### Go implementation

Rejected for the plugin. Zellij officially supports Rust plugins; Go/TinyGo would add WASI/API-binding risk. A Go CLI plus Rust plugin would split compiled logic across two languages and make maintenance worse.

### Daemon/bar split plugin

Deferred. A single WASM binary with a background daemon role and visible bar role could reduce per-tab fetches by sharing `/data/latest`, but `/data` is ephemeral and the added coordination is not justified until real overhead is observed.

## Remaining possible improvements

- Publish screenshots and short install clips for the standalone plugin.
- Add a small compatibility matrix if users report success on Zellij versions older than 0.44.3.
- Add active-tab refresh optimization only if per-tab plugin timers prove noisy in real sessions.
- Expand KDL theme examples once users ask for specific palettes in plugin-only installs.
- Consider a daemon/bar split only after measuring unacceptable overhead.

## Design facts to preserve

- Zellij plugins are WASI/WebAssembly panes; Rust is the officially supported plugin language.
- `Timer` events are one-shot. Re-arm after each timer event.
- `web_request` requires `WebAccess` and returns `WebRequestResult`.
- `run_command` requires `RunCommands` and returns `RunCommandResult`.
- `TabUpdate` and `ModeUpdate` require `ReadApplicationState`; the standalone plugin does not need them.
- `MessageAndLaunchOtherPlugins` is only needed for pipe-to-plugin companion designs, not for the standalone plugin.
- Zellij permission prompts can appear in hidden floating panes; install docs must document `permissions.kdl` pre-granting.
- Zellij `/data` is shared between plugin instances but deleted on unload. Do not treat it as durable cache.
