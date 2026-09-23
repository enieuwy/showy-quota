use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use showy_quota_zellij_core::{render_rows, OutputFormat, RenderConfig, RenderOptions};

#[test]
fn every_registered_provider_sigil_matches_shell_and_rendered_rows() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let registry = include_str!("../../../share/providers.tsv");
    let expected: Vec<(&str, &str)> = registry
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| {
            let mut columns = line.split('\t');
            (columns.next().unwrap(), columns.next().unwrap())
        })
        .collect();
    let shell = Command::new("bash")
        .arg("-c")
        .arg("set -euo pipefail; . \"$1/lib/common.sh\"; . \"$1/lib/strip.sh\"; while IFS=$'\\t' read -r id sigil rank font; do [[ -n \"$id\" && \"$id\" != \\#* ]] || continue; printf '%s\\t' \"$id\"; showy_quota_provider_sigil \"$id\"; printf '\\n'; done < \"$1/share/providers.tsv\"")
        .arg("bash")
        .arg(&root)
        .env("XDG_CONFIG_HOME", root.join("test/fixtures"))
        .output()
        .expect("shell registry lookup");
    assert!(
        shell.status.success(),
        "{}",
        String::from_utf8_lossy(&shell.stderr)
    );
    let shell_sigils: HashMap<_, _> = String::from_utf8(shell.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let (id, sigil) = line.split_once('\t').unwrap();
            (id.to_owned(), sigil.to_owned())
        })
        .collect();
    assert_eq!(shell_sigils.len(), expected.len());

    let payload = serde_json::Value::Array(
        expected
            .iter()
            .map(|(id, _)| {
                serde_json::json!({"provider": id, "usage": {"primary": {"usedPercent": 50}}})
            })
            .collect(),
    );
    let options = RenderOptions {
        color: false,
        stale: false,
        degraded_cli: false,
        now_epoch: 4_070_908_800,
        freshness: None,
        stale_providers: &[],
    };
    let rendered = render_rows(
        &serde_json::to_vec(&payload).unwrap(),
        &RenderConfig::default(),
        options,
        OutputFormat::Zellij,
    )
    .expect("registered providers render");
    let rust_sigils: HashMap<_, _> = rendered
        .iter()
        .map(|row| (row.provider.as_str(), row.sigil.as_str()))
        .collect();
    for (id, sigil) in expected {
        assert_eq!(shell_sigils[id], sigil, "shell sigil for {id}");
        assert_eq!(rust_sigils[id], sigil, "Rust sigil for {id}");
    }
}

#[test]
fn unknown_provider_keeps_the_id_prefix_fallback() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let shell = Command::new("bash")
        .arg("-c")
        .arg(". \"$1/lib/common.sh\"; . \"$1/lib/strip.sh\"; showy_quota_provider_sigil cohort")
        .arg("bash")
        .arg(&root)
        .env("XDG_CONFIG_HOME", root.join("test/fixtures"))
        .output()
        .expect("shell fallback");
    assert!(shell.status.success());
    assert_eq!(shell.stdout, b"CO");
    let payload = br#"[{"provider":"cohort","usage":{"primary":{"usedPercent":50}}}]"#;
    let rendered = render_rows(
        payload,
        &RenderConfig::default(),
        RenderOptions {
            color: false,
            stale: false,
            degraded_cli: false,
            now_epoch: 4_070_908_800,
            freshness: None,
            stale_providers: &[],
        },
        OutputFormat::Zellij,
    )
    .expect("unknown provider renders");
    assert_eq!(rendered[0].sigil, "CO");
}
