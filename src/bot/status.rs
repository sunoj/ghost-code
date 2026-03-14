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
        "ai": fetch_ai_summary(),
        "aid": fetch_aid_summary(),
    });

    std::fs::write(path, serde_json::to_string(&status).unwrap_or_default()).ok();
}

/// Run `ai-summary stats --json` and compute today's compact summary like "80% 51.0K".
fn fetch_ai_summary() -> String {
    let output = Command::new("ai-summary")
        .args(["stats", "--json"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok();
    let Some(output) = output.filter(|o| o.status.success()) else {
        return String::new();
    };
    let data: Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    let Some(history) = data["history"].as_array() else {
        return String::new();
    };

    // Filter to today's entries
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let today_start = now - (now % 86400); // UTC midnight

    let mut total_raw: u64 = 0;
    let mut total_summary: u64 = 0;
    let mut total_saved: u64 = 0;
    for entry in history {
        let ts = entry["timestamp"].as_u64().unwrap_or(0);
        if ts >= today_start {
            total_raw += entry["raw_chars"].as_u64().unwrap_or(0);
            total_summary += entry["summary_chars"].as_u64().unwrap_or(0);
            total_saved += entry["estimated_saved"].as_u64().unwrap_or(0);
        }
    }
    if total_saved == 0 {
        return String::new();
    }
    let compression = if total_raw > 0 {
        100.0 * (1.0 - total_summary as f64 / total_raw as f64)
    } else {
        0.0
    };
    let cost_saved = total_saved as f64 * 3.0 / 1_000_000.0;
    format!(
        "{:.0}% {} ${:.2}",
        compression,
        format_tokens(total_saved),
        cost_saved,
    )
}

/// Run `aid board --today` and `aid board --running` to build a compact summary like "2▶ 117✓ $464".
fn fetch_aid_summary() -> String {
    let parse_summary = |args: &[&str]| -> Option<(u64, u64, u64, f64)> {
        let output = Command::new("aid")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        // First line: "Tasks: 184 total | 117 done | 2 running | 65 failed"
        let first = text.lines().next()?;
        let mut total = 0u64;
        let mut done = 0u64;
        let mut running = 0u64;
        let mut failed = 0u64;
        for part in first.split('|') {
            let part = part.trim();
            if let Some(n) = part.strip_suffix(" total") {
                total = n.trim_start_matches("Tasks: ").trim().parse().unwrap_or(0);
            } else if let Some(n) = part.strip_suffix(" done") {
                done = n.trim().parse().unwrap_or(0);
            } else if let Some(n) = part.strip_suffix(" running") {
                running = n.trim().parse().unwrap_or(0);
            } else if let Some(n) = part.strip_suffix(" failed") {
                failed = n.trim().parse().unwrap_or(0);
            }
        }
        // Second line: "Total tokens: 122.7M  Cost: $464.10"
        let mut cost = 0.0f64;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Total tokens:") {
                if let Some(cost_part) = rest.split("Cost:").nth(1) {
                    cost = cost_part.trim().trim_start_matches('$').parse().unwrap_or(0.0);
                }
            }
        }
        let _ = failed;
        Some((total, done, running, cost))
    };

    // Get today's stats
    let Some((total, done, _running_today, cost)) = parse_summary(&["board", "--today"]) else {
        return String::new();
    };
    if total == 0 {
        return String::new();
    }

    // Get currently running count
    let running = parse_summary(&["board", "--running"])
        .map(|(r, _, _, _)| r)
        .unwrap_or(0);

    let mut parts = Vec::new();
    if running > 0 {
        parts.push(format!("{running}\u{23f3}"));
    }
    parts.push(format!("{done}\u{2713}"));
    let cost_str = crate::usage::format_cost(cost);
    parts.push(cost_str);
    parts.join(" ")
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
