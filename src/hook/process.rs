// Spool event processors: stop, notification, and pre-tool-use.
// Called by the bot daemon after reading spool files from disk.

use crate::config::Config;
use crate::telegram;
use serde_json::{json, Value};

pub fn process_stop(config: &Config, data: &Value, tty: &str) {
    super::debug_log(config, "stop", data);
    let session_id = data["session_id"].as_str().unwrap_or("");
    let project = super::format::project_name(data);

    eprintln!("[spool:stop] session_id={} project={}", session_id, project);

    if !session_id.is_empty() {
        if let Some((poll_msg_id, html)) = super::session::consume_poll_sent(config, session_id) {
            let updated = html
                .replace("\u{23f3}", "\u{2705}")
                .replace("\u{1f4ac}", "\u{2705}")
                .replace("\u{1f916}", "\u{2705}")
                .replace("\u{27a1}\u{fe0f}", "\u{2705}")
                .replace(" <i>(in progress\u{2026})</i>", "");
            telegram::edit_html(&config.bot_token, &config.chat_id, poll_msg_id, &updated);

            let text = super::format::extract_text(data)
                .or_else(|| {
                    let path = data["transcript_path"].as_str()?;
                    super::format::get_transcript_summary(path)
                })
                .unwrap_or_else(|| "Task completed.".to_string());
            let notif = format!(
                "\u{2705} <b>{}</b>\n\n{}",
                telegram::escape_html(&project),
                telegram::escape_html(&text),
            );
            let notif_id = telegram::send_html(
                &config.bot_token, &config.chat_id, &notif, None, None,
            );
            super::session::save_session_mapping(config, notif_id, data, tty);
            super::session::clear_recent_notif(config, session_id);
            eprintln!("[spool:stop] edited poll msg {poll_msg_id} → ✅, sent notif {notif_id:?}");
            return;
        }
    }

    let text = super::format::extract_text(data).or_else(|| {
        let path = data["transcript_path"].as_str()?;
        eprintln!("[spool:stop] reading transcript: {path}");
        super::format::get_transcript_summary(path)
    });

    let text = match text {
        Some(t) => t,
        None => {
            eprintln!("[spool:stop] no content found, suppressing empty stop");
            super::session::save_session_mapping(config, None, data, tty);
            if !session_id.is_empty() {
                super::session::clear_recent_notif(config, session_id);
            }
            return;
        }
    };

    let msg = format!(
        "\u{1f916} <b>Claude Code \u{b7} {}</b>\n\n{}",
        telegram::escape_html(&project),
        telegram::escape_html(&text),
    );
    let reply_to = data["session_id"]
        .as_str()
        .and_then(|sid| super::session::consume_pending_reply(config, sid));
    let msg_id = telegram::send_html_silent(&config.bot_token, &config.chat_id, &msg, reply_to);
    if reply_to.is_some() {
        if let Some(mid) = msg_id {
            super::session::save_poll_sent(config, session_id, mid, &msg);
        }
    }
    super::session::save_session_mapping(config, msg_id, data, tty);
    if !session_id.is_empty() {
        super::session::clear_recent_notif(config, session_id);
    }
    eprintln!("[spool:stop] done, tg_msg_id={msg_id:?}");
}

pub fn process_notification(config: &Config, data: &Value, tty: &str) {
    super::debug_log(config, "notification", data);
    let session_id = data["session_id"].as_str().unwrap_or("");
    let project = super::format::project_name(data);
    let message = data["message"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| super::format::extract_text(data))
        .unwrap_or_else(|| "Needs your attention".to_string());
    let title = data["title"].as_str().unwrap_or("");

    if super::is_noise_notification(&message) {
        if !session_id.is_empty() {
            if let Some((poll_msg_id, html)) = super::session::consume_poll_sent(config, session_id) {
                let updated = html
                    .replace("\u{23f3}", "\u{2705}")
                    .replace("\u{1f4ac}", "\u{2705}")
                    .replace("\u{1f916}", "\u{2705}")
                    .replace(" <i>(in progress\u{2026})</i>", "")
                    .replace(" \u{b7} ", " ");
                telegram::edit_html(&config.bot_token, &config.chat_id, poll_msg_id, &updated);
                super::session::save_session_mapping(config, Some(poll_msg_id), data, tty);
                eprintln!("[spool:notification] suppressed 'waiting' — edited poll msg {poll_msg_id}");
            } else {
                eprintln!("[spool:notification] suppressed 'waiting' (no poll msg to edit)");
            }
        } else {
            eprintln!("[spool:notification] suppressed 'waiting' (no session_id)");
        }
        return;
    }

    let mut body_part = String::new();
    if !title.is_empty() {
        body_part.push_str(&format!("<b>{}</b>\n", telegram::escape_html(title)));
    }
    body_part.push_str(&telegram::escape_html(&message));

    let header = format!(
        "\u{26a1} <b>Claude Code \u{b7} {}</b>",
        telegram::escape_html(&project),
    );

    if !session_id.is_empty() {
        if let Some(recent) = super::session::get_recent_notif(config, session_id) {
            let mut parts: Vec<String> = recent["parts"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let existing_msg_id = recent["msg_id"].as_i64().unwrap_or(0);

            parts.push(body_part.clone());
            let full_body = parts.join("\n────────────────\n");
            let full_msg = format!("{header}\n\n{full_body}");

            if full_msg.len() < 4000 && existing_msg_id > 0 {
                if telegram::edit_html(
                    &config.bot_token,
                    &config.chat_id,
                    existing_msg_id,
                    &full_msg,
                ) {
                    super::session::save_recent_notif(config, session_id, existing_msg_id, &header, &parts);
                    eprintln!(
                        "[spool:notification] consolidated into msg_id={existing_msg_id} ({} parts)",
                        parts.len()
                    );
                    return;
                }
            }
        }
    }

    let full_msg = format!("{header}\n\n{body_part}");
    let reply_to = data["session_id"]
        .as_str()
        .and_then(|sid| super::session::consume_pending_reply(config, sid));
    let msg_id = telegram::send_html(&config.bot_token, &config.chat_id, &full_msg, None, reply_to);
    super::session::save_session_mapping(config, msg_id, data, tty);
    if let Some(mid) = msg_id {
        if !session_id.is_empty() {
            super::session::save_recent_notif(config, session_id, mid, &header, &[body_part]);
        }
    }
    eprintln!("[spool:notification] done, tg_msg_id={msg_id:?}");
}

pub fn process_pre_tool_use(config: &Config, data: &Value, request_id: &str) {
    super::debug_log(config, "pre-tool-use", data);
    let tool_name = data["tool_name"].as_str().unwrap_or("");
    let project = super::format::project_name(data);
    let tool_info = super::format::format_tool_info(tool_name, &data["tool_input"]);
    let escaped_info = telegram::escape_html(&tool_info);

    let msg = format!(
        "\u{1f512} <b>Permission \u{b7} {}</b>\n\n<b>{}</b>\n<pre>{escaped_info}</pre>",
        telegram::escape_html(&project),
        telegram::escape_html(tool_name),
    );
    let reply_markup = json!({
        "inline_keyboard": [[
            {"text": "\u{2705} Allow", "callback_data": format!("allow:{request_id}")},
            {"text": "\u{274c} Deny", "callback_data": format!("deny:{request_id}")},
        ]]
    });
    telegram::send_html(&config.bot_token, &config.chat_id, &msg, Some(&reply_markup), None);
}
