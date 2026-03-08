// JSONL response polling and plan approval notifications.
// After injection, polls CC's conversation JSONL to capture assistant responses.

use crate::config::Config;
use crate::hook;
use crate::telegram;
use serde_json::{json, Value};
use std::io::{Read, Seek};
use std::path::PathBuf;

/// After injection, poll CC's conversation JSONL to capture the assistant response.
pub(super) fn poll_session_response(config: &Config, session_id: &str, project: &str, _user_msg_id: i64, confirm_msg_id: i64) {
    let Some(jsonl_path) = find_session_jsonl(session_id) else {
        eprintln!("{} [poll] session JSONL not found: {session_id}", super::ts());
        return;
    };
    let start_size = std::fs::metadata(&jsonl_path).map(|m| m.len()).unwrap_or(0);
    eprintln!("{} [poll] watching {jsonl_path:?} from offset {start_size}", super::ts());

    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_secs(180);
    let early_send = std::time::Duration::from_secs(60);
    let mut offset = start_size;
    let mut new_lines: Vec<Value> = Vec::new();
    let mut sent = false;
    let mut last_typing = std::time::Instant::now();

    let _ = telegram::call(
        &config.bot_token,
        "sendChatAction",
        &json!({"chat_id": &config.chat_id, "action": "typing"}),
        15,
    );

    loop {
        if std::time::Instant::now() > deadline {
            eprintln!("{} [poll] timeout for session {session_id}", super::ts());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1500));

        if !sent && !hook::has_pending_reply(config, session_id) {
            eprintln!("{} [poll] pending_reply consumed (stop hook handled), exiting", super::ts());
            break;
        }

        if !sent && last_typing.elapsed().as_secs() >= 4 {
            let _ = telegram::call(
                &config.bot_token,
                "sendChatAction",
                &json!({"chat_id": &config.chat_id, "action": "typing"}),
                15,
            );
            last_typing = std::time::Instant::now();
        }

        let cur_size = std::fs::metadata(&jsonl_path).map(|m| m.len()).unwrap_or(0);
        if cur_size <= offset {
            if !sent && start.elapsed() >= early_send && hook::has_pending_reply(config, session_id) {
                if let Some(response) = last_assistant_text(&new_lines) {
                    send_poll_response(config, session_id, project, confirm_msg_id, &response, true);
                    sent = true;
                }
            }
            continue;
        }

        let Ok(mut file) = std::fs::File::open(&jsonl_path) else { break };
        if file.seek(std::io::SeekFrom::Start(offset)).is_err() {
            break;
        }
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            break;
        }
        offset = cur_size;

        let text = String::from_utf8_lossy(&buf);
        for line in text.lines() {
            if let Ok(entry) = serde_json::from_str::<Value>(line) {
                new_lines.push(entry);
            }
        }

        let turn_done = new_lines.iter().any(|e| {
            e["type"].as_str() == Some("system")
                && e["subtype"].as_str() == Some("turn_duration")
        });

        if turn_done {
            if !sent {
                if hook::has_pending_reply(config, session_id) {
                    if let Some(response) = last_assistant_text(&new_lines) {
                        send_poll_response(config, session_id, project, confirm_msg_id, &response, false);
                    }
                } else {
                    eprintln!("{} [poll] stop hook already handled, skipping", super::ts());
                }
            } else {
                hook::consume_pending_reply(config, session_id);
            }
            break;
        }

        if !sent && start.elapsed() >= early_send && hook::has_pending_reply(config, session_id) {
            if let Some(response) = last_assistant_text(&new_lines) {
                send_poll_response(config, session_id, project, confirm_msg_id, &response, true);
                sent = true;
            }
        }
    }
}

fn last_assistant_text(entries: &[Value]) -> Option<String> {
    entries
        .iter()
        .rev()
        .filter(|e| e["type"].as_str() == Some("assistant"))
        .find_map(|e| extract_assistant_text(&e["message"]))
        .filter(|t| t.len() > 5)
}

fn send_poll_response(
    config: &Config,
    session_id: &str,
    project: &str,
    confirm_msg_id: i64,
    response: &str,
    partial: bool,
) {
    let tag = if partial { "\u{23f3}" } else { "\u{1f4ac}" };
    let suffix = if partial { " <i>(in progress\u{2026})</i>" } else { "" };
    eprintln!(
        "{} [poll] {}response ({} chars) → edit msg {}",
        super::ts(),
        if partial { "partial " } else { "" },
        response.len(),
        confirm_msg_id,
    );
    hook::consume_pending_reply(config, session_id);
    let html = format!(
        "{tag} <b>{}</b>{suffix}\n\n{}",
        telegram::escape_html(project),
        telegram::markdown_to_html(response),
    );
    if confirm_msg_id > 0 {
        telegram::edit_html(&config.bot_token, &config.chat_id, confirm_msg_id, &html);
        hook::save_poll_sent(config, session_id, confirm_msg_id, &html);
    } else {
        let msg_id = telegram::send_html(&config.bot_token, &config.chat_id, &html, None, None);
        if let Some(mid) = msg_id {
            hook::save_poll_sent(config, session_id, mid, &html);
        }
    }
}

// ── Plan approval notification ────────────────────────────────────

pub(super) fn is_plan_notification(data: &Value) -> bool {
    let msg = data["message"].as_str().unwrap_or("");
    let lower = msg.to_lowercase();
    lower.contains("approval") && lower.contains("plan")
}

pub(super) fn process_plan_notification(config: &Config, data: &Value, tty: &str) {
    let session_id = data["session_id"].as_str().unwrap_or("");
    let project = hook::project_name(data);
    eprintln!("{} [plan] approval needed: session={session_id} project={project}", super::ts());

    let plan_text = find_session_jsonl(session_id)
        .and_then(|p| read_last_assistant_from_jsonl(&p))
        .unwrap_or_else(|| "Plan details not available.".to_string());

    let plan_html = telegram::markdown_to_html(&plan_text)
        .replace("<blockquote>", "")
        .replace("</blockquote>", "");
    let plan_trimmed = if plan_html.len() > 3900 {
        let end = telegram::char_floor(&plan_html, 3900);
        format!("{}…", &plan_html[..end])
    } else {
        plan_html
    };
    let msg = format!(
        "\u{1f4cb} <b>Plan \u{b7} {}</b>\n\n<blockquote expandable>{}</blockquote>",
        telegram::escape_html(&project),
        plan_trimmed,
    );
    let reply_markup = json!({
        "inline_keyboard": [[
            {"text": "\u{2705} Approve", "callback_data": "plan_yes"},
            {"text": "\u{274c} Reject", "callback_data": "plan_no"},
        ]]
    });
    let msg_id = telegram::send_html(&config.bot_token, &config.chat_id, &msg, Some(&reply_markup), None);
    hook::save_session_mapping(config, msg_id, data, tty);
    eprintln!("{} [plan] sent plan msg_id={msg_id:?}", super::ts());
}

// ── JSONL file utilities ──────────────────────────────────────────

fn read_last_assistant_from_jsonl(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|e| e["type"].as_str() == Some("assistant"))
        .find_map(|e| extract_assistant_text(&e["message"]))
        .filter(|t| t.len() > 10)
}

pub(super) fn read_jsonl_tail_summary(path: &std::path::Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let tail_size = 65536u64;
    if len > tail_size {
        file.seek(std::io::SeekFrom::End(-(tail_size as i64))).ok()?;
    }
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    let text = buf
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|e| e["type"].as_str() == Some("assistant"))
        .find_map(|e| extract_assistant_text(&e["message"]))
        .filter(|t| t.len() > 10)?;
    let first_line = text.lines().next().unwrap_or(&text);
    let trimmed = first_line.trim();
    if trimmed.len() > 80 {
        let end = telegram::char_floor(trimmed, 77);
        Some(format!("{}...", &trimmed[..end]))
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn find_session_jsonl(session_id: &str) -> Option<PathBuf> {
    if !session_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let projects_dir = PathBuf::from(&home).join(".claude/projects");
    let filename = format!("{session_id}.jsonl");
    let Ok(entries) = std::fs::read_dir(&projects_dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let jsonl = path.join(&filename);
            if jsonl.exists() {
                return Some(jsonl);
            }
        }
    }
    None
}

fn extract_assistant_text(msg: &Value) -> Option<String> {
    let content = msg.get("content")?;
    if let Some(arr) = content.as_array() {
        let texts: Vec<&str> = arr
            .iter()
            .filter_map(|block| {
                if block["type"].as_str() == Some("text") {
                    block["text"].as_str()
                } else {
                    None
                }
            })
            .collect();
        if texts.is_empty() {
            return None;
        }
        Some(texts.join("\n\n"))
    } else {
        content.as_str().map(String::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn is_plan_notification_true() {
        let data = json!({"message": "Waiting for plan approval"});
        assert!(is_plan_notification(&data));
    }

    #[test]
    fn is_plan_notification_false() {
        let data = json!({"message": "Permission needed: cargo build"});
        assert!(!is_plan_notification(&data));
    }

    #[test]
    fn extract_assistant_text_content_blocks() {
        let msg = json!({"content": [
            {"type": "text", "text": "hello"},
            {"type": "tool_use", "id": "1"},
            {"type": "text", "text": "world"},
        ]});
        assert_eq!(extract_assistant_text(&msg), Some("hello\n\nworld".into()));
    }

    #[test]
    fn extract_assistant_text_string() {
        let msg = json!({"content": "plain text"});
        assert_eq!(extract_assistant_text(&msg), Some("plain text".into()));
    }

    #[test]
    fn extract_assistant_text_empty() {
        assert_eq!(extract_assistant_text(&json!({})), None);
        let msg = json!({"content": []});
        assert_eq!(extract_assistant_text(&msg), None);
    }
}
