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
