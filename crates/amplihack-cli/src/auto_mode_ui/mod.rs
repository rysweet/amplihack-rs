//! Native terminal rendering helpers and live UI loop for auto-mode UI.

mod render;
mod terminal;

pub use render::{generate_title_from_prompt, render_auto_mode_frame};

use crate::auto_mode_state::AutoModeState;
use anyhow::{Result, bail};
use std::io::{self, IsTerminal, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use terminal::{AutoModeKey, AutoModeTerminalGuard, read_auto_mode_key};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_mode_state::AutoModeState;
    use std::collections::BTreeMap;
    use terminal::decode_auto_mode_key_bytes;

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
            Some(terminal::AutoModeKey::Up)
        );
        assert_eq!(
            decode_auto_mode_key_bytes(b"x"),
            Some(terminal::AutoModeKey::Char('x'))
        );
    }
}
