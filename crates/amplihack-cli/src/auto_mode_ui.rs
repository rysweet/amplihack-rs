//! Native terminal rendering helpers and live UI loop for auto-mode UI.

use crate::auto_mode_state::AutoModeState;
use anyhow::{Result, bail};
use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const MAX_TITLE_LEN: usize = 50;
const MAX_LOG_LINES: usize = 50;

pub struct AutoModeUiHandle {
    active: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AutoModeUiHandle {
    pub fn start(
        state: Arc<AutoModeState>,
        prompt: String,
        active: Arc<AtomicBool>,
    ) -> Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            bail!("--ui requires an interactive terminal");
        }

        let thread_active = Arc::clone(&active);
        let thread_state = Arc::clone(&state);
        let thread = thread::Builder::new()
            .name("auto-mode-ui".to_string())
            .spawn(move || run_live_ui_loop(thread_state, prompt, thread_active))
            .map_err(|error| anyhow::anyhow!("failed to spawn auto-mode UI thread: {error}"))?;

        Ok(Self {
            active,
            thread: Some(thread),
        })
    }

    pub fn finish(mut self) {
        self.active.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn generate_title_from_prompt(prompt: &str) -> String {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return "Auto Mode Session".to_string();
    }
    if trimmed.chars().count() <= MAX_TITLE_LEN {
        return trimmed.to_string();
    }
    let mut title = trimmed.chars().take(MAX_TITLE_LEN - 3).collect::<String>();
    title.push_str("...");
    title
}

pub fn render_auto_mode_frame(
    state: &AutoModeState,
    prompt: &str,
    showing_help: bool,
    queued_inputs: usize,
) -> String {
    let snapshot = state.snapshot();
    let title = generate_title_from_prompt(prompt);
    let elapsed = format_elapsed(snapshot.start_time);
    let status = format_status(&snapshot.status);
    let input_tokens = snapshot.costs.input_tokens;
    let output_tokens = snapshot.costs.output_tokens;
    let estimated_cost = snapshot.costs.estimated_cost;

    let mut lines = vec![
        format!("=== {} ===", title),
        format!(
            "Turn: {}/{} | Time: {} | Status: {}",
            snapshot.turn, snapshot.max_turns, elapsed, status
        ),
        format!(
            "Input: {} | Output: {} | Cost: ${:.4}",
            input_tokens, output_tokens, estimated_cost
        ),
        String::new(),
        "[Tasks]".to_string(),
    ];

    if snapshot.todos.is_empty() {
        lines.push("  No tasks yet".to_string());
    } else {
        for todo in &snapshot.todos {
            let status = todo.get("status").map(String::as_str).unwrap_or("pending");
            let content = todo
                .get("content")
                .or_else(|| todo.get("title"))
                .map(String::as_str)
                .unwrap_or("");
            lines.push(format!("  {} {}", todo_icon(status), content));
        }
    }

    lines.push(String::new());
    lines.push("[Logs]".to_string());
    if snapshot.logs.is_empty() {
        lines.push("  Waiting for logs...".to_string());
    } else {
        let start = snapshot.logs.len().saturating_sub(MAX_LOG_LINES);
        for log in &snapshot.logs[start..] {
            lines.push(format!("  {}", log));
        }
    }

    lines.push(String::new());
    lines.push("[Controls]".to_string());
    lines.push("  x = exit UI (auto mode continues)".to_string());
    lines.push("  h = toggle help".to_string());
    if queued_inputs > 0 {
        lines.push(format!("  queued instructions: {}", queued_inputs));
    }
    if showing_help {
        lines.push(String::new());
        lines.push("[Help]".to_string());
        lines.push("  Auto mode keeps running after UI exit.".to_string());
        lines.push("  Use --append to inject new instructions from another shell.".to_string());
    }

    lines.join("\n")
}

fn format_status(status: &str) -> String {
    match status {
        "running" => "▶ RUNNING".to_string(),
        "completed" => "✓ COMPLETED".to_string(),
        "error" => "✗ ERROR".to_string(),
        other => format!("◆ {}", other.to_ascii_uppercase()),
    }
}

fn todo_icon(status: &str) -> &'static str {
    match status {
        "completed" => "✓",
        "in_progress" => "▶",
        _ => "⏸",
    }
}

fn format_elapsed(start_time: f64) -> String {
    if start_time <= 0.0 {
        return "0s".to_string();
    }
    let elapsed = (chrono::Utc::now().timestamp_millis() as f64 / 1000.0 - start_time).max(0.0);
    if elapsed < 60.0 {
        format!("{}s", elapsed as u64)
    } else {
        format!("{}m {}s", (elapsed as u64) / 60, (elapsed as u64) % 60)
    }
}

fn run_live_ui_loop(state: Arc<AutoModeState>, prompt: String, active: Arc<AtomicBool>) {
    let _guard = match AutoModeTerminalGuard::activate() {
        Ok(guard) => guard,
        Err(error) => {
            state.add_log(format!("UI terminal setup failed: {error}"), true);
            active.store(false, Ordering::Release);
            return;
        }
    };

    let mut showing_help = false;
    while active.load(Ordering::Acquire) {
        let frame = render_auto_mode_frame(state.as_ref(), &prompt, showing_help, 0);
        if print_frame(&frame).is_err() {
            state.add_log("UI render failed".to_string(), true);
            break;
        }

        match read_auto_mode_key(Duration::from_millis(100)) {
            Some(AutoModeKey::Char('x')) | Some(AutoModeKey::Char('X')) => {
                state.add_log("UI exit requested (auto mode continues)".to_string(), true);
                active.store(false, Ordering::Release);
                break;
            }
            Some(AutoModeKey::Char('h')) | Some(AutoModeKey::Char('H')) => {
                showing_help = !showing_help;
                if showing_help {
                    state.add_log("Help: x=exit ui, h=help".to_string(), true);
                }
            }
            _ => {}
        }

        let status = state.status();
        if matches!(status.as_str(), "completed" | "error" | "stopped") {
            let final_frame = render_auto_mode_frame(state.as_ref(), &prompt, showing_help, 0);
            let _ = print_frame(&final_frame);
            thread::sleep(Duration::from_secs(2));
            break;
        }
    }

    active.store(false, Ordering::Release);
}

fn print_frame(frame: &str) -> io::Result<()> {
    print!("\x1b[2J\x1b[H{frame}");
    io::stdout().flush()
}

#[cfg(unix)]
struct AutoModeTerminalGuard {
    fd: i32,
    original: Option<libc::termios>,
}

#[cfg(unix)]
impl AutoModeTerminalGuard {
    fn activate() -> Result<Self> {
        let fd = io::stdin().as_raw_fd();
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            bail!("failed to read terminal attributes");
        }
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            bail!("failed to enable auto-mode raw mode");
        }

        print!("\x1b[?1049h\x1b[?25l");
        io::stdout().flush()?;

        Ok(Self {
            fd,
            original: Some(original),
        })
    }
}

#[cfg(unix)]
impl Drop for AutoModeTerminalGuard {
    fn drop(&mut self) {
        if let Some(original) = self.original.take() {
            let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &original) };
        }
        let _ = io::stdout().write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = io::stdout().flush();
    }
}

#[cfg(not(unix))]
struct AutoModeTerminalGuard;

#[cfg(not(unix))]
impl AutoModeTerminalGuard {
    fn activate() -> Result<Self> {
        Ok(Self)
    }
}

#[cfg(not(unix))]
impl Drop for AutoModeTerminalGuard {
    fn drop(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoModeKey {
    Char(char),
    Left,
    Right,
    Up,
    Down,
}

fn decode_auto_mode_key_bytes(bytes: &[u8]) -> Option<AutoModeKey> {
    match bytes {
        [byte] => Some(AutoModeKey::Char(*byte as char)),
        [0x1b, b'[', b'A'] => Some(AutoModeKey::Up),
        [0x1b, b'[', b'B'] => Some(AutoModeKey::Down),
        [0x1b, b'[', b'C'] => Some(AutoModeKey::Right),
        [0x1b, b'[', b'D'] => Some(AutoModeKey::Left),
        _ => None,
    }
}

#[cfg(unix)]
fn read_auto_mode_key(timeout: Duration) -> Option<AutoModeKey> {
    let fd = io::stdin().as_raw_fd();
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    if ready <= 0 || (poll_fd.revents & libc::POLLIN) == 0 {
        return None;
    }

    let mut first = [0u8; 1];
    if unsafe { libc::read(fd, first.as_mut_ptr().cast(), 1) } != 1 {
        return None;
    }
    if first[0] != 0x1b {
        return decode_auto_mode_key_bytes(&first);
    }

    let mut bytes = vec![first[0]];
    for _ in 0..2 {
        let mut extra_poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut extra_poll, 1, 5) };
        if ready <= 0 || (extra_poll.revents & libc::POLLIN) == 0 {
            break;
        }
        let mut next = [0u8; 1];
        if unsafe { libc::read(fd, next.as_mut_ptr().cast(), 1) } != 1 {
            break;
        }
        bytes.push(next[0]);
    }

    decode_auto_mode_key_bytes(&bytes)
}

#[cfg(not(unix))]
fn read_auto_mode_key(timeout: Duration) -> Option<AutoModeKey> {
    thread::sleep(timeout);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_mode_state::AutoModeState;
    use std::collections::BTreeMap;

    fn todo(status: &str, content: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("status".to_string(), status.to_string()),
            ("content".to_string(), content.to_string()),
        ])
    }

    #[test]
    fn generate_title_from_prompt_truncates_long_prompt() {
        let title = generate_title_from_prompt(
            "Implement a very long prompt that definitely exceeds fifty characters for display",
        );
        assert!(title.ends_with("..."));
        assert!(title.chars().count() <= 50);
    }

    #[test]
    fn render_auto_mode_frame_includes_status_tasks_logs_and_help() {
        let state = AutoModeState::new("session-1", 10, "Ship parity");
        state.update_turn(3);
        state.update_todos(vec![
            todo("completed", "Audit hooks"),
            todo("in_progress", "Port auto mode"),
            todo("pending", "Validate"),
        ]);
        state.update_costs(120, 34, 0.42);
        state.add_log("Started auto mode", false);
        state.add_log("Working on parity", false);

        let frame = render_auto_mode_frame(&state, "Ship parity", true, 2);

        assert!(frame.contains("=== Ship parity ==="));
        assert!(frame.contains("Turn: 3/10"));
        assert!(frame.contains("✓ Audit hooks"));
        assert!(frame.contains("▶ Port auto mode"));
        assert!(frame.contains("⏸ Validate"));
        assert!(frame.contains("Started auto mode"));
        assert!(frame.contains("queued instructions: 2"));
        assert!(frame.contains("Use --append to inject new instructions"));
    }

    #[test]
    fn decode_auto_mode_key_bytes_handles_arrows_and_chars() {
        assert_eq!(
            decode_auto_mode_key_bytes(&[0x1b, b'[', b'A']),
            Some(AutoModeKey::Up)
        );
        assert_eq!(
            decode_auto_mode_key_bytes(b"x"),
            Some(AutoModeKey::Char('x'))
        );
    }
}
