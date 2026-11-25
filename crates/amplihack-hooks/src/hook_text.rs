//! `AMPLIHACK_HOOK_TEXT`: whether the governed hooks add text to the model's
//! context.
//!
//! A hook is governed when it declares a [`ModelText`] category through
//! [`crate::protocol::Hook::model_text`]. Only governed hooks read the
//! variable, once per process, in `run_hook` before stdin is read. Every
//! other hook (pre-tool-use, post-tool-use, pre-compact, session-stop,
//! precommit-prefs) and limit-resume, which runs outside `run_hook`, never
//! read it.
//!
//! Only exact `none` removes the model text. Unset keeps existing behaviour
//! silently. Every other explicit value preserves behaviour and produces one
//! escaped diagnostic on stderr, including empty and non-UTF-8 values.
//!
//! [`action`] explicitly selects the behaviour for every participating
//! category. Text removal happens before host protocol conversion.

use serde_json::Value;
use std::ffi::OsStr;

use amplihack_types::AMPLIHACK_HOOK_TEXT as VAR;

/// What the stop hook prints when it approves without running. Equal to the
/// value `StopHook` itself gives for an event it does not act on.
pub(crate) const APPROVE: &str = r#"{"decision":"approve"}"#;

/// How much of an unknown value the record quotes, in characters.
const RECORD_CHARS: usize = 64;

/// The kind of text a governed hook gives the model, or the decision it
/// makes about the end of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelText {
    /// session-start's context.
    SessionContext,
    /// user-prompt-submit's context (preferences and similar).
    PromptContext,
    /// workflow-classification-reminder's routing reminder.
    ClassificationReminder,
    /// stop's decision to block the end of a turn, and its text.
    StopDecision,
}

/// The parsed setting. Always written `HookText::None`, never glob-imported,
/// so that it cannot be confused with `Option::None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookText {
    Default,
    None,
}

/// What `run_hook` does for one governed hook under one setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Print the hook's output as it is.
    Pass,
    /// Run the hook, then remove its model text with [`strip_model_text`].
    StripContext,
    /// Print [`APPROVE`] without running the hook.
    Approve,
}

/// The setting and, for an unknown value, the one line to write to stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parsed {
    pub(crate) setting: HookText,
    pub(crate) record: Option<String>,
}

/// Parses a value of [`VAR`]. Total: no value can panic.
pub(crate) fn parse(value: Option<&OsStr>) -> Parsed {
    let Some(raw) = value else {
        return Parsed {
            setting: HookText::Default,
            record: None,
        };
    };
    if raw == OsStr::new("none") {
        return Parsed {
            setting: HookText::None,
            record: None,
        };
    }
    Parsed {
        setting: HookText::Default,
        record: Some(unknown_value_record(raw)),
    }
}

/// [`parse`] of this process's value of [`VAR`].
pub(crate) fn from_env() -> Parsed {
    parse(std::env::var_os(VAR).as_deref())
}

/// One line naming the variable and the value. The value is decoded lossily,
/// cut to [`RECORD_CHARS`] characters (never bytes) and Debug-escaped, so a
/// newline or an escape sequence in it can neither split the line nor reach
/// the terminal raw.
fn unknown_value_record(raw: &OsStr) -> String {
    let lossy = raw.to_string_lossy();
    let cut: String = lossy.chars().take(RECORD_CHARS).collect();
    format!(
        "amplihack-hooks: unknown {VAR}={cut:?}; only none is supported; keeping default behaviour"
    )
}

/// The decision for one category under one setting.
pub(crate) fn action(setting: HookText, text: ModelText) -> Action {
    match (setting, text) {
        (
            HookText::Default,
            ModelText::SessionContext
            | ModelText::PromptContext
            | ModelText::ClassificationReminder
            | ModelText::StopDecision,
        ) => Action::Pass,
        (
            HookText::None,
            ModelText::SessionContext
            | ModelText::PromptContext
            | ModelText::ClassificationReminder,
        ) => Action::StripContext,
        (HookText::None, ModelText::StopDecision) => Action::Approve,
    }
}

/// Removes the model text from a hook's output: top-level
/// `additionalContext` and `systemMessage`, and
/// `hookSpecificOutput.additionalContext`. `hookSpecificOutput` itself goes
/// when left empty or holding only `hookEventName`. Other output is preserved.
///
/// Control fields (`decision`, `reason`, `continue`, `stopReason`,
/// `permissionDecision` and the rest) are never touched, so a control
/// decision always outranks text suppression. Idempotent.
pub(crate) fn strip_model_text(output: Value) -> Value {
    let Value::Object(mut map) = output else {
        return output;
    };
    map.remove("additionalContext");
    map.remove("systemMessage");
    let drop_specific = match map.get_mut("hookSpecificOutput") {
        None => false,
        Some(Value::Object(specific)) => {
            specific.remove("additionalContext");
            specific.keys().all(|key| key == "hookEventName")
        }
        Some(_) => false,
    };
    if drop_specific {
        map.remove("hookSpecificOutput");
    }
    Value::Object(map)
}

#[cfg(test)]
#[path = "hook_text_tests.rs"]
mod tests;
