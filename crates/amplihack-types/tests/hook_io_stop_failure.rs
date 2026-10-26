//! Issue #1658 (B): `HookInput::StopFailure`, the payload Claude Code sends
//! when a turn ends on an API error (rate limit, overload, auth failure).
//! Parsed by hand like the other variants, with snake_case and camelCase
//! aliases, but loosely (issue #1659): a wrongly typed field reads as absent.

use amplihack_types::HookInput;
use serde_json::{Value, json};
use std::path::Path;

fn parse(v: Value) -> Result<HookInput, serde_json::Error> {
    serde_json::from_value(v)
}

#[test]
fn full_stop_failure_payload() {
    let input = parse(json!({
        "hook_event_name": "StopFailure",
        "session_id": "s-1",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/repo",
        "error": "rate_limit",
        "error_details": "429 Too Many Requests",
        "last_assistant_message": "You've hit your weekly limit · resets 3pm",
        "stop_hook_active": false
    }))
    .unwrap();
    match input {
        HookInput::StopFailure {
            session_id,
            error,
            error_details,
            last_assistant_message,
            stop_hook_active,
            transcript_path,
            extra,
        } => {
            assert_eq!(session_id.as_deref(), Some("s-1"));
            assert_eq!(error.as_deref(), Some("rate_limit"));
            assert_eq!(error_details.as_deref(), Some("429 Too Many Requests"));
            assert_eq!(
                last_assistant_message.as_deref(),
                Some("You've hit your weekly limit · resets 3pm")
            );
            assert_eq!(stop_hook_active, Some(false));
            assert_eq!(transcript_path.as_deref(), Some(Path::new("/tmp/t.jsonl")));
            // Known fields are not repeated in `extra`; unknown ones are kept.
            assert_eq!(extra.get("cwd"), Some(&json!("/repo")));
            for known in [
                "hook_event_name",
                "session_id",
                "error",
                "error_details",
                "last_assistant_message",
                "stop_hook_active",
                "transcript_path",
            ] {
                assert!(extra.get(known).is_none(), "{known} leaked into extra");
            }
        }
        other => panic!("expected StopFailure, got {other:?}"),
    }
}

#[test]
fn minimal_stop_failure_payload_has_all_options_none() {
    let input = parse(json!({"hook_event_name": "StopFailure"})).unwrap();
    assert!(matches!(
        input,
        HookInput::StopFailure {
            session_id: None,
            error: None,
            error_details: None,
            last_assistant_message: None,
            stop_hook_active: None,
            transcript_path: None,
            ..
        }
    ));
}

#[test]
fn event_name_spellings_are_normalised() {
    for name in ["StopFailure", "stopFailure", "stop_failure", "STOP-FAILURE"] {
        let input = parse(json!({"hook_event_name": name, "error": "rate_limit"})).unwrap();
        assert!(
            matches!(input, HookInput::StopFailure { .. }),
            "{name} must map to StopFailure, got {input:?}"
        );
    }
}

#[test]
fn camel_case_aliases_are_accepted() {
    let input = parse(json!({
        "hookEventName": "StopFailure",
        "sessionId": "s-2",
        "error": "rate_limit",
        "errorDetails": "details",
        "lastAssistantMessage": "hit your limit",
        "stopHookActive": true,
        "transcriptPath": "/t"
    }))
    .unwrap();
    assert!(matches!(
        input,
        HookInput::StopFailure {
            session_id: Some(ref s),
            error: Some(ref e),
            error_details: Some(ref d),
            last_assistant_message: Some(ref m),
            stop_hook_active: Some(true),
            transcript_path: Some(_),
            ..
        } if s == "s-2" && e == "rate_limit" && d == "details" && m == "hit your limit"
    ));
}

#[test]
fn null_fields_are_none() {
    let input = parse(json!({
        "hook_event_name": "StopFailure",
        "error": null,
        "error_details": null,
        "last_assistant_message": null,
        "session_id": null
    }))
    .unwrap();
    assert!(matches!(
        input,
        HookInput::StopFailure {
            error: None,
            error_details: None,
            last_assistant_message: None,
            session_id: None,
            ..
        }
    ));
}

#[test]
fn wrongly_typed_fields_are_dropped_and_the_rest_kept() {
    let input = parse(json!({
        "hook_event_name": "StopFailure",
        "session_id": "s-1",
        "error": "rate_limit",
        "error_details": {"code": 429},
        "last_assistant_message": ["not", "text"],
        "stop_hook_active": "yes",
        "transcript_path": 7
    }))
    .unwrap();
    let HookInput::StopFailure {
        session_id,
        error,
        error_details,
        last_assistant_message,
        stop_hook_active,
        transcript_path,
        ..
    } = input
    else {
        panic!("not StopFailure: {input:?}");
    };
    assert_eq!(session_id.as_deref(), Some("s-1"));
    assert_eq!(error.as_deref(), Some("rate_limit"));
    assert_eq!(error_details, None);
    assert_eq!(last_assistant_message, None);
    assert_eq!(stop_hook_active, None);
    assert_eq!(transcript_path, None);
}

#[test]
fn a_numeric_error_is_no_error() {
    let input = parse(json!({"hook_event_name": "StopFailure", "error": 429})).unwrap();
    assert!(matches!(input, HookInput::StopFailure { error: None, .. }));
}

#[test]
fn stop_payload_still_parses_as_stop() {
    // Regression: adding StopFailure must not capture plain Stop.
    let input = parse(json!({"hook_event_name": "Stop", "error": "rate_limit"})).unwrap();
    assert!(matches!(input, HookInput::Stop { .. }));
}

#[test]
fn payload_without_event_name_is_not_stop_failure() {
    // `error` alone does not infer an event.
    let input = parse(json!({"error": "rate_limit", "session_id": "s"})).unwrap();
    assert!(matches!(input, HookInput::Unknown));
}

/// Payload captured from Claude Code 2.1.306 during the live check (session
/// id and paths replaced). Claude Code adds `prompt_id` and an `effort`
/// object and sends no `stop_hook_active`; both extras must land in `extra`.
#[test]
fn live_claude_code_2_1_306_payload_parses() {
    let input = parse(json!({
        "session_id": "00000000-0000-4000-8000-000000000000",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/tmp/proj",
        "prompt_id": "11111111-1111-4111-8111-111111111111",
        "effort": {"level": "medium"},
        "hook_event_name": "StopFailure",
        "error": "authentication_failed",
        "last_assistant_message": "Invalid API key · Fix external API key"
    }))
    .expect("live payload must parse");
    match input {
        HookInput::StopFailure {
            session_id,
            error,
            error_details,
            last_assistant_message,
            stop_hook_active,
            extra,
            ..
        } => {
            assert_eq!(
                session_id.as_deref(),
                Some("00000000-0000-4000-8000-000000000000")
            );
            assert_eq!(error.as_deref(), Some("authentication_failed"));
            assert_eq!(error_details, None);
            assert_eq!(
                last_assistant_message.as_deref(),
                Some("Invalid API key · Fix external API key")
            );
            assert_eq!(stop_hook_active, None);
            assert_eq!(extra["effort"]["level"], "medium");
            assert!(extra.get("prompt_id").is_some());
        }
        other => panic!("expected StopFailure, got {other:?}"),
    }
}
