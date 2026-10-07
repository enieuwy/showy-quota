//! Native CodexBar fetch data plane.
//!
//! This binary and the Zellij plugin consume the same refresh decisions in
//! [`coordinator`](showy_quota_zellij_core::coordinator). The coordinator owns
//! serve/discovery/fallback/backoff choices and emits host effects. This host
//! supplies wall-clock time and executes those effects with `ureq`,
//! `std::process::Command`, and the on-disk cache. It never signals a
//! session-owned serve: recycle/stop only touch a pid-validated owned serve.
//!
//! Cache, envelope, pidfile, and stamp layouts mirror `bin/showy-quota-fetch`
//! (legacy bash, now a thin shim over this binary) so generations stay
//! interchangeable: one atomic rename publishes payload+source+providerMeta
//! (`showy-quota/cache@2`), then a generation stamp commits.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use showy_quota_zellij_core::coordinator::{self, CliFallback, Effect, Event, Source, State};
use showy_quota_zellij_core::{
    parse_provider_config_payload, parse_usage_payload, parse_usage_payload_indexed,
    valid_provider_id, RenderConfig,
};

/// Mirrors the native parser cap and the shell
/// `SHOWY_QUOTA_MAX_USAGE_JSON_BYTES` default (5 MiB).
const MAX_USAGE_JSON_BYTES: usize = 5 * 1024 * 1024;
const ENVELOPE_SCHEMA: &str = "showy-quota/cache@2";
const HEALTH_PATH: &str = "/health";
const USAGE_PATH: &str = "/usage";

#[derive(Debug, Clone)]
struct Config {
    cache_dir: PathBuf,
    usage_file: PathBuf,
    usage_stamp: PathBuf,
    usage_lock: PathBuf,
    pid_file: PathBuf,
    serve_failure_stamp: PathBuf,
    serve_failure_count_file: PathBuf,
    cli_failure_stamp: PathBuf,
    discovery_failure_stamp: PathBuf,
    provider_failure_dir: PathBuf,
    bin: String,
    serve_url: String,
    serve_port: String,
    serve_refresh_interval: u64,
    serve_start_wait_tenths: u64,
    manage_serve: bool,
    lock_wait_tenths: u64,
    refresh_seconds: i64,
    serve_refresh_seconds: i64,
    health_timeout: Duration,
    usage_timeout: Duration,
    cli_timeout: u64,
    discovery_timeout: u64,
    serve_failures_before_cli: u8,
    serve_failure_backoff: u64,
    cli_failure_backoff: u64,
    discovery_backoff: u64,
    provider_backoff: u64,
    include_status: bool,
    max_usage_json_bytes: usize,
    force_refresh: bool,
    serve_only: bool,
}

#[derive(Debug, Default)]
struct Args {
    force_refresh: bool,
    mode: String,
    serve_action: String,
    status_json: bool,
    record_name: Option<String>,
    fixture_dir: Option<PathBuf>,
    sanitize: bool,
    replay_path: Option<String>,
    version_token_input: Option<String>,
    serve_base_url_check: bool,
}

pub fn run() -> Result<i32, String> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&raw)?;
    if let Some(input) = args.version_token_input.as_deref() {
        let text = if input == "-" {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|error| format!("read stdin: {error}"))?;
            buf
        } else {
            input.to_string()
        };
        match coordinator::codexbar_version_token(&text) {
            Some(token) => {
                println!("{token}");
                return Ok(0);
            }
            None => return Ok(1),
        }
    }
    let mut config = Config::from_env();
    config.force_refresh = args.force_refresh;
    if args.serve_base_url_check {
        return match normalized_loopback_base(&config.serve_url) {
            Some(base) => {
                println!("{base}");
                Ok(0)
            }
            None => {
                eprintln!("showy-quota-fetch: non-loopback serve URL refused");
                Ok(1)
            }
        };
    }
    if let Some(path) = args.replay_path.as_deref() {
        return replay(path);
    }
    if let Some(name) = args.record_name.as_deref() {
        let dir = args
            .fixture_dir
            .clone()
            .ok_or_else(|| "--record needs --fixture-dir DIR".to_string())?;
        return record(name, &dir, args.sanitize, &config);
    }
    match args.serve_action.as_str() {
        "status" => return serve_status(&config, args.status_json),
        "stop" => {
            ensure_cache_dir(&config)?;
            let guard = acquire_lock(&config)?;
            let result = stop_owned_serve(&config);
            drop(guard);
            return result.map(|_| 0);
        }
        "restart" => {
            if !config.manage_serve {
                eprintln!("showy-quota-fetch: serve management disabled");
                return Ok(1);
            }
            ensure_cache_dir(&config)?;
            let guard = acquire_lock(&config)?;
            let result = restart_owned_serve(&config);
            drop(guard);
            return result.map(|_| 0);
        }
        _ => {}
    }
    if args.mode == "path" {
        println!("{}", config.usage_file.display());
        return Ok(0);
    }
    if args.mode == "age" {
        println!("{}", file_age_seconds(&config.usage_file));
        return Ok(0);
    }
    ensure_cache_dir(&config)?;
    let cache_valid = cache_is_valid(&config);
    let cache_age = cache_age_seconds(&config);
    let fresh = cache_valid && cache_age < config.refresh_seconds;
    let serve_fresh =
        !config.serve_url.is_empty() && cache_valid && cache_age < config.serve_refresh_seconds;
    let cache_only = args.mode == "cache-only";
    config.serve_only = fresh && !args.force_refresh;
    if !args.force_refresh
        && (cache_only || (fresh && (config.serve_url.is_empty() || serve_fresh)))
    {
        return emit_cache(&config);
    }
    let Some(guard) = acquire_refresh_lock(&config, args.force_refresh)? else {
        return emit_cache(&config);
    };
    touch_heartbeat(&guard);
    quarantine_invalid_cache(&config);
    let fetched = refresh_via_coordinator(&config, &guard);
    drop(guard);
    match fetched {
        Ok(()) => emit_cache(&config),
        Err(error) => {
            eprintln!("showy-quota-fetch: {error}");
            emit_cache(&config)
        }
    }
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut args = Args {
        mode: "json".to_string(),
        ..Args::default()
    };
    let mut index = 0;
    while index < raw.len() {
        match raw[index].as_str() {
            "--refresh" => args.force_refresh = true,
            "--json" => {
                if !args.serve_action.is_empty() && args.serve_action == "status" {
                    args.status_json = true;
                } else if args.record_name.is_none() && args.replay_path.is_none() {
                    args.mode = "json".to_string();
                } else {
                    return Err("--json only applies to --serve-status".into());
                }
            }
            "--cache-only" => args.mode = "cache-only".to_string(),
            "--age" => args.mode = "age".to_string(),
            "--path" => args.mode = "path".to_string(),
            "--stop-serve" => {
                if !args.serve_action.is_empty() {
                    return Err("choose one serve action".into());
                }
                args.serve_action = "stop".to_string();
            }
            "--serve-status" => {
                if !args.serve_action.is_empty() {
                    return Err("choose one serve action".into());
                }
                args.serve_action = "status".to_string();
            }
            "--restart-serve" => {
                if !args.serve_action.is_empty() {
                    return Err("choose one serve action".into());
                }
                args.serve_action = "restart".to_string();
            }
            "--record" => {
                index += 1;
                let name = raw.get(index).cloned().unwrap_or_default();
                if name.is_empty() || !valid_fixture_name(&name) {
                    return Err("--record needs a safe name".into());
                }
                args.record_name = Some(name);
            }
            "--fixture-dir" => {
                index += 1;
                let dir = raw.get(index).cloned().unwrap_or_default();
                if dir.is_empty() {
                    return Err("--fixture-dir needs a directory".into());
                }
                args.fixture_dir = Some(PathBuf::from(dir));
            }
            "--sanitize" => args.sanitize = true,
            "--replay" => {
                index += 1;
                let path = raw.get(index).cloned().unwrap_or_default();
                if path.is_empty() {
                    return Err("--replay needs FILE|-".into());
                }
                args.replay_path = Some(path);
            }
            "--version-token" => {
                index += 1;
                let input = raw.get(index).cloned().unwrap_or_default();
                if input.is_empty() {
                    return Err("--version-token needs TEXT|-".into());
                }
                args.version_token_input = Some(input);
            }
            "--serve-base-url" => args.serve_base_url_check = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        index += 1;
    }
    if args.record_name.is_some() && args.fixture_dir.is_none() {
        return Err("--record needs --fixture-dir DIR".into());
    }
    if args.sanitize && args.record_name.is_none() {
        return Err("--sanitize only applies to --record".into());
    }
    if !args.serve_action.is_empty()
        && (args.force_refresh || args.mode != "json" || args.record_name.is_some())
    {
        return Err("serve actions do not accept cache/record flags".into());
    }
    Ok(args)
}

fn print_help() {
    println!(
        "usage: showy-quota-fetch [--refresh|--json|--cache-only|--age|--path]\n       \
         showy-quota-fetch --stop-serve|--serve-status [--json]|--restart-serve\n       \
         showy-quota-fetch --record NAME --fixture-dir DIR [--sanitize]\n       \
         showy-quota-fetch --replay FILE|-\n       \
         showy-quota-fetch --version-token TEXT|-\n       \
         showy-quota-fetch --serve-base-url"
    );
}

fn valid_fixture_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && name != "."
        && name != ".."
}

// --- environment -----------------------------------------------------------

impl Config {
    fn from_env() -> Self {
        let cache_dir = nonempty_env("SHOWY_QUOTA_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_cache_dir);
        let usage_file = nonempty_env("SHOWY_QUOTA_USAGE_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| cache_dir.join("usage.json"));
        let usage_stamp = nonempty_env("SHOWY_QUOTA_USAGE_STAMP")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("{}.updated-at", usage_file.display())));
        let usage_lock = nonempty_env("SHOWY_QUOTA_USAGE_LOCK")
            .map(PathBuf::from)
            .unwrap_or_else(|| cache_dir.join("usage.lock"));
        let pid_file = nonempty_env("SHOWY_QUOTA_CODEXBAR_SERVE_PID_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| cache_dir.join("codexbar-serve.pid"));
        let serve_url = std::env::var("SHOWY_QUOTA_CODEXBAR_SERVE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8080".into());
        let serve_port = serve_port_from_url(&serve_url).unwrap_or_else(|| "8080".into());
        let refresh = env_i64("SHOWY_QUOTA_REFRESH_SECONDS", 120);
        let serve_refresh = env_i64(
            "SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS",
            refresh.clamp(1, 60),
        );
        let serve_interval = env_u64("SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_INTERVAL_SECONDS", 0);
        Self {
            usage_file,
            usage_stamp,
            usage_lock,
            provider_failure_dir: cache_dir.join("provider-failures"),
            serve_failure_stamp: cache_dir.join("serve-failed-at"),
            serve_failure_count_file: cache_dir.join("serve-failed-count"),
            cli_failure_stamp: cache_dir.join("cli-failed-at"),
            discovery_failure_stamp: cache_dir.join("config-providers-failed-at"),
            cache_dir,
            pid_file,
            bin: nonempty_env("SHOWY_QUOTA_CODEXBAR_BIN").unwrap_or_else(|| "codexbar".into()),
            serve_url,
            serve_port,
            serve_refresh_interval: if serve_interval > 0 {
                serve_interval
            } else {
                refresh.max(1) as u64
            },
            serve_start_wait_tenths: env_u64("SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS", 30),
            manage_serve: env_bool("SHOWY_QUOTA_MANAGE_SERVE", true),
            lock_wait_tenths: env_u64("SHOWY_QUOTA_LOCK_WAIT_TENTHS", 100).min(36000),
            refresh_seconds: refresh,
            serve_refresh_seconds: serve_refresh,
            health_timeout: env_timeout("SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS", 10.0),
            usage_timeout: env_timeout("SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS", 30.0),
            cli_timeout: env_u64("SHOWY_QUOTA_CODEXBAR_CLI_TIMEOUT_SECONDS", 20).clamp(1, 300),
            discovery_timeout: env_u64("SHOWY_QUOTA_CODEXBAR_CONFIG_PROVIDERS_TIMEOUT_SECONDS", 5)
                .clamp(1, 60),
            serve_failures_before_cli: env_u64("SHOWY_QUOTA_CODEXBAR_SERVE_FAILURES_BEFORE_CLI", 3)
                .min(255) as u8,
            serve_failure_backoff: env_u64(
                "SHOWY_QUOTA_CODEXBAR_SERVE_FAILURE_BACKOFF_SECONDS",
                60,
            ),
            cli_failure_backoff: env_positive_u64(
                "SHOWY_QUOTA_CODEXBAR_CLI_FAILURE_BACKOFF_SECONDS",
                if refresh > 0 { refresh as u64 } else { 120 },
            ),
            discovery_backoff: env_u64("SHOWY_QUOTA_CODEXBAR_CONFIG_PROVIDERS_BACKOFF_SECONDS", 60),
            provider_backoff: env_positive_u64(
                "SHOWY_QUOTA_PROVIDER_FAILURE_BACKOFF_SECONDS",
                if refresh > 0 { refresh as u64 } else { 120 },
            ),
            include_status: env_bool("SHOWY_QUOTA_INCLUDE_STATUS", true),
            max_usage_json_bytes: env_u64(
                "SHOWY_QUOTA_MAX_USAGE_JSON_BYTES",
                MAX_USAGE_JSON_BYTES as u64,
            )
            .clamp(1, MAX_USAGE_JSON_BYTES as u64) as usize,
            force_refresh: false,
            serve_only: false,
        }
    }
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn default_cache_dir() -> PathBuf {
    if let Some(xdg) = nonempty_env("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("showy-quota");
    }
    let home = nonempty_env("HOME").unwrap_or_else(|| "/".into());
    PathBuf::from(home).join(".cache").join("showy-quota")
}

fn env_u64(name: &str, fallback: u64) -> u64 {
    nonempty_env(name)
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(fallback)
}

fn env_positive_u64(name: &str, fallback: u64) -> u64 {
    let value = env_u64(name, fallback);
    if value == 0 {
        fallback
    } else {
        value
    }
}

fn env_timeout(name: &str, fallback: f64) -> Duration {
    let seconds = nonempty_env(name)
        .and_then(|raw| raw.parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .unwrap_or(fallback);
    Duration::from_secs_f64(seconds.min(60.0))
}

fn env_i64(name: &str, fallback: i64) -> i64 {
    nonempty_env(name)
        .and_then(|raw| raw.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(fallback)
}

fn env_bool(name: &str, fallback: bool) -> bool {
    match nonempty_env(name)
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("1") | Some("true") | Some("yes") | Some("on") => true,
        Some("0") | Some("false") | Some("no") | Some("off") => false,
        _ => fallback,
    }
}

fn epoch_override() -> Option<i64> {
    nonempty_env("SHOWY_QUOTA_NOW_EPOCH")
        .filter(|raw| raw.len() <= 18 && raw.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|raw| raw.parse::<i64>().ok())
        .filter(|epoch| (0..=4_102_444_800).contains(epoch))
}

fn now_epoch() -> i64 {
    if let Some(epoch) = epoch_override() {
        return epoch;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn now_seconds() -> f64 {
    static CLOCK: std::sync::LazyLock<(f64, Instant)> = std::sync::LazyLock::new(|| {
        let epoch = epoch_override().map_or_else(
            || {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs_f64())
                    .unwrap_or(0.0)
            },
            |epoch| epoch as f64,
        );
        (epoch, Instant::now())
    });
    let (epoch, started) = &*CLOCK;
    epoch + started.elapsed().as_secs_f64()
}

fn serve_port_from_url(url: &str) -> Option<String> {
    coordinator::derive_port_from_url(url.trim())
}

fn normalized_loopback_base(url: &str) -> Option<String> {
    let base = url.trim().trim_end_matches('/').to_string();
    if base.is_empty() || !coordinator::is_loopback_serve_url(&base) {
        return None;
    }
    Some(base)
}

// --- cache -----------------------------------------------------------------

fn ensure_cache_dir(config: &Config) -> Result<(), String> {
    std::fs::create_dir_all(&config.cache_dir)
        .map_err(|error| format!("cache dir {}: {error}", config.cache_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&config.cache_dir)
            .map_err(|error| format!("cache dir {}: {error}", config.cache_dir.display()))?;
        let before = directory
            .metadata()
            .map_err(|error| format!("cache dir metadata: {error}"))?;
        // SAFETY: geteuid has no arguments or mutable state.
        if before.uid() != unsafe { libc::geteuid() } {
            return Err("cache directory is not owned by the current user".into());
        }
        directory
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cache dir privacy: {error}"))?;
        let after = directory
            .metadata()
            .map_err(|error| format!("cache dir metadata: {error}"))?;
        if after.uid() != before.uid()
            || after.mode() & 0o777 != 0o700
            || directory_identity(&config.cache_dir) != Some((after.dev(), after.ino()))
        {
            return Err("cache directory ownership or privacy changed".into());
        }
    }
    Ok(())
}

fn file_age_seconds(path: &Path) -> i64 {
    let mtime = std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64);
    match mtime {
        Some(mtime) => (now_epoch() - mtime).abs(),
        None => 999_999_999,
    }
}

fn cache_age_seconds(config: &Config) -> i64 {
    file_age_seconds(&config.usage_file)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Measurement {
    source: Source,
    updated_at: i64,
}

struct ValidatedCache {
    providers: Vec<serde_json::Value>,
    source: Source,
    provider_meta: BTreeMap<String, Measurement>,
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::Serve => "serve",
        Source::Cli => "cli",
        _ => "unknown",
    }
}

fn parse_source(value: &serde_json::Value) -> Result<Source, String> {
    match value.as_str() {
        Some("serve") => Ok(Source::Serve),
        Some("cli") => Ok(Source::Cli),
        Some("unknown") => Ok(Source::Unknown),
        _ => Err("invalid cache source".into()),
    }
}

fn validated_array(payload: &[u8], max_bytes: usize) -> Result<Vec<serde_json::Value>, String> {
    if payload.is_empty() || payload.len() > max_bytes {
        return Err("provider payload exceeded size cap".into());
    }
    let indexed =
        parse_usage_payload_indexed(payload).map_err(|_| "invalid provider payload".to_string())?;
    let values: Vec<serde_json::Value> =
        serde_json::from_slice(payload).map_err(|_| "invalid provider array".to_string())?;
    let mut ids = std::collections::BTreeSet::new();
    if values.len() != indexed.len()
        || values.iter().any(|record| !cache_record_is_usable(record))
        || indexed
            .iter()
            .any(|(_, record)| !ids.insert(record.provider.as_str()))
    {
        return Err("invalid or duplicate provider record".into());
    }
    Ok(values)
}

fn cache_usage_window_is_usable(window: &serde_json::Value) -> bool {
    window.is_object()
        && ["usedPercent", "remainingPercent"]
            .iter()
            .all(|field| window[*field].is_null() || window[*field].is_number())
        && ["resetsAt", "resetDescription"]
            .iter()
            .all(|field| window[*field].is_null() || window[*field].is_string())
        && (window["windowMinutes"].is_null() || window["windowMinutes"].as_i64().is_some())
}

/// Match the shell cache reader: retain raw malformed siblings for explanations,
/// but require an empty array or at least one usable provider record.
fn cache_record_is_usable(record: &serde_json::Value) -> bool {
    if !record["provider"].as_str().is_some_and(valid_provider_id) {
        return false;
    }
    let status = &record["status"];
    if !status.is_null()
        && (!status.is_object()
            || ["indicator", "url"]
                .iter()
                .any(|field| !status[*field].is_null() && !status[*field].is_string()))
    {
        return false;
    }
    let usage = &record["usage"];
    if usage.is_null() {
        return true;
    }
    if !usage.is_object()
        || ["primary", "secondary", "tertiary"]
            .iter()
            .any(|field| !usage[*field].is_null() && !cache_usage_window_is_usable(&usage[*field]))
    {
        return false;
    }
    match usage.get("extraRateWindows") {
        None => true,
        Some(serde_json::Value::Array(windows)) => windows.iter().all(|window| {
            window.is_object()
                && ["id", "title"]
                    .iter()
                    .all(|field| window[*field].is_null() || window[*field].is_string())
                && (window["usageKnown"].is_null() || window["usageKnown"].is_boolean())
                && (window["window"].is_null() || cache_usage_window_is_usable(&window["window"]))
        }),
        _ => false,
    }
}

fn validate_cache(bytes: &[u8], max_bytes: usize) -> Result<ValidatedCache, String> {
    #[derive(serde::Deserialize)]
    struct Envelope<'a> {
        #[serde(borrow)]
        providers: &'a serde_json::value::RawValue,
        #[serde(default)]
        source: Option<serde_json::Value>,
        #[serde(default, rename = "providerMeta")]
        provider_meta: Option<serde_json::Value>,
    }
    if bytes.is_empty()
        || bytes.len() > MAX_USAGE_JSON_BYTES
        || bytes.len() > max_bytes.saturating_add(4096)
    {
        return Err("invalid cache size".into());
    }
    let (payload, source, raw_meta) =
        if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{') {
            let envelope: Envelope<'_> =
                serde_json::from_slice(bytes).map_err(|_| "invalid cache envelope".to_string())?;
            let source = envelope
                .source
                .as_ref()
                .and_then(|value| parse_source(value).ok())
                .unwrap_or(Source::Unknown);
            (
                envelope.providers.get().as_bytes(),
                source,
                envelope.provider_meta,
            )
        } else {
            (bytes, Source::Unknown, None)
        };
    if payload.len() > max_bytes {
        return Err("provider payload exceeded size cap".into());
    }
    let providers: Vec<serde_json::Value> =
        serde_json::from_slice(payload).map_err(|_| "invalid cache provider array".to_string())?;
    if !providers.is_empty() && !providers.iter().any(cache_record_is_usable) {
        return Err("no usable cache provider record".into());
    }
    let mut provider_meta = BTreeMap::new();
    if let Some(entries) = raw_meta.as_ref().and_then(serde_json::Value::as_object) {
        for (id, entry) in entries {
            if !valid_provider_id(id) || !entry.is_object() {
                continue;
            }
            let Some(updated_at) = entry["updatedAt"].as_i64().filter(|epoch| *epoch >= 0) else {
                continue;
            };
            let source = parse_source(&entry["source"]).unwrap_or(Source::Unknown);
            provider_meta.insert(id.clone(), Measurement { source, updated_at });
        }
    }
    Ok(ValidatedCache {
        providers,
        source,
        provider_meta,
    })
}

fn validate_publication_cache(bytes: &[u8], max_bytes: usize) -> Result<ValidatedCache, String> {
    if bytes.is_empty()
        || bytes.len() > MAX_USAGE_JSON_BYTES
        || bytes.len() > max_bytes.saturating_add(4096)
    {
        return Err("invalid cache size".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "invalid cache JSON".to_string())?;
    if value.is_array() {
        return Ok(ValidatedCache {
            providers: validated_array(bytes, max_bytes)?,
            source: Source::Unknown,
            provider_meta: BTreeMap::new(),
        });
    }
    if value.get("schema").and_then(serde_json::Value::as_str) != Some(ENVELOPE_SCHEMA) {
        return Err("invalid cache envelope".into());
    }
    let source = parse_source(value.get("source").ok_or("missing cache source")?)?;
    let payload = serde_json::to_vec(value.get("providers").ok_or("missing cache providers")?)
        .map_err(|_| "invalid cache providers".to_string())?;
    let providers = validated_array(&payload, max_bytes)?;
    let raw_meta = value
        .get("providerMeta")
        .and_then(serde_json::Value::as_object)
        .ok_or("missing cache provider metadata")?;
    if raw_meta.len() != providers.len() {
        return Err("incomplete cache provider metadata".into());
    }
    let mut provider_meta = BTreeMap::new();
    for provider in &providers {
        let id = provider["provider"]
            .as_str()
            .ok_or("invalid cache provider")?;
        let entry = raw_meta.get(id).ok_or("missing provider measurement")?;
        let source = parse_source(entry.get("source").ok_or("missing measurement source")?)?;
        let updated_at = entry
            .get("updatedAt")
            .and_then(serde_json::Value::as_i64)
            .filter(|epoch| *epoch >= 0)
            .ok_or("invalid measurement time")?;
        provider_meta.insert(id.to_string(), Measurement { source, updated_at });
    }
    Ok(ValidatedCache {
        providers,
        source,
        provider_meta,
    })
}

fn read_limited(path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let file =
        std::fs::File::open(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() > max_bytes {
        return Err("cache exceeded size cap".into());
    }
    Ok(bytes)
}

fn read_valid_cache(config: &Config) -> Result<ValidatedCache, String> {
    let bytes = read_limited(
        &config.usage_file,
        config
            .max_usage_json_bytes
            .saturating_add(4096)
            .min(MAX_USAGE_JSON_BYTES),
    )?;
    validate_cache(&bytes, config.max_usage_json_bytes)
}

fn cache_is_valid(config: &Config) -> bool {
    read_valid_cache(config).is_ok()
}

fn emit_cache(config: &Config) -> Result<i32, String> {
    let bytes = read_limited(&config.usage_file, MAX_USAGE_JSON_BYTES)?;
    let cache = validate_cache(&bytes, config.max_usage_json_bytes)
        .map_err(|_| "no valid codexbar JSON cache available".to_string())?;
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'[') {
        use std::io::Write;
        std::io::stdout()
            .lock()
            .write_all(&bytes)
            .map_err(|error| format!("cache output: {error}"))?;
    } else {
        let payload =
            serde_json::to_string(&cache.providers).map_err(|error| format!("encode: {error}"))?;
        println!("{payload}");
    }
    Ok(0)
}

fn quarantine_invalid_cache(config: &Config) {
    if !config.usage_file.exists() || cache_is_valid(config) {
        return;
    }
    let target = PathBuf::from(format!(
        "{}.corrupt.{}.{}",
        config.usage_file.display(),
        now_epoch(),
        std::process::id()
    ));
    let _ = std::fs::rename(&config.usage_file, target);
    let retention = env_u64("SHOWY_QUOTA_CORRUPT_CACHE_RETENTION", 3) as usize;
    let Some(parent) = config.usage_file.parent() else {
        return;
    };
    let prefix = format!(
        "{}.corrupt.",
        config
            .usage_file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let mut quarantines: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .collect();
    quarantines.sort_by_key(|entry| entry.file_name());
    let remove_count = quarantines.len().saturating_sub(retention);
    for entry in quarantines.into_iter().take(remove_count) {
        let _ = std::fs::remove_file(entry.path());
    }
}

#[derive(PartialEq, Eq)]
struct CacheGeneration {
    stamp: Option<Vec<u8>>,
    payload: Option<String>,
}

fn cache_generation(config: &Config) -> CacheGeneration {
    CacheGeneration {
        stamp: read_limited(&config.usage_stamp, 4096).ok(),
        payload: payload_marker(config).ok(),
    }
}

fn wait_for_cache(config: &Config, previous: Option<&CacheGeneration>) -> Result<(), String> {
    for attempt in 0..=config.lock_wait_tenths {
        if cache_is_valid(config)
            && previous.is_none_or(|previous| {
                cache_generation(config) != *previous
                    && cache_age_seconds(config) < config.refresh_seconds
            })
        {
            return Ok(());
        }
        if attempt < config.lock_wait_tenths {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Err("refresh wait did not observe a valid cache generation".into())
}

fn acquire_refresh_lock(config: &Config, force_refresh: bool) -> Result<Option<LockGuard>, String> {
    let previous = force_refresh.then(|| cache_generation(config));
    if let Some(guard) = try_acquire_lock(config)? {
        return Ok(Some(guard));
    }
    if !force_refresh && cache_is_valid(config) {
        return Ok(None);
    }
    if wait_for_cache(config, previous.as_ref()).is_ok() {
        return Ok(None);
    }
    // The holder can exit without publishing. Retry the lease once, as the
    // shell does, before falling back to the most recent valid cache.
    if let Some(guard) = try_acquire_lock(config)? {
        return Ok(Some(guard));
    }
    let _ = wait_for_cache(config, None);
    Ok(None)
}

// --- mkdir lease ------------------------------------------------------------

struct LockGuard {
    dir: PathBuf,
    token: String,
    identity: (u64, u64),
    // This persistent file is never unlinked. The kernel releases its flock
    // on exit, and every native recovery/publication holds the same fence.
    _fence: std::fs::File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        release_lock(self);
    }
}

fn lock_dir(config: &Config) -> PathBuf {
    PathBuf::from(format!("{}.d", config.usage_lock.display()))
}

fn directory_identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path).ok()?;
        metadata.is_dir().then(|| (metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn try_acquire_lock(config: &Config) -> Result<Option<LockGuard>, String> {
    let parent = config.usage_lock.parent().ok_or("lock parent missing")?;
    std::fs::create_dir_all(parent).map_err(|error| format!("lock parent: {error}"))?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let fence = options
        .open(&config.usage_lock)
        .map_err(|error| format!("lock fence: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: the descriptor remains open for the entire lease.
        if unsafe { libc::flock(fence.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(format!("lock fence: {error}"));
        }
    }
    #[cfg(not(unix))]
    return Err("native leases require Unix flock".into());

    let dir = lock_dir(config);
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !recover_lock(&dir)? {
                return Ok(None);
            }
            match std::fs::create_dir(&dir) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
                Err(error) => return Err(format!("lock {}: {error}", dir.display())),
            }
        }
        Err(error) => return Err(format!("lock {}: {error}", dir.display())),
    }
    let identity = directory_identity(&dir).ok_or("lock directory identity missing")?;
    let token = unique_token();
    let guard = LockGuard {
        dir,
        token,
        identity,
        _fence: fence,
    };
    claim_lock(&guard)?;
    Ok(Some(guard))
}

fn unique_token() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}.{nanos}.{serial}", std::process::id())
}

fn acquire_lock(config: &Config) -> Result<LockGuard, String> {
    for attempt in 0..=config.lock_wait_tenths {
        if let Some(guard) = try_acquire_lock(config)? {
            return Ok(guard);
        }
        if attempt < config.lock_wait_tenths {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Err("another fetch in flight".into())
}

fn claim_lock(guard: &LockGuard) -> Result<(), String> {
    std::fs::write(guard.dir.join("owner.token"), &guard.token)
        .map_err(|error| format!("lock token: {error}"))?;
    std::fs::write(
        guard.dir.join("owner.pid"),
        format!("{}\n", std::process::id()),
    )
    .map_err(|error| format!("lock claim: {error}"))?;
    touch_heartbeat(guard);
    Ok(())
}

fn touch_heartbeat(guard: &LockGuard) {
    if lease_still_owned(guard) {
        let _ = std::fs::write(guard.dir.join("owner.heartbeat"), b"");
    }
}

fn lock_owner_record(dir: &Path) -> Option<String> {
    String::from_utf8(read_limited(&dir.join("owner.pid"), 4096).ok()?).ok()
}

fn parse_lock_owner(raw: &str) -> Option<(u32, Option<&str>)> {
    let line = raw.lines().next()?.trim();
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    let pid = line[..end].parse::<u32>().ok()?;
    if pid == 0 || pid > i32::MAX as u32 {
        return None;
    }
    let start = line[end..].trim();
    Some((pid, (!start.is_empty()).then_some(start)))
}

fn lock_owner_pid(dir: &Path) -> Option<u32> {
    parse_lock_owner(&lock_owner_record(dir)?).map(|(pid, _)| pid)
}

fn lock_owner_can_publish(pid: u32, start: Option<&str>) -> bool {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "state=", "-o", "lstart="])
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .output()
    else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    let Some(end) = text.find(char::is_whitespace) else {
        return true;
    };
    if text.starts_with(['T', 'Z']) {
        return false;
    }
    let live_start = text[end..].trim();
    live_start.is_empty()
        || start.is_none_or(|start| live_start.split_whitespace().eq(start.split_whitespace()))
}

fn owner_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill with signal 0 only probes liveness. EPERM is alive.
        unsafe {
            libc::kill(pid as libc::pid_t, 0) == 0
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn file_age(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .map(|mtime| (now_epoch() - mtime).max(0))
}

fn recover_lock(dir: &Path) -> Result<bool, String> {
    let identity = match directory_identity(dir) {
        Some(identity) => identity,
        None => return Ok(false),
    };
    let owner_record = lock_owner_record(dir);
    let owner = owner_record.as_deref().and_then(parse_lock_owner);
    let token = std::fs::read_to_string(dir.join("owner.token")).ok();
    // Running owners retain the lease regardless of age. Legacy claims also
    // carry UTC lstart identity; stopped, zombie, and reused-PID owners cannot publish.
    if owner.is_some_and(|(pid, start)| owner_alive(pid) && lock_owner_can_publish(pid, start))
        || (owner.is_none()
            && file_age(dir).unwrap_or(0)
                < env_u64("SHOWY_QUOTA_LOCK_WAIT_TENTHS", 100)
                    .div_ceil(10)
                    .max(1) as i64)
    {
        return Ok(false);
    }
    if directory_identity(dir) != Some(identity)
        || lock_owner_record(dir) != owner_record
        || std::fs::read_to_string(dir.join("owner.token")).ok() != token
    {
        return Ok(false);
    }
    // Detach the observed directory while holding the persistent flock.
    // Cleanup only the detached path, never a replacement owner's directory.
    let retired = dir.with_extension(format!("retired.{}", unique_token()));
    match std::fs::rename(dir, &retired) {
        Ok(()) => {
            if directory_identity(&retired) != Some(identity) {
                return Err("lock identity changed during recovery".into());
            }
            for name in ["owner.pid", "owner.token", "owner.heartbeat"] {
                let _ = std::fs::remove_file(retired.join(name));
            }
            let _ = std::fs::remove_dir(retired);
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("recover lock: {error}")),
    }
}

fn claim_still_matches(guard: &LockGuard) -> bool {
    directory_identity(&guard.dir) == Some(guard.identity)
        && std::fs::read_to_string(guard.dir.join("owner.token"))
            .ok()
            .as_deref()
            == Some(guard.token.as_str())
}

fn release_lock(guard: &LockGuard) {
    if claim_still_matches(guard) {
        for name in ["owner.pid", "owner.heartbeat", "owner.token"] {
            let _ = std::fs::remove_file(guard.dir.join(name));
        }
        let _ = std::fs::remove_dir(&guard.dir);
    }
}

fn lease_still_owned(guard: &LockGuard) -> bool {
    claim_still_matches(guard) && lock_owner_pid(&guard.dir) == Some(std::process::id())
}

// --- subprocesses ------------------------------------------------------------

enum RunOutcome {
    Ok(i32, Vec<u8>),
    Timeout,
    TooLarge,
}

#[cfg(unix)]
static BOUNDED_SIGNAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(unix)]
static BOUNDED_CANCEL_SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

#[cfg(unix)]
extern "C" fn bounded_cancel_signal(signal: libc::c_int) {
    // Only a lock-free atomic write runs in the signal handler.
    BOUNDED_CANCEL_SIGNAL.store(signal, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(unix)]
struct BoundedSignals {
    previous: [libc::sigaction; 2],
    installed: usize,
    pid: Option<u32>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(unix)]
impl BoundedSignals {
    fn install() -> Result<Self, String> {
        let lock = BOUNDED_SIGNAL_LOCK
            .lock()
            .map_err(|_| "bounded signal guard poisoned".to_string())?;
        BOUNDED_CANCEL_SIGNAL.store(0, std::sync::atomic::Ordering::Relaxed);
        // SAFETY: sigaction initializes each saved action before restoration.
        let mut guard = Self {
            previous: unsafe { std::mem::zeroed() },
            installed: 0,
            pid: None,
            _lock: lock,
        };
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = bounded_cancel_signal as *const () as usize;
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        for (index, signal) in [libc::SIGINT, libc::SIGTERM].into_iter().enumerate() {
            // SAFETY: both pointers name initialized, live sigaction storage.
            if unsafe { libc::sigaction(signal, &action, &mut guard.previous[index]) } != 0 {
                return Err(format!(
                    "bounded signal setup: {}",
                    std::io::Error::last_os_error()
                ));
            }
            guard.installed += 1;
        }
        Ok(guard)
    }

    fn cancelled(&self) -> bool {
        BOUNDED_CANCEL_SIGNAL.load(std::sync::atomic::Ordering::Relaxed) != 0
    }
}

#[cfg(unix)]
impl Drop for BoundedSignals {
    fn drop(&mut self) {
        // Block delivery during teardown so a final cancellation cannot disappear
        // between the last flag check and restoration of the original handlers.
        unsafe {
            let mut blocked: libc::sigset_t = std::mem::zeroed();
            let mut previous_mask: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut blocked);
            libc::sigaddset(&mut blocked, libc::SIGINT);
            libc::sigaddset(&mut blocked, libc::SIGTERM);
            libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut previous_mask);
            let signal = BOUNDED_CANCEL_SIGNAL.load(std::sync::atomic::Ordering::Relaxed);
            if signal != 0 {
                if let Some(pid) = self.pid {
                    // Only run_bounded registers a setsid child here. Managed
                    // serve processes never enter this temporary-command guard.
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                    while libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0) == -1
                        && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
                    {
                    }
                }
            }
            for (index, original) in self.previous.iter().take(self.installed).enumerate() {
                libc::sigaction(
                    [libc::SIGINT, libc::SIGTERM][index],
                    original,
                    std::ptr::null_mut(),
                );
            }
            if signal != 0 {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = libc::SIG_DFL;
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(signal, &action, std::ptr::null_mut());
                libc::sigdelset(&mut previous_mask, signal);
                libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut());
                libc::raise(signal);
                libc::_exit(128 + signal);
            }
            libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut());
        }
    }
}

fn run_bounded(argv: &[String], timeout: Duration, max_bytes: usize) -> Result<RunOutcome, String> {
    if argv.is_empty() {
        return Err("empty command".into());
    }
    #[cfg(unix)]
    let mut signals = BoundedSignals::install()?;
    #[cfg(unix)]
    if signals.cancelled() {
        return Err("bounded command cancelled".into());
    }
    let mut command = std::process::Command::new(&argv[0]);
    command.args(&argv[1..]);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: detach so a timeout kill does not signal the fetcher group.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn {}: {error}", argv[0]))?;
    #[cfg(unix)]
    {
        signals.pid = Some(child.id());
    }
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_child(&mut child);
            if child.wait().is_ok() {
                #[cfg(unix)]
                {
                    signals.pid = None;
                }
            }
            return Err("capture pipe missing".into());
        }
    };
    if let Err(error) = nonblocking_stdout(&stdout) {
        kill_child(&mut child);
        if child.wait().is_ok() {
            #[cfg(unix)]
            {
                signals.pid = None;
            }
        }
        return Err(error);
    }
    let deadline = Instant::now() + timeout.max(Duration::from_millis(1));
    let mut bytes = Vec::with_capacity(max_bytes.min(8192));
    let mut buffer = [0u8; 8192];
    let mut eof = false;
    loop {
        #[cfg(unix)]
        if signals.cancelled() {
            return Err("bounded command cancelled".into());
        }
        if !eof {
            let remaining = max_bytes.saturating_add(1).saturating_sub(bytes.len());
            let limit = remaining.min(buffer.len());
            match stdout.read(&mut buffer[..limit]) {
                Ok(0) => eof = true,
                Ok(count) => {
                    bytes.extend_from_slice(&buffer[..count]);
                    if bytes.len() > max_bytes {
                        kill_child(&mut child);
                        if child.wait().is_ok() {
                            #[cfg(unix)]
                            {
                                signals.pid = None;
                            }
                        }
                        return Ok(RunOutcome::TooLarge);
                    }
                    // Check time even while output remains readable.
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => {
                    kill_child(&mut child);
                    if child.wait().is_ok() {
                        #[cfg(unix)]
                        {
                            signals.pid = None;
                        }
                    }
                    return Err(format!("capture: {error}"));
                }
            }
        }
        // Do not reap the leader while a helper can still hold stdout. Its
        // unreaped PID fences the process group against reuse during cleanup.
        if eof {
            match child.try_wait() {
                Ok(Some(status)) => {
                    #[cfg(unix)]
                    {
                        signals.pid = None;
                    }
                    return Ok(RunOutcome::Ok(status.code().unwrap_or(1), bytes));
                }
                Ok(None) => {}
                Err(error) => {
                    kill_child(&mut child);
                    if child.wait().is_ok() {
                        #[cfg(unix)]
                        {
                            signals.pid = None;
                        }
                    }
                    return Err(format!("wait: {error}"));
                }
            }
        }
        if Instant::now() >= deadline {
            kill_child(&mut child);
            if child.wait().is_ok() {
                #[cfg(unix)]
                {
                    signals.pid = None;
                }
            }
            return Ok(RunOutcome::Timeout);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn nonblocking_stdout(stdout: &std::process::ChildStdout) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let fd = stdout.as_raw_fd();
        // SAFETY: only the owned capture descriptor's status flags change.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(format!(
                "capture flags: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = stdout;
        Err("bounded native capture requires Unix".into())
    }
}

fn kill_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        // SAFETY: spawn created this isolated process group. Kill it promptly
        // when output or time exceeds the bound, including inherited writers.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

// --- serve identity (never touch a session-owned serve) ----------------------

fn read_pid_record(config: &Config) -> Option<(u32, i64, String)> {
    let raw = std::fs::read_to_string(&config.pid_file).ok()?;
    let line = raw.lines().next().unwrap_or("").trim();
    let mut parts = line.splitn(3, ':');
    let pid = parts.next()?.parse::<u32>().ok()?;
    let start = parts.next()?.parse::<i64>().ok()?;
    let base = parts.next()?.to_string();
    if base.is_empty() || base.contains([':', '/', ' ', '\t']) {
        return None;
    }
    Some((pid, start, base))
}

fn expected_basename(config: &Config) -> String {
    config
        .bin
        .rsplit('/')
        .next()
        .unwrap_or(&config.bin)
        .to_string()
}

fn pid_start_epoch(pid: u32) -> Option<i64> {
    for _ in 0..10 {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .output()
            .ok()?;
        let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !line.is_empty() {
            if let Some(epoch) = parse_lstart(&line) {
                return Some(epoch);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn parse_lstart(line: &str) -> Option<i64> {
    // `ps -o lstart=` yields local time without a zone, e.g.
    // "Mon Oct  6 12:34:56 2026".
    let formats = ["%a %b %e %T %Y", "%a %e %b %T %Y"];
    for format in formats {
        if let Ok(epoch) = parse_local_epoch(format, line) {
            return Some(epoch);
        }
    }
    None
}

fn parse_local_epoch(format: &str, line: &str) -> Result<i64, String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let fields: Vec<&str> = format.split_whitespace().collect();
    if parts.len() != 5 || fields.len() != 5 {
        return Err("shape".into());
    }
    let (month_token, day_token, time_token, year_token) = if format.contains("%b %e") {
        (parts[1], parts[2], parts[3], parts[4])
    } else {
        (parts[2], parts[1], parts[3], parts[4])
    };
    let month = match month_token.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return Err("month".into()),
    };
    let day: i64 = day_token.parse().map_err(|_| "day".to_string())?;
    let year: i64 = year_token.parse().map_err(|_| "year".to_string())?;
    let mut clock = time_token.split(':');
    let hour: i64 = clock
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| "hh".to_string())?;
    let minute: i64 = clock
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| "mm".to_string())?;
    let second: i64 = clock
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| "ss".to_string())?;
    let month_value = time::Month::try_from(month as u8).map_err(|_| "month".to_string())?;
    let year_value = i32::try_from(year).map_err(|_| "year".to_string())?;
    let day_value = u8::try_from(day).map_err(|_| "day".to_string())?;
    time::Date::from_calendar_date(year_value, month_value, day_value)
        .map_err(|_| "date".to_string())?;
    if clock.next().is_some()
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&second)
    {
        return Err("clock".into());
    }
    #[cfg(unix)]
    {
        // SAFETY: mktime receives an initialized local calendar value. tm_isdst=-1
        // requests the offset for this historical date, rather than today's offset.
        let epoch = unsafe {
            let mut local: libc::tm = std::mem::zeroed();
            local.tm_year = year_value - 1900;
            local.tm_mon = month - 1;
            local.tm_mday = day as i32;
            local.tm_hour = hour as i32;
            local.tm_min = minute as i32;
            local.tm_sec = second as i32;
            local.tm_isdst = -1;
            libc::mktime(&mut local)
        };
        if epoch < 0 {
            return Err("local start time".into());
        }
        Ok(epoch as i64)
    }
    #[cfg(not(unix))]
    {
        Err("local process start conversion requires Unix".into())
    }
}

fn pid_command(pid: u32) -> Option<String> {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|command| !command.is_empty())
}

fn command_matches_owned(command: &str, basename: &str, port: &str) -> bool {
    let mut tokens = command.split_whitespace();
    let Some(mut binary) = tokens.next() else {
        return false;
    };
    // A direct script invocation exposes its shebang interpreter in `ps`.
    // Accept only the immediate script operand, never `-c` or a command string.
    if matches!(binary.rsplit('/').next(), Some("sh" | "bash")) {
        let Some(script) = tokens.next() else {
            return false;
        };
        if !script.contains('/') {
            return false;
        }
        binary = script;
    }
    if binary.rsplit('/').next() != Some(basename) || tokens.next() != Some("serve") {
        return false;
    }
    let mut saw_port = false;
    let mut previous = "";
    for token in tokens {
        if (previous == "--port" && token == port) || token.strip_prefix("--port=") == Some(port) {
            saw_port = true;
        }
        previous = token;
    }
    saw_port
}

/// A pidfile is owned only when pid, start epoch (±2s), basename, and the
/// live command line (`<bin> serve --port <port>`) all agree. Anything else is
/// foreign or stale and must never be signalled.
fn owned_serve_pid(config: &Config) -> Option<u32> {
    let (pid, recorded_start, recorded_base) = read_pid_record(config)?;
    if recorded_base != expected_basename(config) {
        return None;
    }
    if !owner_alive(pid) {
        return None;
    }
    #[cfg(unix)]
    {
        // Native starts detach into their own session. A session-owned command
        // is never adopted solely because its argv happens to match.
        if unsafe { libc::getsid(pid as libc::pid_t) } != pid as libc::pid_t {
            return None;
        }
    }
    let actual_start = pid_start_epoch(pid)?;
    if (actual_start - recorded_start).abs() > 2 {
        return None;
    }
    let command = pid_command(pid)?;
    if !command_matches_owned(&command, &recorded_base, &config.serve_port) {
        return None;
    }
    Some(pid)
}

fn stop_owned_serve(config: &Config) -> Result<bool, String> {
    let record = read_pid_record(config);
    let Some(pid) = owned_serve_pid(config) else {
        if read_pid_record(config) == record {
            let _ = std::fs::remove_file(&config.pid_file);
        }
        return Ok(false);
    };
    if !terminate_owned_pid(config, pid) {
        return Ok(false);
    }
    if read_pid_record(config) == record {
        let _ = std::fs::remove_file(&config.pid_file);
    }
    eprintln!("showy-quota-fetch: stopped managed codexbar serve pid {pid}");
    Ok(true)
}

fn terminate_owned_pid(config: &Config, pid: u32) -> bool {
    #[cfg(unix)]
    {
        if owned_serve_pid(config) != Some(pid) {
            return false;
        }
        let raw = pid as libc::pid_t;
        // SAFETY: pid was ownership-validated immediately before this call.
        if unsafe { libc::kill(raw, libc::SIGTERM) } != 0 {
            return !owner_alive(pid);
        }
        for _ in 0..50 {
            if !owner_alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if owned_serve_pid(config) != Some(pid) {
            return true;
        }
        // SAFETY: escalate only a validated owned serve that ignored TERM.
        if unsafe { libc::kill(raw, libc::SIGKILL) } != 0 {
            return !owner_alive(pid);
        }
        std::thread::sleep(Duration::from_secs(1));
        !owner_alive(pid) || owned_serve_pid(config) != Some(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = (config, pid);
        false
    }
}

// --- HTTP -------------------------------------------------------------------

fn agent(connect: Duration, read: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(connect)
        .timeout_read(read)
        .timeout(read)
        .redirects(0)
        .build()
}

fn http_get(url: &str, timeout: Duration, max_bytes: usize) -> Result<(u16, Vec<u8>), String> {
    let base = url
        .strip_suffix(HEALTH_PATH)
        .or_else(|| url.strip_suffix(USAGE_PATH))
        .ok_or("unsupported serve endpoint")?;
    if !coordinator::is_loopback_serve_url(base) {
        return Err("non-loopback serve URL refused".into());
    }
    let response = agent(timeout.min(Duration::from_secs(30)), timeout)
        .get(url)
        .set("Accept", "application/json")
        .call();
    match response {
        Ok(response) => {
            let status = response.status();
            let mut bytes = Vec::new();
            response
                .into_reader()
                .take(max_bytes.saturating_add(1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("read {url}: {error}"))?;
            if bytes.len() > max_bytes {
                return Err("response exceeded size cap".into());
            }
            Ok((status, bytes))
        }
        Err(ureq::Error::Status(code, _)) => Ok((code, Vec::new())),
        Err(error) => Err(format!("GET {url}: {error}")),
    }
}

fn serve_health(config: &Config) -> Option<(u16, Vec<u8>)> {
    let base = normalized_loopback_base(&config.serve_url)?;
    http_get(
        &format!("{base}{HEALTH_PATH}"),
        config.health_timeout,
        config.max_usage_json_bytes,
    )
    .ok()
}

fn health_ok(config: &Config) -> bool {
    matches!(serve_health(config), Some((200, _)))
}

fn serve_running_version(config: &Config) -> Option<String> {
    let base = normalized_loopback_base(&config.serve_url)?;
    let (status, body) = http_get(
        &format!("{base}{HEALTH_PATH}"),
        config.health_timeout,
        config.max_usage_json_bytes,
    )
    .ok()?;
    if status != 200 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&body).ok()?;
    coordinator::codexbar_version_token(value.get("version")?.as_str()?)
}

/// CodexBar reads its bundle version relative to argv[0]. Resolve bare names
/// before probing or spawning, and pin symlinks only when the basename survives.
fn resolve_codexbar_executable(binary: &str) -> PathBuf {
    if binary.contains('/') {
        return PathBuf::from(binary);
    }
    let Some(path) = std::env::var_os("PATH") else {
        return PathBuf::from(binary);
    };
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(binary);
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
        }
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            match std::env::current_dir() {
                Ok(current) => current.join(candidate),
                Err(_) => continue,
            }
        };
        if let Ok(canonical) = std::fs::canonicalize(&candidate) {
            if canonical.file_name() == candidate.file_name() {
                return canonical;
            }
        }
        return candidate;
    }
    PathBuf::from(binary)
}

fn ondisk_version(config: &Config) -> Option<String> {
    let argv = vec![
        resolve_codexbar_executable(&config.bin)
            .to_string_lossy()
            .into_owned(),
        "--version".to_string(),
    ];
    match run_bounded(&argv, Duration::from_secs(5), 65536) {
        Ok(RunOutcome::Ok(0, bytes)) => {
            coordinator::codexbar_version_token(&String::from_utf8_lossy(&bytes))
        }
        _ => None,
    }
}

// --- failure stamps ----------------------------------------------------------

fn read_epoch_file(path: &Path) -> Option<i64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| raw.lines().next().unwrap_or("").trim().parse::<i64>().ok())
}

fn write_epoch_file(path: &Path, epoch: i64) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format!("{epoch}\n"));
}

fn backoff_remaining(stamp: &Path, backoff: u64) -> u64 {
    if backoff == 0 {
        return 0;
    }
    match read_epoch_file(stamp) {
        Some(failed_at) => {
            let age = (now_epoch() - failed_at).max(0) as u64;
            backoff.saturating_sub(age)
        }
        None => 0,
    }
}

fn record_serve_failure(config: &Config) {
    write_epoch_file(&config.serve_failure_stamp, now_epoch());
    let count = read_epoch_file(&config.serve_failure_count_file).unwrap_or(0) + 1;
    write_epoch_file(&config.serve_failure_count_file, count);
}

fn clear_serve_failure(config: &Config) {
    let _ = std::fs::remove_file(&config.serve_failure_stamp);
    let _ = std::fs::remove_file(&config.serve_failure_count_file);
}

fn record_cli_failure(config: &Config) {
    write_epoch_file(&config.cli_failure_stamp, now_epoch());
}

fn clear_cli_failure(config: &Config) {
    let _ = std::fs::remove_file(&config.cli_failure_stamp);
}

fn provider_stamp(config: &Config, provider: &str) -> PathBuf {
    config.provider_failure_dir.join(provider)
}

fn provider_backoff_remaining(config: &Config, provider: &str) -> u64 {
    backoff_remaining(&provider_stamp(config, provider), config.provider_backoff)
}

fn record_provider_failure(config: &Config, provider: &str, reason: &str) {
    let stamp = provider_stamp(config, provider);
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&stamp, format!("{}\nrc={reason}\n", now_epoch()));
}

fn clear_provider_failure(config: &Config, provider: &str) {
    let _ = std::fs::remove_file(provider_stamp(config, provider));
}

fn log_message(message: &str) {
    let line = format!(
        "{} [showy-quota:{}] {message}\n",
        now_epoch(),
        std::process::id()
    );
    if let Some(path) = nonempty_env("SHOWY_QUOTA_LOG_FILE") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
    if env_bool("SHOWY_QUOTA_DEBUG", false) {
        eprint!("{line}");
    }
}

// --- coordinator host --------------------------------------------------------

fn coordinator_config(config: &Config) -> State {
    let mut state = State::default();
    state.now = now_seconds();
    state.serve_url = normalized_loopback_base(&config.serve_url).unwrap_or_default();
    state.serve_command = resolve_codexbar_executable(&config.bin)
        .to_string_lossy()
        .into_owned();
    state.serve_port = config.serve_port.clone();
    state.serve_refresh_seconds = config.serve_refresh_interval;
    state.cli_command = config.bin.clone();
    state.manage_serve = config.manage_serve && !state.serve_url.is_empty();
    state.interval_seconds = 60.0;
    state.cli_interval_seconds = (config.refresh_seconds.max(1)) as f64;
    state.provider_failure_backoff_seconds = (config.provider_backoff.max(1)) as f64;
    state.discovery_failure_backoff_seconds = (config.discovery_backoff.max(1)) as f64;
    state.health_timeout_seconds = config.health_timeout.as_secs_f64().max(1.0);
    state.usage_timeout_seconds = config.usage_timeout.as_secs_f64().max(1.0);
    state.provider_timeout_seconds = (config.cli_timeout.max(1)) as f64;
    state.discovery_timeout_seconds = (config.discovery_timeout.max(1)) as f64;
    state.failures_before_cli = config.serve_failures_before_cli.max(1);
    state.fallback_jitter_seconds = 0.0;
    state.show_build_marker = false;
    let render = RenderConfig::from_env();
    state.render_config.providers = render.providers.clone();
    state.render_config.providers_exclude = render.providers_exclude;
    state.render_config.provider_order = if render.providers.is_empty() {
        render.provider_order
    } else {
        render.providers.clone()
    };
    state.render_config.include_status = render.include_status;
    state.cli_fallback = CliFallback::Degraded;
    state.permissions_granted = true;
    state
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FetchMode {
    Refresh,
    Record,
}

struct FetchResult {
    payload: Vec<u8>,
    source: Source,
    provider_meta: BTreeMap<String, Measurement>,
}

fn seed_cached_providers(config: &Config, state: &mut State) {
    let Ok(mut cache) = read_valid_cache(config) else {
        return;
    };
    let legacy_time = std::fs::metadata(&config.usage_file)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0);
    // Tolerant reads retain malformed siblings for explanation, but synthesized
    // publications must contain only valid, unique provider records.
    cache.providers.retain(|value| {
        if !cache_record_is_usable(value) {
            return false;
        }
        let provider = value["provider"].as_str().expect("validated provider id");
        if state.provider_states.contains_key(provider) {
            return false;
        }
        let measurement = cache.provider_meta.get(provider);
        state.provider_states.insert(
            provider.to_string(),
            coordinator::ProviderFallbackState {
                last_record: Some(value.clone()),
                last_record_seconds: Some(
                    measurement.map_or(legacy_time, |meta| meta.updated_at) as f64
                ),
                last_record_source: Some(measurement.map_or(cache.source, |meta| meta.source)),
                ..Default::default()
            },
        );
        true
    });
    state.last_payload = serde_json::to_vec(&cache.providers).ok();
}

fn completed_fetch(
    state: &mut State,
    config: &Config,
    mode: FetchMode,
) -> Result<FetchResult, String> {
    if mode == FetchMode::Record
        && state
            .provider_states
            .values()
            .any(|provider| provider.last_failure_seconds.is_some())
    {
        return Err("live capture did not refresh every provider".into());
    }
    let payload = state
        .last_payload
        .take()
        .ok_or("fetch produced no usable output")?;
    let records = validated_array(&payload, config.max_usage_json_bytes)?;
    if mode == FetchMode::Record
        && records.is_empty()
        && state.discovered_providers_at.is_none()
        && state.source != Source::Serve
    {
        return Err("live capture has no confirmed empty inventory".into());
    }
    let mut provider_meta = BTreeMap::new();
    for record in records {
        let provider = record["provider"].as_str().ok_or("invalid provider")?;
        let entry = state
            .provider_states
            .get(provider)
            .ok_or("missing provider measurement")?;
        let updated_at = entry
            .last_record_seconds
            .filter(|time| time.is_finite() && *time >= 0.0)
            .ok_or("missing measurement time")? as i64;
        let source = entry.last_record_source.unwrap_or(Source::Unknown);
        provider_meta.insert(provider.to_string(), Measurement { source, updated_at });
    }
    Ok(FetchResult {
        payload,
        source: state.source,
        provider_meta,
    })
}

/// Drive a full bounded coordinator cycle. Only ordinary refreshes restore the
/// previous cache. Recording uses the same effects with fresh-only acquisition.
fn fetch_once(config: &Config, guard: &LockGuard, mode: FetchMode) -> Result<FetchResult, String> {
    let mut state = coordinator_config(config);
    let measurement_epoch = epoch_override().map(|epoch| epoch as f64);
    if mode == FetchMode::Refresh {
        seed_cached_providers(config, &mut state);
        if !state.render_config.providers.is_empty()
            && !state.provider_states.keys().any(|id| {
                state.render_config.providers.contains(id)
                    && !state.render_config.providers_exclude.contains(id)
            })
        {
            state.last_payload = None;
        }
        state.consecutive_serve_failures = read_epoch_file(&config.serve_failure_count_file)
            .unwrap_or(0)
            .clamp(0, 255) as u8;
        if config.force_refresh {
            state.failures_before_cli = 1;
        }
        let serve_cache = read_valid_cache(config).is_ok_and(|cache| cache.source == Source::Serve);
        if !config.force_refresh
            && !serve_cache
            && backoff_remaining(&config.serve_failure_stamp, config.serve_failure_backoff) > 0
        {
            if config.serve_only {
                return Err("serve in backoff".into());
            }
            state.serve_url.clear();
            state.manage_serve = false;
        }
        if state.manage_serve {
            if let Some(pid) = owned_serve_pid(config) {
                state.managed_serve_pane = Some(coordinator::PaneId::Terminal(pid));
                state.recycle_owned_serve = true;
                if !health_ok(config) && recycle_owned_serve(config, state.managed_serve_pane)? {
                    state.managed_serve_pane = None;
                }
                state.ondisk_version = ondisk_version(config);
                state.ondisk_version_checked_at = Some(state.now);
            }
        }
    }
    state.set_time(now_seconds());
    state.update(Event::PermissionRequestResult(
        coordinator::PermissionStatus::Granted,
    ));
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut scheduled = None;
    let mut cli_collection_started = false;
    let mut serve_failed = false;
    let mut serve_restarted = false;
    let mut inventory_drift = false;
    while Instant::now() < deadline {
        state.set_time(now_seconds());
        if !lease_still_owned(guard) {
            return Err("refresh lease lost".into());
        }
        touch_heartbeat(guard);
        for effect in state.take_effects() {
            if Instant::now() >= deadline {
                return Err("fetch deadline exceeded".into());
            }
            if matches!(&effect, Effect::FetchProvider { .. }) && mode == FetchMode::Refresh {
                if serve_failed
                    && !inventory_drift
                    && !config.force_refresh
                    && (config.serve_only
                        || (cache_is_valid(config)
                            && read_epoch_file(&config.serve_failure_count_file).unwrap_or(0)
                                < config.serve_failures_before_cli as i64))
                {
                    return Err("preserving cache after serve failure".into());
                }
                if !config.force_refresh
                    && !inventory_drift
                    && backoff_remaining(&config.cli_failure_stamp, config.cli_failure_backoff) > 0
                {
                    return Err("CLI in backoff".into());
                }
            }
            match &effect {
                Effect::DiscoverProviders { .. } if !state.usage_after_discovery => {
                    cli_collection_started = true;
                }
                Effect::FetchProvider { .. } => cli_collection_started = true,
                _ => {}
            }
            match effect {
                Effect::Schedule(delay) => {
                    let due = Instant::now() + Duration::from_secs_f64(delay.clamp(0.001, 180.0));
                    scheduled = Some(scheduled.map_or(due, |current: Instant| current.min(due)));
                }
                effect => {
                    execute_effect(
                        config,
                        &mut state,
                        effect,
                        guard,
                        mode,
                        deadline,
                        &mut serve_restarted,
                    )?;
                    if let Some(epoch) = measurement_epoch {
                        for provider in state.provider_states.values_mut() {
                            // The coordinator assigns this cycle's advancing time to
                            // fresh measurements; carried records retain their old time.
                            if provider.last_record_seconds == Some(state.now) {
                                provider.last_record_seconds = Some(epoch);
                            }
                        }
                    }
                    if mode == FetchMode::Refresh
                        && !serve_failed
                        && matches!(
                            state.last_error_class.as_deref(),
                            Some(
                                "serve_unavailable"
                                    | "usage_unavailable"
                                    | "inventory_mismatch"
                                    | "serve_start_failed"
                            )
                        )
                    {
                        record_serve_failure(config);
                        serve_failed = true;
                    }
                    inventory_drift |=
                        state.last_error_class.as_deref() == Some("inventory_mismatch");
                }
            }
            state.set_time(now_seconds());
        }
        // Inspect without consuming effects queued by the completed commands.
        // Finish only after every provider, lifecycle effect, and deferred
        // transition completes; the recurring idle poll does not delay success.
        let deferred =
            state.managed_serve_spawn_after_seconds.is_some() || state.cli_hold_until.is_some();
        if !state.has_work() && state.effects.is_empty() && !deferred {
            if state.last_success_seconds.is_some() {
                return completed_fetch(&mut state, config, mode);
            }
            if mode == FetchMode::Refresh && cli_collection_started {
                record_cli_failure(config);
            }
            return Err("fetch produced no usable output".into());
        }
        if !state.effects.is_empty() {
            continue;
        }
        if scheduled.is_some_and(|due| Instant::now() >= due) {
            scheduled = None;
            state.update(Event::Timer(state.now));
            continue;
        }
        if !state.has_work() && scheduled.is_none() && !deferred {
            break;
        }
        let wait = scheduled.map_or(Duration::from_millis(100), |due| {
            due.saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100))
        });
        std::thread::sleep(wait);
    }
    Err("fetch produced no usable output".into())
}

fn execute_effect(
    config: &Config,
    state: &mut State,
    effect: Effect,
    guard: &LockGuard,
    mode: FetchMode,
    deadline: Instant,
    serve_restarted: &mut bool,
) -> Result<(), String> {
    if !lease_still_owned(guard) {
        return Err("refresh lease lost".into());
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("fetch deadline exceeded".into());
    }
    match effect {
        Effect::Schedule(_) => unreachable!("the host loop owns coordinator timers"),
        Effect::ProbeHealth { url, context } => {
            match http_get(
                &url,
                config.health_timeout.min(remaining),
                config.max_usage_json_bytes,
            ) {
                Ok((status, body)) => {
                    state.update(Event::WebRequestResult(
                        status,
                        BTreeMap::new(),
                        body,
                        context,
                    ));
                }
                Err(_) => {
                    state.update(Event::WebRequestResult(
                        0,
                        BTreeMap::new(),
                        Vec::new(),
                        context,
                    ));
                }
            }
            Ok(())
        }
        Effect::FetchUsage { url, context } => {
            let mut response = http_get(
                &url,
                config.usage_timeout.min(remaining),
                config.max_usage_json_bytes,
            );
            let unusable = matches!(&response, Ok((200, body))
                if validated_array(body, config.max_usage_json_bytes)
                    .map_or(true, |records| records.is_empty()));
            if unusable
                && !*serve_restarted
                && config.manage_serve
                && owned_serve_pid(config).is_some()
                && stop_owned_serve(config)?
            {
                *serve_restarted = true;
                if let Some(pane) = start_managed_serve(config, remaining)? {
                    state.managed_serve_pane = Some(pane);
                    response = http_get(
                        &url,
                        config.usage_timeout.min(remaining),
                        config.max_usage_json_bytes,
                    );
                }
            }
            let (status, body) = response.unwrap_or((0, Vec::new()));
            state.update(Event::WebRequestResult(
                status,
                BTreeMap::new(),
                body,
                context,
            ));
            Ok(())
        }
        Effect::StartServe { command, context } => {
            let mut pane = start_managed_serve(config, remaining).unwrap_or(None);
            if pane.is_some()
                && !health_ok(config)
                && !*serve_restarted
                && stop_owned_serve(config)?
            {
                *serve_restarted = true;
                pane = start_managed_serve(config, remaining).unwrap_or(None);
            }
            let _ = command;
            state.update(Event::ManagedServeStarted(pane, context));
            Ok(())
        }
        Effect::DiscoverProviders { argv, context } => {
            let _ = argv;
            match discover_provider_ids(
                config,
                mode,
                Duration::from_secs(config.discovery_timeout).min(remaining),
            ) {
                Ok(ids) => {
                    let stdout = provider_config_json(&ids);
                    state.update(Event::RunCommandResult(
                        Some(0),
                        stdout,
                        Vec::new(),
                        context,
                    ));
                }
                Err(_) => {
                    state.update(Event::RunCommandResult(
                        Some(1),
                        Vec::new(),
                        Vec::new(),
                        context,
                    ));
                }
            }
            Ok(())
        }
        Effect::FetchProvider { argv, context } => {
            let provider = context
                .get("showy-quota-provider")
                .cloned()
                .unwrap_or_default();
            let _ = argv;
            match fetch_provider_payload(
                config,
                &provider,
                mode,
                Duration::from_secs(config.cli_timeout).min(remaining),
            ) {
                Ok(bytes) => {
                    state.update(Event::RunCommandResult(Some(0), bytes, Vec::new(), context));
                }
                Err(_) => {
                    state.update(Event::RunCommandResult(
                        Some(1),
                        Vec::new(),
                        Vec::new(),
                        context,
                    ));
                }
            }
            Ok(())
        }
        Effect::ProbeVersion { binary, context } => {
            let argv = vec![
                resolve_codexbar_executable(&binary)
                    .to_string_lossy()
                    .into_owned(),
                "--version".to_string(),
            ];
            match run_bounded(&argv, Duration::from_secs(5).min(remaining), 65536) {
                Ok(RunOutcome::Ok(code, bytes)) => {
                    state.update(Event::RunCommandResult(
                        Some(code),
                        bytes,
                        Vec::new(),
                        context,
                    ));
                }
                _ => {
                    state.update(Event::RunCommandResult(
                        Some(1),
                        Vec::new(),
                        Vec::new(),
                        context,
                    ));
                }
            }
            Ok(())
        }
        Effect::RecycleOwnedServe => {
            if !lease_still_owned(guard) {
                return Err("refresh lease lost".into());
            }
            let stopped = recycle_owned_serve(config, state.managed_serve_pane)?;
            state.update(Event::ManagedServeRecycled(stopped));
            Ok(())
        }
    }
}

fn start_managed_serve(
    config: &Config,
    budget: Duration,
) -> Result<Option<coordinator::PaneId>, String> {
    if !config.manage_serve {
        return Ok(None);
    }
    if normalized_loopback_base(&config.serve_url).is_none() {
        return Ok(None);
    }
    if let Some(pid) = owned_serve_pid(config) {
        return Ok(Some(coordinator::PaneId::Terminal(pid)));
    }
    let mut command = std::process::Command::new(resolve_codexbar_executable(&config.bin));
    command.args([
        "serve",
        "--port",
        &config.serve_port,
        "--refresh-interval",
        &config.serve_refresh_interval.to_string(),
    ]);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: detach the serve so fetcher exit never takes it down.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("start serve: {error}"))?;
    let pid = child.id();
    if let Err(error) = write_pid_record(config, pid) {
        kill_child(&mut child);
        let _ = child.wait();
        return Err(error);
    }
    let waits = config.serve_start_wait_tenths.clamp(1, 300);
    let deadline = Instant::now() + Duration::from_millis(waits * 100).min(budget);
    let base = normalized_loopback_base(&config.serve_url).ok_or("serve URL disabled")?;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => {
                if read_pid_record(config).is_some_and(|record| record.0 == pid) {
                    let _ = std::fs::remove_file(&config.pid_file);
                }
                return Ok(None);
            }
            Ok(None) => {}
            Err(error) => {
                kill_child(&mut child);
                let _ = child.wait();
                if read_pid_record(config).is_some_and(|record| record.0 == pid) {
                    let _ = std::fs::remove_file(&config.pid_file);
                }
                return Err(format!("wait for serve: {error}"));
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero()
            && matches!(
                http_get(
                    &format!("{base}{HEALTH_PATH}"),
                    config.health_timeout.min(remaining),
                    config.max_usage_json_bytes
                ),
                Ok((200, _))
            )
            && owned_serve_pid(config) == Some(pid)
            && serve_port_ownership(config, pid).unwrap_or(true)
        {
            return Ok(Some(coordinator::PaneId::Terminal(pid)));
        }
        std::thread::sleep(
            Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    if owned_serve_pid(config) == Some(pid) && !health_ok(config) {
        return Ok(Some(coordinator::PaneId::Terminal(pid)));
    }
    kill_child(&mut child);
    let _ = child.wait();
    if read_pid_record(config).is_some_and(|record| record.0 == pid) {
        let _ = std::fs::remove_file(&config.pid_file);
    }
    Ok(None)
}

fn write_pid_record(config: &Config, pid: u32) -> Result<(), String> {
    let start = pid_start_epoch(pid).ok_or("cannot establish serve start identity")?;
    let base = expected_basename(config);
    let tmp = config
        .pid_file
        .with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, format!("{pid}:{start}:{base}\n"))
        .map_err(|error| format!("pidfile: {error}"))?;
    std::fs::rename(&tmp, &config.pid_file).map_err(|error| format!("pidfile: {error}"))?;
    Ok(())
}

/// Recycle only a validated owned serve that is unhealthy or whose build is stale.
/// A foreign or session-owned serve is never signalled; report it as not stopped so the
/// coordinator resumes the usage path.
fn recycle_owned_serve(
    config: &Config,
    expected: Option<coordinator::PaneId>,
) -> Result<bool, String> {
    if !config.manage_serve {
        return Ok(false);
    }
    let Some(coordinator::PaneId::Terminal(pid)) = expected else {
        return Ok(false);
    };
    if owned_serve_pid(config) != Some(pid) {
        return Ok(false);
    }
    // The coordinator owns the version decision and replacement lifecycle.
    // Revalidate this exact native identity immediately before stopping it.
    stop_owned_serve(config)
}

fn restart_owned_serve(config: &Config) -> Result<(), String> {
    if !config.manage_serve {
        return Err("serve management disabled".into());
    }
    if normalized_loopback_base(&config.serve_url).is_none() {
        return Err("serve restart requires a local loopback URL".into());
    }
    if lsof_executable().is_none() {
        return Err("port ownership unavailable: lsof not found".into());
    }
    let previous = owned_serve_pid(config);
    if previous.is_some() && !stop_owned_serve(config)? {
        return Err("could not stop managed serve".into());
    }
    if health_ok(config) {
        return Err("another serve owns the configured port; refusing to restart it".into());
    }
    let replacement = start_managed_serve(config, Duration::from_secs(30))?;
    if let Some(coordinator::PaneId::Terminal(pid)) = replacement {
        if previous != Some(pid)
            && owned_serve_pid(config) == Some(pid)
            && owned_serve_owns_port(config, pid)
            && health_ok(config)
        {
            clear_serve_failure(config);
            println!("managed codexbar serve restarted (pid {pid})");
            return Ok(());
        }
    }
    record_serve_failure(config);
    Err("managed serve did not become healthy with a verified owned pid".into())
}

fn discover_provider_ids(
    config: &Config,
    mode: FetchMode,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    if mode == FetchMode::Refresh
        && !config.force_refresh
        && backoff_remaining(&config.discovery_failure_stamp, config.discovery_backoff) > 0
    {
        return Err("discovery in backoff".into());
    }
    let argv = vec![
        config.bin.clone(),
        "config".to_string(),
        "providers".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--pretty".to_string(),
    ];
    let outcome = run_bounded(&argv, timeout, config.max_usage_json_bytes.min(65536))?;
    match outcome {
        RunOutcome::Ok(0, bytes) => match parse_provider_config_payload(&bytes) {
            Ok(ids) => {
                if mode == FetchMode::Refresh {
                    let _ = std::fs::remove_file(&config.discovery_failure_stamp);
                }
                Ok(ids)
            }
            Err(_) => {
                if mode == FetchMode::Refresh {
                    write_epoch_file(&config.discovery_failure_stamp, now_epoch());
                }
                Err("invalid provider inventory".into())
            }
        },
        _ => {
            if mode == FetchMode::Refresh {
                write_epoch_file(&config.discovery_failure_stamp, now_epoch());
            }
            Err("discovery failed".into())
        }
    }
}

fn provider_config_json(ids: &[String]) -> Vec<u8> {
    let records: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| serde_json::json!({"provider": id, "enabled": true}))
        .collect();
    serde_json::to_vec(&serde_json::Value::Array(records)).unwrap_or_else(|_| b"[]".to_vec())
}

fn fetch_provider_payload(
    config: &Config,
    provider: &str,
    mode: FetchMode,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    if !valid_provider_id(provider) {
        return Err("invalid provider".into());
    }
    if mode == FetchMode::Refresh
        && !config.force_refresh
        && provider_backoff_remaining(config, provider) > 0
    {
        return Err("provider in backoff".into());
    }
    let mut argv = vec![
        config.bin.clone(),
        "usage".to_string(),
        "--provider".to_string(),
        provider.to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--pretty".to_string(),
    ];
    if config.include_status {
        argv.push("--status".to_string());
    }
    let (result, reason) = match run_bounded(&argv, timeout, config.max_usage_json_bytes) {
        Ok(RunOutcome::Ok(0, bytes)) => (
            validate_provider_payload(&bytes, provider, config.max_usage_json_bytes)
                .map(|()| bytes),
            "unrenderable".to_string(),
        ),
        Ok(RunOutcome::Ok(code, _)) => {
            (Err(format!("provider {provider} failed")), code.to_string())
        }
        Ok(RunOutcome::Timeout) => (Err(format!("provider {provider} timed out")), "124".into()),
        Ok(RunOutcome::TooLarge) => (
            Err(format!("provider {provider} exceeded size cap")),
            "125".into(),
        ),
        Err(error) => (Err(error), "127".into()),
    };
    if mode == FetchMode::Refresh {
        if result.is_ok() {
            clear_provider_failure(config, provider);
        } else {
            record_provider_failure(config, provider, &reason);
            log_message(&format!(
                "codexbar usage --provider {provider} failed (rc={reason})"
            ));
        }
    }
    result
}

fn validate_provider_payload(
    payload: &[u8],
    provider: &str,
    max_bytes: usize,
) -> Result<(), String> {
    let records = validated_array(payload, max_bytes)?;
    if records
        .iter()
        .any(|record| record["provider"].as_str() != Some(provider))
    {
        return Err("mismatched provider payload".into());
    }
    // Keep the validated array transport intact. The coordinator extracts its record.
    Ok(())
}

// --- publish ---------------------------------------------------------------

fn publish_payload(config: &Config, guard: &LockGuard, fetched: FetchResult) -> Result<(), String> {
    let providers = validated_array(&fetched.payload, config.max_usage_json_bytes)?;
    let mut meta = serde_json::Map::new();
    for (provider, measurement) in fetched.provider_meta {
        meta.insert(
            provider,
            serde_json::json!({
                "source": source_name(measurement.source), "updatedAt": measurement.updated_at,
            }),
        );
    }
    let envelope = serde_json::json!({
        "schema": ENVELOPE_SCHEMA,
        "source": source_name(fetched.source),
        "providers": providers,
        "providerMeta": meta,
    });
    let bytes = serde_json::to_vec(&envelope).map_err(|error| format!("encode: {error}"))?;
    validate_publication_cache(&bytes, config.max_usage_json_bytes)?;
    if !lease_still_owned(guard) {
        return Err("refresh lease lost; skipping publication".into());
    }
    atomic_write(&config.usage_file, &bytes)?;
    write_stamp(config)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            std::fs::set_permissions(&config.usage_file, std::fs::Permissions::from_mode(0o600));
        let _ =
            std::fs::set_permissions(&config.usage_stamp, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "cache path".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| format!("cache dir: {error}"))?;
    let tmp = parent.join(format!(".usage.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|error| format!("stage cache: {error}"))?;
    std::fs::rename(&tmp, path).map_err(|error| format!("publish cache: {error}"))?;
    Ok(())
}

fn payload_marker(config: &Config) -> Result<String, String> {
    let metadata =
        std::fs::metadata(&config.usage_file).map_err(|error| format!("stat: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        Ok(format!(
            "file:{}:{}:{}",
            metadata.ino(),
            mtime,
            metadata.len()
        ))
    }
    #[cfg(not(unix))]
    {
        use sha2::Digest;
        let bytes = std::fs::read(&config.usage_file).map_err(|error| format!("stat: {error}"))?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        Ok(format!("sha256:{:x}", hasher.finalize()))
    }
}

fn write_stamp(config: &Config) -> Result<(), String> {
    let marker = payload_marker(config)?;
    let content = format!(
        "{}.{}.{}.{}\n",
        marker,
        now_epoch(),
        std::process::id(),
        now_epoch() % 32768
    );
    atomic_write(&config.usage_stamp, content.as_bytes())
}

fn refresh_via_coordinator(config: &Config, guard: &LockGuard) -> Result<(), String> {
    let fetched = match fetch_once(config, guard, FetchMode::Refresh) {
        Ok(fetched) => fetched,
        Err(error) => {
            return Err(error);
        }
    };
    let source = fetched.source;
    publish_payload(config, guard, fetched)?;
    if source == Source::Serve {
        clear_serve_failure(config);
    }
    clear_cli_failure(config);
    Ok(())
}

// --- record / replay --------------------------------------------------------

fn record(name: &str, dir: &Path, sanitize: bool, config: &Config) -> Result<i32, String> {
    let guard = acquire_lock(config)?;
    std::fs::create_dir_all(dir).map_err(|error| format!("fixture dir: {error}"))?;
    // Recording shares the lease, but never restores cache data or reads/writes
    // ordinary failure stamps. Any failed provider makes capture fail explicitly.
    let fetched = fetch_once(config, &guard, FetchMode::Record)?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&fetched.payload).map_err(|_| "invalid live JSON".to_string())?;
    if sanitize {
        sanitize_value(&mut value);
    }
    let pretty =
        serde_json::to_string_pretty(&value).map_err(|error| format!("encode: {error}"))?;
    let target = dir.join(format!("{name}.json"));
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, format!("{pretty}\n"))
        .map_err(|error| format!("write fixture: {error}"))?;
    if !lease_still_owned(&guard) {
        let _ = std::fs::remove_file(&tmp);
        return Err("capture lease lost; skipping fixture publication".into());
    }
    std::fs::rename(&tmp, &target).map_err(|error| format!("publish fixture: {error}"))?;
    println!("{}", target.display());
    Ok(0)
}

fn canonical_array_bytes(payload: &[u8]) -> Result<Vec<u8>, String> {
    let providers = validate_cache(payload, MAX_USAGE_JSON_BYTES)?.providers;
    serde_json::to_vec(&providers).map_err(|error| format!("encode: {error}"))
}

fn sanitize_value(value: &mut serde_json::Value) {
    const IDENTITY_KEYS: [&str; 10] = [
        "email",
        "username",
        "user",
        "userId",
        "user_id",
        "accountId",
        "account_id",
        "customerId",
        "teamId",
        "orgId",
    ];
    match value {
        serde_json::Value::Object(map) => {
            for key in IDENTITY_KEYS {
                if map.contains_key(key) {
                    map.insert(key.to_string(), serde_json::Value::String("***".into()));
                }
            }
            for (_, child) in map.iter_mut() {
                sanitize_value(child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                sanitize_value(item);
            }
        }
        _ => {}
    }
}

fn replay(path: &str) -> Result<i32, String> {
    let bytes = if path == "-" {
        let fixture = nonempty_env("SHOWY_QUOTA_FIXTURE");
        match fixture {
            Some(inline) if !inline.is_empty() => inline.into_bytes(),
            _ => {
                let mut buf = Vec::new();
                std::io::stdin()
                    .read_to_end(&mut buf)
                    .map_err(|error| format!("read stdin: {error}"))?;
                buf
            }
        }
    } else {
        std::fs::read(path).map_err(|error| format!("read {path}: {error}"))?
    };
    if bytes.len() > MAX_USAGE_JSON_BYTES {
        return Err("fixture exceeded size cap".into());
    }
    parse_usage_payload(&bytes).map_err(|_| "fixture failed validation".to_string())?;
    let canonical = canonical_array_bytes(&bytes)?;
    let value: serde_json::Value =
        serde_json::from_slice(&canonical).map_err(|error| format!("encode: {error}"))?;
    let pretty =
        serde_json::to_string_pretty(&value).map_err(|error| format!("encode: {error}"))?;
    println!("{pretty}");
    Ok(0)
}

// --- serve status ------------------------------------------------------------

fn serve_url_port(config: &Config) -> Option<u16> {
    let base = normalized_loopback_base(&config.serve_url)?;
    let authority = base.strip_prefix("http://")?;
    if authority.ends_with(']') || !authority.contains(':') {
        Some(80)
    } else {
        authority.rsplit(':').next()?.parse().ok()
    }
}

/// Endpoint health alone cannot prove that the recorded process owns its port.
fn owned_serve_owns_port(config: &Config, pid: u32) -> bool {
    serve_port_ownership(config, pid) == Some(true)
}

fn lsof_executable() -> Option<PathBuf> {
    let binary = resolve_codexbar_executable("lsof");
    binary.is_absolute().then_some(binary)
}

/// None means the optional listener tool is unavailable, not proof of ownership.
fn serve_port_ownership(config: &Config, pid: u32) -> Option<bool> {
    let binary = lsof_executable()?;
    let Some(port) = serve_url_port(config) else {
        return Some(false);
    };
    let argv = vec![
        binary.to_string_lossy().into_owned(),
        "-ti".to_string(),
        format!("tcp:{port}"),
        "-sTCP:LISTEN".to_string(),
    ];
    let Ok(RunOutcome::Ok(0, bytes)) = run_bounded(&argv, Duration::from_secs(2), 65536) else {
        return Some(false);
    };
    for line in String::from_utf8_lossy(&bytes).lines() {
        let Ok(mut listener) = line.trim().parse::<u32>() else {
            continue;
        };
        for _ in 0..32 {
            if listener == pid {
                return Some(true);
            }
            let Ok(output) = std::process::Command::new("ps")
                .args(["-p", &listener.to_string(), "-o", "ppid="])
                .output()
            else {
                break;
            };
            let Ok(parent) = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u32>()
            else {
                break;
            };
            if parent <= 1 || parent == listener {
                break;
            }
            listener = parent;
        }
    }
    Some(false)
}

fn serve_status(config: &Config, as_json: bool) -> Result<i32, String> {
    serve_status_to(config, as_json, &mut std::io::stdout().lock())
}

fn serve_status_to(
    config: &Config,
    as_json: bool,
    output: &mut impl std::io::Write,
) -> Result<i32, String> {
    let pid_record = read_pid_record(config);
    let mut ownership_reason = None;
    let (pidfile_state, pid, alive, owned) = match pid_record {
        None => (
            if config.pid_file.exists() {
                "malformed"
            } else {
                "missing"
            }
            .to_string(),
            None,
            false,
            false,
        ),
        Some((pid, _, _)) => {
            let owned = if owned_serve_pid(config) == Some(pid) {
                match serve_port_ownership(config, pid) {
                    Some(true) => true,
                    Some(false) => {
                        ownership_reason = Some("managed pid does not own the configured port");
                        false
                    }
                    None => {
                        ownership_reason = Some("port ownership unavailable: lsof not found");
                        false
                    }
                }
            } else {
                false
            };
            (
                if owned { "owned" } else { "stale" }.to_string(),
                Some(pid),
                owner_alive(pid),
                owned,
            )
        }
    };
    let url = config.serve_url.clone();
    let reachable = health_ok(config);
    let healthy = owned && reachable;
    let port = serve_url_port(config);
    let version = serve_running_version(config).unwrap_or_default();
    let count = read_epoch_file(&config.serve_failure_count_file).unwrap_or(0);
    let failed_at = read_epoch_file(&config.serve_failure_stamp);
    let remaining = failed_at
        .map(|_| backoff_remaining(&config.serve_failure_stamp, config.serve_failure_backoff))
        .unwrap_or(0);
    let (cache_source, cache_age) = match read_valid_cache(config) {
        Ok(cache) => (
            source_name(cache.source).to_string(),
            Some(cache_age_seconds(config)),
        ),
        Err(_) => ("unknown".into(), None),
    };
    if as_json {
        let mut managed = serde_json::json!({
            "pidfileState": pidfile_state, "pid": pid, "alive": alive, "owned": owned
        });
        if let Some(reason) = ownership_reason {
            managed["reason"] = reason.into();
        }
        let value = serde_json::json!({
            "healthy": healthy,
            "managed": managed,
            "serve": {"url": if url.is_empty() { None } else { Some(&url) }, "port": port,
                "localOnly": normalized_loopback_base(&url).is_some(),
                "healthReachable": reachable,
                "version": if version.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(version.clone()) }},
            "failure": {"count": count, "lastFailedAt": failed_at,
                "backoffSeconds": config.serve_failure_backoff, "backoffRemainingSeconds": remaining},
            "cache": {"source": cache_source, "ageSeconds": cache_age},
        });
        writeln!(output, "{value}").map_err(|error| format!("serve status output: {error}"))?;
    } else {
        writeln!(
            output,
            "managed serve: {pidfile_state}{} (alive: {alive}, owned: {owned}){}",
            pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default(),
            ownership_reason
                .map(|reason| format!(" — {reason}"))
                .unwrap_or_default()
        )
        .map_err(|error| format!("serve status output: {error}"))?;
        writeln!(
            output,
            "serve: {} (port: {}, local: {}, /health: {reachable}, version: {})",
            if url.is_empty() { "disabled" } else { &url },
            port.map(|port| port.to_string())
                .unwrap_or_else(|| "none".into()),
            normalized_loopback_base(&url).is_some(),
            if version.is_empty() {
                "unknown"
            } else {
                &version
            }
        )
        .map_err(|error| format!("serve status output: {error}"))?;
        writeln!(
            output,
            "failures: {count} (last: {}, backoff remaining: {remaining}s)",
            failed_at
                .map(|epoch| epoch.to_string())
                .unwrap_or_else(|| "none".into())
        )
        .map_err(|error| format!("serve status output: {error}"))?;
        writeln!(
            output,
            "cache: {cache_source} (age: {})",
            cache_age
                .map(|age| age.to_string())
                .unwrap_or_else(|| "missing".into())
        )
        .map_err(|error| format!("serve status output: {error}"))?;
    }
    Ok(if healthy { 0 } else { 1 })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn historical_process_start_uses_its_local_calendar_offset() {
        let months = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        for epoch in [1704067200_i64, 1719792000_i64] {
            let instant = epoch as libc::time_t;
            // SAFETY: localtime_r writes only the initialized output calendar.
            let mut calendar: libc::tm = unsafe { std::mem::zeroed() };
            let result = unsafe { libc::localtime_r(&instant, &mut calendar) };
            assert!(!result.is_null());
            let text = format!(
                "Mon {} {} {:02}:{:02}:{:02} {}",
                months[calendar.tm_mon as usize],
                calendar.tm_mday,
                calendar.tm_hour,
                calendar.tm_min,
                calendar.tm_sec,
                calendar.tm_year + 1900
            );
            assert_eq!(parse_local_epoch("%a %b %e %T %Y", &text).unwrap(), epoch);
        }
        assert!(parse_local_epoch("%a %b %e %T %Y", "Mon Feb 30 00:00:00 2024").is_err());
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("showy-quota-native-test.{}", unique_token()));
            std::fs::create_dir(&path).expect("isolated test directory");
            Self(path)
        }

        fn config(&self) -> Config {
            let cache_dir = self.0.join("cache");
            Config {
                usage_file: cache_dir.join("usage.json"),
                usage_stamp: cache_dir.join("usage.json.updated-at"),
                usage_lock: cache_dir.join("usage.lock"),
                pid_file: cache_dir.join("serve.pid"),
                serve_failure_stamp: cache_dir.join("serve-failed-at"),
                serve_failure_count_file: cache_dir.join("serve-failed-count"),
                cli_failure_stamp: cache_dir.join("cli-failed-at"),
                discovery_failure_stamp: cache_dir.join("discovery-failed-at"),
                provider_failure_dir: cache_dir.join("provider-failures"),
                cache_dir,
                bin: "/usr/bin/false".into(),
                serve_url: String::new(),
                serve_port: "8080".into(),
                serve_refresh_interval: 120,
                serve_start_wait_tenths: 1,
                manage_serve: false,
                lock_wait_tenths: 2,
                refresh_seconds: 120,
                serve_refresh_seconds: 60,
                health_timeout: Duration::from_secs(1),
                usage_timeout: Duration::from_secs(1),
                cli_timeout: 1,
                discovery_timeout: 1,
                serve_failures_before_cli: 3,
                serve_failure_backoff: 60,
                cli_failure_backoff: 120,
                discovery_backoff: 60,
                provider_backoff: 120,
                include_status: true,
                max_usage_json_bytes: MAX_USAGE_JSON_BYTES,
                force_refresh: false,
                serve_only: false,
            }
        }

        fn script(&self, name: &str, body: &str) -> String {
            let path = self.0.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("test command");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("test command permissions");
            path.to_str().expect("test path").to_string()
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn provider_failure_stamps_keep_exit_reason_and_forced_refresh_bypasses_backoff() {
        let dir = TestDir::new();
        let mut config = dir.config();
        config.bin = dir.script("failed-provider", "exit 7");
        assert!(fetch_provider_payload(
            &config,
            "codex",
            FetchMode::Refresh,
            Duration::from_secs(1)
        )
        .is_err());
        let stamp = provider_stamp(&config, "codex");
        let failed = std::fs::read_to_string(&stamp).expect("failure stamp");
        assert_eq!(failed.lines().nth(1), Some("rc=7"));
        config.bin = dir.script(
            "fresh-provider",
            r#"printf '[{"provider":"codex","usage":{"primary":{"usedPercent":25}}}]'"#,
        );
        assert!(fetch_provider_payload(
            &config,
            "codex",
            FetchMode::Refresh,
            Duration::from_secs(1)
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(&stamp).expect("retained failure"),
            failed
        );
        config.force_refresh = true;
        assert!(fetch_provider_payload(
            &config,
            "codex",
            FetchMode::Refresh,
            Duration::from_secs(1)
        )
        .is_ok());
        assert!(!stamp.exists());
        config.bin = dir.script("hung-provider", "sleep 5");
        assert!(fetch_provider_payload(
            &config,
            "codex",
            FetchMode::Refresh,
            Duration::from_millis(20)
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(&stamp)
                .expect("timeout stamp")
                .lines()
                .nth(1),
            Some("rc=124")
        );
    }

    #[test]
    fn corrupt_cache_quarantine_keeps_only_the_newest_three_files() {
        let dir = TestDir::new();
        let config = dir.config();
        ensure_cache_dir(&config).expect("cache directory");
        for index in 0..5 {
            std::fs::write(
                config
                    .cache_dir
                    .join(format!("usage.json.corrupt.000{index}.0")),
                b"bad",
            )
            .expect("old quarantine");
        }
        std::fs::write(&config.usage_file, b"invalid").expect("invalid cache");
        quarantine_invalid_cache(&config);
        let entries: Vec<_> = std::fs::read_dir(&config.cache_dir)
            .expect("cache files")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("usage.json.corrupt.")
            })
            .collect();
        assert_eq!(entries.len(), 3);
        assert!(!config.usage_file.exists());
        assert!(!config.cache_dir.join("usage.json.corrupt.0000.0").exists());
    }

    #[test]
    fn native_timeout_defaults_clamps_and_clock_override_match_shell_contract() {
        const CHILD: &str = "SHOWY_QUOTA_NATIVE_CONFIG_CHILD";
        if let Ok(expected) = std::env::var(CHILD) {
            let config = Config::from_env();
            assert_eq!(
                config.usage_timeout.as_secs(),
                expected.parse::<u64>().expect("expected timeout")
            );
            assert_eq!(config.cli_failure_backoff, 120);
            assert_eq!(config.provider_backoff, 120);
            if std::env::var("SHOWY_QUOTA_NOW_EPOCH").as_deref() == Ok("1234") {
                assert_eq!(now_epoch(), 1234);
                let first = now_seconds();
                std::thread::sleep(Duration::from_millis(10));
                assert!(now_seconds() > first);
                assert!((1234.0..1235.0).contains(&first));
                assert_eq!(now_epoch(), 1234);
            } else {
                assert!(now_epoch() > 1234);
            }
            return;
        }
        for (timeout, expected, epoch) in [
            ("", "30", "1234"),
            ("0", "30", "+1234"),
            ("999999999", "60", "4102444801"),
        ] {
            let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args(["--exact", "native_fetch::tests::native_timeout_defaults_clamps_and_clock_override_match_shell_contract", "--test-threads=1"])
                .env(CHILD, expected)
                .env("SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS", timeout)
                .env("SHOWY_QUOTA_REFRESH_SECONDS", "0")
                .env("SHOWY_QUOTA_CODEXBAR_CLI_FAILURE_BACKOFF_SECONDS", "0")
                .env("SHOWY_QUOTA_PROVIDER_FAILURE_BACKOFF_SECONDS", "0")
                .env("SHOWY_QUOTA_NOW_EPOCH", epoch)
                .output().expect("isolated config test");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

    struct TestEndpoint {
        url: String,
        status: std::sync::Arc<std::sync::atomic::AtomicU16>,
        usage_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl TestEndpoint {
        fn new(status: u16) -> Self {
            use std::sync::atomic::Ordering;
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("isolated endpoint");
            let url = format!(
                "http://{}",
                listener.local_addr().expect("endpoint address")
            );
            listener.set_nonblocking(true).expect("endpoint shutdown");
            let status = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(status));
            let usage_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let worker_status = status.clone();
            let worker_usage = usage_requests.clone();
            let worker_stop = stop.clone();
            let worker = std::thread::spawn(move || {
                while !worker_stop.load(Ordering::SeqCst) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(error) => panic!("fixture accept: {error}"),
                    };
                    // macOS inherits the listener's nonblocking flag. Request
                    // reads must wait for the rest of a fragmented header.
                    stream.set_nonblocking(false).expect("blocking request");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .expect("request timeout");
                    let mut request = Vec::new();
                    let mut buffer = [0; 2048];
                    while request.len() < 8192
                        && !request.windows(4).any(|bytes| bytes == b"\r\n\r\n")
                    {
                        match stream.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(count) => request.extend_from_slice(&buffer[..count]),
                        }
                    }
                    if !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        continue;
                    }
                    if String::from_utf8_lossy(&request).starts_with("GET /usage ") {
                        worker_usage.fetch_add(1, Ordering::SeqCst);
                    }
                    let body = r#"{"status":"ok","version":"1.2.3"}"#;
                    let status = worker_status.load(Ordering::SeqCst);
                    let _ = write!(stream,
                        "HTTP/1.1 {status} fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len());
                }
            });
            Self {
                url,
                status,
                usage_requests,
                stop,
                worker: Some(worker),
            }
        }
    }

    impl Drop for TestEndpoint {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(worker) = self.worker.take() {
                worker.join().expect("endpoint stopped");
            }
        }
    }
    #[test]
    fn endpoint_fixture_waits_for_complete_request_headers() {
        let endpoint = TestEndpoint::new(200);
        let address = endpoint
            .url
            .strip_prefix("http://")
            .expect("fixture address");
        let mut stream = std::net::TcpStream::connect(address).expect("fixture connection");
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .expect("partial request wait");
        stream
            .write_all(b"GET /health HTTP/1.1\r\n")
            .expect("partial headers");
        let mut first = [0; 1];
        let waiting = stream.read(&mut first);
        assert!(
            matches!(&waiting, Err(error) if matches!(
                error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )),
            "the fixture must keep an incomplete request open: {waiting:?}"
        );
        stream
            .write_all(b"Host: fixture\r\n\r\n")
            .expect("complete headers");
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("complete request wait");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("fixture response");
        assert!(response.starts_with("HTTP/1.1 200 "));
        assert!(response.ends_with(r#"{"status":"ok","version":"1.2.3"}"#));
    }

    struct FakeNativeServe {
        dir: TestDir,
        config: Config,
    }

    impl FakeNativeServe {
        fn new() -> Self {
            let dir = TestDir::new();
            let reservation = std::net::TcpListener::bind("127.0.0.1:0").expect("fixture port");
            let port = reservation.local_addr().expect("fixture address").port();
            let config = Self::config(&dir, port);
            // macOS framework launchers replace argv[0] while they start the real
            // interpreter. Bypass that launcher so exec -a preserves the owned
            // fixture identity just as a native CodexBar executable does.
            let python = std::process::Command::new(resolve_codexbar_executable("python3"))
                .args([
                    "-c",
                    "import pathlib, sys, sysconfig; print(\
                     pathlib.Path(sysconfig.get_config_var('BINDIR')).parent / 'Resources/Python.app/Contents/MacOS/Python' \
                     if sys.platform == 'darwin' and sysconfig.get_config_var('PYTHONFRAMEWORK') \
                     else pathlib.Path(sys.executable))",
                ])
                .output()
                .expect("the existing harness requires python3");
            assert!(python.status.success(), "resolve the fixture interpreter");
            let python = PathBuf::from(
                String::from_utf8(python.stdout)
                    .expect("fixture interpreter path")
                    .trim(),
            );
            assert!(python.is_absolute() && python.is_file());
            let binary = PathBuf::from(&config.bin);
            std::fs::write(
                &binary,
                format!(
                    r#"#!/bin/bash
set -eu
printf '%s\n' "$*" >> "$0.called"
case "$1" in
    --version) cat "$0.version" ;;
    config) printf '%s\n' '[{{"provider":"codex","enabled":true}}]' ;;
    usage) printf '%s\n' '[{{"provider":"codex","usage":{{"primary":{{"usedPercent":25}}}}}}]' ;;
    serve)
        shift
        printf '%s\n' "$$" >> "$0.started"
        printf '%s\n' 200 > "$0.health"
        cd "$(dirname "$0")"
        exec -a "$0" '{}' serve "$@"
        ;;
    *) exit 1 ;;
esac
"#,
                    python.display()
                ),
            )
            .expect("fake executable");
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
                .expect("fake executable permissions");
            std::fs::write(dir.0.join("codexbar.version"), "1.2.3\n").expect("fake version");
            std::fs::write(
                dir.0.join("serve"),
                r#"import argparse
import http.server
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--refresh-interval")
args = parser.parse_args()
version = Path("codexbar.version").read_text().strip()
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            status = int(Path("codexbar.health").read_text().strip())
            body = ('{"status":"ok","version":"' + version + '"}').encode()
        else:
            status = 200
            body = b'[{"provider":"codex","usage":{"primary":{"usedPercent":25}}}]'
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", args.port), Handler).serve_forever()
"#,
            )
            .expect("fake endpoint process");
            drop(reservation);
            Self { dir, config }
        }

        fn config(dir: &TestDir, port: u16) -> Config {
            let mut config = dir.config();
            config.bin = dir.0.join("codexbar").to_str().expect("binary path").into();
            config.serve_url = format!("http://127.0.0.1:{port}");
            config.serve_port = port.to_string();
            config.manage_serve = true;
            config.serve_start_wait_tenths = 50;
            config.health_timeout = Duration::from_millis(200);
            ensure_cache_dir(&config).expect("fixture cache");
            config
        }

        fn managed_start(&self) -> u32 {
            match start_managed_serve(&self.config, Duration::from_secs(5))
                .expect("managed startup")
            {
                Some(coordinator::PaneId::Terminal(pid)) => pid,
                _ => panic!("the fake native serve must start"),
            }
        }

        fn spawn(&self, detached: bool) -> std::process::Child {
            use std::os::unix::process::CommandExt;
            let mut command = std::process::Command::new(&self.config.bin);
            command.args(["serve", "--port", &self.config.serve_port]);
            command
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if detached {
                // SAFETY: the fixture child only detaches before exec.
                unsafe {
                    command.pre_exec(|| {
                        if libc::setsid() == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
            let child = command.spawn().expect("isolated serve process");
            let deadline = Instant::now() + Duration::from_secs(5);
            while !health_ok(&self.config) {
                assert!(Instant::now() < deadline, "fixture did not become healthy");
                std::thread::sleep(Duration::from_millis(20));
            }
            write_pid_record(&self.config, child.id()).expect("fixture pid record");
            child
        }

        fn started_pids(&self) -> Vec<u32> {
            std::fs::read_to_string(self.dir.0.join("codexbar.started"))
                .unwrap_or_default()
                .lines()
                .map(|line| line.parse().expect("fixture pid"))
                .collect()
        }

        fn status(&self) -> (i32, serde_json::Value) {
            let mut bytes = Vec::new();
            let exit = serve_status_to(&self.config, true, &mut bytes).expect("status inspection");
            (exit, serde_json::from_slice(&bytes).expect("status JSON"))
        }
    }

    impl Drop for FakeNativeServe {
        fn drop(&mut self) {
            for pid in self.started_pids() {
                // SAFETY: waitpid first proves this is still our unreaped fixture
                // child. Never signal a reaped PID that another process can reuse.
                unsafe {
                    if libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), libc::WNOHANG) == 0 {
                        libc::kill(pid as libc::pid_t, libc::SIGKILL);
                        libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0);
                    }
                }
            }
        }
    }

    #[test]
    fn refresh_recycles_restored_unhealthy_native_owned_serve() {
        let fixture = FakeNativeServe::new();
        let old_pid = fixture.managed_start();
        std::fs::write(fixture.dir.0.join("codexbar.health"), "503\n").expect("unhealthy serve");
        let guard = acquire_lock(&fixture.config).expect("refresh lease");
        refresh_via_coordinator(&fixture.config, &guard).expect("recover restored native serve");
        let replacement = owned_serve_pid(&fixture.config).expect("replacement identity");
        assert_ne!(replacement, old_pid);
        assert_eq!(fixture.started_pids(), vec![old_pid, replacement]);
        assert_eq!(
            read_valid_cache(&fixture.config)
                .expect("recovered cache")
                .source,
            Source::Serve
        );
        assert_eq!(fixture.status().0, 0);
    }

    #[test]
    fn refresh_never_signals_foreign_or_session_owned_serve() {
        for detached in [true, false] {
            let fixture = FakeNativeServe::new();
            let mut original = fixture.spawn(detached);
            if detached {
                std::fs::write(
                    &fixture.config.pid_file,
                    format!("{}:0:codexbar\n", original.id()),
                )
                .expect("foreign start identity");
            }
            assert!(owned_serve_pid(&fixture.config).is_none());
            std::fs::write(fixture.dir.0.join("codexbar.health"), "503\n")
                .expect("unhealthy endpoint");
            let guard = acquire_lock(&fixture.config).expect("refresh lease");
            refresh_via_coordinator(&fixture.config, &guard).expect("isolated fallback");
            assert!(original
                .try_wait()
                .expect("original process state")
                .is_none());
        }
    }

    #[test]
    fn serve_status_preserves_json_contract_and_owned_health_exit() {
        let fixture = FakeNativeServe::new();
        let pid = fixture.managed_start();
        let original_record = std::fs::read(&fixture.config.pid_file).expect("pid record");
        let (exit, status) = fixture.status();
        assert_eq!(exit, 0);
        assert_eq!(status["healthy"], true);
        assert_eq!(
            status["managed"],
            serde_json::json!({
                "pidfileState":"owned", "pid":pid, "alive":true, "owned":true
            })
        );
        assert_eq!(
            status["serve"]["port"].as_u64(),
            fixture.config.serve_port.parse().ok()
        );
        assert_eq!(status["serve"]["version"], "1.2.3");
        std::fs::write(fixture.dir.0.join("codexbar.health"), "503\n").expect("failed health");
        let (exit, status) = fixture.status();
        assert_eq!(exit, 1);
        assert_eq!(status["healthy"], false);
        assert_eq!(status["managed"]["owned"], true);
        assert_eq!(status["serve"]["healthReachable"], false);
        assert_eq!(
            std::fs::read(&fixture.config.pid_file).expect("unchanged record"),
            original_record
        );
        let commands =
            std::fs::read_to_string(fixture.dir.0.join("codexbar.called")).expect("commands");
        assert!(!commands
            .lines()
            .any(|line| line.starts_with("config ") || line.starts_with("usage ")));
    }

    #[test]
    fn serve_status_reports_foreign_and_malformed_records_without_mutation() {
        let endpoint = TestEndpoint::new(200);
        let dir = TestDir::new();
        let mut config = dir.config();
        config.serve_url = endpoint.url.clone();
        let mut bytes = Vec::new();
        assert_eq!(
            serve_status_to(&config, true, &mut bytes).expect("foreign status"),
            1
        );
        let status: serde_json::Value = serde_json::from_slice(&bytes).expect("foreign JSON");
        assert_eq!(status["healthy"], false);
        assert_eq!(
            status["managed"],
            serde_json::json!({
                "pidfileState":"missing", "pid":null, "alive":false, "owned":false
            })
        );
        assert_eq!(
            status["serve"]["port"],
            serve_url_port(&config).expect("URL port")
        );
        assert_eq!(status["serve"]["healthReachable"], true);
        assert!(!config.cache_dir.exists());
        bytes.clear();
        assert_eq!(
            serve_status_to(&config, false, &mut bytes).expect("foreign text status"),
            1
        );
        let text = String::from_utf8(bytes.clone()).expect("status text");
        assert!(text.contains("managed serve: missing"));
        assert!(text.contains("/health: true"));
        assert!(!config.cache_dir.exists());
        ensure_cache_dir(&config).expect("fixture cache");
        for (record, state) in [
            ("not-a-pid-record\n", "malformed"),
            ("2147483647:0:false\n", "stale"),
        ] {
            std::fs::write(&config.pid_file, record).expect("status record");
            std::fs::write(&config.serve_failure_stamp, "1000\n").expect("failure stamp");
            bytes.clear();
            assert_eq!(
                serve_status_to(&config, true, &mut bytes).expect("stale status"),
                1
            );
            let status: serde_json::Value = serde_json::from_slice(&bytes).expect("stale JSON");
            assert_eq!(status["managed"]["pidfileState"], state);
            assert_eq!(
                std::fs::read_to_string(&config.pid_file).expect("record unchanged"),
                record
            );
            assert_eq!(
                std::fs::read_to_string(&config.serve_failure_stamp).expect("stamp unchanged"),
                "1000\n"
            );
        }
        assert_eq!(
            endpoint
                .usage_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[test]
    fn serve_status_rejects_a_recorded_process_that_does_not_own_the_endpoint() {
        let fixture = FakeNativeServe::new();
        let pid = fixture.managed_start();
        let endpoint = TestEndpoint::new(200);
        let mut config = fixture.config.clone();
        config.serve_url = endpoint.url.clone();
        let mut bytes = Vec::new();
        assert_eq!(
            serve_status_to(&config, true, &mut bytes).expect("foreign listener status"),
            1
        );
        let status: serde_json::Value =
            serde_json::from_slice(&bytes).expect("foreign listener JSON");
        assert_eq!(status["managed"]["pid"], pid);
        assert_eq!(status["managed"]["alive"], true);
        assert_eq!(status["managed"]["owned"], false);
        assert_eq!(status["healthy"], false);
        assert!(status["managed"]["reason"]
            .as_str()
            .expect("ownership reason")
            .contains("does not own the configured port"));
    }

    #[test]
    fn restart_refuses_healthy_foreign_or_session_owned_responder() {
        let endpoint = TestEndpoint::new(200);
        let fixture = FakeNativeServe::new();
        let mut config = fixture.config.clone();
        config.serve_url = endpoint.url.clone();
        assert!(restart_owned_serve(&config).is_err());
        assert!(fixture.started_pids().is_empty());
        assert!(!config.pid_file.exists());
        let mut session = fixture.spawn(false);
        assert!(owned_serve_pid(&fixture.config).is_none());
        assert!(restart_owned_serve(&fixture.config).is_err());
        assert!(session.try_wait().expect("session process state").is_none());
        assert_eq!(fixture.started_pids(), vec![session.id()]);
    }

    #[test]
    fn restart_requires_a_verified_new_owned_listener() {
        let fixture = FakeNativeServe::new();
        let original = fixture.managed_start();
        restart_owned_serve(&fixture.config).expect("owned restart");
        let replacement = owned_serve_pid(&fixture.config).expect("new owned identity");
        assert_ne!(replacement, original);
        assert_eq!(fixture.started_pids(), vec![original, replacement]);
        assert!(owned_serve_owns_port(&fixture.config, replacement));
        assert_eq!(fixture.status().0, 0);
    }

    #[test]
    fn restart_does_not_claim_a_foreign_endpoint_that_becomes_healthy_during_startup() {
        let endpoint = TestEndpoint::new(503);
        let mut fixture = FakeNativeServe::new();
        fixture.config.serve_url = endpoint.url.clone();
        fixture.config.serve_port = serve_url_port(&fixture.config)
            .expect("endpoint port")
            .to_string();
        fixture.config.serve_start_wait_tenths = 10;
        std::fs::write(
            fixture.dir.0.join("serve"),
            "import time\nwhile True: time.sleep(1)\n",
        )
        .expect("live process without a listener");
        let started = fixture.dir.0.join("codexbar.started");
        let status = endpoint.status.clone();
        let health_race = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !started.exists() {
                assert!(Instant::now() < deadline, "startup was not attempted");
                std::thread::sleep(Duration::from_millis(5));
            }
            status.store(200, std::sync::atomic::Ordering::SeqCst);
        });
        let result = restart_owned_serve(&fixture.config);
        health_race.join().expect("foreign health transition");
        assert!(result.is_err());
        assert!(health_ok(&fixture.config));
        assert!(!fixture.config.pid_file.exists());
        assert_eq!(fixture.started_pids().len(), 1);
    }

    #[test]
    fn bare_codexbar_resolution_enables_version_gate_and_managed_start() {
        const CHILD: &str = "SHOWY_QUOTA_TEST_BARE_CODEXBAR_CHILD";
        if let Some(path) = std::env::var_os(CHILD) {
            let dir = TestDir(PathBuf::from(path));
            let port = std::env::var("SHOWY_QUOTA_TEST_BARE_CODEXBAR_PORT")
                .expect("child port")
                .parse()
                .expect("numeric child port");
            let mut config = FakeNativeServe::config(&dir, port);
            config.bin = "codexbar".into();
            let fixture = FakeNativeServe { dir, config };
            assert_eq!(
                resolve_codexbar_executable("codexbar"),
                std::fs::canonicalize(fixture.dir.0.join("codexbar"))
                    .expect("canonical executable")
            );
            assert_eq!(ondisk_version(&fixture.config).as_deref(), Some("1.2.3"));
            let old_pid = fixture.managed_start();
            std::fs::write(fixture.dir.0.join("codexbar.version"), "2.2.2\n")
                .expect("new bundle version");
            let guard = acquire_lock(&fixture.config).expect("version refresh lease");
            refresh_via_coordinator(&fixture.config, &guard).expect("version-gated recycle");
            let replacement = owned_serve_pid(&fixture.config).expect("replacement identity");
            assert_ne!(replacement, old_pid);
            assert_eq!(fixture.started_pids(), vec![old_pid, replacement]);
            assert_eq!(
                serve_running_version(&fixture.config).as_deref(),
                Some("2.2.2")
            );
            return;
        }
        let fixture = FakeNativeServe::new();
        let blocked = fixture.dir.0.join("blocked");
        let linked = fixture.dir.0.join("linked");
        std::fs::create_dir(&blocked).expect("nonexecutable PATH entry");
        std::fs::create_dir(&linked).expect("symlink PATH entry");
        std::fs::write(blocked.join("codexbar"), "not executable")
            .expect("nonexecutable candidate");
        std::os::unix::fs::symlink(fixture.dir.0.join("codexbar"), linked.join("codexbar"))
            .expect("same-basename executable symlink");
        let mut paths = vec![blocked, linked];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let result = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .arg("bare_codexbar_resolution_enables_version_gate_and_managed_start")
            .arg("--nocapture")
            .env(CHILD, &fixture.dir.0)
            .env(
                "SHOWY_QUOTA_TEST_BARE_CODEXBAR_PORT",
                &fixture.config.serve_port,
            )
            .env(
                "PATH",
                std::env::join_paths(paths).expect("isolated child PATH"),
            )
            .output()
            .expect("isolated PATH test process");
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn ordinary_managed_start_survives_without_optional_lsof() {
        const CHILD: &str = "SHOWY_QUOTA_TEST_NO_LSOF_CHILD";
        if std::env::var_os(CHILD).is_some() {
            assert!(lsof_executable().is_none());
            let fixture = FakeNativeServe::new();
            let pid = fixture.managed_start();
            assert_eq!(owned_serve_pid(&fixture.config), Some(pid));
            assert!(health_ok(&fixture.config));
            assert_eq!(serve_port_ownership(&fixture.config, pid), None);
            let (exit, status) = fixture.status();
            assert_eq!(exit, 1);
            assert_eq!(status["healthy"], false);
            assert!(status["managed"]["reason"]
                .as_str()
                .expect("missing tool reason")
                .contains("lsof not found"));
            assert!(restart_owned_serve(&fixture.config).is_err());
            assert_eq!(owned_serve_pid(&fixture.config), Some(pid));
            assert_eq!(fixture.started_pids(), vec![pid]);
            return;
        }
        let dir = TestDir::new();
        for name in ["ps", "cat", "dirname", "python3"] {
            let executable = resolve_codexbar_executable(name);
            assert!(executable.is_absolute(), "existing test dependency: {name}");
            std::os::unix::fs::symlink(executable, dir.0.join(name)).expect("restricted PATH tool");
        }
        let result = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .arg("ordinary_managed_start_survives_without_optional_lsof")
            .arg("--nocapture")
            .env(CHILD, "1")
            .env("PATH", &dir.0)
            .output()
            .expect("isolated optional-tool test process");
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn run_isolated_terminal_refresh_test(name: &str) -> bool {
        const CHILD: &str = "SHOWY_QUOTA_NATIVE_TERMINAL_REFRESH_CHILD";
        if std::env::var(CHILD).as_deref() == Ok(name) {
            return false;
        }
        let output = {
            // BoundedSignals serializes commands process-wide. Run the timed
            // refresh in a child so another test's guard cannot consume its
            // five-second assertion. Holding our guard proves that separation.
            #[cfg(unix)]
            let _parent_commands = BOUNDED_SIGNAL_LOCK.lock().expect("parent command guard");
            std::process::Command::new(std::env::current_exe().expect("test executable"))
                .args([
                    "--exact",
                    &format!("native_fetch::tests::{name}"),
                    "--nocapture",
                ])
                .env(CHILD, name)
                .output()
                .expect("isolated terminal refresh test")
        };
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    #[test]
    fn failed_discovery_refresh_finishes_without_idle_wait() {
        if run_isolated_terminal_refresh_test("failed_discovery_refresh_finishes_without_idle_wait")
        {
            return;
        }
        let dir = TestDir::new();
        let mut config = dir.config();
        config.bin = dir.script(
            "failed-discovery",
            "printf '%s\\n' \"$*\" >> \"$0.called\"\nexit 1",
        );
        let guard = acquire_lock(&config).expect("failed refresh lease");
        let started = Instant::now();
        assert!(refresh_via_coordinator(&config, &guard).is_err());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "idle timer must not retain the lease"
        );
        assert!(config.cli_failure_stamp.exists());
        drop(guard);
        assert!(!lock_dir(&config).exists());
        let calls =
            std::fs::read_to_string(format!("{}.called", config.bin)).expect("discovery calls");
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            vec!["config providers --format json --pretty"]
        );
    }

    #[test]
    fn failed_provider_refresh_finishes_after_every_terminal_provider() {
        if run_isolated_terminal_refresh_test(
            "failed_provider_refresh_finishes_after_every_terminal_provider",
        ) {
            return;
        }
        let dir = TestDir::new();
        let mut config = dir.config();
        config.bin = dir.script(
            "failed-providers",
            r#"printf '%s\n' "$*" >> "$0.called"
if [ "$1" = config ]; then
    printf '%s\n' '[{"provider":"codex","enabled":true},{"provider":"claude","enabled":true}]'
else
    exit 1
fi"#,
        );
        let guard = acquire_lock(&config).expect("provider failure lease");
        let started = Instant::now();
        assert!(refresh_via_coordinator(&config, &guard).is_err());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "completed failure must not idle for 180s"
        );
        let calls =
            std::fs::read_to_string(format!("{}.called", config.bin)).expect("provider calls");
        assert_eq!(calls.lines().count(), 3);
        assert!(calls.contains("usage --provider codex "));
        assert!(calls.contains("usage --provider claude "));
        assert!(provider_stamp(&config, "codex").exists());
        assert!(provider_stamp(&config, "claude").exists());
        drop(guard);
        assert!(!lock_dir(&config).exists());
    }

    #[test]
    fn pinned_epoch_does_not_stop_managed_start_or_cli_fallback_timers() {
        const CHILD: &str = "SHOWY_QUOTA_NATIVE_PINNED_TIMER_CHILD";
        if let Ok(mode) = std::env::var(CHILD) {
            let fixture = FakeNativeServe::new();
            let mut config = fixture.config.clone();
            let expected = if mode == "serve" {
                Source::Serve
            } else {
                config.bin = fixture.dir.script(
                    "failed-start",
                    r#"case "$1" in
    serve) exit 1 ;;
    config) printf '[{"provider":"codex","enabled":true}]' ;;
    usage) printf '[{"provider":"codex","usage":{"primary":{"usedPercent":25}}}]' ;;
    *) exit 1 ;;
esac"#,
                );
                Source::Cli
            };
            let guard = acquire_lock(&config).expect("pinned timer lease");
            refresh_via_coordinator(&config, &guard).expect("pinned refresh completes");
            let cache = read_valid_cache(&config).expect("pinned cache");
            assert_eq!(cache.source, expected);
            assert_eq!(cache.provider_meta["codex"].updated_at, 1234);
            assert_eq!(now_epoch(), 1234);
            let calls = std::fs::read_to_string(format!("{}.called", fixture.config.bin));
            if mode == "serve" {
                assert!(calls.expect("managed commands").contains("serve --port"));
            }
            return;
        }
        for mode in ["serve", "fallback"] {
            let argv = vec![
                "/usr/bin/env".into(),
                format!("{CHILD}={mode}"),
                "SHOWY_QUOTA_NOW_EPOCH=1234".into(),
                std::env::current_exe().expect("test executable").to_string_lossy().into_owned(),
                "--exact".into(),
                "native_fetch::tests::pinned_epoch_does_not_stop_managed_start_or_cli_fallback_timers".into(),
                "--test-threads=1".into(),
            ];
            assert!(
                matches!(
                    run_bounded(&argv, Duration::from_secs(8), 65536)
                        .expect("bounded pinned regression"),
                    RunOutcome::Ok(0, _)
                ),
                "pinned {mode} cycle must finish before the host deadline"
            );
        }
    }

    #[test]
    fn malformed_cached_sibling_cannot_discard_a_fresh_provider() {
        let dir = TestDir::new();
        let mut config = dir.config();
        ensure_cache_dir(&config).expect("cache directory");
        let previous = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":90}}},
                            {"provider":"gemini","usage":3}]"#;
        std::fs::write(&config.usage_file, previous).expect("mixed-validity cache");
        assert_eq!(
            read_valid_cache(&config)
                .expect("tolerant read")
                .providers
                .len(),
            2
        );
        config.bin = dir.script(
            "mixed-refresh",
            r#"case "$1" in
    config) printf '[{"provider":"codex","enabled":true},{"provider":"gemini","enabled":true}]' ;;
    usage)
        if [ "$3" = codex ]; then
            printf '[{"provider":"codex","usage":{"primary":{"usedPercent":25}}}]'
        else
            exit 7
        fi ;;
    *) exit 1 ;;
esac"#,
        );
        let guard = acquire_lock(&config).expect("refresh lease");
        refresh_via_coordinator(&config, &guard).expect("fresh codex survives gemini failure");
        let fresh = read_valid_cache(&config).expect("fresh cache");
        assert_eq!(fresh.source, Source::Cli);
        assert_eq!(fresh.providers.len(), 1);
        assert_eq!(fresh.providers[0]["provider"], "codex");
        assert_eq!(fresh.providers[0]["usage"]["primary"]["usedPercent"], 25);
        assert!(provider_stamp(&config, "gemini").exists());
    }

    #[test]
    fn deferred_managed_start_failure_reaches_every_fallback_provider() {
        let fixture = FakeNativeServe::new();
        let mut config = fixture.config.clone();
        config.bin = fixture.dir.script("failed-start", r#"printf '%s\n' "$*" >> "$0.called"
case "$1" in
    serve) exit 1 ;;
    config) printf '%s\n' '[{"provider":"codex","enabled":true},{"provider":"claude","enabled":true}]' ;;
    usage) printf '[{"provider":"%s","usage":{"primary":{"usedPercent":25}}}]\n' "$3" ;;
    *) exit 1 ;;
esac"#);
        let guard = acquire_lock(&config).expect("startup fallback lease");
        let fetched = fetch_once(&config, &guard, FetchMode::Refresh).expect("deferred fallback");
        assert_eq!(fetched.source, Source::Cli);
        let providers =
            validated_array(&fetched.payload, MAX_USAGE_JSON_BYTES).expect("fallback records");
        assert_eq!(providers.len(), 2);
        let calls =
            std::fs::read_to_string(format!("{}.called", config.bin)).expect("fallback calls");
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.starts_with("serve "))
                .count(),
            1
        );
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.starts_with("usage "))
                .count(),
            2
        );
    }

    #[test]
    fn publication_validation_rejects_unusable_json_and_accepts_empty_inventory() {
        for bytes in [
            &b"null"[..], &b"1"[..], &b"{}"[..], &b"[{}]"[..],
            &br#"[{"provider":"../escape"}]"#[..],
            &br#"[{"provider":"codex"},{"provider":"codex"}]"#[..],
            &br#"{"schema":"showy-quota/cache@2","source":"serve","providers":[]}"#[..],
            &br#"{"schema":"showy-quota/cache@1","source":"serve","providers":[],"providerMeta":{}}"#[..],
            &br#"{"schema":"showy-quota/cache@2","source":"serve","providers":[{"provider":"codex"}],"providerMeta":{"codex":{"source":"serve","updatedAt":-1}}}"#[..],
        ] {
            assert!(validate_publication_cache(bytes, MAX_USAGE_JSON_BYTES).is_err(), "{bytes:?}");
        }
        assert!(validate_publication_cache(b"[]", MAX_USAGE_JSON_BYTES)
            .expect("empty array")
            .providers
            .is_empty());
        let empty =
            br#"{"schema":"showy-quota/cache@2","source":"cli","providers":[],"providerMeta":{}}"#;
        let cache =
            validate_publication_cache(empty, MAX_USAGE_JSON_BYTES).expect("empty envelope");
        assert_eq!(cache.source, Source::Cli);
        assert!(cache.providers.is_empty());
    }

    #[test]
    fn cache_reader_accepts_legacy_envelopes_and_keeps_invalid_siblings() {
        let providers = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":25}}},
                             {"provider":"bad/id","usage":{"primary":{"usedPercent":5}}},
                             {"provider":"gemini","usage":3}]"#;
        let bare = validate_cache(providers, MAX_USAGE_JSON_BYTES).expect("mixed legacy array");
        assert_eq!(bare.providers.len(), 3);
        assert_eq!(bare.source, Source::Unknown);
        let envelope = serde_json::json!({
            "schema": "showy-quota/cache@1",
            "source": "cli",
            "providers": bare.providers,
        });
        let bytes = serde_json::to_vec(&envelope).expect("legacy envelope");
        let cache = validate_cache(&bytes, MAX_USAGE_JSON_BYTES).expect("read legacy envelope");
        assert_eq!(cache.providers.len(), 3);
        assert_eq!(cache.providers[1]["provider"], "bad/id");
        assert_eq!(cache.providers[2]["usage"], 3);
        assert_eq!(cache.source, Source::Cli);
        assert!(cache.provider_meta.is_empty());
        let payload_len = serde_json::to_vec(&envelope["providers"])
            .expect("payload")
            .len();
        assert!(validate_cache(&bytes, payload_len).is_ok());
        assert!(validate_cache(&bytes, payload_len - 1).is_err());
        assert!(validate_publication_cache(&bytes, MAX_USAGE_JSON_BYTES).is_err());
    }

    #[test]
    fn cache_reader_rejects_arrays_without_any_usable_record() {
        for bytes in [
            &b"null"[..],
            &b"1"[..],
            &b"{}"[..],
            &b"[{}]"[..],
            &br#"[{"provider":"../escape"}]"#[..],
            &br#"[{"provider":"codex","usage":3}]"#[..],
            &br#"[{"provider":"codex","status":{"url":{}}}]"#[..],
            &br#"[{"provider":"codex","usage":{"primary":{"remainingPercent":"42"}}}]"#[..],
            &br#"[{"provider":"codex","usage":{"extraRateWindows":null}}]"#[..],
        ] {
            assert!(
                validate_cache(bytes, MAX_USAGE_JSON_BYTES).is_err(),
                "{bytes:?}"
            );
        }
        for bytes in [
            &b"[]"[..],
            &br#"{"schema":"showy-quota/cache@1","source":"serve","providers":[]}"#[..],
            &br#"{"schema":"showy-quota/cache@2","source":"cli","providers":[]}"#[..],
        ] {
            assert!(validate_cache(bytes, MAX_USAGE_JSON_BYTES)
                .expect("empty inventory")
                .providers
                .is_empty());
        }
    }

    #[test]
    fn cache_reader_uses_valid_metadata_without_requiring_a_complete_map() {
        let bytes = br#"{
            "schema":"showy-quota/cache@2","source":"serve",
            "providers":[{"provider":"codex"},{"provider":"claude"},{"provider":"gemini"}],
            "providerMeta":{
                "codex":{"source":"cli","updatedAt":1000},
                "claude":{"source":"serve","updatedAt":-1},
                "gemini":{"source":"unexpected","updatedAt":2000},
                "../escape":{"source":"serve","updatedAt":3000}
            }
        }"#;
        let cache = validate_cache(bytes, MAX_USAGE_JSON_BYTES).expect("read partial metadata");
        assert_eq!(cache.provider_meta.len(), 2);
        assert_eq!(cache.provider_meta["codex"].updated_at, 1000);
        assert_eq!(cache.provider_meta["codex"].source, Source::Cli);
        assert_eq!(cache.provider_meta["gemini"].source, Source::Unknown);
        assert!(validate_publication_cache(bytes, MAX_USAGE_JSON_BYTES).is_err());
    }

    #[test]
    fn invalid_cache_cannot_shortcut_refresh_or_escape_emission() {
        let dir = TestDir::new();
        let config = dir.config();
        ensure_cache_dir(&config).expect("cache directory");
        std::fs::write(&config.usage_file, b"{}").expect("invalid cache");
        assert!(!cache_is_valid(&config));
        assert!(emit_cache(&config).is_err());
        quarantine_invalid_cache(&config);
        assert!(!config.usage_file.exists());
    }

    #[test]
    fn provider_transport_keeps_array_and_rejects_mismatched_or_duplicate_records() {
        let payload = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":25}}}]"#;
        validate_provider_payload(payload, "codex", MAX_USAGE_JSON_BYTES).expect("provider array");
        for invalid in [
            &br#"{"provider":"codex"}"#[..],
            &br#"[{"provider":"claude"}]"#[..],
            &br#"[{"provider":"codex"},{"provider":"codex"}]"#[..],
            &br#"[{"provider":"codex"},{"provider":"-bad"}]"#[..],
            &br#"[{"provider":"codex","status":{"url":{}}}]"#[..],
            &br#"[{"provider":"codex","usage":{"primary":{"remainingPercent":"42"}}}]"#[..],
        ] {
            assert!(validate_provider_payload(invalid, "codex", MAX_USAGE_JSON_BYTES).is_err());
        }
        assert!(validate_provider_payload(payload, "codex", payload.len() - 1).is_err());
    }

    #[test]
    fn publication_preserves_carried_measurement_source_and_age() {
        let dir = TestDir::new();
        let config = dir.config();
        let guard = acquire_lock(&config).expect("lease");
        let old = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":10}}},
                       {"provider":"claude","usage":{"primary":{"usedPercent":30}}}]"#
            .to_vec();
        let mut state = State::default();
        state.set_time(2_000.0);
        assert!(state.restore_payload(old, Source::Serve, 1_000.0));
        state.source = Source::Cli;
        state.serve_url.clear();
        state.manage_serve = false;
        state.cli_fallback = CliFallback::Degraded;
        state.permissions_granted = true;
        state.discovered_providers = vec!["codex".into(), "claude".into()];
        state.discovered_providers_at = Some(2_000.0);
        state.render_config.providers.clear();
        state.render_config.providers_exclude.clear();
        state.refresh();
        for effect in state.take_effects() {
            if let Effect::FetchProvider { context, .. } = effect {
                let provider = context["showy-quota-provider"].clone();
                let (exit, payload) = if provider == "codex" {
                    let bytes = br#"[{"provider":"codex","usage":{"primary":{"usedPercent":75}}}]"#
                        .to_vec();
                    validate_provider_payload(&bytes, &provider, MAX_USAGE_JSON_BYTES)
                        .expect("fresh array");
                    (Some(0), bytes)
                } else {
                    (Some(1), Vec::new())
                };
                state.update(Event::RunCommandResult(exit, payload, Vec::new(), context));
            }
        }
        let fetched =
            completed_fetch(&mut state, &config, FetchMode::Refresh).expect("completed cycle");
        publish_payload(&config, &guard, fetched).expect("publication");
        let cache = read_valid_cache(&config).expect("published envelope");
        assert_eq!(cache.source, Source::Cli);
        assert_eq!(cache.providers[0]["provider"], "codex");
        assert_eq!(cache.providers[0]["usage"]["primary"]["usedPercent"], 75);
        assert_eq!(cache.providers[1]["provider"], "claude");
        assert_eq!(cache.providers[1]["usage"]["primary"]["usedPercent"], 30);
        assert_eq!(
            cache.provider_meta["codex"],
            Measurement {
                source: Source::Cli,
                updated_at: 2_000
            }
        );
        assert_eq!(
            cache.provider_meta["claude"],
            Measurement {
                source: Source::Serve,
                updated_at: 1_000
            }
        );
        let metadata: Vec<_> = cache
            .provider_meta
            .into_iter()
            .map(
                |(provider, measurement)| showy_quota_zellij_core::codexbar::ProviderCacheMeta {
                    provider,
                    source: source_name(measurement.source).into(),
                    updated_at: Some(measurement.updated_at),
                },
            )
            .collect();
        let freshness = showy_quota_zellij_core::cache::freshness_from_parts(
            Some(2_000),
            2_100,
            120,
            "cli".into(),
            None,
            &metadata,
        );
        let stale = freshness.stale_providers();
        assert_eq!(stale, vec!["claude"]);
        assert!(freshness.degraded_cli);
    }

    #[test]
    fn incomplete_lock_claim_gets_grace_and_live_claim_keeps_its_fence() {
        let dir = TestDir::new();
        let config = dir.config();
        std::fs::create_dir_all(&config.cache_dir).expect("lock parent");
        std::fs::create_dir(lock_dir(&config)).expect("incomplete claim");
        assert!(try_acquire_lock(&config).expect("grace").is_none());
        assert!(lock_dir(&config).exists());
        std::fs::remove_dir(lock_dir(&config)).expect("remove own incomplete fixture");
        let guard = acquire_lock(&config).expect("lease");
        assert!(try_acquire_lock(&config).expect("contender").is_none());
        assert!(lease_still_owned(&guard));
        drop(guard);
        assert!(!lock_dir(&config).exists());
        assert!(
            config.usage_lock.exists(),
            "the persistent fence must never be unlinked"
        );
    }

    #[test]
    fn replacement_claim_survives_old_owner_drop_and_blocks_old_publication() {
        let dir = TestDir::new();
        let config = dir.config();
        let guard = acquire_lock(&config).expect("old lease");
        let detached = dir.0.join("old-claim");
        std::fs::rename(lock_dir(&config), &detached).expect("detach old fixture claim");
        std::fs::create_dir(lock_dir(&config)).expect("replacement fixture claim");
        std::fs::write(lock_dir(&config).join("owner.token"), b"replacement")
            .expect("replacement token");
        std::fs::write(
            lock_dir(&config).join("owner.pid"),
            std::process::id().to_string(),
        )
        .expect("replacement pid");
        let original = br#"[{"provider":"codex"}]"#;
        std::fs::write(&config.usage_file, original).expect("existing cache");
        assert!(publish_payload(
            &config,
            &guard,
            FetchResult {
                payload: b"[]".to_vec(),
                source: Source::Cli,
                provider_meta: BTreeMap::new(),
            }
        )
        .is_err());
        assert_eq!(
            std::fs::read(&config.usage_file).expect("cache unchanged"),
            original
        );
        drop(guard);
        assert_eq!(
            std::fs::read_to_string(lock_dir(&config).join("owner.token"))
                .expect("replacement survives"),
            "replacement"
        );
    }

    #[test]
    fn dead_owner_recovery_claims_a_new_token_and_releases_it() {
        let dir = TestDir::new();
        let config = dir.config();
        std::fs::create_dir_all(lock_dir(&config)).expect("dead claim");
        std::fs::write(lock_dir(&config).join("owner.pid"), i32::MAX.to_string())
            .expect("dead pid");
        std::fs::write(lock_dir(&config).join("owner.token"), b"dead-token").expect("old token");
        let guard = acquire_lock(&config).expect("recovered lease");
        assert_ne!(guard.token, "dead-token");
        assert!(lease_still_owned(&guard));
        drop(guard);
        assert!(!lock_dir(&config).exists());
    }

    #[test]
    fn bounded_capture_accepts_the_limit_and_reaps_an_excess_writer() {
        let exact = vec!["/usr/bin/printf".into(), "%64s".into(), "x".into()];
        match run_bounded(&exact, Duration::from_secs(2), 64).expect("exact capture") {
            RunOutcome::Ok(0, bytes) => {
                assert_eq!(bytes, format!("{:>64}", "x").into_bytes());
            }
            _ => panic!("exact-size output must succeed"),
        }
        let dir = TestDir::new();
        let binary = dir.script(
            "noisy",
            "printf '%s' \"$$\" > \"$0.pid\"\nexec /usr/bin/yes",
        );
        assert!(matches!(
            run_bounded(std::slice::from_ref(&binary), Duration::from_secs(2), 64)
                .expect("bounded noisy command"),
            RunOutcome::TooLarge
        ));
        let pid: u32 = std::fs::read_to_string(format!("{binary}.pid"))
            .expect("writer pid")
            .parse()
            .expect("numeric pid");
        assert!(!owner_alive(pid), "the cap must stop and reap the writer");
    }

    #[test]
    fn recording_requires_lease_and_releases_it_on_clean_install_error() {
        let dir = TestDir::new();
        let mut config = dir.config();
        config.bin = dir.script(
            "capture-failure",
            "printf '%s\\n' \"$*\" >> \"$0.called\"\nexit 1",
        );
        let fixtures = dir.0.join("fixtures");
        let guard = acquire_lock(&config).expect("clean-install lease");
        let error = record("occupied", &fixtures, false, &config).expect_err("occupied lease");
        assert!(error.contains("another fetch"));
        assert!(!PathBuf::from(format!("{}.called", config.bin)).exists());
        drop(guard);
        assert!(record("failed", &fixtures, false, &config).is_err());
        assert!(!lock_dir(&config).exists());
        assert!(!fixtures.join("failed.json").exists());
        let commands =
            std::fs::read_to_string(format!("{}.called", config.bin)).expect("fresh acquisition");
        assert_eq!(
            commands.lines().next(),
            Some("config providers --format json --pretty")
        );
    }

    #[test]
    fn failed_fresh_record_acquisition_never_reads_old_cache_or_changes_failure_stamps() {
        let dir = TestDir::new();
        let mut config = dir.config();
        config.bin = dir.script(
            "capture-failure",
            "printf '%s\\n' \"$*\" >> \"$0.called\"\nexit 1",
        );
        ensure_cache_dir(&config).expect("cache");
        let old = br#"{"schema":"showy-quota/cache@2","source":"cli","providers":[{"provider":"codex","usage":{"primary":{"usedPercent":42}}}],"providerMeta":{"codex":{"source":"cli","updatedAt":1000}}}"#;
        std::fs::write(&config.usage_file, old).expect("old cache");
        let stamp_bytes = format!("{}\n", now_epoch());
        let stamps = [
            config.discovery_failure_stamp.clone(),
            config.cli_failure_stamp.clone(),
            config.serve_failure_stamp.clone(),
            provider_stamp(&config, "codex"),
        ];
        for path in &stamps {
            std::fs::create_dir_all(path.parent().expect("stamp parent")).expect("stamp directory");
            std::fs::write(path, &stamp_bytes).expect("active failure stamp");
        }
        assert!(fetch_provider_payload(
            &config,
            "codex",
            FetchMode::Record,
            Duration::from_secs(1)
        )
        .is_err());
        let fixtures = dir.0.join("fixtures");
        assert!(record("failed", &fixtures, false, &config).is_err());
        assert!(!fixtures.join("failed.json").exists());
        assert_eq!(
            std::fs::read(&config.usage_file).expect("old cache preserved"),
            old
        );
        for path in &stamps {
            assert_eq!(
                std::fs::read_to_string(path).expect("stamp preserved"),
                stamp_bytes
            );
        }
        assert!(!lock_dir(&config).exists());
        let commands =
            std::fs::read_to_string(format!("{}.called", config.bin)).expect("fresh commands");
        let mut commands = commands.lines();
        assert_eq!(
            commands.next(),
            Some("usage --provider codex --format json --pretty --status")
        );
        assert_eq!(
            commands.next(),
            Some("config providers --format json --pretty")
        );
    }

    #[test]
    fn native_url_guard_rejects_invalid_authorities_before_requests() {
        let dir = TestDir::new();
        let mut config = dir.config();
        config.serve_url = "http://127.0.0.1:8080@invalid.test".into();
        config.manage_serve = true;
        let state = coordinator_config(&config);
        assert!(state.serve_url.is_empty());
        assert!(!state.manage_serve);
        for url in [
            "http://invalid.test:8080/usage",
            "http://127.0.0.1:8080/path/usage",
            "http://127.0.0.1:8080@invalid.test/health",
        ] {
            assert_eq!(
                http_get(url, Duration::from_secs(1), 64).expect_err("invalid authority"),
                "non-loopback serve URL refused"
            );
        }
    }

    #[test]
    fn redirects_never_reach_another_loopback_endpoint() {
        let target = std::net::TcpListener::bind("127.0.0.1:0").expect("redirect target");
        target.set_nonblocking(true).expect("target probe");
        let server = std::net::TcpListener::bind("127.0.0.1:0").expect("serve fixture");
        let url = format!(
            "http://{}/usage",
            server.local_addr().expect("serve address")
        );
        let location = format!(
            "http://{}/usage",
            target.local_addr().expect("target address")
        );
        let responder = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().expect("initial serve request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("request deadline");
            let mut request = [0; 1024];
            let count = stream.read(&mut request).expect("initial request");
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /usage "));
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("redirect response");
        });
        let result = http_get(&url, Duration::from_secs(2), 64);
        responder.join().expect("serve fixture finished");
        assert!(!matches!(result, Ok((200, _))));
        match target.accept() {
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock),
            Ok(_) => panic!("HTTP redirects must not reach another endpoint"),
        }
    }

    #[test]
    fn owned_command_identity_rejects_foreign_argv_and_wrong_ports() {
        assert!(command_matches_owned(
            "/opt/bin/codexbar serve --port 8080",
            "codexbar",
            "8080"
        ));
        assert!(command_matches_owned(
            "/opt/bin/codexbar serve --port=8080",
            "codexbar",
            "8080"
        ));
        assert!(command_matches_owned(
            "/bin/bash /opt/bin/codexbar serve --port 8080",
            "codexbar",
            "8080"
        ));
        assert!(!command_matches_owned(
            "/bin/bash -c /opt/bin/codexbar serve --port 8080",
            "codexbar",
            "8080"
        ));
        assert!(!command_matches_owned(
            "sh -c codexbar serve --port 8080",
            "codexbar",
            "8080"
        ));
        assert!(!command_matches_owned(
            "/opt/bin/foreign codexbar serve --port 8080",
            "codexbar",
            "8080"
        ));
        assert!(!command_matches_owned(
            "/opt/bin/codexbar usage --port 8080",
            "codexbar",
            "8080"
        ));
        assert!(!command_matches_owned(
            "/opt/bin/codexbar serve --port 9090",
            "codexbar",
            "8080"
        ));
    }

    #[test]
    fn cold_cache_contender_waits_for_holder_publication() {
        let dir = TestDir::new();
        let mut config = dir.config();
        config.lock_wait_tenths = 10;
        let guard = acquire_lock(&config).expect("holder lease");
        let writer_config = config.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            publish_payload(
                &writer_config,
                &guard,
                FetchResult {
                    payload: b"[]".to_vec(),
                    source: Source::Cli,
                    provider_meta: BTreeMap::new(),
                },
            )
            .expect("holder publication");
        });
        assert!(acquire_refresh_lock(&config, false)
            .expect("cold contention")
            .is_none());
        writer.join().expect("holder finished");
        assert!(cache_is_valid(&config));
    }

    #[test]
    fn ordinary_contender_returns_valid_stale_cache_without_waiting() {
        let dir = TestDir::new();
        let mut config = dir.config();
        config.lock_wait_tenths = 20;
        config.refresh_seconds = 0;
        let guard = acquire_lock(&config).expect("holder lease");
        std::fs::write(&config.usage_file, b"[]").expect("valid stale cache");
        let started = Instant::now();
        assert!(acquire_refresh_lock(&config, false)
            .expect("ordinary contention")
            .is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(lease_still_owned(&guard));
    }

    #[test]
    fn forced_contender_waits_for_changed_generation_before_stale_fallback() {
        let dir = TestDir::new();
        let config = dir.config();
        let guard = acquire_lock(&config).expect("holder lease");
        std::fs::write(&config.usage_file, b"[]").expect("existing cache");
        std::fs::write(&config.usage_stamp, b"unchanged").expect("existing generation");
        let previous = cache_generation(&config);
        let started = Instant::now();
        assert!(wait_for_cache(&config, Some(&previous)).is_err());
        assert!(started.elapsed() >= Duration::from_millis(200));
        let started = Instant::now();
        assert!(acquire_refresh_lock(&config, true)
            .expect("forced contention")
            .is_none());
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(cache_is_valid(&config), "timeout retains the valid cache");
        assert!(lease_still_owned(&guard));
    }

    #[test]
    fn forced_contender_accepts_new_payload_before_stamp_changes() {
        let dir = TestDir::new();
        let mut config = dir.config();
        config.lock_wait_tenths = 10;
        let guard = acquire_lock(&config).expect("holder lease");
        std::fs::write(&config.usage_file, b"[]").expect("existing cache");
        std::fs::write(&config.usage_stamp, b"old-stamp").expect("existing stamp");
        let previous = cache_generation(&config);
        let writer_config = config.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            atomic_write(&writer_config.usage_file, br#"[{"provider":"codex"}]"#)
                .expect("new payload before stamp");
            drop(guard);
        });
        assert!(wait_for_cache(&config, Some(&previous)).is_ok());
        writer.join().expect("holder finished");
        assert_eq!(
            std::fs::read(&config.usage_stamp).expect("stamp unchanged"),
            b"old-stamp"
        );
    }

    #[test]
    fn forced_contender_accepts_changed_stamp_for_fresh_payload() {
        let dir = TestDir::new();
        let mut config = dir.config();
        config.lock_wait_tenths = 10;
        let guard = acquire_lock(&config).expect("holder lease");
        std::fs::write(&config.usage_file, b"[]").expect("fresh payload");
        std::fs::write(&config.usage_stamp, b"old-stamp").expect("old stamp");
        let previous = cache_generation(&config);
        let stamp = config.usage_stamp.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            atomic_write(&stamp, b"new-stamp").expect("new generation stamp");
            drop(guard);
        });
        assert!(wait_for_cache(&config, Some(&previous)).is_ok());
        writer.join().expect("holder finished");
        assert_eq!(
            std::fs::read(&config.usage_file).expect("payload unchanged"),
            b"[]"
        );
    }

    #[test]
    fn forced_contender_rejects_changed_stamp_with_stale_payload() {
        let dir = TestDir::new();
        let config = dir.config();
        ensure_cache_dir(&config).expect("cache directory");
        std::fs::write(&config.usage_file, b"[]").expect("valid old payload");
        std::fs::File::open(&config.usage_file)
            .expect("cache file")
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::now() - Duration::from_secs(3600)),
            )
            .expect("old payload mtime");
        std::fs::write(&config.usage_stamp, b"old-stamp").expect("old stamp");
        let previous = cache_generation(&config);
        std::fs::write(&config.usage_stamp, b"new-stamp").expect("stamp-only change");
        assert!(wait_for_cache(&config, Some(&previous)).is_err());
    }

    #[test]
    fn contender_reacquires_once_when_holder_exits_without_publication() {
        for force_refresh in [false, true] {
            let dir = TestDir::new();
            let config = dir.config();
            let guard = acquire_lock(&config).expect("holder lease");
            if force_refresh {
                std::fs::write(&config.usage_file, b"[]").expect("unchanged existing cache");
            }
            let holder = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                drop(guard);
            });
            let started = Instant::now();
            let replacement = acquire_refresh_lock(&config, force_refresh)
                .expect("retry after wait")
                .expect("reacquired lease");
            holder.join().expect("holder finished");
            assert!(started.elapsed() >= Duration::from_millis(200));
            assert!(lease_still_owned(&replacement));
        }
    }

    #[test]
    fn cold_contender_timeout_remains_bounded_without_trusting_invalid_cache() {
        let dir = TestDir::new();
        let config = dir.config();
        let guard = acquire_lock(&config).expect("holder lease");
        std::fs::write(&config.usage_file, b"{}").expect("invalid cache");
        let started = Instant::now();
        assert!(acquire_refresh_lock(&config, false)
            .expect("cold timeout")
            .is_none());
        assert!(started.elapsed() >= Duration::from_millis(400));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(read_valid_cache(&config).is_err());
        assert!(lease_still_owned(&guard));
    }

    fn lease_test_start(pid: u32) -> String {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .output()
            .expect("owned process identity");
        assert!(output.status.success());
        let start = String::from_utf8(output.stdout).expect("process start text");
        let start = start.trim().to_string();
        assert!(!start.is_empty());
        start
    }

    fn lease_test_state(pid: u32) -> Option<String> {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "state="])
            .output()
            .expect("process state");
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!state.is_empty()).then_some(state)
    }

    #[test]
    fn live_legacy_owner_with_tab_or_space_keeps_its_old_lease() {
        let dir = TestDir::new();
        let config = dir.config();
        let lease = lock_dir(&config);
        std::fs::create_dir_all(&lease).expect("legacy lease");
        let pid = std::process::id();
        let start = lease_test_start(pid);
        for separator in ["\t", " "] {
            let record = format!("{pid}{separator}{start}\n");
            std::fs::write(lease.join("owner.pid"), &record).expect("legacy owner record");
            std::fs::File::open(&lease)
                .expect("lease directory")
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(SystemTime::now() - Duration::from_secs(3600)),
                )
                .expect("old lease mtime");
            let identity = directory_identity(&lease);
            assert!(try_acquire_lock(&config)
                .expect("legacy contention")
                .is_none());
            assert_eq!(directory_identity(&lease), identity);
            assert_eq!(
                std::fs::read_to_string(lease.join("owner.pid")).unwrap(),
                record
            );
            assert!(!lease.join("owner.token").exists());
        }
    }

    #[test]
    fn reused_pid_legacy_identity_allows_fenced_recovery() {
        let dir = TestDir::new();
        let config = dir.config();
        let lease = lock_dir(&config);
        std::fs::create_dir_all(&lease).expect("legacy lease");
        std::fs::write(
            lease.join("owner.pid"),
            format!("{}\tMon Jan 1 00:00:00 1900\n", std::process::id()),
        )
        .expect("stale process identity");
        std::fs::write(lease.join("owner.token"), b"old-token").expect("old token");
        let guard = acquire_lock(&config).expect("identity-checked recovery");
        assert_ne!(guard.token, "old-token");
        assert!(lease_still_owned(&guard));
        assert!(config.usage_lock.exists());
    }

    struct LeaseTestChild(std::process::Child);

    impl Drop for LeaseTestChild {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(None)) {
                // SAFETY: the unreaped child belongs to this test.
                unsafe {
                    libc::kill(self.0.id() as libc::pid_t, libc::SIGCONT);
                    libc::kill(self.0.id() as libc::pid_t, libc::SIGTERM);
                }
                let deadline = Instant::now() + Duration::from_secs(2);
                while matches!(self.0.try_wait(), Ok(None)) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if matches!(self.0.try_wait(), Ok(None)) {
                    let _ = self.0.kill();
                    let _ = self.0.wait();
                }
            }
        }
    }

    #[test]
    fn stopped_legacy_and_pid_only_owners_allow_recovery() {
        for legacy in [false, true] {
            let child = LeaseTestChild(
                std::process::Command::new("/bin/sleep")
                    .arg("60")
                    .spawn()
                    .expect("owned lease holder"),
            );
            let pid = child.0.id();
            let start = lease_test_start(pid);
            // SAFETY: this test owns the unreaped sleep process.
            assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGSTOP) }, 0);
            let deadline = Instant::now() + Duration::from_secs(2);
            while !lease_test_state(pid).is_some_and(|state| state.starts_with('T')) {
                assert!(Instant::now() < deadline, "holder did not stop");
                std::thread::sleep(Duration::from_millis(10));
            }
            let dir = TestDir::new();
            let config = dir.config();
            let lease = lock_dir(&config);
            std::fs::create_dir_all(&lease).expect("stopped holder lease");
            let record = if legacy {
                format!("{pid}\t{start}\n")
            } else {
                format!("{pid}\n")
            };
            std::fs::write(lease.join("owner.pid"), record).expect("stopped owner record");
            let guard = acquire_lock(&config).expect("recover stopped holder");
            assert!(lease_still_owned(&guard));
        }
    }

    #[test]
    fn future_cache_mtime_uses_absolute_age_and_cannot_pin_freshness() {
        let dir = TestDir::new();
        let config = dir.config();
        ensure_cache_dir(&config).expect("cache directory");
        std::fs::write(&config.usage_file, b"[]").expect("valid cache");
        std::fs::File::open(&config.usage_file)
            .expect("cache file")
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::now() + Duration::from_secs(3600)),
            )
            .expect("future mtime");
        assert!((3598..=3601).contains(&file_age_seconds(&config.usage_file)));
        assert!(cache_age_seconds(&config) >= config.refresh_seconds);
    }

    #[test]
    fn cache_privacy_setup_rejects_symlink_without_chmod_or_cache_trust() {
        let dir = TestDir::new();
        let config = dir.config();
        let target = dir.0.join("unsafe-target");
        std::fs::create_dir(&target).expect("unsafe target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o777))
            .expect("unsafe target permissions");
        std::fs::write(target.join("usage.json"), b"[]").expect("untrusted cache");
        std::os::unix::fs::symlink(&target, &config.cache_dir).expect("symlink cache");
        assert!(ensure_cache_dir(&config).is_err());
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o777
        );
    }

    #[test]
    fn cache_privacy_setup_rejects_foreign_owned_directory() {
        use std::os::unix::fs::MetadataExt;
        let dir = TestDir::new();
        let mut config = dir.config();
        // SAFETY: geteuid only reads the effective user identifier.
        let uid = unsafe { libc::geteuid() };
        if uid == 0 {
            std::fs::create_dir(&config.cache_dir).expect("foreign fixture");
            std::os::unix::fs::chown(&config.cache_dir, Some(1), None)
                .expect("make test directory foreign-owned");
        } else {
            config.cache_dir = PathBuf::from("/");
        }
        let before = std::fs::metadata(&config.cache_dir).expect("foreign directory");
        assert_ne!(before.uid(), uid);
        let error = ensure_cache_dir(&config).expect_err("foreign ownership must fail");
        assert!(error.contains("not owned"));
        let after = std::fs::metadata(&config.cache_dir).expect("foreign directory unchanged");
        assert_eq!((after.uid(), after.mode()), (before.uid(), before.mode()));
    }

    #[test]
    fn cache_privacy_setup_repairs_owned_directory_before_use() {
        let dir = TestDir::new();
        let config = dir.config();
        std::fs::create_dir(&config.cache_dir).expect("owned cache directory");
        std::fs::set_permissions(&config.cache_dir, std::fs::Permissions::from_mode(0o777))
            .expect("permissive directory fixture");
        ensure_cache_dir(&config).expect("privacy setup");
        assert_eq!(
            std::fs::metadata(&config.cache_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    #[ignore = "subprocess fixture; the cancellation regressions invoke it explicitly"]
    fn bounded_cancel_fixture() {
        let binary = std::env::var("SHOWY_QUOTA_NATIVE_CANCEL_FIXTURE")
            .expect("the parent regression supplies an owned fake CLI");
        let _ = run_bounded(&[binary], Duration::from_secs(60), 1024);
        panic!("the cancellation fixture must not finish normally");
    }

    fn lease_test_pid_file(path: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|raw| raw.trim().parse().ok())
            {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "PID file {} did not appear",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn exercise_bounded_cancellation(leader_exits: bool) {
        use std::os::unix::process::ExitStatusExt;
        let persistent = FakeNativeServe::new();
        let _serve = LeaseTestChild(persistent.spawn(true));
        let serve_pid =
            owned_serve_pid(&persistent.config).expect("identity-checked managed serve");
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let dir = TestDir::new();
            let tail = if leader_exits {
                "exit 0"
            } else {
                "wait \"$helper\""
            };
            let binary = dir.script("blocking-cli", &format!(
                "printf '%s' \"$$\" > \"$0.pid\"\n\
                 /bin/sh -c 'printf \"%s\" \"$$\" > \"$1\"; exec /bin/sleep 60' helper \"$0.helper\" &\n\
                 helper=$!\n\
                 while [ ! -s \"$0.helper\" ]; do /bin/sleep 0.01; done\n\
                 {tail}"
            ));
            let mut runner = LeaseTestChild(
                std::process::Command::new(
                    std::env::current_exe().expect("native test executable"),
                )
                .args([
                    "--exact",
                    "native_fetch::tests::bounded_cancel_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("SHOWY_QUOTA_NATIVE_CANCEL_FIXTURE", &binary)
                .stdout(std::process::Stdio::null())
                .spawn()
                .expect("isolated cancellation runner"),
            );
            let leader = lease_test_pid_file(Path::new(&format!("{binary}.pid")));
            let helper = lease_test_pid_file(Path::new(&format!("{binary}.helper")));
            if leader_exits {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !lease_test_state(leader).is_some_and(|state| state.starts_with('Z')) {
                    assert!(
                        Instant::now() < deadline,
                        "exited leader must remain unreaped"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            assert!(runner.0.try_wait().expect("runner state").is_none());
            // SAFETY: this test owns the unreaped runner; do not signal its group.
            assert_eq!(
                unsafe { libc::kill(runner.0.id() as libc::pid_t, signal) },
                0
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            let status = loop {
                if let Some(status) = runner.0.try_wait().expect("reap cancellation runner") {
                    break status;
                }
                assert!(
                    Instant::now() < deadline,
                    "cancellation did not stop the runner"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(status.signal(), Some(signal));
            assert!(
                !owner_alive(leader),
                "cancellation must reap the direct command child"
            );
            let deadline = Instant::now() + Duration::from_secs(2);
            while lease_test_state(helper).is_some_and(|state| !state.starts_with('Z')) {
                assert!(
                    Instant::now() < deadline,
                    "temporary helper remains running"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(owned_serve_pid(&persistent.config), Some(serve_pid));
            assert!(
                health_ok(&persistent.config),
                "managed serve must survive cancellation"
            );
        }
    }

    #[test]
    fn bounded_cancellation_stops_groups_and_preserves_managed_serve() {
        exercise_bounded_cancellation(false);
    }

    #[test]
    fn bounded_cancellation_reaps_exited_leader_with_inherited_stdout() {
        exercise_bounded_cancellation(true);
    }
}
