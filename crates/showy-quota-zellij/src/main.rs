use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use showy_quota_zellij_core::{
    parse_usage_payload, payload_has_renderable_provider, render_zellij, RenderConfig,
    RenderOptions,
};
use zellij_tile::prelude::*;

const HEALTH_KIND: &str = "showy-quota-health";
const USAGE_KIND: &str = "showy-quota-usage";
const FALLBACK_KIND: &str = "showy-quota-fallback";
const SERVE_FAILURES_BEFORE_CLI: u8 = 3;
const MANAGED_SERVE_RETRY_COOLDOWN_SECONDS: f64 = 30.0;
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
    cli_in_flight: bool,
    permissions_granted: bool,
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
            cli_in_flight: false,
            permissions_granted: false,
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

        shim_request_permission(&[
            PermissionType::WebAccess,
            PermissionType::OpenTerminalsOrPlugins,
            PermissionType::RunCommands,
        ]);
    }
    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.permissions_granted = true;
                self.health_in_flight = false;
                self.usage_in_flight = false;
                self.cli_in_flight = false;
                self.schedule_timer();
                self.tick();
                true
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                self.permissions_granted = false;
                self.health_in_flight = false;
                self.usage_in_flight = false;
                self.cli_in_flight = false;
                self.last_output = " showy-quota: permission denied ".into();
                true
            }
            Event::Timer(_) => {
                self.schedule_timer();
                let changed = self.refresh_output();
                self.tick();
                changed || self.last_payload.is_none()
            }
            Event::Visible(true) => true,
            Event::WebRequestResult(status, _headers, body, context) => {
                match context.get("kind").map(String::as_str) {
                    Some(HEALTH_KIND) => {
                        self.health_in_flight = false;
                        if status == 200 {
                            self.kick_usage();
                        } else {
                            self.handle_serve_unavailable();
                        }
                        true
                    }
                    Some(USAGE_KIND) => {
                        self.usage_in_flight = false;
                        if status == 200 && self.accept_payload(body, Source::Serve) {
                            self.consecutive_serve_failures = 0;
                        } else {
                            self.handle_usage_failure();
                        }
                        true
                    }
                    _ => false,
                }
            }
            Event::RunCommandResult(exit, stdout, _stderr, context) => {
                if context.get("kind").map(String::as_str) != Some(FALLBACK_KIND) {
                    return false;
                }
                self.cli_in_flight = false;
                if !self.should_accept_cli_result() {
                    return false;
                }
                if exit == Some(0) && self.accept_payload(stdout, Source::Cli) {
                    self.last_cli_fetch_seconds = Some(now_seconds());
                    true
                } else {
                    self.render_failure();
                    true
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

    fn should_accept_cli_result(&self) -> bool {
        matches!(
            self.source,
            Source::Cli | Source::Unknown | Source::Unavailable
        )
    }

    fn schedule_timer(&self) {
        shim_set_timeout(match self.source {
            Source::Cli => self.interval_seconds,
            _ => self.interval_seconds,
        });
    }

    fn tick(&mut self) {
        if !self.permissions_granted {
            return;
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
        if self.last_payload.is_some()
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

    fn kick_cli_fallback(&mut self) {
        if !self.permissions_granted || self.cli_in_flight {
            return;
        }
        self.cli_in_flight = true;
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), FALLBACK_KIND.to_string());
        shim_run_command(
            &[
                self.cli_command.as_str(),
                "usage",
                "--format",
                "json",
                "--pretty",
            ],
            context,
        );
    }

    fn accept_payload(&mut self, payload: Vec<u8>, source: Source) -> bool {
        let Ok(records) = parse_usage_payload(&payload) else {
            self.render_failure();
            return false;
        };
        if !payload_has_renderable_provider(&records) && self.last_payload.is_some() {
            self.render_failure();
            return false;
        }
        self.last_payload = Some(payload);
        self.last_success_seconds = Some(now_seconds());
        self.set_source(source);
        self.refresh_output();
        true
    }

    fn render_failure(&mut self) {
        if self.last_payload.is_some() {
            self.refresh_output();
        } else {
            self.last_output = " showy-quota: CodexBar serve unavailable ".into();
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn event_context(kind: &str) -> BTreeMap<String, String> {
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), kind.to_string());
        context
    }

    fn mixed_payload() -> Vec<u8> {
        include_bytes!("../../../test/fixtures/codexbar-mixed.json").to_vec()
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

        assert!(state.update(Event::WebRequestResult(
            200,
            BTreeMap::new(),
            Vec::new(),
            event_context(HEALTH_KIND),
        )));
        assert_eq!(state.source, Source::Cli);

        state.usage_in_flight = true;
        assert!(state.update(Event::WebRequestResult(
            503,
            BTreeMap::new(),
            Vec::new(),
            event_context(USAGE_KIND),
        )));
        assert_eq!(state.source, Source::Cli);
        assert!(state.last_output.contains("⚠cli"));
    }

    #[test]
    fn late_cli_result_is_ignored_after_serve_recovery() {
        let mut state = State {
            permissions_granted: false,
            source: Source::Cli,
            last_payload: Some(mixed_payload()),
            ..State::default()
        };
        assert!(state.refresh_output());
        assert!(state.last_output.contains("⚠cli"));

        assert!(state.accept_payload(mixed_payload(), Source::Serve));
        let serve_output = state.last_output.clone();
        assert!(!serve_output.contains("⚠cli"));

        state.cli_in_flight = true;
        assert!(!state.update(Event::RunCommandResult(
            Some(0),
            mixed_payload(),
            Vec::new(),
            event_context(FALLBACK_KIND),
        )));
        assert_eq!(state.source, Source::Serve);
        assert_eq!(state.last_output, serve_output);
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

        assert!(!state.cli_in_flight);
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
}

#[cfg(target_arch = "wasm32")]
register_plugin!(State);

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
