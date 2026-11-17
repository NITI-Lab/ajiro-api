//! Anonymous usage statistics. OFF by default.
//!
//! Only when the user runs `ajiro-api telemetry on` do we count command
//! executions (command name + CLI version + OS, nothing else: no paths, no
//! URLs, no API definitions). Counts are kept in the user config directory
//! and sent only if `AJIRO_API_TELEMETRY_ENDPOINT` is set. `DO_NOT_TRACK=1`
//! always disables it.

use crate::cli::TelemetryArgs;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    enabled: bool,
    #[serde(default)]
    counts: BTreeMap<String, u64>,
}

fn state_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AJIRO_API_CONFIG_DIR") {
        return Some(PathBuf::from(p).join("telemetry.json"));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("ajiro-api").join("telemetry.json"))
}

fn load() -> State {
    state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(s: &State) -> Result<()> {
    if let Some(p) = state_path() {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, serde_json::to_string_pretty(s)?)?;
    }
    Ok(())
}

fn do_not_track() -> bool {
    std::env::var("DO_NOT_TRACK")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

pub fn enabled() -> bool {
    !do_not_track() && load().enabled
}

/// Record one command execution. Never fails, never blocks for long.
pub fn record(command: &str) {
    if command == "telemetry" || !enabled() {
        return;
    }
    let mut s = load();
    *s.counts.entry(command.to_string()).or_default() += 1;
    if let Ok(endpoint) = std::env::var("AJIRO_API_TELEMETRY_ENDPOINT") {
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "counts": s.counts,
        });
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_millis(800)))
            .build()
            .new_agent();
        if agent.post(&endpoint).send_json(&payload).is_ok() {
            s.counts.clear();
        }
    }
    let _ = save(&s);
}

pub fn run_cli(args: &TelemetryArgs) -> Result<i32> {
    match args.action.as_str() {
        "on" => {
            let mut s = load();
            s.enabled = true;
            save(&s)?;
            println!("Anonymous usage statistics: ON (command counts only). Disable with `ajiro-api telemetry off`.");
        }
        "off" => {
            let s = State {
                enabled: false,
                counts: BTreeMap::new(),
            };
            save(&s)?;
            println!("Anonymous usage statistics: OFF. Local counts deleted.");
        }
        "status" => {
            let s = load();
            let on = s.enabled && !do_not_track();
            println!(
                "Anonymous usage statistics: {}",
                if on { "ON" } else { "OFF" }
            );
            if do_not_track() {
                println!("(DO_NOT_TRACK is set)");
            }
            if let Some(p) = state_path() {
                println!("state file: {}", p.display());
            }
        }
        other => bail!("unknown action `{}` (expected on, off or status)", other),
    }
    Ok(crate::exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Tests mutate process-wide environment variables, so serialize them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Run `f` with an isolated config dir and the given DO_NOT_TRACK value.
    fn with_env<F: FnOnce(&std::path::Path)>(dnt: Option<&str>, f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let old_dir = std::env::var_os("AJIRO_API_CONFIG_DIR");
        let old_dnt = std::env::var_os("DO_NOT_TRACK");
        let old_ep = std::env::var_os("AJIRO_API_TELEMETRY_ENDPOINT");
        std::env::set_var("AJIRO_API_CONFIG_DIR", dir.path());
        std::env::remove_var("AJIRO_API_TELEMETRY_ENDPOINT");
        match dnt {
            Some(v) => std::env::set_var("DO_NOT_TRACK", v),
            None => std::env::remove_var("DO_NOT_TRACK"),
        }
        f(dir.path());
        for (k, v) in [
            ("AJIRO_API_CONFIG_DIR", old_dir),
            ("DO_NOT_TRACK", old_dnt),
            ("AJIRO_API_TELEMETRY_ENDPOINT", old_ep),
        ] {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }

    fn args(action: &str) -> TelemetryArgs {
        TelemetryArgs {
            action: action.to_string(),
        }
    }

    #[test]
    fn test_disabled_by_default() {
        with_env(None, |_| {
            assert!(!enabled());
        });
    }

    #[test]
    fn test_state_parsing_on_and_off() {
        let on: State = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert!(on.enabled);
        assert!(on.counts.is_empty());
        let off: State =
            serde_json::from_str(r#"{"enabled": false, "counts": {"scan": 3}}"#).unwrap();
        assert!(!off.enabled);
        assert_eq!(off.counts["scan"], 3);
        assert!(serde_json::from_str::<State>("not json").is_err());
    }

    #[test]
    fn test_corrupt_state_file_means_disabled() {
        with_env(None, |dir| {
            std::fs::write(dir.join("telemetry.json"), "{ garbage").unwrap();
            assert!(!load().enabled);
            assert!(!enabled());
        });
    }

    #[test]
    fn test_on_off_roundtrip() {
        with_env(None, |dir| {
            assert_eq!(run_cli(&args("on")).unwrap(), crate::exit::OK);
            assert!(dir.join("telemetry.json").is_file());
            assert!(enabled());
            assert_eq!(run_cli(&args("off")).unwrap(), crate::exit::OK);
            assert!(!enabled());
            assert_eq!(run_cli(&args("status")).unwrap(), crate::exit::OK);
        });
    }

    #[test]
    fn test_unknown_action_is_error() {
        with_env(None, |_| {
            let err = run_cli(&args("maybe")).unwrap_err();
            assert!(err.to_string().contains("unknown action"));
        });
    }

    #[test]
    fn test_do_not_track_values() {
        with_env(Some("1"), |_| assert!(do_not_track()));
        with_env(Some("true"), |_| assert!(do_not_track()));
        with_env(Some("0"), |_| assert!(!do_not_track()));
        with_env(Some(""), |_| assert!(!do_not_track()));
        with_env(None, |_| assert!(!do_not_track()));
    }

    #[test]
    fn test_do_not_track_overrides_opt_in() {
        with_env(None, |_| {
            run_cli(&args("on")).unwrap();
            assert!(enabled());
            std::env::set_var("DO_NOT_TRACK", "1");
            assert!(!enabled());
            record("scan");
            assert!(load().counts.is_empty());
        });
    }

    #[test]
    fn test_record_counts_only_when_enabled() {
        with_env(None, |_| {
            record("scan");
            assert!(load().counts.is_empty());
            run_cli(&args("on")).unwrap();
            record("scan");
            record("scan");
            record("test");
            let s = load();
            assert_eq!(s.counts["scan"], 2);
            assert_eq!(s.counts["test"], 1);
        });
    }

    #[test]
    fn test_record_ignores_telemetry_command() {
        with_env(None, |_| {
            run_cli(&args("on")).unwrap();
            record("telemetry");
            assert!(load().counts.is_empty());
        });
    }

    #[test]
    fn test_off_deletes_local_counts() {
        with_env(None, |_| {
            run_cli(&args("on")).unwrap();
            record("scan");
            assert!(!load().counts.is_empty());
            run_cli(&args("off")).unwrap();
            assert!(load().counts.is_empty());
        });
    }

    #[test]
    fn test_stored_state_contains_no_pii() {
        with_env(None, |dir| {
            run_cli(&args("on")).unwrap();
            record("scan");
            record("test");
            let raw = std::fs::read_to_string(dir.join("telemetry.json")).unwrap();
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            let keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
            assert_eq!(keys.len(), 2);
            assert!(keys.contains(&"enabled") && keys.contains(&"counts"));
            for (k, n) in v["counts"].as_object().unwrap() {
                assert!(n.is_u64());
                assert!(!k.contains('/') && !k.contains('\\') && !k.contains("http"));
            }
            let cwd = std::env::current_dir().unwrap();
            assert!(!raw.contains(&cwd.to_string_lossy().to_string()));
        });
    }

    #[test]
    fn test_state_path_honours_config_dir() {
        with_env(None, |dir| {
            assert_eq!(state_path().unwrap(), dir.join("telemetry.json"));
        });
    }
}
