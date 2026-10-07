use showy_quota_zellij_core::sketchybar_ring::ring_units;
use showy_quota_zellij_core::{
    render_plan, render_rows, render_tmux, render_zellij, sketchybar_rows, Freshness, OutputFormat,
    ProviderRecord, RenderConfig, RenderOptions, Severity, SketchybarOptions,
};
use std::collections::BTreeMap;

const NOW: i64 = 4_070_908_800;
const THREE: &[u8] = br#"[{"provider":"claude","usage":{"primary":{"usedPercent":50,"windowMinutes":300,"resetsAt":"2099-01-01T01:00:00Z"},"secondary":{"usedPercent":99,"windowMinutes":10080,"resetsAt":"2099-01-02T00:00:00Z"},"tertiary":{"usedPercent":100,"windowMinutes":20160,"resetsAt":"2099-01-03T00:00:00Z"}}}]"#;
fn options() -> RenderOptions<'static> {
    RenderOptions {
        color: false,
        stale: false,
        degraded_cli: false,
        now_epoch: NOW,
        freshness: None,
        stale_providers: &[],
    }
}
fn sketch_options() -> SketchybarOptions<'static> {
    SketchybarOptions {
        stale: false,
        degraded_cli: false,
        bar_width: 60,
        stale_providers: &[],
    }
}
fn config(values: &[(&str, &str)]) -> RenderConfig {
    RenderConfig::from_env_map(
        &values
            .iter()
            .map(|(k, v)| (format!("SHOWY_QUOTA_{k}"), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
    )
}

#[test]
fn policy_precedence_and_inclusive_boundaries() {
    let cfg = config(&[
        (
            "PROVIDER_THRESHOLDS",
            "claude=good:70,warn:30,time:90;codex=good:60",
        ),
        (
            "WINDOW_THRESHOLDS",
            "cap=warn:40;claude.secondary=good:80,warn:50,time:120",
        ),
    ]);
    let p = cfg.policy("claude", "secondary", Some(10080));
    assert_eq!((p.good, p.warn, p.time), (80, 50, 120));
    assert_eq!(p.severity(80), Severity::Good);
    assert_eq!(p.severity(79), Severity::Warn);
    assert_eq!(p.severity(50), Severity::Warn);
    assert_eq!(p.severity(49), Severity::Bad);
    assert_eq!(cfg.policy("codex", "primary", Some(300)).good, 60);
    assert_eq!(cfg.policy("gemini", "primary", Some(300)).good, 40);
}

#[test]
fn same_remaining_has_distinct_provider_bands() {
    let cfg = config(&[("PROVIDER_THRESHOLDS", "claude=good:70,warn:60")]);
    let payload = br#"[{"provider":"claude","usage":{"primary":{"usedPercent":50}}},{"provider":"codex","usage":{"primary":{"usedPercent":50}}}]"#;
    let rows = render_rows(payload, &cfg, options(), OutputFormat::Zellij).unwrap();
    assert_eq!(
        rows.iter()
            .find(|r| r.provider == "claude")
            .unwrap()
            .severity,
        Some(Severity::Bad)
    );
    assert_eq!(
        rows.iter()
            .find(|r| r.provider == "codex")
            .unwrap()
            .severity,
        Some(Severity::Good)
    );
}

#[test]
fn hidden_windows_cannot_change_color_countdown_or_native_pacing() {
    let cfg = config(&[("WINDOWS", "primary"), ("TERMINAL_BAR_MODE", "mono3")]);
    let rows = render_rows(THREE, &cfg, options(), OutputFormat::Zellij).unwrap();
    assert_eq!(rows[0].severity, Some(Severity::Good));
    assert!(rows[0].text.contains("1h"));
    let native = sketchybar_rows(THREE, &cfg, NOW, sketch_options()).unwrap();
    assert!(native.rows[0].lanes[0].present);
    assert!(!native.rows[0].lanes[1].present);
    assert!(!native.rows[0].lanes[2].present);
    assert_eq!(native.rows[0].lanes[1].marker, None);
    let ring = ring_units(THREE, &cfg, NOW, sketch_options()).unwrap();
    assert_eq!(ring[0].ring.remaining, 50);
    assert!(ring[0].bars.is_empty());
    assert_eq!(ring[0].label, "1h");
}

#[test]
fn secondary_visibility_drives_countdown_and_its_own_policy_after_compaction() {
    let cfg = config(&[
        ("WINDOWS", "secondary"),
        ("WINDOW_THRESHOLDS", "claude.secondary=good:2,warn:1"),
    ]);
    let rows = render_rows(THREE, &cfg, options(), OutputFormat::Zellij).unwrap();
    assert_eq!(rows[0].severity, Some(Severity::Warn));
    assert!(rows[0].text.contains("1d"));
    let units = ring_units(THREE, &cfg, NOW, sketch_options()).unwrap();
    assert_eq!(units[0].ring.policy.severity(1), Severity::Warn);
    assert!(!units[0].ring.blocked);
}

#[test]
fn portable_output_keeps_numeric_windows_and_uses_ascii_with_state_flags() {
    let cfg = config(&[("TERMINAL_BAR_MODE", "portable")]);
    let output = render_zellij(
        THREE,
        &cfg,
        RenderOptions {
            stale: true,
            degraded_cli: true,
            ..options()
        },
    )
    .unwrap();
    assert_eq!(output, "CL 50%/1h 1%/1d 0%/2d stale cli\n");
    assert!(output.is_ascii());
}

#[test]
fn adaptive_width_and_count_keep_whole_provider_groups_and_overflow() {
    let payload = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":10}}},{"provider":"claude","usage":{"primary":{"usedPercent":99}}},{"provider":"gemini","usage":{"primary":{"usedPercent":20}}}]"#;
    let cfg = config(&[
        ("TERMINAL_BAR_MODE", "portable"),
        ("COMPACT_PROVIDER_COUNT", "1"),
        ("COMPACT_ORDER", "urgency"),
        ("WIDTH_BUDGET", "12"),
    ]);
    assert_eq!(
        render_zellij(payload, &cfg, options()).unwrap(),
        "CL 1%/? +2\n"
    );
    let small = config(&[("TERMINAL_BAR_MODE", "portable"), ("WIDTH_BUDGET", "4")]);
    assert_eq!(render_zellij(payload, &small, options()).unwrap(), "+3\n");
}

#[test]
fn adaptive_width_reserves_final_state_markers_on_both_terminal_surfaces() {
    let payload = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":10}}},{"provider":"claude","usage":{"primary":{"usedPercent":99}}},{"provider":"gemini","usage":{"primary":{"usedPercent":20}}}]"#;
    let state = RenderOptions {
        stale: true,
        degraded_cli: true,
        ..options()
    };
    let cfg = config(&[
        ("TERMINAL_BAR_MODE", "portable"),
        ("COMPACT_ORDER", "urgency"),
        ("WIDTH_BUDGET", "20"),
    ]);
    assert_eq!(
        render_zellij(payload, &cfg, state).unwrap(),
        "CL 1%/? +2 stale cli\n"
    );
    let tmux = render_tmux(payload, &cfg, state).unwrap();
    assert!(tmux.contains("CL 1%/?"), "{tmux}");
    assert!(tmux.contains("+2"), "{tmux}");
    let smaller = RenderConfig {
        width_budget: 19,
        ..cfg
    };
    assert_eq!(
        render_zellij(payload, &smaller, state).unwrap(),
        "+3 stale cli\n"
    );
    let tmux = render_tmux(payload, &smaller, state).unwrap();
    assert!(!tmux.contains("CL"), "{tmux}");
    assert!(tmux.contains("+3"), "{tmux}");
}

#[test]
fn adaptive_width_reserves_freshness_and_rejects_an_unfittable_overflow() {
    let payload = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":10}}},{"provider":"claude","usage":{"primary":{"usedPercent":99}}},{"provider":"gemini","usage":{"primary":{"usedPercent":20}}}]"#;
    let cfg = config(&[
        ("TERMINAL_BAR_MODE", "portable"),
        ("COMPACT_ORDER", "urgency"),
        ("FRESHNESS", "age+source"),
        ("WIDTH_BUDGET", "18"),
    ]);
    let fresh = RenderOptions {
        freshness: Some(Freshness {
            age_seconds: 15,
            source: "cli",
        }),
        ..options()
    };
    assert_eq!(
        render_zellij(payload, &cfg, fresh).unwrap(),
        "CL 1%/? +2 15s cli\n"
    );
    let tiny = RenderConfig {
        width_budget: 1,
        ..cfg
    };
    assert_eq!(render_zellij(payload, &tiny, options()).unwrap(), "\n");
    assert_eq!(render_tmux(payload, &tiny, options()).unwrap(), "");
    assert_eq!(render_zellij(payload, &tiny, fresh).unwrap(), "\n");
}

#[test]
fn adaptive_width_counts_wide_and_combining_glyphs_in_provider_chunks() {
    let payload = br#"[{"provider":"claude","error":"authorization failed"}]"#;
    for (glyph, adjustment) in [("界", 1isize), ("e\u{301}", -1isize)] {
        let cfg = config(&[("ERROR_GLYPH", glyph)]);
        let baseline = render_zellij(payload, &cfg, options()).unwrap();
        assert!(baseline.contains(glyph), "{baseline}");
        // Every other glyph in this error chunk occupies one terminal cell.
        let cells = baseline.trim_end_matches('\n').chars().count();
        let exact = RenderConfig {
            width_budget: cells.checked_add_signed(adjustment).unwrap(),
            ..cfg
        };
        assert_eq!(
            render_zellij(payload, &exact, options()).unwrap(),
            baseline,
            "{glyph}"
        );
        let smaller = RenderConfig {
            width_budget: exact.width_budget - 1,
            ..exact
        };
        assert_eq!(
            render_zellij(payload, &smaller, options()).unwrap(),
            "+1\n",
            "{glyph}"
        );
    }
}

#[test]
fn adaptive_width_counts_wide_and_combining_final_state_glyphs() {
    let payload = br#"[{"provider":"claude","usage":{"primary":{"usedPercent":50}}}]"#;
    let cfg = config(&[("STALE_GLYPH", "界"), ("DEGRADED_CLI_GLYPH", "e\u{301}")]);
    let state = RenderOptions {
        stale: true,
        degraded_cli: true,
        ..options()
    };
    let baseline = render_zellij(payload, &cfg, options()).unwrap();
    let body = baseline.trim_end_matches('\n');
    let exact = RenderConfig {
        width_budget: body.chars().count() + 5,
        ..cfg
    };
    assert_eq!(
        render_zellij(payload, &exact, state).unwrap(),
        format!("{body} 界 e\u{301}\n")
    );
    let smaller = RenderConfig {
        width_budget: exact.width_budget - 1,
        ..exact
    };
    assert_eq!(
        render_zellij(payload, &smaller, state).unwrap(),
        "+1 界 e\u{301}\n"
    );
}

const GENERIC_POOLS: &[u8] = br#"[{"provider":"codex","usage":{
    "primary":{"usedPercent":20,"windowMinutes":300,"resetsAt":"2099-01-01T01:00:00Z"},
    "secondary":{"usedPercent":80,"windowMinutes":10080,"resetsAt":"2099-01-02T00:00:00Z"},
    "extraRateWindows":[
        {"id":"alpha-5h","title":"Alpha Session","window":{"usedPercent":20,"windowMinutes":300,"resetsAt":"2099-01-01T01:00:00Z"}},
        {"id":"alpha-weekly","title":"Alpha Weekly","window":{"usedPercent":80,"windowMinutes":10080,"resetsAt":"2099-01-02T00:00:00Z"}},
        {"id":"beta-5h","title":"Beta Session","window":{"usedPercent":30,"windowMinutes":300,"resetsAt":"2099-01-01T02:00:00Z"}},
        {"id":"beta-weekly","title":"Beta Weekly","window":{"usedPercent":90,"windowMinutes":10080,"resetsAt":"2099-01-03T00:00:00Z"}}
    ]
}}]"#;

#[test]
fn generic_pool_families_survive_visibility_and_input_reordering() {
    let source: serde_json::Value = serde_json::from_slice(GENERIC_POOLS).unwrap();
    for order in [[0, 1, 2, 3], [2, 0, 3, 1], [3, 1, 2, 0]] {
        let mut payload = source.clone();
        payload[0]["usage"]["extraRateWindows"] = serde_json::Value::Array(
            order
                .into_iter()
                .map(|index| source[0]["usage"]["extraRateWindows"][index].clone())
                .collect(),
        );
        let bytes = serde_json::to_vec(&payload).unwrap();
        let records: Vec<ProviderRecord> = serde_json::from_slice(&bytes).unwrap();
        for (mode, remaining, bars) in [
            ("all", [20, 10], Some([80, 70])),
            ("session-only", [80, 70], None),
            ("weekly", [20, 10], None),
        ] {
            let cfg = RenderConfig {
                window_mode: mode.into(),
                ..RenderConfig::default()
            };
            let units = ring_units(&bytes, &cfg, NOW, sketch_options()).unwrap();
            let readings: Vec<_> = units
                .iter()
                .map(|unit| {
                    (
                        unit.unit.as_str(),
                        unit.title.as_str(),
                        unit.pool,
                        unit.ring.remaining,
                        unit.bars
                            .iter()
                            .map(|bar| bar.remaining)
                            .collect::<Vec<_>>(),
                    )
                })
                .collect();
            assert_eq!(
                readings,
                vec![
                    (
                        "codex.p0",
                        "Alpha",
                        Some('A'),
                        remaining[0],
                        bars.map_or(vec![], |b| vec![b[0]]),
                    ),
                    (
                        "codex.p1",
                        "Beta",
                        Some('B'),
                        remaining[1],
                        bars.map_or(vec![], |b| vec![b[1]]),
                    ),
                ],
                "{mode}, {order:?}"
            );
            let plan = render_plan(&records, &cfg);
            let families: Vec<_> = plan[0]
                .families
                .iter()
                .map(|family| {
                    (
                        family.label.as_str(),
                        family
                            .lanes
                            .iter()
                            .map(|lane| lane.remaining_percent)
                            .collect::<Vec<_>>(),
                    )
                })
                .collect();
            let expected_lanes = if let Some(bars) = bars {
                vec![
                    vec![bars[0] as i32, remaining[0] as i32],
                    vec![bars[1] as i32, remaining[1] as i32],
                ]
            } else {
                vec![vec![remaining[0] as i32], vec![remaining[1] as i32]]
            };
            assert_eq!(
                families,
                vec![
                    ("A", expected_lanes[0].clone()),
                    ("B", expected_lanes[1].clone()),
                ],
                "{mode}, {order:?}"
            );
        }
    }
}

#[test]
fn hiding_a_whole_generic_family_does_not_rename_the_remaining_pool() {
    let mut payload: serde_json::Value = serde_json::from_slice(GENERIC_POOLS).unwrap();
    payload[0]["usage"]["secondary"]["windowMinutes"] = serde_json::json!(300);
    payload[0]["usage"]["extraRateWindows"][1]["window"]["windowMinutes"] = serde_json::json!(300);
    let cfg = RenderConfig {
        window_mode: "weekly".into(),
        ..RenderConfig::default()
    };
    let units = ring_units(
        &serde_json::to_vec(&payload).unwrap(),
        &cfg,
        NOW,
        sketch_options(),
    )
    .unwrap();
    assert_eq!(
        units
            .iter()
            .map(|unit| (unit.unit.as_str(), unit.title.as_str(), unit.ring.remaining))
            .collect::<Vec<_>>(),
        vec![("codex.p1", "Beta", 10)]
    );
}

#[test]
fn generic_pool_ids_resolve_families_when_titles_are_absent() {
    let mut payload: serde_json::Value = serde_json::from_slice(GENERIC_POOLS).unwrap();
    for extra in payload[0]["usage"]["extraRateWindows"]
        .as_array_mut()
        .unwrap()
    {
        extra.as_object_mut().unwrap().remove("title");
    }
    let cfg = RenderConfig {
        window_mode: "session-only".into(),
        ..RenderConfig::default()
    };
    let units = ring_units(
        &serde_json::to_vec(&payload).unwrap(),
        &cfg,
        NOW,
        sketch_options(),
    )
    .unwrap();
    assert_eq!(
        units
            .iter()
            .map(|unit| (unit.unit.as_str(), unit.title.as_str(), unit.ring.remaining))
            .collect::<Vec<_>>(),
        vec![("codex.p0", "alpha", 80), ("codex.p1", "beta", 70)]
    );
}

#[test]
fn plans_explain_pool_split_single_pool_collapse_and_shared_marker_deduplication() {
    let fixture = |name: &str| {
        std::fs::read(format!(
            "{}/../../test/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    };
    let quad: Vec<ProviderRecord> =
        serde_json::from_slice(&fixture("codexbar-antigravity-quad.json")).unwrap();
    let plan = render_plan(&quad, &RenderConfig::default());
    assert_eq!(plan[0].effective_mode, "dual2");
    assert_eq!(plan[0].families.len(), 2);
    assert_eq!(plan[0].families[0].lanes.len(), 2);
    let oauth: Vec<ProviderRecord> =
        serde_json::from_slice(&fixture("codexbar-antigravity-oauth.json")).unwrap();
    assert_eq!(
        render_plan(&oauth, &RenderConfig::default())[0].effective_mode,
        "dual"
    );
    let no_tertiary: Vec<ProviderRecord> =
        serde_json::from_slice(&fixture("codexbar-no-tertiary.json")).unwrap();
    let plan = render_plan(&no_tertiary, &config(&[("TERMINAL_BAR_MODE", "mono3")]));
    assert!(plan
        .iter()
        .all(|provider| provider.effective_mode == "dual"));
    let cursor: Vec<ProviderRecord> =
        serde_json::from_slice(&fixture("codexbar-cursor.json")).unwrap();
    let plan = render_plan(&cursor, &RenderConfig::default());
    assert!(plan[0].shared_cycle);
    assert_eq!(plan[0].marker_slots, ["primary"]);
    let records: Vec<ProviderRecord> = serde_json::from_slice(THREE).unwrap();
    let plan = render_plan(
        &records,
        &config(&[("WINDOWS", "primary"), ("TERMINAL_BAR_MODE", "mono3")]),
    );
    assert_eq!(plan[0].requested_mode, "mono3");
    assert_eq!(plan[0].effective_mode, "dual");
    assert_eq!(
        plan[0].collapse_reason.as_deref(),
        Some("insufficient-windows")
    );
}

#[test]
fn named_theme_resolves_before_explicit_palette_override() {
    let kdl = BTreeMap::from([
        ("theme".into(), "nord".into()),
        ("palette_primary_good".into(), "112233".into()),
    ]);
    let cfg = RenderConfig::from_kdl_config(&kdl);
    assert_eq!(cfg.palette_primary_good, "112233");
    let catalog: serde_json::Value =
        serde_json::from_str(include_str!("../../../share/themes/catalog.json")).unwrap();
    assert_eq!(
        cfg.palette_bg,
        catalog["nord"]["SHOWY_QUOTA_PALETTE_BG"].as_str().unwrap()
    );
    assert!(cfg.theme_error.is_none());
    assert!(config(&[("THEME", "not-a-theme")]).theme_error.is_some());
}

#[test]
fn native_pooled_units_reuse_ring_groups_and_filter_session_caps() {
    let quad = include_bytes!("../../../test/fixtures/codexbar-antigravity-quad.json");
    let units = ring_units(quad, &RenderConfig::default(), NOW, sketch_options()).unwrap();
    assert_eq!(
        units
            .iter()
            .map(|unit| unit.unit.as_str())
            .collect::<Vec<_>>(),
        ["antigravity.g", "antigravity.c"]
    );
    assert_eq!(units[0].ring.remaining, 0);
    assert_eq!(units[0].bars[0].remaining, 65);
    assert!(units[0].bars[0].blocked);
    assert_eq!(units[1].ring.remaining, 18);
    assert_eq!(units[1].bars[0].remaining, 90);
    let cfg = config(&[("WINDOW_MODE", "session-only")]);
    let units = ring_units(quad, &cfg, NOW, sketch_options()).unwrap();
    assert_eq!(
        units
            .iter()
            .map(|unit| unit.ring.remaining)
            .collect::<Vec<_>>(),
        [65, 90]
    );
    assert!(units
        .iter()
        .all(|unit| unit.bars.is_empty() && !unit.ring.blocked));
}

#[test]
fn native_compact_boundary_hides_lanes_but_preserves_the_ring_and_popup() {
    use showy_quota_zellij_core::sketchybar_frame::{
        build_ring_frame, FrameSettings, RingFrameInputs,
    };
    let payload = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":20},"secondary":{"usedPercent":30}}},{"provider":"claude","usage":{"primary":{"usedPercent":40},"secondary":{"usedPercent":50}}}]"#;
    let cfg = RenderConfig::default();
    let units = ring_units(payload, &cfg, NOW, sketch_options()).unwrap();
    let env = BTreeMap::from([
        ("SHOWY_QUOTA_SKETCHYBAR_AUTO_COMPACT", "on"),
        ("SHOWY_QUOTA_SKETCHYBAR_COMPACT_PROVIDER_COUNT", "1"),
    ]);
    let mut settings =
        FrameSettings::from_getter(|key| env.get(key).map(|value| value.to_string()), &cfg);
    let frame = |settings: &FrameSettings| {
        build_ring_frame(&RingFrameInputs {
            units: &units,
            settings,
            stale: false,
            stale_age_seconds: None,
            degraded_cli: false,
            previous: None,
        })
    };
    let compact = frame(&settings);
    assert!(compact
        .args
        .windows(3)
        .any(|args| args == ["--set", "showy_quota.codex.bar0", "drawing=off"]));
    assert!(compact
        .args
        .windows(3)
        .any(|args| args == ["--set", "showy_quota.codex.ring", "drawing=on"]));
    assert!(compact
        .args
        .iter()
        .any(|arg| arg == "showy_quota.codex.pop_row0"));
    settings.compact_provider_count = 2;
    let normal = frame(&settings);
    assert!(!normal
        .args
        .windows(3)
        .any(|args| args == ["--set", "showy_quota.codex.bar0", "drawing=off"]));
}

#[test]
fn filtered_native_roles_omit_disabled_items_and_updates() {
    use showy_quota_zellij_core::sketchybar_frame::{
        build_frame, row_item_roles, visibility_redeclare_reason, FrameInputs, FrameSettings,
    };
    let cfg = config(&[("WINDOWS", "secondary")]);
    let rows = sketchybar_rows(THREE, &cfg, NOW, sketch_options()).unwrap();
    let settings = FrameSettings::from_getter(|_| None, &cfg);
    let roles = row_item_roles(&rows.rows[0], &settings);
    assert!(roles.contains(&"secondary"));
    assert!(roles.contains(&"secondary_marker"));
    assert!(!roles.contains(&"primary"));
    assert!(!roles.contains(&"tertiary"));
    let frame = build_frame(&FrameInputs {
        rows: &rows,
        settings: &settings,
        label_drawing: true,
        hidden: &[],
        previous: None,
        icon_ready: &|_| false,
        icon_maker: false,
    });
    assert!(!frame
        .args
        .iter()
        .any(|arg| arg == "showy_quota.claude.primary"));
    assert!(!frame
        .args
        .iter()
        .any(|arg| arg == "showy_quota.claude.tertiary_marker"));
    let declared = vec!["claude".to_owned()];
    let plans = vec![("claude".to_owned(), roles)];
    let mut live: Vec<String> = [
        "showy_quota.trigger",
        "showy_quota.stale",
        "showy_quota.degraded",
        "showy_quota.overflow",
        "showy_quota_bracket",
    ]
    .iter()
    .map(|item| item.to_string())
    .collect();
    live.extend(
        plans[0]
            .1
            .iter()
            .map(|role| format!("showy_quota.claude.{role}")),
    );
    assert_eq!(
        visibility_redeclare_reason(false, Some(&live), &declared, &plans, &settings, &[]),
        None
    );
    live.push("showy_quota.claude.primary".into());
    assert_eq!(
        visibility_redeclare_reason(false, Some(&live), &declared, &plans, &settings, &[]),
        Some("window-visibility")
    );
}
