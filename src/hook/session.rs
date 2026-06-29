// Session mapping, pending replies, notification consolidation, and poll-sent tracking.
// File-based state shared between hook handlers and bot daemon.

use crate::config::Config;
use crate::hook::{CONSOLIDATION_WINDOW_SECS, MAX_SESSIONS, PENDING_REPLIES_FILE, POLL_SENT_FILE, RECENT_NOTIF_FILE, SESSIONS_FILE};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

// ── Pending replies ───────────────────────────────────────────────

pub fn save_pending_reply(config: &Config, session_id: &str, user_msg_id: i64) {
    let path = config.hooks_dir.join(PENDING_REPLIES_FILE);
    let _lock = super::lock_file(&config.hooks_dir.join("ghost-code-pending-replies.lock"));
    let mut map: HashMap<String, i64> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    map.insert(session_id.to_string(), user_msg_id);
    std::fs::write(&path, serde_json::to_string(&map).unwrap_or_default()).ok();
}

pub fn has_pending_reply(config: &Config, session_id: &str) -> bool {
    let path = config.hooks_dir.join(PENDING_REPLIES_FILE);
    let _lock = super::lock_file(&config.hooks_dir.join("ghost-code-pending-replies.lock"));
    let map: HashMap<String, i64> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    map.contains_key(session_id)
}

pub fn consume_pending_reply(config: &Config, session_id: &str) -> Option<i64> {
    let path = config.hooks_dir.join(PENDING_REPLIES_FILE);
    let _lock = super::lock_file(&config.hooks_dir.join("ghost-code-pending-replies.lock"));
    let mut map: HashMap<String, i64> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    let reply_to = map.remove(session_id);
    if reply_to.is_some() {
        std::fs::write(&path, serde_json::to_string(&map).unwrap_or_default()).ok();
    }
    reply_to
}

// ── Session mapping ───────────────────────────────────────────────

pub fn save_session_mapping(config: &Config, msg_id: Option<i64>, data: &Value, tty: &str) {
    let (Some(msg_id), Some(session_id)) = (msg_id, data["session_id"].as_str()) else {
        return;
    };
    let project = super::format::project_name(data);
    let short_id = &session_id[..4.min(session_id.len())];
    let tab_title = format!("{project} \u{b7} CC:{short_id}");

    let (tab_index, _) = if !tty.is_empty() {
        super::terminal::detect_tab_by_tty(tty)
    } else {
        super::terminal::set_tab_title(tty, &tab_title);
        super::terminal::detect_tab_index(&tab_title)
    };

    let path = config.hooks_dir.join(SESSIONS_FILE);
    let _lock = super::lock_file(&config.hooks_dir.join("ghost-code-sessions.lock"));
    let mut map: HashMap<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    let cwd = data["cwd"].as_str().unwrap_or("");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    map.insert(
        msg_id.to_string(),
        json!({
            "session_id": session_id,
            "tab_title": tab_title,
            "tab_index": tab_index,
            "project": project,
            "host": super::format::short_hostname(),
            "tty": tty,
            "cwd": cwd,
            "ts": ts,
        }),
    );

    eprintln!(
        "[session] saved mapping msg_id={} session={} project={} tab_index={} tty={}",
        msg_id,
        short_id,
        project,
        tab_index,
        tty,
    );
    if map.len() > MAX_SESSIONS {
        let mut keys: Vec<i64> = map.keys().filter_map(|k| k.parse().ok()).collect();
        keys.sort();
        if let Some(&min_key) = keys.get(keys.len().saturating_sub(MAX_SESSIONS / 2)) {
            map.retain(|k, _| k.parse::<i64>().unwrap_or(0) >= min_key);
        }
    }
    std::fs::write(&path, serde_json::to_string(&map).unwrap_or_default()).ok();
}

pub fn load_session_mapping(config: &Config) -> HashMap<String, Value> {
    let path = config.hooks_dir.join(SESSIONS_FILE);
    let _lock = super::lock_file(&config.hooks_dir.join("ghost-code-sessions.lock"));
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

/// Copy an existing session mapping entry to a new msg_id (e.g. confirm message).
pub fn clone_session_mapping(config: &Config, target_msg_id: i64, entry: &Value) {
    let path = config.hooks_dir.join(SESSIONS_FILE);
    let _lock = super::lock_file(&config.hooks_dir.join("ghost-code-sessions.lock"));
    let mut map: HashMap<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    map.insert(target_msg_id.to_string(), entry.clone());
    if map.len() > MAX_SESSIONS {
        let mut keys: Vec<i64> = map.keys().filter_map(|k| k.parse().ok()).collect();
        keys.sort();
        if let Some(&min_key) = keys.get(keys.len().saturating_sub(MAX_SESSIONS / 2)) {
            map.retain(|k, _| k.parse::<i64>().unwrap_or(0) >= min_key);
        }
    }
    std::fs::write(&path, serde_json::to_string(&map).unwrap_or_default()).ok();
}

// ── Notification consolidation ────────────────────────────────────

fn recent_notif_path(config: &Config) -> PathBuf {
    config.hooks_dir.join(RECENT_NOTIF_FILE)
}

fn load_recent_notifs(config: &Config) -> HashMap<String, Value> {
    std::fs::read_to_string(recent_notif_path(config))
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

pub(super) fn get_recent_notif(config: &Config, session_id: &str) -> Option<Value> {
    let map = load_recent_notifs(config);
    let entry = map.get(session_id)?;
    let ts = entry["ts"].as_u64().unwrap_or(0);
    let now = super::timestamp_ms() as u64 / 1000;
    if now.saturating_sub(ts) > CONSOLIDATION_WINDOW_SECS {
        return None;
    }
    Some(entry.clone())
}

pub(super) fn save_recent_notif(config: &Config, session_id: &str, msg_id: i64, header: &str, parts: &[String]) {
    let mut map = load_recent_notifs(config);
    map.insert(session_id.to_string(), json!({
        "msg_id": msg_id,
        "ts": super::timestamp_ms() / 1000,
        "header": header,
        "parts": parts,
    }));
    std::fs::write(recent_notif_path(config), serde_json::to_string(&map).unwrap_or_default()).ok();
}

pub(super) fn clear_recent_notif(config: &Config, session_id: &str) {
    let mut map = load_recent_notifs(config);
    if map.remove(session_id).is_some() {
        std::fs::write(recent_notif_path(config), serde_json::to_string(&map).unwrap_or_default()).ok();
    }
}

// ── Poll-sent tracking ────────────────────────────────────────────

pub fn save_poll_sent(config: &Config, session_id: &str, msg_id: i64, html: &str) {
    let path = config.hooks_dir.join(POLL_SENT_FILE);
    let _lock = super::lock_file(&config.hooks_dir.join("ghost-code-poll-sent.lock"));
    let mut map: HashMap<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    map.insert(session_id.to_string(), json!({"msg_id": msg_id, "html": html}));
    std::fs::write(&path, serde_json::to_string(&map).unwrap_or_default()).ok();
}

pub(super) fn consume_poll_sent(config: &Config, session_id: &str) -> Option<(i64, String)> {
    let path = config.hooks_dir.join(POLL_SENT_FILE);
    let _lock = super::lock_file(&config.hooks_dir.join("ghost-code-poll-sent.lock"));
    let mut map: HashMap<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    let entry = map.remove(session_id)?;
    std::fs::write(&path, serde_json::to_string(&map).unwrap_or_default()).ok();
    let msg_id = entry["msg_id"].as_i64()?;
    let html = entry["html"].as_str()?.to_string();
    Some((msg_id, html))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ghost-code-session-tests-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn test_config() -> (TempDir, Config) {
        let dir = TempDir::new();
        let config = Config {
            bot_token: String::new(),
            chat_id: String::new(),
            debug: false,
            working_dir: String::new(),
            timeout: 0,
            hooks_dir: dir.0.clone(),
        };
        (dir, config)
    }

    fn session_entry(session_id: &str) -> Value {
        json!({"session_id": session_id, "tab_title": format!("project · CC:{session_id}"), "tab_index": 1, "project": "project", "host": "test-host", "tty": "/dev/pts/1", "cwd": "/tmp/project", "ts": 123})
    }

    #[test]
    fn clone_session_mapping_creates_target_entry_with_same_data() {
        let (_dir, config) = test_config();
        let entry = session_entry("session-a");
        let path = config.hooks_dir.join(SESSIONS_FILE);

        std::fs::write(&path, json!({ "10": entry.clone() }).to_string()).unwrap();
        clone_session_mapping(&config, 20, &entry);

        let map = load_session_mapping(&config);
        assert_eq!(map.get("10"), Some(&entry));
        assert_eq!(map.get("20"), Some(&entry));
    }

    #[test]
    fn clone_session_mapping_overwrites_same_target_cleanly() {
        let (_dir, config) = test_config();
        let first = session_entry("session-a");
        let second = session_entry("session-b");

        clone_session_mapping(&config, 42, &first);
        clone_session_mapping(&config, 42, &second);

        let map = load_session_mapping(&config);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("42"), Some(&second));
    }

    #[test]
    fn clone_session_mapping_prunes_oldest_entries_when_limit_is_exceeded() {
        let (_dir, config) = test_config();

        for msg_id in 0..=(MAX_SESSIONS as i64) {
            clone_session_mapping(&config, msg_id, &session_entry(&format!("session-{msg_id}")));
        }

        let map = load_session_mapping(&config);
        let expected_min = (MAX_SESSIONS + 1 - MAX_SESSIONS / 2) as i64;
        assert_eq!(map.len(), MAX_SESSIONS / 2);
        assert!(!map.contains_key("0"));
        assert!(map.contains_key(&expected_min.to_string()));
        assert!(map.contains_key(&MAX_SESSIONS.to_string()));
    }

    #[test]
    fn clone_session_mapping_creates_sessions_file_from_scratch() {
        let (_dir, config) = test_config();
        let entry = session_entry("session-a");
        let path = config.hooks_dir.join(SESSIONS_FILE);

        assert!(!path.exists());
        clone_session_mapping(&config, 7, &entry);

        assert!(path.exists());
        assert_eq!(load_session_mapping(&config).get("7"), Some(&entry));
    }
}
