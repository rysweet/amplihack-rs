//! Session start detection for triggering workflow classification.

use serde_json::Value;

/// Detects when workflow classification should be triggered.
pub struct SessionStartDetector;

impl Default for SessionStartDetector {
    fn default() -> Self {
        Self
    }
}

impl SessionStartDetector {
    pub fn new() -> Self {
        Self
    }

    /// Check if this is a session start requiring classification.
    pub fn is_session_start(&self, context: &Value) -> bool {
        // Explicit commands bypass classification
        if context
            .get("is_explicit_command")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return false;
        }

        // Slash commands bypass classification
        let user_request = context
            .get("user_request")
            .or_else(|| context.get("prompt"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if user_request.trim_start().starts_with('/') {
            return false;
        }

        // First message requires classification
        context
            .get("is_first_message")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Check if classification should be bypassed.
    pub fn should_bypass(&self, context: &Value) -> bool {
        // Explicit commands always bypass
        if context
            .get("is_explicit_command")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return true;
        }

        // Slash commands bypass
        let user_request = context
            .get("user_request")
            .or_else(|| context.get("prompt"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if user_request.trim_start().starts_with('/') {
            return true;
        }

        // Follow-up messages bypass
        !context
            .get("is_first_message")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Return the reason for bypassing, if applicable.
    pub fn bypass_reason(&self, context: &Value) -> Option<&'static str> {
        if context
            .get("is_explicit_command")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Some("explicit_command");
        }

        let user_request = context
            .get("user_request")
            .or_else(|| context.get("prompt"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if user_request.trim_start().starts_with('/') {
            return Some("explicit_command");
        }

        if !context
            .get("is_first_message")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Some("follow_up_message");
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn first_message_is_session_start() {
        let d = SessionStartDetector::new();
        let ctx = json!({"is_first_message": true, "prompt": "implement X"});
        assert!(d.is_session_start(&ctx));
    }

    #[test]
    fn follow_up_is_not_session_start() {
        let d = SessionStartDetector::new();
        let ctx = json!({"is_first_message": false});
        assert!(!d.is_session_start(&ctx));
    }

    #[test]
    fn explicit_command_bypasses() {
        let d = SessionStartDetector::new();
        let ctx = json!({"is_first_message": true, "is_explicit_command": true});
        assert!(!d.is_session_start(&ctx));
        assert!(d.should_bypass(&ctx));
    }

    #[test]
    fn slash_command_bypasses() {
        let d = SessionStartDetector::new();
        let ctx = json!({"is_first_message": true, "prompt": "/dev fix it"});
        assert!(!d.is_session_start(&ctx));
        assert!(d.should_bypass(&ctx));
    }

    #[test]
    fn bypass_reason_explicit() {
        let d = SessionStartDetector::new();
        let ctx = json!({"is_explicit_command": true});
        assert_eq!(d.bypass_reason(&ctx), Some("explicit_command"));
    }

    #[test]
    fn bypass_reason_follow_up() {
        let d = SessionStartDetector::new();
        let ctx = json!({"is_first_message": false});
        assert_eq!(d.bypass_reason(&ctx), Some("follow_up_message"));
    }

    #[test]
    fn no_bypass_for_first_message() {
        let d = SessionStartDetector::new();
        let ctx = json!({"is_first_message": true, "prompt": "fix the bug"});
        assert_eq!(d.bypass_reason(&ctx), None);
    }
}
