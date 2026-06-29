// Setup and diagnostics for Claude Code hook installation.
// Exports: run_setup, run_test.
// Deps: config, hook, telegram, serde_json.

use crate::{config, hook, telegram};
use serde_json::{json, Value};
use std::path::PathBuf;

pub fn run_setup() {
    let hooks_dir = hook::hooks_dir();
    std::fs::create_dir_all(&hooks_dir).ok();
    let target = install_binary(&hooks_dir);
    let env_file = ensure_env_file(&hooks_dir);

    let settings_path = home_dir().join(".claude/settings.json");
    let mut settings = load_settings(&settings_path);
    let binary = target.to_string_lossy();
    for (event, cmd) in [
        ("Stop", format!("{binary} stop")),
        ("Notification", format!("{binary} notification")),
        ("StatusLine", format!("{binary} statusline")),
    ] {
        merge_hook_command(&mut settings, event, &cmd);
    }
    write_settings(&settings_path, &settings);

    println!("\nSetup complete.");
    println!("  1. Edit {} with your bot token and chat ID", env_file.display());
    println!("  2. Run 'ghost-code test' to verify");
}

pub fn run_test() {
    let config = config::load();
    if config.bot_token.is_empty() || config.chat_id.is_empty() {
        eprintln!("Set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID in ~/.claude/hooks/ghost-code.env first.");
        eprintln!("Run 'ghost-code setup' if you haven't yet.");
        std::process::exit(1);
    }

    send_stop_test(&config);
    send_notification_test(&config);
    println!("Done. Check Telegram for two test messages.");
}

fn install_binary(hooks_dir: &PathBuf) -> PathBuf {
    let target = hooks_dir.join("ghost-code");
    if let Ok(exe) = std::env::current_exe() {
        if exe != target {
            match std::fs::copy(&exe, &target) {
                Ok(_) => {
                    sign_binary(&target);
                    println!("Installed binary to {}", target.display());
                }
                Err(e) => eprintln!("Warning: could not copy binary to {}: {e}", target.display()),
            }
        }
    }
    target
}

fn sign_binary(target: &PathBuf) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("codesign")
            .args(["--force", "--sign", "-", &target.to_string_lossy()])
            .output();
    }
}

fn ensure_env_file(hooks_dir: &PathBuf) -> PathBuf {
    let env_file = hooks_dir.join("ghost-code.env");
    if env_file.exists() {
        println!("ghost-code.env already exists, skipping.");
        return env_file;
    }
    let template = include_str!("../.env.example");
    std::fs::write(&env_file, template).ok();
    set_owner_only_permissions(&env_file);
    println!("Created {} — edit it with your bot token and chat ID.", env_file.display());
    env_file
}

fn set_owner_only_permissions(path: &PathBuf) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
    }
}

fn load_settings(settings_path: &PathBuf) -> Value {
    std::fs::read_to_string(settings_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_else(|| json!({}))
}

fn write_settings(settings_path: &PathBuf, settings: &Value) {
    std::fs::write(
        settings_path,
        serde_json::to_string_pretty(settings).unwrap_or_default() + "\n",
    )
    .ok();
}

fn merge_hook_command(settings: &mut Value, event: &str, cmd: &str) {
    let hooks = settings.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!({}));
    let entries = hooks.as_object_mut().unwrap().entry(event).or_insert_with(|| json!([]));
    if settings_has_command(entries, cmd) {
        println!("{event} hook already configured.");
        return;
    }
    entries
        .as_array_mut()
        .unwrap()
        .push(json!({"hooks": [{"type": "command", "command": cmd}]}));
    println!("Added {event} hook.");
}

fn settings_has_command(entries: &Value, cmd: &str) -> bool {
    entries.as_array().is_some_and(|arr| {
        arr.iter().any(|entry| {
            entry["hooks"].as_array().is_some_and(|hooks| {
                hooks.iter().any(|hook| hook["command"].as_str() == Some(cmd))
            })
        })
    })
}

fn send_stop_test(config: &config::Config) {
    println!("Testing Stop hook...");
    let stop_msg = format!(
        "\u{1f916} <b>Claude Code \u{b7} test@{}</b>\n\n{}",
        telegram::escape_html(&hook::short_hostname()),
        "Refactored the swap router to support multi-hop paths. (test message)",
    );
    telegram::send_html_silent(&config.bot_token, &config.chat_id, &stop_msg, None);
}

fn send_notification_test(config: &config::Config) {
    println!("Testing Notification hook...");
    let notif_msg = format!(
        "\u{26a1} <b>Claude Code \u{b7} test@{}</b>\n\n{}",
        telegram::escape_html(&hook::short_hostname()),
        "Permission needed: cargo build (test message)",
    );
    telegram::send_html(&config.bot_token, &config.chat_id, &notif_msg, None, None);
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_hook_command_adds_missing_command() {
        let mut settings = json!({"hooks": {"Stop": []}});
        merge_hook_command(&mut settings, "Stop", "/tmp/ghost-code stop");

        assert!(settings_has_command(
            &settings["hooks"]["Stop"],
            "/tmp/ghost-code stop"
        ));
    }

    #[test]
    fn merge_hook_command_does_not_duplicate_existing_command() {
        let cmd = "/tmp/ghost-code stop";
        let mut settings = json!({"hooks": {"Stop": [{"hooks": [{"command": cmd}]}]}});
        merge_hook_command(&mut settings, "Stop", cmd);

        assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }
}
