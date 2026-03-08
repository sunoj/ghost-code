// Fetch Claude Code plan usage limits from Anthropic OAuth API.
// Uses macOS Keychain for OAuth token, caches results to disk (60s TTL).

use serde_json::Value;
use std::path::PathBuf;

pub struct PlanUsage {
    pub five_hour_pct: Option<f64>,
    pub seven_day_pct: Option<f64>,
    pub five_hour_reset_secs: Option<i64>,
    pub seven_day_reset_secs: Option<i64>,
}

const CACHE_FILE: &str = "ghost-code-plan-usage.json";
const CACHE_MAX_AGE_SECS: u64 = 300;

/// Get plan usage with file-based caching (60s TTL).
/// Falls back to stale cache if API fetch fails.
pub fn fetch_cached() -> Option<PlanUsage> {
    let cache_path = crate::hook::hooks_dir().join(CACHE_FILE);

    if is_fresh(&cache_path) {
        if let Some(u) = read_cache(&cache_path) {
            return Some(u);
        }
    }

    if let Some(data) = fetch_from_api() {
        let tmp = cache_path.with_extension("tmp");
        if std::fs::write(&tmp, &data).is_ok() {
            std::fs::rename(&tmp, &cache_path).ok();
        }
        return parse_usage(&data);
    }

    // Fallback: use stale cache rather than returning None
    read_cache(&cache_path)
}

fn is_fresh(path: &PathBuf) -> bool {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs() < CACHE_MAX_AGE_SECS)
        .unwrap_or(false)
}

fn read_cache(path: &PathBuf) -> Option<PlanUsage> {
    parse_usage(&std::fs::read_to_string(path).ok()?)
}

fn parse_usage(json_str: &str) -> Option<PlanUsage> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let now_epoch = now_epoch_secs();
    Some(PlanUsage {
        five_hour_pct: v["five_hour"]["utilization"].as_f64(),
        seven_day_pct: v["seven_day"]["utilization"].as_f64(),
        five_hour_reset_secs: v["five_hour"]["resets_at"]
            .as_str()
            .and_then(|s| secs_until_reset(s, now_epoch)),
        seven_day_reset_secs: v["seven_day"]["resets_at"]
            .as_str()
            .and_then(|s| secs_until_reset(s, now_epoch)),
    })
}

fn fetch_from_api() -> Option<String> {
    let token = get_oauth_token()?;
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(5))
        .build();
    let resp = agent
        .get("https://api.anthropic.com/api/oauth/usage")
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call()
        .ok()?;
    let body = resp.into_string().ok()?;
    // Validate response has expected fields (not an error response)
    let v: Value = serde_json::from_str(&body).ok()?;
    v.get("five_hour")?;
    Some(body)
}

fn get_oauth_token() -> Option<String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let json_str = String::from_utf8(output.stdout).ok()?;
    let creds: Value = serde_json::from_str(json_str.trim()).ok()?;
    creds["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(String::from)
}

/// Format: "82% block (3h18m) · 38% weekly (5d5h)"
pub fn format_status(u: &PlanUsage) -> String {
    let mut parts = Vec::new();
    if let Some(pct) = u.five_hour_pct {
        let reset = u
            .five_hour_reset_secs
            .filter(|&s| s > 0)
            .map(|s| format!(" ({})", format_duration(s)))
            .unwrap_or_default();
        parts.push(format!("{:.0}% block{reset}", pct));
    }
    if let Some(pct) = u.seven_day_pct {
        let reset = u
            .seven_day_reset_secs
            .filter(|&s| s > 0)
            .map(|s| format!(" ({})", format_duration(s)))
            .unwrap_or_default();
        parts.push(format!("{:.0}% weekly{reset}", pct));
    }
    parts.join(" · ")
}

fn format_duration(secs: i64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 && h > 0 {
        format!("{d}d{h}h")
    } else if d > 0 {
        format!("{d}d")
    } else if h > 0 && m > 0 {
        format!("{h}h{m}m")
    } else if h > 0 {
        format!("{h}h")
    } else if m > 0 {
        format!("{m}m")
    } else {
        "<1m".to_string()
    }
}

fn secs_until_reset(iso: &str, now_epoch: i64) -> Option<i64> {
    let dt_part = iso.split('.').next()?;
    let tz_offset_secs = parse_tz_offset(iso);

    let parts: Vec<&str> = dt_part.split('T').collect();
    if parts.len() != 2 {
        return None;
    }
    let date: Vec<i32> = parts[0].split('-').filter_map(|s| s.parse().ok()).collect();
    let time: Vec<i32> = parts[1]
        .split(':')
        .filter_map(|s| s.parse().ok())
        .collect();
    if date.len() != 3 || time.len() != 3 {
        return None;
    }

    let reset_epoch = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = date[0] - 1900;
        tm.tm_mon = date[1] - 1;
        tm.tm_mday = date[2];
        tm.tm_hour = time[0];
        tm.tm_min = time[1];
        tm.tm_sec = time[2];
        libc::timegm(&mut tm)
    };

    let reset_utc = reset_epoch - tz_offset_secs as i64;
    let remaining = reset_utc - now_epoch;
    Some(if remaining > 0 { remaining } else { 0 })
}

fn parse_tz_offset(iso: &str) -> i32 {
    let after_t = match iso.find('T') {
        Some(pos) => &iso[pos..],
        None => return 0,
    };
    let tz_str = if let Some(pos) = after_t.rfind('+') {
        &after_t[pos..]
    } else if let Some(pos) = after_t.rfind('-') {
        &after_t[pos..]
    } else {
        return 0;
    };
    let sign = if tz_str.starts_with('-') { -1 } else { 1 };
    let digits: Vec<i32> = tz_str[1..]
        .split(':')
        .filter_map(|s| s.parse().ok())
        .collect();
    if digits.len() == 2 {
        sign * (digits[0] * 3600 + digits[1] * 60)
    } else {
        0
    }
}

fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
