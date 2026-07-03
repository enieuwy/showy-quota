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
    degraded_cli: bool,
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
            degraded_cli: false,
            configure: |_| {},
        },
        Case {
            name: "antigravity quad mono4 color",
            fixture: "codexbar-antigravity-quad.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.terminal_bar_mode = "mono4".into();
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "codex forced mono4 four distinct windows no color",
            fixture: "codexbar-codex-mono4-four.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.provider_modes = vec![("codex".into(), "mono4".into())];
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "codex forced mono4 three distinct windows collapses mono3",
            fixture: "codexbar-codex-mono4-three.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.provider_modes = vec![("codex".into(), "mono4".into())];
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "antigravity quad dual2 color",
            fixture: "codexbar-antigravity-quad.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.terminal_bar_mode = "dual2".into();
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "antigravity quad dual2 no color",
            fixture: "codexbar-antigravity-quad.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.terminal_bar_mode = "dual2".into();
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "auto mono no color",
            fixture: "codexbar-mono.json",
            color: false,
            now_epoch: 4_070_912_400,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.zellij_bar_width = 8;
            },
        },
        Case {
            name: "antigravity quad mono4 two markers no color",
            fixture: "codexbar-antigravity-quad.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.terminal_bar_mode = "mono4".into();
                config.zellij_bar_width = 12;
                config.mono_markers = vec!["primary".into(), "tertiary".into()];
            },
        },
        Case {
            name: "antigravity oauth one pool dual color",
            fixture: "codexbar-antigravity-oauth.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "antigravity oauth one pool dual no color",
            fixture: "codexbar-antigravity-oauth.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "codex manual pool dual2 color",
            fixture: "codexbar-codex-spark.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.provider_modes = vec![("codex".into(), "dual2".into())];
                config.providers = vec!["codex".into()];
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "codex manual pool dual2 no color",
            fixture: "codexbar-codex-spark.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.provider_modes = vec![("codex".into(), "dual2".into())];
                config.providers = vec!["codex".into()];
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "antigravity dual2 stale dims weekly lanes",
            fixture: "codexbar-antigravity-quad.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: true,
            degraded_cli: false,
            configure: |config| {
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "antigravity three pools caps at two color",
            fixture: "codexbar-antigravity-tri.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "provider allow filter",
            fixture: "codexbar-mixed.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
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
            degraded_cli: false,
            configure: |_| {},
        },
        Case {
            name: "idle no reset",
            fixture: "codexbar-idle-no-reset.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |_| {},
        },
        Case {
            name: "missing primary mono color",
            fixture: "codexbar-missing-primary.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |_| {},
        },
        Case {
            name: "missing primary forced dual no color",
            fixture: "codexbar-missing-primary.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.terminal_bar_mode = "dual".into();
                config.zellij_bar_width = 8;
            },
        },
        Case {
            name: "no tertiary mono collapses to dual color",
            fixture: "codexbar-no-tertiary.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.terminal_bar_mode = "mono3".into();
                config.zellij_bar_width = 8;
            },
        },
        Case {
            name: "cursor shared-cycle mono3 color",
            fixture: "codexbar-cursor.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |_| {},
        },
        Case {
            name: "cursor shared-cycle forced dual color",
            fixture: "codexbar-cursor.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: false,
            configure: |config| {
                config.terminal_bar_mode = "dual".into();
                config.zellij_bar_width = 8;
            },
        },
        Case {
            name: "cursor shared-cycle stale color",
            fixture: "codexbar-cursor.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: true,
            degraded_cli: false,
            configure: |_| {},
        },
        Case {
            name: "antigravity quad mono4 stale color",
            fixture: "codexbar-antigravity-quad.json",
            color: true,
            now_epoch: 4_070_908_800,
            stale: true,
            degraded_cli: false,
            configure: |config| {
                config.terminal_bar_mode = "mono4".into();
                config.zellij_bar_width = 12;
            },
        },
        Case {
            name: "stale mixed color",
            fixture: "codexbar-mixed.json",
            color: true,
            now_epoch: 4_070_928_480,
            stale: true,
            degraded_cli: false,
            configure: |_| {},
        },
        Case {
            name: "stale mono no marker",
            fixture: "codexbar-mono.json",
            color: true,
            now_epoch: 4_070_912_400,
            stale: true,
            degraded_cli: false,
            configure: |config| {
                config.zellij_bar_width = 8;
            },
        },
        Case {
            name: "degraded mixed no color",
            fixture: "codexbar-mixed.json",
            color: false,
            now_epoch: 4_070_908_800,
            stale: false,
            degraded_cli: true,
            configure: |_| {},
        },
        Case {
            name: "stale and degraded mixed no color",
            fixture: "codexbar-mixed.json",
            color: false,
            now_epoch: 4_070_928_480,
            stale: true,
            degraded_cli: true,
            configure: |_| {},
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
                degraded_cli: case.degraded_cli,
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
    for (key, _) in std::env::vars() {
        if key.starts_with("SHOWY_QUOTA_") {
            cmd.env_remove(key);
        }
    }
    cmd.env("SHOWY_QUOTA_NO_CONFIG", "1")
        .env(
            "SHOWY_QUOTA_DEGRADED_CLI",
            if case.degraded_cli { "1" } else { "0" },
        )
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
    let def = RenderConfig::default();
    if config.terminal_bar_mode != def.terminal_bar_mode {
        cmd.env("SHOWY_QUOTA_TERMINAL_BAR_MODE", &config.terminal_bar_mode);
    }
    if config.zellij_bar_width != def.zellij_bar_width {
        cmd.env(
            "SHOWY_QUOTA_ZELLIJ_BAR_WIDTH",
            config.zellij_bar_width.to_string(),
        );
    }
    if config.provider_modes != def.provider_modes {
        let modes = config
            .provider_modes
            .iter()
            .map(|(provider, mode)| format!("{provider}={mode}"))
            .collect::<Vec<_>>()
            .join(",");
        cmd.env("SHOWY_QUOTA_PROVIDER_MODES", modes);
    }
    if config.mono_color_mode != def.mono_color_mode {
        cmd.env("SHOWY_QUOTA_MONO_COLOR_MODE", &config.mono_color_mode);
    }
    if config.mono_markers != def.mono_markers {
        cmd.env("SHOWY_QUOTA_MONO_MARKERS", config.mono_markers.join(","));
    }
    if config.palette_elapsed_long != def.palette_elapsed_long {
        cmd.env(
            "SHOWY_QUOTA_PALETTE_ELAPSED_LONG",
            &config.palette_elapsed_long,
        );
    }
    if config.dim_window_minutes != def.dim_window_minutes {
        cmd.env(
            "SHOWY_QUOTA_DIM_WINDOW_MINUTES",
            config.dim_window_minutes.to_string(),
        );
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
