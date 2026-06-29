// Configuration loading from .env file with env var overrides.

use std::collections::HashMap;
use std::path::PathBuf;
use std::{env, fs};

#[derive(Clone)]
pub struct Config {
    pub bot_token: String,
    pub chat_id: String,
    pub debug: bool,
    pub working_dir: String,
    pub timeout: u64,
    pub hooks_dir: PathBuf,
}

pub fn load() -> Config {
    let hooks_dir = home_dir().join(".claude/hooks");
    let env_file = hooks_dir.join("ghost-code.env");
    let file = load_env_file(&env_file);

    Config {
        bot_token: env::var("TELEGRAM_BOT_TOKEN")
            .unwrap_or_else(|_| file.get("TELEGRAM_BOT_TOKEN").cloned().unwrap_or_default()),
        chat_id: env::var("TELEGRAM_CHAT_ID")
            .unwrap_or_else(|_| file.get("TELEGRAM_CHAT_ID").cloned().unwrap_or_default()),
        debug: env::var("DEBUG")
            .ok()
            .or_else(|| file.get("DEBUG").cloned())
            .is_some_and(|v| v == "true"),
        working_dir: env::var("WORKING_DIR")
            .ok()
            .or_else(|| file.get("WORKING_DIR").cloned())
            .unwrap_or_else(|| {
                let ws = home_dir().join(".claude/ghost-code/workspace");
                std::fs::create_dir_all(&ws).ok();
                ws.to_string_lossy().to_string()
            }),
        timeout: env::var("CLAUDE_TIMEOUT")
            .ok()
            .or_else(|| file.get("CLAUDE_TIMEOUT").cloned())
            .and_then(|v| v.parse().ok())
            .unwrap_or(300),
        hooks_dir,
    }
}

fn home_dir() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

fn load_env_file(path: &PathBuf) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(content) = fs::read_to_string(path) else {
        return map;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            map.insert(key.trim().to_string(), value.to_string());
        }
    }
    map
}
