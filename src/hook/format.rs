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

pub fn get_transcript_summary(path: &str) -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let allowed_prefix = std::fs::canonicalize(format!("{home}/.claude/")).ok()?;
    let canonical = std::fs::canonicalize(path).ok()?;
    if !canonical.starts_with(&allowed_prefix) || !path.ends_with(".jsonl") {
        return None;
    }
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let tail_size = 65536u64;
    if len > tail_size {
        file.seek(SeekFrom::End(-(tail_size as i64))).ok()?;
    }
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;

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

pub fn format_tool_info(tool_name: &str, input: &Value) -> String {
    let text = match tool_name {
        "Bash" => input["command"]
            .as_str()
            .unwrap_or("(no command)")
            .to_string(),
        "Edit" | "Write" => {
            let path = input["file_path"].as_str().unwrap_or("?");
            format!("{tool_name}: {path}")
        }
        _ => serde_json::to_string_pretty(input)
            .unwrap_or_else(|_| "(unknown)".to_string()),
    };
    if text.len() > 500 {
        let end = crate::telegram::char_floor(&text, 497);
        format!("{}…", &text[..end])
    } else {
        text
    }
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
    fn format_tool_info_bash() {
        let input = json!({"command": "ls -la"});
        assert_eq!(format_tool_info("Bash", &input), "ls -la");
    }

    #[test]
    fn format_tool_info_edit() {
        let input = json!({"file_path": "/src/main.rs"});
        assert_eq!(format_tool_info("Edit", &input), "Edit: /src/main.rs");
    }

    #[test]
    fn format_tool_info_truncates_long() {
        let input = json!({"command": "x".repeat(600)});
        let result = format_tool_info("Bash", &input);
        assert!(result.len() < 510);
        assert!(result.ends_with('…'));
    }
}
