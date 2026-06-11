#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use showy_quota_zellij_core::{
    parse_provider_config_payload, parse_usage_payload, payload_has_renderable_provider,
    provider_ids_from_records, render_zellij, valid_provider_id, ProviderConfigError,
    ProviderRecord, RenderConfig, RenderOptions,
};
use zellij_tile::prelude::*;

const HEALTH_KIND: &str = "showy-quota-health";
const USAGE_KIND: &str = "showy-quota-usage";
const FALLBACK_DISCOVER_KIND: &str = "showy-quota-fallback-discover";
const FALLBACK_DISCOVER_ATTEMPT_KEY: &str = "showy-quota-discover-attempt";
const FALLBACK_PROVIDER_KIND: &str = "showy-quota-fallback-provider";
const FALLBACK_PROVIDER_CONTEXT_KEY: &str = "showy-quota-provider";
const FALLBACK_PROVIDER_ATTEMPT_KEY: &str = "showy-quota-provider-attempt";
const SERVE_FAILURES_BEFORE_CLI: u8 = 3;
const MANAGED_SERVE_RETRY_COOLDOWN_SECONDS: f64 = 30.0;
const PROVIDER_DISCOVERY_BACKOFF_SECONDS_DEFAULT: f64 = 60.0;
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
        self.serve_command = configuration
            .get("serve_command")
            .or_else(|| configuration.get("SHOWY_QUOTA_CODEXBAR_BIN"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
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
            .filter(|value| !value.is_empty())
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
                self.discovery_in_flight = false;
                self.discovery_attempt_token = None;
                self.discovery_started_at = None;
                self.usage_after_discovery = false;
                self.serve_inventory_mismatch = false;
                self.clear_all_provider_in_flight();
                self.schedule_timer();
                self.tick();
                self.last_output != previous_output
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                self.permissions_granted = false;
                self.health_in_flight = false;
                self.usage_in_flight = false;
                self.discovery_in_flight = false;
                self.discovery_attempt_token = None;
                self.discovery_started_at = None;
                self.usage_after_discovery = false;
                self.serve_inventory_mismatch = false;
                self.clear_all_provider_in_flight();
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
                        self.health_in_flight = false;
                        if status == 200 {
                            self.kick_usage();
                        } else {
                            self.handle_serve_unavailable();
                        }
                        self.last_output != previous_output
                    }
                    Some(USAGE_KIND) => {
                        self.usage_in_flight = false;
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
        shim_set_timeout(self.interval_seconds);
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


    fn tick(&mut self) {
        if !self.permissions_granted {
            return;
        }
        self.expire_stale_discovery();
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
        self.health_in_flight = true;
        if self.source != Source::Cli {
            self.set_source(Source::Probing);
        }
        let mut headers = BTreeMap::new();
        headers.insert("Accept".to_string(), "application/json".to_string());
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), HEALTH_KIND.to_string());
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
        self.usage_in_flight = true;
        let mut headers = BTreeMap::new();
        headers.insert("Accept".to_string(), "application/json".to_string());
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), USAGE_KIND.to_string());
        let url = format!("{}/usage", self.serve_url.trim_end_matches('/'));
        shim_web_request(url, HttpVerb::Get, headers, Vec::new(), context);
    }

    fn handle_serve_unavailable(&mut self) {
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
            self.set_source(Source::Unavailable);
            self.render_failure();
            return;
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
        if self.cli_fallback == CliFallback::Off || self.discovery_in_flight || !self.permissions_granted {
            return;
        }
        let attempt_token = format!("{:.6}", now_seconds());
        self.discovery_started_at = Some(now_seconds());
        self.discovery_attempt_token = Some(attempt_token.clone());
        self.discovery_in_flight = true;
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), FALLBACK_DISCOVER_KIND.to_string());
        context.insert(
            FALLBACK_DISCOVER_ATTEMPT_KEY.to_string(),
            attempt_token,
        );
        let argv = [
            self.cli_command.as_str(),
            "config",
            "providers",
            "--format",
            "json",
            "--pretty",
        ];
        shim_run_command(&argv, context);
    }

    fn handle_discovery_result(&mut self, exit: Option<i32>, stdout: Vec<u8>, attempt: Option<&str>) {
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
            Err(ProviderConfigError::AllInvalid) | Err(ProviderConfigError::Parse(_)) => {
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
            if elapsed < self.provider_failure_backoff_seconds {
                return true;
            }
        }
        false
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
        shim_run_command(&argv, context);
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
            if let Some(active_attempt) = state.active_attempt_token.as_deref() {
                if attempt != Some(active_attempt) {
                    // Late result from an expired command; ignore it so a
                    // previously wedged provider cannot overwrite newer state.
                    return false;
                }
            } else if attempt.is_some() && state.last_failure_seconds.is_some() && !state.in_flight
            {
                return false;
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
            return self.render_failure();
        };
        if source == Source::Serve
            && self.cli_fallback != CliFallback::Off
            && self.discovered_providers_at.is_some()
        {
            self.serve_inventory_mismatch = false;
            let payload_providers: std::collections::BTreeSet<&str> = records.iter().map(|r| r.provider.as_str()).filter(|id| valid_provider_id(id)).collect();
            let discovered_providers: std::collections::BTreeSet<&str> = self.discovered_providers.iter().map(|s| s.as_str()).filter(|id| valid_provider_id(id)).collect();
            if discovered_providers.is_empty() && !payload_providers.is_empty() {
                self.serve_inventory_mismatch = true;
                return false;
            }
            if !discovered_providers.is_subset(&payload_providers) {
                self.serve_inventory_mismatch = true;
                return false;
            }
            // Serve may have extra providers (newly enabled) — accept the
            // payload but force re-discovery so subsequent ticks use the
            // updated inventory for per-provider fallback eligibility.
            if payload_providers != discovered_providers {
                self.discovered_providers_at = None;
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
                let changed = self.last_output != output;
                if changed {
                    self.last_output.clear();
                    self.last_output.push_str(output);
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

        assert!(!state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            Vec::new(),
            event_context(HEALTH_KIND),
        )));
        assert_eq!(state.source, Source::Cli);

        state.usage_in_flight = true;
        assert!(!state.update(Event::WebRequestResult(
            503,
            BTreeMap::new(),
            Vec::new(),
            event_context(USAGE_KIND),
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

        assert!(!state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            mixed_payload(),
            event_context(USAGE_KIND),
        )));

        assert_eq!(state.consecutive_serve_failures, 0);
        assert_eq!(state.source, Source::Serve);
        assert_eq!(state.last_output, output);
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

        // 1. Exact match is accepted and discovery remains valid
        state.discovered_providers = vec!["claude".to_string(), "codex".to_string(), "gemini".to_string(), "cursor".to_string()];
        state.discovered_providers_at = Some(now_seconds());
        assert!(state.accept_payload(mixed_payload(), Source::Serve));
        assert!(state.discovered_providers_at.is_some());

        // 2. Superset is accepted (serve has new providers) but discovery is invalidated
        state.discovered_providers = vec!["claude".to_string(), "codex".to_string()];
        state.discovered_providers_at = Some(now_seconds());
        assert!(state.accept_payload(mixed_payload(), Source::Serve));
        assert!(state.discovered_providers_at.is_none());

        // 3. Subset is rejected (serve missing expected providers) and discovery remains valid
        // so the fallback path can query the expected providers immediately.
        state.discovered_providers = vec!["claude".to_string(), "codex".to_string(), "gemini".to_string(), "cursor".to_string(), "antigravity".to_string()];
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

        assert!(!state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            Vec::new(),
            event_context(HEALTH_KIND),
        )));

        assert!(state.discovery_in_flight);
        assert!(state.usage_after_discovery);
        assert!(!state.usage_in_flight);

        let attempt = state.discovery_attempt_token.clone().expect("discovery attempt");
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
}
