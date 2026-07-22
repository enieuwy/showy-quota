use std::env;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::codexbar::MAX_USAGE_JSON_BYTES;

pub const MISSING_AGE_SECONDS: i64 = 999_999_999;
const DEFAULT_REFRESH_SECONDS: i64 = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheFreshness {
    pub age_seconds: i64,
    pub stale: bool,
    pub source: String,
    pub degraded_cli: bool,
}

#[derive(Debug)]
pub struct CacheSnapshot {
    pub payload: Vec<u8>,
    pub freshness: CacheFreshness,
}

#[derive(Debug)]
pub struct CacheReadError {
    path: PathBuf,
    source: io::Error,
}

impl fmt::Display for CacheReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to read cache payload {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for CacheReadError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePaths {
    pub usage_file: PathBuf,
    pub source_file: PathBuf,
}

pub fn read_cache_from_env(now_epoch: i64) -> Result<CacheSnapshot, CacheReadError> {
    let paths = cache_paths_from_env();
    // Read the payload FIRST, then the freshness metadata (mtime + source).
    // The fetcher publishes stamp+source before renaming usage.json last, so a
    // payload we observe is always paired with matching-or-newer metadata; do
    // not reorder these two reads or the mixed-generation race reopens.
    let payload = read_usage_payload(&paths.usage_file).map_err(|source| CacheReadError {
        path: paths.usage_file.clone(),
        source,
    })?;
    let freshness = freshness_for_paths(&paths, now_epoch);
    Ok(CacheSnapshot { payload, freshness })
}

fn read_usage_payload(path: &PathBuf) -> io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    read_bounded_payload(file)
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

pub fn cache_paths_from_env() -> CachePaths {
    let cache_dir = env_nonempty("SHOWY_QUOTA_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_cache_dir);
    let usage_file = env_nonempty("SHOWY_QUOTA_USAGE_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| cache_dir.join("usage.json"));
    let source_file = env_nonempty("SHOWY_QUOTA_SOURCE_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| cache_dir.join("source"));
    CachePaths {
        usage_file,
        source_file,
    }
}

pub fn freshness_for_paths(paths: &CachePaths, now_epoch: i64) -> CacheFreshness {
    let mtime_epoch = fs::metadata(&paths.usage_file)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(system_time_epoch);
    let source = read_cache_source(&paths.source_file);
    freshness_from_parts(
        mtime_epoch,
        now_epoch,
        refresh_seconds_from_env(),
        source,
        env::var("SHOWY_QUOTA_DEGRADED_CLI").ok(),
    )
}

pub fn freshness_from_parts(
    mtime_epoch: Option<i64>,
    now_epoch: i64,
    refresh_seconds: i64,
    source: String,
    degraded_cli_env: Option<String>,
) -> CacheFreshness {
    let age_seconds = age_seconds(now_epoch, mtime_epoch);
    let stale_after = refresh_seconds.saturating_mul(2);
    let stale = age_seconds > stale_after;
    // Tri-state, matching the shell driver: SHOWY_QUOTA_DEGRADED_CLI="1" forces
    // the marker on; any other non-empty value (e.g. "0") forces it off; unset
    // or empty derives it from the cache source. A two-state
    // `== Some("1") || source == "cli"` wrongly ignored the explicit "0" off.
    let degraded_cli = match degraded_cli_env.as_deref() {
        Some("1") => true,
        Some(value) if !value.is_empty() => false,
        _ => source == "cli",
    };
    CacheFreshness {
        age_seconds,
        stale,
        source,
        degraded_cli,
    }
}

pub fn age_seconds(now_epoch: i64, mtime_epoch: Option<i64>) -> i64 {
    let Some(mtime_epoch) = mtime_epoch else {
        return MISSING_AGE_SECONDS;
    };
    let diff = i128::from(now_epoch) - i128::from(mtime_epoch);
    diff.abs().min(i128::from(i64::MAX)) as i64
}

pub fn refresh_seconds_from_env() -> i64 {
    env::var("SHOWY_QUOTA_REFRESH_SECONDS")
        .ok()
        .and_then(|value| parse_refresh_seconds(&value))
        .unwrap_or(DEFAULT_REFRESH_SECONDS)
}

pub fn parse_refresh_seconds(raw: &str) -> Option<i64> {
    if raw.is_empty() || raw.len() > 18 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

fn env_nonempty(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn default_cache_dir() -> PathBuf {
    if let Some(xdg) = env_nonempty("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("showy-quota");
    }
    env_nonempty("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".cache")
        .join("showy-quota")
}

fn read_cache_source(path: &PathBuf) -> String {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| value.lines().next().map(str::trim).map(str::to_owned))
        .unwrap_or_else(|| String::from("unknown"))
}

fn system_time_epoch(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs().min(i64::MAX as u64) as i64,
        Err(err) => -(err.duration().as_secs().min(i64::MAX as u64) as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_matches_shell_age_stale_and_degraded_rules() {
        let fresh = freshness_from_parts(Some(1_000), 1_100, 60, "serve".into(), None);
        assert_eq!(fresh.age_seconds, 100);
        assert!(!fresh.stale);
        assert!(!fresh.degraded_cli);

        let boundary = freshness_from_parts(Some(1_000), 1_120, 60, "serve".into(), None);
        assert_eq!(boundary.age_seconds, 120);
        assert!(!boundary.stale);

        let stale = freshness_from_parts(Some(1_000), 1_121, 60, "serve".into(), None);
        assert_eq!(stale.age_seconds, 121);
        assert!(stale.stale);

        let future = freshness_from_parts(Some(1_300), 1_000, 120, "serve".into(), None);
        assert_eq!(future.age_seconds, 300);
        assert!(future.stale);

        let source_cli = freshness_from_parts(Some(1_000), 1_000, 120, "cli".into(), None);
        assert!(source_cli.degraded_cli);

        let env_cli =
            freshness_from_parts(Some(1_000), 1_000, 120, "serve".into(), Some("1".into()));
        assert!(env_cli.degraded_cli);

        // Tri-state override: an explicit non-"1" value forces the marker off
        // even when the source is cli (matches the shell's -z guard).
        let forced_off =
            freshness_from_parts(Some(1_000), 1_000, 120, "cli".into(), Some("0".into()));
        assert!(!forced_off.degraded_cli);
        let empty_derives =
            freshness_from_parts(Some(1_000), 1_000, 120, "cli".into(), Some(String::new()));
        assert!(empty_derives.degraded_cli);
    }

    #[test]
    fn bounded_payload_reader_rejects_oversize_input() {
        let payload = vec![b' '; MAX_USAGE_JSON_BYTES + 1];
        let error = read_bounded_payload(std::io::Cursor::new(payload))
            .expect_err("oversize payload must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn missing_mtime_uses_shell_sentinel() {
        let freshness = freshness_from_parts(None, 1_000, 120, "unknown".into(), None);
        assert_eq!(freshness.age_seconds, MISSING_AGE_SECONDS);
        assert!(freshness.stale);
        assert!(!freshness.degraded_cli);
    }

    #[test]
    fn refresh_seconds_parse_is_defensive() {
        assert_eq!(parse_refresh_seconds("0"), Some(0));
        assert_eq!(parse_refresh_seconds("120"), Some(120));
        assert_eq!(parse_refresh_seconds("000120"), Some(120));
        assert_eq!(parse_refresh_seconds(""), None);
        assert_eq!(parse_refresh_seconds("12x"), None);
        assert_eq!(parse_refresh_seconds("1234567890123456789"), None);
    }
}
