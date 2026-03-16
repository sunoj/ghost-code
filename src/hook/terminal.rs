// Ghostty terminal integration: tab detection and title management.
// Uses macOS Accessibility API via osascript to interact with Ghostty tabs.

pub fn set_tab_title(tty: &str, title: &str) {
    if tty.is_empty() || tty == "??" {
        return;
    }
    let safe: String = title.chars().filter(|c| !c.is_control()).collect();
    let path = format!("/dev/{tty}");
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&path) {
        use std::io::Write;
        write!(f, "\x1b]2;{safe}\x07").ok();
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
}

/// Detect which Ghostty tab owns the given tty by injecting a unique marker title.
pub fn detect_tab_by_tty(tty: &str) -> (i64, String) {
    if tty.is_empty() || tty == "??" {
        return (0, String::new());
    }

    let mut buf = [0u8; 4];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut buf);
    }
    let marker = format!("GC-{:08x}", u32::from_le_bytes(buf));

    let delays_ms = [80, 200, 400];
    let mut last_titles = String::new();
    for (attempt, &delay) in delays_ms.iter().enumerate() {
        set_tab_title(tty, &marker);
        std::thread::sleep(std::time::Duration::from_millis(delay));
        let (idx, titles) = detect_tab_index(&marker);
        last_titles = titles.clone();
        if idx > 0 {
            let method = if titles.is_empty() { "window-title" } else { "marker" };
            eprintln!(
                "[detect_tab_by_tty] found tab {idx} via {method} (attempt {})",
                attempt + 1,
            );
            return (idx, titles);
        }
        eprintln!(
            "[detect_tab_by_tty] attempt {}: marker={marker} not found, titles={titles:?}",
            attempt + 1,
        );
    }

    if last_titles.is_empty() {
        eprintln!("[detect_tab_by_tty] titles empty, activating Ghostty and retrying");
        std::process::Command::new("osascript")
            .args(["-e", r#"tell application "Ghostty" to activate"#])
            .output()
            .ok();
        std::thread::sleep(std::time::Duration::from_millis(1000));
        set_tab_title(tty, &marker);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let (idx, titles) = detect_tab_index(&marker);
        if idx > 0 {
            let method = if titles.is_empty() { "window-title" } else { "marker" };
            eprintln!("[detect_tab_by_tty] found tab {idx} via {method} (after activate)");
            return (idx, titles);
        }
        eprintln!("[detect_tab_by_tty] still failed after activate, titles={titles:?}");

        let tty_alive = std::path::Path::new(&format!("/dev/{tty}")).exists();
        if tty_alive {
            eprintln!("[detect_tab_by_tty] tty {tty} alive but tab not verifiable, refusing inject");
        }
    }

    (0, String::new())
}

pub const SCREEN_LOCKED_ERR: &str = "SCREEN_LOCKED";

/// Check if macOS screen is locked via ioreg.
pub fn is_screen_locked() -> bool {
    std::process::Command::new("ioreg")
        .args(["-n", "Root", "-d1"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("\"CGSSessionScreenIsLocked\" = Yes"))
        .unwrap_or(false)
}

/// Atomically find the Ghostty tab owning the given tty and inject text.
/// Sets a unique marker title on the tty, then a single AppleScript searches
/// all windows/tabs for the marker, clicks the correct tab, and pastes.
/// This eliminates the TOCTOU race between tab detection and injection.
pub fn atomic_inject(tty: &str, text: &str, restore_title: &str) -> Result<(), String> {
    if tty.is_empty() || tty == "??" {
        return Err("No tty available for tab detection".to_string());
    }
    if is_screen_locked() {
        return Err(SCREEN_LOCKED_ERR.to_string());
    }

    // Escape text for embedding in AppleScript string literal.
    // The clipboard is set inside the AppleScript, immediately before Cmd+V,
    // to eliminate the race window where an external process could overwrite it.
    let escaped_text = escape_applescript(text);

    let marker = generate_marker();

    let delays_ms = [150, 300, 500];
    for (attempt, &delay) in delays_ms.iter().enumerate() {
        set_tab_title(tty, &marker);
        std::thread::sleep(std::time::Duration::from_millis(delay));

        match run_find_and_inject(&marker, &escaped_text) {
            Ok(true) => {
                eprintln!(
                    "[atomic_inject] ok (attempt {}, marker={marker}, {} chars)",
                    attempt + 1,
                    text.len()
                );
                if !restore_title.is_empty() {
                    set_tab_title(tty, restore_title);
                }
                return Ok(());
            }
            Ok(false) => {
                eprintln!(
                    "[atomic_inject] attempt {}: marker={marker} not found",
                    attempt + 1,
                );
            }
            Err(e) => return Err(e),
        }
    }

    // Final attempt: activate Ghostty explicitly first
    eprintln!("[atomic_inject] activating Ghostty for final attempt");
    std::process::Command::new("osascript")
        .args(["-e", r#"tell application "Ghostty" to activate"#])
        .output()
        .ok();
    std::thread::sleep(std::time::Duration::from_millis(1000));
    set_tab_title(tty, &marker);
    std::thread::sleep(std::time::Duration::from_millis(500));

    match run_find_and_inject(&marker, &escaped_text) {
        Ok(true) => {
            eprintln!("[atomic_inject] ok (after activate, marker={marker})");
            if !restore_title.is_empty() {
                set_tab_title(tty, restore_title);
            }
            Ok(())
        }
        Ok(false) => Err("Tab not found — marker not visible in any Ghostty tab".to_string()),
        Err(e) => Err(e),
    }
}

/// Escape a string for safe embedding inside an AppleScript double-quoted string literal.
/// Backslashes and double quotes must be escaped; other characters (including Unicode) pass through.
fn escape_applescript(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

fn generate_marker() -> String {
    let mut buf = [0u8; 4];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut buf);
    }
    format!("GC-{:08x}", u32::from_le_bytes(buf))
}

/// Single AppleScript: search all Ghostty windows/tabs for marker, set clipboard, click, paste, Enter.
/// The clipboard is set inside the script to minimize the race window with external clipboard writes.
fn run_find_and_inject(marker: &str, escaped_text: &str) -> Result<bool, String> {
    let script = format!(
        r#"tell application "Ghostty" to activate
delay 0.3
tell application "System Events"
    tell process "Ghostty"
        set maxWait to 4
        repeat while (count of windows) is 0 and maxWait > 0
            delay 0.5
            set maxWait to maxWait - 1
        end repeat
        set winCount to count of windows
        if winCount is 0 then error "Ghostty has no windows"

        repeat with w from 1 to winCount
            set foundTab to 0
            tell window w
                try
                    tell (first tab group)
                        set tabButtons to every radio button
                        repeat with i from 1 to count of tabButtons
                            if title of item i of tabButtons contains "{marker}" then
                                set foundTab to i
                                exit repeat
                            end if
                        end repeat
                        if foundTab > 0 and (count of tabButtons) > 1 then
                            click radio button foundTab
                        end if
                    end tell
                end try
            end tell

            if foundTab > 0 then
                if w > 1 then
                    try
                        tell window w to perform action "AXRaise"
                    end try
                    delay 0.2
                end if
                delay 0.2
                set the clipboard to "{escaped_text}"
                keystroke "v" using command down
                delay 0.3
                key code 36
                return "ok"
            end if
        end repeat

        -- Fallback: single window, no tab bar (no radio buttons visible)
        if winCount is 1 then
            set winTitle to title of window 1
            if winTitle contains "{marker}" then
                delay 0.2
                set the clipboard to "{escaped_text}"
                keystroke "v" using command down
                delay 0.3
                key code 36
                return "ok"
            end if
        end if

        return "not_found"
    end tell
end tell"#
    );

    let output = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("{e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        Ok(stdout == "ok")
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if is_screen_locked() {
            return Err(SCREEN_LOCKED_ERR.to_string());
        }
        Err(stderr)
    }
}

pub fn detect_tab_index(tab_title: &str) -> (i64, String) {
    let output = std::process::Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to tell process "Ghostty"
    if (count of windows) is 0 then return ""
    tell window 1 to tell (first tab group) to get title of every radio button
end tell"#,
        ])
        .output()
        .ok();
    let titles = output
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    for (i, name) in titles.split(", ").enumerate() {
        if name.contains(tab_title) {
            return ((i + 1) as i64, titles);
        }
    }
    let tabs: Vec<&str> = titles.split(", ").filter(|s| !s.is_empty()).collect();
    if tabs.len() == 1 {
        if tabs[0].contains(tab_title) {
            eprintln!(
                "[detect_tab] single tab fallback: matched={tab_title:?}, actual={:?}",
                tabs[0]
            );
            return (1, titles);
        }
        eprintln!(
            "[detect_tab] single tab, title mismatch: expected={tab_title:?}, actual={:?}",
            tabs[0]
        );
        return (0, titles);
    }
    if tabs.is_empty() {
        let window_title = std::process::Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to tell process "Ghostty"
    if (count of windows) > 0 then
        return title of window 1
    end if
end tell"#,
            ])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if !window_title.is_empty() && window_title.contains(tab_title) {
            eprintln!(
                "[detect_tab] single window fallback (verified via window title), expected={tab_title:?}, window_title={window_title:?}"
            );
            return (1, String::new());
        }
        if !window_title.is_empty() {
            eprintln!(
                "[detect_tab] single window, title mismatch: expected={tab_title:?}, window={window_title:?}"
            );
            return (0, String::new());
        }
    }
    (0, titles)
}
