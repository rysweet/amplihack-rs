//! Auto mode UI data structures (simplified for Rust).
//!
//! Matches Python `amplihack/launcher/auto_mode_ui.py` data model.
//! The Rich TUI rendering is not ported — only the data layer and title
//! generation, input queue, and display-state API are provided so that
//! a Rust TUI (ratatui, crossterm, etc.) can be built on top.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::auto_mode_state::AutoModeState;

/// Simplified UI state for auto mode.
pub struct AutoModeUi {
    state: Arc<AutoModeState>,
    title: String,
    should_exit: bool,
    showing_help: bool,
    pending_input: VecDeque<String>,
}

impl AutoModeUi {
    /// Create a new UI wrapper.
    pub fn new(state: Arc<AutoModeState>, prompt: &str) -> Self {
        let title = generate_title(prompt);
        Self {
            state,
            title,
            should_exit: false,
            showing_help: false,
            pending_input: VecDeque::new(),
        }
    }

    /// Get the generated session title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Handle a single keyboard command character.
    pub fn handle_keyboard_input(&mut self, key: char) {
        match key.to_ascii_lowercase() {
            'x' => {
                self.should_exit = true;
                self.state
                    .add_log("UI exit requested (auto mode continues)", true);
            }
            'h' => {
                self.showing_help = !self.showing_help;
                if self.showing_help {
                    self.state.add_log("Help: x=exit ui, h=help", true);
                }
            }
            _ => {}
        }
    }

    /// Queue a new instruction for injection.
    pub fn submit_input(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        self.pending_input.push_back(text.to_string());
        self.state
            .add_log(&format!("Instruction queued: {}", text.trim()), true);
    }

    /// Check if UI should exit.
    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    /// Check if help overlay is showing.
    pub fn is_showing_help(&self) -> bool {
        self.showing_help
    }

    /// Check if there are pending instructions.
    pub fn has_pending_input(&self) -> bool {
        !self.pending_input.is_empty()
    }

    /// Get and remove the next pending instruction.
    pub fn get_pending_input(&mut self) -> Option<String> {
        self.pending_input.pop_front()
    }

    /// Get the log content as a single string.
    pub fn get_log_content(&self) -> String {
        self.state.get_logs(None).join("\n")
    }

    /// Get the input placeholder text.
    pub fn get_input_placeholder(&self) -> &str {
        "Type new instructions..."
    }
}

/// Generate short title from user prompt (max 50 chars).
pub fn generate_title(prompt: &str) -> String {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return "Auto Mode Session".to_string();
    }
    if trimmed.len() <= 50 {
        return trimmed.to_string();
    }
    format!("{}...", &trimmed[..47])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ui(prompt: &str) -> AutoModeUi {
        let state = Arc::new(AutoModeState::new("s1", 10, prompt));
        AutoModeUi::new(state, prompt)
    }

    #[test]
    fn title_empty_prompt() {
        assert_eq!(generate_title(""), "Auto Mode Session");
        assert_eq!(generate_title("  "), "Auto Mode Session");
    }

    #[test]
    fn title_short_prompt() {
        assert_eq!(generate_title("Build a REST API"), "Build a REST API");
    }

    #[test]
    fn title_long_prompt_truncated() {
        let long = "A".repeat(100);
        let title = generate_title(&long);
        assert!(title.len() <= 50);
        assert!(title.ends_with("..."));
    }

    #[test]
    fn handle_exit_key() {
        let mut ui = make_ui("test");
        assert!(!ui.should_exit());
        ui.handle_keyboard_input('x');
        assert!(ui.should_exit());
    }

    #[test]
    fn handle_help_key_toggles() {
        let mut ui = make_ui("test");
        assert!(!ui.is_showing_help());
        ui.handle_keyboard_input('h');
        assert!(ui.is_showing_help());
        ui.handle_keyboard_input('h');
        assert!(!ui.is_showing_help());
    }

    #[test]
    fn submit_and_get_input() {
        let mut ui = make_ui("test");
        assert!(!ui.has_pending_input());
        assert!(ui.get_pending_input().is_none());

        ui.submit_input("do something");
        assert!(ui.has_pending_input());
        assert_eq!(ui.get_pending_input().unwrap(), "do something");
        assert!(!ui.has_pending_input());
    }

    #[test]
    fn submit_empty_ignored() {
        let mut ui = make_ui("test");
        ui.submit_input("");
        ui.submit_input("   ");
        assert!(!ui.has_pending_input());
    }

    #[test]
    fn input_placeholder() {
        let ui = make_ui("test");
        assert_eq!(ui.get_input_placeholder(), "Type new instructions...");
    }

    #[test]
    fn log_content_from_state() {
        let state = Arc::new(AutoModeState::new("s1", 10, "test"));
        state.add_log("hello", false);
        state.add_log("world", false);
        let ui = AutoModeUi::new(state, "test");
        let content = ui.get_log_content();
        assert!(content.contains("hello"));
        assert!(content.contains("world"));
    }
}
