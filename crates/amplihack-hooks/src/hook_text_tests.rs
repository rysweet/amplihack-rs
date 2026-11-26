use super::*;
use serde_json::json;

#[test]
fn only_exact_none_suppresses_text() {
    assert_eq!(parse(None).setting, HookText::Default);
    assert!(parse(None).record.is_none());
    assert_eq!(parse(Some(OsStr::new("none"))).setting, HookText::None);
    assert!(parse(Some(OsStr::new("none"))).record.is_none());
    for value in [
        "",
        " ",
        "NONE",
        "None",
        " none",
        "none\n",
        "full",
        "development",
        "operations",
        "step",
        "off",
    ] {
        let parsed = parse(Some(OsStr::new(value)));
        assert_eq!(parsed.setting, HookText::Default, "{value:?}");
        assert!(parsed.record.is_some(), "{value:?}");
    }
}

#[test]
fn unknown_diagnostics_are_bounded_single_lines() {
    for value in [
        "bad\nvalue",
        "bad\x1b[31m",
        "x\u{0085}y",
        "x\u{2028}y",
        &"é".repeat(200),
        &"😀".repeat(200),
    ] {
        let record = parse(Some(OsStr::new(value))).record.unwrap();
        assert_eq!(record.lines().count(), 1);
        assert!(!record.chars().any(char::is_control));
        assert!(!record.contains('\u{2028}'));
        assert!(record.chars().count() < 256);
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_is_an_unknown_value() {
    use std::os::unix::ffi::OsStrExt;
    let parsed = parse(Some(OsStr::from_bytes(b"none\xff")));
    assert_eq!(parsed.setting, HookText::Default);
    assert!(parsed.record.is_some());
}

#[test]
fn actions_preserve_default_and_only_shortcut_stop() {
    for category in [
        ModelText::SessionContext,
        ModelText::PromptContext,
        ModelText::ClassificationReminder,
        ModelText::StopDecision,
    ] {
        assert_eq!(action(HookText::Default, category), Action::Pass);
        assert_eq!(
            action(HookText::None, category),
            if category == ModelText::StopDecision {
                Action::Approve
            } else {
                Action::StripContext
            }
        );
    }
}

#[test]
fn removal_preserves_control_fields_and_diagnostics() {
    let output = json!({
        "decision": "block", "reason": "r", "continue": false, "stopReason": "s",
        "additionalContext": "context", "systemMessage": "notice", "warnings": ["w"],
        "hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "context",
            "permissionDecision": "deny", "indexing_status": "started", "custom": {"additionalContext": "data"}}
    });
    let expected = json!({
        "decision": "block", "reason": "r", "continue": false, "stopReason": "s",
        "systemMessage": "notice", "warnings": ["w"],
        "hookSpecificOutput": {"hookEventName": "SessionStart", "permissionDecision": "deny",
            "indexing_status": "started", "custom": {"additionalContext": "data"}}
    });
    assert_eq!(strip_model_text(output), expected);
    assert_eq!(strip_model_text(expected.clone()), expected);
}

#[test]
fn empty_context_envelopes_are_removed() {
    assert_eq!(
        strip_model_text(
            json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": "context"}, "systemMessage": "notice"})
        ),
        json!({"systemMessage": "notice"})
    );
}

#[cfg(unix)]
#[test]
fn suppression_keeps_capacity_notices_from_both_context_hooks() {
    use crate::capacity_notice::tests::{Capacity, HOUR};
    use crate::protocol::Hook;
    use amplihack_types::HookInput;
    use amplihack_utils::claude_capacity::State;

    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for session_start in [true, false] {
        let capacity = Capacity::new(true);
        capacity.plant(State::Exhausted, Some(HOUR));
        let output = if session_start {
            crate::session_start::SessionStartHook.process(HookInput::SessionStart {
                session_id: Some("capacity-session".into()),
                cwd: None,
                extra: Value::Null,
            })
        } else {
            crate::user_prompt::UserPromptSubmitHook::with_prompt_root(|| None).process(
                HookInput::UserPromptSubmit {
                    user_prompt: Some("continue".into()),
                    session_id: Some("capacity-session".into()),
                    extra: Value::Null,
                },
            )
        }
        .unwrap();
        assert!(output["systemMessage"].is_string(), "{output}");
        let stripped = strip_model_text(output.clone());
        assert_eq!(stripped["systemMessage"], output["systemMessage"]);
        assert!(stripped.get("additionalContext").is_none());
        assert!(
            stripped
                .pointer("/hookSpecificOutput/additionalContext")
                .is_none()
        );
    }
}

#[test]
fn unrelated_output_shapes_are_preserved() {
    for value in [
        json!(null),
        json!("data"),
        json!([1, 2]),
        json!({"hookSpecificOutput": "data"}),
    ] {
        assert_eq!(strip_model_text(value.clone()), value);
    }
}
