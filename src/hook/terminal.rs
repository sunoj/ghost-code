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
            eprintln!(
                "[detect_tab_by_tty] found tab {idx} via marker (attempt {})",
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
            eprintln!("[detect_tab_by_tty] found tab {idx} via marker (after activate)");
            return (idx, titles);
        }
        eprintln!("[detect_tab_by_tty] still failed after activate, titles={titles:?}");

        let tty_alive = std::path::Path::new(&format!("/dev/{tty}")).exists();
        if tty_alive {
            eprintln!("[detect_tab_by_tty] tty {tty} alive, falling back to tab 1");
            return (1, String::new());
        }
    }

    (0, String::new())
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
        eprintln!(
            "[detect_tab] single tab fallback: expected={tab_title:?}, actual={:?}",
            tabs[0]
        );
        return (1, titles);
    }
    if tabs.is_empty() {
        let has_window = std::process::Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to tell process "Ghostty"
    return (count of windows) > 0
end tell"#,
            ])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
            .unwrap_or(false);
        if has_window {
            eprintln!("[detect_tab] single window fallback (no tab bar), expected={tab_title:?}");
            return (1, String::new());
        }
    }
    (0, titles)
}
