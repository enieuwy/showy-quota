use std::fs;
use std::io::{self, Read, Write};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use showy_quota_zellij_core::{
    cache::read_cache_from_env, emit_prompt_segment, emit_provider_metrics, render_tmux,
    render_zellij, valid_provider_id, PromptOptions, RenderConfig, RenderError, RenderOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Zellij,
    Tmux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emit {
    Render,
    Metrics,
    Prompt,
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
}

fn main() {
    if let Err(message) = run() {
        eprintln!("showy-quota-render: {message}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = parse_args(std::env::args().skip(1))?;
    let config = RenderConfig::from_env();
    let now_epoch = now_epoch();
    let input = match read_input(&cli, now_epoch) {
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
    };

    if cli.emit == Emit::Metrics {
        let mut rendered =
            emit_provider_metrics(&input.payload, &config, now_epoch).map_err(render_error)?;
        rendered.push('\n');
        return write_output(&rendered);
    }

    if cli.emit == Emit::Prompt {
        let mut rendered = emit_prompt_segment(
            &input.payload,
            &config,
            now_epoch,
            PromptOptions {
                provider_filter: &cli.provider_filter,
                ansi: cli.ansi,
                stale,
            },
        )
        .unwrap_or_else(|_| String::from("AI ?"));
        rendered.push('\n');
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
    let mut json_path = Some(String::from("-"));
    let mut json_seen = false;
    let mut from_cache = false;
    let mut stale = false;
    let mut degraded_cli = false;
    let mut ansi = false;
    let mut provider_filter = Vec::new();
    let mut emit = Emit::Render;
    let mut args = args;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--format" => {
                let value = args
                    .next()
                    .ok_or_else(|| String::from("--format requires zellij or tmux"))?;
                format = match value.as_str() {
                    "zellij" => Format::Zellij,
                    "tmux" => Format::Tmux,
                    _ => return Err(format!("unknown format: {value}")),
                };
            }
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
                let value = args
                    .next()
                    .ok_or_else(|| String::from("--emit requires render, metrics, or prompt"))?;
                emit = match value.as_str() {
                    "render" => Emit::Render,
                    "metrics" => Emit::Metrics,
                    "prompt" => Emit::Prompt,
                    _ => return Err(format!("unknown emit mode: {value}")),
                };
            }
            "--provider" => {
                let value = args
                    .next()
                    .ok_or_else(|| String::from("--provider requires ID[,ID...]"))?;
                provider_filter = parse_provider_filter(&value)?;
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

struct InputPayload {
    payload: Vec<u8>,
    stale: bool,
    degraded_cli: bool,
}

fn read_input(cli: &Cli, now_epoch: i64) -> Result<InputPayload, String> {
    match &cli.input {
        Input::Json(path) => Ok(InputPayload {
            payload: read_payload(path)?,
            stale: false,
            degraded_cli: false,
        }),
        Input::Cache => {
            let snapshot = read_cache_from_env(now_epoch).map_err(|err| err.to_string())?;
            Ok(InputPayload {
                payload: snapshot.payload,
                stale: snapshot.freshness.stale,
                degraded_cli: snapshot.freshness.degraded_cli,
            })
        }
    }
}

fn read_payload(path: &str) -> Result<Vec<u8>, String> {
    if path == "-" {
        let mut payload = Vec::new();
        io::stdin()
            .read_to_end(&mut payload)
            .map_err(|err| format!("failed to read JSON from stdin: {err}"))?;
        return Ok(payload);
    }

    fs::read(path).map_err(|err| format!("failed to read JSON {path}: {err}"))
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

fn now_epoch() -> i64 {
    if let Ok(value) = std::env::var("SHOWY_QUOTA_NOW_EPOCH") {
        if let Ok(epoch) = value.parse() {
            return epoch;
        }
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn print_help() {
    println!(
        "Usage: showy-quota-render [--emit render|metrics|prompt] [--format zellij|tmux] [--json <path|-> | --from-cache] [--provider ID[,ID...]] [--ansi] [--stale] [--degraded-cli]\n\nPrints a rendered quota strip, providerMetrics JSON, or shell prompt segment from CodexBar JSON."
    );
}
