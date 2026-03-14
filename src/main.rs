// Entry point: dispatches to hook handlers or bot daemon.
// Hook commands (stop/notification) use ultra-fast spool path — no config load, no network.

mod bot;
mod config;
mod hook;
mod plan_usage;
mod telegram;
mod usage;

fn main() {
    let command = std::env::args().nth(1).unwrap_or_default();

    match command.as_str() {
        // Ultra-fast path: spool to disk and exit. No config load, no network calls.
        "stop" | "notification" => {
            hook::spool_and_exit(&command);
            ensure_bot_quick();
        }

        // Pre-tool-use needs config (approval list) and blocks until decision.
        "pre-tool-use" => {
            let config = config::load();
            if config.bot_token.is_empty() || config.chat_id.is_empty() {
                return;
            }
            ensure_bot_quick();
            hook::handle_pre_tool_use(&config);
        }

        "statusline" => {
            ensure_bot_quick();
            print_statusline();
        }

        "bot" => {
            let config = config::load();
            if config.bot_token.is_empty() || config.chat_id.is_empty() {
                eprintln!("Set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID");
                std::process::exit(1);
            }
            bot::run(config);
        }

        "setup" => run_setup(),

        "test" => run_test(),

        "--version" | "-V" => println!("ghost-code {}", env!("CARGO_PKG_VERSION")),

        _ => eprintln!("Usage: ghost-code <stop|notification|pre-tool-use|statusline|bot|setup|test>"),
    }
}

/// Print statusline: session data from stdin + pre-computed data from status file.
/// Format: 🤖 Opus 4.6 | 💰 $5 / $463 today | 📊 82% block · 38% weekly | 🧠 25% | 🌐 AIS 80% 51.0K $0.15 | 🔧 AID 2▶ 117✓ $464 | 📡 TG
fn print_statusline() {
    use std::io::Read;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();
    let cc: serde_json::Value = serde_json::from_str(&input).unwrap_or_default();

    let model = cc["model"]["display_name"].as_str().unwrap_or("");
    let session_cost = cc["cost"]["total_cost_usd"].as_f64().unwrap_or(0.0);
    let ctx_pct = cc["context_window"]["used_percentage"]
        .as_f64()
        .or_else(|| cc["context_window"]["used_percentage"].as_i64().map(|v| v as f64));

    let mut parts: Vec<String> = Vec::new();

    // 🤖 Model
    if !model.is_empty() {
        parts.push(format!("\u{1f916} {model}"));
    }

    // 💰 Costs: session / today
    let status_file = hook::hooks_dir().join("ghost-code-status.txt");
    let status_data = std::fs::read_to_string(&status_file).unwrap_or_default();
    let parsed: serde_json::Value =
        serde_json::from_str(status_data.trim()).unwrap_or_default();

    let today_cost = parsed["today_cost"].as_f64().unwrap_or(0.0);
    if session_cost > 0.0 || today_cost > 0.0 {
        let session_str = usage::format_cost(session_cost);
        let today_str = usage::format_cost(today_cost);
        parts.push(format!("\u{1f4b0} {session_str} / {today_str} today"));
    }

    // 📊 Plan limits: block + weekly with reset times
    let plan_str = parsed["plan"].as_str().unwrap_or("");
    if !plan_str.is_empty() {
        parts.push(format!("\u{1f4ca} {plan_str}"));
    }

    // 🧠 Context window
    if let Some(pct) = ctx_pct {
        parts.push(format!("\u{1f9e0} {:.0}%", pct));
    }

    // 🌐 AI Summary savings
    let ai_str = parsed["ai"].as_str().unwrap_or("");
    if !ai_str.is_empty() {
        parts.push(format!("\u{1f310} AIS {ai_str}"));
    }

    // 🔧 AID stats: running/done/cost today
    let aid_str = parsed["aid"].as_str().unwrap_or("");
    if !aid_str.is_empty() {
        parts.push(format!("\u{1f527} AID {aid_str}"));
    }

    // 📡 Bot status
    if parsed["bot"].as_bool().unwrap_or(false) {
        parts.push("\u{1f4e1} TG".to_string());
    } else {
        parts.push("TG off".to_string());
    }

    print!("{}", parts.join(" | "));
}

/// Check if bot daemon is running, start it if not.
/// Uses flock on PID file as the single source of truth (same lock the daemon holds).
fn ensure_bot_quick() {
    use std::os::unix::io::AsRawFd;
    use std::os::unix::process::CommandExt;

    let hooks_dir = hook::hooks_dir();
    let pid_file = hooks_dir.join("ghost-code.pid");

    // Try to acquire the PID lock non-blocking. If it fails, a daemon holds it.
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&pid_file)
    {
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if locked != 0 {
            // Lock held by another process → daemon is running
            return;
        }
        // We got the lock → no daemon running. Release it so the new daemon can acquire it.
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        drop(file);
    }

    // Kill any stale ghost-code bot processes before spawning a new one
    let _ = std::process::Command::new("pkill")
        .args(["-f", "ghost-code bot"])
        .output();
    // Brief pause to let old process exit
    std::thread::sleep(std::time::Duration::from_millis(100));

    let binary = hooks_dir.join("ghost-code");
    let log_file = hooks_dir.join("ghost-code.log");
    if let Ok(log) = std::fs::OpenOptions::new().create(true).append(true).open(&log_file) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&log_file, std::fs::Permissions::from_mode(0o600)).ok();
        }
        if let Ok(stderr_log) = log.try_clone() {
            match std::process::Command::new(&binary)
                .arg("bot")
                .process_group(0) // detach from hook's process group
                .stdout(log)
                .stderr(stderr_log)
                .stdin(std::process::Stdio::null())
                .spawn()
            {
                Ok(_) => eprintln!("Auto-started bot daemon"),
                Err(e) => eprintln!("Failed to auto-start bot: {e}"),
            }
        }
    }
}

/// Set up Claude Code hooks in ~/.claude/settings.json and create .env template.
fn run_setup() {
    let hooks_dir = hook::hooks_dir();
    std::fs::create_dir_all(&hooks_dir).ok();

    // Copy binary to hooks dir if not already there
    let target = hooks_dir.join("ghost-code");
    if let Ok(exe) = std::env::current_exe() {
        if exe != target {
            if let Err(e) = std::fs::copy(&exe, &target) {
                eprintln!("Warning: could not copy binary to {}: {e}", target.display());
            } else {
                // Sign on macOS
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("codesign")
                        .args(["--force", "--sign", "-", &target.to_string_lossy()])
                        .output();
                }
                println!("Installed binary to {}", target.display());
            }
        }
    }

    // Create .env from template if missing
    let env_file = hooks_dir.join("ghost-code.env");
    if !env_file.exists() {
        let template = "\
# Telegram Bot Token (from @BotFather)\n\
TELEGRAM_BOT_TOKEN=your-bot-token-here\n\
\n\
# Your Telegram Chat ID (from @userinfobot)\n\
TELEGRAM_CHAT_ID=your-chat-id-here\n\
\n\
# Set to true to log raw hook data for debugging\n\
DEBUG=false\n";
        std::fs::write(&env_file, template).ok();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&env_file, std::fs::Permissions::from_mode(0o600)).ok();
        }
        println!("Created {} — edit it with your bot token and chat ID.", env_file.display());
    } else {
        println!("ghost-code.env already exists, skipping.");
    }

    // Merge hooks into settings.json
    let settings_path = home_dir().join(".claude/settings.json");
    let mut settings: serde_json::Value = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let binary_str = target.to_string_lossy();
    let hook_defs = [
        ("Stop", format!("{binary_str} stop")),
        ("Notification", format!("{binary_str} notification")),
        ("PreToolUse", format!("{binary_str} pre-tool-use")),
        ("StatusLine", format!("{binary_str} statusline")),
    ];

    for (event, cmd) in &hook_defs {
        let entries = hooks
            .as_object_mut()
            .unwrap()
            .entry(*event)
            .or_insert_with(|| serde_json::json!([]));
        let arr = entries.as_array().unwrap_or(&vec![]).clone();

        let already = arr.iter().any(|e| {
            e["hooks"]
                .as_array()
                .map(|hs| hs.iter().any(|h| h["command"].as_str() == Some(cmd)))
                .unwrap_or(false)
        });

        if !already {
            let entry = serde_json::json!({"hooks": [{"type": "command", "command": cmd}]});
            entries.as_array_mut().unwrap().push(entry);
            println!("Added {event} hook.");
        } else {
            println!("{event} hook already configured.");
        }
    }

    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).unwrap_or_default() + "\n",
    )
    .ok();

    println!("\nSetup complete.");
    println!("  1. Edit {} with your bot token and chat ID", env_file.display());
    println!("  2. Run 'ghost-code test' to verify");
}

/// Send test messages to Telegram to verify the setup works.
fn run_test() {
    let config = config::load();
    if config.bot_token.is_empty() || config.chat_id.is_empty() {
        eprintln!("Set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID in ~/.claude/hooks/ghost-code.env first.");
        eprintln!("Run 'ghost-code setup' if you haven't yet.");
        std::process::exit(1);
    }

    println!("Testing Stop hook...");
    let stop_msg = format!(
        "\u{1f916} <b>Claude Code \u{b7} test@{}</b>\n\n{}",
        telegram::escape_html(&hook::short_hostname()),
        "Refactored the swap router to support multi-hop paths. (test message)",
    );
    telegram::send_html_silent(&config.bot_token, &config.chat_id, &stop_msg, None);

    println!("Testing Notification hook...");
    let notif_msg = format!(
        "\u{26a1} <b>Claude Code \u{b7} test@{}</b>\n\n{}",
        telegram::escape_html(&hook::short_hostname()),
        "Permission needed: cargo build (test message)",
    );
    telegram::send_html(&config.bot_token, &config.chat_id, &notif_msg, None, None);

    println!("Done. Check Telegram for two test messages.");
}

fn home_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}
