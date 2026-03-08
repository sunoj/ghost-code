// Hook handlers for Claude Code events.
// Fast path: spool event data to disk, return immediately to Claude Code.
// Async processing happens in the bot daemon (see bot/).

pub mod format;
pub mod process;
pub mod session;
pub mod terminal;

// Re-export public API so callers use hook::function_name unchanged.
pub use format::{project_name, short_hostname};
pub use process::{process_notification, process_pre_tool_use, process_stop};
pub use session::{
    clone_session_mapping, consume_pending_reply, has_pending_reply, load_session_mapping,
    save_pending_reply, save_poll_sent, save_session_mapping,
};
pub use terminal::{detect_tab_by_tty, detect_tab_index};

use crate::config::Config;
use serde_json::{json, Value};
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

pub const SPOOL_DIR_NAME: &str = "ghost-code-spool";
pub(crate) const SESSIONS_FILE: &str = "ghost-code-sessions.json";
pub(crate) const MAX_SESSIONS: usize = 200;
pub(crate) const PENDING_REPLIES_FILE: &str = "ghost-code-pending-replies.json";
pub(crate) const RECENT_NOTIF_FILE: &str = "ghost-code-recent-notif.json";
pub(crate) const POLL_SENT_FILE: &str = "ghost-code-poll-sent.json";
pub(crate) const CONSOLIDATION_WINDOW_SECS: u64 = 300;

// ── Fast hook handlers (called by main.rs, must return ASAP) ──────

/// Spool a stop or notification event to disk and return immediately.
pub fn spool_and_exit(event_type: &str) {
    let input = read_stdin_raw();
    let tty = parent_tty();
    write_spool(event_type, &input, &tty, None);
}

/// Handle pre-tool-use: spool the event, then poll for daemon's response.
pub fn handle_pre_tool_use(config: &Config) {
    let input = read_stdin_raw();
    let data: Value = serde_json::from_str(&input).unwrap_or(json!({}));
    let tool_name = data["tool_name"].as_str().unwrap_or("");
    let session = data["session_id"].as_str().unwrap_or("?");

    if config.approval_tools.is_empty()
        || !config.approval_tools.iter().any(|t| t == tool_name)
    {
        eprintln!("[hook:pre-tool-use] session={session} tool={tool_name} -> skipped");
        return;
    }

    eprintln!("[hook:pre-tool-use] session={session} tool={tool_name} -> requesting approval");
    let request_id = generate_request_id();
    let tty = parent_tty();
    write_spool("pre-tool-use", &input, &tty, Some(&request_id));

    let response_file = config
        .hooks_dir
        .join(format!("ghost-code-response-{request_id}.json"));
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(config.approval_timeout);

    loop {
        if std::time::Instant::now() > deadline {
            eprintln!("[hook:pre-tool-use] timeout after {}s", config.approval_timeout);
            println!("{}", json!({"decision": "deny", "reason": "Telegram approval timeout"}));
            return;
        }
        if let Ok(content) = std::fs::read_to_string(&response_file) {
            let _ = std::fs::remove_file(&response_file);
            let decision = serde_json::from_str::<Value>(&content)
                .ok()
                .and_then(|v| v["response"].as_str().map(String::from))
                .unwrap_or_else(|| "deny".to_string());
            if decision == "allow" {
                println!("{}", json!({"decision": "allow"}));
            } else {
                println!("{}", json!({"decision": "deny", "reason": "Denied via Telegram"}));
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

// ── Spool I/O ─────────────────────────────────────────────────────

fn write_spool(event_type: &str, raw_data: &str, tty: &str, request_id: Option<&str>) {
    let spool_dir = hooks_dir().join(SPOOL_DIR_NAME);
    std::fs::create_dir_all(&spool_dir).ok();
    let ts = timestamp_ms();
    let mut spool = json!({
        "type": event_type,
        "data_raw": raw_data,
        "timestamp_ms": ts,
        "tty": tty,
    });
    if let Some(rid) = request_id {
        spool["request_id"] = json!(rid);
    }
    let pid = std::process::id();
    let filename = format!("{ts}-{pid}-{event_type}.json");
    let tmp = spool_dir.join(format!(".tmp.{filename}"));
    let final_path = spool_dir.join(filename);
    if std::fs::write(&tmp, spool.to_string()).is_ok() {
        std::fs::rename(&tmp, &final_path).ok();
    }
}

fn read_stdin_raw() -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).ok();
        tx.send(input).ok();
    });
    rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap_or_default()
}

pub fn hooks_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string())).join(".claude/hooks")
}

pub fn spool_dir() -> PathBuf {
    hooks_dir().join(SPOOL_DIR_NAME)
}

pub(crate) fn timestamp_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn generate_request_id() -> String {
    let millis = timestamp_ms();
    let mut buf = [0u8; 4];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    let rand = u32::from_le_bytes(buf);
    format!("{millis}-{rand:08x}")
}

fn parent_tty() -> String {
    let ppid = unsafe { libc::getppid() };
    let tty = std::process::Command::new("ps")
        .args(["-p", &ppid.to_string(), "-o", "tty="])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if tty.contains("..") || !tty.chars().all(|c| c.is_alphanumeric() || c == '/') {
        return String::new();
    }
    tty
}

// ── Utilities ─────────────────────────────────────────────────────

pub(crate) fn lock_file(path: &std::path::Path) -> Option<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(path)
        .ok()?;
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    Some(file)
}

pub(crate) fn is_noise_notification(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("waiting for your input")
        || lower.contains("waiting for input")
        || lower.contains("needs your attention")
        || lower.contains("quota recovered")
        || lower.contains("session resumed")
}

pub(crate) fn debug_log(config: &Config, hook_type: &str, data: &Value) {
    if !config.debug {
        return;
    }
    let log_file = config.hooks_dir.join("ghost-code.debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(log_file)
    {
        use std::io::Write;
        writeln!(f, "\n=== {hook_type} ===").ok();
        writeln!(f, "{}", serde_json::to_string_pretty(data).unwrap_or_default()).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_noise_waiting_for_input() {
        assert!(is_noise_notification("Waiting for your input"));
        assert!(is_noise_notification("WAITING FOR INPUT"));
        assert!(is_noise_notification("Session resumed after pause"));
        assert!(is_noise_notification("Quota recovered, resuming"));
    }

    #[test]
    fn is_noise_real_notification() {
        assert!(!is_noise_notification("Permission needed: cargo build"));
        assert!(!is_noise_notification("Task completed successfully"));
        assert!(!is_noise_notification("Error: compilation failed"));
    }
}
