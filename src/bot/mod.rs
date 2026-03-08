// Telegram bot daemon: main loop, signal handling, spool processing.
// Submodules handle specific concerns (commands, messages, callbacks, etc.).

mod callbacks;
mod commands;
mod messages;
mod polling;
mod status;
mod streaming;

use crate::config::Config;
use crate::hook;
use crate::telegram;
use serde_json::{json, Value};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub(crate) fn ts() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    let ms = now.subsec_millis();
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static CAFFEINATE_PID: Mutex<Option<u32>> = Mutex::new(None);
static PROCESSED_MSGS: Mutex<Option<Vec<i64>>> = Mutex::new(None);
const PROCESSED_MSGS_MAX: usize = 50;

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
enum PollAction {
    Retry(std::time::Duration),
    Notify(std::time::Duration),
    Continue,
    Break,
}

extern "C" fn handle_signal(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

fn try_lock_pid(path: &std::path::Path) -> Option<std::fs::File> {
    use std::io::Write;
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(path)
        .ok()?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return None;
    }
    file.set_len(0).ok();
    (&file).write_all(std::process::id().to_string().as_bytes()).ok();
    Some(file)
}

fn handle_poll_success(consecutive_409: &mut u32) -> PollAction {
    *consecutive_409 = 0;
    PollAction::Continue
}

fn handle_poll_error(error: &str, consecutive_409: &mut u32) -> PollAction {
    if error.contains("409") {
        *consecutive_409 += 1;
        if *consecutive_409 == 5 {
            PollAction::Notify(std::time::Duration::from_secs(35))
        } else {
            PollAction::Retry(std::time::Duration::from_secs(35))
        }
    } else {
        PollAction::Retry(std::time::Duration::from_secs(5))
    }
}

pub fn run(mut config: Config) {
    let pid_file = config.hooks_dir.join("ghost-code.pid");
    let response_file = config.hooks_dir.join("ghost-code-response.json");

    let _pid_lock = match try_lock_pid(&pid_file) {
        Some(f) => f,
        None => {
            eprintln!("{} [bot] already running, exiting", ts());
            return;
        }
    };

    unsafe {
        libc::signal(libc::SIGINT, handle_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle_signal as *const () as libc::sighandler_t);
    }

    eprintln!("{} [bot] started v{} (PID {}) on {}", ts(), env!("CARGO_PKG_VERSION"), std::process::id(), hook::short_hostname());
    eprintln!("{} [bot] chat_id={}", ts(), config.chat_id);
    eprintln!("{} [bot] working_dir={}", ts(), config.working_dir);
    eprintln!("{} [bot] timeout={}s", ts(), config.timeout);
    if !config.approval_tools.is_empty() {
        eprintln!("{} [bot] approval_tools={:?} (timeout={}s)", ts(), config.approval_tools, config.approval_timeout);
    }
    eprintln!("{} [bot] debug={}", ts(), config.debug);

    let _ = telegram::call(&config.bot_token, "setMyCommands", &json!({
        "commands": [
            {"command": "help", "description": "Show help"},
            {"command": "sessions", "description": "List active sessions"},
            {"command": "dir", "description": "Get/set working directory"},
            {"command": "status", "description": "Show bot status"},
            {"command": "stop", "description": "Stop the bot"},
        ]
    }), 10);

    let status_file = config.hooks_dir.join("ghost-code-status.txt");
    status::write_status(&status_file, &config, true);

    let spool_config = config.clone();
    let status_config = config.clone();
    let status_path = status_file.clone();
    std::thread::spawn(move || {
        let spool_dir = hook::spool_dir();
        std::fs::create_dir_all(&spool_dir).ok();
        eprintln!("{} [spool] processor started: {}", ts(), spool_dir.display());
        let mut tick = 0u64;
        loop {
            if SHUTDOWN.load(Ordering::SeqCst) {
                break;
            }
            process_spool_files(&spool_config, &spool_dir);
            tick += 1;
            if tick % 25 == 0 {
                status::write_status(&status_path, &status_config, true);
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        eprintln!("{} [spool] processor stopped", ts());
    });

    // Wait for stale getUpdates connections (from killed daemons) to expire.
    for attempt in 1..=3 {
        match telegram::call(&config.bot_token, "getUpdates", &json!({"offset": -1, "timeout": 0, "limit": 1}), 10) {
            Ok(v) => {
                if let Some(arr) = v["result"].as_array() {
                    if let Some(last) = arr.last() {
                        let last_id = last["update_id"].as_i64().unwrap_or(0);
                        eprintln!("{} [poll] flushed pending updates (last_id={last_id})", ts());
                    }
                }
                break;
            }
            Err(e) if e.contains("409") => {
                eprintln!("{} [poll] startup 409 (attempt {attempt}/3), waiting 35s for stale connection to expire", ts());
                std::thread::sleep(std::time::Duration::from_secs(35));
            }
            Err(_) => break,
        }
    }

    let mut offset: i64 = 0;
    let mut consecutive_409: u32 = 0;
    loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            break;
        }

        let updates = match telegram::call(
            &config.bot_token,
            "getUpdates",
            &json!({
                "offset": offset,
                "timeout": 30,
                "allowed_updates": ["message", "callback_query"],
            }),
            60,
        ) {
            Ok(v) => {
                handle_poll_success(&mut consecutive_409);
                v
            }
            Err(e) => {
                match handle_poll_error(&e, &mut consecutive_409) {
                    PollAction::Retry(delay) => {
                        if e.contains("409") {
                            eprintln!("{} [poll] error: {e}, retrying in 35s (409 #{consecutive_409})", ts());
                        } else {
                            eprintln!("{} [poll] error: {e}, retrying in 5s", ts());
                        }
                        std::thread::sleep(delay);
                    }
                    PollAction::Notify(delay) => {
                        let host = hook::short_hostname();
                        let msg = format!(
                            "\u{26a0}\u{fe0f} <b>ghost-code@{}</b>\n\n\
                            Another bot instance is polling with the same token. \
                            Each device needs its own bot token from @BotFather. \
                            Receiving messages is disabled until the conflict is resolved.",
                            telegram::escape_html(&host),
                        );
                        telegram::send_html(&config.bot_token, &config.chat_id, &msg, None, None);
                        eprintln!("{} [poll] error: {e}, retrying in 35s (409 #{consecutive_409})", ts());
                        std::thread::sleep(delay);
                    }
                    PollAction::Continue => continue,
                    PollAction::Break => break,
                }
                continue;
            }
        };

        let results = updates["result"].as_array();
        let count = results.map(|a| a.len()).unwrap_or(0);
        if count > 0 {
            eprintln!("{} [poll] received {} update(s)", ts(), count);
        }

        for update in results.into_iter().flatten() {
            if let Some(id) = update["update_id"].as_i64() {
                offset = id + 1;
            }

            if update.get("message").is_some() {
                messages::handle_message(&mut config, &update["message"]);
            } else if update.get("callback_query").is_some() {
                callbacks::handle_callback(&config, &update["callback_query"], &response_file);
            } else {
                eprintln!("{} [poll] unknown update type: {}", ts(), serde_json::to_string(update).unwrap_or_default());
            }
        }
    }

    kill_caffeinate();
    let _ = std::fs::remove_file(&pid_file);
    let _ = std::fs::remove_file(&status_file);
    eprintln!("{} [bot] stopped", ts());
}

/// Process pending spool files written by hook handlers.
fn process_spool_files(config: &Config, spool_dir: &std::path::Path) {
    let entries = match std::fs::read_dir(spool_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.ends_with(".json") && !name.starts_with(".tmp.")
        })
        .collect();

    if files.is_empty() {
        return;
    }

    files.sort_by_key(|e| e.file_name());

    for entry in files {
        let path = entry.path();
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let spool: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => {
                let stale = entry.metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .map(|d| d.as_secs() > 60)
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
        };

        let _ = std::fs::remove_file(&path);

        let event_type = spool["type"].as_str().unwrap_or("");
        let data: Value = spool["data_raw"]
            .as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(json!({}));
        let tty = spool["tty"].as_str().unwrap_or("");

        eprintln!("{} [spool] processing: type={event_type}", ts());

        match event_type {
            "stop" => hook::process_stop(config, &data, tty),
            "notification" => {
                if polling::is_plan_notification(&data) {
                    polling::process_plan_notification(config, &data, tty);
                } else {
                    hook::process_notification(config, &data, tty);
                }
            }
            "pre-tool-use" => {
                let request_id = spool["request_id"].as_str().unwrap_or("");
                hook::process_pre_tool_use(config, &data, request_id);
            }
            _ => eprintln!("{} [spool] unknown event type: {event_type}", ts()),
        }
    }
}

/// Spawn a 1-hour caffeinate to prevent sleep. Kills any previous instance.
pub(crate) fn refresh_caffeinate() {
    let mut pid = CAFFEINATE_PID.lock().unwrap();
    if let Some(old) = pid.take() {
        unsafe { libc::kill(old as i32, libc::SIGTERM) };
    }
    match Command::new("caffeinate")
        .args(["-d", "-i", "-s", "-t", "3600"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => {
            eprintln!("{} [caffeinate] started pid={} (1h)", ts(), child.id());
            *pid = Some(child.id());
        }
        Err(e) => eprintln!("{} [caffeinate] failed: {e}", ts()),
    }
}

fn kill_caffeinate() {
    if let Some(old) = CAFFEINATE_PID.lock().unwrap().take() {
        unsafe { libc::kill(old as i32, libc::SIGTERM) };
        eprintln!("{} [caffeinate] killed pid={old}", ts());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_retry_for_non_409_error_without_changing_counter() {
        let mut consecutive_409 = 2;

        let action = handle_poll_error("timeout", &mut consecutive_409);

        assert_eq!(action, PollAction::Retry(std::time::Duration::from_secs(5)));
        assert_eq!(consecutive_409, 2);
    }

    #[test]
    fn returns_retry_for_first_409_and_increments_counter() {
        let mut consecutive_409 = 0;

        let action = handle_poll_error("409 conflict", &mut consecutive_409);

        assert_eq!(action, PollAction::Retry(std::time::Duration::from_secs(35)));
        assert_eq!(consecutive_409, 1);
    }

    #[test]
    fn returns_notify_on_fifth_consecutive_409() {
        let mut consecutive_409 = 4;

        let action = handle_poll_error("409 conflict", &mut consecutive_409);

        assert_eq!(action, PollAction::Notify(std::time::Duration::from_secs(35)));
        assert_eq!(consecutive_409, 5);
    }

    #[test]
    fn resets_409_counter_after_successful_poll() {
        let mut consecutive_409 = 4;

        let action = handle_poll_success(&mut consecutive_409);

        assert_eq!(action, PollAction::Continue);
        assert_eq!(consecutive_409, 0);
    }
}
