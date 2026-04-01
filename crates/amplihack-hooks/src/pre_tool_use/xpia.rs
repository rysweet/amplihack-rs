//! XPIA security integration for pre-tool-use hook.

use amplihack_security::{ContentType, RiskLevel, XpiaDefender};
use serde_json::Value;
use tracing::warn;

/// Run XPIA validation on a tool call. Returns `Some(block_json)` if blocked.
pub(super) fn check_xpia(tool_name: &str, tool_input: &Value) -> Option<Value> {
    let defender = XpiaDefender::from_env();
    if !defender.is_enabled() {
        return None;
    }

    match tool_name {
        "WebFetch" => check_webfetch(&defender, tool_input),
        "Bash" => check_bash(&defender, tool_input),
        _ => check_general(&defender, tool_name, tool_input),
    }
}

fn check_webfetch(defender: &XpiaDefender, params: &Value) -> Option<Value> {
    let url = params.get("url").and_then(Value::as_str).unwrap_or("");
    let prompt = params
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("");

    if url.is_empty() && prompt.is_empty() {
        return None;
    }

    let result = defender.validate_webfetch(url, prompt);
    if result.should_block {
        warn!(
            risk = %result.risk_level,
            threats = result.threats.len(),
            url = url,
            "XPIA blocked WebFetch request"
        );
        return Some(block_response(&result.threat_summary(), result.risk_level));
    }
    None
}

fn check_bash(defender: &XpiaDefender, params: &Value) -> Option<Value> {
    let command = params
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("");

    if command.is_empty() {
        return None;
    }

    let result = defender.validate_bash(command);
    if result.should_block {
        warn!(
            risk = %result.risk_level,
            threats = result.threats.len(),
            "XPIA blocked Bash command"
        );
        return Some(block_response(&result.threat_summary(), result.risk_level));
    }
    None
}

fn check_general(defender: &XpiaDefender, tool_name: &str, params: &Value) -> Option<Value> {
    let content = params.to_string();
    if content.len() < 10 {
        return None;
    }

    let result = defender.validate_content(&content, ContentType::ToolParameters);
    if result.should_block {
        warn!(
            risk = %result.risk_level,
            tool = tool_name,
            "XPIA blocked tool call"
        );
        return Some(block_response(&result.threat_summary(), result.risk_level));
    }
    None
}

fn block_response(summary: &str, risk: RiskLevel) -> Value {
    serde_json::json!({
        "block": true,
        "message": format!(
            "🛡️ XPIA Security Alert: Request blocked due to potential security risk.\n\
             Risk Level: {risk}\n\
             {summary}\n\n\
             Please review and modify your request."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_bash_not_blocked() {
        let result = check_xpia("Bash", &serde_json::json!({"command": "ls -la"}));
        assert!(result.is_none());
    }

    #[test]
    fn safe_webfetch_not_blocked() {
        let result = check_xpia(
            "WebFetch",
            &serde_json::json!({"url": "https://github.com", "prompt": "read docs"}),
        );
        assert!(result.is_none());
    }

    #[test]
    fn empty_params_not_blocked() {
        let result = check_xpia("Bash", &serde_json::json!({}));
        assert!(result.is_none());
    }

    #[test]
    fn short_general_content_not_blocked() {
        let result = check_xpia("Read", &serde_json::json!({"path": "/tmp"}));
        assert!(result.is_none());
    }
}
