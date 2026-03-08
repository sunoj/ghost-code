// Statusline data: writes bot status JSON for the statusline command.
// Fetches costs, plan limits, and tool savings summaries.

use crate::config::Config;
use crate::hook;
use serde_json::{json, Value};
use std::process::Command;

pub(super) fn write_status(path: &std::path::Path, _config: &Config, running: bool) {
    let usage = crate::usage::scan_today();
    let plan_str = crate::plan_usage::fetch_cached()
        .map(|u| crate::plan_usage::format_status(&u))
        .unwrap_or_default();

    let status = json!({
        "bot": running,
        "host": hook::short_hostname(),
        "today_cost": usage.cost_usd,
        "plan": plan_str,
        "rtk": fetch_rtk_summary(),
        "ws": fetch_ws_summary(),
    });

    std::fs::write(path, serde_json::to_string(&status).unwrap_or_default()).ok();
}

/// Run `rtk gain -f json` and return a compact summary string like "80% 3.6M".
fn fetch_rtk_summary() -> String {
    let output = Command::new("rtk")
        .args(["gain", "-f", "json"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok();
    let Some(output) = output.filter(|o| o.status.success()) else {
        return String::new();
    };
    let data: Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    let saved = data["summary"]["total_saved"].as_f64().unwrap_or(0.0);
    let pct = data["summary"]["avg_savings_pct"].as_f64().unwrap_or(0.0);
    if saved < 1.0 {
        return String::new();
    }
    format!("{:.0}% {}", pct, format_tokens(saved as u64))
}

/// Run `websummary stats` and parse the text output for a compact summary like "93% 3.3K".
fn fetch_ws_summary() -> String {
    let output = Command::new("websummary")
        .args(["stats"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok();
    let Some(output) = output.filter(|o| o.status.success()) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut compression = String::new();
    let mut saved = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(v) = trimmed.strip_prefix("Compression:") {
            compression = v.trim().to_string();
        } else if let Some(v) = trimmed.strip_prefix("Claude tokens saved:") {
            saved = v.trim().trim_start_matches('~').to_string();
        }
    }
    if compression.is_empty() && saved.is_empty() {
        return String::new();
    }
    if !compression.is_empty() && !saved.is_empty() {
        format!("{compression} {saved}")
    } else if !compression.is_empty() {
        compression
    } else {
        saved
    }
}

pub(super) fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_small() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(0), "0");
    }

    #[test]
    fn format_tokens_thousands() {
        assert_eq!(format_tokens(3600), "3.6K");
        assert_eq!(format_tokens(1000), "1.0K");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(3_600_000), "3.6M");
        assert_eq!(format_tokens(1_000_000), "1.0M");
    }
}
