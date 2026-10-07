#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

mod broker;
mod watchdog;

use broker::Broker;
use showy_quota_zellij_core::coordinator::{self as core, Effect, RefreshCoordinator, Source};
use showy_quota_zellij_core::parse_usage_payload;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use zellij_tile::prelude::*;

const BROKER_PIPE: &str = "showy-quota-broker-v1";
const FIXTURE_KIND: &str = "showy-quota-fixture";

#[derive(Default)]
struct State {
    coordinator: RefreshCoordinator,
    broker: Broker,
    control_pipe_name: String,
    pipe_name: String,
    emit_pipe_format: String,
    debug: bool,
    optional_permissions: bool,
    retried_base_permissions: bool,
    fixture_mode: bool,
    fixture_path: Option<String>,
    fixture_pending: bool,
    next_refresh_at: Option<f64>,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs_f64())
        .unwrap_or(0.0)
}
fn enabled(value: Option<&String>) -> bool {
    value.is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
}
fn safe_pipe(value: Option<&String>) -> String {
    value
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
        .cloned()
        .unwrap_or_default()
}

impl State {
    fn base_permissions(&self) -> Vec<PermissionType> {
        if self.fixture_mode {
            return if self.fixture_path.is_some() {
                vec![PermissionType::RunCommands]
            } else {
                vec![]
            };
        }
        core::requested_permissions(self.coordinator.manage_serve, self.coordinator.cli_fallback)
            .into_iter()
            .map(|permission| match permission {
                core::PermissionType::WebAccess => PermissionType::WebAccess,
                core::PermissionType::OpenTerminalsOrPlugins => {
                    PermissionType::OpenTerminalsOrPlugins
                }
                core::PermissionType::RunCommands => PermissionType::RunCommands,
            })
            .collect()
    }
    fn diagnostics(&self) -> serde_json::Value {
        let mut result = self.coordinator.diagnostics();
        result["permissions"] = serde_json::json!(if self.coordinator.permissions_granted {
            "granted"
        } else if self.coordinator.last_error_class.as_deref() == Some("permission_denied") {
            "denied"
        } else {
            "pending"
        });
        result["broker"] = serde_json::json!({"enabled":self.broker.enabled,"leader":self.broker.leader,
            "role":if !self.broker.enabled {"self_contained"} else if self.broker.is_owner() {"owner"} else {"follower"},
            "lastSeenAgeSeconds":self.broker.last_seen.map(|seen|(self.coordinator.now-seen).max(0.0) as u64)});
        result["fixture"] = serde_json::json!(self.fixture_mode);
        result
    }
    fn send(&self, name: &str, payload: String, destination: Option<u32>) {
        #[cfg(target_arch = "wasm32")]
        {
            let mut message = MessageToPlugin::new(name).with_payload(payload);
            message.destination_plugin_id = destination;
            pipe_message_to_plugin(message);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (name, payload, destination);
    }
    fn announce(&self, destination: Option<u32>, snapshot: bool) {
        if !self.broker.enabled || !self.coordinator.permissions_granted {
            return;
        }
        let mut payload = serde_json::json!({"group":self.broker.group,"sender":self.broker.id,
            "owner":self.broker.is_owner(),"sequence":self.broker.sequence});
        if snapshot && self.broker.is_owner() {
            if let Some(bytes) = self.coordinator.last_payload.as_deref() {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
                    payload["providers"] = value;
                    payload["source"] =
                        serde_json::json!(if self.coordinator.source == Source::Cli {
                            "cli"
                        } else {
                            "serve"
                        });
                    payload["measuredAt"] =
                        serde_json::json!(self.coordinator.last_success_seconds);
                    let meta: BTreeMap<_, _> = self
                        .coordinator
                        .provider_states
                        .iter()
                        .map(|(id, state)| (id, state.last_record_seconds))
                        .collect();
                    payload["providerMeasuredAt"] = serde_json::json!(meta);
                }
            }
        }
        self.send(BROKER_PIPE, payload.to_string(), destination);
    }
    fn after_refresh(&mut self, previous_success: Option<f64>) {
        if self.coordinator.last_success_seconds != previous_success {
            self.broker.next_sequence();
            self.announce(None, true);
            if self.optional_permissions && !self.pipe_name.is_empty() {
                self.send(&self.pipe_name, self.rendered_emission(), None);
            }
        }
    }
    fn rendered_emission(&self) -> String {
        if self.emit_pipe_format == "json" {
            return serde_json::json!({"rendered":self.coordinator.last_output,"diagnostics":self.diagnostics()}).to_string();
        }
        if self.emit_pipe_format == "plain" {
            return self.coordinator.render_output(false);
        }
        self.coordinator.last_output.clone()
    }
    fn execute(&mut self) {
        let effects = self.coordinator.take_effects();
        for effect in effects {
            match effect {
                Effect::Schedule(seconds) => {
                    self.next_refresh_at = Some(self.coordinator.now + seconds);
                    if !self.broker.enabled {
                        self.set_timer(seconds);
                    }
                }
                Effect::ProbeHealth { url, context } | Effect::FetchUsage { url, context } => {
                    #[cfg(target_arch = "wasm32")]
                    web_request(
                        url,
                        HttpVerb::Get,
                        BTreeMap::from([("Accept".into(), "application/json".into())]),
                        Vec::new(),
                        context,
                    );
                    #[cfg(not(target_arch = "wasm32"))]
                    let _ = (url, context);
                }
                Effect::DiscoverProviders { argv, context }
                | Effect::FetchProvider { argv, context } => {
                    let args: Vec<_> = argv.iter().map(String::as_str).collect();
                    let script = watchdog::watchdog_script(15);
                    self.run(&watchdog::watchdog_argv(&script, &args), context);
                }
                Effect::ProbeVersion { binary, context } => {
                    let script = watchdog::version_probe_script(5);
                    self.run(&watchdog::version_probe_argv(&script, &binary), context);
                }
                Effect::StartServe { command, context } => {
                    #[cfg(target_arch = "wasm32")]
                    let pane = open_command_pane_background(
                        CommandToRun {
                            path: command.path,
                            args: command.args,
                            cwd: command.cwd,
                        },
                        context.clone(),
                    )
                    .map(|pane| match pane {
                        PaneId::Terminal(id) => core::PaneId::Terminal(id),
                        PaneId::Plugin(id) => core::PaneId::Plugin(id),
                    });
                    #[cfg(not(target_arch = "wasm32"))]
                    let pane = {
                        let _ = command;
                        None
                    };
                    self.coordinator
                        .update(core::Event::ManagedServeStarted(pane, context));
                    self.execute();
                }
                Effect::RecycleOwnedServe => {
                    // This transport never signals a session-owned command pane.
                    self.coordinator
                        .update(core::Event::ManagedServeRecycled(false));
                    self.execute();
                }
            }
        }
    }
    fn arm_broker_timer(&mut self) {
        if self.coordinator.permissions_granted {
            if let Some(delay) = self
                .broker
                .arm_timer(self.coordinator.now, self.next_refresh_at)
            {
                self.set_timer(delay);
            }
        }
    }
    fn set_timer(&self, seconds: f64) {
        #[cfg(target_arch = "wasm32")]
        set_timeout(seconds.max(0.1));
        #[cfg(not(target_arch = "wasm32"))]
        let _ = seconds;
    }
    fn run(&self, args: &[&str], context: BTreeMap<String, String>) {
        #[cfg(target_arch = "wasm32")]
        run_command(args, context);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (args, context);
    }
    fn start_fixture(&mut self) {
        if self.fixture_pending {
            return;
        }
        if let Some(path) = self.fixture_path.as_deref() {
            self.fixture_pending = true;
            let script = watchdog::watchdog_script(5);
            self.run(
                &watchdog::watchdog_argv(&script, &["cat", "--", path]),
                BTreeMap::from([("kind".into(), FIXTURE_KIND.into())]),
            );
        }
    }
    fn broker_message(&mut self, message: &PipeMessage) -> bool {
        if !self.broker.enabled {
            return false;
        }
        if !matches!(&message.source, PipeSource::Plugin(_)) {
            return false;
        }
        let sender = match &message.source {
            PipeSource::Plugin(sender) => *sender,
            _ => return false,
        };
        let Some(raw) = message.payload.as_deref() else {
            return false;
        };
        if raw.len() > showy_quota_zellij_core::codexbar::MAX_USAGE_JSON_BYTES + 65536 {
            return false;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            return false;
        };
        if value["group"].as_str() != Some(self.broker.group.as_str())
            || value["sender"].as_u64() != Some(sender as u64)
            || sender == self.broker.id
        {
            return false;
        }
        let was_owner = self.broker.is_owner();
        self.broker.observe(
            sender,
            self.coordinator.now,
            value["owner"].as_bool() == Some(true),
        );
        if was_owner && !self.broker.is_owner() {
            self.coordinator.suspend();
            self.next_refresh_at = None;
        }
        if self.broker.is_owner() {
            if value["owner"].as_bool() != Some(true) {
                self.announce(Some(sender), true);
            }
            return false;
        }
        if self.broker.leader != Some(sender) || value["owner"].as_bool() != Some(true) {
            return false;
        }
        let Some(sequence) = value["sequence"].as_u64() else {
            return false;
        };
        if sequence < self.broker.received_sequence {
            return false;
        }
        let Some(providers) = value.get("providers") else {
            return false;
        };
        let Ok(bytes) = serde_json::to_vec(providers) else {
            return false;
        };
        if parse_usage_payload(&bytes).is_err() {
            return false;
        }
        let source = if value["source"].as_str() == Some("cli") {
            Source::Cli
        } else {
            Source::Serve
        };
        let measured = value["measuredAt"]
            .as_f64()
            .filter(|time| time.is_finite() && *time <= self.coordinator.now)
            .unwrap_or(self.coordinator.now);
        self.coordinator.restore_payload(bytes, source, measured);
        if let Some(meta) = value["providerMeasuredAt"].as_object() {
            for (id, time) in meta {
                if let Some(time) = time.as_f64().filter(|time| *time <= self.coordinator.now) {
                    self.coordinator
                        .provider_states
                        .entry(id.clone())
                        .or_default()
                        .last_record_seconds = Some(time);
                }
            }
        }
        self.coordinator.repaint();
        self.broker.received_sequence = sequence;
        true
    }
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.coordinator.set_time(now());
        #[cfg(target_arch = "wasm32")]
        {
            let ids = get_plugin_ids();
            self.coordinator.instance_id = ids.plugin_id;
            self.coordinator.initial_cwd = ids.initial_cwd.to_string_lossy().into_owned();
            set_selectable(false);
            subscribe(&[
                EventType::PermissionRequestResult,
                EventType::Timer,
                EventType::Visible,
                EventType::WebRequestResult,
                EventType::RunCommandResult,
                EventType::CommandPaneExited,
            ]);
        }
        self.coordinator.load(configuration.clone());
        self.debug = enabled(configuration.get("debug"));
        self.control_pipe_name = safe_pipe(configuration.get("control_pipe_name"));
        self.pipe_name = safe_pipe(configuration.get("pipe_name"));
        self.emit_pipe_format = configuration
            .get("emit_pipe_format")
            .filter(|value| matches!(value.as_str(), "ansi" | "plain" | "json"))
            .cloned()
            .unwrap_or_else(|| "ansi".into());
        self.fixture_path = configuration
            .get("fixture")
            .or_else(|| configuration.get("SHOWY_QUOTA_FIXTURE"))
            .filter(|path| !path.is_empty())
            .cloned();
        self.fixture_mode =
            self.fixture_path.is_some() || configuration.contains_key("fixture_json");
        if let Some(json) = configuration.get("fixture_json") {
            if !self.coordinator.accept_fixture(json.as_bytes().to_vec()) {
                self.coordinator.last_error_class = Some("invalid_fixture".into());
                self.coordinator.last_output = " showy-quota: invalid fixture ".into();
            }
            self.coordinator.permissions_granted = true;
        }
        if !self.fixture_mode && enabled(configuration.get("session_broker")) {
            // Include the whole configuration so unlike data or render settings never share a lease.
            let group = serde_json::to_string(&configuration).unwrap_or_default();
            self.broker
                .start(self.coordinator.instance_id, group, self.coordinator.now);
        }
        self.optional_permissions =
            self.broker.enabled || !self.control_pipe_name.is_empty() || !self.pipe_name.is_empty();
        let mut permissions = self.base_permissions();
        if self.optional_permissions {
            permissions.push(PermissionType::ReadCliPipes);
            permissions.push(PermissionType::MessageAndLaunchOtherPlugins);
        }
        #[cfg(target_arch = "wasm32")]
        if !permissions.is_empty() {
            request_permission(&permissions);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = permissions;
    }
    fn update(&mut self, event: Event) -> bool {
        self.coordinator.set_time(now());
        let before = self.coordinator.last_output.clone();
        let previous_success = self.coordinator.last_success_seconds;
        match event {
            Event::PermissionRequestResult(PermissionStatus::Denied)
                if self.optional_permissions && !self.retried_base_permissions =>
            {
                self.retried_base_permissions = true;
                self.optional_permissions = false;
                self.broker.enabled = false;
                self.coordinator
                    .update(core::Event::PermissionRequestResult(
                        core::PermissionStatus::Denied,
                    ));
                #[cfg(target_arch = "wasm32")]
                request_permission(&self.base_permissions());
            }
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                if self.fixture_mode {
                    self.coordinator.permissions_granted = true;
                    self.start_fixture();
                } else if self.broker.enabled {
                    self.coordinator.permissions_granted = true;
                    self.announce(None, false);
                } else {
                    self.coordinator
                        .update(core::Event::PermissionRequestResult(
                            core::PermissionStatus::Granted,
                        ));
                }
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                self.coordinator
                    .update(core::Event::PermissionRequestResult(
                        core::PermissionStatus::Denied,
                    ));
            }
            Event::Timer(_) if self.broker.enabled => {
                self.broker.timer_fired();
                if !self.coordinator.permissions_granted {
                    return false;
                }
                let was_owner = self.broker.is_owner();
                let owner = self.broker.tick(self.coordinator.now);
                self.announce(None, owner);
                if owner && !was_owner {
                    self.coordinator
                        .update(core::Event::PermissionRequestResult(
                            core::PermissionStatus::Granted,
                        ));
                } else if owner
                    && self
                        .next_refresh_at
                        .is_some_and(|deadline| self.coordinator.now >= deadline)
                {
                    self.next_refresh_at = None;
                    self.coordinator.update(core::Event::Timer(0.0));
                } else if !owner {
                    self.coordinator.repaint();
                }
            }
            Event::RunCommandResult(exit, stdout, _, context)
                if context.get("kind").map(String::as_str) == Some(FIXTURE_KIND) =>
            {
                if !self.fixture_pending {
                    return false;
                }
                self.fixture_pending = false;
                if exit != Some(0) || !self.coordinator.accept_fixture(stdout) {
                    self.coordinator.last_error_class = Some("invalid_fixture".into());
                    self.coordinator.last_output = " showy-quota: invalid fixture ".into();
                }
            }
            _ if self.fixture_mode => return false,
            Event::CommandPaneExited(pane, exit, context) => {
                self.coordinator
                    .update(core::Event::CommandPaneExited(pane, exit, context));
            }
            _ if self.broker.enabled && !self.broker.is_owner() => return false,
            Event::Timer(seconds) => {
                self.coordinator.update(core::Event::Timer(seconds));
            }
            Event::Visible(visible) => {
                self.coordinator.update(core::Event::Visible(visible));
            }
            Event::WebRequestResult(status, headers, body, context) => {
                self.coordinator.update(core::Event::WebRequestResult(
                    status, headers, body, context,
                ));
            }
            Event::RunCommandResult(exit, stdout, stderr, context) => {
                self.coordinator
                    .update(core::Event::RunCommandResult(exit, stdout, stderr, context));
            }
            _ => return false,
        }
        self.execute();
        self.arm_broker_timer();
        self.after_refresh(previous_success);
        self.debug || self.coordinator.last_output != before
    }
    fn pipe(&mut self, message: PipeMessage) -> bool {
        self.coordinator.set_time(now());
        if message.name == BROKER_PIPE {
            return self.broker_message(&message);
        }
        if !self.optional_permissions || message.name != self.control_pipe_name {
            return false;
        }
        let Some(command) = message.payload.as_deref() else {
            return false;
        };
        match command.trim() {
            "refresh" => {
                let before = self.coordinator.last_output.clone();
                let previous_success = self.coordinator.last_success_seconds;
                if self.fixture_mode {
                    self.start_fixture();
                } else if self.broker.enabled && !self.broker.is_owner() {
                    if let Some(leader) = self.broker.leader {
                        self.send(&self.control_pipe_name, "refresh".into(), Some(leader));
                    }
                } else if self.coordinator.permissions_granted {
                    self.coordinator.refresh();
                    self.execute();
                    self.arm_broker_timer();
                }
                self.after_refresh(previous_success);
                self.coordinator.last_output != before
            }
            "dump_rendered" | "diagnostics" => {
                #[cfg(target_arch = "wasm32")]
                {
                    let output = if command.trim() == "diagnostics" {
                        self.diagnostics().to_string()
                    } else {
                        self.rendered_emission()
                    };
                    match &message.source {
                        PipeSource::Cli(id) => cli_pipe_output(id, &format!("{output}\n")),
                        PipeSource::Plugin(id) => self.send(&message.name, output, Some(*id)),
                        PipeSource::Keybind => {}
                    }
                }
                true
            }
            _ => false,
        }
    }
    fn render(&mut self, _rows: usize, _cols: usize) {
        if self.debug {
            print!("{}", self.diagnostics());
        } else {
            print!("{}", self.coordinator.last_output);
        }
    }
}

#[cfg(target_arch = "wasm32")]
register_plugin!(State);

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let mut configuration = BTreeMap::new();
    let mut arguments = std::env::args().skip(1);
    let mut diagnostics = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--fixture" => {
                let Some(path) = arguments.next() else {
                    eprintln!("--fixture needs a path");
                    std::process::exit(2);
                };
                let bytes = if path == "-" {
                    use std::io::Read;
                    let mut bytes = String::new();
                    std::io::stdin().read_to_string(&mut bytes).map(|_| bytes)
                } else {
                    std::fs::read_to_string(path)
                };
                match bytes {
                    Ok(bytes) => {
                        configuration.insert("fixture_json".into(), bytes);
                    }
                    Err(error) => {
                        eprintln!("{error}");
                        std::process::exit(1);
                    }
                }
            }
            "--diagnostics" => diagnostics = true,
            "--config" => {
                let Some(value) = arguments.next() else {
                    std::process::exit(2);
                };
                let Some((key, value)) = value.split_once('=') else {
                    std::process::exit(2);
                };
                configuration.insert(key.into(), value.into());
            }
            _ => {
                eprintln!("usage: showy-quota-zellij --fixture FILE|- [--diagnostics] [--config key=value]");
                std::process::exit(2);
            }
        }
    }
    if !configuration.contains_key("fixture_json") {
        eprintln!("the native preview needs --fixture");
        std::process::exit(2);
    }
    let mut state = State::default();
    state.load(configuration);
    if state.coordinator.last_error_class.is_some() {
        eprintln!("{}", state.coordinator.last_output);
        std::process::exit(1);
    }
    if diagnostics {
        println!("{}", state.diagnostics());
    } else {
        state.render(1, 160);
        println!();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;

    fn payload() -> Vec<u8> {
        br#"[{"provider":"codex","usage":{"primary":{"usedPercent":10}}},
             {"provider":"claude","usage":{"primary":{"usedPercent":30}}}]"#
            .to_vec()
    }

    fn message(name: &str, source: PipeSource, payload: String) -> PipeMessage {
        PipeMessage {
            source,
            name: name.into(),
            payload: Some(payload),
            args: BTreeMap::new(),
            is_private: false,
        }
    }

    fn without_color(output: &str) -> String {
        let mut plain = String::new();
        let mut chars = output.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                assert_eq!(chars.next(), Some('['));
                for byte in chars.by_ref() {
                    if ('@'..='~').contains(&byte) {
                        break;
                    }
                }
            } else {
                plain.push(ch);
            }
        }
        plain
    }

    fn assert_plain_parity(state: &State) -> String {
        let plain = state.rendered_emission();
        assert!(!plain.contains('\x1b'));
        assert_eq!(plain, without_color(&state.coordinator.last_output));
        plain
    }

    #[test]
    fn broker_follower_initial_output_marks_old_provider_measurement() {
        let mut state = State::default();
        state.coordinator.set_time(1_700_000_000.0);
        state.coordinator.interval_seconds = 60.0;
        state.coordinator.render_config.stale_glyph = "OLD".into();
        state.broker.start(2, "test".into(), state.coordinator.now);
        let snapshot = serde_json::json!({
            "group": "test", "sender": 1, "owner": true, "sequence": 1,
            "providers": serde_json::from_slice::<serde_json::Value>(&payload()).unwrap(),
            "source": "serve", "measuredAt": state.coordinator.now,
            "providerMeasuredAt": {
                "codex": state.coordinator.now - 121.0,
                "claude": state.coordinator.now
            }
        });

        assert!(state.broker_message(&message(
            BROKER_PIPE,
            PipeSource::Plugin(1),
            snapshot.to_string(),
        )));
        assert_eq!(state.broker.leader, Some(1));
        assert_eq!(state.broker.received_sequence, 1);
        assert!(!state.coordinator.last_output.contains("OLD"));

        let mut fresh = RefreshCoordinator::default();
        fresh.set_time(state.coordinator.now);
        fresh.interval_seconds = 60.0;
        fresh.render_config.stale_glyph = "OLD".into();
        fresh.restore_payload(payload(), Source::Serve, fresh.now);
        assert!(!fresh.last_output.contains("OLD"));
        assert_ne!(state.coordinator.last_output, fresh.last_output);
        let (old_codex, current_claude) = state
            .coordinator
            .last_output
            .split_once(' ')
            .expect("two provider chunks");
        let (fresh_codex, fresh_claude) = fresh
            .last_output
            .split_once(' ')
            .expect("two provider chunks");
        assert!(old_codex.contains("108;112;134"), "{old_codex}");
        assert_ne!(old_codex, fresh_codex);
        assert_eq!(current_claude, fresh_claude);
    }

    #[test]
    fn plain_emission_preserves_stale_cli_and_freshness_state() {
        let mut state = State::default();
        state.emit_pipe_format = "plain".into();
        state.coordinator.set_time(1_700_000_000.0);
        state.coordinator.cli_interval_seconds = 120.0;
        state.coordinator.render_config.freshness = "age+source".into();
        state.coordinator.render_config.stale_glyph = "OLD".into();
        state
            .coordinator
            .restore_payload(payload(), Source::Cli, state.coordinator.now - 185.0);

        let fresh = assert_plain_parity(&state);
        assert!(fresh.contains("⚠cli"));
        assert!(fresh.contains("3m cli"));
        assert!(!fresh.contains("OLD"));

        state.coordinator.set_time(state.coordinator.now + 60.0);
        state.coordinator.repaint();
        let stale = assert_plain_parity(&state);
        assert!(stale.contains("OLD"));
        assert!(!stale.contains("4m cli"));
    }

    #[test]
    fn plain_emission_preserves_provider_age_and_build_warning() {
        let mut state = State::default();
        state.emit_pipe_format = "plain".into();
        state.coordinator.set_time(1_700_000_000.0);
        state.coordinator.interval_seconds = 60.0;
        state.coordinator.render_config.stale_glyph = "OLD".into();
        state.coordinator.render_config.freshness = "age+source".into();
        state.coordinator.show_build_marker = true;
        state.coordinator.serve_build_version = Some("0.37.1".into());
        state.coordinator.ondisk_version = Some("0.37.2".into());
        state
            .coordinator
            .restore_payload(payload(), Source::Serve, state.coordinator.now - 10.0);
        let fresh_output = state.coordinator.last_output.clone();
        state
            .coordinator
            .provider_states
            .get_mut("codex")
            .unwrap()
            .last_record_seconds = Some(state.coordinator.now - 121.0);
        state.coordinator.repaint();

        let plain = assert_plain_parity(&state);
        assert!(!plain.contains("OLD"));
        let (old_codex, current_remainder) = state
            .coordinator
            .last_output
            .split_once(' ')
            .expect("provider chunks and freshness");
        let (fresh_codex, fresh_remainder) = fresh_output
            .split_once(' ')
            .expect("provider chunks and freshness");
        assert!(old_codex.contains("108;112;134"), "{old_codex}");
        assert_ne!(old_codex, fresh_codex);
        assert_eq!(current_remainder, fresh_remainder);
        assert!(plain.contains("serve"));
        assert!(plain.ends_with("⚠ver"));
        assert!(!plain.contains("⚠cli"));
    }

    #[test]
    fn pipe_refresh_repaints_synchronous_empty_inventory_and_announces_success() {
        let mut state = State::default();
        state.optional_permissions = true;
        state.control_pipe_name = "control".into();
        state.coordinator.permissions_granted = true;
        state.coordinator.cli_fallback = core::CliFallback::Degraded;
        state.coordinator.serve_url.clear();
        state.coordinator.set_time(now());
        state.coordinator.discovered_providers_at = Some(state.coordinator.now);
        state
            .coordinator
            .restore_payload(payload(), Source::Cli, state.coordinator.now - 300.0);
        let before = state.coordinator.last_output.clone();
        let previous_success = state.coordinator.last_success_seconds;

        assert!(state.pipe(message("control", PipeSource::Keybind, "refresh".into(),)));
        assert_eq!(
            state.coordinator.last_payload.as_deref(),
            Some(b"[]".as_ref())
        );
        assert_ne!(state.coordinator.last_output, before);
        assert_ne!(state.coordinator.last_success_seconds, previous_success);
        assert_eq!(state.broker.sequence, 1);
    }

    #[test]
    fn pipe_refresh_fixture_waits_for_async_result_before_repainting() {
        let mut state = State::default();
        state.optional_permissions = true;
        state.control_pipe_name = "control".into();
        state.fixture_mode = true;
        state.fixture_path = Some("/fixture.json".into());

        assert!(!state.pipe(message("control", PipeSource::Keybind, "refresh".into(),)));
        assert!(state.fixture_pending);
        assert!(state.coordinator.last_success_seconds.is_none());
        assert_eq!(state.broker.sequence, 0);

        assert!(state.update(Event::RunCommandResult(
            Some(0),
            payload(),
            Vec::new(),
            BTreeMap::from([("kind".into(), FIXTURE_KIND.into())]),
        )));
        assert!(!state.fixture_pending);
        assert_eq!(state.coordinator.last_payload, Some(payload()));
        assert!(!state.coordinator.last_output.is_empty());
        assert_eq!(state.broker.sequence, 1);
    }
}
