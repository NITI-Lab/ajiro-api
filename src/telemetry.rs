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
    std::env::var("DO_NOT_TRACK").map(|v| v == "1" || v == "true").unwrap_or(false)
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
            let s = State { enabled: false, counts: BTreeMap::new() };
            save(&s)?;
            println!("Anonymous usage statistics: OFF. Local counts deleted.");
        }
        "status" => {
            let s = load();
            let on = s.enabled && !do_not_track();
            println!("Anonymous usage statistics: {}", if on { "ON" } else { "OFF" });
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
