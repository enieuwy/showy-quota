# Roadmap

## A possible community-driven Zellij plugin

showy-bar already works today: install the scripts, add the Zellij layout
fragment, start one feeder process per Zellij session, and the bar shows
CodexBar quota state without asking Zellij for elevated plugin permissions.
That path should remain supported.

A native Zellij plugin is a possible enhancement if there is community interest.
The user-facing promise would be simple: install a single WASM plugin, add one
borderless pane to a layout, and get the same CodexBar quota strip without a
zjstatus dependency, external feeder loop, or manual repaint/kick workflow.

This is worth doing only if it makes the project easier to adopt and share. The
plugin should feel like a normal Zellij component: loadable from a layout or the
plugin manager, documented with screenshots, explicit about its CodexBar data
source, clear about permissions, and graceful when CodexBar is unavailable. It
must not make the existing SketchyBar or tmux integrations worse.

The only plugin paths worth the implementation lift are:

1. **Path 3: Rust-native Zellij plugin using `showy-bar-fetch` as the cache/data
   helper.** The plugin owns Zellij lifecycle, parsing, and rendering, while the
   existing shell helper continues to own CodexBar invocation, cache locking,
   validation, and last-known-good semantics.
2. **Path 4: Shared Rust core plus native Zellij plugin and target adapters.** A
   new Rust core owns schema, filtering, palette, countdown, and render math;
   the Zellij plugin and future adapters reuse it.

Paths that merely polish the current shell/zjstatus setup or wrap the existing
ANSI renderer from a thin plugin are useful experiments, but they do not change
the user story enough to justify another supported integration mode.

## Current baseline

The current Zellij integration is intentionally boring and reliable:

- `bin/showy-bar-fetch` is the data-plane owner. It invokes `codexbar`, validates
  provider JSON, writes the cache atomically, and returns stale-but-valid data
  when a refresh fails.
- `bin/showy-bar-zellij-bar` renders the compact ANSI strip from cached data.
- `bin/showy-bar-zellij-pipe` periodically sends that strip to a zjstatus pipe
  widget.
- `bin/showy-bar-zellij-kick` sends one immediate repaint.
- `bin/showy-bar-zellij-new-tab` wraps `zellij action new-tab` plus the kick.
- `zellij/layout-pane.kdl.fragment` declares the visible zjstatus pipe widget.

The recent cleanup removed metadata polling from the feeder. That leaves a
clean model: periodic feeder plus explicit kicks where the caller can issue them
outside Zellij's transient pane lifecycle.

The remaining friction is adoption and first-paint UX. A new tab's zjstatus
plugin starts with empty pipe state until the next feeder tick or explicit kick.
Zellij does not expose a first-class CLI subscription to `TabUpdate`; passive tab
awareness requires a Zellij plugin that subscribes to application-state events.

## Zellij plugin facts to preserve

These points must be compile-verified against the pinned `zellij-tile` version
before implementation, but they are the design assumptions:

- Zellij plugins are WASI/WebAssembly panes. Rust is the officially supported
  plugin language.
- A plugin can render raw UTF-8 ANSI from `render(rows, cols)` by printing to
  stdout. Zellij clears prior plugin-pane output before each render.
- `Timer`, `Visible`, `RunCommandResult`, and `TabUpdate` events exist in modern
  Zellij.
- `TabUpdate` requires `ReadApplicationState`.
- Host command execution uses `run_command` or
  `run_command_with_env_variables_and_cwd` and requires
  `PermissionType::RunCommands` in the Rust API.
- `RunCommandResult` returns exit code, stdout, stderr, and a context map.
- Workers exist for long synchronous plugin work, but command execution itself is
  already asynchronous through `RunCommandResult`.
- Filesystem access from WASI is path-mapped (`/host`, `/data`, `/tmp`), and
  reading arbitrary home-cache paths may require broader access. Avoid that
  permission in the first design.

## Product and community framing

A Zellij plugin should be marketed around the thing Zellij users get, not around
internal architecture:

- No external feeder loop.
- No zjstatus dependency.
- No new-tab kick wrapper for the native path.
- A loadable WASM pane that fits normal Zellij layouts.
- Clear permission prompt with an explicit command path.
- Same CodexBar multi-provider data users already trust.
- Theme-aware or palette-configurable output.
- Screenshots and copy-pasteable KDL.
- Graceful states for missing CodexBar, denied permissions, empty cache, stale
  cache, and narrow panes.

A good public-facing name can keep repository continuity while explaining the
source:

- Repository: `showy-bar`
- Artifact: `showy-bar-zellij.wasm`
- Documentation title: `CodexBar Quota for Zellij`

Do not pretend this is generic if it depends on CodexBar. A future generic data
source can be added through explicit configuration once users ask for it. The
first plugin should be honest: it renders CodexBar quota data for Zellij.

## Path 3: Rust-native plugin, existing fetch helper

Path 3 is the smallest implementation that produces a genuinely better Zellij
experience without rewriting the data plane.

### Shape

```text
Zellij plugin pane
  ├─ subscribes to Timer / Visible / RunCommandResult
  ├─ optionally subscribes to TabUpdate
  ├─ calls configured showy-bar-fetch via run_command
  ├─ parses CodexBar provider JSON in Rust
  ├─ stores last-good parsed state
  └─ renders the ANSI quota strip directly from render()
```

### Why this path is attractive

- Removes the external Zellij feeder process.
- Removes the zjstatus dependency for users who opt into the plugin.
- Removes pipe backpressure and empty pipe state from the default native path.
- Preserves the cache, lock, validation, and stale-data behavior already proven
  in `showy-bar-fetch`.
- Limits Rust scope to schema parsing, filtering, theme/config reading, and ANSI
  rendering.
- Creates a clean seam for Path 4 later.

### Tradeoffs

- The plugin still needs host command permission.
- The user still needs the shell helper, `jq`, and CodexBar installed.
- Some logic is duplicated initially: provider schema parsing, filtering,
  countdowns, palette roles, and render math.
- Install docs must explain both a WASM artifact and helper scripts.

### Permissions

Default permission request:

```rust
request_permission(&[
    PermissionType::RunCommands,
]);
```

Optional permission request when tab-aware repaint is enabled:

```rust
request_permission(&[
    PermissionType::RunCommands,
    PermissionType::ReadApplicationState,
]);
```

Avoid by default:

- `FullHdAccess`: do not read `~/.cache/showy-bar/usage.json` directly from the
  plugin in the first version.
- `ReadSessionEnvironmentVariables`: prefer explicit `fetch_bin` or
  `codexbar_bin` config instead of implicit PATH discovery.
- `sh -lc`: it broadens the command surface and makes permission UX harder to
  explain. Run an explicit binary path with explicit arguments.

### Plugin lifecycle

#### `load`

Responsibilities:

- Parse plugin configuration.
- Hide cursor.
- `set_selectable(false)` for status-pane behavior.
- Request permissions.
- Subscribe to:
  - `PermissionRequestResult`
  - `RunCommandResult`
  - `Timer`
  - `Visible`
  - optionally `TabUpdate` after `ReadApplicationState` is granted
- Schedule an immediate refresh attempt.
- Render a loading or permission-pending state.

#### Refresh scheduling

The plugin must preserve CodexBar cache discipline. A tab event or pane resize
is a reason to repaint, not a reason to hammer CodexBar.

State fields:

```rust
struct AppState {
    config: Config,
    permission: PermissionState,
    providers: Vec<ProviderUsage>,
    last_good: Option<RenderedState>,
    last_error: Option<PluginError>,
    last_refresh_started: Option<InstantLike>,
    last_refresh_finished: Option<InstantLike>,
    next_allowed_refresh: Option<InstantLike>,
    in_flight_request_id: Option<String>,
    visible: bool,
    stale: bool,
}
```

Pseudo-flow:

```text
refresh(reason):
  if RunCommands not granted:
    record permission error
    render
    return

  if in_flight_request_id exists:
    return

  if now < next_allowed_refresh and reason does not permit cache read:
    render last_good
    return

  request_id = monotonically increasing id
  in_flight_request_id = request_id
  run_command([fetch_bin], context={ request_id, reason })
```

`showy-bar-fetch` already gates CodexBar refreshes. Even so, the plugin should
avoid launching repeated helper processes on every render, resize, or tab event.

#### `RunCommandResult`

On matching request id:

- Clear `in_flight_request_id`.
- If exit status is success and stdout is valid provider JSON:
  - parse providers;
  - apply filter/order config;
  - compute stale status if the helper exposes age or if plugin config includes a
    stale threshold;
  - update `last_good`;
  - set `next_allowed_refresh`.
- If command failed, stdout was empty, or JSON was invalid:
  - preserve `last_good`;
  - set `last_error`;
  - render a degraded state only if no last-good data exists.
- Always schedule the next timer.

#### `Timer`

- If visible and refresh window has elapsed, call `refresh("timer")`.
- Otherwise schedule the next timeout based on `next_allowed_refresh`.
- Do not render if nothing changed.

#### `Visible`

- When visible, render immediately from `last_good`.
- If stale or no state exists, call `refresh("visible")`.
- When hidden, avoid unnecessary command launches.

#### `TabUpdate`

Only when configured and permission is granted:

- Debounce rapid tab changes.
- Render cached state immediately so new plugin panes paint as soon as they
  receive an event.
- Do not bypass `refresh_seconds` or CodexBar cache TTL merely because tab state
  changed.

### Configuration

KDL example:

```kdl
pane size=1 borderless=true {
    plugin location="file:/Users/me/.config/zellij/plugins/showy-bar-zellij.wasm" {
        fetch_bin "/Users/me/.local/bin/showy-bar-fetch"
        refresh_seconds "120"
        providers "codex,claude,gemini"
        providers_exclude ""
        provider_order "codex,claude,opencode,gemini"
        theme "default"
        bar_width "12"
        refresh_on_tab_update "true"
        debug "false"
    }
}
```

Configuration principles:

- Prefer explicit paths over shell lookup.
- Default to the same provider order and thresholds as shell renderers.
- Keep names close to existing `SHOWY_BAR_*` variables.
- Do not read user shell config from inside the plugin.
- Support a minimal config first; add theme/source abstractions only when they
  unblock real users.

### Rendering

The plugin should initially match `bin/showy-bar-zellij-bar`:

```text
<SIGIL>▕<12-cell 5h/7d half-block bar>▏<countdown>
```

Rendering responsibilities:

- Provider sigils.
- Primary/secondary remaining percentages.
- Role colors for good/warn/bad/unknown.
- Secondary elapsed marker.
- Compact countdown labels.
- Stale-cache state: keep quota colors from last-known data, render countdowns
  as `?`, hide elapsed reset markers.
- Width-aware truncation when `cols` is too narrow.
- No trailing newline surprises beyond what Zellij render expects.

Snapshot tests should compare Rust output against `showy-bar-zellij-bar --json`
for the existing fixture set wherever exact parity is expected.

### Path 3 repository layout

```text
showy-bar/
  Cargo.toml
  crates/
    showy-zellij-plugin/
      Cargo.toml
      src/
        main.rs
        config.rs
        events.rs
        parse.rs
        render.rs
        palette.rs
        countdown.rs
      tests/
        fixtures.rs
  bin/
    showy-bar-fetch
    showy-bar-zellij-bar
    showy-bar-zellij-pipe        # legacy path
    showy-bar-zellij-kick        # legacy path
  zellij/
    layout-plugin.kdl.fragment
    layout-pane.kdl.fragment     # legacy zjstatus path
    detail-pane.kdl.fragment
  test/fixtures/
  docs/
    zellij.md
    roadmap.md
```

## Path 4: shared Rust core plus target adapters

Path 4 is the long-term maintainer and community path. It should follow a
successful Path 3 prototype unless the team decides up front that Rust should
own the domain model for every target.

### Shape

```text
showy-core
  ├─ CodexBar provider schema
  ├─ validation and normalization
  ├─ provider include/exclude/order logic
  ├─ reset-time parsing and countdown math
  ├─ role palette derivation
  ├─ stale-state policy
  └─ target-neutral render model

showy-zellij-plugin
  ├─ Zellij permissions/events/config
  ├─ host command refresh
  └─ ANSI pane renderer using showy-core

legacy shell adapters
  ├─ kept stable initially
  └─ optionally replaced or cross-checked later
```

### Why this path is attractive

- Reduces duplicate render and parsing logic across integrations.
- Makes the Zellij plugin feel like a first-class artifact rather than an
  adapter around shell code.
- Allows direct unit tests in Rust against fixture JSON.
- Creates a cleaner release story for a WASM plugin.
- Opens the door to future Rust CLIs for terminal/tmux rendering without making
  SketchyBar absorb plugin-specific concerns.

### Tradeoffs

- Larger refactor.
- More build tooling and CI complexity.
- Requires careful parity tests to avoid changing existing shell behavior.
- Could distract from the current reliable shell integrations if done too early.

### Core boundaries

Good candidates for `showy-core`:

- Provider schema structs.
- Lenient numeric parsing for `usedPercent`.
- Provider ID normalization and safety.
- Provider allow-list/exclude-list/order behavior.
- Built-in provider sigils.
- Reset timestamp parsing.
- Countdown formatting.
- Good/warn/bad/unknown role mapping.
- Palette defaults and theme parsing.
- Terminal ANSI string rendering.
- Stale-cache policy.

Keep target-specific:

- Zellij permission and event lifecycle.
- Zellij pane width handling.
- Zellij KDL configuration.
- SketchyBar item declaration and slider/image behavior.
- tmux format-string escaping.
- Shell install/uninstall behavior.
- CodexBar detail panes.

### Path 4 repository layout

```text
showy-bar/
  Cargo.toml
  crates/
    showy-core/
      Cargo.toml
      src/
        lib.rs
        schema.rs
        provider.rs
        filter.rs
        reset_time.rs
        countdown.rs
        palette.rs
        stale.rs
        render/
          mod.rs
          ansi.rs
          tmux.rs          # optional, later
      tests/
        fixtures.rs
        snapshots.rs
    showy-zellij-plugin/
      Cargo.toml
      src/
        main.rs
        config.rs
        events.rs
        command.rs
        view.rs
    showy-cli/             # optional, later
      Cargo.toml
      src/main.rs
  bin/                     # stable shell entrypoints remain during migration
  lib/                     # shell helpers remain until replaced or frozen
  sketchybar/
  tmux/
  zellij/
    plugin/
      layout-plugin.kdl.fragment
      showy-bar.kdl.example
    legacy-zjstatus/
      layout-pane.kdl.fragment
  share/
    themes/
    config.env.example
  test/
    fixtures/
  docs/
    zellij.md
    zellij-plugin.md       # optional once the plugin exists
    roadmap.md
```

### Split repo vs monorepo

Keep a monorepo unless the plugin develops its own independent user base.

Reasons to keep a monorepo now:

- The fixture JSON belongs to every target.
- The data model is shared: CodexBar provider objects, reset windows, status
  degradation, and provider ordering.
- Shell and Rust outputs need parity tests during migration.
- Splitting early invites schema drift and duplicate bug fixes.
- A single issue tracker makes community feedback easier while the plugin shape
  is still settling.

A split repo becomes attractive only if:

- the Zellij plugin has its own release cadence;
- users install it independently through a Zellij plugin ecosystem;
- the core crate is published and stable enough for reuse;
- SketchyBar/tmux maintenance becomes noise for plugin contributors.

If that happens, split around a published `showy-core` contract, not around a
copy of parsing/render code.

## Migration strategy

Do not remove the current Zellij integration when adding a plugin.

Documentation should present two supported Zellij paths:

1. **Native plugin path** for new users who want the cleanest Zellij experience.
2. **Legacy zjstatus pipe path** for users who prefer the current shell-only
   integration or do not want plugin command permissions.

Migration for plugin users:

- Install or download `showy-bar-zellij.wasm`.
- Keep `showy-bar-fetch` installed for Path 3.
- Add `layout-plugin.kdl.fragment` to Zellij layout.
- Remove the zjstatus pipe pane from that layout.
- Stop launching `showy-bar-zellij-pipe` for that session.
- Stop binding `showy-bar-zellij-new-tab` for immediate paint; normal Zellij
  new-tab actions should paint through plugin lifecycle events.
- Keep the detail pane if desired; it remains CodexBar's text UI.

Legacy users keep:

- `showy-bar-zellij-pipe`
- `showy-bar-zellij-kick`
- `showy-bar-zellij-new-tab`
- `zellij/layout-pane.kdl.fragment`

## Prototype checklist

### Phase 1: compile and render a static fixture

- Add Cargo workspace.
- Add `crates/showy-zellij-plugin`.
- Pin a `zellij-tile` version compatible with Zellij 0.44.x.
- Build a hello-world WASM in CI.
- Verify `PermissionType::RunCommands` spelling.
- Verify `EventType::Timer`, `EventType::Visible`, `EventType::RunCommandResult`,
  `EventType::TabUpdate`, and permission-result handling.
- Embed one fixture JSON.
- Render the existing compact ANSI shape in a borderless pane.
- Verify resize repaint behavior in a live Zellij session.

### Phase 2: command-backed refresh

- Read plugin config.
- Request `RunCommands`.
- Call configured `showy-bar-fetch` with `run_command`.
- Match results by request id.
- Parse stdout.
- Preserve last-good state on bad output.
- Render explicit errors when no last-good state exists.
- Add a `debug` config that logs command duration, parse failures, and stderr.

### Phase 3: timer and visibility behavior

- Subscribe to `Timer` and `Visible`.
- Default refresh interval to the current `SHOWY_BAR_REFRESH_SECONDS` behavior.
- Track `in_flight` so at most one helper command runs at a time.
- Avoid refreshes while hidden unless needed for first paint.
- Repaint from cached state on visibility changes.

### Phase 4: optional tab awareness

- Add `refresh_on_tab_update` config.
- Request `ReadApplicationState` only when enabled.
- Subscribe to `TabUpdate` only after permission is granted.
- Debounce tab updates.
- Repaint promptly from cached state.
- Do not bypass cache TTL on tab events.

### Phase 5: parity and release hardening

- Port fixture tests from `test/render_test.sh` into Rust snapshot tests.
- Compare output with `bin/showy-bar-zellij-bar --json` for fixtures.
- Add CI artifact build for `showy-bar-zellij.wasm`.
- Add KDL snippets and screenshots.
- Add failure-mode tests and documented degraded states.

## Validation matrix

### Data and parsing

- Empty CodexBar array.
- Non-array JSON.
- Missing usage object.
- Missing primary reset time.
- Human reset description.
- Floating or string-like `usedPercent` values if CodexBar emits them.
- Provider status degradation.
- Unknown provider IDs.
- Unsafe provider IDs.
- Include/exclude overlap.
- Provider order changes.

### Cache and refresh

- First launch with no cache.
- Cache exists and is fresh.
- Cache older than `2 × refresh_seconds`.
- `showy-bar-fetch` exits nonzero.
- `showy-bar-fetch` returns empty stdout.
- CodexBar is missing.
- CodexBar hangs or exceeds helper lock wait.
- Multiple plugin instances call refresh at once.

### Zellij lifecycle

- Permission granted.
- `RunCommands` denied.
- `ReadApplicationState` denied.
- Pane hidden then visible.
- Pane resized narrow and wide.
- New tab creation.
- Multiple tabs with plugin panes.
- Session started without Homebrew PATH.
- Plugin hot reload during development.

### Performance

Record at least:

- first paint time;
- warm repaint time from cached state;
- helper command duration;
- JSON parse duration;
- render duration;
- time from new-tab creation to painted plugin pane;
- comparison with the current manual kick baseline.

## Release and packaging

A community-friendly release should include:

- `showy-bar-zellij.wasm` attached to GitHub releases.
- Checksums for the WASM artifact.
- A copy-pasteable `layout-plugin.kdl.fragment`.
- Legacy `layout-pane.kdl.fragment` retained and clearly marked.
- Installation instructions for both source-tree and release-asset users.
- A screenshot in `docs/images/`.
- Permission explanation in plain language:
  - why the plugin asks to run commands;
  - exactly what command it runs by default;
  - how to configure an absolute helper path;
  - what happens if permission is denied.
- Troubleshooting for missing CodexBar, missing helper, stale cache, and PATH.

## Documentation shape if implemented

`docs/zellij.md` should eventually be reorganized as:

```text
# Zellij integration

## Recommended: native plugin
  - what users get
  - install WASM
  - KDL snippet
  - permissions
  - config
  - troubleshooting

## Legacy: zjstatus pipe widget
  - feeder
  - kick
  - new-tab wrapper
  - when to choose it

## Detail pane
  - unchanged CodexBar text UI
```

`README.md` should stay concise: show screenshots, link to Zellij docs, and avoid
making the plugin sound mandatory.

## Decision criteria

Choose Path 3 when:

- community wants a cleaner Zellij installation soon;
- preserving existing cache behavior matters more than eliminating every shell
  dependency;
- the team wants a plugin MVP before reshaping the whole repo;
- the old shell integrations must remain stable.

Choose Path 4 when:

- Zellij adoption becomes a central project goal;
- duplicated render/schema logic starts causing bugs;
- a Rust workspace and CI artifacts are acceptable maintenance overhead;
- the project wants a stronger public API around CodexBar provider data.

Do not proceed with either path unless the plugin can beat the current user
story on adoption clarity. The goal is not Rust for its own sake; the goal is a
shareable Zellij integration people actually want to install.

## Open questions

- Which `zellij-tile` version should be pinned for Zellij 0.44.x compatibility?
- Does the plugin configuration API support all desired keys without awkward
  string parsing?
- Can the release artifact target `wasm32-wasip1`, `wasm32-wasi`, or both?
- What is the smallest acceptable permission prompt for a plugin that shells out
  to `showy-bar-fetch`?
- Should the plugin command default to `showy-bar-fetch` or require an explicit
  absolute path in KDL?
- Should theme selection be plugin-local, inherited from showy-bar config, or
  mapped from Zellij theme colors?
- How much exact byte-for-byte parity with `showy-bar-zellij-bar` is required?
- Would users prefer a one-line pane, replacement status bar, or both?
- Is CodexBar-specific naming better than generic AI quota naming for discovery?
- At what point, if any, should the plugin move to its own repository?
