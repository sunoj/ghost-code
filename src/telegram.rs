// Telegram Bot API client.
// Handles sending messages (HTML/plain), drafts (streaming), and API calls.

use serde_json::{json, Value};
use std::time::Duration;

const API_BASE: &str = "https://api.telegram.org/bot";
const LINK_PREVIEW_OFF: &str = r#"{"is_disabled":true}"#;

pub fn call(token: &str, method: &str, params: &Value, timeout_secs: u64) -> Result<Value, String> {
    let url = format!("{API_BASE}{token}/{method}");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(timeout_secs))
        .build();
    let resp = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .send_json(params)
        .map_err(|e| format!("{e}").replace(token, "<token>"))?;
    let body: Value = resp.into_json().map_err(|e| format!("{e}"))?;
    if body.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        let desc = body["description"].as_str().unwrap_or("unknown error");
        return Err(format!("Telegram API: {desc}"));
    }
    Ok(body)
}

fn link_preview_off() -> Value {
    serde_json::from_str(LINK_PREVIEW_OFF).unwrap()
}

/// Send an HTML-formatted message with optional inline keyboard and reply. Returns message_id.
pub fn send_html(
    token: &str,
    chat_id: &str,
    html: &str,
    reply_markup: Option<&Value>,
    reply_to: Option<i64>,
) -> Option<i64> {
    send_chunks(token, chat_id, html, Some("HTML"), reply_markup, reply_to, false)
}

/// Send an HTML message silently (no notification sound). Returns message_id.
pub fn send_html_silent(
    token: &str,
    chat_id: &str,
    html: &str,
    reply_to: Option<i64>,
) -> Option<i64> {
    send_chunks(token, chat_id, html, Some("HTML"), None, reply_to, true)
}

/// Edit an existing HTML message. Returns true on success.
pub fn edit_html(token: &str, chat_id: &str, msg_id: i64, html: &str) -> bool {
    let params = json!({
        "chat_id": chat_id,
        "message_id": msg_id,
        "text": html,
        "parse_mode": "HTML",
        "link_preview_options": link_preview_off(),
    });
    match call(token, "editMessageText", &params, 15) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("[telegram] editMessageText failed: {e}");
            false
        }
    }
}

/// Send a plain text message (no formatting). Returns message_id.
pub fn send_text(token: &str, chat_id: &str, text: &str) -> Option<i64> {
    send_chunks(token, chat_id, text, None, None, None, false)
}

/// Send a streaming draft message. Returns message_id on first call.
pub fn send_draft(token: &str, chat_id: &str, text: &str) -> Option<i64> {
    let params = json!({
        "chat_id": chat_id,
        "text": text,
        "link_preview_options": link_preview_off(),
    });
    call(token, "sendMessageDraft", &params, 15)
        .ok()
        .and_then(|v| v["result"]["message_id"].as_i64())
}

pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Convert markdown to Telegram-compatible HTML.
pub fn markdown_to_html(md: &str) -> String {
    let mut out = String::new();
    let mut in_code_block = false;
    let mut code_has_lang = false;
    let mut in_blockquote = false;

    for line in md.lines() {
        if line.starts_with("```") {
            if in_blockquote {
                out.push_str("</blockquote>\n");
                in_blockquote = false;
            }
            if in_code_block {
                if out.ends_with('\n') {
                    out.pop();
                }
                out.push_str(if code_has_lang {
                    "</code></pre>\n"
                } else {
                    "</pre>\n"
                });
                in_code_block = false;
                code_has_lang = false;
            } else {
                in_code_block = true;
                let lang = line.trim_start_matches('`').trim();
                if lang.is_empty() {
                    out.push_str("<pre>");
                } else {
                    out.push_str(&format!(
                        "<pre><code class=\"language-{}\">",
                        escape_html(lang)
                    ));
                    code_has_lang = true;
                }
            }
            continue;
        }

        if in_code_block {
            out.push_str(&escape_html(line));
            out.push('\n');
            continue;
        }

        if line.starts_with("> ") || line == ">" {
            let content = if line == ">" { "" } else { &line[2..] };
            if !in_blockquote {
                in_blockquote = true;
                out.push_str("<blockquote>");
            } else {
                out.push('\n');
            }
            out.push_str(&format_inline(content));
            continue;
        } else if in_blockquote {
            out.push_str("</blockquote>\n");
            in_blockquote = false;
        }

        let stripped = line.trim_start_matches('#');
        if stripped.len() < line.len() && stripped.starts_with(' ') {
            out.push_str("<b>");
            out.push_str(&escape_html(stripped.trim_start()));
            out.push_str("</b>\n");
            continue;
        }

        out.push_str(&format_inline(line));
        out.push('\n');
    }

    if in_code_block {
        if out.ends_with('\n') {
            out.pop();
        }
        out.push_str(if code_has_lang {
            "</code></pre>"
        } else {
            "</pre>"
        });
    }
    if in_blockquote {
        out.push_str("</blockquote>");
    }

    out.trim_end().to_string()
}

/// Process inline markdown: **bold**, `code`, and HTML-escape the rest.
fn format_inline(line: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '`') {
                let end = i + 1 + end;
                let code: String = chars[i + 1..end].iter().collect();
                out.push_str("<code>");
                out.push_str(&escape_html(&code));
                out.push_str("</code>");
                i = end + 1;
                continue;
            }
        }

        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(pos) = find_double_marker(&chars[i + 2..], '*') {
                let end = i + 2 + pos;
                let text: String = chars[i + 2..end].iter().collect();
                out.push_str("<b>");
                out.push_str(&escape_html(&text));
                out.push_str("</b>");
                i = end + 2;
                continue;
            }
        }

        match chars[i] {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
        i += 1;
    }

    out
}

fn find_double_marker(chars: &[char], c: char) -> Option<usize> {
    for i in 0..chars.len().saturating_sub(1) {
        if chars[i] == c && chars[i + 1] == c {
            return Some(i);
        }
    }
    None
}

fn send_chunks(
    token: &str,
    chat_id: &str,
    text: &str,
    parse_mode: Option<&str>,
    reply_markup: Option<&Value>,
    reply_to: Option<i64>,
    silent: bool,
) -> Option<i64> {
    let mut remaining = text.to_string();
    let mut is_first = true;

    while remaining.len() > 4000 {
        let limit = char_floor(&remaining, 4000);
        let split = remaining[..limit].rfind('\n').filter(|&p| p > 0).unwrap_or(limit);
        let chunk = &remaining[..split];
        let mut params = json!({
            "chat_id": chat_id,
            "text": chunk,
            "link_preview_options": link_preview_off(),
        });
        if let Some(mode) = parse_mode {
            params["parse_mode"] = json!(mode);
        }
        if silent {
            params["disable_notification"] = json!(true);
        }
        if is_first {
            if let Some(markup) = reply_markup {
                params["reply_markup"] = markup.clone();
            }
            if let Some(msg_id) = reply_to {
                params["reply_parameters"] = json!({"message_id": msg_id});
            }
            is_first = false;
        }
        let _ = call(token, "sendMessage", &params, 15);
        remaining = remaining[split..].trim_start_matches('\n').to_string();
    }

    let display = if remaining.is_empty() {
        "(empty)"
    } else {
        &remaining
    };
    let mut params = json!({
        "chat_id": chat_id,
        "text": display,
        "link_preview_options": link_preview_off(),
    });
    if let Some(mode) = parse_mode {
        params["parse_mode"] = json!(mode);
    }
    if is_first {
        if let Some(markup) = reply_markup {
            params["reply_markup"] = markup.clone();
        }
        if let Some(msg_id) = reply_to {
            params["reply_parameters"] = json!({"message_id": msg_id});
        }
    }
    if silent {
        params["disable_notification"] = json!(true);
    }

    let result = call(token, "sendMessage", &params, 15).or_else(|e| {
        if parse_mode.is_some() {
            eprintln!("[telegram] sendMessage failed with parse_mode={:?}: {e}, retrying plain", parse_mode);
            if let Some(obj) = params.as_object_mut() {
                obj.remove("parse_mode");
                obj.remove("reply_markup");
            }
            call(token, "sendMessage", &params, 15)
        } else {
            Err("send failed".to_string())
        }
    });

    match &result {
        Ok(v) => {
            let mid = v["result"]["message_id"].as_i64();
            eprintln!("[telegram] sent msg_id={:?} len={} mode={:?} silent={silent}", mid, display.len(), parse_mode);
        }
        Err(e) => eprintln!("[telegram] send failed: {e}"),
    }

    result
        .ok()
        .and_then(|v| v["result"]["message_id"].as_i64())
}

/// Find the largest byte position <= `pos` that is a UTF-8 char boundary.
pub(crate) fn char_floor(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut i = pos;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_html_special_chars() {
        assert_eq!(escape_html("<b>test&</b>"), "&lt;b&gt;test&amp;&lt;/b&gt;");
    }

    #[test]
    fn escape_html_passthrough() {
        assert_eq!(escape_html("hello world"), "hello world");
    }

    #[test]
    fn markdown_to_html_headings() {
        assert_eq!(markdown_to_html("# Title"), "<b>Title</b>");
        assert_eq!(markdown_to_html("## Sub"), "<b>Sub</b>");
    }

    #[test]
    fn markdown_to_html_bold() {
        assert_eq!(markdown_to_html("**bold**"), "<b>bold</b>");
    }

    #[test]
    fn markdown_to_html_inline_code() {
        assert_eq!(markdown_to_html("`code`"), "<code>code</code>");
    }

    #[test]
    fn markdown_to_html_code_block() {
        let md = "```rust\nfn main() {}\n```";
        let html = markdown_to_html(md);
        assert!(html.contains("<pre><code class=\"language-rust\">"));
        assert!(html.contains("fn main() {}"));
        assert!(html.contains("</code></pre>"));
    }

    #[test]
    fn markdown_to_html_code_block_no_lang() {
        let md = "```\nhello\n```";
        let html = markdown_to_html(md);
        assert!(html.starts_with("<pre>"));
        assert!(html.contains("hello"));
        assert!(html.ends_with("</pre>"));
    }

    #[test]
    fn markdown_to_html_blockquote() {
        let md = "> quoted text";
        let html = markdown_to_html(md);
        assert!(html.contains("<blockquote>"));
        assert!(html.contains("quoted text"));
    }

    #[test]
    fn markdown_to_html_html_in_code() {
        let md = "```\n<script>alert('xss')</script>\n```";
        let html = markdown_to_html(md);
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn char_floor_ascii() {
        assert_eq!(char_floor("hello", 3), 3);
        assert_eq!(char_floor("hello", 10), 5);
    }

    #[test]
    fn char_floor_multibyte() {
        // "café" = [99, 97, 102, 195, 169] = 5 bytes
        // char boundaries: 0(c), 1(a), 2(f), 3(é start), 5(end)
        let s = "café";
        assert_eq!(char_floor(s, 3), 3); // start of é — valid boundary
        assert_eq!(char_floor(s, 4), 3); // mid-é — snaps back to 3
        assert_eq!(char_floor(s, 5), 5); // end of string
    }

    #[test]
    fn char_floor_emoji() {
        let s = "hi🎉ok";  // 🎉 is 4 bytes, starts at byte 2
        let floor = char_floor(s, 3); // mid-emoji
        assert_eq!(floor, 2); // snaps back to emoji start
    }
}
