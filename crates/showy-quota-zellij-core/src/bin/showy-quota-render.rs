use std::fs;
use std::io::{self, Read};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use showy_quota_zellij_core::{
    render_tmux, render_zellij, RenderConfig, RenderError, RenderOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Zellij,
    Tmux,
}

struct Cli {
    format: Format,
    json_path: String,
    stale: bool,
    degraded_cli: bool,
}

fn main() {
    if let Err(message) = run() {
        eprintln!("showy-quota-render: {message}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = parse_args(std::env::args().skip(1))?;
    let payload = read_payload(&cli.json_path)?;
    let config = RenderConfig::from_env();
    let options = RenderOptions {
        color: want_zellij_color(),
        stale: cli.stale,
        degraded_cli: cli.degraded_cli,
        now_epoch: now_epoch(),
    };

    let rendered = match cli.format {
        Format::Zellij => render_zellij(&payload, &config, options),
        Format::Tmux => render_tmux(&payload, &config, options),
    }
    .map_err(render_error)?;

    print!("{rendered}");
    Ok(())
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut format = Format::Zellij;
    let mut json_path = String::from("-");
    let mut stale = false;
    let mut degraded_cli = false;
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
                json_path = args
                    .next()
                    .ok_or_else(|| String::from("--json requires path or -"))?;
            }
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
        format,
        json_path,
        stale,
        degraded_cli,
    })
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
        "Usage: showy-quota-render [--format zellij|tmux] [--json <path|->] [--stale] [--degraded-cli]\n\nPrints a rendered quota strip from CodexBar JSON."
    );
}
