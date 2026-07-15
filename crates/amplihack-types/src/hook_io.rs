//! Hook input/output types — host-agnostic (Claude Code, Amplifier, Copilot).
//!
//! These types model the JSON protocol that hook hosts use to communicate
//! with hook binaries via stdin/stdout.

use serde::{Deserialize, Deserializer, de::Error as DeError};
use serde_json::Value;
use std::path::PathBuf;

/// Top-level input from the hook host.
///
/// Uses `#[serde(other)]` for forward-compatibility: unknown hook events
/// deserialize to `Unknown` instead of failing.
#[derive(Debug, Clone)]
pub enum HookInput {
    /// Pre-tool-use: decide whether to allow/deny a tool invocation.
    PreToolUse {
        tool_name: String,
        tool_input: Value,
        session_id: Option<String>,
    },

    /// Post-tool-use: observe tool results for metrics/validation.
    PostToolUse {
        tool_name: String,
        tool_input: Value,
        tool_result: Option<Value>,
        session_id: Option<String>,
    },

    /// Stop: session is ending, decide whether to block or allow.
    Stop {
        stop_hook_active: Option<bool>,
        transcript_path: Option<PathBuf>,
        session_id: Option<String>,
    },

    /// Session start: initialize session state.
    SessionStart {
        session_id: Option<String>,
        cwd: Option<PathBuf>,
        extra: Value,
    },

    /// Session stop: finalize session state.
    SessionStop {
        session_id: Option<String>,
        transcript_path: Option<PathBuf>,
        extra: Value,
    },

    /// User prompt submission: process user prompt before LLM call.
    UserPromptSubmit {
        user_prompt: Option<String>,
        session_id: Option<String>,
        extra: Value,
    },

    /// Pre-compact: context window is about to be compacted.
    PreCompact {
        session_id: Option<String>,
        transcript_path: Option<PathBuf>,
        extra: Value,
    },

    /// Unknown hook event — forward-compatibility.
    /// New hook events from the host deserialize here instead of failing.
    Unknown,
}

type JsonMap = serde_json::Map<String, Value>;

const EVENT_FIELDS: &[&str] = &["hook_event_name", "hookEventName"];
const TOOL_NAME_FIELDS: &[&str] = &["tool_name", "toolName"];
const TOOL_INPUT_FIELDS: &[&str] = &["tool_input", "toolInput", "toolArgs"];
const TOOL_RESULT_FIELDS: &[&str] = &["tool_result", "toolResult"];
const SESSION_ID_FIELDS: &[&str] = &["session_id", "sessionId"];
const STOP_HOOK_ACTIVE_FIELDS: &[&str] = &["stop_hook_active", "stopHookActive"];
const TRANSCRIPT_PATH_FIELDS: &[&str] = &["transcript_path", "transcriptPath"];
const CWD_FIELDS: &[&str] = &["cwd"];
const USER_PROMPT_FIELDS: &[&str] = &["user_prompt", "userPrompt"];

impl<'de> Deserialize<'de> for HookInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let Some(map) = value.as_object() else {
            return Ok(HookInput::Unknown);
        };

        let event = optional_typed_field::<String, D::Error>(map, EVENT_FIELDS)?
            .map(|name| normalize_hook_event_name(&name))
            .or_else(|| infer_hook_event_name(map));
        let extra = || extra_fields(map);

        match event.as_deref() {
            Some("pretooluse") => Ok(HookInput::PreToolUse {
                tool_name: required_typed_field(map, TOOL_NAME_FIELDS, "PreToolUse")?,
                tool_input: required_tool_input_field(map, TOOL_INPUT_FIELDS, "PreToolUse")?,
                session_id: optional_typed_field(map, SESSION_ID_FIELDS)?,
            }),
            Some("posttooluse") => Ok(HookInput::PostToolUse {
                tool_name: required_typed_field(map, TOOL_NAME_FIELDS, "PostToolUse")?,
                tool_input: required_tool_input_field(map, TOOL_INPUT_FIELDS, "PostToolUse")?,
                tool_result: optional_value_field(map, TOOL_RESULT_FIELDS),
                session_id: optional_typed_field(map, SESSION_ID_FIELDS)?,
            }),
            Some("stop") => Ok(HookInput::Stop {
                stop_hook_active: optional_typed_field(map, STOP_HOOK_ACTIVE_FIELDS)?,
                transcript_path: optional_typed_field(map, TRANSCRIPT_PATH_FIELDS)?,
                session_id: optional_typed_field(map, SESSION_ID_FIELDS)?,
            }),
            Some("sessionstart") => Ok(HookInput::SessionStart {
                session_id: optional_typed_field(map, SESSION_ID_FIELDS)?,
                cwd: optional_typed_field(map, CWD_FIELDS)?,
                extra: extra(),
            }),
            Some("sessionstop") => Ok(HookInput::SessionStop {
                session_id: optional_typed_field(map, SESSION_ID_FIELDS)?,
                transcript_path: optional_typed_field(map, TRANSCRIPT_PATH_FIELDS)?,
                extra: extra(),
            }),
            Some("userpromptsubmit") => Ok(HookInput::UserPromptSubmit {
                user_prompt: optional_typed_field(map, USER_PROMPT_FIELDS)?,
                session_id: optional_typed_field(map, SESSION_ID_FIELDS)?,
                extra: extra(),
            }),
            Some("precompact") => Ok(HookInput::PreCompact {
                session_id: optional_typed_field(map, SESSION_ID_FIELDS)?,
                transcript_path: optional_typed_field(map, TRANSCRIPT_PATH_FIELDS)?,
                extra: extra(),
            }),
            Some(_) | None => Ok(HookInput::Unknown),
        }
    }
}

fn find_field<'a>(map: &'a JsonMap, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| map.get(*name))
}

fn has_any_field(map: &JsonMap, names: &[&str]) -> bool {
    names.iter().any(|name| map.contains_key(*name))
}

fn optional_typed_field<T, E>(map: &JsonMap, names: &[&str]) -> Result<Option<T>, E>
where
    T: for<'de> Deserialize<'de>,
    E: DeError,
{
    let Some(value) = find_field(map, names) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    T::deserialize(value.clone()).map(Some).map_err(E::custom)
}

fn required_typed_field<T, E>(map: &JsonMap, names: &[&str], event: &str) -> Result<T, E>
where
    T: for<'de> Deserialize<'de>,
    E: DeError,
{
    optional_typed_field(map, names)?.ok_or_else(|| E::custom(missing_field_message(event, names)))
}

fn optional_value_field(map: &JsonMap, names: &[&str]) -> Option<Value> {
    find_field(map, names).cloned()
}

fn required_value_field<E>(map: &JsonMap, names: &[&str], event: &str) -> Result<Value, E>
where
    E: DeError,
{
    optional_value_field(map, names).ok_or_else(|| E::custom(missing_field_message(event, names)))
}

/// Extract the tool-input value, normalizing host-specific encodings.
///
/// Claude Code / Amplifier send `tool_input` as a JSON object. GitHub Copilot
/// CLI instead sends the tool input under `toolArgs` as a JSON-ENCODED STRING
/// (e.g. `"{\"command\":\"echo hi\"}"`). To keep downstream consumers like
/// `tool_input.get("command")` working across hosts, a string value is parsed
/// once via `serde_json::from_str` into a `Value`. Object-shaped inputs are
/// returned unchanged.
///
/// Parse failures are surfaced as a deserialization error (never swallowed): a
/// present-but-malformed `toolArgs` string must not be silently treated as
/// success. The raw payload is intentionally excluded from the error message to
/// avoid leaking potentially sensitive tool arguments.
fn required_tool_input_field<E>(map: &JsonMap, names: &[&str], event: &str) -> Result<Value, E>
where
    E: DeError,
{
    let value = required_value_field(map, names, event)?;
    match value {
        Value::String(raw) => serde_json::from_str::<Value>(&raw).map_err(|err| {
            E::custom(format!(
                "{event} payload field `{}` contained an invalid JSON string: {err}",
                names.join("`/`")
            ))
        }),
        other => Ok(other),
    }
}

fn missing_field_message(event: &str, names: &[&str]) -> String {
    format!(
        "{event} payload missing required field `{}`",
        names.join("`/`")
    )
}

fn normalize_hook_event_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn infer_hook_event_name(map: &JsonMap) -> Option<String> {
    if has_any_field(map, TOOL_RESULT_FIELDS) {
        return Some("posttooluse".to_string());
    }
    if has_any_field(map, TOOL_NAME_FIELDS) || has_any_field(map, TOOL_INPUT_FIELDS) {
        return Some("pretooluse".to_string());
    }
    None
}

fn extra_fields(map: &JsonMap) -> Value {
    let mut extra = map.clone();
    for field in [
        EVENT_FIELDS,
        TOOL_NAME_FIELDS,
        TOOL_INPUT_FIELDS,
        TOOL_RESULT_FIELDS,
        SESSION_ID_FIELDS,
        STOP_HOOK_ACTIVE_FIELDS,
        TRANSCRIPT_PATH_FIELDS,
        CWD_FIELDS,
        USER_PROMPT_FIELDS,
    ]
    .into_iter()
    .flatten()
    {
        extra.remove(*field);
    }
    Value::Object(extra)
}

/// Normalize generated executable shell or hook script content to LF-only.
///
/// Windows-native checkouts or CRLF-tainted templates can introduce carriage
/// returns that bash treats as part of the interpreter or command token. This
/// helper is intentionally narrow: apply it only to generated executable script
/// content at the write/staging boundary, not arbitrary repository files.
pub fn normalize_executable_script_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_pre_tool_use() {
        let json = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "ls -la"}
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(matches!(input, HookInput::PreToolUse { tool_name, .. } if tool_name == "Bash"));
    }

    #[test]
    fn deserialize_unknown_event() {
        let json = r#"{"hook_event_name": "FutureEvent", "data": "test"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(matches!(input, HookInput::Unknown));
    }

    #[test]
    fn deserialize_stop_with_defaults() {
        let json = r#"{"hook_event_name": "Stop"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(matches!(
            input,
            HookInput::Stop {
                stop_hook_active: None,
                transcript_path: None,
                session_id: None,
            }
        ));
    }

    #[test]
    fn deserialize_pre_tool_use_with_extra_fields() {
        let json = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "future_field": "should be ignored"
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(matches!(input, HookInput::PreToolUse { .. }));
    }

    #[test]
    fn deserialize_pre_tool_use_accepts_camel_case_host_aliases() {
        let json = r#"{
            "hookEventName": "PreToolUse",
            "toolName": "Bash",
            "toolInput": {"command": "git commit --no-verify -m test"},
            "sessionId": "session-123"
        }"#;

        let input: HookInput = serde_json::from_str(json).unwrap();

        assert!(matches!(
            input,
            HookInput::PreToolUse {
                tool_name,
                tool_input,
                session_id: Some(session_id),
            } if tool_name == "Bash"
                && tool_input["command"] == "git commit --no-verify -m test"
                && session_id == "session-123"
        ));
    }

    // -----------------------------------------------------------------------
    // Issue #912: Copilot CLI sends the tool input under the `toolArgs` key as a
    // JSON-ENCODED STRING (not an object). The prior deserializer only accepted
    // `tool_input`/`toolInput` object shapes, so every Copilot PreToolUse payload
    // failed to deserialize and the hook fail-opened to `{}` — silently disabling
    // ALL pre-tool-use protections. These tests pin the real Copilot payload
    // shape and the string→object normalization contract.
    // -----------------------------------------------------------------------

    #[test]
    fn deserialize_pre_tool_use_accepts_copilot_tool_args_string() {
        // Exact Copilot CLI shape: no hookEventName, `toolName`, and a
        // JSON-encoded STRING under `toolArgs`.
        let json = r#"{
            "sessionId": "abc-123",
            "timestamp": 1700000000,
            "cwd": "/repo",
            "toolName": "bash",
            "toolArgs": "{\"command\":\"echo hi\",\"description\":\"greet\"}"
        }"#;

        let input: HookInput = serde_json::from_str(json)
            .expect("real Copilot PreToolUse payload (stringified toolArgs) must deserialize");

        match input {
            HookInput::PreToolUse {
                tool_name,
                tool_input,
                session_id,
            } => {
                assert_eq!(tool_name, "bash");
                assert_eq!(
                    tool_input.get("command").and_then(Value::as_str),
                    Some("echo hi"),
                    "stringified toolArgs must be normalized into an object so \
                     downstream `tool_input.get(\"command\")` works: {tool_input}"
                );
                assert_eq!(
                    tool_input.get("description").and_then(Value::as_str),
                    Some("greet")
                );
                assert_eq!(session_id.as_deref(), Some("abc-123"));
            }
            other => panic!("expected PreToolUse, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_post_tool_use_accepts_copilot_tool_args_string() {
        // Copilot PostToolUse carries a result field alongside stringified
        // toolArgs; presence of a result triggers posttooluse inference.
        let json = r#"{
            "sessionId": "abc-456",
            "toolName": "bash",
            "toolArgs": "{\"command\":\"cargo test\"}",
            "toolResult": {"exit_code": 0}
        }"#;

        let input: HookInput = serde_json::from_str(json)
            .expect("real Copilot PostToolUse payload (stringified toolArgs) must deserialize");

        match input {
            HookInput::PostToolUse {
                tool_name,
                tool_input,
                tool_result,
                session_id,
            } => {
                assert_eq!(tool_name, "bash");
                assert_eq!(
                    tool_input.get("command").and_then(Value::as_str),
                    Some("cargo test"),
                    "stringified toolArgs must be normalized into an object: {tool_input}"
                );
                assert_eq!(tool_result.expect("tool_result present")["exit_code"], 0);
                assert_eq!(session_id.as_deref(), Some("abc-456"));
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_pre_tool_use_accepts_object_shaped_tool_args() {
        // Robustness: if a host ever sends `toolArgs` as an object (not a
        // stringified JSON), it must pass through unchanged.
        let json = r#"{
            "toolName": "Bash",
            "toolArgs": {"command": "ls -la"}
        }"#;

        let input: HookInput = serde_json::from_str(json)
            .expect("object-shaped toolArgs must be accepted as a tool-input alias");

        assert!(matches!(
            input,
            HookInput::PreToolUse { tool_name, tool_input, .. }
                if tool_name == "Bash" && tool_input["command"] == "ls -la"
        ));
    }

    #[test]
    fn deserialize_pre_tool_use_rejects_invalid_tool_args_string() {
        // A `toolArgs` string that is NOT valid JSON must surface as a
        // deserialize error — never silently swallowed into `{}` or Unknown.
        let json = r#"{
            "toolName": "bash",
            "toolArgs": "{not valid json"
        }"#;

        let err = serde_json::from_str::<HookInput>(json).expect_err(
            "a malformed toolArgs string must produce a deserialize error, not a silent success",
        );

        // The error must reference the field, not the raw payload contents
        // (avoid leaking potentially secret command text into telemetry).
        let msg = err.to_string();
        assert!(
            !msg.contains("not valid json"),
            "error message must not echo raw toolArgs payload content: {msg}"
        );
    }

    #[test]
    fn deserialize_claude_object_tool_input_still_works_unchanged() {
        // Regression guard: the existing Claude object-shaped `tool_input`
        // schema must keep deserializing identically after the toolArgs change.
        let json = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "git status"}
        }"#;

        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(matches!(
            input,
            HookInput::PreToolUse { tool_name, tool_input, .. }
                if tool_name == "Bash" && tool_input["command"] == "git status"
        ));
    }

    #[test]
    fn deserialize_post_tool_use_accepts_camel_case_host_aliases() {
        let json = r#"{
            "hookEventName": "PostToolUse",
            "toolName": "Bash",
            "toolInput": {"command": "cargo test"},
            "toolResult": {"exit_code": 0},
            "sessionId": "session-456"
        }"#;

        let input: HookInput = serde_json::from_str(json).unwrap();

        assert!(matches!(
            input,
            HookInput::PostToolUse {
                tool_name,
                tool_input,
                tool_result: Some(tool_result),
                session_id: Some(session_id),
            } if tool_name == "Bash"
                && tool_input["command"] == "cargo test"
                && tool_result["exit_code"] == 0
                && session_id == "session-456"
        ));
    }

    #[test]
    fn deserialize_stop_accepts_camel_case_optional_aliases() {
        let json = r#"{
            "hookEventName": "Stop",
            "stopHookActive": true,
            "transcriptPath": "/tmp/transcript.jsonl",
            "sessionId": "session-789"
        }"#;

        let input: HookInput = serde_json::from_str(json).unwrap();

        assert!(matches!(
            input,
            HookInput::Stop {
                stop_hook_active: Some(true),
                transcript_path: Some(transcript_path),
                session_id: Some(session_id),
            } if transcript_path.as_path() == std::path::Path::new("/tmp/transcript.jsonl")
                && session_id == "session-789"
        ));
    }

    #[test]
    fn deserialize_known_tool_event_missing_required_field_stays_invalid() {
        let missing_tool_input = r#"{
            "hookEventName": "PreToolUse",
            "toolName": "Bash"
        }"#;

        assert!(
            serde_json::from_str::<HookInput>(missing_tool_input).is_err(),
            "known PreToolUse payloads without required toolInput must fail instead of degrading to Unknown"
        );
    }

    #[test]
    fn deserialize_pre_tool_use_without_event_name() {
        let json = r#"{
            "tool_name": "Bash",
            "tool_input": {"command": "git merge --no-verify feature-branch"}
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(matches!(input, HookInput::PreToolUse { tool_name, .. } if tool_name == "Bash"));
    }

    #[test]
    fn deserialize_known_tool_shape_without_event_missing_required_field_stays_invalid() {
        let missing_tool_input = r#"{"toolName": "Bash"}"#;

        assert!(
            serde_json::from_str::<HookInput>(missing_tool_input).is_err(),
            "tool-like payloads without hookEventName must still require toolInput"
        );
    }

    #[test]
    fn executable_script_line_ending_normalization_converts_crlf_to_lf() {
        let script = "#!/usr/bin/env bash\r\nset -euo pipefail\r\necho ok\r\n";

        let normalized = normalize_executable_script_line_endings(script);

        assert_eq!(
            normalized,
            "#!/usr/bin/env bash\nset -euo pipefail\necho ok\n"
        );
        assert!(
            !normalized.as_bytes().contains(&b'\r'),
            "normalized executable script content must not contain carriage returns"
        );
    }

    #[test]
    fn executable_script_line_ending_normalization_converts_lone_cr_to_lf() {
        let script = "#!/usr/bin/env bash\rset -euo pipefail\recho ok\r";

        let normalized = normalize_executable_script_line_endings(script);

        assert_eq!(
            normalized,
            "#!/usr/bin/env bash\nset -euo pipefail\necho ok\n"
        );
        assert!(
            !normalized.as_bytes().contains(&b'\r'),
            "lone carriage returns must be normalized before bash sees generated hooks"
        );
    }

    #[test]
    fn executable_script_line_ending_normalization_preserves_lf_only_content() {
        let script = "#!/usr/bin/env bash\nset -euo pipefail\necho ok\n";

        let normalized = normalize_executable_script_line_endings(script);

        assert_eq!(normalized, script);
    }

    #[test]
    fn executable_script_line_ending_normalization_handles_mixed_inputs() {
        let script = "#!/usr/bin/env bash\r\nset -euo pipefail\necho before\recho after\r\n";

        let normalized = normalize_executable_script_line_endings(script);

        assert_eq!(
            normalized,
            "#!/usr/bin/env bash\nset -euo pipefail\necho before\necho after\n"
        );
        assert!(
            !normalized.as_bytes().contains(&b'\r'),
            "mixed CRLF, LF, and lone CR input must become LF-only"
        );
    }
}
