use crate::codexbar::MAX_USAGE_JSON_BYTES;
use crate::palette::hex_to_rgb;
use crate::{
    carry_last_known_usage, parse_provider_config_payload, parse_usage_payload,
    parse_usage_payload_indexed, payload_has_renderable_provider, provider_ids_from_records,
    render_zellij, valid_provider_id, Freshness, ProviderConfigError, ProviderRecord, RenderConfig,
    RenderOptions,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Terminal(u32),
    Plugin(u32),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionType {
    WebAccess,
    OpenTerminalsOrPlugins,
    RunCommands,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    Granted,
    Denied,
}
#[derive(Debug)]
pub enum Event {
    PermissionRequestResult(PermissionStatus),
    Timer(f64),
    Visible(bool),
    WebRequestResult(
        u16,
        BTreeMap<String, String>,
        Vec<u8>,
        BTreeMap<String, String>,
    ),
    CommandPaneExited(u32, Option<i32>, BTreeMap<String, String>),
    RunCommandResult(Option<i32>, Vec<u8>, Vec<u8>, BTreeMap<String, String>),
    ManagedServeStarted(Option<PaneId>, BTreeMap<String, String>),
    ManagedServeRecycled(bool),
}
#[derive(Debug, Clone)]
pub struct CommandToRun {
    pub path: std::path::PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
}
#[derive(Debug, Clone)]
pub enum Effect {
    ProbeHealth {
        url: String,
        context: BTreeMap<String, String>,
    },
    FetchUsage {
        url: String,
        context: BTreeMap<String, String>,
    },
    StartServe {
        command: CommandToRun,
        context: BTreeMap<String, String>,
    },
    DiscoverProviders {
        argv: Vec<String>,
        context: BTreeMap<String, String>,
    },
    FetchProvider {
        argv: Vec<String>,
        context: BTreeMap<String, String>,
    },
    ProbeVersion {
        binary: String,
        context: BTreeMap<String, String>,
    },
    RecycleOwnedServe,
    Schedule(f64),
}
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
const MAX_SUBPROCESS_STDOUT_BYTES: usize = 5 * 1024 * 1024;
const MANAGED_SERVE_SPAWN_JITTER_SECONDS: f64 = 2.0;
const MANAGED_SERVE_KIND: &str = "showy-quota-serve";
const MANAGED_SERVE_ATTEMPT_KEY: &str = "showy-quota-serve-attempt";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Unknown,
    Probing,
    Serve,
    ManagedServeStarting,
    Cli,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliFallback {
    Off,
    Degraded,
}

pub fn requested_permissions(manage_serve: bool, cli_fallback: CliFallback) -> Vec<PermissionType> {
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
pub struct ProviderFallbackState {
    pub in_flight: bool,
    pub last_record: Option<serde_json::Value>,
    /// When `last_record` was actually measured. The synthesized CLI payload
    /// republishes every provider's last-known-good slice on each per-provider
    /// success, so the payload's own age says nothing about a record the
    /// plugin merely carried forward.
    pub last_record_seconds: Option<f64>,
    pub last_record_source: Option<Source>,
    pub last_result_empty: bool,
    pub last_attempt_seconds: Option<f64>,
    pub last_failure_seconds: Option<f64>,
    pub active_attempt_token: Option<String>,
    pub consecutive_failures: u32,
}

#[derive(Debug)]
pub struct State {
    pub now: f64,
    pub instance_id: u32,
    pub initial_cwd: String,
    pub effects: Vec<Effect>,
    pub last_error_class: Option<String>,
    pub recycle_owned_serve: bool,
    pub health_timeout_seconds: f64,
    pub usage_timeout_seconds: f64,
    pub provider_timeout_seconds: f64,
    pub discovery_timeout_seconds: f64,
    pub failures_before_cli: u8,
    pub render_config: RenderConfig,
    pub serve_url: String,
    pub interval_seconds: f64,
    pub cli_interval_seconds: f64,
    pub manage_serve: bool,
    pub serve_command: String,
    pub serve_port: String,
    pub serve_refresh_seconds: u64,
    pub cli_fallback: CliFallback,
    pub cli_command: String,
    // Backoff window between repeated per-provider retries after a failure;
    // defaults to cli_interval_seconds so backoff scales with the cadence that
    // drives the bar.
    pub provider_failure_backoff_seconds: f64,
    pub source: Source,
    // A managed serve is a Zellij background command pane shared by every
    // plugin instance that targets this loopback port. Keep the exact pane and
    // command-attempt identity until Zellij reports that pane exited; a
    // transient HTTP/CLI failure cannot safely prove it is gone.
    pub managed_serve_requested: bool,
    pub managed_serve_pane: Option<PaneId>,
    pub managed_serve_attempt_token: Option<String>,
    pub managed_serve_spawn_after_seconds: Option<f64>,
    pub managed_serve_attempt_counter: u64,
    pub managed_serve_last_attempt_seconds: Option<f64>,
    pub consecutive_serve_failures: u8,
    pub last_payload: Option<Vec<u8>>,
    pub last_success_seconds: Option<f64>,
    pub last_cli_fetch_seconds: Option<f64>,
    pub last_output: String,
    pub health_in_flight: bool,
    pub usage_in_flight: bool,
    pub health_generation: u64,
    pub usage_generation: u64,
    pub active_health_generation: Option<u64>,
    pub active_usage_generation: Option<u64>,
    // When the in-flight /health or /usage web request was started, so a hung
    // request can be expired (Zellij never reports request failure).
    pub web_flight_started_at: Option<f64>,
    pub permissions_granted: bool,
    // Provider discovery state: cached output of `codexbar config providers`.
    // An empty `discovered_providers` with `discovered_providers_at == None`
    // means no discovery attempt has succeeded; an empty list with a set
    // timestamp means CodexBar reports zero enabled providers (canonical
    // empty inventory → publish `[]`).
    pub discovered_providers: Vec<String>,
    pub discovered_providers_at: Option<f64>,
    pub discovery_in_flight: bool,
    pub discovery_attempt_token: Option<String>,
    pub discovery_started_at: Option<f64>,
    pub usage_after_discovery: bool,
    pub serve_inventory_mismatch: bool,
    pub discovery_failed_at: Option<f64>,
    pub discovery_failure_backoff_seconds: f64,
    // Per-provider fallback state: tracks in-flight commands and last-known
    // good records so one provider's failure never blocks the others.
    pub provider_states: BTreeMap<String, ProviderFallbackState>,
    // Stale-serve build gate. `serve_build_version` is the version reported by
    // the running serve's /health (None for a pre-#1703 serve that omits it);
    // `ondisk_version` is the installed binary's version from a periodic
    // `codexbar --version` probe. A marker shows only when both are known and
    // differ. Probe state mirrors the discovery in-flight/token guard.
    pub serve_build_version: Option<String>,
    pub ondisk_version: Option<String>,
    pub ondisk_version_checked_at: Option<f64>,
    pub version_probe_in_flight: bool,
    pub version_probe_token: Option<String>,
    pub version_probe_started_at: Option<f64>,
    // Opt-in (KDL `build_marker true`, default off). When off, the on-disk
    // version probe never runs and the ⚠ver marker is never appended — the
    // whole stale-build gate is a silent no-op. The plugin only flags; it
    // never recycles a session-owned serve.
    pub show_build_marker: bool,
    // Stable per-instance seed (Zellij plugin id + cwd + serve/cli command),
    // used to disperse the degraded-fallback hold and per-provider retry backoff
    // across the N same-config tab instances without any shared state or RNG.
    pub instance_hash: u64,
    // Monotonic per-instance discriminator for every subprocess context token.
    // Time records scheduling, but never participates in identity.
    pub subprocess_attempt_counter: u64,
    // Max per-instance random hold (seconds) before the first CLI fallback after
    // a serve outage; 0 disables the hold (legacy immediate-fallback behavior).
    pub fallback_jitter_seconds: f64,
    // Deadline of the active degraded-fallback hold, if any. While set and in the
    // future, the plugin re-probes serve instead of spawning CLI work.
    pub cli_hold_until: Option<f64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            now: 1_700_000_000.0,
            instance_id: 0,
            initial_cwd: String::new(),
            effects: Vec::new(),
            last_error_class: None,
            recycle_owned_serve: false,
            health_timeout_seconds: SERVE_HEALTH_TIMEOUT_SECONDS,
            usage_timeout_seconds: SERVE_USAGE_TIMEOUT_SECONDS,
            provider_timeout_seconds: PROVIDER_COMMAND_TIMEOUT_SECONDS as f64,
            discovery_timeout_seconds: PROVIDER_COMMAND_TIMEOUT_SECONDS as f64,
            failures_before_cli: SERVE_FAILURES_BEFORE_CLI,
            render_config: RenderConfig::default(),
            serve_url: "http://127.0.0.1:8080".into(),
            interval_seconds: 60.0,
            cli_interval_seconds: 120.0,
            manage_serve: true,
            serve_command: "codexbar".into(),
            serve_port: "8080".into(),
            serve_refresh_seconds: 120,
            cli_fallback: CliFallback::Degraded,
            cli_command: "codexbar".into(),
            provider_failure_backoff_seconds: 120.0,
            source: Source::Unknown,
            managed_serve_requested: false,
            managed_serve_pane: None,
            managed_serve_attempt_token: None,
            managed_serve_spawn_after_seconds: None,
            managed_serve_attempt_counter: 0,
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
            version_probe_started_at: None,
            show_build_marker: false,
            instance_hash: 0,
            subprocess_attempt_counter: 0,
            fallback_jitter_seconds: 60.0,
            cli_hold_until: None,
        }
    }
}
impl State {
    pub fn load(&mut self, configuration: BTreeMap<String, String>) {
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
        if !is_loopback_serve_url(&self.serve_url) {
            self.serve_url.clear();
        }
        // Serve /usage poll cadence. `codexbar serve` only re-collects once
        // per its --refresh-interval, so polling much faster than that just
        // re-downloads identical JSON; 60s keeps worst-case data age well
        // inside quota-window granularity while waking the host far less.
        self.interval_seconds = parse_positive_f64(
            configuration.get("interval_seconds").map(String::as_str),
            60.0,
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
            .map(|value| value.trim())
            .filter(|value| valid_port(value))
            .map(str::to_string)
            .or_else(|| derive_port_from_url(&self.serve_url))
            .unwrap_or_else(|| "8080".into());
        self.serve_refresh_seconds = parse_positive_u64(
            configuration
                .get("serve_refresh_seconds")
                .or_else(|| configuration.get("SHOWY_QUOTA_REFRESH_SECONDS"))
                .map(String::as_str),
            120,
        );
        self.cli_command = configuration
            .get("cli_command")
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
        let (plugin_id, initial_cwd) = (self.instance_id, self.initial_cwd.clone());
        self.instance_hash =
            instance_seed(plugin_id, &initial_cwd, &self.serve_url, &self.cli_command);
        self.fallback_jitter_seconds = parse_nonnegative_f64(
            configuration
                .get("fallback_jitter_seconds")
                .map(String::as_str),
            self.cli_interval_seconds.min(60.0),
        );

        // Runtime work begins only after Zellij explicitly grants the requested
        // capabilities. Pre-granted permissions.kdl configurations follow the
        // same path: Zellij emits PermissionRequestResult::Granted.
        self.permissions_granted = false;
    }
    pub fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                let previous_output = self.last_output.clone();
                self.permissions_granted = true;
                self.last_error_class = None;
                self.health_in_flight = false;
                self.usage_in_flight = false;
                self.active_health_generation = None;
                self.active_usage_generation = None;
                self.web_flight_started_at = None;
                self.discovery_in_flight = false;
                self.discovery_attempt_token = None;
                self.discovery_started_at = None;
                self.usage_after_discovery = false;
                self.version_probe_in_flight = false;
                self.version_probe_token = None;
                self.version_probe_started_at = None;
                self.serve_inventory_mismatch = false;
                self.clear_all_provider_in_flight();
                self.cli_hold_until = None;
                self.schedule_timer();
                self.tick();
                self.last_output != previous_output
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                self.permissions_granted = false;
                self.last_error_class = Some("permission_denied".into());
                self.health_in_flight = false;
                self.usage_in_flight = false;
                self.active_health_generation = None;
                self.active_usage_generation = None;
                self.web_flight_started_at = None;
                self.discovery_in_flight = false;
                self.discovery_attempt_token = None;
                self.discovery_started_at = None;
                self.usage_after_discovery = false;
                self.version_probe_in_flight = false;
                self.version_probe_token = None;
                self.version_probe_started_at = None;
                self.serve_inventory_mismatch = false;
                self.clear_all_provider_in_flight();
                self.cli_hold_until = None;
                self.last_output = " showy-quota: permission denied ".into();
                true
            }
            Event::Timer(_) => {
                if !self.permissions_granted {
                    return false;
                }
                let previous_output = self.last_output.clone();
                self.schedule_timer();
                self.refresh_output();
                self.tick();
                self.last_output != previous_output
            }
            Event::Visible(true) => {
                if self.permissions_granted {
                    self.refresh_output();
                }
                let interval = if self.source == Source::Cli {
                    self.cli_interval_seconds
                } else {
                    self.interval_seconds
                };
                // A fresh snapshot needs no tab-switch probe or extra wakeup.
                if self.permissions_granted
                    && self
                        .last_success_seconds
                        .map(|last| (self.now - last).max(0.0) >= interval)
                        .unwrap_or(true)
                    && !self.health_in_flight
                    && !self.usage_in_flight
                    && !self.discovery_in_flight
                    && !self.version_probe_in_flight
                    && !self.has_provider_work_in_flight()
                {
                    self.tick();
                }
                true
            }
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
                            self.adopt_healthy_serve();
                            self.update_serve_build_version(&body);
                            if self.should_recycle_owned_serve() {
                                self.effects.push(Effect::RecycleOwnedServe);
                            } else {
                                self.kick_usage();
                            }
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
            Event::CommandPaneExited(pane_id, _exit, context) => {
                self.handle_managed_serve_exit(pane_id, &context);
                false
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
            Event::ManagedServeStarted(pane, context) => {
                if context.get(MANAGED_SERVE_ATTEMPT_KEY).map(String::as_str)
                    != self.managed_serve_attempt_token.as_deref()
                {
                    return false;
                }
                match pane {
                    Some(pane) => {
                        self.managed_serve_pane = Some(pane);
                        self.kick_health_probe();
                    }
                    None => {
                        self.managed_serve_requested = false;
                        self.managed_serve_attempt_token = None;
                        self.last_error_class = Some("serve_start_failed".into());
                        self.kick_cli_fallback_or_render_failure();
                    }
                }
                true
            }
            Event::ManagedServeRecycled(stopped) => {
                self.recycle_owned_serve = false;
                if stopped {
                    self.managed_serve_pane = None;
                    self.managed_serve_attempt_token = None;
                    self.managed_serve_requested = false;
                    self.managed_serve_last_attempt_seconds = None;
                    self.kick_health_probe();
                } else {
                    self.kick_usage();
                }
                false
            }
            _ => false,
        }
    }
}
impl State {
    fn set_source(&mut self, source: Source) {
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
            && self.managed_serve_pane.is_none()
            && self.managed_serve_retry_allowed(now_seconds)
    }

    fn next_subprocess_attempt_token(&mut self) -> String {
        self.subprocess_attempt_counter = self.subprocess_attempt_counter.saturating_add(1);
        format!(
            "{:016x}-{}",
            self.instance_hash, self.subprocess_attempt_counter
        )
    }

    /// A short, stable phase keeps tabs that share one loopback port from
    /// opening competing command panes after observing the same outage.
    fn managed_serve_spawn_delay_seconds(&self) -> f64 {
        unit_from(splitmix64(self.instance_hash ^ 0xa5a5_a5a5_a5a5_a5a5))
            * MANAGED_SERVE_SPAWN_JITTER_SECONDS
    }

    /// The preceding failed /health response is only the first half of a
    /// startup attempt. Wait for this instance's deterministic phase, then
    /// probe once more before opening a pane so a concurrently started serve is
    /// always adopted rather than duplicated.
    fn arm_managed_serve_start(&mut self, now: f64) {
        self.managed_serve_requested = true;
        self.managed_serve_last_attempt_seconds = Some(now);
        self.managed_serve_spawn_after_seconds =
            Some(now + self.managed_serve_spawn_delay_seconds());
        self.set_source(Source::ManagedServeStarting);
        self.schedule_timer();
    }

    /// A healthy responder owns the endpoint regardless of who launched it
    /// (another plugin instance, the shell integration, or the user). Stand
    /// down from any pending spawn immediately. A tracked pane remains only as
    /// a lifecycle identity: it is not cleared until Zellij confirms exit.
    fn adopt_healthy_serve(&mut self) {
        self.managed_serve_requested = false;
        self.managed_serve_spawn_after_seconds = None;
        if self.managed_serve_pane.is_none() {
            self.managed_serve_attempt_token = None;
        }
    }

    /// Command-pane exit is the only lifecycle signal that lets this instance
    /// relinquish its managed-serve identity. A stale exit event is ignored by
    /// both the pane id and the attempt token.
    fn handle_managed_serve_exit(&mut self, pane_id: u32, context: &BTreeMap<String, String>) {
        if context.get("kind").map(String::as_str) != Some(MANAGED_SERVE_KIND)
            || context.get(MANAGED_SERVE_ATTEMPT_KEY).map(String::as_str)
                != self.managed_serve_attempt_token.as_deref()
            || !matches!(self.managed_serve_pane, Some(PaneId::Terminal(id)) if id == pane_id)
        {
            return;
        }
        self.managed_serve_pane = None;
        self.managed_serve_attempt_token = None;
        self.managed_serve_requested = false;
        self.managed_serve_spawn_after_seconds = None;
        // Re-check the shared endpoint before considering a replacement: the
        // exiting pane may have lost a bind race to a healthy foreign serve.
        self.kick_health_probe();
    }

    /// Late per-provider results from a previous CLI burst must be discarded
    /// once serve has recovered. Discovery results, in contrast, are always
    /// safe to absorb because they only update the inventory cache.
    fn should_accept_cli_result(&self) -> bool {
        !matches!(self.source, Source::Serve)
    }

    fn schedule_timer(&mut self) {
        self.effects
            .push(Effect::Schedule(self.next_timeout_seconds(self.now)));
    }

    /// Single scheduling chokepoint. Outage holds and deferred managed-serve
    /// starts both wake only when their next bounded re-probe is due.
    fn next_timeout_seconds(&self, now: f64) -> f64 {
        let mut next = self.interval_seconds;
        if let Some(until) = self.cli_hold_until {
            if now < until {
                let remaining = (until - now).max(0.1);
                let reprobe = HOLD_REPROBE_INTERVAL_SECONDS.min(self.interval_seconds);
                next = next.min(remaining.min(reprobe));
            }
        }
        if let Some(spawn_after) = self.managed_serve_spawn_after_seconds {
            if now < spawn_after {
                next = next.min((spawn_after - now).max(0.1));
            }
        }
        next
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
        let now = self.now;
        let Some(started_at) = self.discovery_started_at else {
            self.discovery_started_at = Some(now);
            return;
        };
        if (now - started_at).max(0.0) < self.discovery_timeout_seconds {
            return;
        }
        self.discovery_in_flight = false;
        self.discovery_attempt_token = None;
        self.discovered_providers_at = None;
        self.usage_after_discovery = false;
        self.discovery_started_at = None;
        self.discovery_failed_at = Some(now);
        self.last_error_class = Some("discovery_failed".into());
    }

    /// Expire a lost `codexbar --version` result so its in-flight latch cannot
    /// suppress every later probe. Treat expiry like a failed result: preserve
    /// the last-known version and defer the next attempt until its normal TTL.
    fn expire_stale_version_probe(&mut self) {
        if !self.version_probe_in_flight {
            return;
        }
        let now = self.now;
        let Some(started_at) = self.version_probe_started_at else {
            self.version_probe_started_at = Some(now);
            return;
        };
        if (now - started_at).max(0.0) < VERSION_COMMAND_TIMEOUT_SECONDS as f64 {
            return;
        }
        self.version_probe_in_flight = false;
        self.version_probe_token = None;
        self.version_probe_started_at = None;
        self.ondisk_version_checked_at = Some(now);
    }

    /// Expire a hung /health or /usage probe. Returns true when it expired and
    /// routed the timeout through the same failure path a non-200 result would,
    /// so the caller (tick) should not also kick a fresh request this pass.
    fn expire_stale_web_flight(&mut self) -> bool {
        if !self.health_in_flight && !self.usage_in_flight {
            return false;
        }
        let now = self.now;
        let Some(started_at) = self.web_flight_started_at else {
            // In-flight without a recorded start: stamp it so a later tick can
            // expire it rather than wedging indefinitely.
            self.web_flight_started_at = Some(now);
            return false;
        };
        let timeout = if self.usage_in_flight {
            self.usage_timeout_seconds
        } else {
            self.health_timeout_seconds
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
        self.expire_stale_version_probe();
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
            .map(|seconds| (self.now - seconds).max(0.0) >= self.cli_interval_seconds)
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
        self.web_flight_started_at = Some(self.now);
        if self.source != Source::Cli {
            self.set_source(Source::Probing);
        }
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), HEALTH_KIND.to_string());
        context.insert(
            WEB_REQUEST_GENERATION_KEY.to_string(),
            generation.to_string(),
        );
        let url = format!("{}/health", self.serve_url.trim_end_matches('/'));
        self.effects.push(Effect::ProbeHealth { url, context });
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
        self.web_flight_started_at = Some(self.now);
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), USAGE_KIND.to_string());
        context.insert(
            WEB_REQUEST_GENERATION_KEY.to_string(),
            generation.to_string(),
        );
        let url = format!("{}/usage", self.serve_url.trim_end_matches('/'));
        self.effects.push(Effect::FetchUsage { url, context });
    }

    fn handle_serve_unavailable(&mut self) {
        self.last_error_class = Some("serve_unavailable".into());
        self.serve_build_version = None;
        let now = self.now;
        if self.managed_serve_requested {
            if self.managed_serve_pane.is_none()
                && self
                    .managed_serve_spawn_after_seconds
                    .is_some_and(|spawn_after| now >= spawn_after)
            {
                // This failure is the post-jitter confirmation probe. A healthy
                // responder would already have been adopted by the 200 path.
                self.start_managed_serve();
                return;
            }
            if self.managed_serve_spawn_after_seconds.is_some() {
                self.schedule_timer();
                return;
            }
        }
        if self.should_start_managed_serve(now) {
            self.arm_managed_serve_start(now);
            return;
        }
        self.kick_cli_fallback_or_render_failure();
    }

    fn handle_usage_failure(&mut self) {
        self.last_error_class = Some(
            if self.serve_inventory_mismatch {
                "inventory_mismatch"
            } else {
                "usage_unavailable"
            }
            .into(),
        );
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
            && self.consecutive_serve_failures < self.failures_before_cli
        {
            self.refresh_output();
            return;
        }
        self.kick_cli_fallback_or_render_failure();
    }

    fn start_managed_serve(&mut self) {
        if self.managed_serve_pane.is_some() {
            return;
        }
        self.managed_serve_requested = true;
        self.managed_serve_spawn_after_seconds = None;
        self.managed_serve_last_attempt_seconds = Some(self.now);
        self.managed_serve_attempt_counter = self.managed_serve_attempt_counter.saturating_add(1);
        let attempt_token = format!(
            "{:016x}-{}",
            self.instance_hash, self.managed_serve_attempt_counter
        );
        self.set_source(Source::ManagedServeStarting);
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), MANAGED_SERVE_KIND.to_string());
        context.insert(MANAGED_SERVE_ATTEMPT_KEY.to_string(), attempt_token.clone());
        let command = CommandToRun {
            path: self.serve_command.clone().into(),
            args: vec![
                "serve".into(),
                "--port".into(),
                self.serve_port.clone(),
                "--refresh-interval".into(),
                // Configurable managed-serve collection cadence; defaults to
                // the shell data plane freshness contract (120 seconds).
                self.serve_refresh_seconds.to_string(),
            ],
            cwd: None,
        };
        self.managed_serve_attempt_token = Some(attempt_token);
        self.effects.push(Effect::StartServe { command, context });
    }

    fn kick_cli_fallback_or_render_failure(&mut self) {
        if self.cli_fallback == CliFallback::Off {
            self.cli_hold_until = None;
            self.set_source(Source::Unavailable);
            self.render_failure();
            return;
        }
        if self.should_cli_hold() {
            let now = self.now;
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
        let now = self.now;
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
            let elapsed = (self.now - discovered_at).max(0.0);
            if elapsed < self.discovery_failure_backoff_seconds {
                return false;
            }
        }
        if let Some(failed_at) = self.discovery_failed_at {
            let elapsed = (self.now - failed_at).max(0.0);
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
        let started_at = self.now;
        let attempt_token = self.next_subprocess_attempt_token();
        self.discovery_started_at = Some(started_at);
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
        self.effects.push(Effect::DiscoverProviders {
            argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
            context,
        });
    }

    fn handle_discovery_result(
        &mut self,
        exit: Option<i32>,
        stdout: Vec<u8>,
        attempt: Option<&str>,
    ) {
        if !self.discovery_in_flight || attempt != self.discovery_attempt_token.as_deref() {
            return;
        }
        self.discovery_in_flight = false;
        self.discovery_attempt_token = None;
        self.discovery_started_at = None;
        let now = self.now;
        let resume_usage = self.usage_after_discovery;
        self.usage_after_discovery = false;
        if exit != Some(0) || stdout.len() > MAX_SUBPROCESS_STDOUT_BYTES {
            self.discovery_failed_at = Some(now);
            self.last_error_class = Some("discovery_failed".into());
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
                self.last_error_class = Some("discovery_failed".into());
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
        let now = self.now;
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
        let attempt_token = self.next_subprocess_attempt_token();
        self.version_probe_started_at = Some(now);
        self.version_probe_token = Some(attempt_token.clone());
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), VERSION_KIND.to_string());
        context.insert(VERSION_ATTEMPT_KEY.to_string(), attempt_token);
        self.effects.push(Effect::ProbeVersion {
            binary: self.cli_command.clone(),
            context,
        });
    }

    /// Absorb an on-disk version probe result. A stale token (a late result from
    /// a superseded probe) is ignored. On failure or an unparseable version the
    /// last-known on-disk version is kept (no marker flapping); only the check
    /// timestamp advances.
    fn handle_version_result(&mut self, exit: Option<i32>, stdout: Vec<u8>, attempt: Option<&str>) {
        if !self.version_probe_in_flight || attempt != self.version_probe_token.as_deref() {
            return;
        }
        self.version_probe_in_flight = false;
        self.version_probe_token = None;
        self.version_probe_started_at = None;
        self.ondisk_version_checked_at = Some(self.now);
        if exit != Some(0) || stdout.len() > MAX_SUBPROCESS_STDOUT_BYTES {
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
        let mut seen = std::collections::BTreeSet::new();
        candidates.retain(|id| seen.insert(id.clone()));
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
        let now = self.now;
        let attempt_token = self.next_subprocess_attempt_token();
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
        self.effects.push(Effect::FetchProvider {
            argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
            context,
        });
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
        let Some(state) = self.provider_states.get(provider) else {
            return false;
        };
        // Every launched attempt has a token. A missing, late, or mismatched
        // completion cannot clear a newer in-flight command or overwrite
        // fresher data.
        match (attempt, state.active_attempt_token.as_deref()) {
            (Some(result_token), Some(active)) if state.in_flight && result_token == active => {}
            _ => return false,
        }
        if let Some(state) = self.provider_states.get_mut(provider) {
            state.in_flight = false;
            state.active_attempt_token = None;
        }
        if !self.should_accept_cli_result() {
            return false;
        }
        let now = self.now;
        if exit != Some(0) || stdout.len() > MAX_SUBPROCESS_STDOUT_BYTES {
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
        // An answer that measured nothing (error, or CodexBar's offline
        // placeholder) keeps the last-known usage and its measurement time,
        // so the provider goes stale in place instead of vanishing.
        let mut record = record;
        let carried = match (record.as_mut(), entry.last_record.as_ref()) {
            (Some(fresh), Some(previous)) => carry_last_known_usage(fresh, previous),
            _ => false,
        };
        entry.last_record = record;
        if !carried {
            entry.last_record_seconds = Some(now);
            entry.last_record_source = Some(Source::Cli);
        }
        entry.last_result_empty = entry.last_record.is_none();
        entry.last_failure_seconds = None;
        entry.consecutive_failures = 0;
        entry.in_flight = false;
        entry.active_attempt_token = None;
        self.publish_synthesized_cli_payload(now)
    }

    fn record_provider_failure(&mut self, provider: &str, now: f64) {
        self.last_error_class = Some("provider_failed".into());
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
            state.last_result_empty = false;
            state.last_attempt_seconds = None;
            state.last_failure_seconds = None;
            state.active_attempt_token = None;
            state.consecutive_failures = 0;
        }
    }

    fn publish_empty_cli_payload(&mut self) -> bool {
        let previous_output = self.last_output.clone();
        let payload = b"[]".to_vec();
        if self.accept_payload(payload, Source::Cli) {
            self.last_cli_fetch_seconds = Some(self.now);
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
        // The seeded records are exactly as old as the payload they come
        // from, which is when the plugin last accepted a snapshot.
        let payload_seconds = self.last_success_seconds;
        if needs_seed {
            // Seed any unqueried eligible providers from the existing payload so a
            // single per-provider success does not blow away the rest of the bar.
            // Providers outside the current inventory are intentionally ignored:
            // discovery is canonical, so disabled providers must not reappear.
            if let Some(payload) = self.last_payload.as_deref() {
                if let Ok(records) = parse_usage_payload_indexed(payload) {
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) {
                        if let Some(array) = value.as_array() {
                            for (index, record) in &records {
                                if !eligible_set.contains(record.provider.as_str()) {
                                    continue;
                                }
                                let Some(value) = array.get(*index) else {
                                    continue;
                                };
                                let entry = self
                                    .provider_states
                                    .entry(record.provider.clone())
                                    .or_default();
                                if entry.last_record.is_none() && !entry.last_result_empty {
                                    entry.last_record = Some(value.clone());
                                    entry.last_record_seconds = payload_seconds;
                                    entry.last_record_source = Some(self.source);
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

    /// A provider remains in flight for its actual watchdog lifetime. Retry
    /// backoff begins only after the attempt has completed or this deadline has
    /// expired, so a tiny configured backoff cannot pile up live subprocesses.
    fn expire_stale_provider_flights(&mut self, now: f64) {
        for state in self.provider_states.values_mut() {
            if !state.in_flight {
                continue;
            }
            let Some(started_at) = state.last_attempt_seconds else {
                continue;
            };
            if (now - started_at).max(0.0) < self.provider_timeout_seconds {
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
        let Ok(records) = parse_usage_payload_indexed(payload) else {
            return;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
            return;
        };
        let Some(array) = value.as_array() else {
            return;
        };
        let mut pruned: Vec<serde_json::Value> = Vec::new();
        // Index by original position; a positional zip would prune by one
        // provider's eligibility while keeping another's raw record.
        for (index, record) in &records {
            if eligible_set.contains(record.provider.as_str()) {
                if let Some(value) = array.get(*index) {
                    pruned.push(value.clone());
                }
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

        // Per-provider CLI results already carry last-known usage before
        // synthesis. Publish their error records too, including later errors
        // in an all-error inventory. A failed aggregate serve probe still
        // cannot replace the last-known-good snapshot.
        if source != Source::Cli
            && !payload_has_renderable_provider(&records)
            && self.last_payload.is_some()
        {
            return self.render_failure();
        }

        // When serve returns a fresh aggregate, refresh per-provider state so
        // a future degraded transition has a baseline of every record CodexBar
        // just published. A record that measured nothing keeps its last-known
        // usage, so the payload may be rewritten.
        let payload = if source == Source::Serve {
            self.seed_provider_states_from_payload(&records, payload)
        } else {
            payload
        };
        self.last_payload = Some(payload);
        self.last_success_seconds = Some(self.now);
        self.last_error_class = None;
        self.set_source(source);
        self.refresh_output();
        true
    }

    /// Store every serve record as its provider's baseline and return the
    /// payload to publish. A record that measured nothing (an error, or
    /// CodexBar's offline placeholder) keeps the previous record's usage and
    /// measurement time instead of replacing it, so one provider's probe
    /// failure cannot drop it from an otherwise healthy snapshot.
    fn seed_provider_states_from_payload(
        &mut self,
        records: &[ProviderRecord],
        payload: Vec<u8>,
    ) -> Vec<u8> {
        let Ok(indexed) = parse_usage_payload_indexed(&payload) else {
            return payload;
        };
        debug_assert_eq!(indexed.len(), records.len());
        let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&payload) else {
            return payload;
        };
        let Some(array) = value.as_array_mut() else {
            return payload;
        };
        // Index by the record's ORIGINAL array position: the validated list is a
        // subsequence, so a positional zip would store another record's raw JSON
        // under this provider's id.
        let measured_at = self.now;
        let mut any_carried = false;
        for (index, record) in &indexed {
            if !valid_provider_id(&record.provider) {
                continue;
            }
            let Some(value) = array.get_mut(*index) else {
                continue;
            };
            let entry = self
                .provider_states
                .entry(record.provider.clone())
                .or_default();
            let carried = entry
                .last_record
                .as_ref()
                .is_some_and(|previous| carry_last_known_usage(value, previous));
            any_carried |= carried;
            entry.last_record = Some(value.clone());
            if !carried {
                entry.last_record_seconds = Some(measured_at);
                entry.last_record_source = Some(Source::Serve);
            }
        }
        if !any_carried {
            return payload;
        }
        // Restoring older usage can grow the array; never publish past the
        // parser's cap, or every later accept would fail to parse.
        match serde_json::to_vec(&value) {
            Ok(bytes) if bytes.len() <= MAX_USAGE_JSON_BYTES => bytes,
            _ => payload,
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

    /// Providers whose slice has aged past the stale horizon while the
    /// published payload as a whole has not: a CLI slice carried across
    /// failed attempts, or a serve slice whose record measured nothing and
    /// kept its last-known usage (every measured serve record is re-stamped on
    /// accept, so it never appears here). Empty once the bar is wholly stale,
    /// because the strip already carries that marker once.
    fn stale_provider_slices(&self, now_seconds: f64, interval: f64, stale: bool) -> Vec<String> {
        if stale {
            return Vec::new();
        }
        self.provider_states
            .iter()
            .filter(|(_, state)| state.last_record.is_some())
            .filter(|(_, state)| {
                state
                    .last_record_seconds
                    .is_some_and(|seconds| (now_seconds - seconds).max(0.0) >= interval * 2.0)
            })
            .map(|(provider, _)| provider.clone())
            .collect()
    }

    /// Render the current state with the requested color mode.
    pub fn render_output(&self, color: bool) -> String {
        let Some(payload) = self.last_payload.as_deref() else {
            return self.last_output.clone();
        };
        let now = self.now as i64;
        let now_seconds = self.now;
        let interval = match self.source {
            Source::Cli => self.cli_interval_seconds,
            _ => self.interval_seconds,
        };
        let stale = self
            .last_success_seconds
            .map(|seconds| (now_seconds - seconds).max(0.0) >= interval * 2.0)
            .unwrap_or(false);
        // The synthesized CLI payload is republished on every per-provider
        // success, so its age only describes the newest slice. Mark the
        // providers whose own record has aged out, so one recovered provider
        // cannot make the whole bar look live.
        let stale_providers = self.stale_provider_slices(now_seconds, interval, stale);
        let freshness = self.last_success_seconds.map(|seconds| Freshness {
            age_seconds: (now_seconds - seconds).max(0.0) as i64,
            source: match self.source {
                Source::Serve => "serve",
                Source::Cli => "cli",
                _ => "",
            },
        });
        match render_zellij(
            payload,
            &self.render_config,
            RenderOptions {
                color,
                stale,
                degraded_cli: self.source == Source::Cli,
                now_epoch: now,
                freshness,
                stale_providers: &stale_providers,
            },
        ) {
            Ok(output) => {
                let output = output.trim_end_matches(['\r', '\n']);
                let mut composed = output.to_string();
                if self.show_build_marker && self.serve_build_stale() {
                    composed.push(' ');
                    if color {
                        style_build_marker(
                            &mut composed,
                            BUILD_STALE_MARKER,
                            &self.render_config.palette_countdown_warn,
                            &self.render_config.palette_bg,
                        );
                    } else {
                        composed.push_str(BUILD_STALE_MARKER);
                    }
                }
                composed
            }
            Err(_) if self.last_output.is_empty() => " showy-quota: invalid CodexBar JSON ".into(),
            Err(_) => self.last_output.clone(),
        }
    }

    fn refresh_output(&mut self) -> bool {
        if self.last_payload.is_none() {
            return false;
        }
        let output = self.render_output(true);
        let changed = self.last_output != output;
        if changed {
            self.last_output = output;
        }
        changed
    }
}
/// Extract the record from a per-provider CLI payload whose provider id matches
/// the requested one. Returns `Err` for malformed JSON, invalid usage shape,
/// non-array payloads, mismatched provider ids, or any element the validator
/// dropped. Returns `Ok(None)` for a valid but empty per-provider payload.
fn extract_provider_record(
    payload: &[u8],
    provider: &str,
) -> Result<Option<serde_json::Value>, ()> {
    let records = parse_usage_payload_indexed(payload).map_err(|_| ())?;
    if records
        .iter()
        .any(|(_, record)| record.provider != provider)
    {
        return Err(());
    }
    let value: serde_json::Value = serde_json::from_slice(payload).map_err(|_| ())?;
    let array = value.as_array().ok_or(())?;
    // A record dropped by validation is invisible to the id guard above, so a
    // payload carrying anything beyond the validated records cannot be trusted
    // to be a response about `provider` at all: reject it rather than reach
    // past it. Without this, a reply whose first element fails validation would
    // hand that element back as this provider's record.
    if array.len() != records.len() {
        return Err(());
    }
    for (index, record) in &records {
        if record.provider == provider {
            return Ok(Some(array.get(*index).ok_or(())?.clone()));
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

fn parse_positive_u64(value: Option<&str>, default: u64) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
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
pub fn codexbar_version_token(raw: &str) -> Option<String> {
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

pub fn derive_port_from_url(url: &str) -> Option<String> {
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

/// True only for the documented local serve authority:
/// `http://(127.0.0.1|localhost|[::1]):1..65535`. Unlike a general URL parser,
/// this deliberately rejects credentials, paths, queries, fragments, HTTPS,
/// and implicit ports so `serve_url` cannot silently describe a different
/// endpoint than the shell integration uses.
pub fn is_loopback_serve_url(url: &str) -> bool {
    let Some(authority) = url.strip_prefix("http://") else {
        return false;
    };
    if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
        return false;
    }
    let (host, port) = if let Some(port) = authority.strip_prefix("[::1]:") {
        ("[::1]", port)
    } else {
        let Some((host, port)) = authority.split_once(':') else {
            return false;
        };
        if host.contains(':') || port.contains(':') {
            return false;
        }
        (host, port)
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]") && valid_port(port)
}

/// Runtime-neutral refresh coordinator. Hosts supply time and execute emitted effects.
pub type RefreshCoordinator = State;
impl State {
    pub fn set_time(&mut self, now: f64) {
        if now.is_finite() {
            self.now = now;
        }
    }
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }
    pub fn suspend(&mut self) {
        self.effects.clear();
        self.health_in_flight = false;
        self.usage_in_flight = false;
        self.active_health_generation = None;
        self.active_usage_generation = None;
        self.web_flight_started_at = None;
        self.discovery_in_flight = false;
        self.discovery_attempt_token = None;
        self.discovery_started_at = None;
        self.version_probe_in_flight = false;
        self.version_probe_token = None;
        self.version_probe_started_at = None;
        self.clear_all_provider_in_flight();
    }
    fn should_recycle_owned_serve(&self) -> bool {
        self.recycle_owned_serve
            && self.manage_serve
            && self.managed_serve_pane.is_some()
            && matches!((self.serve_build_version.as_deref(), self.ondisk_version.as_deref()),
                (Some(running), Some(installed)) if running != installed)
    }
    pub fn refresh(&mut self) {
        self.last_cli_fetch_seconds = None;
        self.tick();
    }
    pub fn repaint(&mut self) -> bool {
        self.refresh_output()
    }
    pub fn has_work(&self) -> bool {
        self.health_in_flight
            || self.usage_in_flight
            || self.discovery_in_flight
            || self.version_probe_in_flight
            || self.has_provider_work_in_flight()
    }
    /// Shared fixture bypass for native `--fixture`, the WASM KDL/env dev
    /// flag, and follower payload restores: validated JSON renders without a
    /// live serve or CLI.
    pub fn accept_fixture(&mut self, payload: Vec<u8>) -> bool {
        if payload.len() > MAX_USAGE_JSON_BYTES {
            self.last_error_class = Some("invalid_fixture".into());
            return false;
        }
        // Clear discovery context: ambiguity between stale discovery and a
        // fixture payload caused inventory mismatches in early prototypes.
        self.discovered_providers_at = None;
        self.serve_inventory_mismatch = false;
        if self.accept_payload(payload, Source::Serve) {
            self.last_cli_fetch_seconds = None;
            true
        } else {
            self.last_error_class = Some("invalid_fixture".into());
            false
        }
    }
    pub fn restore_payload(&mut self, payload: Vec<u8>, source: Source, measured_at: f64) -> bool {
        let Ok(indexed) = parse_usage_payload_indexed(&payload) else {
            return false;
        };
        let Ok(values) = serde_json::from_slice::<serde_json::Value>(&payload) else {
            return false;
        };
        self.provider_states.clear();
        for (index, record) in indexed {
            if let Some(value) = values.get(index) {
                self.provider_states.insert(
                    record.provider,
                    ProviderFallbackState {
                        last_record: Some(value.clone()),
                        last_record_seconds: Some(measured_at),
                        last_record_source: Some(source),
                        ..ProviderFallbackState::default()
                    },
                );
            }
        }
        self.last_payload = Some(payload);
        self.last_success_seconds = Some(measured_at);
        self.source = source;
        self.refresh_output();
        true
    }
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({"schema":"showy-quota/zellij-diagnostics@1",
            "source": format!("{:?}", self.source).to_ascii_lowercase(),
            "permissions": if self.permissions_granted { "granted" } else { "denied_or_pending" },
            "lastSuccessAgeSeconds": self.last_success_seconds.map(|last| (self.now-last).max(0.0) as u64),
            "serveUrl":self.serve_url, "managedServe":self.managed_serve_pane.is_some(),
            "cliFallback":format!("{:?}",self.cli_fallback).to_ascii_lowercase(),
            "cliInFlight":self.has_provider_work_in_flight(),
            "discovery": if self.discovery_in_flight {"in_flight"} else if self.discovery_failed_at.is_some() {"failed"} else if self.discovered_providers_at.is_some() {"ready"} else {"unknown"},
            "providers":self.discovered_providers, "lastErrorClass":self.last_error_class,
            "healthInFlight":self.health_in_flight, "usageInFlight":self.usage_in_flight,
            "serveBuildVersion":self.serve_build_version,"onDiskVersion":self.ondisk_version,
            "buildStale":self.serve_build_stale()})
    }
}
#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;
    fn now_seconds() -> f64 {
        1_700_000_000.0
    }

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

    const TEST_PROVIDER_ATTEMPT: &str = "test-provider-attempt";

    fn arm_provider_attempt(state: &mut State, provider: &str) {
        let entry = state
            .provider_states
            .entry(provider.to_string())
            .or_default();
        entry.in_flight = true;
        entry.active_attempt_token = Some(TEST_PROVIDER_ATTEMPT.to_string());
    }

    fn provider_fallback_context(provider: &str) -> BTreeMap<String, String> {
        let mut context = event_context(FALLBACK_PROVIDER_KIND);
        context.insert(
            FALLBACK_PROVIDER_CONTEXT_KEY.to_string(),
            provider.to_string(),
        );
        context.insert(
            FALLBACK_PROVIDER_ATTEMPT_KEY.to_string(),
            TEST_PROVIDER_ATTEMPT.to_string(),
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

    fn oversized_stdout() -> Vec<u8> {
        vec![b'x'; MAX_SUBPROCESS_STDOUT_BYTES + 1]
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
    fn load_configures_managed_serve_refresh_seconds() {
        let mut configuration = BTreeMap::new();
        configuration.insert("serve_refresh_seconds".to_string(), "45".to_string());
        let mut state = State::default();

        state.load(configuration);

        assert_eq!(state.serve_refresh_seconds, 45);

        let mut default_state = State::default();
        default_state.load(BTreeMap::new());

        assert_eq!(default_state.serve_refresh_seconds, 120);
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

    fn visible_serve_state(age_seconds: Option<f64>) -> State {
        State {
            permissions_granted: true,
            source: Source::Serve,
            cli_fallback: CliFallback::Off,
            last_payload: Some(mixed_payload()),
            last_success_seconds: age_seconds.map(|age| now_seconds() - age),
            ..State::default()
        }
    }
    #[test]
    fn freshness_uses_last_success_time_and_fetch_source() {
        let mut state = visible_serve_state(Some(185.0));
        state.interval_seconds = 120.0;
        state.render_config.freshness = "age+source".into();
        assert!(state.refresh_output());
        assert!(
            state.last_output.contains("3m serve"),
            "{}",
            state.last_output
        );

        state.source = Source::Cli;
        state.cli_interval_seconds = 120.0;
        assert!(state.refresh_output());
        assert!(
            state.last_output.contains("3m cli"),
            "{}",
            state.last_output
        );

        state.last_success_seconds = Some(now_seconds() - 241.0);
        assert!(state.refresh_output());
        assert!(
            !state.last_output.contains("4m cli"),
            "{}",
            state.last_output
        );
        assert!(state.last_output.contains(&state.render_config.stale_glyph));
    }

    #[test]
    fn visible_stale_snapshot_kicks_usage_probe() {
        let mut state = visible_serve_state(Some(61.0));

        assert!(state.update(Event::Visible(true)));
        assert_ne!(state.last_output, " showy-quota: loading ");
        assert!(state.usage_in_flight);
        assert_eq!(state.active_usage_generation, Some(1));

        let mut without_success = visible_serve_state(None);
        assert!(without_success.update(Event::Visible(true)));
        assert!(without_success.usage_in_flight);
    }

    #[test]
    fn visible_with_request_in_flight_does_not_start_duplicate() {
        let mut usage = visible_serve_state(Some(61.0));
        arm_usage_probe(&mut usage, 1);
        assert!(usage.update(Event::Visible(true)));
        assert_eq!(usage.active_usage_generation, Some(1));
        assert_eq!(usage.health_generation, 0);

        let mut health = visible_serve_state(Some(61.0));
        arm_health_probe(&mut health, 1);
        assert!(health.update(Event::Visible(true)));
        assert_eq!(health.active_health_generation, Some(1));
        assert_eq!(health.usage_generation, 0);

        let mut cli = visible_serve_state(Some(61.0));
        arm_provider_attempt(&mut cli, "codex");
        assert!(cli.update(Event::Visible(true)));
        assert_eq!(cli.usage_generation, 0);
    }

    #[test]
    fn visible_fresh_snapshot_does_not_probe() {
        let mut state = visible_serve_state(Some(1.0));

        assert!(state.update(Event::Visible(true)));
        assert_eq!(state.usage_generation, 0);
        assert_eq!(state.health_generation, 0);
        assert!(!state.discovery_in_flight);
    }

    #[test]
    fn visible_without_permission_preserves_denial() {
        let mut state = visible_serve_state(Some(61.0));
        state.permissions_granted = false;
        state.last_output = " showy-quota: permission denied ".into();

        assert!(state.update(Event::Visible(true)));
        assert_eq!(state.last_output, " showy-quota: permission denied ");
        assert_eq!(state.usage_generation, 0);
    }

    #[test]
    fn unchanged_usage_success_still_resets_failure_counter() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        state.consecutive_serve_failures = state.failures_before_cli - 1;
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
            ..State::default()
        };
        state.web_flight_started_at = Some(now_seconds() - (state.usage_timeout_seconds + 1.0));
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
            ..State::default()
        };
        state.web_flight_started_at = Some(now_seconds() - (state.usage_timeout_seconds + 1.0));

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
            ..State::default()
        };
        state.web_flight_started_at = Some(now_seconds() - (state.health_timeout_seconds + 1.0));

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
            ..State::default()
        };
        state.web_flight_started_at = Some(now_seconds() - (state.health_timeout_seconds + 1.0));
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
            ..State::default()
        };
        state.web_flight_started_at = Some(now_seconds() - (state.health_timeout_seconds + 1.0));
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
    fn is_loopback_serve_url_requires_strict_local_http_authority() {
        for url in [
            "http://127.0.0.1:1",
            "http://localhost:8080",
            "http://[::1]:65535",
        ] {
            assert!(is_loopback_serve_url(url), "{url:?} should be accepted");
        }
        for url in [
            "http://localhost",
            "http://localhost:0",
            "http://localhost:65536",
            "http://localhost:not-a-port",
            "http://localhost:8080/",
            "http://localhost:8080?x=1",
            "http://localhost:8080#fragment",
            "http://user:pass@localhost:8080",
            "https://127.0.0.1:8080",
            "http://::1:8080",
            "http://127.0.0.1.evil.com:8080",
            "ftp://127.0.0.1:8080",
            " http://127.0.0.1:8080",
            "",
        ] {
            assert!(!is_loopback_serve_url(url), "{url:?} should be rejected");
        }
    }

    #[test]
    fn oversized_discovery_output_fails_without_replacing_inventory() {
        let mut state = State {
            source: Source::Cli,
            discovery_in_flight: true,
            discovery_attempt_token: Some("discover-1".to_string()),
            discovered_providers: vec!["codex".to_string()],
            discovered_providers_at: Some(now_seconds()),
            ..State::default()
        };

        state.handle_discovery_result(Some(0), oversized_stdout(), Some("discover-1"));

        assert!(!state.discovery_in_flight);
        assert_eq!(state.discovered_providers, vec!["codex".to_string()]);
        assert!(state.discovery_failed_at.is_some());
    }

    #[test]
    fn oversized_provider_output_fails_without_replacing_last_known_record() {
        let last_record = serde_json::json!({"provider": "codex"});
        let mut state = State {
            source: Source::Cli,
            discovered_providers: vec!["codex".to_string()],
            discovered_providers_at: Some(now_seconds()),
            ..State::default()
        };
        let provider = state
            .provider_states
            .entry("codex".to_string())
            .or_default();
        provider.in_flight = true;
        provider.active_attempt_token = Some("provider-1".to_string());
        provider.last_record = Some(last_record.clone());

        assert!(!state.handle_provider_fallback_result(
            "codex",
            Some("provider-1"),
            Some(0),
            oversized_stdout(),
        ));

        let provider = state.provider_states.get("codex").expect("provider state");
        assert_eq!(provider.last_record.as_ref(), Some(&last_record));
        assert!(provider.last_failure_seconds.is_some());
    }

    #[test]
    fn oversized_version_output_keeps_last_known_version() {
        let mut state = State {
            ondisk_version: Some("1.0.0".to_string()),
            version_probe_in_flight: true,
            version_probe_token: Some("version-1".to_string()),
            ..State::default()
        };

        state.handle_version_result(Some(0), oversized_stdout(), Some("version-1"));

        assert_eq!(state.ondisk_version.as_deref(), Some("1.0.0"));
        assert!(state.ondisk_version_checked_at.is_some());
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
        arm_provider_attempt(&mut state, "codex");
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
            discovery_attempt_token: Some("attempt".to_string()),
            usage_after_discovery: true,
            discovered_providers: vec!["claude".to_string(), "antigravity".to_string()],
            discovered_providers_at: Some(now_seconds() - 120.0),
            ..State::default()
        };

        state.handle_discovery_result(Some(7), Vec::new(), Some("attempt"));

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
    fn discovery_in_flight_uses_watchdog_deadline_not_retry_backoff() {
        let mut state = State {
            discovery_in_flight: true,
            discovery_attempt_token: Some("discover-1".to_string()),
            discovery_started_at: Some(now_seconds() - 1.0),
            discovery_failure_backoff_seconds: 0.1,
            ..State::default()
        };

        state.expire_stale_discovery();
        assert!(state.discovery_in_flight);
        assert_eq!(state.discovery_attempt_token.as_deref(), Some("discover-1"));

        state.discovery_started_at = Some(now_seconds() - state.provider_timeout_seconds - 1.0);
        state.expire_stale_discovery();
        assert!(!state.discovery_in_flight);
        assert!(state.discovery_attempt_token.is_none());
        assert!(state.discovery_failed_at.is_some());
    }

    #[test]
    fn stale_version_probe_expires_before_serve_tick() {
        let mut state = State {
            permissions_granted: true,
            source: Source::Serve,
            cli_fallback: CliFallback::Degraded,
            show_build_marker: true,
            serve_build_version: Some("0.37.2".into()),
            discovered_providers_at: Some(now_seconds()),
            version_probe_in_flight: true,
            version_probe_token: Some("lost".into()),
            version_probe_started_at: Some(
                now_seconds() - 5.0 - 1.0, // version watchdog
            ),
            ..State::default()
        };

        state.tick();

        assert!(!state.version_probe_in_flight);
        assert!(state.version_probe_token.is_none());
        assert!(state.version_probe_started_at.is_none());
        assert!(state.ondisk_version_checked_at.is_some());
    }

    #[test]
    fn per_provider_result_after_usage_failure_is_accepted_while_probing() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Probing,
            ..State::default()
        };
        arm_provider_attempt(&mut state, "codex");

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
    fn managed_serve_ownership_survives_cli_and_unavailable_transitions() {
        let mut state = State {
            managed_serve_requested: true,
            managed_serve_pane: Some(PaneId::Terminal(17)),
            managed_serve_attempt_token: Some("managed-1".to_string()),
            managed_serve_last_attempt_seconds: Some(100.0),
            ..State::default()
        };

        state.set_source(Source::Cli);
        assert!(state.managed_serve_requested);
        assert_eq!(state.managed_serve_pane, Some(PaneId::Terminal(17)));
        assert!(!state.should_start_managed_serve(1_000.0));

        state.set_source(Source::Unavailable);
        assert!(state.managed_serve_requested);
        assert_eq!(
            state.managed_serve_attempt_token.as_deref(),
            Some("managed-1")
        );
    }

    #[test]
    fn healthy_serve_adoption_cancels_pending_spawn_without_dropping_pane_identity() {
        let mut state = State {
            managed_serve_requested: true,
            managed_serve_pane: Some(PaneId::Terminal(17)),
            managed_serve_attempt_token: Some("managed-1".to_string()),
            managed_serve_spawn_after_seconds: Some(now_seconds() + 1.0),
            ..State::default()
        };

        state.adopt_healthy_serve();

        assert!(!state.managed_serve_requested);
        assert!(state.managed_serve_spawn_after_seconds.is_none());
        assert_eq!(state.managed_serve_pane, Some(PaneId::Terminal(17)));
        assert_eq!(
            state.managed_serve_attempt_token.as_deref(),
            Some("managed-1")
        );
    }

    #[test]
    fn managed_serve_exit_releases_only_the_matching_pane_identity() {
        let mut state = State {
            managed_serve_requested: true,
            managed_serve_pane: Some(PaneId::Terminal(17)),
            managed_serve_attempt_token: Some("managed-1".to_string()),
            ..State::default()
        };
        let mut context = event_context(MANAGED_SERVE_KIND);
        context.insert(
            MANAGED_SERVE_ATTEMPT_KEY.to_string(),
            "managed-1".to_string(),
        );

        state.handle_managed_serve_exit(18, &context);
        assert_eq!(state.managed_serve_pane, Some(PaneId::Terminal(17)));

        state.handle_managed_serve_exit(17, &context);
        assert!(state.managed_serve_pane.is_none());
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
    fn duplicate_allow_list_entries_produce_one_eligible_provider() {
        let mut state = State::default();
        state.render_config.providers = vec!["codex".to_string(), "codex".to_string()];

        assert_eq!(state.eligible_provider_inventory(), vec!["codex"]);
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
    fn permission_regrant_clears_provider_failure_backoff() {
        let now = now_seconds();
        let mut state = State {
            source: Source::Unknown,
            cli_fallback: CliFallback::Off,
            provider_failure_backoff_seconds: 60.0,
            ..State::default()
        };
        state.record_provider_failure("codex", now);
        state.record_provider_failure("codex", now);
        assert!(state.provider_in_flight_or_backoff("codex", now));

        state.update(Event::PermissionRequestResult(PermissionStatus::Denied));
        state.update(Event::PermissionRequestResult(PermissionStatus::Granted));

        let provider = state.provider_states.get("codex").expect("provider state");
        assert!(provider.last_failure_seconds.is_none());
        assert_eq!(provider.consecutive_failures, 0);
        assert!(provider.last_attempt_seconds.is_none());
        assert!(!provider.last_result_empty);
        assert!(!state.provider_in_flight_or_backoff("codex", now));
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
        arm_provider_attempt(&mut state, "codex");
        arm_provider_attempt(&mut state, "claude");

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
    fn synthesized_cli_all_error_snapshot_keeps_every_provider_and_carried_usage() {
        for with_previous_usage in [false, true] {
            let mut state = State {
                source: Source::Cli,
                discovered_providers: vec!["cursor".into(), "factory".into()],
                discovered_providers_at: Some(1_000.0),
                ..State::default()
            };
            state.set_time(1_000.0);
            if with_previous_usage {
                assert!(state.restore_payload(
                    br#"[{"provider":"factory","usage":{"primary":{"usedPercent":30}}}]"#.to_vec(),
                    Source::Serve,
                    1_000.0,
                ));
                state.source = Source::Cli;
            }
            state.set_time(2_000.0);
            let errors = br#"[
                {"provider":"cursor","error":{"message":"No Cursor session found."}},
                {"provider":"factory","error":{"message":"Safari cookies not readable."}}
            ]"#;
            for provider in ["cursor", "factory"] {
                arm_provider_attempt(&mut state, provider);
                state.update(Event::RunCommandResult(
                    Some(0),
                    provider_record(errors, provider),
                    Vec::new(),
                    provider_fallback_context(provider),
                ));
            }

            let payload = state.last_payload.as_deref().expect("CLI error snapshot");
            let records = parse_usage_payload(payload).expect("valid CLI error snapshot");
            assert_eq!(
                provider_ids_from_records(&records),
                vec!["cursor", "factory"]
            );
            assert!(records.iter().all(|record| record.error.is_some()));
            assert!(state.last_output.contains("CR"));
            assert!(state.last_output.contains("FA"));
            let factory = &state.provider_states["factory"];
            let factory_record = factory.last_record.as_ref().expect("factory error");
            if with_previous_usage {
                assert_eq!(factory_record["usage"]["primary"]["usedPercent"], 30);
                assert_eq!(factory.last_record_seconds, Some(1_000.0));
                assert_eq!(factory.last_record_source, Some(Source::Serve));
            } else {
                assert!(factory_record.get("usage").is_none());
                assert_eq!(factory.last_record_seconds, Some(2_000.0));
                assert_eq!(factory.last_record_source, Some(Source::Cli));
            }
        }
    }

    #[test]
    fn all_error_serve_snapshot_keeps_last_known_good_payload() {
        let old = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":30}}}]"#.to_vec();
        let mut state = State::default();
        state.set_time(1_000.0);
        assert!(state.restore_payload(old.clone(), Source::Serve, 1_000.0));
        state.set_time(2_000.0);
        state.accept_payload(
            br#"[{"provider":"codex","error":{"message":"login failed"}}]"#.to_vec(),
            Source::Serve,
        );
        assert_eq!(state.last_payload, Some(old));
        assert_eq!(state.last_success_seconds, Some(1_000.0));
        let codex = &state.provider_states["codex"];
        assert_eq!(codex.last_record_seconds, Some(1_000.0));
        assert_eq!(codex.last_record_source, Some(Source::Serve));
        assert_eq!(
            codex.last_record.as_ref().expect("last-known usage")["usage"]["primary"]
                ["usedPercent"],
            30
        );
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
        state.kick_discovery();
        let attempt = state
            .discovery_attempt_token
            .clone()
            .expect("discovery attempt");
        // Simulate a malformed discovery payload → records a failure stamp.
        state.handle_discovery_result(Some(0), b"{\"providers\":[]}".to_vec(), Some(&attempt));
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
        state.kick_discovery();
        let attempt = state
            .discovery_attempt_token
            .clone()
            .expect("discovery attempt");
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
        state.handle_discovery_result(Some(0), payload, Some(&attempt));
        assert!(!state.provider_states.contains_key("antigravity"));
        assert_eq!(state.discovered_providers, vec!["claude".to_string()]);
    }

    #[test]
    fn late_provider_result_does_not_resurrect_discovery_pruned_provider() {
        let mut state = State {
            source: Source::Cli,
            discovery_in_flight: true,
            discovery_attempt_token: Some("discovery".into()),
            ..State::default()
        };
        state.kick_provider_call("codex");
        let attempt = state
            .provider_states
            .get("codex")
            .and_then(|provider| provider.active_attempt_token.clone())
            .expect("provider attempt");

        state.handle_discovery_result(
            Some(0),
            br#"[{"provider":"claude","enabled":true}]"#.to_vec(),
            Some("discovery"),
        );
        assert!(!state.provider_states.contains_key("codex"));

        assert!(!state.handle_provider_fallback_result(
            "codex",
            Some(&attempt),
            Some(0),
            provider_record(&mixed_payload(), "codex"),
        ));
        assert!(!state.provider_states.contains_key("codex"));
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
        state.kick_discovery();
        let attempt = state
            .discovery_attempt_token
            .clone()
            .expect("discovery attempt");
        let payload = br#"[
            {"provider": "claude", "enabled": true}
        ]"#
        .to_vec();

        state.handle_discovery_result(Some(0), payload, Some(&attempt));

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
        arm_provider_attempt(&mut state, "claude");

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
        arm_provider_attempt(&mut state, "codex");

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
        arm_provider_attempt(&mut state, "codex");

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
    fn per_provider_payload_with_an_invalid_sibling_is_rejected_not_substituted() {
        // parse_usage_payload DROPS invalid records, so the validated list is a
        // subsequence of the raw array. Pairing the two by position would hand
        // back element 0 (the attacker's object) as codex's record.
        let payload =
            br#"[{"provider":"../../pwned","attacker":"controlled"},{"provider":"codex","usage":{"primary":{"usedPercent":7}}}]"#;
        assert_eq!(extract_provider_record(payload, "codex"), Err(()));

        // The same shape, but the decoy is itself renderable: dropped by
        // valid_provider_record (empty `secondary`) yet renderable downstream.
        let spoof = br#"[{"provider":"claude","usage":{"primary":{"usedPercent":99},"secondary":{}}},{"provider":"codex","usage":{"primary":{"usedPercent":3}}}]"#;
        assert_eq!(extract_provider_record(spoof, "codex"), Err(()));
    }

    #[test]
    fn per_provider_payload_still_accepts_clean_and_empty_replies() {
        let clean = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":42}}}]"#;
        let got = extract_provider_record(clean, "codex").expect("clean reply accepted");
        assert_eq!(
            got.and_then(|value| value.get("provider").cloned()),
            Some(serde_json::Value::String("codex".into()))
        );
        assert_eq!(extract_provider_record(b"[]", "codex"), Ok(None));
    }

    fn antigravity_quad_record() -> serde_json::Value {
        let quad = include_str!("../../../test/fixtures/codexbar-antigravity-quad.json");
        serde_json::from_str::<serde_json::Value>(quad).expect("quad fixture")[0].clone()
    }

    const ANTIGRAVITY_OFFLINE: &str =
        include_str!("../../../test/fixtures/codexbar-antigravity-offline.json");

    #[test]
    fn cli_offline_placeholder_keeps_last_known_usage_and_age() {
        let good = antigravity_quad_record();
        let mut state = State {
            source: Source::Cli,
            discovered_providers: vec!["antigravity".to_string()],
            discovered_providers_at: Some(now_seconds()),
            ..State::default()
        };
        let provider = state
            .provider_states
            .entry("antigravity".to_string())
            .or_default();
        provider.in_flight = true;
        provider.active_attempt_token = Some("provider-1".to_string());
        provider.last_record = Some(good.clone());
        provider.last_record_seconds = Some(1_000.0);

        state.handle_provider_fallback_result(
            "antigravity",
            Some("provider-1"),
            Some(0),
            ANTIGRAVITY_OFFLINE.as_bytes().to_vec(),
        );

        let provider = state.provider_states.get("antigravity").expect("state");
        let record = provider.last_record.as_ref().expect("record kept");
        assert_eq!(record["usage"], good["usage"]);
        assert_eq!(provider.last_record_seconds, Some(1_000.0));
        assert!(state.last_output.contains("AG"), "{:?}", state.last_output);
    }

    #[test]
    fn serve_offline_placeholder_keeps_last_known_usage_beside_healthy_providers() {
        let good = antigravity_quad_record();
        let codex = serde_json::json!({"provider":"codex","usage":{"primary":{"usedPercent":10,"windowMinutes":300}}});
        let mut state = State::default();
        let first = serde_json::to_vec(&serde_json::json!([good, codex])).expect("payload");
        assert!(state.accept_payload(first, Source::Serve));
        let seeded_at = state.provider_states["antigravity"].last_record_seconds;

        let offline = serde_json::from_str::<serde_json::Value>(ANTIGRAVITY_OFFLINE)
            .expect("offline fixture")[0]
            .clone();
        let second = serde_json::to_vec(&serde_json::json!([offline, codex])).expect("payload");
        assert!(state.accept_payload(second, Source::Serve));

        let published: serde_json::Value =
            serde_json::from_slice(state.last_payload.as_deref().expect("payload")).expect("json");
        assert_eq!(published[0]["usage"], good["usage"]);
        assert_eq!(
            state.provider_states["antigravity"].last_record_seconds,
            seeded_at
        );
        assert!(state.last_output.contains("AG"), "{:?}", state.last_output);
        // The carried slice ages on its own while codex keeps serve healthy:
        // once its measurement is past the horizon, it alone is marked.
        let now = now_seconds();
        state
            .provider_states
            .get_mut("antigravity")
            .expect("state")
            .last_record_seconds = Some(now - 1_000.0);
        assert_eq!(
            state.stale_provider_slices(now, 60.0, false),
            vec!["antigravity".to_string()]
        );
    }

    #[test]
    fn seeding_binds_each_provider_to_its_own_raw_record() {
        // First element is dropped by validation; the survivors must still be
        // paired with their OWN raw JSON, never shifted by one.
        let payload = br#"[{"provider":"bad/id","usage":{"primary":{"usedPercent":1}}},{"provider":"codex","usage":{"primary":{"usedPercent":11}}},{"provider":"claude","usage":{"primary":{"usedPercent":22}}}]"#;
        let records = parse_usage_payload(payload).expect("payload parses");
        let mut state = State::default();
        state.seed_provider_states_from_payload(&records, payload.to_vec());

        for (provider, used) in [("codex", 11.0), ("claude", 22.0)] {
            let raw = state
                .provider_states
                .get(provider)
                .and_then(|entry| entry.last_record.clone())
                .unwrap_or_else(|| panic!("{provider} seeded"));
            assert_eq!(
                raw.get("provider").and_then(|v| v.as_str()),
                Some(provider),
                "seeded record must be {provider}'s own element"
            );
            assert_eq!(
                raw.pointer("/usage/primary/usedPercent")
                    .and_then(|v| v.as_f64()),
                Some(used)
            );
        }
        assert!(!state.provider_states.contains_key("bad/id"));
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
        entry.last_attempt_seconds = Some(100.0);
        entry.active_attempt_token = Some("old".to_string());

        state.expire_stale_provider_flights(101.0);
        assert!(state
            .provider_states
            .get("codex")
            .is_some_and(|entry| entry.in_flight));

        state.expire_stale_provider_flights(100.0 + state.provider_timeout_seconds + 1.0);

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
    fn permission_results_reset_version_probe_and_start_work_after_grant() {
        let mut denied = State {
            version_probe_in_flight: true,
            version_probe_token: Some("denied".into()),
            version_probe_started_at: Some(now_seconds()),
            ..State::default()
        };
        denied.update(Event::PermissionRequestResult(PermissionStatus::Denied));
        assert!(!denied.permissions_granted);
        assert!(!denied.version_probe_in_flight);
        assert!(denied.version_probe_token.is_none());
        assert!(denied.version_probe_started_at.is_none());

        let mut granted = State {
            source: Source::Unknown,
            cli_fallback: CliFallback::Off,
            version_probe_in_flight: true,
            version_probe_token: Some("granted".into()),
            version_probe_started_at: Some(now_seconds()),
            ..State::default()
        };
        granted.update(Event::PermissionRequestResult(PermissionStatus::Granted));
        assert!(granted.permissions_granted);
        assert!(!granted.version_probe_in_flight);
        assert!(granted.version_probe_token.is_none());
        assert!(granted.version_probe_started_at.is_none());
        assert!(granted.health_in_flight);
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
    fn coordinator_recycles_only_owned_stale_build() {
        let mut state = State::default();
        state.permissions_granted = true;
        state.manage_serve = true;
        state.recycle_owned_serve = true;
        state.managed_serve_pane = Some(PaneId::Terminal(9));
        state.serve_build_version = Some("0.37.1".into());
        state.ondisk_version = Some("0.37.2".into());
        assert!(state.should_recycle_owned_serve());
        arm_health_probe(&mut state, 1);
        let context = current_web_context(&state, HEALTH_KIND);
        state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            br#"{"version":"0.37.1"}"#.to_vec(),
            context,
        ));
        assert!(state
            .take_effects()
            .iter()
            .any(|effect| matches!(effect, Effect::RecycleOwnedServe)));
        state.update(Event::ManagedServeRecycled(true));
        assert!(state.managed_serve_pane.is_none());
        let mut foreign = State::default();
        foreign.serve_build_version = Some("0.37.1".into());
        foreign.ondisk_version = Some("0.37.2".into());
        assert!(!foreign.should_recycle_owned_serve());
    }

    #[test]
    fn coordinator_timeouts_are_host_configurable() {
        let mut state = State::default();
        state.now = 1_700_000_000.0;
        state.usage_timeout_seconds = 90.0;
        state.usage_in_flight = true;
        state.active_usage_generation = Some(1);
        state.web_flight_started_at = Some(1_700_000_000.0 - 31.0);
        assert!(!state.expire_stale_web_flight());
        state.usage_timeout_seconds = 30.0;
        // A fresh derived provider inventory is needed before the expired
        // usage probe falls back; seed discovery so expiry reaches CLI work.
        state.discovered_providers = vec!["codex".into()];
        state.discovered_providers_at = Some(1_700_000_000.0);
        state.expire_stale_web_flight();
        assert!(!state.usage_in_flight);
    }

    #[test]
    fn follower_restore_rebuilds_provider_measurement_times() {
        let mut state = State::default();
        state.now = 1_700_000_000.0;
        assert!(state.restore_payload(mixed_payload(), Source::Serve, 1_699_999_000.0));
        assert_eq!(state.source, Source::Serve);
        for provider in ["codex", "claude"] {
            let entry = state
                .provider_states
                .get(provider)
                .expect("restored provider");
            assert_eq!(entry.last_record_seconds, Some(1_699_999_000.0));
        }
        // Restore already renders the rebuilt measurement times.
        assert!(!state.refresh_output());
    }

    #[test]
    fn follower_repaint_applies_restored_provider_measurement_times() {
        let mut state = State::default();
        state.set_time(1_700_000_000.0);
        state.interval_seconds = 60.0;
        state.render_config.stale_glyph = "OLD".into();
        let payload = br#"[
            {"provider":"codex","usage":{"primary":{"usedPercent":25}}},
            {"provider":"claude","usage":{"primary":{"usedPercent":40}}}
        ]"#
        .to_vec();
        assert!(state.restore_payload(payload, Source::Serve, state.now));
        let fresh = state.last_output.clone();
        assert!(!state.last_output.contains("OLD"));
        state
            .provider_states
            .get_mut("codex")
            .expect("restored codex")
            .last_record_seconds = Some(state.now - 121.0);
        assert!(state.repaint());
        let (fresh_codex, fresh_claude) = fresh.split_once(' ').expect("fresh chunks");
        let (stale_codex, stale_claude) =
            state.last_output.split_once(' ').expect("restored chunks");
        assert_ne!(stale_codex, fresh_codex);
        assert!(stale_codex.contains("108;112;134"), "{stale_codex}");
        assert_eq!(stale_claude, fresh_claude);
        assert!(!state.last_output.contains("OLD"));
        assert!(!state.repaint());
    }

    #[test]
    fn broker_follower_suspends_in_flight_work() {
        let mut state = State::default();
        state.now = 1_700_000_000.0;
        state.permissions_granted = true;
        arm_health_probe(&mut state, 3);
        arm_provider_attempt(&mut state, "codex");
        state.suspend();
        assert!(!state.has_work());
        assert!(state.take_effects().is_empty());
    }

    #[test]
    fn parse_nonnegative_allows_zero_rejects_negative() {
        assert_eq!(parse_nonnegative_f64(Some("0"), 60.0), 0.0);
        assert_eq!(parse_nonnegative_f64(Some("30"), 60.0), 30.0);
        assert_eq!(parse_nonnegative_f64(Some("-5"), 60.0), 60.0);
        assert_eq!(parse_nonnegative_f64(Some("nope"), 60.0), 60.0);
        assert_eq!(parse_nonnegative_f64(None, 60.0), 60.0);
    }

    #[test]
    fn deferred_managed_start_timer_and_spawn_failure_reach_all_cli_providers() {
        let mut state = State {
            now: 1_000.0,
            permissions_granted: true,
            manage_serve: true,
            cli_fallback: CliFallback::Degraded,
            fallback_jitter_seconds: 0.0,
            ..State::default()
        };
        state.update(Event::PermissionRequestResult(PermissionStatus::Granted));
        let health = state
            .take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                Effect::ProbeHealth { context, .. } => Some(context),
                _ => None,
            })
            .expect("initial health probe");
        state.update(Event::WebRequestResult(
            0,
            BTreeMap::new(),
            Vec::new(),
            health,
        ));
        let spawn_after = state
            .managed_serve_spawn_after_seconds
            .expect("deferred startup");
        let scheduled = state.take_effects();
        assert!(scheduled
            .iter()
            .any(|effect| matches!(effect, Effect::Schedule(_))));
        state.set_time(spawn_after + 0.01);
        state.update(Event::Timer(state.now));
        let confirmation = state
            .take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                Effect::ProbeHealth { context, .. } => Some(context),
                _ => None,
            })
            .expect("post-delay confirmation probe");
        state.update(Event::WebRequestResult(
            0,
            BTreeMap::new(),
            Vec::new(),
            confirmation,
        ));
        let spawn = state
            .take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                Effect::StartServe { context, .. } => Some(context),
                _ => None,
            })
            .expect("managed start");
        state.update(Event::ManagedServeStarted(None, spawn));
        let discovery = state
            .take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                Effect::DiscoverProviders { context, .. } => Some(context),
                _ => None,
            })
            .expect("fallback discovery after failed spawn");
        state.update(Event::RunCommandResult(
            Some(0),
            br#"[{"provider":"codex","enabled":true},{"provider":"claude","enabled":true}]"#
                .to_vec(),
            Vec::new(),
            discovery,
        ));
        for effect in state.take_effects() {
            if let Effect::FetchProvider { context, .. } = effect {
                let provider = context[FALLBACK_PROVIDER_CONTEXT_KEY].clone();
                let payload = serde_json::to_vec(&serde_json::json!([{
                    "provider": provider,
                    "usage": {"primary": {"usedPercent": 25}},
                }]))
                .expect("fresh provider array");
                state.update(Event::RunCommandResult(
                    Some(0),
                    payload,
                    Vec::new(),
                    context,
                ));
            }
        }
        let records = parse_usage_payload(state.last_payload.as_deref().expect("CLI payload"))
            .expect("validated CLI array");
        assert_eq!(
            records
                .iter()
                .map(|record| record.provider.as_str())
                .collect::<Vec<_>>(),
            vec!["codex", "claude"]
        );
        assert_eq!(state.source, Source::Cli);
        assert!(!state.has_work());
    }

    #[test]
    fn serve_refresh_keeps_cli_provenance_only_for_carried_measurements() {
        let old = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":10}}},
                       {"provider":"claude","usage":{"primary":{"usedPercent":30}}}]"#
            .to_vec();
        let mut state = State::default();
        state.set_time(1_000.0);
        assert!(state.restore_payload(old, Source::Cli, 1_000.0));
        state.set_time(2_000.0);
        assert!(state.accept_fixture(
            br#"[
            {"provider":"codex","usage":{"primary":{"usedPercent":40}}},
            {"provider":"claude","error":"offline"}
        ]"#
            .to_vec()
        ));
        assert_eq!(
            state.provider_states["codex"].last_record_seconds,
            Some(2_000.0)
        );
        assert_eq!(
            state.provider_states["codex"].last_record_source,
            Some(Source::Serve)
        );
        assert_eq!(
            state.provider_states["claude"].last_record_seconds,
            Some(1_000.0)
        );
        assert_eq!(
            state.provider_states["claude"].last_record_source,
            Some(Source::Cli)
        );
        let payload: serde_json::Value =
            serde_json::from_slice(state.last_payload.as_deref().expect("serve payload"))
                .expect("JSON");
        assert_eq!(payload[1]["usage"]["primary"]["usedPercent"], 30);
    }
}
