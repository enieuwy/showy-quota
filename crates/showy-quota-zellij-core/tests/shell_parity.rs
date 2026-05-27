use std::path::{Path, PathBuf};
use std::process::Command;

use showy_quota_zellij_core::{render_zellij, RenderConfig, RenderOptions};

#[derive(Debug, Clone, Copy)]
struct Case<'a> {
    name: &'a str,
    fixture: &'a str,
    color: bool,
    now_epoch: i64,
    stale: bool,
    configure: fn(&mut RenderConfig),
}

#[test]
fn rust_renderer_matches_shell_zellij_renderer() {
    let cases = [
        Case {
            name: "mixed default color",
            fixture: "codexbar-mixed.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            configure: |_| {},
        },
        Case {
            name: "forced sextant no color",
            fixture: "codexbar-sextant.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            configure: |config| {
                config.terminal_bar_mode = "sextant3".into();
                config.zellij_bar_width = 8;
            },
        },
        Case {
            name: "auto mono no color",
            fixture: "codexbar-mono.json",
            color: false,
            now_epoch: 4_070_912_400,
            stale: false,
            configure: |config| {
                config.zellij_bar_width = 8;
            },
        },
        Case {
            name: "mono insert marker no color",
            fixture: "codexbar-mono.json",
            color: false,
            now_epoch: 4_070_912_400,
            stale: false,
            configure: |config| {
                config.zellij_bar_width = 8;
                config.mono3_marker_style = "insert".into();
            },
        },
        Case {
            name: "provider allow filter",
            fixture: "codexbar-mixed.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            configure: |config| {
                config.providers = vec!["claude".into()];
            },
        },
        Case {
            name: "no reset countdown",
            fixture: "codexbar-no-reset.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            configure: |_| {},
        },
        Case {
            name: "idle no reset",
            fixture: "codexbar-idle-no-reset.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            configure: |_| {},
        },
        Case {
            name: "stale mixed color",
            fixture: "codexbar-mixed.json",
            color: true,
            now_epoch: 4_070_928_480,
            stale: true,
            configure: |_| {},
        },
        Case {
            name: "stale mono no marker",
            fixture: "codexbar-mono.json",
            color: true,
            now_epoch: 4_070_912_400,
            stale: true,
            configure: |config| {
                config.zellij_bar_width = 8;
            },
        },
    ];

    for case in cases {
        let root = repo_root();
        let fixture = root.join("test/fixtures").join(case.fixture);
        let mut config = RenderConfig::default();
        (case.configure)(&mut config);
        let payload = std::fs::read(&fixture).expect("fixture readable");
        let rust = render_zellij(
            &payload,
            &config,
            RenderOptions {
                color: case.color,
                stale: case.stale,
                now_epoch: case.now_epoch,
            },
        )
        .unwrap_or_else(|_| panic!("rust render failed for {}", case.name));
        let shell = if case.stale {
            shell_render_stale(&root, &fixture, case)
        } else {
            shell_render_json(&root, &fixture, case)
        };
        assert_eq!(rust, shell, "{}", case.name);
    }
}

fn shell_render_json(root: &Path, fixture: &Path, case: Case<'_>) -> String {
    let mut cmd = base_shell_command(root, case);
    cmd.arg("--json").arg(fixture);
    apply_case_env(&mut cmd, case);
    let output = cmd.output().expect("shell renderer launched");
    assert!(
        output.status.success(),
        "shell renderer failed for {}: {}",
        case.name,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8 output")
}

fn shell_render_stale(root: &Path, fixture: &Path, case: Case<'_>) -> String {
    let temp = std::env::temp_dir().join(format!(
        "showy-quota-stale-{}-{}",
        std::process::id(),
        case.name.replace(' ', "-")
    ));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).expect("temp dir");
    std::fs::copy(fixture, temp.join("usage.json")).expect("copy stale fixture");
    let touch = Command::new("touch")
        .arg("-t")
        .arg("198801010000")
        .arg(temp.join("usage.json"))
        .status()
        .expect("touch stale fixture");
    assert!(touch.success(), "touch failed for stale fixture");

    let mut cmd = base_shell_command(root, case);
    cmd.env("SHOWY_QUOTA_CACHE_DIR", &temp)
        .env("SHOWY_QUOTA_CODEXBAR_BIN", temp.join("no-such-codexbar"))
        .env("SHOWY_QUOTA_CODEXBAR_SERVE_URL", "")
        .env("SHOWY_QUOTA_REFRESH_SECONDS", "1");
    apply_case_env(&mut cmd, case);
    let output = cmd.output().expect("shell stale renderer launched");
    let _ = std::fs::remove_dir_all(&temp);
    assert!(
        output.status.success(),
        "shell stale renderer failed for {}: {}",
        case.name,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8 output")
}

fn base_shell_command(root: &Path, case: Case<'_>) -> Command {
    let mut cmd = Command::new(root.join("bin/showy-quota-zellij-bar"));
    cmd.env("SHOWY_QUOTA_NO_CONFIG", "1")
        .env("SHOWY_QUOTA_NOW_EPOCH", case.now_epoch.to_string());
    if case.color {
        cmd.env("SHOWY_QUOTA_FORCE_COLOR", "1")
            .env_remove("NO_COLOR");
    } else {
        cmd.env("SHOWY_QUOTA_FORCE_COLOR", "0").env("NO_COLOR", "1");
    }
    cmd
}

fn apply_case_env(cmd: &mut Command, case: Case<'_>) {
    let mut config = RenderConfig::default();
    (case.configure)(&mut config);
    if config.terminal_bar_mode != RenderConfig::default().terminal_bar_mode {
        cmd.env("SHOWY_QUOTA_TERMINAL_BAR_MODE", &config.terminal_bar_mode);
    }
    if config.zellij_bar_width != RenderConfig::default().zellij_bar_width {
        cmd.env(
            "SHOWY_QUOTA_ZELLIJ_BAR_WIDTH",
            config.zellij_bar_width.to_string(),
        );
    }
    if config.mono3_marker_style != RenderConfig::default().mono3_marker_style {
        cmd.env("SHOWY_QUOTA_MONO3_MARKER_STYLE", &config.mono3_marker_style);
    }
    if !config.providers.is_empty() {
        cmd.env("SHOWY_QUOTA_PROVIDERS", config.providers.join(","));
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}
