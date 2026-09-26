use std::io::{self, Read, Write};
use std::path::Path;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use showy_quota_zellij_core::{
    codexbar::{unwrap_cache_transport, MAX_USAGE_JSON_BYTES},
    emit_formatted_prompt_segment, emit_pick, emit_prompt_segment, emit_provider_metrics,
    emit_rows, render_tmux, render_vertical, render_zellij,
    sketchybar_frame::{
        build_frame, build_ring_frame, parse_bar_items, redeclare_reason, ring_extra_items,
        ring_redeclare_reason, wire_line, Body, FrameInputs, FrameOutput, FrameSettings,
        RingFrameInputs,
    },
    sketchybar_notch::{notch_layout, parse_previous_plan, NotchSettings, PreviousPlan},
    sketchybar_ring::{apply_stale_ages, ring_units},
    sketchybar_rows, valid_provider_id, Freshness, PickOptions, PromptOptions, RenderConfig,
    RenderError, RenderOptions, SketchybarOptions, SketchybarRows, Template, TemplateScope,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Zellij,
    Tmux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emit {
    Render,
    Rows,
    Vertical,
    Metrics,
    Prompt,
    Template,
    Pick,
    SketchybarFrame,
    SketchybarQuery,
    SketchybarLayout,
}

/// Plugin state and flags for the SketchyBar emit modes.
#[derive(Debug, Default)]
struct SketchybarCli {
    /// `sketchybar --query bar` reply; `-` reads stdin.
    bar: Option<String>,
    /// Provider list the last redeclare stored (`providers.txt`).
    state: Option<String>,
    /// Frame file: what the plugin sent last.
    frame: Option<String>,
    /// Notch plan file (`notch-layout.json`).
    plan: Option<String>,
    force_redeclare: bool,
    assume_declared: bool,
    icon_maker: bool,
    or_empty: bool,
    layout_providers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Input {
    Json(String),
    Cache,
}

struct Cli {
    format: Format,
    emit: Emit,
    input: Input,
    stale: bool,
    degraded_cli: bool,
    ansi: bool,
    provider_filter: Vec<String>,
    template_format: Option<String>,
    join: String,
    pick_window: String,
    pick_min_remaining: i32,
    pick_json: bool,
    sketchybar: SketchybarCli,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--run-bounded") {
        process::exit(bounded::run(&args[1..]));
    }
    let cli = match parse_args(args.into_iter()) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("showy-quota-render: {message}");
            process::exit(2);
        }
    };
    let template = match cli
        .template_format
        .as_deref()
        .map(Template::parse)
        .transpose()
    {
        Ok(template) => template,
        Err(message) => {
            eprintln!("showy-quota-render: {message}");
            process::exit(2);
        }
    };
    if let Err(message) = run(&cli, template.as_ref()) {
        eprintln!("showy-quota-render: {message}");
        process::exit(1);
    }
}

fn run(cli: &Cli, template: Option<&Template<'_>>) -> Result<(), String> {
    let configured = RenderConfig::from_env();
    let config = if cli.emit == Emit::Pick {
        configured
    } else {
        scoped_config(configured, &cli.provider_filter)
    };
    let now_epoch = now_epoch()?;
    match cli.emit {
        Emit::SketchybarFrame => return run_sketchybar_frame(cli, &config, now_epoch),
        Emit::SketchybarQuery => return run_sketchybar_query(cli),
        Emit::SketchybarLayout => return run_sketchybar_layout(cli, &config),
        _ => {}
    }
    let input = match read_input(cli, now_epoch, &config) {
        Ok(input) => input,
        Err(_) if cli.emit == Emit::Prompt => return write_output("AI ?\n"),
        Err(err) => return Err(err),
    };
    let stale = cli.stale || input.stale;
    let degraded_cli = cli.degraded_cli || input.degraded_cli;
    let options = RenderOptions {
        color: want_zellij_color(),
        stale,
        degraded_cli,
        now_epoch,
        freshness: input.age_seconds.map(|age_seconds| Freshness {
            age_seconds,
            source: &input.source,
        }),
        stale_providers: &input.stale_providers,
    };
    if cli.emit == Emit::Template {
        let mut rendered = template
            .as_ref()
            .expect("template format is required")
            .render(
                &input.payload,
                &config,
                now_epoch,
                &cli.join,
                stale,
                TemplateScope::PerProvider,
            )
            .map_err(render_error)?;
        rendered.push('\n');
        return write_output(&rendered);
    }

    if cli.emit == Emit::Pick {
        let selected = emit_pick(
            &input.payload,
            &config,
            now_epoch,
            PickOptions {
                provider_filter: &cli.provider_filter,
                window: &cli.pick_window,
                min_remaining: cli.pick_min_remaining,
                json: cli.pick_json,
            },
        )
        .map_err(render_error)?
        .ok_or_else(|| String::from("no provider meets the requested quota floor"))?;
        return write_output(&format!("{selected}\n"));
    }

    if cli.emit == Emit::Metrics {
        let mut rendered =
            emit_provider_metrics(&input.payload, &config, now_epoch).map_err(render_error)?;
        rendered.push('\n');
        return write_output(&rendered);
    }

    if cli.emit == Emit::Prompt {
        let prompt_options = PromptOptions {
            provider_filter: &cli.provider_filter,
            ansi: cli.ansi,
            stale,
        };
        let mut rendered = match template.as_ref() {
            Some(spec) => emit_formatted_prompt_segment(
                &input.payload,
                &config,
                now_epoch,
                prompt_options,
                spec,
            ),
            None => emit_prompt_segment(&input.payload, &config, now_epoch, prompt_options),
        }
        .unwrap_or_else(|_| String::from("AI ?"));
        rendered.push('\n');
        return write_output(&rendered);
    }

    if cli.emit == Emit::Rows {
        // Rows are a structured transport: the band travels in `severity` and
        // `color`, so the text stays plain unless `--ansi` asks otherwise. A
        // consumer that strips control bytes (a Herdr sidebar token) would
        // otherwise show the escape bodies as literal text.
        let mut rendered = emit_rows(
            &input.payload,
            &config,
            RenderOptions {
                color: cli.ansi,
                ..options
            },
            match cli.format {
                Format::Zellij => showy_quota_zellij_core::OutputFormat::Zellij,
                Format::Tmux => showy_quota_zellij_core::OutputFormat::Tmux,
            },
        )
        .map_err(render_error)?;
        rendered.push('\n');
        return write_output(&rendered);
    }

    if cli.emit == Emit::Vertical {
        // The vertical view is ANSI-only: `--format tmux` markup describes one
        // status line, and this surface is a pane of its own lines.
        let rendered = render_vertical(&input.payload, &config, options).map_err(render_error)?;
        return write_output(&rendered);
    }

    let rendered = match cli.format {
        Format::Zellij => render_zellij(&input.payload, &config, options),
        Format::Tmux => render_tmux(&input.payload, &config, options),
    }
    .map_err(render_error)?;

    write_output(&rendered)
}

fn write_output(rendered: &str) -> Result<(), String> {
    // A consumer that stops reading early (e.g. `... | head`) closes the
    // pipe; that is not an error for a status renderer — exit quietly
    // instead of panicking on EPIPE like the default print! path would.
    if let Err(err) = io::stdout().write_all(rendered.as_bytes()) {
        if err.kind() == io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(format!("failed writing output: {err}"));
    }
    Ok(())
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut format = Format::Zellij;
    let mut format_option = None;
    let mut json_path = Some(String::from("-"));
    let mut json_seen = false;
    let mut from_cache = false;
    let mut stale = false;
    let mut degraded_cli = false;
    let mut ansi = false;
    let mut provider_filter = Vec::new();
    let mut emit = Emit::Render;
    let mut join = String::from(" ");
    let mut pick_window = String::from("worst");
    let mut pick_min_remaining = 0;
    let mut pick_json = false;
    let mut sketchybar = SketchybarCli::default();
    let mut sketchybar_flag_seen = false;
    let mut args = args;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--format" => {
                format_option = Some(args.next().ok_or("--format requires a value")?);
            }
            "--join" => join = args.next().ok_or("--join requires a separator")?,
            "--json" => {
                if from_cache {
                    return Err(String::from(
                        "--json and --from-cache are mutually exclusive",
                    ));
                }
                json_seen = true;
                json_path = Some(
                    args.next()
                        .ok_or_else(|| String::from("--json requires path or -"))?,
                );
            }
            "--from-cache" => {
                if json_seen {
                    return Err(String::from(
                        "--json and --from-cache are mutually exclusive",
                    ));
                }
                from_cache = true;
            }
            "--emit" => {
                let value = args.next().ok_or_else(|| {
                    String::from(
                        "--emit requires render, rows, vertical, metrics, prompt, template, pick, sketchybar-frame, sketchybar-query, or sketchybar-layout",
                    )
                })?;
                emit = match value.as_str() {
                    "render" => Emit::Render,
                    "rows" => Emit::Rows,
                    "vertical" => Emit::Vertical,
                    "metrics" => Emit::Metrics,
                    "prompt" => Emit::Prompt,
                    "template" => Emit::Template,
                    "pick" => Emit::Pick,
                    "sketchybar-frame" => Emit::SketchybarFrame,
                    "sketchybar-query" => Emit::SketchybarQuery,
                    "sketchybar-layout" => Emit::SketchybarLayout,
                    _ => return Err(format!("unknown emit mode: {value}")),
                };
            }
            "--bar" | "--state" | "--frame" | "--plan" | "--layout-providers" => {
                sketchybar_flag_seen = true;
                let value = args
                    .next()
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                match arg.as_str() {
                    "--bar" => sketchybar.bar = Some(value),
                    "--state" => sketchybar.state = Some(value),
                    "--frame" => sketchybar.frame = Some(value),
                    "--plan" => sketchybar.plan = Some(value),
                    _ => sketchybar.layout_providers = parse_provider_filter(&value)?,
                }
            }
            "--force-redeclare" | "--assume-declared" | "--icon-maker" | "--or-empty" => {
                sketchybar_flag_seen = true;
                match arg.as_str() {
                    "--force-redeclare" => sketchybar.force_redeclare = true,
                    "--assume-declared" => sketchybar.assume_declared = true,
                    "--icon-maker" => sketchybar.icon_maker = true,
                    _ => sketchybar.or_empty = true,
                }
            }
            "--provider" => {
                let value = args
                    .next()
                    .ok_or_else(|| String::from("--provider requires ID[,ID...]"))?;
                provider_filter = parse_provider_filter(&value)?;
            }
            "--window" => {
                let value = args.next().ok_or("--window requires a value")?;
                if !matches!(
                    value.as_str(),
                    "primary" | "secondary" | "tertiary" | "worst"
                ) {
                    return Err(format!("invalid window: {value}"));
                }
                pick_window = value;
            }
            "--min-remaining" => {
                let value = args.next().ok_or("--min-remaining requires 0-100")?;
                pick_min_remaining = value
                    .parse::<i32>()
                    .map_err(|_| "invalid --min-remaining")?;
                if !(0..=100).contains(&pick_min_remaining) {
                    return Err(String::from("invalid --min-remaining"));
                }
            }
            "--pick-format" => {
                pick_json = match args.next().as_deref() {
                    Some("id") => false,
                    Some("json") => true,
                    _ => return Err(String::from("--pick-format requires id or json")),
                };
            }
            "--ansi" => ansi = true,
            "--stale" => stale = true,
            "--degraded-cli" => degraded_cli = true,
            "-h" | "--help" => {
                print_help();
                process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }

    let template_format = match emit {
        Emit::Template | Emit::Prompt => {
            if emit == Emit::Template && format_option.is_none() {
                return Err(String::from("--emit template requires --format"));
            }
            if emit == Emit::Template && ansi {
                return Err(String::from("--emit template does not support --ansi"));
            }
            format_option
        }
        _ => {
            if let Some(value) = format_option {
                format = match value.as_str() {
                    "zellij" => Format::Zellij,
                    "tmux" => Format::Tmux,
                    _ => return Err(format!("unknown format: {value}")),
                };
            }
            None
        }
    };
    if emit != Emit::Template && join != " " {
        return Err(String::from("--join requires --emit template"));
    }
    if sketchybar_flag_seen
        && !matches!(
            emit,
            Emit::SketchybarFrame | Emit::SketchybarQuery | Emit::SketchybarLayout
        )
    {
        return Err(String::from(
            "SketchyBar flags require --emit sketchybar-frame, sketchybar-query, or sketchybar-layout",
        ));
    }

    Ok(Cli {
        emit,
        format,
        input: if from_cache {
            Input::Cache
        } else {
            Input::Json(json_path.expect("json path is present without --from-cache"))
        },
        stale,
        degraded_cli,
        ansi,
        provider_filter,
        template_format,
        join,
        pick_window,
        pick_min_remaining,
        pick_json,
        sketchybar,
    })
}

fn parse_provider_filter(raw: &str) -> Result<Vec<String>, String> {
    let mut providers = Vec::new();
    for part in raw.split(',') {
        let provider = part.trim();
        if provider.is_empty() {
            continue;
        }
        if !valid_provider_id(provider) {
            return Err(format!("invalid provider id: {provider}"));
        }
        providers.push(provider.to_owned());
    }
    Ok(providers)
}

fn scoped_config(mut config: RenderConfig, provider_filter: &[String]) -> RenderConfig {
    if !provider_filter.is_empty() {
        config.providers = provider_filter.to_vec();
    }
    config
}

struct InputPayload {
    payload: Vec<u8>,
    stale: bool,
    degraded_cli: bool,
    age_seconds: Option<i64>,
    source: String,
    /// Providers whose cached slice is older than the stale horizon even
    /// though the cache file itself is current. Only the cache path can know
    /// this: `--json` input carries no publish metadata.
    stale_providers: Vec<String>,
    /// Each provider's own slice age in seconds, for the ring popup's stale
    /// row. Empty for `--json` input.
    provider_ages: Vec<(String, i64)>,
}

fn read_input(cli: &Cli, now_epoch: i64, config: &RenderConfig) -> Result<InputPayload, String> {
    match &cli.input {
        Input::Json(path) => Ok(InputPayload {
            payload: unwrap_cache_transport(read_payload(path)?).payload,
            stale: false,
            degraded_cli: false,
            age_seconds: None,
            source: String::new(),
            stale_providers: Vec::new(),
            provider_ages: Vec::new(),
        }),
        Input::Cache => {
            let snapshot = read_cache_snapshot(now_epoch, config).map_err(|err| err.to_string())?;
            let stale_providers = snapshot.freshness.stale_providers();
            let provider_ages = snapshot
                .freshness
                .providers
                .iter()
                .map(|provider| (provider.provider.clone(), provider.age_seconds))
                .collect();
            Ok(InputPayload {
                payload: snapshot.payload,
                stale: snapshot.freshness.stale,
                degraded_cli: snapshot.freshness.degraded_cli,
                age_seconds: Some(snapshot.freshness.age_seconds),
                source: snapshot.freshness.source,
                stale_providers,
                provider_ages,
            })
        }
    }
}

/// Read the cache envelope, then restrict the freshness decision to the
/// providers this invocation renders: the degraded marker must reflect the
/// visible slices, not metadata for filtered-out or absent providers.
fn read_cache_snapshot(
    now_epoch: i64,
    config: &RenderConfig,
) -> Result<
    showy_quota_zellij_core::cache::CacheSnapshot,
    showy_quota_zellij_core::cache::CacheReadError,
> {
    use showy_quota_zellij_core::cache::{
        cache_paths_from_env, freshness_from_parts_filtered, refresh_seconds_from_env,
    };
    use showy_quota_zellij_core::codexbar::{parse_usage_payload, valid_provider_id};
    let paths = cache_paths_from_env();
    let raw = std::fs::read(&paths.usage_file).map_err(|source| {
        showy_quota_zellij_core::cache::CacheReadError::for_path(paths.usage_file.clone(), source)
    })?;
    let mtime = std::fs::metadata(&paths.usage_file)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(showy_quota_zellij_core::cache::system_time_epoch);
    let transport = showy_quota_zellij_core::codexbar::unwrap_cache_transport(raw);
    let visible: Vec<String> = parse_usage_payload(&transport.payload)
        .unwrap_or_default()
        .iter()
        .filter(|record| {
            valid_provider_id(&record.provider)
                && (config.providers.is_empty() || config.providers.contains(&record.provider))
                && !config.providers_exclude.contains(&record.provider)
        })
        .map(|record| record.provider.clone())
        .collect();
    let freshness = freshness_from_parts_filtered(
        mtime,
        now_epoch,
        refresh_seconds_from_env(),
        transport.source,
        std::env::var("SHOWY_QUOTA_DEGRADED_CLI").ok(),
        &transport.provider_meta,
        Some(&visible),
    );
    Ok(showy_quota_zellij_core::cache::CacheSnapshot {
        payload: transport.payload,
        freshness,
    })
}

fn read_payload(path: &str) -> Result<Vec<u8>, String> {
    use std::fs::File;
    if path == "-" {
        return read_bounded_payload(io::stdin())
            .map_err(|err| format!("failed to read JSON from stdin: {err}"));
    }
    let file = File::open(path).map_err(|err| format!("failed to read JSON {path}: {err}"))?;
    read_bounded_payload(file).map_err(|err| format!("failed to read JSON {path}: {err}"))
}

fn read_bounded_payload(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    reader
        .take((MAX_USAGE_JSON_BYTES + 1) as u64)
        .read_to_end(&mut payload)?;
    if payload.len() > MAX_USAGE_JSON_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "CodexBar usage payload exceeds size cap",
        ));
    }
    Ok(payload)
}

// ── SketchyBar ─────────────────────────────────────────────────────────

/// `--emit sketchybar-frame`: the plugin's whole per-tick compute. Reads the
/// cache, the live item list (`--bar`), the stored provider list (`--state`),
/// the notch plan (`--plan`), and the last frame (`--frame`); writes the new
/// frame; prints the wire records described in `sketchybar_frame`.
fn run_sketchybar_frame(cli: &Cli, config: &RenderConfig, now_epoch: i64) -> Result<(), String> {
    let sb = &cli.sketchybar;
    let settings = FrameSettings::from_getter(|name| std::env::var(name).ok(), config);

    // Ring mode owns its own frame path; the rows path below is unchanged.
    if settings.body == Body::Ring {
        return run_sketchybar_ring_frame(cli, config, now_epoch, &settings);
    }

    // An unusable cache is an error, so the plugin can fetch and retry;
    // `--or-empty` (the retry) renders an empty frame instead, which tears
    // the providers down while stale/degraded still reflect the file.
    let (rows, age_seconds) = match read_input(cli, now_epoch, config) {
        Ok(input) => {
            let stale = cli.stale || input.stale;
            let degraded_cli = cli.degraded_cli || input.degraded_cli;
            let options = SketchybarOptions {
                stale,
                degraded_cli,
                bar_width: png_bar_width_from_env(),
                stale_providers: &input.stale_providers,
            };
            match sketchybar_rows(&input.payload, config, now_epoch, options) {
                Ok(rows) => (rows, input.age_seconds),
                Err(_) if sb.or_empty => (empty_rows(stale, degraded_cli), None),
                Err(err) => return Err(render_error(err)),
            }
        }
        Err(_) if sb.or_empty && cli.input == Input::Cache => {
            use showy_quota_zellij_core::cache::{cache_paths_from_env, freshness_for_paths};
            let freshness =
                freshness_for_paths(&cache_paths_from_env(), now_epoch, String::from("unknown"));
            (
                empty_rows(
                    cli.stale || freshness.stale,
                    cli.degraded_cli || freshness.degraded_cli,
                ),
                None,
            )
        }
        Err(err) => return Err(err),
    };

    let items = match &sb.bar {
        Some(path) => read_payload(path)
            .ok()
            .and_then(|raw| parse_bar_items(&raw)),
        None => None,
    };
    let declared = sb
        .state
        .as_deref()
        .map(read_provider_list)
        .unwrap_or_default();
    let desired: Vec<String> = rows.rows.iter().map(|row| row.provider.clone()).collect();
    let redeclare = if sb.assume_declared {
        None
    } else {
        redeclare_reason(
            sb.force_redeclare,
            items.as_deref(),
            &declared,
            &desired,
            settings.notch,
        )
    };

    // The last notch plan decides what this frame draws: compact hides the
    // countdown labels, hidden providers stay off behind the `+N` item.
    let plan = read_plan(sb, &settings);
    let previous_frame = match (&sb.frame, redeclare) {
        (Some(path), None) => std::fs::read_to_string(path).ok(),
        _ => None,
    };
    let icon_ready = |path: &str| {
        std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
    };
    let frame = build_frame(&FrameInputs {
        rows: &rows,
        settings: &settings,
        label_drawing: !plan.compact,
        hidden: &plan.hidden,
        previous: previous_frame.as_deref(),
        icon_ready: &icon_ready,
        icon_maker: sb.icon_maker,
    });
    if let Some(path) = &sb.frame {
        write_atomic(path, &frame.frame_text)?;
    }

    // Items a redeclare replaces are gone from `items`; the plugin re-queries.
    let query = (settings.notch && redeclare.is_none())
        .then_some(items.as_deref())
        .flatten();
    let output = FrameOutput {
        redeclare,
        refresh: background_refresh_due(age_seconds),
        providers: desired.iter().map(String::as_str).collect(),
        icon_requests: &frame.icon_requests,
        query,
        args: &frame.args,
        units: None,
    };
    write_output(&output.to_wire())
}

/// `--emit sketchybar-frame` with `SHOWY_QUOTA_SKETCHYBAR_BODY=ring`: the same
/// tick contract as the rows path, but one ring per model family. Ring mode
/// draws font logos instead of rasterized icons, so it never requests any,
/// and it stays out of the notch planner (no `query` record).
fn run_sketchybar_ring_frame(
    cli: &Cli,
    config: &RenderConfig,
    now_epoch: i64,
    settings: &FrameSettings,
) -> Result<(), String> {
    let sb = &cli.sketchybar;
    let (units, age_seconds, stale, degraded_cli) = match read_input(cli, now_epoch, config) {
        Ok(input) => {
            let stale = cli.stale || input.stale;
            let degraded_cli = cli.degraded_cli || input.degraded_cli;
            let options = SketchybarOptions {
                stale,
                degraded_cli,
                bar_width: png_bar_width_from_env(),
                stale_providers: &input.stale_providers,
            };
            match ring_units(&input.payload, config, now_epoch, options) {
                Ok(mut units) => {
                    apply_stale_ages(
                        &mut units,
                        input.age_seconds,
                        &input.provider_ages,
                        now_epoch,
                        config.reset_description_timezone_offset_minutes,
                    );
                    (units, input.age_seconds, stale, degraded_cli)
                }
                Err(_) if sb.or_empty => (Vec::new(), None, stale, degraded_cli),
                Err(err) => return Err(render_error(err)),
            }
        }
        Err(_) if sb.or_empty && cli.input == Input::Cache => {
            use showy_quota_zellij_core::cache::{cache_paths_from_env, freshness_for_paths};
            let freshness =
                freshness_for_paths(&cache_paths_from_env(), now_epoch, String::from("unknown"));
            (
                Vec::new(),
                None,
                cli.stale || freshness.stale,
                cli.degraded_cli || freshness.degraded_cli,
            )
        }
        Err(err) => return Err(err),
    };

    let items = match &sb.bar {
        Some(path) => read_payload(path)
            .ok()
            .and_then(|raw| parse_bar_items(&raw)),
        None => None,
    };
    let declared = sb
        .state
        .as_deref()
        .map(read_provider_list)
        .unwrap_or_default();
    let desired_units: Vec<String> = units.iter().map(|unit| unit.unit.clone()).collect();
    let mut desired_providers: Vec<String> = Vec::new();
    for unit in &units {
        if !desired_providers
            .iter()
            .any(|provider| provider == &unit.provider)
        {
            desired_providers.push(unit.provider.clone());
        }
    }
    let extra = ring_extra_items(&units);
    let redeclare = if sb.assume_declared {
        None
    } else {
        ring_redeclare_reason(
            sb.force_redeclare,
            items.as_deref(),
            &declared,
            &desired_units,
            &extra,
        )
    };
    let previous_frame = match (&sb.frame, redeclare) {
        (Some(path), None) => std::fs::read_to_string(path).ok(),
        _ => None,
    };
    let frame = build_ring_frame(&RingFrameInputs {
        units: &units,
        settings,
        stale,
        stale_age_seconds: age_seconds,
        degraded_cli,
        previous: previous_frame.as_deref(),
    });
    if let Some(path) = &sb.frame {
        write_atomic(path, &frame.frame_text)?;
    }
    let pairs: Vec<String> = units
        .iter()
        .map(|unit| format!("{}={}", unit.unit, unit.provider))
        .collect();
    let output = FrameOutput {
        redeclare,
        refresh: background_refresh_due(age_seconds),
        providers: desired_providers.iter().map(String::as_str).collect(),
        icon_requests: &frame.icon_requests,
        query: None,
        args: &frame.args,
        units: Some(&pairs),
    };
    write_output(&output.to_wire())
}

/// `--emit sketchybar-query`: the live item names from a `--query bar`
/// reply on stdin, for the notch planner's batched geometry query.
fn run_sketchybar_query(cli: &Cli) -> Result<(), String> {
    let raw = read_payload(cli.sketchybar.bar.as_deref().unwrap_or("-"))?;
    let items = parse_bar_items(&raw).ok_or("no readable SketchyBar item list")?;
    let mut out = String::new();
    wire_line(&mut out, "query", items.iter().map(String::as_str));
    write_output(&out)
}

/// `--emit sketchybar-layout`: plan the notch split from one batched
/// geometry query on stdin, store the plan (`--plan`), and print the
/// arguments that apply it. `layout noreply` means SketchyBar dropped the
/// reply and the placement stays as it is.
fn run_sketchybar_layout(cli: &Cli, config: &RenderConfig) -> Result<(), String> {
    let sb = &cli.sketchybar;
    let settings = FrameSettings::from_getter(|name| std::env::var(name).ok(), config);
    let measured = read_payload("-")?;
    let previous = sb
        .plan
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|raw| parse_previous_plan(&raw))
        .unwrap_or_default();
    let notch = NotchSettings {
        margin: settings.notch_margin as f64,
        icon_padding_left: settings.icon_padding_left as f64,
        label_width: settings.label_width as f64,
        icon_width: settings.icon_width as f64,
        bar_width: settings.slot_width as f64,
    };
    let mut out = String::new();
    let Some(layout) = notch_layout(&measured, &sb.layout_providers, &notch, &previous) else {
        wire_line(&mut out, "layout", ["noreply"]);
        return write_output(&out);
    };
    if let Some(path) = &sb.plan {
        write_atomic(path, &format!("{}\n", layout.plan_json))?;
    }
    wire_line(
        &mut out,
        "layout",
        ["ok", if layout.reveal { "1" } else { "0" }],
    );
    wire_line(&mut out, "set", layout.args.iter().map(String::as_str));
    write_output(&out)
}

fn empty_rows(stale: bool, degraded_cli: bool) -> SketchybarRows {
    SketchybarRows {
        stale,
        degraded_cli,
        rows: Vec::new(),
    }
}

/// The notch plan that shapes this frame. Left placement ignores the file.
fn read_plan(sb: &SketchybarCli, settings: &FrameSettings) -> PreviousPlan {
    if !settings.notch {
        return PreviousPlan::default();
    }
    sb.plan
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|raw| parse_previous_plan(&raw))
        .unwrap_or_default()
}

/// One provider id per line; invalid ids are dropped.
fn read_provider_list(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| valid_provider_id(line))
        .map(str::to_owned)
        .collect()
}

/// Whether the cache is old enough for the plugin to start a background
/// refresh: `SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS` (60) with a serve
/// URL, else `SHOWY_QUOTA_REFRESH_SECONDS` (120).
fn background_refresh_due(age_seconds: Option<i64>) -> bool {
    let Some(age) = age_seconds else {
        return false;
    };
    let seconds = |name: &str, fallback: i64| {
        std::env::var(name)
            .ok()
            .filter(|value| {
                !value.is_empty() && value.len() <= 18 && value.bytes().all(|b| b.is_ascii_digit())
            })
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(fallback)
    };
    let serve = std::env::var("SHOWY_QUOTA_CODEXBAR_SERVE_URL").is_ok_and(|url| !url.is_empty());
    let threshold = if serve {
        seconds("SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS", 60)
    } else {
        seconds("SHOWY_QUOTA_REFRESH_SECONDS", 120)
    };
    age >= threshold
}

/// Replace `path` through a sibling temp file, so a reader never sees a
/// half-written file.
fn write_atomic(path: &str, content: &str) -> Result<(), String> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    // `create_new` refuses an existing file or symlink at the temp name, so a
    // planted link cannot redirect the write; a fresh suffix retries.
    let target = Path::new(path);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_nanos());
    for attempt in 0..8u32 {
        let tmp = target.with_extension(format!("tmp.{}.{stamp}.{attempt}", process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp);
        let mut file = match file {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(format!("failed to write {path}: {err}")),
        };
        return file
            .write_all(content.as_bytes())
            .and_then(|()| std::fs::rename(&tmp, target))
            .map_err(|err| {
                let _ = std::fs::remove_file(&tmp);
                format!("failed to write {path}: {err}")
            });
    }
    Err(format!("failed to write {path}: no free temp name"))
}

fn render_error(error: RenderError) -> String {
    match error {
        RenderError::InvalidPayload => String::from("invalid JSON quota payload"),
    }
}

fn want_zellij_color() -> bool {
    let mut color = true;
    if std::env::var_os("NO_COLOR").is_some()
        || std::env::var("TERM").is_ok_and(|term| term == "dumb")
    {
        color = false;
    }
    if std::env::var("SHOWY_QUOTA_FORCE_COLOR").is_ok_and(|value| value == "1") {
        color = true;
    }
    color
}

const MAX_NOW_EPOCH: i64 = 4_102_444_800;

fn now_epoch() -> Result<i64, String> {
    match std::env::var("SHOWY_QUOTA_NOW_EPOCH") {
        Ok(value) => parse_now_epoch(&value),
        Err(std::env::VarError::NotPresent) => Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
            .unwrap_or(0)),
        Err(std::env::VarError::NotUnicode(_)) => Err(String::from(
            "SHOWY_QUOTA_NOW_EPOCH must be an ASCII Unix epoch",
        )),
    }
}

fn parse_now_epoch(value: &str) -> Result<i64, String> {
    if value.is_empty() || value.len() > 18 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(String::from(
            "SHOWY_QUOTA_NOW_EPOCH must be a non-negative Unix epoch no later than 2100-01-01",
        ));
    }
    let epoch = value.parse::<i64>().map_err(|_| {
        String::from(
            "SHOWY_QUOTA_NOW_EPOCH must be a non-negative Unix epoch no later than 2100-01-01",
        )
    })?;
    if epoch > MAX_NOW_EPOCH {
        return Err(String::from(
            "SHOWY_QUOTA_NOW_EPOCH must be a non-negative Unix epoch no later than 2100-01-01",
        ));
    }
    Ok(epoch)
}

/// `SHOWY_QUOTA_PNG_BAR_W`: SketchyBar slider width in pixels. The shell
/// data plane validates and exports it (default 80, capped 4096); fall back
/// to the stock width on garbage so marker math never divides by zero.
fn png_bar_width_from_env() -> i64 {
    std::env::var("SHOWY_QUOTA_PNG_BAR_W")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (2..=4096).contains(value))
        .unwrap_or(80)
}

fn print_help() {
    println!(
        "Usage: showy-quota-render [--emit render|rows|vertical|metrics|prompt|template|pick|sketchybar-frame|sketchybar-query|sketchybar-layout] [--format zellij|tmux|SPEC] [--join SEP] [--json <path|-> | --from-cache] [--provider ID[,ID...]] [--ansi] [--stale] [--degraded-cli]\n\nTemplate mode requires --format SPEC and expands once per provider. Prompt accepts --format SPEC for the worst window overall. Fields: {{provider}}, {{sigil}}, {{used}}, {{remaining}}, {{countdown}}, {{class}}, {{window}}, {{stale}}. Escape braces with {{{{ and }}}}.\n\nPick mode accepts --window primary|secondary|tertiary|worst, --min-remaining 0-100, and --pick-format id|json.\n\n--run-bounded SECONDS MAX_BYTES CMD [ARG...] runs CMD in its own session with a hard timeout (exit 124) and an output cap (MAX_BYTES + 1 bytes pass, exit 125), for showy-quota-fetch.\n\nThe sketchybar-* modes serve the SketchyBar plugin: frame accepts --bar PATH|-, --state PATH, --frame PATH, --plan PATH, --force-redeclare, --assume-declared, --icon-maker, and --or-empty; query reads a `--query bar` reply; layout accepts --plan PATH and --layout-providers ID[,ID...] and reads the batched geometry query on stdin."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_extreme_now_epoch_values() {
        for value in ["-1", "4102444801", "9223372036854775807"] {
            let error = parse_now_epoch(value).expect_err("extreme epoch must fail");
            assert!(error.contains("SHOWY_QUOTA_NOW_EPOCH"));
        }
        assert_eq!(parse_now_epoch("4102444800"), Ok(MAX_NOW_EPOCH));
    }

    #[test]
    fn bounded_payload_reader_rejects_oversize_input() {
        let payload = vec![b' '; MAX_USAGE_JSON_BYTES + 1];
        let error =
            read_bounded_payload(io::Cursor::new(payload)).expect_err("oversize payload must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn metrics_emit_scopes_to_requested_provider() {
        let config = scoped_config(RenderConfig::default(), &[String::from("codex")]);
        let output = emit_provider_metrics(
            br#"[
                {"provider":"codex","usage":{"primary":{"usedPercent":10}}},
                {"provider":"claude","usage":{"primary":{"usedPercent":90}}}
            ]"#,
            &config,
            0,
        )
        .expect("metrics");
        assert!(output.contains(r#""provider":"codex""#));
        assert!(!output.contains(r#""provider":"claude""#));
    }
}

/// `--run-bounded`: the fetcher's hard timeout and output cap around one
/// CodexBar command, so a hung or runaway call cannot stall the refresh or
/// grow a command substitution without bound.
///
/// The command runs in its own session with stdin and stderr closed. Stdout
/// streams through until it closes; at most `MAX_BYTES + 1` bytes pass, and
/// reaching that many ends the run with 125 so the caller sees the cap.
/// Timeout exits 124. Either way, and on SIGINT/SIGTERM (128 + signal), the
/// command's process group gets SIGTERM, then SIGKILL after one second. A
/// command that finishes is never signalled, so helpers it left running
/// survive. Otherwise the exit code is the command's (128 + signal when a
/// signal killed it); 127 when it cannot start, 2 for bad arguments.
mod bounded {
    use std::io::{self, Read, Write};
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::{Duration, Instant};

    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
        fn setsid() -> i32;
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    }

    const SIGINT: i32 = 2;
    const SIGKILL: i32 = 9;
    const SIGTERM: i32 = 15;
    const POLL: Duration = Duration::from_millis(100);

    static CAUGHT: AtomicI32 = AtomicI32::new(0);

    extern "C" fn on_signal(sig: i32) {
        CAUGHT.store(sig, Ordering::SeqCst);
    }

    fn caught() -> Option<i32> {
        match CAUGHT.load(Ordering::SeqCst) {
            0 => None,
            sig => Some(sig),
        }
    }

    pub fn run(args: &[String]) -> i32 {
        let timeout = args
            .first()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0);
        let max_bytes = args.get(1).and_then(|value| value.parse::<u64>().ok());
        let (Some(timeout), Some(max_bytes), Some(program)) = (timeout, max_bytes, args.get(2))
        else {
            eprintln!("showy-quota-render: --run-bounded SECONDS MAX_BYTES CMD [ARG...]");
            return 2;
        };
        let capture = max_bytes.saturating_add(1);
        let deadline = Instant::now() + Duration::from_secs_f64(timeout.min(1e9));

        let mut command = Command::new(program);
        command
            .args(&args[3..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // SAFETY: setsid is async-signal-safe and touches no parent state.
        unsafe {
            command.pre_exec(|| {
                if setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let Ok(mut child) = command.spawn() else {
            return 127;
        };
        // SAFETY: the handler only stores into an atomic.
        unsafe {
            signal(SIGINT, on_signal);
            signal(SIGTERM, on_signal);
        }

        let Some(mut stdout) = child.stdout.take() else {
            terminate(&mut child);
            return 1;
        };
        // Bounded: a slow stdout consumer stalls the reader, then the child's
        // pipe, instead of queueing unbounded output in memory.
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(4);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 65_536];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });

        let out = io::stdout();
        let mut out = out.lock();
        let mut written = 0u64;
        loop {
            if let Some(sig) = caught() {
                terminate(&mut child);
                return 128 + sig;
            }
            let now = Instant::now();
            if now >= deadline {
                terminate(&mut child);
                return 124;
            }
            match rx.recv_timeout((deadline - now).min(POLL)) {
                Ok(chunk) => {
                    let room = usize::try_from(capture - written).unwrap_or(usize::MAX);
                    let take = chunk.len().min(room);
                    if out.write_all(&chunk[..take]).is_err() {
                        terminate(&mut child);
                        return 1;
                    }
                    written += take as u64;
                    if written == capture {
                        let _ = out.flush();
                        terminate(&mut child);
                        return 125;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = out.flush();

        loop {
            if let Some(sig) = caught() {
                terminate(&mut child);
                return 128 + sig;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    return status
                        .code()
                        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
                }
                Ok(None) => {}
                Err(_) => return 1,
            }
            let now = Instant::now();
            if now >= deadline {
                terminate(&mut child);
                return 124;
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(20)));
        }
    }

    /// SIGTERM the command's group, then SIGKILL after a second, then reap.
    /// Callers never reaped the child, so a leader that already exited stays a
    /// zombie and still holds the pgid: signalling the group cannot hit a
    /// reused pid, and helpers the leader left in the group are killed too.
    fn terminate(child: &mut Child) {
        let Ok(pgid) = i32::try_from(child.id()) else {
            return;
        };
        // SAFETY: plain syscalls on a group we created and have not reaped.
        unsafe {
            kill(-pgid, SIGTERM);
        }
        std::thread::sleep(Duration::from_secs(1));
        unsafe {
            kill(-pgid, SIGKILL);
        }
        let _ = child.wait();
    }
}
