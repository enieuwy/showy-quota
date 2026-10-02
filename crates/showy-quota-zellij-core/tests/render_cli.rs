use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use showy_quota_zellij_core::{render_tmux, render_zellij, RenderConfig, RenderOptions};

#[derive(Debug, Clone, Copy)]
enum Format {
    Zellij,
    Tmux,
}

impl Format {
    fn as_arg(self) -> &'static str {
        match self {
            Format::Zellij => "zellij",
            Format::Tmux => "tmux",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Case {
    name: &'static str,
    fixture: &'static str,
    format: Format,
    color: bool,
    now_epoch: i64,
    stale: bool,
    degraded_cli: bool,
    json_stdin: bool,
    env: &'static [(&'static str, &'static str)],
    configure: fn(&mut RenderConfig),
}

#[test]
fn render_cli_matches_in_process_renderer() {
    let cases = [
        Case {
            name: "mixed default color",
            fixture: "codexbar-mixed.json",
            format: Format::Zellij,
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            json_stdin: true,
            env: &[],
            configure: |_| {},
        },
        Case {
            name: "antigravity quad mono4 color",
            fixture: "codexbar-antigravity-quad.json",
            format: Format::Zellij,
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            json_stdin: true,
            env: &[
                ("SHOWY_QUOTA_TERMINAL_BAR_MODE", "mono4"),
                ("SHOWY_QUOTA_ZELLIJ_BAR_WIDTH", "12"),
            ],
            configure: |config| {
                config.terminal_bar_mode = "mono4".into();
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "antigravity quad dual2 no color",
            fixture: "codexbar-antigravity-quad.json",
            format: Format::Zellij,
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            json_stdin: true,
            env: &[
                ("SHOWY_QUOTA_TERMINAL_BAR_MODE", "dual2"),
                ("SHOWY_QUOTA_ZELLIJ_BAR_WIDTH", "12"),
            ],
            configure: |config| {
                config.terminal_bar_mode = "dual2".into();
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "stale mixed color",
            fixture: "codexbar-mixed.json",
            format: Format::Zellij,
            color: true,
            now_epoch: 4_070_928_480,
            stale: true,
            degraded_cli: false,
            json_stdin: true,
            env: &[],
            configure: |_| {},
        },
        Case {
            name: "degraded mixed no color",
            fixture: "codexbar-mixed.json",
            format: Format::Zellij,
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: true,
            json_stdin: true,
            env: &[],
            configure: |_| {},
        },
        Case {
            name: "tmux mixed custom width",
            fixture: "codexbar-mixed.json",
            format: Format::Tmux,
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            json_stdin: false,
            env: &[("SHOWY_QUOTA_TMUX_BAR_WIDTH", "9")],
            configure: |config| {
                config.tmux_bar_width = Some(9);
            },
        },
    ];

    for case in cases {
        assert_cli_case(case);
    }
}

fn assert_cli_case(case: Case) {
    let root = repo_root();
    let fixture = root.join("test/fixtures").join(case.fixture);
    let payload = std::fs::read(&fixture).expect("fixture readable");

    let mut config = RenderConfig::default();
    (case.configure)(&mut config);
    let options = RenderOptions {
        color: case.color,
        stale: case.stale,
        degraded_cli: case.degraded_cli,
        now_epoch: case.now_epoch,
        freshness: None,
        stale_providers: &[],
    };
    let expected = match case.format {
        Format::Zellij => render_zellij(&payload, &config, options),
        Format::Tmux => render_tmux(&payload, &config, options),
    }
    .unwrap_or_else(|_| panic!("in-process render failed for {}", case.name));

    let mut command = Command::new(renderer_bin(&root));
    command
        .env_clear()
        .env("SHOWY_QUOTA_NOW_EPOCH", case.now_epoch.to_string());
    if case.color {
        command.env("SHOWY_QUOTA_FORCE_COLOR", "1");
    } else {
        command
            .env("NO_COLOR", "1")
            .env("SHOWY_QUOTA_FORCE_COLOR", "0");
    }
    for (key, value) in case.env {
        command.env(key, value);
    }

    command
        .arg("--format")
        .arg(case.format.as_arg())
        .arg("--json");
    if case.json_stdin {
        command.arg("-");
    } else {
        command.arg(&fixture);
    }
    if case.stale {
        command.arg("--stale");
    }
    if case.degraded_cli {
        command.arg("--degraded-cli");
    }

    let output = if case.json_stdin {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|err| panic!("spawn renderer for {}: {err}", case.name));
        child
            .stdin
            .as_mut()
            .expect("stdin piped")
            .write_all(&payload)
            .unwrap_or_else(|err| panic!("write renderer stdin for {}: {err}", case.name));
        child
            .wait_with_output()
            .unwrap_or_else(|err| panic!("wait renderer for {}: {err}", case.name))
    } else {
        command
            .output()
            .unwrap_or_else(|err| panic!("run renderer for {}: {err}", case.name))
    };

    assert!(
        output.status.success(),
        "renderer failed for {}: {}",
        case.name,
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = String::from_utf8(output.stdout).expect("renderer stdout utf8");
    assert_eq!(actual, expected, "{}", case.name);
}

fn renderer_bin(root: &Path) -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_showy-quota-render") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_showy-quota-render") {
        return PathBuf::from(path);
    }

    let exe = format!("showy-quota-render{}", std::env::consts::EXE_SUFFIX);
    for profile in ["debug", "release"] {
        let path = root.join("target").join(profile).join(&exe);
        if path.is_file() {
            return path;
        }
    }
    panic!(
        "showy-quota-render binary not found; expected Cargo to provide CARGO_BIN_EXE_showy-quota-render"
    );
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "showy-quota-{label}-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cli_output(command: &mut Command) -> String {
    let output = command.output().expect("renderer runs");
    assert!(
        output.status.success(),
        "renderer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("renderer stdout utf8")
}

fn assert_frame_acknowledgment(body: &str) {
    let dir = TestDir::new(body);
    let input = dir.0.join("usage.json");
    let frame = dir.0.join("frame.txt");
    let pending = dir.0.join("frame.txt.pending");
    let render = || {
        let mut command = Command::new(renderer_bin(&repo_root()));
        command
            .env_clear()
            .env("SHOWY_QUOTA_NOW_EPOCH", "4070908800")
            .env("SHOWY_QUOTA_SKETCHYBAR_BODY", body)
            .args(["--emit", "sketchybar-frame", "--assume-declared", "--json"])
            .arg(&input)
            .arg("--frame")
            .arg(&frame);
        cli_output(&mut command)
    };
    let ack = || {
        cli_output(
            Command::new(renderer_bin(&repo_root()))
                .env_clear()
                .args(["--emit", "sketchybar-ack", "--frame"])
                .arg(&frame),
        )
    };
    let set = |wire: &str| {
        wire.lines()
            .find(|line| *line == "set" || line.starts_with("set\u{1f}"))
            .expect("set record")
            .to_owned()
    };

    std::fs::write(
        &input,
        br#"[{"provider":"codex","usage":{"primary":{"usedPercent":10}}}]"#,
    )
    .unwrap();
    assert!(set(&render()).contains("--set"));
    assert!(
        !frame.exists(),
        "rendering must not acknowledge the first frame"
    );
    let first_frame = std::fs::read(&pending).unwrap();
    assert_eq!(ack(), "");
    assert_eq!(std::fs::read(&frame).unwrap(), first_frame);
    assert!(!pending.exists());
    assert_eq!(set(&render()), "set");

    std::fs::write(
        &input,
        br#"[{"provider":"codex","usage":{"primary":{"usedPercent":90}}}]"#,
    )
    .unwrap();
    let changed = set(&render());
    assert!(changed.contains("--set"));
    let changed_frame = std::fs::read(&pending).unwrap();
    assert_ne!(changed_frame, first_frame);
    assert_eq!(std::fs::read(&frame).unwrap(), first_frame);
    // A failed send, or a crash before sending, never runs ack. The next
    // identical tick must therefore repeat the changed arguments.
    assert_eq!(set(&render()), changed);
    assert_eq!(std::fs::read(&frame).unwrap(), first_frame);
    assert_eq!(ack(), "");
    assert_eq!(std::fs::read(&frame).unwrap(), changed_frame);
    assert_eq!(set(&render()), "set");
}

#[test]
fn rows_frame_resends_until_successful_send_is_acknowledged() {
    assert_frame_acknowledgment("rows");
}

#[test]
fn ring_frame_resends_until_successful_send_is_acknowledged() {
    assert_frame_acknowledgment("ring");
}

#[test]
fn frame_ack_without_pending_preserves_acknowledged_state() {
    let dir = TestDir::new("missing-pending");
    let frame = dir.0.join("frame.txt");
    std::fs::write(&frame, "previous frame\n").unwrap();
    let output = Command::new(renderer_bin(&repo_root()))
        .env_clear()
        .args(["--emit", "sketchybar-ack", "--frame"])
        .arg(&frame)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(&frame).unwrap(), "previous frame\n");
}

#[test]
fn cached_prompt_marks_the_selected_carried_forward_provider_stale() {
    let dir = TestDir::new("stale-prompt");
    let input = dir.0.join("usage.json");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let envelope = serde_json::json!({
        "schema": "showy-quota/cache@2",
        "source": "serve",
        "providers": [
            {"provider": "codex", "usage": {"primary": {"usedPercent": 90}}},
            {"provider": "claude", "usage": {"primary": {"usedPercent": 10}}}
        ],
        "providerMeta": {
            "codex": {"source": "serve", "updatedAt": now - 1000},
            "claude": {"source": "serve", "updatedAt": now}
        }
    });
    std::fs::write(&input, serde_json::to_vec(&envelope).unwrap()).unwrap();
    for (provider, format, expected) in [
        (None, None, "CX 90% ⚠\n"),
        (Some("claude"), None, "CL 10%\n"),
        (None, Some("{provider}{stale}"), "codex ⚠\n"),
        (Some("claude"), Some("{provider}{stale}"), "claude\n"),
    ] {
        let mut command = Command::new(renderer_bin(&repo_root()));
        command
            .env_clear()
            .env("SHOWY_QUOTA_NOW_EPOCH", now.to_string())
            .env("SHOWY_QUOTA_USAGE_FILE", &input)
            .args(["--emit", "prompt", "--from-cache"]);
        if let Some(provider) = provider {
            command.args(["--provider", provider]);
        }
        if let Some(format) = format {
            command.args(["--format", format]);
        }
        assert_eq!(cli_output(&mut command), expected);
    }
}
