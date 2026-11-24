//! Unit tests for `hook_text.rs` (AMPLIHACK_HOOK_TEXT).
//!
//! Attached from `hook_text.rs` the way `capacity_notice_tests.rs` is:
//!
//! ```rust,ignore
//! #[cfg(test)]
//! #[path = "hook_text_tests.rs"]
//! mod tests;
//! ```
//!
//! The contract these tests hold `hook_text.rs` to:
//!
//! - `pub(crate) const VAR: &str = "AMPLIHACK_HOOK_TEXT";`
//! - `pub(crate) const APPROVE`, the bytes `{"decision":"approve"}` (a `&str`
//!   or a `&[u8]`; the tests read it through `AsRef<[u8]>`);
//! - `pub enum ModelText { SessionContext, PromptContext,
//!   ClassificationReminder, StopDecision }`;
//! - `pub(crate) enum HookText { Full, None }`;
//! - `pub(crate) enum Action { Pass, StripContext, Approve }`;
//! - `pub(crate) struct Parsed { pub(crate) setting: HookText,
//!   pub(crate) record: Option<String> }`;
//! - `pub(crate) fn parse(value: Option<&OsStr>) -> Parsed`;
//! - `pub(crate) fn from_env() -> Parsed`, which is `parse` of the process's
//!   value of [`VAR`];
//! - `pub(crate) fn action(setting: HookText, text: ModelText) -> Action`;
//! - `pub(crate) fn strip_model_text(output: Value) -> Value`.
//!
//! The three enums derive `Debug, Clone, Copy, PartialEq, Eq`; `Parsed`
//! derives `Debug, Clone, PartialEq, Eq`.
//!
//! No test pins the wording of the unknown-value record: the tests check that
//! it is one line, names the variable, carries the value escaped and cut to
//! 64 characters, and holds no raw control character.

use super::*;
use crate::protocol::Hook;
use amplihack_types::HookInput;
use serde_json::{Value, json};
use std::ffi::OsStr;

const GOVERNED: [ModelText; 4] = [
    ModelText::SessionContext,
    ModelText::PromptContext,
    ModelText::ClassificationReminder,
    ModelText::StopDecision,
];

const SETTINGS: [HookText; 2] = [HookText::Full, HookText::None];

fn parse_str(value: &str) -> Parsed {
    parse(Some(OsStr::new(value)))
}

/// The record for `value`, which must be an unknown value.
fn record_for(value: &OsStr) -> String {
    let parsed = parse(Some(value));
    assert_eq!(
        parsed.setting,
        HookText::Full,
        "an unknown value behaves as full: {value:?}"
    );
    parsed
        .record
        .unwrap_or_else(|| panic!("an unknown value is recorded: {value:?}"))
}

/// One line, naming the variable, with no raw control character in it.
fn assert_one_clean_line(record: &str) {
    assert!(
        record.contains(VAR),
        "the record names the variable: {record:?}"
    );
    assert!(
        !record.chars().any(char::is_control),
        "the record holds no raw control character (a trailing newline is the writer's job): {record:?}"
    );
    assert_eq!(record.lines().count(), 1, "one line: {record:?}");
}

// ---- parsing ----

#[test]
fn the_variable_name_is_amplihack_hook_text() {
    assert_eq!(VAR, "AMPLIHACK_HOOK_TEXT");
}

#[test]
fn unset_empty_and_whitespace_mean_full_with_no_record() {
    assert_eq!(
        parse(None),
        Parsed {
            setting: HookText::Full,
            record: None
        }
    );
    for value in ["", " ", "   ", "\t", "\n", " \t\r\n "] {
        assert_eq!(
            parse_str(value),
            Parsed {
                setting: HookText::Full,
                record: None
            },
            "{value:?}"
        );
    }
}

#[test]
fn full_and_none_are_trimmed_and_matched_without_regard_to_ascii_case() {
    for value in ["full", "FULL", "Full", " full ", "\tfUlL\n"] {
        assert_eq!(
            parse_str(value),
            Parsed {
                setting: HookText::Full,
                record: None
            },
            "{value:?}"
        );
    }
    for value in ["none", "NONE", "None", " none ", "\tnOnE\n", "none\r\n"] {
        assert_eq!(
            parse_str(value),
            Parsed {
                setting: HookText::None,
                record: None
            },
            "{value:?}"
        );
    }
}

#[test]
fn every_other_value_is_full_with_one_record() {
    for value in [
        "bogus-value",
        "nonee",
        "no",
        "nothing",
        "off",
        "0",
        "false",
        "full none",
        "n one",
        "\u{feff}none",
        // Unicode case folding is not ASCII case: the Kelvin sign folds to `k`
        // in Unicode, and `ſ` (long s) to `s`. Neither must select anything.
        "ſull",
        "NONE\u{0307}",
    ] {
        let record = record_for(OsStr::new(value));
        assert_one_clean_line(&record);
    }
}

#[test]
fn the_record_carries_the_value() {
    let record = record_for(OsStr::new("bogus-value"));
    assert!(record.contains("bogus-value"), "{record:?}");
}

/// SF7: 200 two-byte characters. The value in the record is cut to 64
/// characters, never by bytes, so this cannot panic on a char boundary.
#[test]
fn a_long_multibyte_value_is_cut_to_64_characters_without_panicking() {
    let value = "é".repeat(200);
    let record = record_for(OsStr::new(&value));
    assert_one_clean_line(&record);
    let longest_run = record
        .split(|c| c != 'é')
        .map(|run| run.chars().count())
        .max()
        .unwrap_or(0);
    assert!(
        (1..=64).contains(&longest_run),
        "the value is cut to at most 64 characters, got {longest_run}: {record:?}"
    );
}

/// SF7: a four-byte character that starts at character 64 is kept whole, and
/// nothing after it appears.
#[test]
fn a_four_byte_character_at_the_cut_is_kept_whole() {
    let value = format!("{}\u{1F600}{}", "a".repeat(63), "\u{2603}".repeat(10));
    let record = record_for(OsStr::new(&value));
    assert_one_clean_line(&record);
    assert!(
        record.contains('\u{1F600}'),
        "character 64 is kept: {record:?}"
    );
    assert!(
        !record.contains('\u{2603}'),
        "characters after the 64th are cut: {record:?}"
    );
}

/// SF7: a value that is not valid UTF-8 is unknown, even when it also holds
/// the bytes of `none`.
#[cfg(unix)]
#[test]
fn a_value_that_is_not_utf8_is_full_with_a_record() {
    use std::os::unix::ffi::OsStrExt;
    for bytes in [
        &b"\xff\xfe"[..],
        &b"\xffnone"[..],
        &b"no\xffne"[..],
        &b"none\x80"[..],
        &b"\xc3"[..],
    ] {
        let record = record_for(OsStr::from_bytes(bytes));
        assert_one_clean_line(&record);
    }
}

/// SF7: a newline or an escape sequence in the value cannot split the record
/// or reach the terminal raw.
#[test]
fn control_characters_in_the_value_are_escaped() {
    for value in [
        "bad\nvalue",
        "bad\x1b[31mred",
        "a\rb",
        "\x07bell",
        "x\u{0085}y",
        "x\u{2028}y",
    ] {
        let record = record_for(OsStr::new(value));
        assert_one_clean_line(&record);
        assert!(
            !record.contains('\u{2028}') && !record.contains('\u{0085}'),
            "line separators are escaped too: {record:?}"
        );
    }
}

#[test]
fn parse_is_total_over_awkward_values() {
    // No value may panic. These are the shapes most likely to reach slicing
    // or indexing code.
    let long_ascii = "x".repeat(10_000);
    let long_wide = "\u{1F600}".repeat(5_000);
    for value in [
        long_ascii.as_str(),
        long_wide.as_str(),
        "\0",
        "none\0",
        "\u{10FFFF}",
    ] {
        let parsed = parse_str(value);
        assert_eq!(parsed.setting, HookText::Full, "{value:?}");
        assert_one_clean_line(parsed.record.as_deref().expect("recorded"));
    }
}

// ---- the decision for each governed category ----

#[test]
fn full_passes_every_category() {
    for text in GOVERNED {
        assert_eq!(action(HookText::Full, text), Action::Pass, "{text:?}");
    }
}

#[test]
fn none_strips_the_three_context_categories() {
    for text in [
        ModelText::SessionContext,
        ModelText::PromptContext,
        ModelText::ClassificationReminder,
    ] {
        assert_eq!(
            action(HookText::None, text),
            Action::StripContext,
            "{text:?}"
        );
    }
}

#[test]
fn none_approves_stop() {
    assert_eq!(
        action(HookText::None, ModelText::StopDecision),
        Action::Approve
    );
}

/// SEC-H3: approval is reachable only from the stop category.
#[test]
fn approve_is_reachable_only_from_stop_decision() {
    for setting in SETTINGS {
        for text in GOVERNED {
            if action(setting, text) == Action::Approve {
                assert_eq!(
                    (setting, text),
                    (HookText::None, ModelText::StopDecision),
                    "approve from {setting:?} / {text:?}"
                );
            }
        }
    }
}

// ---- stripping ----

fn strip(value: Value) -> Value {
    strip_model_text(value)
}

#[test]
fn session_start_keeps_its_diagnostics_and_loses_its_text() {
    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "indexing_status": "complete",
            "additionalContext": "project context"
        },
        "warnings": ["code-graph setup failed"],
        "systemMessage": "no capacity"
    });
    assert_eq!(
        strip(output),
        json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "indexing_status": "complete"
            },
            "warnings": ["code-graph setup failed"]
        })
    );
}

#[test]
fn user_prompt_output_becomes_empty() {
    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": "## User Preferences"
        },
        "systemMessage": "no capacity"
    });
    assert_eq!(strip(output), json!({}));
}

#[test]
fn classification_output_with_both_context_keys_becomes_empty() {
    let output = json!({
        "additionalContext": "routing reminder",
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": "routing reminder"
        }
    });
    assert_eq!(strip(output), json!({}));
}

#[test]
fn hook_specific_output_that_is_empty_or_not_an_object_is_dropped() {
    for hook_specific in [
        json!({}),
        json!("text"),
        json!(null),
        json!([1, 2]),
        json!(7),
    ] {
        let output = json!({"hookSpecificOutput": hook_specific, "additionalContext": "x"});
        assert_eq!(strip(output), json!({}), "{hook_specific}");
    }
}

#[test]
fn hook_specific_output_with_only_an_event_name_is_dropped() {
    let output = json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit"}});
    assert_eq!(strip(output), json!({}));
}

#[test]
fn a_non_object_output_becomes_an_empty_object() {
    for output in [
        json!(null),
        json!("text"),
        json!([{"additionalContext": "x"}]),
        json!(true),
    ] {
        assert_eq!(strip(output.clone()), json!({}), "{output}");
    }
}

#[test]
fn only_the_three_text_fields_are_removed() {
    let output = json!({
        "version": 1,
        "additionalContext": "x",
        "systemMessage": "y",
        "suppressOutput": true,
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": "z",
            "indexing_status": "started",
            "custom": {"additionalContext": "nested is not a hook field"}
        }
    });
    assert_eq!(
        strip(output),
        json!({
            "version": 1,
            "suppressOutput": true,
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "indexing_status": "started",
                "custom": {"additionalContext": "nested is not a hook field"}
            }
        })
    );
}

/// SEC-H2: a control decision always outranks text suppression.
#[test]
fn control_fields_survive_stripping() {
    let output = json!({
        "decision": "block",
        "reason": "lock mode is active",
        "continue": false,
        "stopReason": "stopped",
        "additionalContext": "c",
        "systemMessage": "m",
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": "destructive command",
            "additionalContext": "c"
        }
    });
    assert_eq!(
        strip(output),
        json!({
            "decision": "block",
            "reason": "lock mode is active",
            "continue": false,
            "stopReason": "stopped",
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "destructive command"
            }
        })
    );
}

#[test]
fn stripping_is_idempotent() {
    for output in [
        json!({}),
        json!(null),
        json!({"additionalContext": "x"}),
        json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "indexing_status": "complete", "additionalContext": "x"}, "warnings": ["w"]}),
        json!({"decision": "block", "reason": "r", "systemMessage": "m"}),
        json!({"hookSpecificOutput": "text"}),
    ] {
        let once = strip(output.clone());
        assert_eq!(strip(once.clone()), once, "{output}");
    }
}

// ---- the stop approval ----

#[test]
fn approve_is_the_value_the_stop_hook_gives() {
    let approve: Value =
        serde_json::from_slice(AsRef::<[u8]>::as_ref(APPROVE)).expect("APPROVE is one JSON value");
    let from_hook = crate::stop::StopHook
        .process(HookInput::Unknown)
        .expect("the stop hook approves an unknown event");
    assert_eq!(approve, from_hook);
    assert_eq!(approve, json!({"decision": "approve"}));
}

// ---- which hooks are governed (SEC-H1) ----

#[test]
fn the_four_governed_hooks_declare_their_category() {
    assert_eq!(
        crate::session_start::SessionStartHook.model_text(),
        Some(ModelText::SessionContext)
    );
    assert_eq!(
        crate::user_prompt::UserPromptSubmitHook::with_prompt_root(|| None).model_text(),
        Some(ModelText::PromptContext)
    );
    assert_eq!(
        crate::workflow_classification::WorkflowClassificationReminderHook.model_text(),
        Some(ModelText::ClassificationReminder)
    );
    assert_eq!(
        crate::stop::StopHook.model_text(),
        Some(ModelText::StopDecision)
    );
}

#[test]
fn ungoverned_hooks_declare_no_category() {
    assert_eq!(crate::pre_tool_use::PreToolUseHook.model_text(), None);
    assert_eq!(crate::post_tool_use::PostToolUseHook.model_text(), None);
    assert_eq!(crate::pre_compact::PreCompactHook.model_text(), None);
    assert_eq!(crate::session_stop::SessionStopHook.model_text(), None);
}

/// A hook written without thinking about the setting is ungoverned.
#[test]
fn the_trait_default_is_ungoverned() {
    struct Plain;
    impl Hook for Plain {
        fn process(&self, _: HookInput) -> anyhow::Result<Value> {
            Ok(json!({}))
        }
        fn name(&self) -> &'static str {
            "plain"
        }
    }
    assert_eq!(Plain.model_text(), None);
}

// ---- isolation (AC19) ----

/// The unit tests above must not depend on the variable the test process
/// inherited: a local model running `cargo test` inside a local session has
/// `AMPLIHACK_HOOK_TEXT=none` exported. `parse` takes its input as an
/// argument, so only `from_env` reads the process, and it is checked here
/// under the crate's environment lock.
#[test]
fn from_env_reads_the_variable_once_per_call() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    {
        let _unset = crate::test_support::EnvVarGuard::unset("AMPLIHACK_HOOK_TEXT");
        assert_eq!(from_env(), parse(None));
    }
    {
        let _none = crate::test_support::EnvVarGuard::set("AMPLIHACK_HOOK_TEXT", "none");
        assert_eq!(from_env().setting, HookText::None);
    }
    {
        let _bogus = crate::test_support::EnvVarGuard::set("AMPLIHACK_HOOK_TEXT", "bogus-value");
        let parsed = from_env();
        assert_eq!(parsed.setting, HookText::Full);
        assert!(parsed.record.is_some());
    }
}
