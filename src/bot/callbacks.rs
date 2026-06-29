// Telegram callback query handler: plan approval.
// Injects the plan decision into the Ghostty terminal.

use crate::config::Config;
use crate::hook;
use crate::telegram;
use serde_json::{json, Value};

pub(super) fn handle_callback(config: &Config, cb: &Value) {
    let cb_chat_id = cb["message"]["chat"]["id"]
        .as_i64()
        .map(|v| v.to_string())
        .unwrap_or_default();
    if cb_chat_id != config.chat_id {
        return;
    }

    let cb_data = cb["data"].as_str().unwrap_or("");
    let cb_id = cb["id"].as_str().unwrap_or("");
    let cb_from = cb["from"]["first_name"].as_str().unwrap_or("?");
    let cb_msg_id = cb["message"]["message_id"].as_i64().unwrap_or(0);

    eprintln!("{} [callback] from={cb_from} action={cb_data} msg_id={cb_msg_id}", super::ts());

    super::refresh_caffeinate();

    // Plan approval: inject response into Ghostty terminal
    if cb_data == "plan_yes" || cb_data == "plan_no" {
        handle_plan_approval(config, cb_data, cb_id, cb_msg_id);
    } else {
        eprintln!("{} [callback] ignoring unknown action: {cb_data}", super::ts());
    }
}

fn handle_plan_approval(config: &Config, action: &str, cb_id: &str, cb_msg_id: i64) {
    let label = if action == "plan_yes" { "Approved" } else { "Rejected" };
    let emoji = if action == "plan_yes" { "\u{2705}" } else { "\u{274c}" };
    let inject_text = if action == "plan_yes" { "y" } else { "no, reject this plan" };

    let _ = telegram::call(
        &config.bot_token,
        "answerCallbackQuery",
        &json!({"callback_query_id": cb_id, "text": label}),
        15,
    );

    // Try injection BEFORE removing keyboard — if screen is locked, keep buttons for retry.
    let mut injected = false;
    let map = hook::load_session_mapping(config);
    let my_host = hook::short_hostname();
    if let Some(entry) = map.get(&cb_msg_id.to_string()) {
        let entry_host = entry["host"].as_str().unwrap_or("");
        if !entry_host.is_empty() && entry_host != my_host {
            eprintln!("{} [plan] skipping: session belongs to {entry_host}, not {my_host}", super::ts());
            injected = true; // not our host, treat as handled
        } else {
            let tty = entry["tty"].as_str().unwrap_or("");
            let tab_title = entry["tab_title"].as_str().unwrap_or("");
            match hook::atomic_inject(tty, inject_text, tab_title) {
                Ok(_) => {
                    eprintln!("{} [plan] injected '{inject_text}' → tty={tty}", super::ts());
                    injected = true;
                }
                Err(e) => {
                    let is_locked = e == hook::SCREEN_LOCKED_ERR;
                    eprintln!("{} [plan] injection failed: {e} (screen_locked={is_locked})", super::ts());
                    if is_locked {
                        // Keep inline keyboard for retry, just notify
                        let _ = telegram::call(
                            &config.bot_token,
                            "sendMessage",
                            &json!({
                                "chat_id": &config.chat_id,
                                "text": "\u{1f512} Screen locked \u{2014} unlock and tap again",
                                "reply_parameters": {"message_id": cb_msg_id},
                            }),
                            15,
                        );
                        return;
                    }
                }
            }
        }
    }

    // Remove keyboard only after successful injection (or non-lock error)
    let _ = telegram::call(
        &config.bot_token,
        "editMessageReplyMarkup",
        &json!({"chat_id": &config.chat_id, "message_id": cb_msg_id}),
        15,
    );

    let status_text = if injected {
        format!("{emoji} {label}")
    } else {
        format!("\u{274c} Injection failed")
    };
    let _ = telegram::call(
        &config.bot_token,
        "sendMessage",
        &json!({
            "chat_id": &config.chat_id,
            "text": status_text,
            "reply_parameters": {"message_id": cb_msg_id},
        }),
        15,
    );
}
