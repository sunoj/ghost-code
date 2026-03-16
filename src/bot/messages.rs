// Telegram message handler: routing, dedup, injection into Ghostty.
// Handles both session replies (inject to terminal) and direct messages (stream claude).

use crate::config::Config;
use crate::hook;
use crate::telegram;
use serde_json::{json, Value};
use std::path::PathBuf;
pub(super) fn handle_message(config: &mut Config, msg: &Value) {
    let chat_id = msg["chat"]["id"]
        .as_i64()
        .map(|v| v.to_string())
        .unwrap_or_default();
    if chat_id != config.chat_id {
        eprintln!("{} [msg] ignored: chat_id={chat_id} (expected {})", super::ts(), config.chat_id);
        return;
    }

    let text = msg["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        eprintln!("{} [msg] ignored: empty text (msg_id={})", super::ts(), msg["message_id"]);
        return;
    }

    let from = msg["from"]["first_name"].as_str().unwrap_or("?");
    let msg_id = msg["message_id"].as_i64().unwrap_or(0);
    eprintln!("{} [msg] from={from} msg_id={msg_id} len={}: {}", super::ts(), text.len(), truncate(text, 120));

    // Deduplicate
    {
        let mut guard = super::PROCESSED_MSGS.lock().unwrap();
        let seen = guard.get_or_insert_with(Vec::new);
        if seen.contains(&msg_id) {
            eprintln!("{} [msg] duplicate msg_id={msg_id}, skipping", super::ts());
            return;
        }
        seen.push(msg_id);
        if seen.len() > super::PROCESSED_MSGS_MAX {
            seen.drain(..seen.len() - super::PROCESSED_MSGS_MAX);
        }
    }

    super::refresh_caffeinate();

    if text.starts_with('/') && super::commands::handle_command(config, text) {
        return;
    }

    // Check if replying to a notification/stop message
    let reply_to_id = msg["reply_to_message"]["message_id"].as_i64();
    let mapping = reply_to_id.and_then(|reply_id| {
        eprintln!("{} [msg] reply_to msg_id={reply_id}, looking up session mapping", super::ts());
        let map = hook::load_session_mapping(config);
        map.get(&reply_id.to_string()).cloned()
    });

    let my_host = hook::short_hostname();
    let mapping = mapping.filter(|entry| {
        let entry_host = entry["host"].as_str().unwrap_or("");
        if !entry_host.is_empty() && entry_host != my_host {
            eprintln!("{} [inject] skipping: session belongs to {entry_host}, not {my_host}", super::ts());
            return false;
        }
        true
    });

    if reply_to_id.is_some() && mapping.is_none() {
        eprintln!("{} [msg] reply_to msg not in our mapping, ignoring (likely another device's session)", super::ts());
        return;
    }

    // Reply to any mapped message -> inject into Ghostty
    if let Some(ref entry) = mapping {
        handle_injection(config, msg, text, entry);
        return;
    }

    // Direct message -> stream claude response
    eprintln!("{} [claude] starting: dir={} timeout={}s prompt={}", super::ts(), config.working_dir, config.timeout, truncate(text, 80));
    let _ = telegram::call(
        &config.bot_token,
        "sendChatAction",
        &json!({"chat_id": &config.chat_id, "action": "typing"}),
        15,
    );

    let bot_token = config.bot_token.clone();
    let tg_chat_id = config.chat_id.clone();
    let working_dir = config.working_dir.clone();
    let timeout = config.timeout;
    let message = text.to_string();

    std::thread::spawn(move || {
        super::streaming::run_claude_streaming(&message, &working_dir, timeout, &bot_token, &tg_chat_id);
    });
}

fn handle_injection(config: &Config, msg: &Value, text: &str, entry: &Value) {
    let tab_title = entry["tab_title"].as_str().unwrap_or("");
    let session = entry["session_id"].as_str().unwrap_or("?");
    let project = entry["project"].as_str().unwrap_or("?");
    let tty = entry["tty"].as_str().unwrap_or("");
    let tab_index = entry["tab_index"].as_i64().unwrap_or(0);
    let user_msg_id = msg["message_id"].as_i64();

    let _ = telegram::call(
        &config.bot_token,
        "sendChatAction",
        &json!({"chat_id": &config.chat_id, "action": "typing"}),
        15,
    );

    eprintln!(
        "{} [inject] session={session} project={project} tab_index={tab_index} tty={tty}",
        super::ts()
    );

    let confirm = format!("\u{27a1}\u{fe0f} <b>{}</b>", telegram::escape_html(project));
    let confirm_msg_id = telegram::send_html_silent(&config.bot_token, &config.chat_id, &confirm, user_msg_id);

    // Save session mapping for the confirm message so replies to it find the session.
    if let Some(cmid) = confirm_msg_id {
        hook::clone_session_mapping(config, cmid, entry);
    }

    match hook::atomic_inject(tty, text, tab_title) {
        Ok(_) => {
            eprintln!("{} [inject] ok, text injected ({} chars)", super::ts(), text.len());
            if let Some(mid) = user_msg_id {
                hook::save_pending_reply(config, session, mid);
                eprintln!("{} [inject] saved pending reply: session={session} msg_id={mid}", super::ts());
                let cfg = config.clone();
                let sid = session.to_string();
                let proj = project.to_string();
                let cmid = confirm_msg_id.unwrap_or(0);
                std::thread::spawn(move || {
                    super::polling::poll_session_response(&cfg, &sid, &proj, mid, cmid);
                });
            }
        }
        Err(e) => {
            let is_locked = e == hook::SCREEN_LOCKED_ERR;
            eprintln!("{} [inject] failed: {e} (screen_locked={is_locked})", super::ts());
            let (icon, detail) = if is_locked {
                ("\u{1f512}", "Screen locked — unlock and resend")
            } else {
                ("\u{274c}", e.as_str())
            };
            if let Some(cmid) = confirm_msg_id {
                let err_html = format!(
                    "{icon} <b>{}</b>\n\n{}",
                    telegram::escape_html(project),
                    telegram::escape_html(detail),
                );
                telegram::edit_html(&config.bot_token, &config.chat_id, cmid, &err_html);
            } else {
                telegram::send_text(
                    &config.bot_token,
                    &config.chat_id,
                    &format!("{icon} {detail}"),
                );
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let end = s.char_indices().take_while(|(i, _)| *i < max).last().map(|(i, c)| i + c.len_utf8()).unwrap_or(max);
        format!("{}...", &s[..end])
    }
}

pub(super) fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(rest)
    } else if path == "~" {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
    } else {
        PathBuf::from(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_long() {
        let result = truncate("hello world", 5);
        assert_eq!(result, "hello...");
    }

    #[test]
    fn truncate_unicode() {
        let result = truncate("héllo wörld", 5);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 12); // 5 bytes of content + "..."
    }

    #[test]
    fn expand_tilde_home() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_tilde("~/foo"), PathBuf::from(&home).join("foo"));
    }

    #[test]
    fn expand_tilde_bare() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
    }

    #[test]
    fn expand_tilde_absolute() {
        assert_eq!(expand_tilde("/usr/bin"), PathBuf::from("/usr/bin"));
    }
}
