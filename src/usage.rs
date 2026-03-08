// Calculate Claude Code usage costs from local JSONL session logs.
// Scans ~/.claude/projects/*/*.jsonl for today's usage data.
// Deduplicates entries by message.id:requestId hash.

use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

pub struct DaySummary {
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub requests: usize,
}

impl Default for DaySummary {
    fn default() -> Self {
        Self {
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            requests: 0,
        }
    }
}

/// Pricing per million tokens (USD) — (input, output, cache_read, cache_write)
fn cost_per_mtok(model: &str) -> (f64, f64, f64, f64) {
    if model.contains("opus") {
        (15.0, 75.0, 3.75, 18.75)
    } else if model.contains("haiku") {
        (0.80, 4.0, 0.08, 1.0)
    } else {
        // sonnet / default
        (3.0, 15.0, 0.30, 3.75)
    }
}

/// Scan all JSONL files modified today and calculate usage summary.
pub fn scan_today() -> DaySummary {
    let mut summary = DaySummary::default();
    let mut seen = HashSet::new();

    let claude_dir = PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".claude/projects");

    let today_start = today_midnight_epoch();

    let Ok(projects) = std::fs::read_dir(&claude_dir) else {
        return summary;
    };

    for project in projects.flatten() {
        if !project.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in files.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e != "jsonl").unwrap_or(true) {
                continue;
            }
            // Skip files not modified today
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d: std::time::Duration| d.as_secs())
                .unwrap_or(0);
            if mtime < today_start {
                continue;
            }
            scan_file(&path, &mut seen, &mut summary);
        }
    }

    summary
}

fn scan_file(path: &std::path::Path, seen: &mut HashSet<String>, summary: &mut DaySummary) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(usage) = entry.get("message").and_then(|m| m.get("usage")) else {
            continue;
        };
        let model = entry["message"]["model"].as_str().unwrap_or("sonnet");

        // Deduplicate by message.id + requestId
        let msg_id = entry["message"]["id"].as_str().unwrap_or("");
        let req_id = entry["requestId"].as_str().unwrap_or("");
        if !msg_id.is_empty() && !req_id.is_empty() {
            let hash = format!("{msg_id}:{req_id}");
            if !seen.insert(hash) {
                continue;
            }
        }

        let input = usage["input_tokens"].as_u64().unwrap_or(0);
        let output = usage["output_tokens"].as_u64().unwrap_or(0);
        let cache_write = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        let cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);

        let (pi, po, pcr, pcw) = cost_per_mtok(model);
        let cost = (input as f64 * pi
            + output as f64 * po
            + cache_read as f64 * pcr
            + cache_write as f64 * pcw)
            / 1_000_000.0;

        summary.cost_usd += cost;
        summary.input_tokens += input;
        summary.output_tokens += output;
        summary.cache_read_tokens += cache_read;
        summary.cache_write_tokens += cache_write;
        summary.requests += 1;
    }
}

/// Get epoch seconds for midnight local time today.
fn today_midnight_epoch() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let now_i64 = now as i64;
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now_i64 as *const i64, &mut tm);
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        libc::mktime(&mut tm) as u64
    }
}

/// Format cost as compact string: $0.12, $1.23, $12.3, $123
pub fn format_cost(usd: f64) -> String {
    if usd < 0.01 {
        format!("${:.3}", usd)
    } else if usd < 10.0 {
        format!("${:.2}", usd)
    } else if usd < 100.0 {
        format!("${:.1}", usd)
    } else {
        format!("${:.0}", usd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_cost_micro() {
        assert_eq!(format_cost(0.005), "$0.005");
        assert_eq!(format_cost(0.0), "$0.000");
    }

    #[test]
    fn format_cost_small() {
        assert_eq!(format_cost(1.23), "$1.23");
        assert_eq!(format_cost(0.50), "$0.50");
    }

    #[test]
    fn format_cost_medium() {
        assert_eq!(format_cost(42.567), "$42.6");
    }

    #[test]
    fn format_cost_large() {
        assert_eq!(format_cost(463.0), "$463");
    }

    #[test]
    fn cost_per_mtok_models() {
        let (i, o, _, _) = cost_per_mtok("claude-opus-4");
        assert_eq!(i, 15.0);
        assert_eq!(o, 75.0);

        let (i, o, _, _) = cost_per_mtok("claude-haiku-3.5");
        assert_eq!(i, 0.80);
        assert_eq!(o, 4.0);

        let (i, o, _, _) = cost_per_mtok("claude-sonnet-4");
        assert_eq!(i, 3.0);
        assert_eq!(o, 15.0);
    }
}
