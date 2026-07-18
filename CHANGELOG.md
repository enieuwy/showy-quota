# Changelog

All notable changes to `showy-quota` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Security
- Config/theme `.env` files are now dot-sourced only when the resolved target is
  a regular file owned by the current user and not writable by group or other
  (SSH-style). Symlinks are still followed (chezmoi-style deployment keeps
  working), but a foreign-owned or group/world-writable config can no longer
  inject shell code; the file is skipped with a stderr warning.
- `showy-quota-fetch` refuses to publish into a cache dir that is not private
  (foreign-owned or group/other-writable), closing a local cache-poisoning path
  into `usage.json`/`status.url`. The SketchyBar image cache dir is created
  `0700`.
- The standalone Zellij plugin's loopback-serve URL check parses the authority
  before `?`/`#`, so `http://evil.com?@localhost` no longer bypasses the
  loopback guard (SSRF).
- The plugin keeps `permissions_granted` false until `PermissionRequestResult`,
  so no WebAccess/RunCommands/OpenTerminals work runs during the consent prompt.
  `--grant-zellij` now emits the minimal permission set (WebAccess only unless
  `--manage-serve`/`--cli-fallback` opt in) and refuses to write permissions for
  an absent `.wasm` without `--force`.
- `SHOWY_QUOTA_SKETCHYBAR_CLICK`, cached CodexBar `status.url`, and
  `SHOWY_QUOTA_CODEXBAR_RESOURCES` are validated before reaching `sh -c`,
  `open`, or ImageMagick; the agent-CLI statusline routes `SHOWY_QUOTA_BAR_BIN`
  through `showy_quota_valid_bin`; `SHOWY_QUOTA_ZELLIJ_PLUGIN` is validated
  before `zellij pipe --plugin`.
- Provider error messages are redacted (emails, token/cookie/Authorization
  fragments, `$HOME` username) before appearing in `showy-quota-state` JSON and
  `--diagnose` output that users paste into bug reports.
- A shared 5 MiB usage-JSON cap now bounds the CLI-fallback capture, cache
  validation, and the native `parse_usage_payload`, matching the existing
  `/usage` HTTP cap. CI runs `cargo audit` on every matrix leg and before every
  release tarball build.

### Fixed
- Cache publish renames the `usage.json` payload last (after stamp/source) so a
  reader that observes a new payload always sees matching-or-newer metadata,
  closing the mixed-generation window that could pair fresh data with a stale
  source marker. The managed-serve pidfile is now written via temp-file +
  atomic rename.
- The shell provider-id validators and the Rust `valid_provider_id` both cap ids
  at 64 characters; `showy_quota_json_valid` and the serve publish gate validate
  `extraRateWindows`. Shell reset-description parsing now honors
  `SHOWY_QUOTA_RESET_DESCRIPTION_TIMEZONE_OFFSET`, matching the native renderer.
- `elapsed_marker_cell` widens to `i128` before subtraction, avoiding overflow
  at extreme `SHOWY_QUOTA_NOW_EPOCH`. The managed `codexbar` binary is
  canonicalized (basename-preserving) before launch.
- Zellij plugin: version-probe in-flight guard resets on permission transition
  and expires stale, provider fallback rejects orphaned/pruned callbacks,
  permission re-grant clears provider backoff, the eligible-provider inventory
  de-duplicates, and watchdog/version-probe subshells reap their `sleep`
  children. The `showy-quota-zellij-pipe` feeder traps teardown, backs off on
  repeated pipe failures, and validates the plugin name.
- `scripts/check_plugin_exports.py` no longer relies on `assert` (bypassed under
  `python -O`). `make install-plugin` verifies a `.sha256` sidecar when present;
  `make uninstall FORCE_PLUGIN_REMOVE=1` removes an orphaned plugin. The README
  release-tarball install documents checksum verification.

## [0.6.0] — 2026-07-13

### Changed
- Serve cadence now derives from the freshness contract instead of
  oversampling it. A managed `codexbar serve` starts with `--refresh-interval`
  equal to `SHOWY_QUOTA_REFRESH_SECONDS` (was a fixed 60s), and the `/usage`
  poll gate `SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS` defaults to half the
  contract (60s with stock config; was a fixed 10s). Serve only re-collects
  once per its refresh interval, so the old 10s poll mostly re-downloaded
  identical JSON — the new defaults cut steady-state probe work ~6x and
  collection wakeups 2x while keeping worst-case displayed data age at
  ~1.5x the contract, inside the 2x stale horizon. The standalone Zellij
  plugin follows suit: `interval_seconds` defaults to 60 (was 10) and its
  managed serve collects every 120s. Explicit overrides win everywhere;
  `SHOWY_QUOTA_MANAGE_SERVE=1` stays the default (a resident serve does the
  same collection work as the CLI fallback minus a process launch per
  refresh, and degrades per provider).

- The SketchyBar plugin's per-tick compute moves into the native renderer:
  `showy-quota-render` gains `--emit sketchybar`, emitting final per-provider
  row fields (remaining percentages, elapsed-marker positions, countdown
  labels, resolved colors, stale/degraded and shared-cycle handling) from the
  shared cache. The shell plugin now only manages SketchyBar items, icons,
  and click scripts — the render tick runs no `jq` or `date` (was 3 jq plus
  per-provider `date` spawns every `SHOWY_QUOTA_SKETCHYBAR_UPDATE_FREQ`).
  Row semantics are unchanged (verified by the full shell suite, including
  pooled lanes, `usageKnown:false` placeholders, shared-cycle marker
  suppression, and pinned-clock marker positions); the plugin now requires
  the `showy-quota-render` binary like the tmux/Zellij drivers already do,
  and clears its items with a hint when it is missing.

- The bar drivers and `showy-quota prompt` collapse their hot path into the
  native renderer: `showy-quota-render` gains `--from-cache` (reads the cache
  payload/source/mtime and computes stale/degraded itself) and `--emit prompt`
  (the worst-remaining prompt segment). The shell drivers now fetch to warm the
  cache (cold path, unchanged) then render in one binary call with no shell
  `jq`/`date` on the render path, and `showy-quota prompt` is a single native
  invocation (was ~20 process spawns incl. 8 `jq`; now 0 `jq`/`date`). Output
  is byte-identical across all fixtures and freshness states. This also removes
  the last shell `date` reset-parsing from the render/prompt paths (no BSD/GNU
  divergence) and matches the AGENTS.md scope: Rust owns the hot path, shell
  owns cold fetch. `SHOWY_QUOTA_DEGRADED_CLI` remains tri-state (`1` on, other
  non-empty off, unset derives from source).

- `showy-quota-state` now computes `providerMetrics` via the native renderer
  (`showy-quota-render --emit metrics`) instead of shell jq: the per-provider
  window math, reset-time parsing, and minutes-until-reset move into the Rust
  core (shared with the strip renderer), so the output is deterministic across
  platforms with no BSD/GNU `date` divergence. The `providerMetrics` schema is
  byte-identical (verified across all fixtures), with one intentional
  tightening: provider ids are validated strictly (`.`, `..`, and leading-dash
  rejected), matching the rest of the pipeline. `guard` and `prompt` consume
  the same JSON and are unchanged. The render-bin resolver and config export
  are now shared helpers in `lib/common.sh` used by the drivers and state.

### Security
- Hardening pass across the shell and Rust surfaces: cap glyphs now go through
  the glyph validator (rejecting NUL/C0/C1/DEL bytes) and tmux glyph output is
  `#`-escaped, closing terminal-escape/`#(...)` injection via renderer config;
  the tmux wrapper escapes `@showy-quota-separator`/`@showy-quota-popup-title`
  and rejects a `@showy-quota-bin` containing shell metacharacters; provider-id
  validation is now consistent everywhere (rejecting `.`, `..`, and leading-dash
  ids) in `showy_quota_json_valid`, `showy_quota_filter_renderable`, the serve
  payload validators, and the core predicate; SketchyBar refuses to `open` a
  `status.url` on a loopback/link-local/non-http(s) host; config/theme files are
  sourced only when regular (a readable FIFO no longer blocks); `config.env`,
  the cache dir, and the serve pidfile are created with restrictive modes by
  construction; the `/usage` fetch is bounded with `--max-filesize`; the
  `timeout` fallback escalates with `--kill-after`; the release workflow runs
  `cargo audit` before publishing artifacts; and the documented plugin install
  verifies the published `.sha256`.

### Fixed
- `showy_quota_uint` and the palette dim-scale parser clamp oversized/overflowing
  values instead of wrapping negative; the native renderer colors pacing markers
  by visible rank and includes the fourth mono4 lane in shared-cycle detection
  (mirrored in SketchyBar); `showy_quota_reset_description_epoch` and the
  SketchyBar elapsed marker honor `SHOWY_QUOTA_NOW_EPOCH`; shell serve port
  precedence now matches the plugin (explicit port, then URL, then default); the
  mkdir-lock ownerless window and the zjstatus pipe truncation (partial
  UTF-8/ANSI) are closed.
- The Zellij plugin rejects late per-provider CLI results whose attempt token
  no longer matches the live attempt, using the same strict rule as discovery.
  Previously a delayed result from a superseded attempt (its token cleared by
  a permission re-grant while the command was still running) was accepted
  whenever the provider had no recorded failure, letting stale data overwrite
  a newer successful record until the next refresh.
- Codex renders correctly after OpenAI temporarily removed the 5-hour limit
  (`usage.primary: null`). When the primary slot is absent, the renderers now
  left-compact the present windows so the live weekly cap drives the primary
  row and its reset countdown instead of an empty top row and an `idle` label —
  applied identically on the terminal bars and SketchyBar (previously the two
  diverged: SketchyBar promoted the weekly and showed the countdown while the
  tmux/Zellij bars left the top row empty with `idle`). Auto model-pool
  detection also no longer fires on a coincidental `windowMinutes`/`resetsAt`
  collision between a positional slot and a single extra (Codex's main weekly
  vs its Spark weekly); pooling now requires the extras to carry more pools
  than the positional slots expose. A provider left with a single live window
  (like Codex now) renders as one full-height bar — a solid `█` body in the
  terminals, one centered native row on SketchyBar — instead of a half-filled
  bar with an empty second row. Slot promotion is render-only;
  `showy-quota-state` still reports the raw positional windows. An unused 5h
  window (`usedPercent: 0`, not `null`) still renders its own full row.

### Added
- `showy-quota guard`: scriptable quota gate for automation (CI pre-flight,
  agent hooks, cron). Selects providers/windows (`--provider`, `--window
  primary|secondary|tertiary|worst`), evaluates `--min-remaining`/`--max-used`
  thresholds with a strict-breach/inclusive-pass boundary, and exits with a
  stable code: 0 pass, 1 breach, 2 unusable data (stale without
  `--allow-stale`, unknown/errored provider, empty cache), 3 usage error.
  `--json` emits a machine-readable verdict; `--wait-max` sleeps through a
  known reset and re-evaluates once. See `docs/automation.md`.
- `showy-quota prompt`: one-segment quota readout for shell prompts
  (`CX 59% 7d` — worst-remaining provider/window), cache-only by default so
  a prompt never blocks on CodexBar collection (`SHOWY_QUOTA_PROMPT_FETCH=1`
  opts into refreshing); `--ansi` severity color honoring `NO_COLOR`.
  Starship, powerlevel10k, and plain-PS1 snippets in `docs/automation.md`.
- `showy-quota-state --no-fetch`: additive flag reporting the cache as-is
  (no refresh-window fetch) for hot-path consumers.
- Agent-CLI statusline adapter (`adapters/agent-cli/showy-quota-statusline`):
  renders the quota strip inside Claude Code (`statusLine` command) and any
  tool that displays a command's one-line ANSI output. Rounded Powerline
  caps are inherited like every other strip (`SHOWY_QUOTA_STATUSLINE_CAPS=0`
  drops them on plain-font hosts), width defaults to 8 cells via
  `SHOWY_QUOTA_STATUSLINE_WIDTH`. See `docs/statusline.md`.
- `make ci-gates`: runs every CI gate locally (lint, shell suite, rustfmt,
  clippy `-D warnings`, workspace tests, cargo audit, plugin build, WASM
  export check) — run before tagging a release. The WASM export check moved
  to `scripts/check_plugin_exports.py`, shared by CI and the make target.

## [0.5.0] — 2026-07-07

### Added
- The bar drivers resolve `showy-quota-render` as a sibling of the driver
  script before consulting PATH or the repo `target/release` build, so
  copied installs and release tarballs work hermetically without PATH
  setup; the `SHOWY_QUOTA_RENDER_BIN` override is now documented in
  `config.env.example`.
- Native `showy-quota-render` binary (built from the core crate): renders the
  terminal strip from CodexBar JSON on stdin or `--json <path|->` in zellij
  ANSI (`--format zellij`, default) or tmux markup (`--format tmux`), takes
  its configuration from the same `SHOWY_QUOTA_*` environment variables the
  shell uses, and honors `SHOWY_QUOTA_NOW_EPOCH` for deterministic output.
  `make render-bin` builds it; `make install-bin` installs it.
- `showy-quota-state --json` grows a `providerMetrics` array: normalized
  per-provider positional windows and `extraRateWindows` with `usedPercent`,
  `remainingPercent`, `resetsAt`, `resetDescription`, `windowMinutes`, and
  `minutesUntilReset`. The stable `providers` id array is unchanged.
- Provider errors are surfaced instead of hidden: errored providers render a
  compact `⚠err` chunk (new `SHOWY_QUOTA_ERROR_GLYPH`/`error_glyph` knob) in
  the zellij/tmux strips, all-error payloads are no longer rendered as
  `AI idle`, `providerMetrics` entries carry a sanitized
  `error {kind, message}` (kind bucketed auth/cookies/network/unknown,
  message stripped and truncated to 160 chars), and `make diagnose` prints
  per-provider error lines. Raw provider messages never reach the strip.
- Per-platform release tarballs (`macos-arm64`, `macos-x86_64`,
  `linux-x86_64`) packaging the full runtime tree plus the prebuilt native
  renderer, with sha256 sidecars and a CI smoke job that installs each
  tarball into a temp prefix and renders a fixture without the checkout.
- `make install-copy` / `make install-copy-sketchybar`: checkout-independent
  copied install into `DATA_DIR` (default `PREFIX/share/showy-quota`) with
  `BIN_DIR` links into the copied tree; `make uninstall` cleans both modes.

### Changed
- `bin/showy-quota-zellij-bar` and `bin/showy-quota-tmux-bar` are now thin
  drivers: fetch, stale/degraded detection, and the `AI ?` fallback stay in
  shell while rendering delegates to `showy-quota-render` (resolution:
  `SHOWY_QUOTA_RENDER_BIN`, then `PATH`, then the repo `target/release`;
  a missing binary degrades to `AI ?` with a `make render-bin` hint). The
  tmux driver gains `--json <path|->`. Byte parity with the retired shell
  renderers was verified over a 2,814-case fixture/env sweep before cutover.

### Removed
- The duplicated shell strip renderers: `lib/strip.sh` keeps only shared
  data helpers with live callers; the Rust-vs-shell `shell_parity.rs`
  harness is replaced by `render_cli.rs`, which pins the CLI's env parsing
  and flag wiring against the in-process renderer.

### Fixed
- The core crate's provider discovery now matches the shell fetcher's strict
  contract: any enabled inventory record with an invalid provider id is a
  discovery failure (`ProviderConfigError::InvalidInventory`) instead of
  being silently dropped when other records are valid, and the core
  `valid_provider_id` rejects the path components `.` and `..` like the
  shell predicate does.
- Shell pacing-marker math (`showy_quota_elapsed_marker_cell` in `lib/strip.sh`
  and `elapsed_marker_x` in the SketchyBar plugin) no longer divides by zero
  when an absurd `windowMinutes` (e.g. `2^62`) wraps 64-bit arithmetic to a
  zero duration. The wrap is detected and no marker is drawn, mirroring the
  Rust core's `checked_mul`.
- `showy_quota_age_seconds` now reports the absolute distance from the file
  mtime, so a future-dated cache (clock skew, restored backup) ages out and
  triggers refresh/stale handling instead of being treated as pinned-fresh
  forever.
- The SketchyBar render lock recovers ownerless `render.lock` directories with
  a future-dated mtime; previously the negative age never crossed the
  ownerless threshold and rendering wedged until the lock was removed by hand.
- `discover_providers` validates every discovered provider id with the shell
  `valid_provider_id` predicate, so path-component ids (`.`, `..`) are
  rejected as invalid inventory instead of being counted valid — an all-dot
  inventory can no longer clear the cache to a canonical-empty `[]`.
  Payload-derived provider ids get the same `.`/`..` rejection.
- Stale-serve recycling only signals port listeners it can verify as CodexBar
  serve processes (basename + `serve` argument check via `ps`), and the blind
  `pkill -f 'codexbar serve'` fallback is removed: an unrelated service on the
  configured port — or another session's serve on a different port — is never
  killed. When no listener can be verified, the stale serve is reused instead.

## [0.4.1] — 2026-07-03

### Fixed
- `bin/showy-quota-fetch` no longer leaks the `flock` lock descriptor (fd 9)
  into the managed `codexbar serve` daemon. `start_managed_serve` runs under
  `showy_quota_with_lock`, and `setsid`/`nohup` detach the session but still
  inherit open descriptors, so a serve started while the lock was held kept fd 9
  open for its whole life and pinned the advisory lock — every later fetch then
  timed out on `flock -n` and fell back to stale cache (most visibly, the
  build-version gate could never recycle a stale serve). The daemon now closes
  fd 9 (`9>&-`). Only `flock` hosts (Linux) were affected; the `mkdir` lock
  fallback (e.g. macOS without `flock`) has no descriptor to leak.

## [0.4.0] — 2026-07-03

### Added
- `SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS` (default `30`) gives the
  `/usage` probe its own budget, separate from the `/health` probe's
  `SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS` (default `10`). A healthy
  `codexbar serve` bounds collection per provider and can take up to ~0.8x its
  request deadline (~24s by default) to return the healthy providers when a slow
  one degrades to an error row; the short health timeout previously abandoned
  that usable partial response and dropped the bar to degraded CLI output
  whenever any provider was briefly slow. This relies on CodexBar's per-provider
  serve bounding, landed via
  [steipete/CodexBar#1748](https://github.com/steipete/CodexBar/pull/1748).
- The standalone Zellij plugin now expires a hung `/health` probe on a short
  10s window while giving the `/usage` probe a 30s budget, mirroring the shell
  fetcher's split. Previously both shared one 30s watchdog, so an unreachable
  serve latched for 30s before the plugin fell back. Both surfaces now use the
  same fast-health / patient-usage model.
- The standalone Zellij plugin now staggers its degraded CLI fallback across
  instances. On a serve outage, each per-tab plugin instance waits a distinct,
  stable per-instance hold (`fallback_jitter_seconds`, default
  `min(cli_interval_seconds, 60)`) — re-probing serve on a short cadence — before
  spawning the first `codexbar usage --provider <id>` call. The offset is derived
  from the Zellij plugin id, so N tabs no longer stampede `codexbar` at the same
  instant when serve blips; because a fast managed-serve restart is usually seen
  during the hold, most instances return to the HTTP path and spawn no CLI work
  at all. Set `fallback_jitter_seconds 0` to restore the previous immediate
  fallback.

### Security
- The bundled ImageMagick policy now denies all external delegates
  (`delegate` domain, pattern `*`), and the SketchyBar plugin rasterizes
  provider SVGs through the internal `MSVG:` decoder. Previously only the
  URL/HTTP/HTTPS/FTP/MSL coders were denied, so a delegate-based SVG path
  (librsvg/inkscape) could still resolve external references from a hostile
  SVG — the SSRF/exfiltration hole the policy was meant to close.
- The standalone Zellij plugin now validates `stale_glyph` and
  `degraded_cli_glyph` from KDL/env config with the same rules as the shell
  renderers (no control characters, length-capped); invalid glyphs keep the
  defaults instead of flowing into raw ANSI output.
- `SHOWY_QUOTA_ZELLIJ_WIDGET` and `SHOWY_QUOTA_ZELLIJ_PIPE_NAME` are now
  length-capped (128 bytes); an overlong identifier previously bypassed the
  4096-byte pipe payload cap via the `zjstatus::pipe::<widget>::` prefix.
- `SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS` and
  `SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS` now reject non-positive
  values and fall back to their defaults; `0` previously reached
  `curl --max-time 0`, which disables the transfer timeout entirely and
  reintroduced the unbounded `/usage` probe.

### Fixed
- The SketchyBar plugin no longer drops a model-pooled provider's whole family
  when CodexBar transiently marks one family's windows `usageKnown:false` (e.g.
  Antigravity's Claude/GPT pool during a collection hiccup). The pooled layout
  previously rendered those placeholder windows as absent rows, so the
  `has_t`/`has_q` lane gate collapsed and the bar dropped to just the measured
  (Gemini) lanes. Such a window now keeps its lane drawn as an empty track (no
  pacing marker), so a transiently-thin family stays visible — matching the
  Zellij renderer, which keeps the `AGᶜ` lane.
- Forced `mono4` again reaches the documented four-lane path: a non-pooled
  provider with three positional slots plus a distinct `extraRateWindows`
  entry renders four lanes instead of collapsing to `mono3`, and a
  three-window shape under forced `mono4` falls back to `mono3` instead of
  drawing an empty fourth row. Mode collapse and the `mono4` body are now
  both gated on the distinct assembled render-window count, in the Rust core
  and both shell renderers.
- The standalone Zellij plugin now generation-tags `/health` and `/usage`
  web requests and drops responses from expired probes; previously a
  timed-out old `/usage` response could clear the in-flight flag for a newer
  request and surface stale data.
- The standalone Zellij plugin now schedules its short re-probe timer when
  arming the degraded CLI-fallback hold; with a long `interval_seconds` the
  promised fast serve re-probe cadence previously never fired and recovery
  waited for the old normal timer.
- Numeric config values with leading zeros (e.g. `SHOWY_QUOTA_PNG_BAR_W=09`)
  no longer abort the renderers with an octal arithmetic error; unsigned
  integer config is normalized to base-10 on load.
- The pre-commit rustfmt hook now includes renamed Rust files
  (`--diff-filter=ACMR`); a rename+edit previously skipped the local check
  and failed in CI.
- `bin/showy-quota-fetch` now matches the managed `codexbar serve` process by
  splitting the `ps` command output with `read -ra` instead of an unquoted
  expansion. The previous `for token in ${command}` glob-expanded the command
  string against the working directory, so a cwd containing a file matching a
  glob character in the command line could misclassify a healthy serve and
  trigger an unnecessary restart (or miss a running serve).
- `bin/showy-quota-fetch`'s cache publisher now checks each `mv` when promoting
  the temporary `usage`/`usage-stamp`/`source` files and removes the leftover
  temporaries on failure instead of leaking them into the cache directory.
- The Rust renderer's pacing-marker math now computes in `u64` before narrowing
  to `usize`, so a large `windowMinutes` no longer truncates or overflows on the
  32-bit `usize` of the `wasm32` Zellij plugin (it already matched the shell's
  64-bit arithmetic on native targets).
- Palette dim-scaling widens to `u128` before multiplying, so a pathological
  scale factor can no longer overflow (debug panic / release wrap); channels
  clamp to `0xff` as before.
- Numeric render config (`SHOWY_QUOTA_GOOD_MIN_REMAINING`, `…_WARN_MIN_REMAINING`,
  `…_TIME_WARN_MINUTES`, `…_REFRESH_SECONDS`, `…_DIM_WINDOW_MINUTES`,
  `…_LOCK_WAIT_TENTHS`) is validated at load: a non-numeric value — which bash
  arithmetic silently evaluates to `0` — is replaced by the documented default
  instead of, e.g., forcing every provider to render "good" or reporting the
  cache as never stale. `LOCK_WAIT_TENTHS` is also clamped to a ceiling so a
  pathological value cannot stall the cache wait loop.
- The standalone Zellij plugin no longer latches in a permanent loading/broken
  state when `codexbar serve` answers HTTP 200 with a corrupt body (captive
  portal / proxy page): an unparseable payload now counts as a serve failure and
  advances toward CLI fallback instead of resetting the failure counter.
- The standalone Zellij plugin now expires a `/health` or `/usage` web request
  that hangs without ever producing a result (Zellij reports no request
  failure) after 30s, so a dropped connection no longer wedges the plugin — it
  retries and falls back to the CLI exactly as a returned failure would.
- `share/config.env.example` no longer documents removed knobs: the
  `SHOWY_QUOTA_PALETTE_{SECONDARY,TERTIARY}_*`/`*_SCALE` entries (replaced by
  `SHOWY_QUOTA_PALETTE_DIM_SCALE` + `SHOWY_QUOTA_DIM_WINDOW_MINUTES`) and the
  `SHOWY_QUOTA_MONO3_*` entries (replaced by `SHOWY_QUOTA_PROVIDER_MODES`,
  `SHOWY_QUOTA_MONO_COLOR_MODE`, `SHOWY_QUOTA_MONO_MARKERS`) are updated to the
  live names so following the example no longer sets variables that have no
  effect.
- `bin/showy-quota-fetch` now build-version-gates `codexbar serve` reuse: when a
  healthy serve's `/health` reports a `version`, it is reused only if that build
  matches the installed `codexbar --version`. Both strings are reduced to a
  comparable version token (matching glean's stale-serve detector), so a
  `CodexBar`-prefixed `/health` value is not mistaken for a stale build. A
  mismatch (e.g. after a CodexBar
  update left a stale in-memory binary on the port) recycles the serve —
  terminating a managed serve through its pidfile or freeing the configured
  port of a foreign one — and starts a fresh build. A serve whose `/health`
  omits `version` is reused unchanged, so the gate is a no-op for builds that
  predate the field. This stops reuse of a stale serve binary, the trigger for
  the post-update SecurityAgent keychain-prompt storm (see also the CodexBar-side
  fix [steipete/CodexBar#1717](https://github.com/steipete/CodexBar/pull/1717)).
  The configured `codexbar` binary is resolved to an absolute path before
  `--version` and before launching a managed serve, because CodexBar reads its
  version from the app bundle via `argv[0]`: a bare command name reports no
  version (in `--version` or `/health`), which would otherwise leave the gate
  inert and spawn version-less serves. This gate and the plugin's `build_marker`
  below both depend on CodexBar reporting `version` on `/health`, landed via
  [steipete/CodexBar#1703](https://github.com/steipete/CodexBar/pull/1703).
- The standalone Zellij plugin can now detect a stale-build `codexbar serve` and
  append a `⚠ver` marker, behind the opt-in `build_marker` config key (default
  off). When enabled it parses the serve's `/health` `version`, compares it
  against the installed `codexbar --version` (a periodic, absolute-path-
  resolving, watchdog-bounded probe gated on `cli_fallback`), and flags a
  mismatch using the same version-token normalization as the shell and glean.
  Detection only — the plugin never kills or recycles a session serve (that
  stays with `showy-quota-fetch`). With `build_marker` off the version probe
  never runs; a serve or build that reports no version, or `cli_fallback "off"`,
  is also a silent no-op.

### Security
- All GitHub Actions in `ci.yml` and `release.yml` are pinned to commit SHAs
  (`actions/checkout`, `dtolnay/rust-toolchain`, `softprops/action-gh-release`)
  instead of mutable floating tags, so a moved/compromised tag cannot alter the
  build or the signed WASM release artifact.
- The standalone Zellij plugin enforces the same loopback-only `serve_url`
  contract as the shell data plane: a non-loopback URL is dropped (falling back
  to the CLI) so Zellij's granted WebAccess cannot be turned into an
  SSRF/exfiltration vector by a shared KDL layout.
- `bin/showy-quota-tmux-bar` doubles any `#` in the user-configured
  `SHOWY_QUOTA_CAP_LEFT`/`SHOWY_QUOTA_CAP_RIGHT` so cap text renders literally
  instead of being parsed as a tmux format/expansion directive.
- Terminal bar width (`SHOWY_QUOTA_TMUX_BAR_WIDTH` / `SHOWY_QUOTA_ZELLIJ_BAR_WIDTH`)
  is clamped to `[8, 400]` across the shell renderers and the Rust core so an
  unbounded value cannot drive a runaway render loop.
- The `*_BIN` overrides (`SHOWY_QUOTA_FETCH_BIN`, `SHOWY_QUOTA_CODEXBAR_BIN`,
  `SHOWY_QUOTA_ZELLIJ_BIN`) are validated before use across every renderer
  entry point and `lib/common.sh`: a value carrying whitespace or shell
  metacharacters (e.g. `/bin/sh -c …`) is rejected back to a trusted default
  instead of being exec'd. A plain command name or path — including a
  not-yet-installed one — is still honored, so cache-only and custom-path
  installs are unaffected. The standalone Zellij plugin applies the same check
  to the `serve_command`/`cli_command` KDL knobs.
- Status glyphs (`SHOWY_QUOTA_STALE_GLYPH`, `SHOWY_QUOTA_DEGRADED_CLI_GLYPH`)
  are rejected back to their defaults when they carry control characters or an
  absurd length, so a poisoned env/config value cannot corrupt SketchyBar or
  terminal-strip output.
- `SHOWY_QUOTA_THEME` is validated against the same bare-name charset the CLI
  enforces (`^[A-Za-z0-9._-]+$`) before `showy_quota_load_config` builds and
  sources the theme `.env`, so a value like `../../../tmp/evil` can no longer
  traverse out of the themes directory to source an arbitrary file. An invalid
  name is ignored (defaults kept) rather than aborting the renderer.
- CodexBar-supplied `resetsAt`/`resetDescription` date text is length-capped (64
  chars) before it is handed to `date`/`gdate -d` in `showy_quota_reset_epoch`
  and `showy_quota_reset_description_epoch`, so a pathologically long payload
  field cannot stall the date parser.
- The SketchyBar string knobs `SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR` (must be an
  8-digit `0xAARRGGBB` ARGB literal) and `SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT`
  (no control characters, ≤128 chars) are validated back to their defaults when
  malformed, so a bad value yields the default item style instead of a broken
  `sketchybar --set`. `share/config.env.example` now also documents that
  `SHOWY_QUOTA_SKETCHYBAR_CLICK` is run by SketchyBar as a shell command on click.
- `valid_provider_id` in `bin/showy-quota-fetch` now rejects the path components
  `.` and `..` (which matched the existing `^[A-Za-z0-9_.-]+$` charset), so a
  CodexBar payload with a provider id of `..` can no longer point the
  per-provider failure-stamp path outside `SHOWY_QUOTA_PROVIDER_FAILURE_DIR`.
- Zellij pipe identifiers are validated before use: `SHOWY_QUOTA_ZELLIJ_WIDGET`
  must match `^[A-Za-z0-9_.-]+$` (so a `::` cannot shift the zjstatus
  `pipe::<widget>::<output>` field split and misroute output to another widget)
  and `SHOWY_QUOTA_ZELLIJ_PIPE_NAME` must match `^[A-Za-z0-9_-]+$`; both fall
  back to their defaults otherwise. `bin/showy-quota-zellij-pipe` also caps the
  rendered payload at 4096 chars so a pathological render can't silently exceed
  the `zellij pipe` argument limit.
- `showy-quota --grant-zellij` hardens the permissions.kdl write: the plugin
  path is rejected if it contains a quote, backslash, or control character (so
  it cannot break out of the KDL string literal and inject permission nodes),
  and a `SHOWY_QUOTA_ZELLIJ_PERMISSIONS_FILE` override must be an absolute
  `*.kdl` path (so an attacker-set env var cannot redirect the write to a shell
  rc, LaunchAgent plist, `~/.ssh/authorized_keys`, or cron file).
- `showy-quota-zellij-bar --json <file>` now requires a regular file and
  validates it as quota JSON before rendering, so it can no longer be pointed at
  a FIFO/device or an arbitrary non-quota file (e.g. `/etc/passwd`).
- The SketchyBar plugin renders provider-icon SVGs under a bundled restrictive
  ImageMagick policy (`adapters/sketchybar/imagemagick/policy.xml`, injected via
  `MAGICK_CONFIGURE_PATH`) that denies the network coders, so a provider SVG
  from `SHOWY_QUOTA_CODEXBAR_RESOURCES` cannot make `magick` fetch a remote
  `href` (SSRF/data exfiltration) regardless of the system ImageMagick policy.
- The serve/CLI timeout knobs in `bin/showy-quota-fetch` are now bounded above:
  `SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS` clamps to 60s (the `curl --max-time`
  for the serve probe), the CLI and config-providers timeouts to 300s, and the
  serve start-wait poll to 300s, so a pathological value cannot turn a fetch into
  an effectively unbounded wait.
- The release workflow now generates a signed build-provenance attestation for
  the WASM plugin artifact (`actions/attest-build-provenance`), so a downloaded
  `showy-quota-zellij.wasm` can be verified to have been built by this repo's
  release workflow (`gh attestation verify showy-quota-zellij.wasm --repo
  enieuwy/showy-quota`) rather than only checked against a recomputable sha256.

## [0.3.0] — 2026-06-20

### Fixed
- The SketchyBar countdown label no longer truncates the widest `HH:MM`
  reset times (e.g. Gemini's `23:59` lost its last digit). The pinned
  `SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH` default is now `32` (was `27`), which
  fits the longest countdown form; shorter strings stay jitter-free as before.
- The Zellij/tmux `mono3` body now collapses to the two-lane
  `dual` body for providers without a tertiary window, matching the SketchyBar
  renderer which already drops the absent tertiary row, so a provider whose
  third positional slot is empty no longer shows a blank bottom lane.
- The standalone Zellij plugin no longer floods macOS with keychain prompts
  (and no longer leaks `codexbar usage --provider …` processes as zellij-server
  children) when `codexbar serve` cannot reach a provider that blocks on an
  interactive prompt — e.g. the login-keychain "Claude Code-credentials" dialog.
  Per-provider CLI fallback and provider discovery now run through a POSIX-sh
  watchdog that terminates the spawned command after
  `PROVIDER_COMMAND_TIMEOUT_SECONDS` (15s); Zellij has no command-cancel API, so
  a wedged command previously survived forever and a fresh prompt queued on
  every retry. Per-provider failure backoff now escalates exponentially (base
  doubling per consecutive failure, capped at 30 min), so a persistently
  blocking provider is probed rarely instead of every tick while its
  last-known-good slice keeps rendering as degraded.
- `bin/showy-quota-fetch` no longer restarts the managed `codexbar serve` when
  `/usage` merely times out (curl exit 28). A serve whose `/health` is OK but
  whose `/usage` blocks on provider collection (keychain) is alive, not
  unhealthy; restarting it only spawned a fresh serve that re-triggered the same
  blocking collection and prompt. It now records a serve failure and falls back
  without churning serve.
- Per-provider CLI fallback now carries forward each provider's last-known-good
  record from the existing cache when its fresh fetch fails or is in backoff, so
  a transiently blocking provider (claude, cursor) renders stale instead of
  vanishing from the bars. Carry-forward is gated on at least one fresh success,
  so a cycle where every provider fails still preserves the prior cache and lets
  it age to stale rather than republishing it under a fresh timestamp.

### Changed
- Model-pooled providers now adapt to the data with no configuration: in `auto`
  mode a provider whose `usage.extraRateWindows` carry every present positional
  slot is auto-detected and **split into one standalone `dual` provider per
  pool** — `AGᴳ` (Gemini), `AGᶜ` (Claude+GPT) — each a normal dual widget
  (semantic-colored sigil, full-width bar, pacing marker). A single pool stays
  one plain `dual`. This fixes Antigravity, whose pools depend on CodexBar's
  auth method: OAuth reports only Gemini (one pool → `AG`); the Antigravity IDE
  reports Gemini plus Claude+GPT (two pools → `AGᴳ` + `AGᶜ`). The split reuses the
  existing `dual` renderer on every surface — no combined-widget code.
  `SHOWY_QUOTA_PROVIDER_MODES` / `provider_modes` force it per provider
  (`provider=dual2`), unioning a positional pool with extra pools (e.g. Codex +
  Spark → `CXᶜ` + `CXˢ`), or select the opt-in octant body (`provider=mono4`,
  terminal support required). The SketchyBar plugin shows a pooled provider's
  windows as adaptive 2–4 native slider rows.
- `make install-plugin` now pre-grants the standalone Zellij plugin's
  permissions for the installed path, so a fresh install is prompt-free on
  first launch (previously it only printed a reminder to run
  `make grant-zellij-permissions`). The grant is best-effort and never fails
  the install. It covers first launch only: a later macOS cache purge still
  drops the grant, so re-run `make grant-zellij-permissions` if the prompt
  returns — an upstream Zellij limitation, not a regression.
- The SketchyBar bar slot default (`SHOWY_QUOTA_SKETCHYBAR_BAR_WIDTH`) is now
  `SHOWY_QUOTA_PNG_BAR_W + 3` (was `+ 4`), trimming 1px of dead space between
  each provider's bars and its countdown. This offsets the wider countdown
  label above so per-provider width stays effectively unchanged; the 80px bars
  are untouched.
- Bar dimming and pacing are now driven by each usage window's **horizon**
  instead of its row position. A window is dimmed only when its `windowMinutes`
  is at or beyond `SHOWY_QUOTA_DIM_WINDOW_MINUTES` (default `10080`, i.e.
  weekly/monthly), so a 5h live tier stays bright while its weekly/monthly cap
  dims — regardless of which slot it occupies. Time-tiered providers (Codex,
  Claude) keep a bright 5h row over a dimmed weekly row; model-pooled providers
  (Antigravity) keep each pool's bright 5h row over its dimmed weekly; Gemini's uniform-daily pools dim
  none. The `dual` body now draws a pacing marker on **both** rows, and the
  SketchyBar plugin gained a `primary_marker` so every pool is paced (was
  secondary/tertiary only).
- The Zellij/tmux marker is now a configurable list: `SHOWY_QUOTA_MONO_MARKERS`
  (default `primary`) names which window slots get a pacing separator (`none`
  disables), replacing the single `SHOWY_QUOTA_MONO3_MARKER_SOURCE`. The first
  marker uses `palette_elapsed`, the rest a distinct `palette_elapsed_long`.
- `auto` mode now reads the per-provider `SHOWY_QUOTA_PROVIDER_MODES` map
  (default `gemini=mono3,cursor=mono3`) instead of the `mono3_providers`
  allow/deny lists; providers without an entry render `dual`, except
  auto-detected model pools (see above). `mono4` is opt-in via this map and is
  never chosen automatically.
- Pools that share one billing cycle — identical `resetsAt` and `windowMinutes`
  across at least two present windows — now render at full brightness and draw a
  single pacing marker in every renderer (Zellij, tmux, SketchyBar). These are
  parallel usage *categories* within one budget (e.g. Cursor's Total/Auto/API on
  one 30-day cycle), not a live tier over a longer cap, so the horizon-based
  dimming and per-row markers that suit Codex/Claude are suppressed for them.
  `cursor` now ships as `mono3` by default so all three pools are visible (the
  `dual` body dropped the API pool); the SketchyBar rows and tmux/Zellij bodies
  stop showing redundant dimming and duplicate markers for it.

### Added
- `SHOWY_QUOTA_PALETTE_DIM_SCALE` / `palette_dim_scale` (default `0.55`) and
  `SHOWY_QUOTA_PALETTE_DIM_{GOOD,WARN,BAD,UNKNOWN}` / `palette_dim_*` set the
  color of dimmed (long-horizon) windows, plus `SHOWY_QUOTA_DIM_WINDOW_MINUTES`
  / `dim_window_minutes` (default `10080`) sets the weekly/monthly dim threshold.
- New `mono4` terminal body: four per-pool windows packed into one Unicode 16
  octant cell (`U+1CD00`), the 4-lane sibling of `mono3`'s sextants. mono4's
  windows are assembled from `usage.extraRateWindows` (e.g. Antigravity's Gemini
  and Claude+GPT session/weekly pools) and collapse to `mono3`/`dual` when fewer
  than four are available. Requires an octant-capable terminal (Ghostty, kitty,
  WezTerm); `docs/scripts/preview-quad-octants.py` tests a terminal and previews it.
- `SHOWY_QUOTA_PALETTE_ELAPSED_LONG` (default `3ddbd9`) colors the second and
  later pacing markers; `SHOWY_QUOTA_MONO_MARKERS` (list) and
  `SHOWY_QUOTA_PROVIDER_MODES` (map) configure markers and per-provider bodies.
- New `dual2` terminal body: splits a model-pooled provider into one standalone
  `dual` per pool (`AGᴳ` Gemini, `AGᶜ` Claude+GPT) from `usage.extraRateWindows`,
  each rendered by the normal half-block `dual` path (every terminal, unlike
  `mono4`). Auto-detected in `auto` when a provider's extras carry all its
  positional slots; force per provider via
  `SHOWY_QUOTA_PROVIDER_MODES=<provider>=dual2`; a single pool stays one plain
  `dual`.

### Removed
- The row-position palette knobs `SHOWY_QUOTA_PALETTE_SECONDARY_*`,
  `SHOWY_QUOTA_PALETTE_TERTIARY_*`, `SHOWY_QUOTA_PALETTE_SECONDARY_SCALE`, and
  `SHOWY_QUOTA_PALETTE_TERTIARY_SCALE` (and their KDL equivalents). Dimming is
  now horizon-based via `SHOWY_QUOTA_PALETTE_DIM_*` / `SHOWY_QUOTA_PALETTE_DIM_SCALE`;
  move any secondary/tertiary color or scale overrides to the `DIM` keys.
- The `sextant3` terminal mode (its per-column-color variant of `mono3` was a
  forced-only niche `auto` never selected). Use `mono3` (or `mono4`).
- `SHOWY_QUOTA_MONO3_PROVIDERS` / `_EXCLUDE` (use `SHOWY_QUOTA_PROVIDER_MODES`),
  `SHOWY_QUOTA_MONO3_MARKER_SOURCE` (use `SHOWY_QUOTA_MONO_MARKERS`),
  `SHOWY_QUOTA_MONO3_COLOR_MODE` (renamed `SHOWY_QUOTA_MONO_COLOR_MODE`), and
  `SHOWY_QUOTA_MONO3_MARKER_STYLE` (markers now always replace a cell; the
  `insert` style and the `shared` marker source are gone).

## [0.2.5] — 2026-06-12

### Changed
- The SketchyBar countdown label now reserves a fixed width
  (`SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH`, default `27`) so provider rows and
  the pill no longer jitter as the remaining-time string changes length.

### Fixed
- Shell and standalone Zellij renderers now keep providers whose primary quota
  window is absent when secondary or tertiary quota windows are still valid.
  This restores Antigravity, whose CodexBar payload currently reports
  `usage.primary: null` with usable secondary/tertiary windows.
- Usage window slots are now semantic across every renderer (SketchyBar,
  tmux, Zellij shell strip, and the standalone Zellij plugin): primary,
  secondary, and tertiary windows always map to their documented top, middle,
  and bottom rows. Previously missing windows were compacted away, so a
  provider without a primary window rendered its secondary window in the
  primary row and borrowed its reset for the countdown. A missing primary now
  renders an empty top row with an `idle` label, while present windows stay
  in their own rows, markers, and color roles.
- `bin/showy-quota-fetch` now treats a successful empty CodexBar provider
  inventory as canonical empty state, publishes `[]`, and bypasses stale serve
  or CLI backoff data instead of preserving disabled providers.
- The standalone Zellij plugin now validates `codexbar serve /usage` provider
  sets against the discovered canonical inventory before publishing. Any
  mismatch falls back to per-provider CLI data or an empty payload, preventing
  stale disabled providers from rendering after CodexBar config changes.

## [0.2.4] — 2026-06-07

### Added
- `showy-quota --grant-zellij [path]` and `make grant-zellij-permissions`
  can pre-grant the standalone plugin's Zellij permissions by writing
  `permissions.kdl` for the installed plugin path. This is optional setup
  convenience for source/shell users; the release WASM remains standalone and
  can still be used by accepting Zellij's native prompt or adding the documented
  KDL block by hand. The helper is idempotent, preserves other plugins' grants,
  and re-heals after a macOS cache purge. `make install-plugin` now prints the
  command as an optional follow-up.

### Changed
- Zellij plugin permission requests now match enabled runtime paths: `WebAccess`
  is always requested, while `OpenTerminalsOrPlugins` is skipped when
  `manage_serve` is disabled and `RunCommands` is skipped when degraded CLI
  fallback is off.
- Shell cache refreshes now allow local `codexbar serve /usage` requests up to
  10 seconds by default, preventing slow provider collection from being
  misclassified as CLI fallback and shown as `⚠cli` in SketchyBar/Zellij.

## [0.2.3] — 2026-06-05

### Fixed
- `bin/showy-quota-fetch`: avoid Bash here-strings and heredocs in runtime
  fetch paths so Homebrew Bash 5.3 no longer hangs while validating CodexBar
  payloads. This lets SketchyBar refresh the shared cache again instead of
  showing stale warnings while the standalone Zellij plugin remains fresh.

## [0.2.2] — 2026-06-02

### Fixed
- `bin/showy-quota-fetch`: preserve serve-backed last-known-good usage data
  during transient `/usage` failures, retry serve normally while the serve cache
  is active, and fall back to CLI only after repeated serve-unavailable failures.
- SketchyBar: render stale/degraded markers from the same cache snapshot as the
  quota payload, avoiding background-refresh races that could show stale `cli`
  markers.

## [0.2.1] — 2026-06-02

### Added
- `bin/showy-quota --diagnose --json` emits stable machine-readable diagnostics
  (paths, tool availability, config/cache state, env knobs, and a CodexBar
  probe) alongside the existing human-readable output.
- Configurable degraded CLI glyph via the `degraded_cli_glyph` plugin/KDL key
  and `SHOWY_QUOTA_DEGRADED_CLI_GLYPH`.
- `reset_description_timezone_offset` (KDL) /
  `SHOWY_QUOTA_RESET_DESCRIPTION_TIMEZONE_OFFSET` to make local-time
  `resetDescription` countdowns deterministic under WASM, where the host
  timezone cannot be inferred. ISO `resetsAt` timestamps remain preferred.
- Corrupt cache quarantine: an invalid `usage.json` is moved aside as
  `usage.json.corrupt.<epoch>.<pid>` with bounded retention
  (`SHOWY_QUOTA_CORRUPT_CACHE_RETENTION`, default `3`) instead of being
  silently rechecked.
- CI now runs `cargo clippy --workspace --all-targets -- -D warnings` and a
  Linux `cargo audit` dependency advisory scan, plus `cargo fmt --check`.

### Changed
- Moved host-specific SketchyBar, tmux, and Zellij fragments under
  `adapters/` to keep the repository root focused on shared code and metadata.
- Shell renderers no longer block on lock contention when a valid cache already
  exists: non-forced callers emit the current snapshot immediately while the
  lock holder refreshes. Forced refreshes still wait and retry.
- `make doctor` distinguishes `codexbar` CLI vs serve-only data sources and
  reports optional-tool availability without failing valid setups.
- The TPM `showy-quota.tmux` wrapper now preflights `tmux` and verifies the
  renderer is executable before wiring a `#(...)` status command.
- Palette helpers (shell and Rust) accept a leading `#` and normalize hex to
  lowercase.
- Smaller Zellij WASM artifact via a release profile (`lto`,
  `opt-level = "z"`, `codegen-units = 1`, `panic = "abort"`, stripped
  debuginfo).
- SketchyBar icon cache now keys on palette inputs and cleans up temporary
  icon files.

### Fixed
- The Zellij plugin no longer requests a repaint when an event leaves the
  rendered strip unchanged, while still repainting on real output changes and
  synchronous timer-driven state transitions.
- `bin/showy-quota-fetch` closes stdin for spawned CodexBar subprocesses so a
  hung command cannot block on terminal input.
- `crates/showy-quota-zellij-core` reset-description day rollover uses checked
  date arithmetic instead of a panicking add.
- `make install*` targets mark source scripts executable; `make uninstall`
  removes the copied plugin only when it matches the current artifact.

## [0.2.0] — 2026-06-01

### Added
- Standalone `showy-quota-zellij.wasm` plugin for Zellij. It fetches CodexBar
  serve directly, renders the quota strip in-process, and removes the normal
  Zellij dependency on `zjstatus`, feeder loops, and showy-quota shell scripts.
- TPM-compatible `showy-quota.tmux` wrapper that wires the existing tmux
  renderer into `status-right` and can optionally bind the detail popup.
- Rust renderer parity tests that compare plugin output against the existing
  shell Zellij renderer over JSON fixtures.
- `make plugin` / `make install-plugin` and a release workflow that attaches
  the prebuilt Zellij WASM artifact to `v*` releases.
- Provider-aware per-provider fallback in both `bin/showy-quota-fetch` and the
  self-contained Zellij plugin, so individual provider refresh failures no
  longer collapse the whole quota strip.

### Changed
- Zellij docs now make the standalone plugin primary and move the zjstatus pipe
  feeder to the advanced composition path.
- Renamed project from `showy-bar` to `showy-quota`. All binaries, env vars,
  config paths, SketchyBar/zjstatus widget names, cache paths, and docs use
  `showy-quota` / `showy_quota` / `SHOWY_QUOTA` consistently. The Zellij
  pipe widget is now `pipe_showy_quota`, the SketchyBar item prefix is
  `showy_quota`, and the config directory is `~/.config/showy-quota/`. Git
  remote updated to `enieuwy/showy-quota`.
- Provider discovery now uses CodexBar `config providers` as the canonical
  inventory for fallback while preserving `SHOWY_QUOTA_PROVIDERS`,
  `SHOWY_QUOTA_PROVIDERS_EXCLUDE`, and `SHOWY_QUOTA_PROVIDER_ORDER` filters.
- Fallback refreshes now use per-provider backoff and preserve stale cache
  entries when a provider is temporarily unavailable.

### Fixed
- Canonical empty provider inventories now publish an empty usage payload
  instead of being treated as refresh failures.

### Removed
- `showy-quota-zellij-kick` and `showy-quota-zellij-new-tab`; the standalone
  plugin paints on load and timer events, so the recommended path no longer
  needs manual repaint wrappers.

## [0.1.0] — 2026-05-17

### Added
- Initial scaffold: SketchyBar, Zellij, and tmux indicator strips driven by
  `codexbar usage --format json`.
- Shared cache fetcher `bin/showy-quota-fetch` with flock and last-known-good
  fallback.
- Stable state surface `bin/showy-quota-state` for external layout managers.
- SketchyBar `showy_quota_provider_change` event trigger on provider-set changes.
- ANSI strip renderer `bin/showy-quota-zellij-bar`.
- tmux markup renderer `bin/showy-quota-tmux-bar`.
- Long-running zjstatus pipe loop `bin/showy-quota-zellij-pipe`.
- SketchyBar item + plugin scripts that source provider icons from
  CodexBar's bundled SVGs (`/Applications/CodexBar.app/Contents/Resources`).
- `make install` / `make uninstall` symlink installer.
- Smoke tests over JSON fixtures.
- Built-in `default` palette theme matching the original ai-quota/showy-quota
  colors.
- Renamed the former `example` palette to `catppuccin-mocha-blue` and made its
  Catppuccin Mocha colors explicit.
- Renamed countdown and fallback-icon palette knobs to user-facing role names:
  `SHOWY_QUOTA_PALETTE_COUNTDOWN`, `SHOWY_QUOTA_PALETTE_COUNTDOWN_WARN`, and
  `SHOWY_QUOTA_PALETTE_ICON_TEXT`.
- `lib/common.sh`: explicit Bash 4 guard with a friendly error pointing macOS
  users at Homebrew bash.
- `bin/showy-quota --diagnose` / `make diagnose`: prints tool paths, config
  state, cache age, provider state, and active env knobs for bug reports.
- `make doctor`: validates `bash` 4+, `jq`, and `codexbar` before installing.
  `make install` now depends on `doctor`; bypass with `make install-bin` if
  you really must.

### Fixed
- `lib/common.sh`: `showy_quota_log` now treats `SHOWY_QUOTA_DEBUG=0` as off (was
  any non-empty value).
- `bin/showy-quota-zellij-bar`: `SHOWY_QUOTA_FORCE_COLOR=0` no longer force-enables
  color; only `=1` does.
- `bin/showy-quota-zellij-pipe`: validate `SHOWY_QUOTA_ZELLIJ_PIPE_INTERVAL`
  numeric input to avoid feeder-killing `sleep` failures under `set -euo pipefail`.

### Security
- `bin/showy-quota-fetch`: cache dir and files now persist as `0700`/`0600`
  instead of the user's default umask. CodexBar usage JSON stays user-only.

[Unreleased]: https://github.com/enieuwy/showy-quota/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/enieuwy/showy-quota/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/enieuwy/showy-quota/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/enieuwy/showy-quota/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/enieuwy/showy-quota/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/enieuwy/showy-quota/compare/v0.2.5...v0.3.0
[0.2.5]: https://github.com/enieuwy/showy-quota/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/enieuwy/showy-quota/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/enieuwy/showy-quota/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/enieuwy/showy-quota/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/enieuwy/showy-quota/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/enieuwy/showy-quota/releases/tag/v0.2.0
