// Telegram bot command handlers: /help, /sessions, /dir, /status, /stop.
// Called from messages.rs when a message starts with '/'.

use crate::config::Config;
use crate::hook;
use crate::telegram;
use serde_json::Value;
use std::sync::atomic::Ordering;

pub(super) fn handle_command(config: &mut Config, text: &str) -> bool {
    let (cmd, arg) = text.split_once(' ').unwrap_or((text, ""));
    let cmd = cmd.to_lowercase();
    eprintln!("{} [cmd] {cmd} arg={:?}", super::ts(), if arg.is_empty() { "(none)" } else { arg });

    match cmd.as_str() {
        "/start" | "/help" => {
            let host = hook::short_hostname();
            telegram::send_text(
                &config.bot_token,
                &config.chat_id,
                &format!(
                    "Ghost Code ({host})\n\n\
                     Send a message to chat with Claude (streaming).\n\
                     Reply to a notification to inject into terminal.\n\n\
                     /help - this help\n\
                     /sessions - list active sessions\n\
                     /dir [path] - get/set working directory\n\
                     /status - bot status\n\
                     /stop - stop the bot"
                ),
            );
            true
        }
        "/dir" => {
            if arg.is_empty() {
                telegram::send_text(
                    &config.bot_token,
                    &config.chat_id,
                    &format!("Working directory: {}", config.working_dir),
                );
            } else {
                let p = super::messages::expand_tilde(arg);
                if p.is_dir() {
                    config.working_dir = p.to_string_lossy().to_string();
                    eprintln!("{} [cmd] working_dir changed to: {}", super::ts(), config.working_dir);
                    telegram::send_text(
                        &config.bot_token,
                        &config.chat_id,
                        &format!("Working directory: {}", config.working_dir),
                    );
                } else {
                    telegram::send_text(
                        &config.bot_token,
                        &config.chat_id,
                        &format!("Not a directory: {arg}"),
                    );
                }
            }
            true
        }
        "/status" => {
            let host = hook::short_hostname();
            telegram::send_text(
                &config.bot_token,
                &config.chat_id,
                &format!(
                    "Running on {host} (PID {})\nDir: {}",
                    std::process::id(),
                    config.working_dir
                ),
            );
            true
        }
        "/sessions" => {
            handle_sessions(config);
            true
        }
        "/stop" => {
            telegram::send_text(&config.bot_token, &config.chat_id, "Bot stopping...");
            super::SHUTDOWN.store(true, Ordering::SeqCst);
            true
        }
        _ => false,
    }
}

fn handle_sessions(config: &Config) {
    let map = hook::load_session_mapping(config);

    let mut sessions: std::collections::HashMap<String, (i64, &Value)> =
        std::collections::HashMap::new();
    for (msg_id_str, entry) in &map {
        let sid = entry["session_id"].as_str().unwrap_or("").to_string();
        let mid: i64 = msg_id_str.parse().unwrap_or(0);
        if sid.is_empty() {
            continue;
        }
        let existing = sessions.get(&sid).map(|(m, _)| *m).unwrap_or(0);
        if mid > existing {
            sessions.insert(sid, (mid, entry));
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let one_hour = 3600u64;
    let mut active: Vec<_> = sessions
        .into_iter()
        .filter(|(_sid, (_mid, entry))| {
            let ts = entry["ts"].as_u64().unwrap_or(0);
            if ts == 0 || now.saturating_sub(ts) > one_hour {
                return false;
            }
            let tty = entry["tty"].as_str().unwrap_or("");
            !tty.is_empty() && std::path::Path::new(&format!("/dev/{tty}")).exists()
        })
        .collect();

    if active.is_empty() {
        let host = hook::short_hostname();
        telegram::send_text(&config.bot_token, &config.chat_id, &format!("No active sessions on {host}."));
        return;
    }

    active.sort_by(|a, b| {
        let ts_a = a.1 .1["ts"].as_u64().unwrap_or(0);
        let ts_b = b.1 .1["ts"].as_u64().unwrap_or(0);
        ts_b.cmp(&ts_a).then(b.1 .0.cmp(&a.1 .0))
    });

    let mut lines = Vec::new();
    for (sid, (_mid, entry)) in &active {
        let project = entry["project"].as_str().unwrap_or("?");
        let cwd = entry["cwd"].as_str().unwrap_or("");
        let tty = entry["tty"].as_str().unwrap_or("");
        let short = &sid[..8.min(sid.len())];
        let ts = entry["ts"].as_u64().unwrap_or(0);

        let alive = !tty.is_empty()
            && std::path::Path::new(&format!("/dev/{tty}")).exists();
        let status = if alive { "\u{1f7e2}" } else { "\u{26aa}" };

        let ago = if ts > 0 && now > ts {
            let delta = now - ts;
            if delta < 60 {
                format!("{}s ago", delta)
            } else if delta < 3600 {
                format!("{}m ago", delta / 60)
            } else if delta < 86400 {
                format!("{}h ago", delta / 3600)
            } else {
                format!("{}d ago", delta / 86400)
            }
        } else {
            "just now".to_string()
        };

        let display_cwd = if cwd.is_empty() {
            String::new()
        } else {
            let home = std::env::var("HOME").unwrap_or_default();
            let short_path = if !home.is_empty() && cwd.starts_with(&home) {
                format!("~{}", &cwd[home.len()..])
            } else {
                cwd.to_string()
            };
            format!("\n     <code>{}</code>", telegram::escape_html(&short_path))
        };

        let summary = super::polling::find_session_jsonl(sid)
            .and_then(|p| super::polling::read_jsonl_tail_summary(&p))
            .unwrap_or_default();
        let display_summary = if summary.is_empty() {
            String::new()
        } else {
            format!("\n     <i>{}</i>", telegram::escape_html(&summary))
        };

        lines.push(format!(
            "{status} <b>{}</b>  <code>{short}</code>  {ago}{display_cwd}{display_summary}",
            telegram::escape_html(project),
        ));
    }

    let host = hook::short_hostname();
    let msg = format!(
        "\u{1f4cb} <b>Sessions \u{b7} {}</b> ({})\n\n{}",
        telegram::escape_html(&host),
        active.len(),
        lines.join("\n\n"),
    );
    telegram::send_html(&config.bot_token, &config.chat_id, &msg, None, None);
}
