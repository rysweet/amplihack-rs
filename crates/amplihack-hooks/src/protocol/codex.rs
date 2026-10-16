//! Native Codex payload mapping and strict security response transport.
use amplihack_types::HookInput;

pub(super) fn normalize_codex_input(input: HookInput) -> HookInput {
    let shell = |name: String, mut args: serde_json::Value| {
        if matches!(name.as_str(), "shell_command" | "exec_command" | "shell") {
            if args.get("command").is_none()
                && let Some(cmd) = args.get("cmd").cloned()
            {
                args["command"] = cmd;
            }
            ("Bash".to_string(), args)
        } else {
            (name, args)
        }
    };
    match input {
        HookInput::PreToolUse {
            tool_name,
            tool_input,
            session_id,
        } => {
            let (tool_name, tool_input) = shell(tool_name, tool_input);
            HookInput::PreToolUse {
                tool_name,
                tool_input,
                session_id,
            }
        }
        HookInput::PostToolUse {
            tool_name,
            tool_input,
            tool_result,
            session_id,
        } => {
            let (tool_name, tool_input) = shell(tool_name, tool_input);
            HookInput::PostToolUse {
                tool_name,
                tool_input,
                tool_result,
                session_id,
            }
        }
        HookInput::SessionStop {
            session_id, extra, ..
        } => HookInput::SessionStop {
            session_id,
            transcript_path: None,
            extra,
        },
        other => other,
    }
}

/// Emit only fields supported by the native event. An explicit denial always
/// wins over advisory context or shared generic approval fields.
pub(super) fn codex_output(event: &str, output: serde_json::Value) -> serde_json::Value {
    use serde_json::json;
    if event == "SessionEnd" {
        return json!({});
    }
    if event == "PreToolUse" {
        if output
            .pointer("/hookSpecificOutput/permissionDecision")
            .and_then(serde_json::Value::as_str)
            == Some("deny")
        {
            return json!({"hookSpecificOutput": output["hookSpecificOutput"]});
        }
        if output.get("block").and_then(serde_json::Value::as_bool) == Some(true)
            || output.get("decision").and_then(serde_json::Value::as_str) == Some("block")
        {
            return json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse", "permissionDecision": "deny",
                "permissionDecisionReason": output.get("message").or_else(|| output.get("reason"))
                    .and_then(serde_json::Value::as_str).unwrap_or("Amplihack security check denied this tool")
            }});
        }
    }
    if matches!(
        event,
        "SessionStart" | "UserPromptSubmit" | "PreToolUse" | "PostToolUse"
    ) && let Some(context) = output
        .pointer("/hookSpecificOutput/additionalContext")
        .and_then(serde_json::Value::as_str)
    {
        return json!({"hookSpecificOutput":{"hookEventName":event,"additionalContext":context}});
    }
    json!({})
}

/// Read all of stdin as a string.
fn security_denial() -> serde_json::Value {
    serde_json::json!({"hookSpecificOutput":{"hookEventName":"PreToolUse",
        "permissionDecision":"deny", "permissionDecisionReason":"Amplihack security check could not process tool input"}})
}

/// Security responses must reach the host completely, including the newline and flush.
pub(super) fn write_security_response(data: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(data)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

/// Exit 2 is the native blocking fallback when JSON cannot be delivered.
pub(super) fn deny_on_failure() {
    if write_security_response(&serde_json::to_vec(&security_denial()).expect("static denial"))
        .is_err()
    {
        use std::io::Write;
        let mut stderr = std::io::stderr().lock();
        let _ = stderr
            .write_all(b"Amplihack security response delivery failed; tool execution blocked\n");
        let _ = stderr.flush();
        std::process::exit(2);
    }
}
