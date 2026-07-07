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
