#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use showy_quota_zellij_core::palette::hex_to_rgb;
use showy_quota_zellij_core::{
    parse_provider_config_payload, parse_usage_payload, payload_has_renderable_provider,
    provider_ids_from_records, render_zellij, valid_provider_id, ProviderConfigError,
    ProviderRecord, RenderConfig, RenderOptions,
};
use zellij_tile::prelude::*;

const HEALTH_KIND: &str = "showy-quota-health";
const USAGE_KIND: &str = "showy-quota-usage";
const WEB_REQUEST_GENERATION_KEY: &str = "showy-quota-web-generation";
const FALLBACK_DISCOVER_KIND: &str = "showy-quota-fallback-discover";
const FALLBACK_DISCOVER_ATTEMPT_KEY: &str = "showy-quota-discover-attempt";
const FALLBACK_PROVIDER_KIND: &str = "showy-quota-fallback-provider";
const FALLBACK_PROVIDER_CONTEXT_KEY: &str = "showy-quota-provider";
const FALLBACK_PROVIDER_ATTEMPT_KEY: &str = "showy-quota-provider-attempt";
const SERVE_FAILURES_BEFORE_CLI: u8 = 3;
const MANAGED_SERVE_RETRY_COOLDOWN_SECONDS: f64 = 30.0;
const PROVIDER_DISCOVERY_BACKOFF_SECONDS_DEFAULT: f64 = 60.0;
const PROVIDER_COMMAND_TIMEOUT_SECONDS: u64 = 15;
// While an outage hold is active the bar wakes on this short cadence to re-probe
// serve, so a fast managed-serve restart is detected within seconds instead of a
// full `interval_seconds`.
const HOLD_REPROBE_INTERVAL_SECONDS: f64 = 3.0;
// Zellij never reports a failed/cancelled web_request, so an in-flight probe
// that hangs (dropped connection, wedged proxy) is expired after its window and
// treated as a serve failure so the plugin retries and can fall back to the CLI
// instead of latching forever. /health is a cheap liveness check and expires
// fast; /usage gets a larger budget because a healthy serve bounds collection
// per provider (~0.8x its request deadline, ~24s by default) and still returns
// the healthy providers when a slow one degrades to an error row — expiring it
// on the short health window would abandon that usable partial response. These
// mirror the shell fetcher's SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS (health)
// and SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS (usage) defaults.
const SERVE_HEALTH_TIMEOUT_SECONDS: f64 = 10.0;
const SERVE_USAGE_TIMEOUT_SECONDS: f64 = 30.0;
const PROVIDER_BACKOFF_MAX_SECONDS: f64 = 1800.0;
const VERSION_KIND: &str = "showy-quota-version";
const VERSION_ATTEMPT_KEY: &str = "showy-quota-version-attempt";
const VERSION_COMMAND_TIMEOUT_SECONDS: u64 = 5;
// How long an on-disk `codexbar --version` result is trusted before re-probing.
const ONDISK_VERSION_TTL_SECONDS: f64 = 300.0;
// Plugin-appended marker (integration boundary) meaning "the running serve is an
// older build than the installed binary; restart it." Distinct from the core
// `stale_glyph` (data old) and `degraded_cli_glyph` (on CLI fallback); styled to
// match them (bold, countdown-warn fg, bar bg).
const BUILD_STALE_MARKER: &str = "⚠ver";

/// POSIX-sh watchdog that runs `"$@"` but guarantees the spawned process cannot
/// outlive `timeout_secs`. Zellij exposes no API to cancel a `run_command`, so a
/// CLI fallback that wedges on an interactive prompt (e.g. the macOS login
/// keychain for Claude credentials) would otherwise leak as a zellij-server
/// child and queue a fresh prompt on every retry. The watchdog reaps it instead.
fn watchdog_script(timeout_secs: u64) -> String {
    format!(
        "\"$@\" & __p=$!; (sleep {t}; kill \"$__p\" 2>/dev/null; sleep 2; kill -9 \"$__p\" 2>/dev/null) & __k=$!; wait \"$__p\"; __r=$?; kill \"$__k\" 2>/dev/null; exit \"$__r\"",
        t = timeout_secs
    )
}

/// Wrap a command in the self-terminating watchdog. `$0` is only a label; the
/// real command rides in `"$@"`, so provider ids and the configured binary path
/// are never interpolated into the shell string (injection-safe).
fn watchdog_argv<'a>(script: &'a str, command: &[&'a str]) -> Vec<&'a str> {
    let mut argv = vec!["/bin/sh", "-c", script, "showy-quota-watchdog"];
    argv.extend_from_slice(command);
    argv
}

/// Like `watchdog_script`, but first resolves the binary in `$1` to an absolute
/// path: CodexBar reads its version from the app bundle via `argv[0]`, so a bare
/// command name reports no version. Mirrors the shell `codexbar_bin_abs` — a
/// value containing a slash is trusted as-is, otherwise `command -v` resolves
/// it. The binary still rides in `$1` (never interpolated), so this is
/// injection-safe like `watchdog_script`.
fn version_probe_script(timeout_secs: u64) -> String {
    format!(
        "case $1 in */*) b=$1;; *) b=$(command -v \"$1\" 2>/dev/null) || b=$1;; esac; \"$b\" --version & __p=$!; (sleep {t}; kill \"$__p\" 2>/dev/null; sleep 2; kill -9 \"$__p\" 2>/dev/null) & __k=$!; wait \"$__p\"; __r=$?; kill \"$__k\" 2>/dev/null; exit \"$__r\"",
        t = timeout_secs
    )
}

/// Build the argv for a version probe: the binary rides in `$1` (never
/// interpolated into the script); `$0` is only a label.
fn version_probe_argv<'a>(script: &'a str, bin: &'a str) -> Vec<&'a str> {
    vec!["/bin/sh", "-c", script, "showy-quota-version", bin]
}
#[cfg(target_arch = "wasm32")]
fn shim_set_selectable(selectable: bool) {
    set_selectable(selectable);
}

#[cfg(not(target_arch = "wasm32"))]
fn shim_set_selectable(_selectable: bool) {}

#[cfg(target_arch = "wasm32")]
fn shim_subscribe(event_types: &[EventType]) {
    subscribe(event_types);
}

#[cfg(not(target_arch = "wasm32"))]
fn shim_subscribe(_event_types: &[EventType]) {}

#[cfg(target_arch = "wasm32")]
fn shim_request_permission(permissions: &[PermissionType]) {
    request_permission(permissions);
}

#[cfg(not(target_arch = "wasm32"))]
fn shim_request_permission(_permissions: &[PermissionType]) {}

#[cfg(target_arch = "wasm32")]
fn shim_set_timeout(secs: f64) {
    set_timeout(secs);
}

#[cfg(not(target_arch = "wasm32"))]
fn shim_set_timeout(_secs: f64) {}

#[cfg(target_arch = "wasm32")]
fn shim_web_request(
    url: String,
    verb: HttpVerb,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    context: BTreeMap<String, String>,
) {
    web_request(url, verb, headers, body, context);
}

#[cfg(not(target_arch = "wasm32"))]
fn shim_web_request(
    _url: String,
    _verb: HttpVerb,
    _headers: BTreeMap<String, String>,
    _body: Vec<u8>,
    _context: BTreeMap<String, String>,
) {
}

#[cfg(target_arch = "wasm32")]
fn shim_run_command(cmd: &[&str], context: BTreeMap<String, String>) {
    run_command(cmd, context);
}

#[cfg(not(target_arch = "wasm32"))]
fn shim_run_command(_cmd: &[&str], _context: BTreeMap<String, String>) {}

#[cfg(target_arch = "wasm32")]
fn shim_open_command_pane_background(
    command: CommandToRun,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    open_command_pane_background(command, context)
}

#[cfg(not(target_arch = "wasm32"))]
fn shim_open_command_pane_background(
    _command: CommandToRun,
    _context: BTreeMap<String, String>,
) -> Option<PaneId> {
    Some(PaneId::Plugin(0))
}

#[cfg(target_arch = "wasm32")]
fn shim_get_plugin_ids() -> (u32, String) {
    let ids = get_plugin_ids();
    (
        ids.plugin_id,
        ids.initial_cwd.to_string_lossy().into_owned(),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn shim_get_plugin_ids() -> (u32, String) {
    (0, String::new())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Unknown,
    Probing,
    Serve,
    ManagedServeStarting,
    Cli,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliFallback {
    Off,
    Degraded,
}

fn requested_permissions(manage_serve: bool, cli_fallback: CliFallback) -> Vec<PermissionType> {
    let mut permissions = vec![PermissionType::WebAccess];
    if manage_serve {
        permissions.push(PermissionType::OpenTerminalsOrPlugins);
    }
    if cli_fallback != CliFallback::Off {
        permissions.push(PermissionType::RunCommands);
    }
    permissions
}

#[derive(Debug, Default, Clone)]
struct ProviderFallbackState {
    in_flight: bool,
    last_record: Option<serde_json::Value>,
    last_result_empty: bool,
    last_attempt_seconds: Option<f64>,
    last_failure_seconds: Option<f64>,
    active_attempt_token: Option<String>,
    consecutive_failures: u32,
}

#[derive(Debug)]
struct State {
    render_config: RenderConfig,
    serve_url: String,
    interval_seconds: f64,
    cli_interval_seconds: f64,
    manage_serve: bool,
    serve_command: String,
    serve_port: String,
    cli_fallback: CliFallback,
    cli_command: String,
    // backoff window between repeated per-provider retries after a failure;
    // defaults to cli_interval_seconds so backoff scales with the same cadence
    // that drives the bar.
    provider_failure_backoff_seconds: f64,
    source: Source,
    managed_serve_requested: bool,
    managed_serve_last_attempt_seconds: Option<f64>,
    consecutive_serve_failures: u8,
    last_payload: Option<Vec<u8>>,
    last_success_seconds: Option<f64>,
    last_cli_fetch_seconds: Option<f64>,
    last_output: String,
    health_in_flight: bool,
    usage_in_flight: bool,
    health_generation: u64,
    usage_generation: u64,
    active_health_generation: Option<u64>,
    active_usage_generation: Option<u64>,
    // When the in-flight /health or /usage web request was started, so a hung
    // request can be expired (Zellij never reports request failure).
    web_flight_started_at: Option<f64>,
    permissions_granted: bool,
    // Provider discovery state: cached output of `codexbar config providers`.
    // An empty `discovered_providers` with `discovered_providers_at == None`
    // means no discovery attempt has succeeded; an empty list with a set
    // timestamp means CodexBar reports zero enabled providers (canonical
    // empty inventory → publish `[]`).
    discovered_providers: Vec<String>,
    discovered_providers_at: Option<f64>,
    discovery_in_flight: bool,
    discovery_attempt_token: Option<String>,
    discovery_started_at: Option<f64>,
    usage_after_discovery: bool,
    serve_inventory_mismatch: bool,
    discovery_failed_at: Option<f64>,
    discovery_failure_backoff_seconds: f64,
    // Per-provider fallback state: tracks in-flight commands and last-known
    // good records so one provider's failure never blocks the others.
    provider_states: BTreeMap<String, ProviderFallbackState>,
    // Stale-serve build gate. `serve_build_version` is the version reported by
    // the running serve's /health (None for a pre-#1703 serve that omits it);
    // `ondisk_version` is the installed binary's version from a periodic
    // `codexbar --version` probe. A marker shows only when both are known and
    // differ. Probe state mirrors the discovery in-flight/token guard.
    serve_build_version: Option<String>,
    ondisk_version: Option<String>,
    ondisk_version_checked_at: Option<f64>,
    version_probe_in_flight: bool,
    version_probe_token: Option<String>,
    // Opt-in (KDL `build_marker true`, default off). When off, the on-disk
    // version probe never runs and the ⚠ver marker is never appended — the
    // whole stale-build gate is a silent no-op. The plugin only flags; it
    // never recycles a session-owned serve.
    show_build_marker: bool,
    // Stable per-instance seed (Zellij plugin id + cwd + serve/cli command),
    // used to disperse the degraded-fallback hold and per-provider retry backoff
    // across the N same-config tab instances without any shared state or RNG.
    instance_hash: u64,
    // Max per-instance random hold (seconds) before the first CLI fallback after
    // a serve outage; 0 disables the hold (legacy immediate-fallback behavior).
    fallback_jitter_seconds: f64,
    // Deadline of the active degraded-fallback hold, if any. While set and in the
    // future, the plugin re-probes serve instead of spawning CLI work.
    cli_hold_until: Option<f64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            render_config: RenderConfig::default(),
            serve_url: "http://127.0.0.1:8080".into(),
            interval_seconds: 10.0,
            cli_interval_seconds: 120.0,
            manage_serve: true,
            serve_command: "codexbar".into(),
            serve_port: "8080".into(),
            cli_fallback: CliFallback::Degraded,
            cli_command: "codexbar".into(),
            provider_failure_backoff_seconds: 120.0,
            source: Source::Unknown,
            managed_serve_requested: false,
            managed_serve_last_attempt_seconds: None,
            consecutive_serve_failures: 0,
            last_payload: None,
            last_success_seconds: None,
            last_cli_fetch_seconds: None,
            last_output: " showy-quota: loading ".into(),
            health_in_flight: false,
            usage_in_flight: false,
            health_generation: 0,
            usage_generation: 0,
            active_health_generation: None,
            active_usage_generation: None,
            web_flight_started_at: None,
            permissions_granted: false,
            discovered_providers: Vec::new(),
            discovered_providers_at: None,
            discovery_in_flight: false,
            discovery_attempt_token: None,
            discovery_started_at: None,
            usage_after_discovery: false,
            serve_inventory_mismatch: false,
            discovery_failed_at: None,
            discovery_failure_backoff_seconds: PROVIDER_DISCOVERY_BACKOFF_SECONDS_DEFAULT,
            provider_states: BTreeMap::new(),
            serve_build_version: None,
            ondisk_version: None,
            ondisk_version_checked_at: None,
            version_probe_in_flight: false,
            version_probe_token: None,
            show_build_marker: false,
            instance_hash: 0,
            fallback_jitter_seconds: 60.0,
            cli_hold_until: None,
        }
    }
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.render_config = RenderConfig::from_kdl_config(&configuration);
        self.serve_url = configuration
            .get("serve_url")
            .or_else(|| configuration.get("SHOWY_QUOTA_CODEXBAR_SERVE_URL"))
            .cloned()
            .unwrap_or_else(|| "http://127.0.0.1:8080".into());
        // Mirror the shell `serve_base_url` guard: only a loopback serve URL is
        // honored. A non-loopback URL would turn Zellij's granted WebAccess into
        // an SSRF / exfiltration vector (e.g. a shared KDL layout pointing the
        // plugin at an internal or metadata endpoint), so drop it and fall back
        // to the CLI path instead.
        if !self.serve_url.trim().is_empty() && !is_loopback_serve_url(&self.serve_url) {
            self.serve_url = String::new();
        }
        self.interval_seconds = parse_positive_f64(
            configuration.get("interval_seconds").map(String::as_str),
            10.0,
        );
        self.cli_interval_seconds = parse_positive_f64(
            configuration
                .get("cli_interval_seconds")
                .map(String::as_str),
            120.0,
        );
        self.manage_serve = parse_bool(configuration.get("manage_serve").map(String::as_str), true);
        self.show_build_marker =
            parse_bool(configuration.get("build_marker").map(String::as_str), false);
        self.serve_command = configuration
            .get("serve_command")
            .or_else(|| configuration.get("SHOWY_QUOTA_CODEXBAR_BIN"))
            .map(|value| value.trim().to_string())
            .filter(|value| valid_command(value))
            .unwrap_or_else(|| "codexbar".into());
        self.serve_port = configuration
            .get("serve_port")
            .or_else(|| configuration.get("SHOWY_QUOTA_CODEXBAR_SERVE_PORT"))
            .map(|value| value.trim())
            .filter(|value| valid_port(value))
            .map(str::to_string)
            .or_else(|| derive_port_from_url(&self.serve_url))
            .unwrap_or_else(|| "8080".into());
        self.cli_command = configuration
            .get("cli_command")
            .or_else(|| configuration.get("fallback_command"))
            .or_else(|| configuration.get("SHOWY_QUOTA_CODEXBAR_BIN"))
            .map(|value| value.trim().to_string())
            .filter(|value| valid_command(value))
            .unwrap_or_else(|| "codexbar".into());
        // Per-provider CLI backoff after a failure. Defaults to the same
        // interval that drives the bar so backoff scales with refresh cadence.
        self.provider_failure_backoff_seconds = parse_positive_f64(
            configuration
                .get("provider_failure_backoff_seconds")
                .map(String::as_str),
            self.cli_interval_seconds,
        );
        self.discovery_failure_backoff_seconds = parse_positive_f64(
            configuration
                .get("provider_discovery_backoff_seconds")
                .map(String::as_str),
            PROVIDER_DISCOVERY_BACKOFF_SECONDS_DEFAULT,
        );
        self.cli_fallback = match configuration
            .get("cli_fallback")
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("off") | Some("false") | Some("0") | Some("none") => CliFallback::Off,
            _ => CliFallback::Degraded,
        };
        let (plugin_id, initial_cwd) = shim_get_plugin_ids();
        self.instance_hash =
            instance_seed(plugin_id, &initial_cwd, &self.serve_url, &self.cli_command);
        self.fallback_jitter_seconds = parse_nonnegative_f64(
            configuration
                .get("fallback_jitter_seconds")
                .map(String::as_str),
            self.cli_interval_seconds.min(60.0),
        );

        shim_set_selectable(false);
        shim_subscribe(&[
            EventType::PermissionRequestResult,
            EventType::Timer,
            EventType::Visible,
            EventType::WebRequestResult,
            EventType::RunCommandResult,
        ]);

        // Some Zellij versions do not emit PermissionRequestResult when a
        // local file plugin is already pre-granted in permissions.kdl. Start
        // optimistically from a timer so pre-granted plugins render instead
        // of staying blank; an explicit Denied result below still shuts
        // requests down.
        self.permissions_granted = true;
        shim_set_timeout(0.1);

        let permissions = requested_permissions(self.manage_serve, self.cli_fallback);
        shim_request_permission(&permissions);
    }
    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                let previous_output = self.last_output.clone();
                self.permissions_granted = true;
                self.health_in_flight = false;
                self.usage_in_flight = false;
                self.active_health_generation = None;
                self.active_usage_generation = None;
                self.web_flight_started_at = None;
                self.discovery_in_flight = false;
                self.discovery_attempt_token = None;
                self.discovery_started_at = None;
                self.usage_after_discovery = false;
                self.serve_inventory_mismatch = false;
                self.clear_all_provider_in_flight();
                self.cli_hold_until = None;
                self.schedule_timer();
                self.tick();
                self.last_output != previous_output
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                self.permissions_granted = false;
                self.health_in_flight = false;
                self.usage_in_flight = false;
                self.active_health_generation = None;
                self.active_usage_generation = None;
                self.web_flight_started_at = None;
                self.discovery_in_flight = false;
                self.discovery_attempt_token = None;
                self.discovery_started_at = None;
                self.usage_after_discovery = false;
                self.serve_inventory_mismatch = false;
                self.clear_all_provider_in_flight();
                self.cli_hold_until = None;
                self.last_output = " showy-quota: permission denied ".into();
                true
            }
            Event::Timer(_) => {
                let previous_output = self.last_output.clone();
                self.schedule_timer();
                self.refresh_output();
                self.tick();
                self.last_output != previous_output
            }
            Event::Visible(true) => true,
            Event::WebRequestResult(status, _headers, body, context) => {
                let previous_output = self.last_output.clone();
                match context.get("kind").map(String::as_str) {
                    Some(HEALTH_KIND) => {
                        if !self.web_response_matches(HEALTH_KIND, &context) {
                            return false;
                        }
                        self.health_in_flight = false;
                        self.active_health_generation = None;
                        self.web_flight_started_at = None;
                        if status == 200 {
                            self.update_serve_build_version(&body);
                            self.kick_usage();
                        } else {
                            self.handle_serve_unavailable();
                        }
                        self.last_output != previous_output
                    }
                    Some(USAGE_KIND) => {
                        if !self.web_response_matches(USAGE_KIND, &context) {
                            return false;
                        }
                        self.usage_in_flight = false;
                        self.active_usage_generation = None;
                        self.web_flight_started_at = None;
                        if status == 200 && self.accept_payload(body, Source::Serve) {
                            self.consecutive_serve_failures = 0;
                        } else {
                            self.handle_usage_failure();
                        }
                        self.last_output != previous_output
                    }
                    _ => false,
                }
            }
            Event::RunCommandResult(exit, stdout, _stderr, context) => {
                match context.get("kind").map(String::as_str) {
                    Some(FALLBACK_DISCOVER_KIND) => {
                        let previous_output = self.last_output.clone();
                        let attempt = context
                            .get(FALLBACK_DISCOVER_ATTEMPT_KEY)
                            .map(String::as_str);
                        self.handle_discovery_result(exit, stdout, attempt);
                        self.last_output != previous_output
                    }
                    Some(FALLBACK_PROVIDER_KIND) => {
                        let provider = context
                            .get(FALLBACK_PROVIDER_CONTEXT_KEY)
                            .cloned()
                            .unwrap_or_default();
                        let attempt = context
                            .get(FALLBACK_PROVIDER_ATTEMPT_KEY)
                            .map(String::as_str);
                        self.handle_provider_fallback_result(&provider, attempt, exit, stdout)
                    }
                    Some(VERSION_KIND) => {
                        let previous_output = self.last_output.clone();
                        let attempt = context.get(VERSION_ATTEMPT_KEY).map(String::as_str);
                        self.handle_version_result(exit, stdout, attempt);
                        self.last_output != previous_output
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        print!("{}", self.last_output);
    }
}

impl State {
    fn set_source(&mut self, source: Source) {
        if matches!(source, Source::Cli | Source::Unavailable) {
            self.managed_serve_requested = false;
        }
        // Returning to the serve HTTP path makes any pending degraded-fallback
        // hold moot: the outage we were holding through is over.
        if source == Source::Serve {
            self.cli_hold_until = None;
        }
        self.source = source;
    }

    fn managed_serve_retry_allowed(&self, now_seconds: f64) -> bool {
        self.managed_serve_last_attempt_seconds
            .map(|last_attempt| {
                (now_seconds - last_attempt).max(0.0) >= MANAGED_SERVE_RETRY_COOLDOWN_SECONDS
            })
            .unwrap_or(true)
    }

    fn should_start_managed_serve(&self, now_seconds: f64) -> bool {
        self.manage_serve
            && !self.managed_serve_requested
            && self.managed_serve_retry_allowed(now_seconds)
    }

    /// Late per-provider results from a previous CLI burst must be discarded
    /// once serve has recovered. Discovery results, in contrast, are always
    /// safe to absorb because they only update the inventory cache.
    fn should_accept_cli_result(&self) -> bool {
        !matches!(self.source, Source::Serve)
    }

    fn schedule_timer(&self) {
        shim_set_timeout(self.next_timeout_seconds(now_seconds()));
    }

    /// Single scheduling chokepoint. While an outage hold is active, wake on a
    /// short cadence to re-probe serve; otherwise use the normal bar cadence.
    /// Centralized so the hold never spawns a second, overlapping timer stream.
    fn next_timeout_seconds(&self, now: f64) -> f64 {
        if let Some(until) = self.cli_hold_until {
            if now < until {
                let remaining = (until - now).max(0.1);
                let reprobe = HOLD_REPROBE_INTERVAL_SECONDS.min(self.interval_seconds);
                return remaining.min(reprobe);
            }
        }
        self.interval_seconds
    }

    fn web_response_matches(&self, kind: &str, context: &BTreeMap<String, String>) -> bool {
        let generation = context
            .get(WEB_REQUEST_GENERATION_KEY)
            .and_then(|value| value.parse::<u64>().ok());
        match kind {
            HEALTH_KIND => matches!(
                (generation, self.active_health_generation),
                (Some(response), Some(active)) if self.health_in_flight && response == active
            ),
            USAGE_KIND => matches!(
                (generation, self.active_usage_generation),
                (Some(response), Some(active)) if self.usage_in_flight && response == active
            ),
            _ => false,
        }
    }

    /// Whether a serve->CLI transition should first wait out a per-instance
    /// jittered hold (re-probing serve) instead of immediately stampeding the
    /// per-provider CLI. Gated to genuine outage transitions: never delays a
    /// cold start (no prior payload), an already-committed CLI source, the
    /// inventory-mismatch correctness fallback (which set source to Cli), or
    /// pure-CLI mode (empty serve_url). `fallback_jitter_seconds = 0` disables.
    fn should_cli_hold(&self) -> bool {
        self.fallback_jitter_seconds > 0.0
            && !self.serve_url.trim().is_empty()
            && !matches!(self.source, Source::Cli)
            && self.last_payload.is_some()
    }

    /// Deterministic per-instance hold length in `[0, fallback_jitter_seconds)`.
    /// Pure function of the instance seed, so the N same-config tab instances
    /// disperse to distinct offsets with no shared state or RNG.
    fn hold_delay_seconds(&self) -> f64 {
        unit_from(self.instance_hash) * self.fallback_jitter_seconds
    }
    fn expire_stale_discovery(&mut self) {
        if !self.discovery_in_flight {
            return;
        }
        let now = now_seconds();
        let Some(started_at) = self.discovery_started_at else {
            self.discovery_started_at = Some(now);
            return;
        };
        if (now - started_at).max(0.0) < self.discovery_failure_backoff_seconds {
            return;
        }
        self.discovery_in_flight = false;
        self.discovery_attempt_token = None;
        self.discovered_providers_at = None;
        self.usage_after_discovery = false;
        self.discovery_started_at = None;
        self.discovery_failed_at = Some(now);
    }

    /// Expire a hung /health or /usage probe. Returns true when it expired and
    /// routed the timeout through the same failure path a non-200 result would,
    /// so the caller (tick) should not also kick a fresh request this pass.
    fn expire_stale_web_flight(&mut self) -> bool {
        if !self.health_in_flight && !self.usage_in_flight {
            return false;
        }
        let now = now_seconds();
        let Some(started_at) = self.web_flight_started_at else {
            // In-flight without a recorded start: stamp it so a later tick can
            // expire it rather than wedging indefinitely.
            self.web_flight_started_at = Some(now);
            return false;
        };
        let timeout = if self.usage_in_flight {
            SERVE_USAGE_TIMEOUT_SECONDS
        } else {
            SERVE_HEALTH_TIMEOUT_SECONDS
        };
        if (now - started_at).max(0.0) < timeout {
            return false;
        }
        let was_usage = self.usage_in_flight;
        if was_usage {
            self.usage_in_flight = false;
            self.active_usage_generation = None;
        } else {
            self.health_in_flight = false;
            self.active_health_generation = None;
        }
        self.web_flight_started_at = None;
        if was_usage {
            self.handle_usage_failure();
        } else {
            self.handle_serve_unavailable();
        }
        true
    }

    fn tick(&mut self) {
        if !self.permissions_granted {
            return;
        }
        if self.expire_stale_web_flight() {
            return;
        }
        self.expire_stale_discovery();
        self.maybe_kick_version_probe();
        if self.source == Source::Serve {
            if self.discovery_in_flight {
                self.usage_after_discovery = true;
                return;
            }
            if self.needs_discovery() {
                self.usage_after_discovery = true;
                self.kick_discovery();
                return;
            }
        }
        match self.source {
            Source::Unknown | Source::Unavailable => self.kick_health_probe(),
            Source::Probing | Source::ManagedServeStarting => self.kick_health_probe(),
            Source::Serve => self.kick_usage(),
            Source::Cli => self.tick_cli(),
        }
    }

    fn tick_cli(&mut self) {
        if self.health_in_flight {
            return;
        }
        if self.serve_url.trim().is_empty() {
            if self.cli_due() {
                self.kick_cli_fallback();
            }
            return;
        }
        self.kick_health_probe();
    }

    fn cli_due(&self) -> bool {
        self.last_cli_fetch_seconds
            .map(|seconds| (now_seconds() - seconds).max(0.0) >= self.cli_interval_seconds)
            .unwrap_or(true)
    }

    fn kick_health_probe(&mut self) {
        if !self.permissions_granted || self.health_in_flight {
            return;
        }
        if self.serve_url.trim().is_empty() {
            self.kick_cli_fallback_or_render_failure();
            return;
        }
        self.health_generation = self.health_generation.saturating_add(1);
        let generation = self.health_generation;
        self.active_health_generation = Some(generation);
        self.health_in_flight = true;
        self.web_flight_started_at = Some(now_seconds());
        if self.source != Source::Cli {
            self.set_source(Source::Probing);
        }
        let mut headers = BTreeMap::new();
        headers.insert("Accept".to_string(), "application/json".to_string());
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), HEALTH_KIND.to_string());
        context.insert(
            WEB_REQUEST_GENERATION_KEY.to_string(),
            generation.to_string(),
        );
        let url = format!("{}/health", self.serve_url.trim_end_matches('/'));
        shim_web_request(url, HttpVerb::Get, headers, Vec::new(), context);
    }

    fn kick_usage(&mut self) {
        if !self.permissions_granted || self.usage_in_flight || self.serve_url.trim().is_empty() {
            return;
        }
        self.expire_stale_discovery();
        if self.discovery_in_flight {
            self.usage_after_discovery = true;
            return;
        }
        if self.needs_discovery() {
            self.usage_after_discovery = true;
            self.kick_discovery();
            return;
        }
        self.usage_generation = self.usage_generation.saturating_add(1);
        let generation = self.usage_generation;
        self.active_usage_generation = Some(generation);
        self.usage_in_flight = true;
        self.web_flight_started_at = Some(now_seconds());
        let mut headers = BTreeMap::new();
        headers.insert("Accept".to_string(), "application/json".to_string());
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), USAGE_KIND.to_string());
        context.insert(
            WEB_REQUEST_GENERATION_KEY.to_string(),
            generation.to_string(),
        );
        let url = format!("{}/usage", self.serve_url.trim_end_matches('/'));
        shim_web_request(url, HttpVerb::Get, headers, Vec::new(), context);
    }

    fn handle_serve_unavailable(&mut self) {
        self.serve_build_version = None;
        if self.should_start_managed_serve(now_seconds()) {
            self.start_managed_serve();
            return;
        }
        self.kick_cli_fallback_or_render_failure();
    }

    fn handle_usage_failure(&mut self) {
        self.consecutive_serve_failures = self.consecutive_serve_failures.saturating_add(1);
        let inventory_mismatch = self.serve_inventory_mismatch;
        self.serve_inventory_mismatch = false;
        if self.cli_fallback != CliFallback::Off
            && self.discovered_providers_at.is_some()
            && self.discovered_providers.is_empty()
        {
            self.publish_empty_cli_payload();
            return;
        }
        if inventory_mismatch && self.cli_fallback != CliFallback::Off {
            self.set_source(Source::Cli);
        }
        if !inventory_mismatch
            && self.last_payload.is_some()
            && self.consecutive_serve_failures < SERVE_FAILURES_BEFORE_CLI
        {
            self.refresh_output();
            return;
        }
        self.kick_cli_fallback_or_render_failure();
    }

    fn start_managed_serve(&mut self) {
        self.managed_serve_requested = true;
        self.managed_serve_last_attempt_seconds = Some(now_seconds());
        self.set_source(Source::ManagedServeStarting);
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), "showy-quota-serve".to_string());
        let command = CommandToRun {
            path: self.serve_command.clone().into(),
            args: vec![
                "serve".into(),
                "--port".into(),
                self.serve_port.clone(),
                "--refresh-interval".into(),
                "60".into(),
            ],
            cwd: None,
        };
        if shim_open_command_pane_background(command, context).is_none() {
            self.kick_cli_fallback_or_render_failure();
        }
    }

    fn kick_cli_fallback_or_render_failure(&mut self) {
        if self.cli_fallback == CliFallback::Off {
            self.cli_hold_until = None;
            self.set_source(Source::Unavailable);
            self.render_failure();
            return;
        }
        if self.should_cli_hold() {
            let now = now_seconds();
            match self.cli_hold_until {
                // Still holding: do NOT re-probe inline. Re-probes are paced by
                // the short-cadence Timer (see next_timeout_seconds; source is
                // Probing during the hold so tick() issues them every ~3s) and
                // bounded by expire_stale_web_flight. Probing here would chain
                // off every fast non-200 WebRequestResult into a tight request
                // loop that all N tabs run at once against a recovering serve.
                Some(until) if now < until => return,
                // Hold expired with serve still down: commit to CLI and latch
                // the source so a subsequent failure does not re-arm the hold
                // every cycle (we stay steadily degraded until serve recovers).
                Some(_) => {
                    self.cli_hold_until = None;
                    self.set_source(Source::Cli);
                }
                // First fallback this outage: arm the per-instance hold and do a
                // single re-probe (which moves source off Serve so a later
                // committed CLI result is accepted), then let the Timer pace it.
                None => {
                    let delay = self.hold_delay_seconds();
                    if delay > 0.0 {
                        self.cli_hold_until = Some(now + delay);
                        self.schedule_timer();
                        self.kick_health_probe();
                        return;
                    }
                }
            }
        } else {
            self.cli_hold_until = None;
        }
        self.kick_cli_fallback();
    }

    /// Drive provider-aware CLI fallback: discover providers first (if we
    /// don't have a fresh inventory yet), then issue one `RunCommand` per
    /// eligible provider whose previous attempt is not in-flight or backoff.
    fn kick_cli_fallback(&mut self) {
        if !self.permissions_granted {
            return;
        }
        self.expire_stale_discovery();
        if self.discovery_in_flight {
            return;
        }
        if self.needs_discovery() {
            self.kick_discovery();
            return;
        }
        // Discovery succeeded (possibly with an empty inventory). If empty,
        // publish `[]` so the bar shows idle instead of stale/blank.
        let now = now_seconds();
        self.expire_stale_provider_flights(now);
        let providers = self.eligible_provider_inventory();
        if providers.is_empty() {
            if self.discovered_providers_at.is_some() {
                // Canonical empty inventory: publish `[]` so the bar shows idle.
                self.publish_empty_cli_payload();
            } else if !self.has_provider_work_in_flight() && self.last_payload.is_none() {
                self.render_cli_failure();
            }
            return;
        }
        let mut spawned_any = false;
        for provider in providers.iter() {
            if self.provider_in_flight_or_backoff(provider, now) {
                continue;
            }
            self.kick_provider_call(provider);
            spawned_any = true;
        }
        if !spawned_any
            && self.last_payload.is_none()
            && self.all_provider_attempts_terminal(&providers)
        {
            self.render_cli_failure();
        }
    }

    fn needs_discovery(&self) -> bool {
        if self.cli_fallback == CliFallback::Off || self.discovery_in_flight {
            return false;
        }
        if let Some(discovered_at) = self.discovered_providers_at {
            let elapsed = (now_seconds() - discovered_at).max(0.0);
            if elapsed < self.discovery_failure_backoff_seconds {
                return false;
            }
        }
        if let Some(failed_at) = self.discovery_failed_at {
            let elapsed = (now_seconds() - failed_at).max(0.0);
            if elapsed < self.discovery_failure_backoff_seconds {
                return false;
            }
        }
        true
    }

    fn kick_discovery(&mut self) {
        if self.cli_fallback == CliFallback::Off
            || self.discovery_in_flight
            || !self.permissions_granted
        {
            return;
        }
        let attempt_token = format!("{:.6}", now_seconds());
        self.discovery_started_at = Some(now_seconds());
        self.discovery_attempt_token = Some(attempt_token.clone());
        self.discovery_in_flight = true;
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), FALLBACK_DISCOVER_KIND.to_string());
        context.insert(FALLBACK_DISCOVER_ATTEMPT_KEY.to_string(), attempt_token);
        let argv = [
            self.cli_command.as_str(),
            "config",
            "providers",
            "--format",
            "json",
            "--pretty",
        ];
        let script = watchdog_script(PROVIDER_COMMAND_TIMEOUT_SECONDS);
        shim_run_command(&watchdog_argv(&script, &argv), context);
    }

    fn handle_discovery_result(
        &mut self,
        exit: Option<i32>,
        stdout: Vec<u8>,
        attempt: Option<&str>,
    ) {
        if attempt != self.discovery_attempt_token.as_deref() {
            return;
        }
        self.discovery_in_flight = false;
        self.discovery_attempt_token = None;
        self.discovery_started_at = None;
        let now = now_seconds();
        let resume_usage = self.usage_after_discovery;
        self.usage_after_discovery = false;
        if exit != Some(0) {
            self.discovery_failed_at = Some(now);
            if resume_usage {
                self.discovered_providers_at = None;
                self.kick_usage();
            } else {
                self.fallback_after_discovery();
            }
            return;
        }
        match parse_provider_config_payload(&stdout) {
            Ok(providers) => {
                self.discovered_providers = providers;
                self.discovered_providers_at = Some(now);
                self.discovery_failed_at = None;
                // Drop per-provider state for providers no longer reported as
                // enabled so a disabled provider does not linger in the bar.
                let allowed: std::collections::BTreeSet<String> =
                    self.discovered_providers.iter().cloned().collect();
                self.provider_states.retain(|id, _| allowed.contains(id));
                self.prune_last_payload_to_current_inventory();
                if resume_usage {
                    self.kick_usage();
                } else {
                    self.fallback_after_discovery();
                }
            }
            Err(ProviderConfigError::InvalidInventory) | Err(ProviderConfigError::Parse(_)) => {
                self.discovery_failed_at = Some(now);
                if resume_usage {
                    self.discovered_providers_at = None;
                    self.kick_usage();
                } else {
                    self.fallback_after_discovery();
                }
            }
        }
    }

    /// Parse the running serve build version from a /health 200 body. A body
    /// without a `version` field (pre-#1703 serve) yields None, which keeps the
    /// gate inert. Never alters the serve/usage flow.
    fn update_serve_build_version(&mut self, body: &[u8]) {
        self.serve_build_version = serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|value| {
                value
                    .get("version")
                    .and_then(|v| v.as_str())
                    .and_then(codexbar_version_token)
            });
    }

    /// True only when we are rendering serve data AND both versions are known
    /// AND they differ. Unknown either side (or any non-serve source) => false,
    /// so the marker never shows on CLI output or pre-#1703 serves.
    fn serve_build_stale(&self) -> bool {
        self.source == Source::Serve
            && matches!(
                (
                    self.serve_build_version.as_deref(),
                    self.ondisk_version.as_deref(),
                ),
                (Some(running), Some(ondisk)) if running != ondisk
            )
    }

    /// Issue an on-disk `codexbar --version` probe when we have a serve build to
    /// compare against, CLI fallback (RunCommands) is available, and the cached
    /// on-disk version is stale. Strictly orthogonal to the serve failure path:
    /// it never touches consecutive_serve_failures or the CLI burst.
    fn maybe_kick_version_probe(&mut self) {
        if !self.show_build_marker {
            return;
        }
        if self.cli_fallback == CliFallback::Off || !self.permissions_granted {
            return;
        }
        if self.source != Source::Serve || self.serve_build_version.is_none() {
            return;
        }
        if self.version_probe_in_flight {
            return;
        }
        let now = now_seconds();
        let due = self
            .ondisk_version_checked_at
            .map(|checked| (now - checked).max(0.0) >= ONDISK_VERSION_TTL_SECONDS)
            .unwrap_or(true);
        if due {
            self.kick_version_probe(now);
        }
    }

    fn kick_version_probe(&mut self, now: f64) {
        self.version_probe_in_flight = true;
        let attempt_token = format!("{:.6}", now);
        self.version_probe_token = Some(attempt_token.clone());
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), VERSION_KIND.to_string());
        context.insert(VERSION_ATTEMPT_KEY.to_string(), attempt_token);
        let script = version_probe_script(VERSION_COMMAND_TIMEOUT_SECONDS);
        shim_run_command(&version_probe_argv(&script, &self.cli_command), context);
    }

    /// Absorb an on-disk version probe result. A stale token (a late result from
    /// a superseded probe) is ignored. On failure or an unparseable version the
    /// last-known on-disk version is kept (no marker flapping); only the check
    /// timestamp advances.
    fn handle_version_result(&mut self, exit: Option<i32>, stdout: Vec<u8>, attempt: Option<&str>) {
        if attempt != self.version_probe_token.as_deref() {
            return;
        }
        self.version_probe_in_flight = false;
        self.version_probe_token = None;
        self.ondisk_version_checked_at = Some(now_seconds());
        if exit != Some(0) {
            return;
        }
        let raw = String::from_utf8_lossy(&stdout);
        if let Some(token) = codexbar_version_token(&raw) {
            if self.ondisk_version.as_deref() != Some(token.as_str()) {
                self.ondisk_version = Some(token);
                self.refresh_output();
            }
        }
    }

    /// After a discovery result lands, immediately kick per-provider calls
    /// (or fall back to the cache-derived inventory) so the bar refreshes in
    /// the same tick rather than waiting one full `interval_seconds`.
    fn fallback_after_discovery(&mut self) {
        if !matches!(
            self.source,
            Source::Cli
                | Source::Probing
                | Source::Unknown
                | Source::Unavailable
                | Source::ManagedServeStarting
        ) {
            return;
        }
        self.kick_cli_fallback();
    }

    /// Build the eligible per-provider work list: discovered providers minus
    /// `providers_exclude`, optionally filtered through `providers` (allow-
    /// list), then ordered by `provider_order`. Discovery output stays the
    /// canonical inventory — callers may only filter or order it.
    fn eligible_provider_inventory(&self) -> Vec<String> {
        let mut candidates: Vec<String> = if self.discovered_providers_at.is_some() {
            self.discovered_providers.clone()
        } else if let Some(payload) = self.last_payload.as_deref() {
            // Fallback when discovery is unavailable: pull ids from the
            // current cache rather than going completely blind.
            match parse_usage_payload(payload) {
                Ok(records) => provider_ids_from_records(&records),
                Err(_) => Vec::new(),
            }
        } else if !self.render_config.providers.is_empty() {
            // Last-resort explicit override: only consulted when discovery
            // and cache both failed.
            self.render_config.providers.clone()
        } else if !self.provider_states.is_empty() {
            // Provider command results can arrive after a transition cleared
            // cache/discovery context; keep those already-launched providers
            // eligible so a valid result can render instead of synthesizing [].
            self.provider_states.keys().cloned().collect()
        } else {
            Vec::new()
        };
        candidates.retain(|id| valid_provider_id(id));
        if !self.render_config.providers.is_empty() {
            let allow: std::collections::BTreeSet<&str> = self
                .render_config
                .providers
                .iter()
                .map(String::as_str)
                .collect();
            candidates.retain(|id| allow.contains(id.as_str()));
        }
        if !self.render_config.providers_exclude.is_empty() {
            let block: std::collections::BTreeSet<&str> = self
                .render_config
                .providers_exclude
                .iter()
                .map(String::as_str)
                .collect();
            candidates.retain(|id| !block.contains(id.as_str()));
        }
        // Promote providers in `provider_order` to the front, preserving the
        // remaining first-seen order for everything else.
        if !self.render_config.provider_order.is_empty() {
            let mut ordered: Vec<String> = Vec::with_capacity(candidates.len());
            for token in &self.render_config.provider_order {
                if let Some(idx) = candidates.iter().position(|id| id == token) {
                    ordered.push(candidates.remove(idx));
                }
            }
            ordered.extend(candidates);
            ordered
        } else {
            candidates
        }
    }

    fn provider_in_flight_or_backoff(&self, provider: &str, now: f64) -> bool {
        let Some(state) = self.provider_states.get(provider) else {
            return false;
        };
        if state.in_flight {
            return true;
        }
        if let Some(failed_at) = state.last_failure_seconds {
            let elapsed = (now - failed_at).max(0.0);
            if elapsed < self.effective_provider_backoff(state) {
                return true;
            }
        }
        false
    }

    /// Per-provider failure backoff with exponential escalation. A provider
    /// whose CLI call keeps wedging (keychain prompt, offline backend) doubles
    /// its retry window each consecutive failure up to `PROVIDER_BACKOFF_MAX_SECONDS`,
    /// so a persistently blocking provider is probed rarely instead of every tick.
    fn effective_provider_backoff(&self, state: &ProviderFallbackState) -> f64 {
        let base = self.provider_failure_backoff_seconds;
        let cap = PROVIDER_BACKOFF_MAX_SECONDS.max(base);
        match state.consecutive_failures {
            0 | 1 => base,
            n => {
                let shift = (n - 1).min(20);
                (base * 2f64.powi(shift as i32)).min(cap)
            }
        }
    }

    fn kick_provider_call(&mut self, provider: &str) {
        let now = now_seconds();
        let attempt_token = format!("{now:.6}");
        let entry = self
            .provider_states
            .entry(provider.to_string())
            .or_default();
        entry.in_flight = true;
        entry.last_attempt_seconds = Some(now);
        entry.active_attempt_token = Some(attempt_token.clone());
        let include_status = self.render_config.include_status;
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), FALLBACK_PROVIDER_KIND.to_string());
        context.insert(
            FALLBACK_PROVIDER_CONTEXT_KEY.to_string(),
            provider.to_string(),
        );
        context.insert(FALLBACK_PROVIDER_ATTEMPT_KEY.to_string(), attempt_token);
        let mut argv: Vec<&str> = vec![
            self.cli_command.as_str(),
            "usage",
            "--provider",
            provider,
            "--format",
            "json",
            "--pretty",
        ];
        if include_status {
            argv.push("--status");
        }
        let script = watchdog_script(PROVIDER_COMMAND_TIMEOUT_SECONDS);
        shim_run_command(&watchdog_argv(&script, &argv), context);
    }

    fn handle_provider_fallback_result(
        &mut self,
        provider: &str,
        attempt: Option<&str>,
        exit: Option<i32>,
        stdout: Vec<u8>,
    ) -> bool {
        if provider.is_empty() {
            return false;
        }
        if let Some(state) = self.provider_states.get(provider) {
            // Strict attempt matching, same rule as handle_discovery_result:
            // a result is accepted only for the exact live attempt (or on the
            // legacy tokenless path). A tokened result arriving when no
            // attempt is active is late, orphaned (its token was cleared by
            // clear_all_provider_in_flight), or a duplicate — accepting it
            // could overwrite a newer success with stale data. Rejection is
            // cheap: in_flight is already false and no failure/backoff is
            // recorded, so the next tick simply re-kicks the provider.
            match (attempt, state.active_attempt_token.as_deref()) {
                (Some(result_token), Some(active)) if result_token == active => {}
                (None, None) => {}
                _ => return false,
            }
        }
        if let Some(state) = self.provider_states.get_mut(provider) {
            state.in_flight = false;
            state.active_attempt_token = None;
        }
        if !self.should_accept_cli_result() {
            return false;
        }
        let now = now_seconds();
        if exit != Some(0) {
            self.record_provider_failure(provider, now);
            let mut changed = self.refresh_output();
            changed |= self.render_cli_failure_if_all_terminal();
            return changed;
        }
        let record = match extract_provider_record(&stdout, provider) {
            Ok(record) => record,
            Err(()) => {
                self.record_provider_failure(provider, now);
                let mut changed = self.refresh_output();
                changed |= self.render_cli_failure_if_all_terminal();
                return changed;
            }
        };
        let entry = self
            .provider_states
            .entry(provider.to_string())
            .or_default();
        entry.last_record = record;
        entry.last_result_empty = entry.last_record.is_none();
        entry.last_failure_seconds = None;
        entry.consecutive_failures = 0;
        entry.in_flight = false;
        entry.active_attempt_token = None;
        self.publish_synthesized_cli_payload(now)
    }

    fn record_provider_failure(&mut self, provider: &str, now: f64) {
        let entry = self
            .provider_states
            .entry(provider.to_string())
            .or_default();
        entry.in_flight = false;
        entry.active_attempt_token = None;
        entry.last_result_empty = false;
        entry.last_failure_seconds = Some(now);
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
    }

    fn clear_all_provider_in_flight(&mut self) {
        for state in self.provider_states.values_mut() {
            state.in_flight = false;
            state.active_attempt_token = None;
        }
    }

    fn publish_empty_cli_payload(&mut self) -> bool {
        let previous_output = self.last_output.clone();
        let payload = b"[]".to_vec();
        if self.accept_payload(payload, Source::Cli) {
            self.last_cli_fetch_seconds = Some(now_seconds());
        }
        self.last_output != previous_output
    }

    /// Re-serialize the current `provider_states.last_record` map (plus any
    /// existing serve payload entries for providers we haven't yet queried)
    /// into one aggregate JSON array and feed it to `accept_payload` as a
    /// CLI-source refresh. Preserves last-known-good data: a single provider
    /// success after a serve outage updates that provider's slice without
    /// blowing away the rest.
    fn publish_synthesized_cli_payload(&mut self, now: f64) -> bool {
        let eligible = self.eligible_provider_inventory();
        let eligible_set: std::collections::BTreeSet<&str> =
            eligible.iter().map(String::as_str).collect();
        let needs_seed = eligible.iter().any(|provider| {
            self.provider_states
                .get(provider)
                .is_none_or(|state| state.last_record.is_none() && !state.last_result_empty)
        });
        if needs_seed {
            // Seed any unqueried eligible providers from the existing payload so a
            // single per-provider success does not blow away the rest of the bar.
            // Providers outside the current inventory are intentionally ignored:
            // discovery is canonical, so disabled providers must not reappear.
            if let Some(payload) = self.last_payload.as_deref() {
                if let Ok(records) = parse_usage_payload(payload) {
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) {
                        if let Some(array) = value.as_array() {
                            for (record, value) in records.iter().zip(array.iter()) {
                                if !eligible_set.contains(record.provider.as_str()) {
                                    continue;
                                }
                                let entry = self
                                    .provider_states
                                    .entry(record.provider.clone())
                                    .or_default();
                                if entry.last_record.is_none() && !entry.last_result_empty {
                                    entry.last_record = Some(value.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        // Emit records in the eligible inventory's order so the bar's
        // provider sequence stays deterministic. Do not append leftovers:
        // anything outside `eligible` is stale, disabled, or excluded.
        let mut array: Vec<serde_json::Value> = Vec::new();
        for provider in eligible {
            if let Some(state) = self.provider_states.get(&provider) {
                if let Some(record) = state.last_record.as_ref() {
                    array.push(record.clone());
                }
            }
        }
        let bytes = match serde_json::to_vec(&serde_json::Value::Array(array)) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };
        let previous_output = self.last_output.clone();
        if self.accept_payload(bytes, Source::Cli) {
            self.last_cli_fetch_seconds = Some(now);
        }
        self.last_output != previous_output
    }

    fn expire_stale_provider_flights(&mut self, now: f64) {
        for state in self.provider_states.values_mut() {
            if !state.in_flight {
                continue;
            }
            let Some(started_at) = state.last_attempt_seconds else {
                continue;
            };
            if (now - started_at).max(0.0) < self.provider_failure_backoff_seconds {
                continue;
            }
            state.in_flight = false;
            state.active_attempt_token = None;
            state.last_failure_seconds = Some(now);
            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        }
    }

    fn has_provider_work_in_flight(&self) -> bool {
        self.provider_states.values().any(|state| state.in_flight)
    }

    fn all_provider_attempts_terminal(&self, providers: &[String]) -> bool {
        !providers.is_empty()
            && providers.iter().all(|provider| {
                self.provider_states.get(provider).is_some_and(|state| {
                    !state.in_flight
                        && state.last_record.is_none()
                        && state.last_failure_seconds.is_some()
                })
            })
    }

    fn render_cli_failure_if_all_terminal(&mut self) -> bool {
        if self.last_payload.is_some() {
            return false;
        }
        let providers = self.eligible_provider_inventory();
        if self.all_provider_attempts_terminal(&providers) {
            return self.render_cli_failure();
        }
        false
    }

    fn render_cli_failure(&mut self) -> bool {
        self.set_source(Source::Unavailable);
        let output = " showy-quota: CodexBar CLI unavailable ";
        if self.last_output == output {
            return false;
        }
        self.last_output = output.into();
        true
    }

    fn prune_last_payload_to_current_inventory(&mut self) {
        let Some(payload) = self.last_payload.as_deref() else {
            return;
        };
        let eligible = self.eligible_provider_inventory();
        let eligible_set: std::collections::BTreeSet<&str> =
            eligible.iter().map(String::as_str).collect();
        let Ok(records) = parse_usage_payload(payload) else {
            return;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
            return;
        };
        let Some(array) = value.as_array() else {
            return;
        };
        let mut pruned: Vec<serde_json::Value> = Vec::new();
        for (record, value) in records.iter().zip(array.iter()) {
            if eligible_set.contains(record.provider.as_str()) {
                pruned.push(value.clone());
            }
        }
        if pruned.len() == array.len() {
            return;
        }
        if let Ok(bytes) = serde_json::to_vec(&serde_json::Value::Array(pruned)) {
            self.last_payload = Some(bytes);
            self.refresh_output();
        }
    }

    fn accept_payload(&mut self, payload: Vec<u8>, source: Source) -> bool {
        let Ok(records) = parse_usage_payload(&payload) else {
            // Corrupt/invalid payload (e.g. a captive-portal or proxy page from
            // an otherwise-200 serve): surface the failure state but report
            // non-acceptance so the caller advances consecutive_serve_failures
            // and can fall back to the CLI instead of latching on bad data.
            self.render_failure();
            return false;
        };
        if source == Source::Serve
            && self.cli_fallback != CliFallback::Off
            && self.discovered_providers_at.is_some()
        {
            self.serve_inventory_mismatch = false;
            let payload_providers: std::collections::BTreeSet<&str> = records
                .iter()
                .map(|r| r.provider.as_str())
                .filter(|id| valid_provider_id(id))
                .collect();
            let discovered_providers: std::collections::BTreeSet<&str> = self
                .discovered_providers
                .iter()
                .map(|s| s.as_str())
                .filter(|id| valid_provider_id(id))
                .collect();
            if payload_providers != discovered_providers {
                self.serve_inventory_mismatch = true;
                return false;
            }
        }

        if !payload_has_renderable_provider(&records)
            && self.last_payload.is_some()
            && !(source == Source::Cli && records.is_empty())
        {
            return self.render_failure();
        }

        // When serve returns a fresh aggregate, refresh per-provider state so
        // a future degraded transition has a baseline of every record CodexBar
        // just published.
        if source == Source::Serve {
            self.seed_provider_states_from_payload(&records, &payload);
        }
        self.last_payload = Some(payload);
        self.last_success_seconds = Some(now_seconds());
        self.set_source(source);
        self.refresh_output();
        true
    }

    fn seed_provider_states_from_payload(&mut self, records: &[ProviderRecord], payload: &[u8]) {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
            return;
        };
        let Some(array) = value.as_array() else {
            return;
        };
        for (record, value) in records.iter().zip(array.iter()) {
            if !valid_provider_id(&record.provider) {
                continue;
            }
            let entry = self
                .provider_states
                .entry(record.provider.clone())
                .or_default();
            entry.last_record = Some(value.clone());
        }
    }

    fn render_failure(&mut self) -> bool {
        if self.last_payload.is_some() {
            return self.refresh_output();
        }
        let output = " showy-quota: CodexBar serve unavailable ";
        if self.last_output == output {
            return false;
        }
        self.last_output = output.into();
        true
    }

    fn refresh_output(&mut self) -> bool {
        let Some(payload) = self.last_payload.as_deref() else {
            return false;
        };
        let now = now_epoch();
        let now_seconds = now_seconds();
        let interval = match self.source {
            Source::Cli => self.cli_interval_seconds,
            _ => self.interval_seconds,
        };
        let stale = self
            .last_success_seconds
            .map(|seconds| (now_seconds - seconds).max(0.0) >= interval * 2.0)
            .unwrap_or(false);
        match render_zellij(
            payload,
            &self.render_config,
            RenderOptions {
                color: true,
                stale,
                degraded_cli: self.source == Source::Cli,
                now_epoch: now,
            },
        ) {
            Ok(output) => {
                let output = output.trim_end_matches(['\r', '\n']);
                let mut composed = output.to_string();
                if self.show_build_marker && self.serve_build_stale() {
                    composed.push(' ');
                    style_build_marker(
                        &mut composed,
                        BUILD_STALE_MARKER,
                        &self.render_config.palette_countdown_warn,
                        &self.render_config.palette_bg,
                    );
                }
                let changed = self.last_output != composed;
                if changed {
                    self.last_output = composed;
                }
                changed
            }
            Err(_) if self.last_output.is_empty() => {
                self.last_output = " showy-quota: invalid CodexBar JSON ".into();
                true
            }
            Err(_) => false,
        }
    }
}

/// Extract the first valid record from a per-provider CLI payload whose
/// provider id matches the requested one. Returns `Err` for malformed JSON,
/// invalid usage shape, non-array payloads, or mismatched provider ids.
/// Returns `Ok(None)` for a valid but empty per-provider payload.
fn extract_provider_record(
    payload: &[u8],
    provider: &str,
) -> Result<Option<serde_json::Value>, ()> {
    let records = parse_usage_payload(payload).map_err(|_| ())?;
    if records.iter().any(|record| record.provider != provider) {
        return Err(());
    }
    let value: serde_json::Value = serde_json::from_slice(payload).map_err(|_| ())?;
    let array = value.as_array().ok_or(())?;
    for (record, value) in records.iter().zip(array.iter()) {
        if record.provider == provider {
            return Ok(Some(value.clone()));
        }
    }
    Ok(None)
}

fn parse_positive_f64(value: Option<&str>, default: f64) -> f64 {
    value
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(default)
}

fn parse_nonnegative_f64(value: Option<&str>, default: f64) -> f64 {
    value
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(default)
}

// FNV-1a + SplitMix64 finalizer: a tiny, dependency-free, stable hash used to
// derive deterministic per-instance phases. Not for security; chosen over
// DefaultHasher because the latter's algorithm is not a stable contract.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Map a 64-bit hash to a uniform `[0, 1)` double (top 53 bits).
fn unit_from(seed: u64) -> f64 {
    (seed >> 11) as f64 / ((1u64 << 53) as f64)
}

/// Stable per-instance seed for jitter dispersion. The decisive input is the
/// Zellij `plugin_id` (a unique-per-instance pane id), salted with cwd / serve /
/// cli so distinct configs also differ. `\u{1}` field delimiters keep adjacent
/// fields from colliding (e.g. id `1` + cwd `2…` vs id `12` + cwd `…`). The
/// SplitMix64 finalizer decorrelates small consecutive plugin ids.
fn instance_seed(plugin_id: u32, cwd: &str, serve_url: &str, cli_command: &str) -> u64 {
    let seed_src = format!("{plugin_id}\u{1}{cwd}\u{1}{serve_url}\u{1}{cli_command}");
    splitmix64(fnv1a(seed_src.as_bytes()))
}

fn parse_bool(value: Option<&str>, default: bool) -> bool {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("1") | Some("true") | Some("yes") | Some("on") => true,
        Some("0") | Some("false") | Some("no") | Some("off") => false,
        _ => default,
    }
}

fn valid_port(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|ch| ch.is_ascii_digit())
        && matches!(value.parse::<u16>(), Ok(port) if port > 0)
}

// Defense-in-depth for the serve_command/cli_command KDL knobs: they are spawned
// via Zellij's RunCommand capability, so reject any value carrying whitespace or
// shell metacharacters (the documented injection vector) back to the default
// "codexbar". A bare name or plain path is accepted; the host resolves it.
fn valid_command(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+' | '@' | '/'))
}

/// Extract a comparable CodexBar version token: the first whitespace-separated
/// field that looks like a version (optional leading `v` then a digit), with the
/// `v` stripped. Mirrors the shell `codexbar_version_token` and glean's
/// `ParseCodexBarVersion` so all three agree. Returns None for a string with no
/// version-looking field (e.g. a transient bare `CodexBar`).
fn codexbar_version_token(raw: &str) -> Option<String> {
    raw.split_whitespace().find_map(|field| {
        let candidate = field.strip_prefix('v').unwrap_or(field);
        match candidate.chars().next() {
            Some(first) if first.is_ascii_digit() => Some(candidate.to_string()),
            _ => None,
        }
    })
}

/// Append an ANSI-styled marker mirroring the core `style_text` used for the
/// ⚠/⚠cli glyphs (bold, truecolor fg/bg, trailing reset), so a plugin-appended
/// marker matches the rendered bar without the core's private styling helpers.
fn style_build_marker(out: &mut String, glyph: &str, fg_hex: &str, bg_hex: &str) {
    let (fr, fg, fb) = hex_to_rgb(fg_hex);
    let (br, bg, bb) = hex_to_rgb(bg_hex);
    out.push_str("\x1b[1m");
    out.push_str(&format!("\x1b[38;2;{fr};{fg};{fb}m"));
    out.push_str(&format!("\x1b[48;2;{br};{bg};{bb}m"));
    out.push_str(glyph);
    out.push_str("\x1b[0m");
}

fn default_port_for_scheme(scheme: Option<&str>) -> Option<&'static str> {
    match scheme {
        Some(scheme) if scheme.eq_ignore_ascii_case("http") => Some("80"),
        Some(scheme) if scheme.eq_ignore_ascii_case("https") => Some("443"),
        _ => None,
    }
}

fn derive_port_from_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let (scheme, remainder) = match url.find("://") {
        Some(index) => (Some(&url[..index]), &url[index + 3..]),
        None => (None, url),
    };
    let authority = remainder
        .split_once('/')
        .map(|(authority, _)| authority)
        .unwrap_or(remainder);
    let authority = authority
        .rsplit_once('@')
        .map(|(_, authority)| authority)
        .unwrap_or(authority);

    if authority.is_empty() {
        return default_port_for_scheme(scheme).map(str::to_string);
    }

    if let Some(authority) = authority.strip_prefix('[') {
        let (_, rest) = authority.split_once(']')?;
        return match rest.strip_prefix(':') {
            Some(port) if valid_port(port) => Some(port.to_string()),
            Some(_) => None,
            None => default_port_for_scheme(scheme).map(str::to_string),
        };
    }

    match authority.rsplit_once(':').map(|(_, port)| port) {
        Some(port) if valid_port(port) => Some(port.to_string()),
        Some(_) => None,
        None => default_port_for_scheme(scheme).map(str::to_string),
    }
}

/// True when `url` is an `http(s)://` URL whose host is a loopback literal
/// (`127.0.0.1`, `localhost`, or `::1`). Mirrors the shell `serve_base_url`
/// regex so the plugin honors the same localhost-only serve contract.
fn is_loopback_serve_url(url: &str) -> bool {
    let rest = match url
        .trim()
        .strip_prefix("http://")
        .or_else(|| url.trim().strip_prefix("https://"))
    {
        Some(rest) => rest,
        None => return false,
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = authority
        .rsplit_once('@')
        .map(|(_, authority)| authority)
        .unwrap_or(authority);
    let host = if let Some(after) = authority.strip_prefix('[') {
        match after.split_once(']') {
            Some((host, _)) => host,
            None => return false,
        }
    } else {
        authority
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(authority)
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(target_arch = "wasm32")]
register_plugin!(State);

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;

    fn event_context(kind: &str) -> BTreeMap<String, String> {
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), kind.to_string());
        context
    }

    fn web_context(kind: &str, generation: u64) -> BTreeMap<String, String> {
        let mut context = event_context(kind);
        context.insert(
            WEB_REQUEST_GENERATION_KEY.to_string(),
            generation.to_string(),
        );
        context
    }

    fn current_web_context(state: &State, kind: &str) -> BTreeMap<String, String> {
        let generation = match kind {
            HEALTH_KIND => state.active_health_generation,
            USAGE_KIND => state.active_usage_generation,
            _ => None,
        }
        .expect("active web request generation");
        web_context(kind, generation)
    }

    fn arm_health_probe(state: &mut State, generation: u64) {
        state.health_generation = generation;
        state.active_health_generation = Some(generation);
        state.health_in_flight = true;
    }

    fn arm_usage_probe(state: &mut State, generation: u64) {
        state.usage_generation = generation;
        state.active_usage_generation = Some(generation);
        state.usage_in_flight = true;
    }

    fn provider_fallback_context(provider: &str) -> BTreeMap<String, String> {
        let mut context = event_context(FALLBACK_PROVIDER_KIND);
        context.insert(
            FALLBACK_PROVIDER_CONTEXT_KEY.to_string(),
            provider.to_string(),
        );
        context
    }

    fn provider_fallback_context_with_attempt(
        provider: &str,
        attempt: &str,
    ) -> BTreeMap<String, String> {
        let mut context = provider_fallback_context(provider);
        context.insert(
            FALLBACK_PROVIDER_ATTEMPT_KEY.to_string(),
            attempt.to_string(),
        );
        context
    }

    fn mixed_payload() -> Vec<u8> {
        include_bytes!("../../../test/fixtures/codexbar-mixed.json").to_vec()
    }

    fn provider_record(payload: &[u8], provider: &str) -> Vec<u8> {
        let value: serde_json::Value = serde_json::from_slice(payload).expect("valid fixture");
        let array = value.as_array().expect("fixture is array");
        let filtered: Vec<serde_json::Value> = array
            .iter()
            .filter(|record| {
                record
                    .as_object()
                    .and_then(|object| object.get("provider"))
                    .and_then(|value| value.as_str())
                    == Some(provider)
            })
            .cloned()
            .collect();
        serde_json::to_vec(&serde_json::Value::Array(filtered)).expect("re-serialize")
    }

    #[test]
    fn requested_permissions_follow_enabled_runtime_paths() {
        assert_eq!(
            requested_permissions(true, CliFallback::Degraded),
            vec![
                PermissionType::WebAccess,
                PermissionType::OpenTerminalsOrPlugins,
                PermissionType::RunCommands,
            ]
        );
        assert_eq!(
            requested_permissions(false, CliFallback::Degraded),
            vec![PermissionType::WebAccess, PermissionType::RunCommands]
        );
        assert_eq!(
            requested_permissions(true, CliFallback::Off),
            vec![
                PermissionType::WebAccess,
                PermissionType::OpenTerminalsOrPlugins,
            ]
        );
        assert_eq!(
            requested_permissions(false, CliFallback::Off),
            vec![PermissionType::WebAccess]
        );
    }

    #[test]
    fn load_derives_managed_serve_port_from_serve_url() {
        let mut configuration = BTreeMap::new();
        configuration.insert(
            "serve_url".to_string(),
            "http://127.0.0.1:58290".to_string(),
        );
        let mut state = State::default();

        state.load(configuration);

        assert_eq!(state.serve_url, "http://127.0.0.1:58290");
        assert_eq!(state.serve_port, "58290");
    }

    #[test]
    fn load_keeps_explicit_managed_serve_port_over_serve_url_port() {
        let mut configuration = BTreeMap::new();
        configuration.insert(
            "serve_url".to_string(),
            "http://127.0.0.1:58290".to_string(),
        );
        configuration.insert("serve_port".to_string(), "8080".to_string());
        let mut state = State::default();

        state.load(configuration);

        assert_eq!(state.serve_port, "8080");
    }

    #[test]
    fn derive_port_from_url_supports_standard_and_ipv6_urls() {
        assert_eq!(
            derive_port_from_url("http://127.0.0.1:58290"),
            Some("58290".to_string())
        );
        assert_eq!(
            derive_port_from_url("http://[::1]:58291/usage"),
            Some("58291".to_string())
        );
        assert_eq!(
            derive_port_from_url("https://localhost/"),
            Some("443".to_string())
        );
        assert_eq!(derive_port_from_url("http://localhost:99999"), None);
    }

    #[test]
    fn health_success_then_usage_failure_keeps_cli_degraded_output() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Cli,
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        assert!(state.refresh_output());
        assert!(state.last_output.contains("⚠cli"));
        arm_health_probe(&mut state, 1);
        let health_context = current_web_context(&state, HEALTH_KIND);

        assert!(!state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            Vec::new(),
            health_context,
        )));
        assert_eq!(state.source, Source::Cli);

        arm_usage_probe(&mut state, 1);
        let usage_context = current_web_context(&state, USAGE_KIND);
        assert!(!state.update(Event::WebRequestResult(
            503,
            BTreeMap::new(),
            Vec::new(),
            usage_context,
        )));
        assert_eq!(state.source, Source::Cli);
        assert!(state.last_output.contains("⚠cli"));
    }

    #[test]
    fn unchanged_timer_tick_does_not_request_render() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        assert!(state.refresh_output());

        assert!(!state.update(Event::Timer(0.0)));
    }

    #[test]
    fn unchanged_usage_success_still_resets_failure_counter() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            last_payload: Some(mixed_payload()),
            consecutive_serve_failures: SERVE_FAILURES_BEFORE_CLI - 1,
            ..State::default()
        };
        assert!(state.refresh_output());
        let output = state.last_output.clone();
        arm_usage_probe(&mut state, 1);
        let usage_context = current_web_context(&state, USAGE_KIND);

        assert!(!state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            mixed_payload(),
            usage_context,
        )));

        assert_eq!(state.consecutive_serve_failures, 0);
        assert_eq!(state.source, Source::Serve);
        assert_eq!(state.last_output, output);
    }

    #[test]
    fn corrupt_serve_payload_advances_failure_counter() {
        // A 200 response carrying invalid JSON (captive portal / proxy page)
        // must not count as success: the failure counter has to advance so the
        // plugin eventually falls back to the CLI instead of latching forever.
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            cli_fallback: CliFallback::Off,
            ..State::default()
        };
        arm_usage_probe(&mut state, 1);
        let usage_context = current_web_context(&state, USAGE_KIND);
        let before = state.consecutive_serve_failures;
        state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            b"not-json".to_vec(),
            usage_context,
        ));
        assert_eq!(state.consecutive_serve_failures, before + 1);
    }

    #[test]
    fn hung_usage_web_request_expires_and_advances_failure() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            cli_fallback: CliFallback::Off,
            usage_in_flight: true,
            web_flight_started_at: Some(now_seconds() - (SERVE_USAGE_TIMEOUT_SECONDS + 1.0)),
            ..State::default()
        };
        let before = state.consecutive_serve_failures;
        state.tick();
        assert!(!state.usage_in_flight);
        assert!(state.web_flight_started_at.is_none());
        assert_eq!(state.consecutive_serve_failures, before + 1);
    }

    #[test]
    fn stale_usage_response_after_timeout_does_not_clear_newer_flight() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            cli_fallback: CliFallback::Off,
            usage_in_flight: true,
            usage_generation: 1,
            active_usage_generation: Some(1),
            web_flight_started_at: Some(now_seconds() - (SERVE_USAGE_TIMEOUT_SECONDS + 1.0)),
            ..State::default()
        };

        assert!(state.expire_stale_web_flight());
        assert!(!state.usage_in_flight);
        state.kick_usage();
        assert_eq!(state.active_usage_generation, Some(2));

        assert!(!state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            mixed_payload(),
            web_context(USAGE_KIND, 1),
        )));
        assert!(state.usage_in_flight);
        assert_eq!(state.active_usage_generation, Some(2));
        assert!(state.last_payload.is_none());
    }

    #[test]
    fn stale_health_response_after_timeout_does_not_clear_newer_probe() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Probing,
            manage_serve: false,
            cli_fallback: CliFallback::Off,
            health_in_flight: true,
            health_generation: 1,
            active_health_generation: Some(1),
            web_flight_started_at: Some(now_seconds() - (SERVE_HEALTH_TIMEOUT_SECONDS + 1.0)),
            ..State::default()
        };

        assert!(state.expire_stale_web_flight());
        assert!(!state.health_in_flight);
        state.kick_health_probe();
        assert_eq!(state.active_health_generation, Some(2));

        assert!(!state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            br#"{"status":"ok","version":"0.37.2"}"#.to_vec(),
            web_context(HEALTH_KIND, 1),
        )));
        assert!(state.health_in_flight);
        assert_eq!(state.active_health_generation, Some(2));
        assert!(state.serve_build_version.is_none());
    }
    #[test]
    fn fresh_usage_web_request_is_not_expired() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            cli_fallback: CliFallback::Off,
            usage_in_flight: true,
            web_flight_started_at: Some(now_seconds()),
            ..State::default()
        };
        state.tick();
        assert!(state.usage_in_flight);
        assert!(state.web_flight_started_at.is_some());
    }
    #[test]
    fn usage_probe_survives_the_health_timeout_window() {
        // A /usage probe must get the longer usage budget, not the short health
        // window: still in flight at health-timeout + 1s, well under the usage
        // timeout, so the bounded partial response is not abandoned.
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            cli_fallback: CliFallback::Off,
            usage_in_flight: true,
            web_flight_started_at: Some(now_seconds() - (SERVE_HEALTH_TIMEOUT_SECONDS + 1.0)),
            ..State::default()
        };
        state.tick();
        assert!(state.usage_in_flight);
        assert!(state.web_flight_started_at.is_some());
    }

    #[test]
    fn hung_health_probe_expires_at_the_short_health_timeout() {
        // A /health probe expires on the short window so an unreachable serve is
        // abandoned quickly instead of latching for the full usage budget.
        let mut state = State {
            permissions_granted: true,
            source: Source::Unknown,
            cli_fallback: CliFallback::Off,
            health_in_flight: true,
            web_flight_started_at: Some(now_seconds() - (SERVE_HEALTH_TIMEOUT_SECONDS + 1.0)),
            ..State::default()
        };
        state.tick();
        assert!(!state.health_in_flight);
        assert!(state.web_flight_started_at.is_none());
    }

    #[test]
    fn parse_bool_accepts_known_truthy_and_falsy_tokens() {
        for truthy in ["1", "true", "yes", "on", "ON", " Yes "] {
            assert!(parse_bool(Some(truthy), false), "{truthy:?} should be true");
        }
        for falsy in ["0", "false", "no", "off", "OFF"] {
            assert!(!parse_bool(Some(falsy), true), "{falsy:?} should be false");
        }
        assert!(parse_bool(None, true));
        assert!(!parse_bool(None, false));
        assert!(!parse_bool(Some("maybe"), false));
    }

    #[test]
    fn valid_port_rejects_out_of_range_and_nonnumeric() {
        assert!(valid_port("8080"));
        assert!(valid_port("1"));
        assert!(valid_port("65535"));
        assert!(!valid_port("0"));
        assert!(!valid_port("65536"));
        assert!(!valid_port("abc"));
        assert!(!valid_port(""));
        assert!(!valid_port("-1"));
    }

    #[test]
    fn valid_command_rejects_shell_metacharacters_and_whitespace() {
        assert!(valid_command("codexbar"));
        assert!(valid_command("/usr/local/bin/codexbar"));
        assert!(valid_command("my-tool.v2_beta"));
        assert!(!valid_command(""));
        assert!(!valid_command("/bin/sh -c evil"));
        assert!(!valid_command("codexbar; rm -rf /"));
        assert!(!valid_command("$(curl evil)"));
        assert!(!valid_command("a`b`"));
    }

    #[test]
    fn prune_last_payload_drops_excluded_providers() {
        let mut state = State {
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        state.render_config.providers_exclude = vec!["cursor".to_string()];
        state.prune_last_payload_to_current_inventory();
        let payload = state.last_payload.as_deref().expect("payload retained");
        let records = parse_usage_payload(payload).expect("valid pruned payload");
        let ids = provider_ids_from_records(&records);
        assert_eq!(ids, vec!["claude", "codex", "gemini"]);
        assert!(!ids.iter().any(|id| id == "cursor"));
    }

    #[test]
    fn is_loopback_serve_url_matches_localhost_only() {
        assert!(is_loopback_serve_url("http://127.0.0.1:8080"));
        assert!(is_loopback_serve_url("http://localhost:8080/"));
        assert!(is_loopback_serve_url("http://[::1]:8080"));
        assert!(is_loopback_serve_url("https://127.0.0.1"));
        assert!(!is_loopback_serve_url("http://169.254.169.254/"));
        assert!(!is_loopback_serve_url(
            "https://internal-service.corp/secret"
        ));
        assert!(!is_loopback_serve_url("http://127.0.0.1.evil.com"));
        assert!(!is_loopback_serve_url("ftp://127.0.0.1"));
        assert!(!is_loopback_serve_url(""));
    }

    #[test]
    fn load_drops_non_loopback_serve_url() {
        let mut state = State::default();
        let mut config = BTreeMap::new();
        config.insert(
            "serve_url".to_string(),
            "https://internal-service.corp/secret".to_string(),
        );
        state.load(config);
        assert!(state.serve_url.is_empty());
    }

    #[test]
    fn load_keeps_loopback_serve_url() {
        let mut state = State::default();
        let mut config = BTreeMap::new();
        config.insert("serve_url".to_string(), "http://127.0.0.1:9000".to_string());
        state.load(config);
        assert_eq!(state.serve_url, "http://127.0.0.1:9000");
    }

    #[test]
    fn timer_reports_synchronous_failure_render() {
        let mut state = State {
            permissions_granted: true,
            serve_url: String::new(),
            cli_fallback: CliFallback::Off,
            ..State::default()
        };

        assert!(state.update(Event::Timer(0.0)));
        assert_eq!(state.source, Source::Unavailable);
        assert!(state.last_output.contains("serve unavailable"));
    }
    #[test]
    fn late_per_provider_result_is_ignored_after_serve_recovery() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Cli,
            ..State::default()
        };
        // Seed a Cli payload so the initial output carries the degraded marker.
        state
            .provider_states
            .entry("codex".into())
            .or_default()
            .in_flight = true;
        state.last_payload = Some(mixed_payload());
        assert!(state.refresh_output());
        assert!(state.last_output.contains("⚠cli"));

        assert!(state.accept_payload(mixed_payload(), Source::Serve));
        let serve_output = state.last_output.clone();
        assert!(!serve_output.contains("⚠cli"));

        // A late per-provider result that arrives after serve has recovered
        // must not regress the bar back to degraded output.
        assert!(!state.update(Event::RunCommandResult(
            Some(0),
            provider_record(&mixed_payload(), "codex"),
            Vec::new(),
            provider_fallback_context("codex"),
        )));
        assert_eq!(state.source, Source::Serve);
        assert_eq!(state.last_output, serve_output);
    }

    #[test]
    fn accept_payload_drift_handling() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            ..State::default()
        };

        // 1. Exact match is accepted and discovery remains valid.
        state.discovered_providers = vec![
            "claude".to_string(),
            "codex".to_string(),
            "gemini".to_string(),
            "cursor".to_string(),
        ];
        state.discovered_providers_at = Some(now_seconds());
        assert!(state.accept_payload(mixed_payload(), Source::Serve));
        assert!(state.discovered_providers_at.is_some());

        // 2. Superset is rejected (serve has stale disabled providers) and
        // discovery remains valid so fallback can query the canonical set.
        state.discovered_providers = vec!["claude".to_string(), "codex".to_string()];
        state.discovered_providers_at = Some(now_seconds());
        assert!(!state.accept_payload(mixed_payload(), Source::Serve));
        assert!(state.discovered_providers_at.is_some());

        // 3. Subset is rejected (serve missing expected providers) and
        // discovery remains valid so fallback can query the expected providers.
        state.discovered_providers = vec![
            "claude".to_string(),
            "codex".to_string(),
            "gemini".to_string(),
            "cursor".to_string(),
            "antigravity".to_string(),
        ];
        state.discovered_providers_at = Some(now_seconds());
        assert!(!state.accept_payload(mixed_payload(), Source::Serve));
        assert!(state.discovered_providers_at.is_some());

        // 4. Canonical empty inventory rejects stale non-empty serve payloads.
        state.discovered_providers = Vec::new();
        state.discovered_providers_at = Some(now_seconds());
        assert!(!state.accept_payload(mixed_payload(), Source::Serve));
        assert!(state.discovered_providers_at.is_some());
    }

    #[test]
    fn serve_inventory_rejection_launches_provider_fallback() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            discovered_providers: vec!["claude".to_string(), "antigravity".to_string()],
            discovered_providers_at: Some(now_seconds()),
            last_payload: Some(mixed_payload()),
            ..State::default()
        };

        assert!(!state.accept_payload(mixed_payload(), Source::Serve));
        state.handle_usage_failure();

        assert!(!state.discovery_in_flight);
        assert!(state
            .provider_states
            .get("claude")
            .is_some_and(|provider| provider.in_flight));
        assert!(state
            .provider_states
            .get("antigravity")
            .is_some_and(|provider| provider.in_flight));

        let claude_attempt = state
            .provider_states
            .get("claude")
            .and_then(|provider| provider.active_attempt_token.clone())
            .expect("claude attempt token");

        assert!(state.update(Event::RunCommandResult(
            Some(0),
            provider_record(&mixed_payload(), "claude"),
            Vec::new(),
            provider_fallback_context_with_attempt("claude", &claude_attempt),
        )));
        assert_eq!(state.source, Source::Cli);
        assert!(state.last_output.contains("⚠cli"));
    }

    #[test]
    fn empty_inventory_rejection_publishes_idle_payload() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            discovered_providers: Vec::new(),
            discovered_providers_at: Some(now_seconds()),
            last_payload: Some(mixed_payload()),
            ..State::default()
        };

        assert!(!state.accept_payload(mixed_payload(), Source::Serve));
        state.handle_usage_failure();

        assert_eq!(state.last_payload.as_deref(), Some(b"[]".as_ref()));
        assert_eq!(state.source, Source::Cli);
    }

    #[test]
    fn health_success_waits_for_discovery_before_usage_poll() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Probing,
            health_in_flight: true,
            ..State::default()
        };
        state.active_health_generation = Some(1);
        state.health_generation = 1;
        let health_context = current_web_context(&state, HEALTH_KIND);

        assert!(!state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            Vec::new(),
            health_context,
        )));

        assert!(state.discovery_in_flight);
        assert!(state.usage_after_discovery);
        assert!(!state.usage_in_flight);

        let attempt = state
            .discovery_attempt_token
            .clone()
            .expect("discovery attempt");
        state.handle_discovery_result(
            Some(0),
            br#"[{"provider":"claude","enabled":true}]"#.to_vec(),
            Some(&attempt),
        );
        assert!(!state.discovery_in_flight);
        assert!(!state.usage_after_discovery);
        assert!(state.usage_in_flight);
    }

    #[test]
    fn stale_discovery_result_is_ignored() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            discovery_in_flight: true,
            discovery_attempt_token: Some("new".to_string()),
            discovered_providers: vec!["claude".to_string()],
            ..State::default()
        };

        state.handle_discovery_result(
            Some(0),
            br#"[{"provider":"codex","enabled":true}]"#.to_vec(),
            Some("old"),
        );

        assert!(state.discovery_in_flight);
        assert_eq!(state.discovered_providers, vec!["claude".to_string()]);
    }

    #[test]
    fn discovery_failure_resume_does_not_validate_against_stale_inventory() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            discovery_in_flight: true,
            usage_after_discovery: true,
            discovered_providers: vec!["claude".to_string(), "antigravity".to_string()],
            discovered_providers_at: Some(now_seconds() - 120.0),
            ..State::default()
        };

        state.handle_discovery_result(Some(7), Vec::new(), None);

        assert!(state.discovered_providers_at.is_none());
        assert!(state.usage_in_flight);
    }

    #[test]
    fn stale_discovery_in_flight_expires_before_serve_tick() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            discovery_in_flight: true,
            discovered_providers: vec!["claude".to_string(), "antigravity".to_string()],
            discovered_providers_at: Some(now_seconds() - 120.0),
            usage_after_discovery: true,
            discovery_started_at: Some(now_seconds() - 120.0),
            discovery_failure_backoff_seconds: 60.0,
            ..State::default()
        };

        state.tick();

        assert!(!state.discovery_in_flight);
        assert!(!state.usage_after_discovery);
        assert!(state.discovered_providers_at.is_none());
        assert!(state.discovery_failed_at.is_some());
        assert!(state.usage_in_flight);
    }

    #[test]
    fn per_provider_result_after_usage_failure_is_accepted_while_probing() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Probing,
            ..State::default()
        };
        state
            .provider_states
            .entry("codex".into())
            .or_default()
            .in_flight = true;

        assert!(state.update(Event::RunCommandResult(
            Some(0),
            provider_record(&mixed_payload(), "codex"),
            Vec::new(),
            provider_fallback_context("codex"),
        )));

        assert_eq!(state.source, Source::Cli);
        assert!(state.last_payload.is_some());
        assert!(state.last_output.contains("⚠cli"));
        assert!(state.last_output.contains("CX"));
    }

    #[test]
    fn cli_tick_defers_fallback_while_health_probe_is_in_flight() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Cli,
            health_in_flight: true,
            ..State::default()
        };

        state.tick();

        assert!(state.provider_states.values().all(|s| !s.in_flight));
        assert!(!state.discovery_in_flight);
    }

    #[test]
    fn cli_tick_waits_for_discovery_before_cache_derived_fallback() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Cli,
            serve_url: String::new(),
            last_payload: Some(mixed_payload()),
            ..State::default()
        };

        state.tick();

        assert!(state.discovery_in_flight);
        assert!(state.provider_states.is_empty());
    }

    #[test]
    fn serve_tick_waits_for_discovery_before_usage_poll() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            last_payload: Some(mixed_payload()),
            discovered_providers: vec!["claude".to_string(), "antigravity".to_string()],
            discovered_providers_at: None,
            ..State::default()
        };

        state.tick();

        assert!(state.discovery_in_flight);
        assert!(!state.usage_in_flight);
    }

    #[test]
    fn serve_only_tick_does_not_start_discovery_without_run_command_permission() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            cli_fallback: CliFallback::Off,
            ..State::default()
        };

        state.tick();

        assert!(!state.discovery_in_flight);
        assert!(state.usage_in_flight);
    }

    #[test]
    fn managed_serve_retry_rearms_after_cli_or_unavailable_transition() {
        let mut state = State {
            managed_serve_requested: true,
            managed_serve_last_attempt_seconds: Some(100.0),
            ..State::default()
        };

        state.set_source(Source::Cli);
        assert!(!state.managed_serve_requested);
        assert!(!state.should_start_managed_serve(129.9));
        assert!(state.should_start_managed_serve(130.0));

        state.managed_serve_requested = true;
        state.set_source(Source::Unavailable);
        assert!(!state.managed_serve_requested);
    }

    #[test]
    fn discovery_inventory_drives_per_provider_eligibility() {
        let mut state = State::default();
        state.discovered_providers = vec![
            "codex".to_string(),
            "claude".to_string(),
            "antigravity".to_string(),
        ];
        state.discovered_providers_at = Some(0.0);
        let providers = state.eligible_provider_inventory();
        assert_eq!(providers, vec!["codex", "claude", "antigravity"]);
    }

    #[test]
    fn providers_exclude_prunes_inventory_before_per_provider_calls() {
        let mut state = State::default();
        state.discovered_providers = vec![
            "codex".to_string(),
            "claude".to_string(),
            "antigravity".to_string(),
        ];
        state.discovered_providers_at = Some(0.0);
        state.render_config.providers_exclude = vec!["antigravity".to_string()];
        let providers = state.eligible_provider_inventory();
        assert_eq!(providers, vec!["codex", "claude"]);
    }

    #[test]
    fn successful_discovery_refreshes_after_backoff_window() {
        let now = now_seconds();
        let mut state = State {
            discovery_failure_backoff_seconds: 60.0,
            discovered_providers_at: Some(now),
            ..State::default()
        };
        assert!(!state.needs_discovery());

        state.discovered_providers_at = Some(now - 61.0);
        assert!(state.needs_discovery());
    }

    #[test]
    fn providers_allow_list_filters_discovered_inventory() {
        let mut state = State::default();
        state.discovered_providers = vec![
            "codex".to_string(),
            "claude".to_string(),
            "antigravity".to_string(),
        ];
        state.discovered_providers_at = Some(0.0);
        state.render_config.providers = vec!["claude".to_string()];
        let providers = state.eligible_provider_inventory();
        assert_eq!(providers, vec!["claude"]);
    }

    #[test]
    fn provider_order_promotes_listed_providers_to_the_front() {
        let mut state = State::default();
        state.discovered_providers = vec![
            "antigravity".to_string(),
            "claude".to_string(),
            "codex".to_string(),
        ];
        state.discovered_providers_at = Some(0.0);
        state.render_config.provider_order = vec!["codex".to_string(), "claude".to_string()];
        let providers = state.eligible_provider_inventory();
        assert_eq!(providers, vec!["codex", "claude", "antigravity"]);
    }

    #[test]
    fn provider_inventory_falls_back_to_cache_ids_when_discovery_missing() {
        let state = State {
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        let providers = state.eligible_provider_inventory();
        // mixed fixture: claude, codex, gemini, cursor (all valid ids).
        assert!(providers.contains(&"claude".to_string()));
        assert!(providers.contains(&"codex".to_string()));
        assert!(providers.contains(&"gemini".to_string()));
    }

    #[test]
    fn provider_failure_backoff_blocks_repeat_within_window() {
        let mut state = State {
            provider_failure_backoff_seconds: 60.0,
            ..State::default()
        };
        let now = 1_000.0;
        state.record_provider_failure("codex", now);
        assert!(state.provider_in_flight_or_backoff("codex", now + 30.0));
        assert!(!state.provider_in_flight_or_backoff("codex", now + 61.0));
    }

    #[test]
    fn provider_backoff_escalates_on_consecutive_failures() {
        let mut state = State {
            provider_failure_backoff_seconds: 60.0,
            ..State::default()
        };
        let now = 1_000.0;
        // Two consecutive failures double the retry window to 120s.
        state.record_provider_failure("codex", now);
        state.record_provider_failure("codex", now);
        assert!(state.provider_in_flight_or_backoff("codex", now + 90.0));
        assert!(!state.provider_in_flight_or_backoff("codex", now + 121.0));
        // A success clears the escalation back to the base window.
        let entry = state.provider_states.get_mut("codex").unwrap();
        entry.consecutive_failures = 0;
        entry.last_failure_seconds = Some(now);
        assert!(!state.provider_in_flight_or_backoff("codex", now + 61.0));
    }

    #[test]
    fn watchdog_argv_wraps_command_without_interpolation() {
        let script = watchdog_script(15);
        let argv = watchdog_argv(&script, &["codexbar", "usage", "--provider", "claude"]);
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert_eq!(argv[2], script.as_str());
        assert_eq!(&argv[4..], &["codexbar", "usage", "--provider", "claude"]);
        assert!(script.contains("sleep 15"));
        assert!(script.contains("kill -9"));
        // Provider ids and the binary path ride in "$@", never the shell string.
        assert!(!script.contains("claude"));
    }

    #[test]
    fn provider_in_flight_blocks_duplicate_spawn() {
        let mut state = State::default();
        state
            .provider_states
            .entry("codex".into())
            .or_default()
            .in_flight = true;
        assert!(state.provider_in_flight_or_backoff("codex", 0.0));
    }

    #[test]
    fn one_provider_failure_does_not_blow_away_others() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Cli,
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        state
            .provider_states
            .entry("codex".into())
            .or_default()
            .in_flight = true;
        state
            .provider_states
            .entry("claude".into())
            .or_default()
            .in_flight = true;

        // codex returns a fresh record → it lands.
        assert!(state.update(Event::RunCommandResult(
            Some(0),
            provider_record(&mixed_payload(), "codex"),
            Vec::new(),
            provider_fallback_context("codex"),
        )));
        // claude's CLI call fails → its slot must keep the seeded record.
        assert!(!state.update(Event::RunCommandResult(
            Some(7),
            Vec::new(),
            Vec::new(),
            provider_fallback_context("claude"),
        )));
        assert!(state
            .provider_states
            .get("claude")
            .and_then(|state| state.last_record.as_ref())
            .is_some());
        assert!(state.last_output.contains("CX"));
        assert!(state.last_output.contains("CL"));
    }

    #[test]
    fn canonical_empty_inventory_publishes_idle_payload() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Cli,
            discovered_providers: Vec::new(),
            discovered_providers_at: Some(now_seconds()),
            cli_fallback: CliFallback::Degraded,
            ..State::default()
        };
        // serve_url empty so kick_cli_fallback runs the direct path instead
        // of probing serve health first.
        state.serve_url.clear();
        state.kick_cli_fallback();
        assert_eq!(state.last_payload.as_deref(), Some(b"[]".as_ref()));
        assert_eq!(state.source, Source::Cli);
    }

    #[test]
    fn canonical_empty_inventory_overrides_stale_payload() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Cli,
            discovered_providers: Vec::new(),
            discovered_providers_at: Some(now_seconds()),
            last_payload: Some(mixed_payload()),
            cli_fallback: CliFallback::Degraded,
            ..State::default()
        };
        state.serve_url.clear();

        state.kick_cli_fallback();

        assert_eq!(state.last_payload.as_deref(), Some(b"[]".as_ref()));
        assert_eq!(state.source, Source::Cli);
    }

    #[test]
    fn discovery_failure_falls_back_to_cache_inventory() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Cli,
            last_payload: Some(mixed_payload()),
            cli_fallback: CliFallback::Degraded,
            ..State::default()
        };
        state.serve_url.clear();
        // Simulate a malformed discovery payload → records a failure stamp.
        state.handle_discovery_result(Some(0), b"{\"providers\":[]}".to_vec(), None);
        assert!(state.discovery_failed_at.is_some());
        // Without discovery, the eligible inventory comes from cache ids.
        let providers = state.eligible_provider_inventory();
        assert!(providers.contains(&"claude".to_string()));
        assert!(providers.contains(&"codex".to_string()));
    }

    #[test]
    fn discovery_drops_disabled_provider_state() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Cli,
            ..State::default()
        };
        state.serve_url.clear();
        // Pre-seed a provider that the next discovery payload will not list.
        state
            .provider_states
            .entry("antigravity".to_string())
            .or_default()
            .last_record = Some(serde_json::json!({"provider": "antigravity"}));
        // Discovery payload reports only claude as enabled.
        let payload = br#"[
            {"provider": "claude", "enabled": true}
        ]"#
        .to_vec();
        state.handle_discovery_result(Some(0), payload, None);
        assert!(!state.provider_states.contains_key("antigravity"));
        assert_eq!(state.discovered_providers, vec!["claude".to_string()]);
    }

    #[test]
    fn discovery_prunes_disabled_providers_from_cached_payload() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Cli,
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        state.serve_url.clear();
        let payload = br#"[
            {"provider": "claude", "enabled": true}
        ]"#
        .to_vec();

        state.handle_discovery_result(Some(0), payload, None);

        let payload = state.last_payload.as_deref().expect("payload");
        let records = parse_usage_payload(payload).expect("valid payload");
        assert_eq!(provider_ids_from_records(&records), vec!["claude"]);
    }

    #[test]
    fn synthesized_payload_does_not_reintroduce_disabled_cached_providers() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Cli,
            discovered_providers: vec!["claude".to_string()],
            discovered_providers_at: Some(now_seconds()),
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        state
            .provider_states
            .entry("claude".into())
            .or_default()
            .in_flight = true;

        assert!(state.update(Event::RunCommandResult(
            Some(0),
            provider_record(&mixed_payload(), "claude"),
            Vec::new(),
            provider_fallback_context("claude"),
        )));

        let payload = state.last_payload.as_deref().expect("payload");
        let records = parse_usage_payload(payload).expect("valid payload");
        assert_eq!(provider_ids_from_records(&records), vec!["claude"]);
    }

    #[test]
    fn malformed_provider_record_fails_without_poisoning_payload() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Cli,
            discovered_providers: vec!["codex".to_string()],
            discovered_providers_at: Some(now_seconds()),
            ..State::default()
        };
        state
            .provider_states
            .entry("codex".into())
            .or_default()
            .in_flight = true;

        assert!(state.update(Event::RunCommandResult(
            Some(0),
            br#"[{"provider":"codex","usage":{"primary":{"usedPercent":"bad"}}}]"#.to_vec(),
            Vec::new(),
            provider_fallback_context("codex"),
        )));

        assert!(state.last_payload.is_none());
        assert!(state.last_output.contains("CodexBar CLI unavailable"));
        assert!(state
            .provider_states
            .get("codex")
            .and_then(|state| state.last_failure_seconds)
            .is_some());
    }

    #[test]
    fn empty_provider_result_removes_stale_cached_record() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Cli,
            discovered_providers: vec!["codex".to_string()],
            discovered_providers_at: Some(now_seconds()),
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        state
            .provider_states
            .entry("codex".into())
            .or_default()
            .in_flight = true;

        assert!(state.update(Event::RunCommandResult(
            Some(0),
            b"[]".to_vec(),
            Vec::new(),
            provider_fallback_context("codex"),
        )));

        assert_eq!(state.last_payload.as_deref(), Some(b"[]".as_ref()));
        assert!(state
            .provider_states
            .get("codex")
            .is_some_and(|state| state.last_result_empty));
    }

    #[test]
    fn expired_provider_command_is_failed_and_late_result_ignored() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Cli,
            provider_failure_backoff_seconds: 1.0,
            discovered_providers: vec!["codex".to_string()],
            discovered_providers_at: Some(now_seconds()),
            ..State::default()
        };
        let entry = state.provider_states.entry("codex".into()).or_default();
        entry.in_flight = true;
        entry.last_attempt_seconds = Some(0.0);
        entry.active_attempt_token = Some("old".to_string());

        state.expire_stale_provider_flights(2.0);

        let entry = state.provider_states.get("codex").expect("provider state");
        assert!(!entry.in_flight);
        assert!(entry.last_failure_seconds.is_some());
        assert!(entry.active_attempt_token.is_none());

        assert!(!state.update(Event::RunCommandResult(
            Some(0),
            provider_record(&mixed_payload(), "codex"),
            Vec::new(),
            provider_fallback_context_with_attempt("codex", "old"),
        )));
        assert!(state.last_payload.is_none());
    }

    #[test]
    fn late_result_after_newer_success_is_ignored() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Cli,
            provider_failure_backoff_seconds: 1.0,
            discovered_providers: vec!["codex".to_string()],
            discovered_providers_at: Some(now_seconds()),
            ..State::default()
        };
        // Attempt "a" is kicked, then its token is wiped while the command is
        // still running (PermissionRequestResult -> clear_all_provider_in_flight).
        let entry = state.provider_states.entry("codex".into()).or_default();
        entry.in_flight = true;
        entry.active_attempt_token = Some("a".to_string());
        state.clear_all_provider_in_flight();

        // Attempt "b" is kicked and succeeds; the provider now has fresh data
        // and, crucially, no recorded failure.
        let entry = state.provider_states.entry("codex".into()).or_default();
        entry.in_flight = true;
        entry.active_attempt_token = Some("b".to_string());
        assert!(state.update(Event::RunCommandResult(
            Some(0),
            provider_record(&mixed_payload(), "codex"),
            Vec::new(),
            provider_fallback_context_with_attempt("codex", "b"),
        )));
        let fresh_payload = state.last_payload.clone();
        assert!(fresh_payload.is_some());
        let fresh_record = state
            .provider_states
            .get("codex")
            .and_then(|entry| entry.last_record.clone());
        assert!(fresh_record.is_some());

        // Attempt "a"'s delayed result must be rejected outright — before this
        // guard, the missing last_failure_seconds let it overwrite "b"'s
        // fresher record with stale data.
        assert!(!state.update(Event::RunCommandResult(
            Some(0),
            b"[]".to_vec(),
            Vec::new(),
            provider_fallback_context_with_attempt("codex", "a"),
        )));
        assert_eq!(state.last_payload, fresh_payload);
        assert_eq!(
            state
                .provider_states
                .get("codex")
                .and_then(|entry| entry.last_record.clone()),
            fresh_record
        );
        // The rejected result must not disturb scheduling state either: no
        // failure recorded, nothing in flight, so the next tick may re-kick.
        let entry = state.provider_states.get("codex").expect("provider state");
        assert!(!entry.in_flight);
        assert!(entry.last_failure_seconds.is_none());
    }

    #[test]
    fn extract_provider_record_matches_requested_provider() {
        let payload = provider_record(&mixed_payload(), "codex");
        let record = extract_provider_record(&payload, "codex")
            .expect("valid payload")
            .expect("record");
        assert_eq!(
            record.get("provider").and_then(|v| v.as_str()),
            Some("codex")
        );
    }

    #[test]
    fn extract_provider_record_rejects_mismatch() {
        let payload = br#"[
            {"provider": "claude"}
        ]"#;
        assert!(extract_provider_record(payload, "codex").is_err());
    }

    #[test]
    fn extract_provider_record_accepts_empty_provider_payload() {
        assert!(matches!(extract_provider_record(b"[]", "codex"), Ok(None)));
    }

    #[test]
    fn codexbar_version_token_truth_table() {
        assert_eq!(codexbar_version_token("0.37.2").as_deref(), Some("0.37.2"));
        assert_eq!(
            codexbar_version_token("CodexBar 0.37.2").as_deref(),
            Some("0.37.2")
        );
        assert_eq!(codexbar_version_token("CodexBar"), None);
        assert_eq!(codexbar_version_token("v0.37.1").as_deref(), Some("0.37.1"));
        assert_eq!(
            codexbar_version_token("CodexBar v0.37.1").as_deref(),
            Some("0.37.1")
        );
        assert_eq!(
            codexbar_version_token("0.37.1 (build abc)").as_deref(),
            Some("0.37.1")
        );
        assert_eq!(codexbar_version_token(""), None);
    }

    #[test]
    fn serve_build_version_parsed_from_health_body() {
        let mut state = State::default();
        state.update_serve_build_version(br#"{"status":"ok","version":"0.37.2"}"#);
        assert_eq!(state.serve_build_version.as_deref(), Some("0.37.2"));
        // A pre-#1703 serve omits the field -> None -> gate inert.
        state.update_serve_build_version(br#"{"status":"ok"}"#);
        assert_eq!(state.serve_build_version, None);
        // A prefixed /health value normalizes the same as --version.
        state.update_serve_build_version(br#"{"version":"CodexBar 0.37.2"}"#);
        assert_eq!(state.serve_build_version.as_deref(), Some("0.37.2"));
    }

    #[test]
    fn health_version_does_not_disturb_serve_source() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        arm_health_probe(&mut state, 1);
        let health_context = current_web_context(&state, HEALTH_KIND);
        state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            br#"{"status":"ok","version":"0.37.2"}"#.to_vec(),
            health_context,
        ));
        assert_eq!(state.serve_build_version.as_deref(), Some("0.37.2"));
        assert_eq!(state.source, Source::Serve);
        assert_eq!(state.consecutive_serve_failures, 0);
    }

    #[test]
    fn serve_build_stale_truth_table() {
        let stale = |source: Source, sv: Option<&str>, ov: Option<&str>| -> bool {
            State {
                source,
                serve_build_version: sv.map(String::from),
                ondisk_version: ov.map(String::from),
                ..State::default()
            }
            .serve_build_stale()
        };
        assert!(stale(Source::Serve, Some("0.37.1"), Some("0.37.2")));
        assert!(!stale(Source::Serve, Some("0.37.2"), Some("0.37.2")));
        assert!(!stale(Source::Serve, Some("0.37.1"), None));
        assert!(!stale(Source::Serve, None, Some("0.37.2")));
        // Never flag on non-serve output even when versions differ.
        assert!(!stale(Source::Cli, Some("0.37.1"), Some("0.37.2")));
    }

    #[test]
    fn build_marker_appended_only_when_stale_on_serve() {
        // Mismatch on serve data -> marker present and is the final token.
        // (default config has the marker off; these cases opt in.)
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            last_payload: Some(mixed_payload()),
            serve_build_version: Some("0.37.1".into()),
            ondisk_version: Some("0.37.2".into()),
            show_build_marker: true,
            ..State::default()
        };
        assert!(state.refresh_output());
        assert!(state.last_output.contains(BUILD_STALE_MARKER));
        assert!(state.last_output.ends_with("ver\u{1b}[0m"));

        // Matching versions -> no marker.
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            last_payload: Some(mixed_payload()),
            serve_build_version: Some("0.37.2".into()),
            ondisk_version: Some("0.37.2".into()),
            ..State::default()
        };
        assert!(state.refresh_output());
        assert!(!state.last_output.contains(BUILD_STALE_MARKER));

        // CLI source -> no build marker even if versions differ (still ⚠cli).
        let mut state = State {
            permissions_granted: true,
            source: Source::Cli,
            last_payload: Some(mixed_payload()),
            serve_build_version: Some("0.37.1".into()),
            ondisk_version: Some("0.37.2".into()),
            ..State::default()
        };
        assert!(state.refresh_output());
        assert!(!state.last_output.contains(BUILD_STALE_MARKER));
        assert!(state.last_output.contains("⚠cli"));

        // Marker disabled (default) -> no ⚠ver even when versions differ.
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            last_payload: Some(mixed_payload()),
            serve_build_version: Some("0.37.1".into()),
            ondisk_version: Some("0.37.2".into()),
            show_build_marker: false,
            ..State::default()
        };
        assert!(state.refresh_output());
        assert!(!state.last_output.contains(BUILD_STALE_MARKER));
    }

    #[test]
    fn version_probe_kicks_only_when_gated() {
        let gated = |source: Source, sv: Option<&str>, cli: CliFallback, perms: bool| -> bool {
            let mut state = State {
                permissions_granted: perms,
                source,
                serve_build_version: sv.map(String::from),
                cli_fallback: cli,
                show_build_marker: true,
                ..State::default()
            };
            state.maybe_kick_version_probe();
            state.version_probe_in_flight
        };
        // All conditions met -> probe kicked.
        assert!(gated(
            Source::Serve,
            Some("0.37.2"),
            CliFallback::Degraded,
            true
        ));
        // No RunCommands (cli_fallback off) -> no probe.
        assert!(!gated(
            Source::Serve,
            Some("0.37.2"),
            CliFallback::Off,
            true
        ));
        // Not on serve -> no probe.
        assert!(!gated(
            Source::Cli,
            Some("0.37.2"),
            CliFallback::Degraded,
            true
        ));
        // No serve build to compare against -> no probe.
        assert!(!gated(Source::Serve, None, CliFallback::Degraded, true));
        // No permissions -> no probe.
        assert!(!gated(
            Source::Serve,
            Some("0.37.2"),
            CliFallback::Degraded,
            false
        ));
        // Marker disabled (default) -> no probe even with all else satisfied.
        let mut off = State {
            permissions_granted: true,
            source: Source::Serve,
            serve_build_version: Some("0.37.2".into()),
            cli_fallback: CliFallback::Degraded,
            show_build_marker: false,
            ..State::default()
        };
        off.maybe_kick_version_probe();
        assert!(!off.version_probe_in_flight);
    }

    #[test]
    fn handle_version_result_success_sets_ondisk() {
        let mut state = State {
            version_probe_in_flight: true,
            version_probe_token: Some("tok".into()),
            ..State::default()
        };
        state.handle_version_result(Some(0), b"CodexBar 0.37.2\n".to_vec(), Some("tok"));
        assert_eq!(state.ondisk_version.as_deref(), Some("0.37.2"));
        assert!(!state.version_probe_in_flight);
        assert!(state.ondisk_version_checked_at.is_some());
    }

    #[test]
    fn handle_version_result_ignores_stale_token() {
        let mut state = State {
            version_probe_in_flight: true,
            version_probe_token: Some("tok".into()),
            ondisk_version: Some("1.0.0".into()),
            ..State::default()
        };
        state.handle_version_result(Some(0), b"CodexBar 2.0.0\n".to_vec(), Some("other"));
        assert_eq!(state.ondisk_version.as_deref(), Some("1.0.0"));
        assert!(state.version_probe_in_flight);
    }

    #[test]
    fn handle_version_result_failure_keeps_last_known() {
        let mut state = State {
            version_probe_in_flight: true,
            version_probe_token: Some("tok".into()),
            ondisk_version: Some("1.0.0".into()),
            ..State::default()
        };
        state.handle_version_result(Some(124), Vec::new(), Some("tok"));
        assert_eq!(state.ondisk_version.as_deref(), Some("1.0.0"));
        assert!(!state.version_probe_in_flight);
        assert!(state.ondisk_version_checked_at.is_some());
    }

    #[test]
    fn version_probe_argv_wraps_bin_without_interpolation() {
        let script = version_probe_script(5);
        let argv = version_probe_argv(&script, "codexbar");
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert_eq!(argv[2], script.as_str());
        assert_eq!(argv[3], "showy-quota-version");
        assert_eq!(argv[4], "codexbar");
        assert!(script.contains("command -v"));
        assert!(script.contains("--version"));
        assert!(script.contains("sleep 5"));
        // The binary rides in $1, never interpolated into the shell string.
        assert!(!script.contains("codexbar"));
    }

    fn holding_state() -> State {
        State {
            fallback_jitter_seconds: 60.0,
            serve_url: "http://127.0.0.1:8080".into(),
            source: Source::Serve,
            last_payload: Some(mixed_payload()),
            permissions_granted: true,
            instance_hash: splitmix64(fnv1a(b"fixture-tab")),
            ..State::default()
        }
    }

    #[test]
    fn should_cli_hold_only_on_genuine_outage_transitions() {
        assert!(holding_state().should_cli_hold());

        let mut s = holding_state();
        s.fallback_jitter_seconds = 0.0;
        assert!(!s.should_cli_hold(), "jitter=0 disables the hold");

        let mut s = holding_state();
        s.serve_url = String::new();
        assert!(!s.should_cli_hold(), "pure-CLI mode never holds");

        let mut s = holding_state();
        s.source = Source::Cli;
        assert!(
            !s.should_cli_hold(),
            "already-degraded source never re-holds"
        );

        let mut s = holding_state();
        s.last_payload = None;
        assert!(!s.should_cli_hold(), "cold start never holds");
    }

    #[test]
    fn instance_seed_disperses_consecutive_plugin_ids() {
        // Worst case: identical config across tabs, only plugin_id differs.
        // Derive via the production seed fn so a regression in the real
        // derivation (dropped plugin_id, delimiter collision, weakened mixing)
        // is caught here instead of silently reintroducing the herd.
        let jitter = 60.0;
        let delays: Vec<f64> = (1u32..=16)
            .map(|id| {
                unit_from(instance_seed(id, "", "http://127.0.0.1:8080", "codexbar")) * jitter
            })
            .collect();

        assert!(
            delays.iter().all(|d| (0.0..jitter).contains(d)),
            "every delay within [0, jitter)"
        );
        let mut sorted = delays.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for w in sorted.windows(2) {
            assert!(w[1] > w[0], "consecutive plugin ids must not collide");
        }
        // Anti-herd: no 5s sub-window may bunch too many instances (actual max
        // is 3 for ids 1..=16; guard at 4 to catch a mixing regression).
        let max_bunch = (0..12)
            .map(|i| {
                let lo = i as f64 * 5.0;
                delays.iter().filter(|&&d| d >= lo && d < lo + 5.0).count()
            })
            .max()
            .unwrap();
        assert!(
            max_bunch <= 4,
            "instances bunched in a 5s window: {max_bunch}"
        );

        // Pure / deterministic.
        assert_eq!(
            instance_seed(7, "", "http://127.0.0.1:8080", "codexbar"),
            instance_seed(7, "", "http://127.0.0.1:8080", "codexbar")
        );
    }

    #[test]
    fn next_timeout_uses_short_cadence_while_holding() {
        let mut s = holding_state();
        s.interval_seconds = 120.0;
        assert_eq!(
            s.next_timeout_seconds(1000.0),
            120.0,
            "no hold -> normal cadence"
        );
        s.cli_hold_until = Some(2000.0);
        assert_eq!(
            s.next_timeout_seconds(1000.0),
            HOLD_REPROBE_INTERVAL_SECONDS,
            "holding far out -> short re-probe cadence"
        );
        s.cli_hold_until = Some(1000.5);
        assert!(
            (s.next_timeout_seconds(1000.0) - 0.5).abs() < 1e-9,
            "near expiry -> wake at the deadline"
        );
        s.cli_hold_until = Some(500.0);
        assert_eq!(
            s.next_timeout_seconds(1000.0),
            120.0,
            "expired -> normal cadence"
        );
    }

    #[test]
    fn returning_to_serve_clears_the_hold() {
        let mut s = holding_state();
        s.cli_hold_until = Some(now_seconds() + 100.0);
        s.set_source(Source::Serve);
        assert!(s.cli_hold_until.is_none());
    }

    #[test]
    fn first_fallback_arms_hold_and_reprobes_without_cli() {
        let mut s = holding_state();
        s.cli_fallback = CliFallback::Degraded;
        s.discovered_providers = vec!["codex".to_string()];
        s.discovered_providers_at = Some(now_seconds());
        assert!(s.cli_hold_until.is_none());

        s.kick_cli_fallback_or_render_failure();

        assert!(s.cli_hold_until.is_some(), "hold armed on first transition");
        assert_eq!(
            s.source,
            Source::Probing,
            "the one-shot re-probe moves source off Serve so a later commit is accepted"
        );
        assert!(s.health_in_flight, "the one-shot re-probe is in flight");
        assert!(
            !s.has_provider_work_in_flight(),
            "no CLI spawned during the hold"
        );
        assert!(
            !s.discovery_in_flight,
            "no discovery spawned during the hold"
        );
    }

    #[test]
    fn active_hold_is_idempotent() {
        let mut s = holding_state();
        s.source = Source::Probing;
        let deadline = now_seconds() + 100.0;
        s.cli_hold_until = Some(deadline);
        s.kick_cli_fallback_or_render_failure();
        assert_eq!(
            s.cli_hold_until,
            Some(deadline),
            "deadline unchanged by re-probe"
        );
        assert!(!s.has_provider_work_in_flight());
        assert!(
            !s.health_in_flight,
            "active hold must not re-probe inline; the Timer paces re-probes"
        );
    }

    #[test]
    fn expired_hold_commits_to_cli() {
        let mut s = holding_state();
        s.source = Source::Probing;
        s.cli_fallback = CliFallback::Degraded;
        s.discovered_providers = vec!["codex".to_string()];
        s.discovered_providers_at = Some(now_seconds());
        s.cli_hold_until = Some(now_seconds() - 1.0);
        s.kick_cli_fallback_or_render_failure();
        assert!(s.cli_hold_until.is_none(), "hold cleared on commit");
        assert!(s.has_provider_work_in_flight(), "committed -> CLI spawned");
        assert_eq!(
            s.source,
            Source::Cli,
            "commit latches source to Cli so the hold cannot re-arm next cycle"
        );
    }

    #[test]
    fn serve_recovery_during_hold_spawns_no_cli() {
        let mut s = holding_state();
        s.cli_fallback = CliFallback::Degraded;
        // Inventory matches mixed_payload so accept_payload(Serve) succeeds.
        s.discovered_providers = vec![
            "claude".to_string(),
            "codex".to_string(),
            "gemini".to_string(),
            "cursor".to_string(),
        ];
        s.discovered_providers_at = Some(now_seconds());

        // Arm the hold (source Serve -> Probing, one-shot re-probe in flight).
        s.kick_cli_fallback_or_render_failure();
        assert!(s.cli_hold_until.is_some());
        assert!(!s.has_provider_work_in_flight());

        let health_context = current_web_context(&s, HEALTH_KIND);
        // Serve recovers mid-hold: /health 200 kicks /usage, /usage 200 accepts.
        s.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            b"{}".to_vec(),
            health_context,
        ));
        let usage_context = current_web_context(&s, USAGE_KIND);
        s.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            mixed_payload(),
            usage_context,
        ));

        assert_eq!(s.source, Source::Serve, "recovered to the serve HTTP path");
        assert!(s.cli_hold_until.is_none(), "hold cleared on recovery");
        assert!(
            !s.has_provider_work_in_flight(),
            "zero CLI spawned across the whole recovery"
        );
    }

    #[test]
    fn jitter_zero_keeps_legacy_immediate_fallback() {
        let mut s = holding_state();
        s.fallback_jitter_seconds = 0.0;
        s.source = Source::Probing;
        s.cli_fallback = CliFallback::Degraded;
        s.discovered_providers = vec!["codex".to_string()];
        s.discovered_providers_at = Some(now_seconds());
        s.kick_cli_fallback_or_render_failure();
        assert!(s.cli_hold_until.is_none(), "disabled -> no hold");
        assert!(
            s.has_provider_work_in_flight(),
            "disabled -> immediate fallback"
        );
    }

    #[test]
    fn cli_fallback_off_clears_hold_and_marks_unavailable() {
        let mut s = holding_state();
        s.cli_fallback = CliFallback::Off;
        s.cli_hold_until = Some(now_seconds() + 100.0);
        s.kick_cli_fallback_or_render_failure();
        assert!(s.cli_hold_until.is_none());
        assert_eq!(s.source, Source::Unavailable);
    }

    #[test]
    fn parse_nonnegative_allows_zero_rejects_negative() {
        assert_eq!(parse_nonnegative_f64(Some("0"), 60.0), 0.0);
        assert_eq!(parse_nonnegative_f64(Some("30"), 60.0), 30.0);
        assert_eq!(parse_nonnegative_f64(Some("-5"), 60.0), 60.0);
        assert_eq!(parse_nonnegative_f64(Some("nope"), 60.0), 60.0);
        assert_eq!(parse_nonnegative_f64(None, 60.0), 60.0);
    }
}
