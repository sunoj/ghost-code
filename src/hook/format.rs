// Text extraction, formatting, and hostname utilities.
// Used by hook processors and bot for message construction.

use serde_json::Value;
use std::path::Path;

pub fn extract_text(data: &Value) -> Option<String> {
    for key in ["last_assistant_message", "message", "content", "text"] {
        match data.get(key) {
            Some(Value::String(s)) if !s.is_empty() => return Some(s.clone()),
            Some(Value::Array(arr)) => {
                let texts: Vec<String> = arr
                    .iter()
                    .filter_map(|block| match block {
                        Value::String(s) => Some(s.clone()),
                        Value::Object(obj)
                            if obj.get("type").and_then(|t| t.as_str()) == Some("text") =>
                        {
                            obj.get("text").and_then(|t| t.as_str()).map(String::from)
                        }
                        _ => None,
                    })
                    .collect();
                if !texts.is_empty() {
                    return Some(texts.join("\n"));
                }
            }
            _ => {}
        }
    }
    None
}

pub fn project_name(data: &Value) -> String {
    let project = data["cwd"]
        .as_str()
        .and_then(|cwd| Path::new(cwd).file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_else(|| "unknown".to_string())
        });
    let host = short_hostname();
    if host.is_empty() { project } else { format!("{project}@{host}") }
}

pub fn short_hostname() -> String {
    std::env::var("HOST")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| {
            let mut buf = [0u8; 256];
            if unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0 {
                let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                String::from_utf8_lossy(&buf[..len]).to_string()
            } else {
                String::new()
            }
        })
        .split('.')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Read the last 64 KiB of a transcript as text, dropping a leading partial
/// line. Restricted to ~/.claude/*.jsonl. Uses lossy UTF-8 so a multi-byte
/// (e.g. Chinese) character split at the read boundary isn't lost.
fn read_transcript_tail(path: &str) -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let allowed_prefix = std::fs::canonicalize(format!("{home}/.claude/")).ok()?;
    let canonical = std::fs::canonicalize(path).ok()?;
    if !canonical.starts_with(&allowed_prefix) || !path.ends_with(".jsonl") {
        return None;
    }
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(65536);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0 {
        if let Some(nl) = text.find('\n') {
            text.drain(..=nl);
        }
    }
    Some(text)
}

pub fn get_transcript_summary(path: &str) -> Option<String> {
    let buf = read_transcript_tail(path)?;

    let jsonl_result = buf
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|e| e["type"].as_str() == Some("assistant"))
        .find_map(|e| {
            let msg = &e["message"];
            extract_text(msg).filter(|t| t.trim().len() > 10)
        });
    if jsonl_result.is_some() {
        return jsonl_result;
    }

    let data: Value = serde_json::from_str(&buf).ok()?;
    let messages = if let Some(arr) = data.as_array() {
        arr.clone()
    } else {
        data.get("messages")
            .or_else(|| data.get("transcript"))
            .and_then(|v| v.as_array())
            .cloned()?
    };
    messages
        .iter()
        .rev()
        .filter(|msg| msg["role"].as_str() == Some("assistant"))
        .find_map(|msg| {
            let text = extract_text(msg)?;
            (text.trim().len() > 10).then_some(text)
        })
}

/// Extract the most recent AskUserQuestion (question text + numbered options)
/// from a transcript, so a "needs your input" notification can show what
/// Claude actually asked. Returns None when the last assistant turn is not a
/// question prompt.
pub fn get_pending_question(path: &str) -> Option<String> {
    let buf = read_transcript_tail(path)?;
    buf.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|e| e["type"].as_str() == Some("assistant"))
        .and_then(|e| format_ask_question(e["message"]["content"].as_array()?))
}

/// Render an AskUserQuestion tool call's questions and options as plain text.
/// Handles both the parsed input and the rare `__unparsedToolInput.raw` form.
fn format_ask_question(content: &[Value]) -> Option<String> {
    let tool = content.iter().find(|b| {
        b["type"].as_str() == Some("tool_use") && b["name"].as_str() == Some("AskUserQuestion")
    })?;
    let input = &tool["input"];
    let parsed = if input.get("questions").is_some() {
        input.clone()
    } else {
        serde_json::from_str(input["__unparsedToolInput"]["raw"].as_str()?).ok()?
    };

    let mut out = String::new();
    for q in parsed["questions"].as_array()? {
        if let Some(text) = q["question"].as_str() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(text);
        }
        for (i, opt) in q["options"].as_array().into_iter().flatten().enumerate() {
            let label = opt["label"].as_str().unwrap_or("");
            out.push_str(&format!("\n{}. {label}", i + 1));
            if let Some(desc) = opt["description"].as_str().filter(|d| !d.is_empty()) {
                let desc: String = if desc.chars().count() > 160 {
                    desc.chars().take(157).chain("…".chars()).collect()
                } else {
                    desc.to_string()
                };
                out.push_str(&format!("\n   {desc}"));
            }
        }
    }
    let trimmed = out.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_text_string_field() {
        let data = json!({"message": "hello world"});
        assert_eq!(extract_text(&data), Some("hello world".into()));
    }

    #[test]
    fn extract_text_content_blocks() {
        let data = json!({"content": [
            {"type": "text", "text": "first"},
            {"type": "tool_use", "name": "bash"},
            {"type": "text", "text": "second"},
        ]});
        assert_eq!(extract_text(&data), Some("first\nsecond".into()));
    }

    #[test]
    fn extract_text_empty() {
        assert_eq!(extract_text(&json!({})), None);
        assert_eq!(extract_text(&json!({"message": ""})), None);
    }

    #[test]
    fn extract_text_priority() {
        let data = json!({"last_assistant_message": "first", "message": "second"});
        assert_eq!(extract_text(&data), Some("first".into()));
    }

    #[test]
    fn project_name_from_cwd() {
        let data = json!({"cwd": "/home/user/my-project"});
        let name = project_name(&data);
        assert!(name.starts_with("my-project"));
    }

    #[test]
    fn format_ask_question_parsed() {
        let content = vec![json!({
            "type": "tool_use",
            "name": "AskUserQuestion",
            "input": {"questions": [{
                "question": "怎么落地 v0.9.38?",
                "options": [
                    {"label": "授权合并", "description": "止血并部署"},
                    {"label": "先合 main", "description": "攒一批再发"},
                ]
            }]}
        })];
        let out = format_ask_question(&content).unwrap();
        assert!(out.starts_with("怎么落地 v0.9.38?"));
        assert!(out.contains("1. 授权合并"));
        assert!(out.contains("   止血并部署"));
        assert!(out.contains("2. 先合 main"));
    }

    #[test]
    fn format_ask_question_unparsed_raw() {
        let raw = r#"{"questions":[{"question":"Q?","options":[{"label":"A","description":"da"},{"label":"B"}]}]}"#;
        let content = vec![json!({
            "type": "tool_use",
            "name": "AskUserQuestion",
            "input": {"__unparsedToolInput": {"raw": raw, "len": raw.len()}}
        })];
        let out = format_ask_question(&content).unwrap();
        assert!(out.contains("Q?"));
        assert!(out.contains("1. A"));
        assert!(out.contains("   da"));
        assert!(out.contains("2. B"));
    }

    #[test]
    fn format_ask_question_truncates_long_description() {
        let long = "x".repeat(300);
        let content = vec![json!({
            "type": "tool_use",
            "name": "AskUserQuestion",
            "input": {"questions": [{"question": "Q", "options": [{"label": "L", "description": long}]}]}
        })];
        let out = format_ask_question(&content).unwrap();
        assert!(out.ends_with('…'));
        assert!(out.chars().count() < 200);
    }

    #[test]
    fn format_ask_question_none_without_tool() {
        let content = vec![json!({"type": "text", "text": "hi"})];
        assert_eq!(format_ask_question(&content), None);
    }
}
