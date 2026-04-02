//! Session message capture for transcript export.
//!
//! Matches Python `amplihack/launcher/session_capture.py`:
//! - Capture user and assistant messages
//! - Phase/turn tracking for structured export
//! - JSON export format compatible with transcript builder

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// A captured message in the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: f64,
    pub turn: usize,
    pub phase: String,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// Message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

/// Session message capture for transcript generation.
pub struct MessageCapture {
    messages: Vec<CapturedMessage>,
    current_turn: usize,
    current_phase: String,
    session_id: String,
}

impl MessageCapture {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            messages: Vec::new(),
            current_turn: 0,
            current_phase: "init".into(),
            session_id: session_id.into(),
        }
    }

    /// Capture a user message.
    pub fn capture_user_message(&mut self, content: impl Into<String>) {
        self.current_turn += 1;
        self.messages.push(CapturedMessage {
            role: MessageRole::User,
            content: content.into(),
            timestamp: now_secs(),
            turn: self.current_turn,
            phase: self.current_phase.clone(),
            metadata: Default::default(),
        });
    }

    /// Capture an assistant response.
    pub fn capture_assistant_message(&mut self, content: impl Into<String>) {
        self.messages.push(CapturedMessage {
            role: MessageRole::Assistant,
            content: content.into(),
            timestamp: now_secs(),
            turn: self.current_turn,
            phase: self.current_phase.clone(),
            metadata: Default::default(),
        });
    }

    /// Capture a system message.
    pub fn capture_system_message(&mut self, content: impl Into<String>) {
        self.messages.push(CapturedMessage {
            role: MessageRole::System,
            content: content.into(),
            timestamp: now_secs(),
            turn: self.current_turn,
            phase: self.current_phase.clone(),
            metadata: Default::default(),
        });
    }

    /// Set the current phase label.
    pub fn set_phase(&mut self, phase: impl Into<String>) {
        self.current_phase = phase.into();
    }

    /// Get the current turn number.
    pub fn current_turn(&self) -> usize {
        self.current_turn
    }

    /// Get total message count.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Get all captured messages.
    pub fn messages(&self) -> &[CapturedMessage] {
        &self.messages
    }

    /// Export the session as a JSON transcript.
    pub fn export_json(&self) -> serde_json::Value {
        serde_json::json!({
            "session_id": self.session_id,
            "total_turns": self.current_turn,
            "message_count": self.messages.len(),
            "messages": self.messages,
            "exported_at": now_secs(),
        })
    }

    /// Export as a human-readable markdown transcript.
    pub fn export_markdown(&self) -> String {
        let mut md = format!("# Session Transcript: {}\n\n", self.session_id);
        md.push_str(&format!(
            "Turns: {} | Messages: {}\n\n---\n\n",
            self.current_turn,
            self.messages.len()
        ));

        for msg in &self.messages {
            let role_label = match msg.role {
                MessageRole::User => "**User**",
                MessageRole::Assistant => "**Assistant**",
                MessageRole::System => "**System**",
            };
            md.push_str(&format!(
                "### Turn {} ({}) [{}]\n\n",
                msg.turn, role_label, msg.phase
            ));
            md.push_str(&msg.content);
            md.push_str("\n\n---\n\n");
        }
        md
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_user_and_assistant() {
        let mut capture = MessageCapture::new("test-session");
        capture.capture_user_message("Hello, what is 2+2?");
        capture.capture_assistant_message("4");
        assert_eq!(capture.message_count(), 2);
        assert_eq!(capture.current_turn(), 1);
        assert_eq!(capture.messages()[0].role, MessageRole::User);
        assert_eq!(capture.messages()[1].role, MessageRole::Assistant);
    }

    #[test]
    fn turn_increments_on_user_message() {
        let mut capture = MessageCapture::new("s1");
        capture.capture_user_message("first");
        assert_eq!(capture.current_turn(), 1);
        capture.capture_assistant_message("response");
        assert_eq!(capture.current_turn(), 1); // assistant doesn't increment
        capture.capture_user_message("second");
        assert_eq!(capture.current_turn(), 2);
    }

    #[test]
    fn phase_tracking() {
        let mut capture = MessageCapture::new("s1");
        capture.set_phase("planning");
        capture.capture_user_message("plan this");
        capture.set_phase("execution");
        capture.capture_user_message("do this");
        assert_eq!(capture.messages()[0].phase, "planning");
        assert_eq!(capture.messages()[1].phase, "execution");
    }

    #[test]
    fn export_json_structure() {
        let mut capture = MessageCapture::new("s1");
        capture.capture_user_message("hello");
        capture.capture_assistant_message("hi");
        let json = capture.export_json();
        assert_eq!(json["session_id"], "s1");
        assert_eq!(json["total_turns"], 1);
        assert_eq!(json["message_count"], 2);
        assert!(json["messages"].is_array());
    }

    #[test]
    fn export_markdown_format() {
        let mut capture = MessageCapture::new("s1");
        capture.capture_user_message("What is Rust?");
        capture.capture_assistant_message("A systems programming language.");
        let md = capture.export_markdown();
        assert!(md.contains("# Session Transcript: s1"));
        assert!(md.contains("**User**"));
        assert!(md.contains("**Assistant**"));
        assert!(md.contains("What is Rust?"));
    }

    #[test]
    fn system_message_captured() {
        let mut capture = MessageCapture::new("s1");
        capture.capture_system_message("Context injected");
        assert_eq!(capture.messages()[0].role, MessageRole::System);
    }

    #[test]
    fn message_timestamps_are_recent() {
        let mut capture = MessageCapture::new("s1");
        capture.capture_user_message("test");
        let ts = capture.messages()[0].timestamp;
        let now = now_secs();
        assert!(
            (now - ts).abs() < 1.0,
            "timestamp should be within 1 second"
        );
    }
}
