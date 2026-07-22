use std::fs;
use std::io::{self, Read, Write};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use showy_quota_zellij_core::{
    cache::read_cache_from_env, codexbar::MAX_USAGE_JSON_BYTES, emit_prompt_segment,
    emit_provider_metrics, emit_sketchybar, render_tmux, render_zellij, valid_provider_id,
    PromptOptions, RenderConfig, RenderError, RenderOptions, SketchybarOptions,
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
    Sketchybar,
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
    let config = scoped_config(RenderConfig::from_env(), &cli.provider_filter);
    let now_epoch = now_epoch()?;
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

    if cli.emit == Emit::Sketchybar {
        let mut rendered = emit_sketchybar(
            &input.payload,
            &config,
            now_epoch,
            SketchybarOptions {
                stale,
                degraded_cli,
                bar_width: png_bar_width_from_env(),
            },
        )
        .map_err(render_error)?;
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
                let value = args.next().ok_or_else(|| {
                    String::from("--emit requires render, metrics, prompt, or sketchybar")
                })?;
                emit = match value.as_str() {
                    "render" => Emit::Render,
                    "metrics" => Emit::Metrics,
                    "prompt" => Emit::Prompt,
                    "sketchybar" => Emit::Sketchybar,
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
        return read_bounded_payload(io::stdin())
            .map_err(|err| format!("failed to read JSON from stdin: {err}"));
    }

    let file = fs::File::open(path).map_err(|err| format!("failed to read JSON {path}: {err}"))?;
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
        "Usage: showy-quota-render [--emit render|metrics|prompt|sketchybar] [--format zellij|tmux] [--json <path|-> | --from-cache] [--provider ID[,ID...]] [--ansi] [--stale] [--degraded-cli]\n\nPrints a rendered quota strip, providerMetrics JSON, SketchyBar row data, or shell prompt segment from CodexBar JSON."
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
