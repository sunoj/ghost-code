// Claude CLI streaming: spawns `claude -p` and streams output to Telegram.
// Uses sendMessageDraft for real-time updates, then finalizes with editMessageText.

use crate::telegram;
use std::io::Read;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub(super) fn run_claude_streaming(message: &str, working_dir: &str, timeout: u64, token: &str, chat_id: &str) {
    let start = std::time::Instant::now();
    eprintln!("{} [claude] spawning: claude -p ({}chars) in {working_dir}", super::ts(), message.len());

    let mut child = match Command::new("claude")
        .args(["-p", message])
        .current_dir(working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => {
            eprintln!("{} [claude] spawned pid={}", super::ts(), c.id());
            c
        }
        Err(e) => {
            eprintln!("{} [claude] spawn failed: {e}", super::ts());
            telegram::send_text(token, chat_id, &format!("Error: {e}"));
            return;
        }
    };

    let stdout = child.stdout.take().unwrap();
    let buffer = Arc::new(Mutex::new(String::new()));
    let reader_done = Arc::new(AtomicBool::new(false));

    let buf_clone = buffer.clone();
    let done_clone = reader_done.clone();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut chunk = [0u8; 512];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(s) = std::str::from_utf8(&chunk[..n]) {
                        buf_clone.lock().unwrap().push_str(s);
                    }
                }
                Err(_) => break,
            }
        }
        done_clone.store(true, Ordering::SeqCst);
    });

    let mut last_sent_len = 0;
    let mut draft_count = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);

    loop {
        std::thread::sleep(std::time::Duration::from_millis(800));

        let current = buffer.lock().unwrap().clone();
        let is_done = reader_done.load(Ordering::SeqCst);

        if current.len() > last_sent_len {
            telegram::send_draft(token, chat_id, &current);
            draft_count += 1;
            last_sent_len = current.len();
            if draft_count % 10 == 0 {
                eprintln!("{} [claude] streaming... {}chars, {} drafts sent", super::ts(), current.len(), draft_count);
            }
        }

        if is_done {
            break;
        }

        if std::time::Instant::now() > deadline {
            eprintln!("{} [claude] timeout after {timeout}s, killing", super::ts());
            let _ = child.kill();
            let _ = child.wait();
            let current = buffer.lock().unwrap().clone();
            let msg = if current.is_empty() {
                format!("Timed out ({timeout}s limit).")
            } else {
                format!("{current}\n\n(timed out after {timeout}s)")
            };
            telegram::send_text(token, chat_id, &msg);
            return;
        }
    }

    let status = child.wait();
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        err.read_to_string(&mut stderr).ok();
    }
    let stderr = stderr.trim();
    let final_text = buffer.lock().unwrap().clone();
    let final_text = final_text.trim();
    let elapsed = start.elapsed();

    eprintln!(
        "{} [claude] done in {:.1}s ({} chars, {} drafts) status={:?}",
        super::ts(), elapsed.as_secs_f64(), final_text.len(), draft_count, status
    );

    let response = if !final_text.is_empty() {
        if !stderr.is_empty() && status.as_ref().map(|s| !s.success()).unwrap_or(false) {
            format!("{final_text}\n\n(stderr: {stderr})")
        } else {
            final_text.to_string()
        }
    } else if !stderr.is_empty() {
        stderr.to_string()
    } else {
        "No response.".to_string()
    };

    let html = telegram::markdown_to_html(&response);
    telegram::send_html(token, chat_id, &html, None, None);
}
