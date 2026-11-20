//! "A newer version is available" notice, on by default.
//!
//! At most once every 24 hours, a background thread asks the npm registry for
//! the latest published version (a plain GET; nothing about the user, the
//! project or the command is sent). The answer is cached in the user config
//! directory, and the notice is printed to stderr at most once a day.
//!
//! It never runs where it could get in the way:
//! - `ajiro-api mcp` (stdout/stdin are the MCP protocol) and `--json` output;
//! - when stderr is not a terminal (agents, pipes, CI logs);
//! - when `CI`, `DO_NOT_TRACK=1` or `AJIRO_API_NO_UPDATE_CHECK=1` is set.
//!
//! `--version` and `--help` exit inside clap before this module runs, so
//! startup time is unaffected.

use serde::{Deserialize, Serialize};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_URL: &str = "https://registry.npmjs.org/ajiro-api/latest";
const RELEASES_URL: &str = "https://github.com/niti-lab/ajiro-api/releases";
const INTERVAL_SECS: u64 = 24 * 60 * 60;
/// How long the registry request may take in the background.
const FETCH_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long the command may wait for that request after it has finished its
/// own work. A slower answer is simply used on the next run.
const FINISH_WAIT: Duration = Duration::from_millis(200);

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Cache {
    /// Unix seconds of the last registry request (successful or not).
    #[serde(default)]
    checked_at: u64,
    /// Latest version the registry reported.
    #[serde(default)]
    latest: Option<String>,
    /// Unix seconds when the notice was last printed.
    #[serde(default)]
    notified_at: u64,
}

/// A started check. Call [`Pending::finish`] after the command has run.
pub struct Pending {
    cache: Cache,
    rx: Option<mpsc::Receiver<Option<String>>>,
}

/// Whether the update check is allowed to run at all.
pub fn enabled(command: &str, json: bool) -> bool {
    if command == "mcp" || json {
        return false;
    }
    if env_set("CI") || env_true("AJIRO_API_NO_UPDATE_CHECK") || crate::telemetry::do_not_track() {
        return false;
    }
    std::io::stderr().is_terminal()
}

/// Start the check (a background request when the cache is stale).
pub fn start(command: &str, json: bool) -> Option<Pending> {
    if !enabled(command, json) {
        return None;
    }
    let mut cache = load();
    let now = now();
    let rx = if needs_fetch(&cache, now) {
        // Record the attempt first so an offline machine does not retry on
        // every run.
        cache.checked_at = now;
        let _ = save(&cache);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_latest());
        });
        Some(rx)
    } else {
        None
    };
    Some(Pending { cache, rx })
}

impl Pending {
    /// Collect the background answer (waiting briefly) and print the notice
    /// if one is due.
    pub fn finish(mut self) {
        if let Some(rx) = self.rx.take() {
            if let Ok(Some(v)) = rx.recv_timeout(FINISH_WAIT) {
                self.cache.latest = Some(v);
                let _ = save(&self.cache);
            }
        }
        if let Some(msg) = notice(&mut self.cache, env!("CARGO_PKG_VERSION"), now()) {
            eprintln!("{}", msg);
            let _ = save(&self.cache);
        }
    }
}

fn needs_fetch(cache: &Cache, now: u64) -> bool {
    now.saturating_sub(cache.checked_at) >= INTERVAL_SECS || cache.checked_at > now
}

/// The notice to print, if the cached latest version is newer than `current`
/// and the notice has not been printed in the last 24 hours. Marks it as
/// printed.
fn notice(cache: &mut Cache, current: &str, now: u64) -> Option<String> {
    let latest = cache.latest.clone()?;
    if !is_newer(&latest, current) {
        return None;
    }
    if now.saturating_sub(cache.notified_at) < INTERVAL_SECS && cache.notified_at <= now {
        return None;
    }
    cache.notified_at = now;
    Some(format!(
        "\nA new version of ajiro-api is available: {current} -> {latest}\n\
         Release notes: {RELEASES_URL}\n\
         (Set AJIRO_API_NO_UPDATE_CHECK=1 to turn this notice off.)"
    ))
}

/// `a > b` for plain `MAJOR.MINOR.PATCH` versions. Pre-releases and anything
/// unparsable are never "newer".
fn is_newer(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim().trim_start_matches('v').split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn fetch_latest() -> Option<String> {
    let url = std::env::var("AJIRO_API_UPDATE_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .new_agent();
    let mut resp = agent.get(&url).call().ok()?;
    let body: serde_json::Value = resp.body_mut().read_json().ok()?;
    let v = body.get("version")?.as_str()?.to_string();
    parse(&v).map(|_| v)
}

fn cache_path() -> Option<PathBuf> {
    crate::telemetry::config_dir().map(|d| d.join("update-check.json"))
}

fn load() -> Cache {
    cache_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(c: &Cache) -> std::io::Result<()> {
    if let Some(p) = cache_path() {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, serde_json::to_string_pretty(c)?)?;
    }
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

fn env_true(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = INTERVAL_SECS;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.4", "0.2.3"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.2.3", "0.2.3"));
        assert!(!is_newer("0.2.2", "0.2.3"));
        assert!(!is_newer("0.3.0-beta.1", "0.2.3"));
        assert!(!is_newer("garbage", "0.2.3"));
    }

    #[test]
    fn fetches_at_most_once_a_day() {
        let c = Cache {
            checked_at: 10 * DAY,
            ..Default::default()
        };
        assert!(!needs_fetch(&c, 10 * DAY + DAY - 1));
        assert!(needs_fetch(&c, 11 * DAY));
        assert!(needs_fetch(&Cache::default(), 10 * DAY));
        // A clock that went backwards does not block checks forever.
        assert!(needs_fetch(&c, 5 * DAY));
    }

    #[test]
    fn notifies_only_when_newer_and_once_a_day() {
        let mut c = Cache {
            latest: Some("0.2.4".into()),
            ..Default::default()
        };
        let msg = notice(&mut c, "0.2.3", 10 * DAY).expect("notice");
        assert!(msg.contains("0.2.3 -> 0.2.4"));
        assert!(msg.contains("AJIRO_API_NO_UPDATE_CHECK"));
        assert_eq!(c.notified_at, 10 * DAY);
        assert!(notice(&mut c, "0.2.3", 10 * DAY + 60).is_none());
        assert!(notice(&mut c, "0.2.3", 11 * DAY).is_some());

        let mut same = Cache {
            latest: Some("0.2.3".into()),
            ..Default::default()
        };
        assert!(notice(&mut same, "0.2.3", 10 * DAY).is_none());
        assert!(notice(&mut Cache::default(), "0.2.3", 10 * DAY).is_none());
    }

    #[test]
    fn never_runs_for_mcp_or_json() {
        assert!(!enabled("mcp", false));
        assert!(!enabled("test", true));
    }
}
