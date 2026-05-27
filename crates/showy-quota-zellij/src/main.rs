#[cfg(target_arch = "wasm32")]
use std::collections::BTreeMap;
#[cfg(target_arch = "wasm32")]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_arch = "wasm32")]
use showy_quota_zellij_core::{
    parse_usage_payload, payload_has_renderable_provider, render_zellij, RenderConfig,
    RenderOptions,
};
#[cfg(target_arch = "wasm32")]
use zellij_tile::prelude::*;

#[cfg(target_arch = "wasm32")]
const FETCH_KIND: &str = "showy-quota-fetch";
#[cfg(target_arch = "wasm32")]
const FALLBACK_KIND: &str = "showy-quota-fallback";

#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
struct State {
    render_config: RenderConfig,
    serve_url: String,
    interval_seconds: f64,
    fallback_command: Option<String>,
    last_payload: Option<Vec<u8>>,
    last_success_epoch: Option<i64>,
    last_output: String,
    fetch_in_flight: bool,
    fallback_in_flight: bool,
    permissions_granted: bool,
}

#[cfg(target_arch = "wasm32")]
impl Default for State {
    fn default() -> Self {
        Self {
            render_config: RenderConfig::default(),
            serve_url: "http://127.0.0.1:8080".into(),
            interval_seconds: 10.0,
            fallback_command: None,
            last_payload: None,
            last_success_epoch: None,
            last_output: String::new(),
            fetch_in_flight: false,
            fallback_in_flight: false,
            permissions_granted: false,
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.render_config = RenderConfig::from_kdl_config(&configuration);
        self.serve_url = configuration
            .get("serve_url")
            .or_else(|| configuration.get("SHOWY_QUOTA_CODEXBAR_SERVE_URL"))
            .cloned()
            .unwrap_or_else(|| "http://127.0.0.1:8080".into());
        self.interval_seconds = configuration
            .get("interval_seconds")
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| *value > 0.0)
            .unwrap_or(10.0);
        self.fallback_command = configuration
            .get("fallback_command")
            .or_else(|| configuration.get("SHOWY_QUOTA_CODEXBAR_BIN"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        set_selectable(false);
        subscribe(&[
            EventType::PermissionRequestResult,
            EventType::Timer,
            EventType::Visible,
            EventType::WebRequestResult,
            EventType::RunCommandResult,
        ]);

        let mut permissions = vec![PermissionType::WebAccess];
        if self.fallback_command.is_some() {
            permissions.push(PermissionType::RunCommands);
        }
        request_permission(&permissions);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.permissions_granted = true;
                self.fetch_in_flight = false;
                self.fallback_in_flight = false;
                self.schedule_timer();
                self.kick_fetch();
                true
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                self.permissions_granted = false;
                self.fetch_in_flight = false;
                self.fallback_in_flight = false;
                self.last_output = " showy-quota: permission denied ".into();
                true
            }
            Event::Timer(_) => {
                self.schedule_timer();
                self.kick_fetch();
                false
            }
            Event::Visible(true) => true,
            Event::WebRequestResult(status, _headers, body, context) => {
                if context.get("kind").map(String::as_str) != Some(FETCH_KIND) {
                    return false;
                }
                self.fetch_in_flight = false;
                if status == 200 && self.accept_payload(body) {
                    true
                } else {
                    self.kick_fallback_or_render_failure();
                    true
                }
            }
            Event::RunCommandResult(exit, stdout, _stderr, context) => {
                if context.get("kind").map(String::as_str) != Some(FALLBACK_KIND) {
                    return false;
                }
                self.fallback_in_flight = false;
                if exit == Some(0) && self.accept_payload(stdout) {
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

#[cfg(target_arch = "wasm32")]
impl State {
    fn schedule_timer(&self) {
        set_timeout(self.interval_seconds);
    }

    fn kick_fetch(&mut self) {
        if !self.permissions_granted || self.fetch_in_flight {
            return;
        }
        self.fetch_in_flight = true;
        let mut headers = BTreeMap::new();
        headers.insert("Accept".to_string(), "application/json".to_string());
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), FETCH_KIND.to_string());
        let url = format!("{}/usage", self.serve_url.trim_end_matches('/'));
        web_request(url, HttpVerb::Get, headers, Vec::new(), context);
    }

    fn kick_fallback_or_render_failure(&mut self) {
        let Some(command) = self.fallback_command.as_deref() else {
            self.render_failure();
            return;
        };
        if self.fallback_in_flight {
            return;
        }
        self.fallback_in_flight = true;
        let mut context = BTreeMap::new();
        context.insert("kind".to_string(), FALLBACK_KIND.to_string());
        run_command(&[command, "usage", "--format", "json", "--pretty"], context);
    }

    fn accept_payload(&mut self, payload: Vec<u8>) -> bool {
        let Ok(records) = parse_usage_payload(&payload) else {
            self.render_failure();
            return false;
        };
        if !payload_has_renderable_provider(&records) && self.last_payload.is_some() {
            self.render_failure();
            return false;
        }
        self.last_payload = Some(payload);
        self.last_success_epoch = Some(now_epoch());
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

    fn refresh_output(&mut self) {
        let Some(payload) = self.last_payload.as_deref() else {
            return;
        };
        let now = now_epoch();
        let stale = self
            .last_success_epoch
            .map(|epoch| now.saturating_sub(epoch) > (self.interval_seconds * 2.0) as i64)
            .unwrap_or(false);
        match render_zellij(
            payload,
            &self.render_config,
            RenderOptions {
                color: false,
                stale,
                now_epoch: now,
            },
        ) {
            Ok(output) => self.last_output = output.trim_end_matches(['\r', '\n']).to_string(),
            Err(_) if self.last_output.is_empty() => {
                self.last_output = " showy-quota: invalid CodexBar JSON ".into();
            }
            Err(_) => {}
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(target_arch = "wasm32")]
register_plugin!(State);

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
