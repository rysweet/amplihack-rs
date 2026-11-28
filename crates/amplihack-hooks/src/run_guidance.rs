//! Operator guidance for an agent step that is already running (issue
//! #3435).
//!
//! Inside a recipe run the launcher names each agent step in
//! `AMPLIHACK_GUIDANCE_STEP_ID`. At every PostToolUse and UserPromptSubmit
//! hook of such a step, [`new_guidance`] runs `amplihack recipe guidance
//! deliver`, which prints the entries the step has not been given yet and
//! records them, and [`merge_context`] adds them to the hook's added
//! context. The reader, renderer and record live in `amplihack`, so there is
//! one of each; this crate does not depend on `amplihack-cli`.
//!
//! Outside a recipe run (either variable unset or empty) no process is
//! started. A failure never blocks the tool call or the prompt and never
//! changes the hook's exit status: `deliver` records it for the sender where
//! it can, and this side writes one line on stderr.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The variable that names the running agent step.
const STEP_ID_ENV: &str = "AMPLIHACK_GUIDANCE_STEP_ID";
/// The guidance folders of the recipe run.
const DIRS_ENV: &str = "AMPLIHACK_RUN_GUIDANCE_DIRS";
/// The override for the `amplihack` binary, as `BinaryFinder` reads it.
const BINARY_OVERRIDE_ENV: &str = "AMPLIHACK_AMPLIHACK_BINARY_PATH";

fn set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

/// Whether this hook runs inside a named recipe step.
fn in_named_step() -> bool {
    set(STEP_ID_ENV) && set(DIRS_ENV)
}

/// The `amplihack` binary: the override when set, else the one beside this
/// hooks binary (same release), else the one `BinaryFinder` finds.
fn amplihack_binary() -> anyhow::Result<PathBuf> {
    if !set(BINARY_OVERRIDE_ENV)
        && let Ok(exe) = std::env::current_exe()
    {
        let sibling = exe.with_file_name("amplihack");
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    Ok(amplihack_utils::binary_finder::BinaryFinder::find("amplihack")?.path)
}

/// The block of guidance this step has not been given yet, for an `event`
/// that can carry added context; `None` when there is none, outside a
/// recipe step, and on failure.
pub(crate) fn new_guidance(event: &str) -> Option<String> {
    if !matches!(event, "PostToolUse" | "UserPromptSubmit") || !in_named_step() {
        return None;
    }
    let binary = match amplihack_binary() {
        Ok(binary) => binary,
        Err(error) => {
            eprintln!("amplihack-hooks: operator guidance not checked: {error}");
            return None;
        }
    };
    let output = Command::new(&binary)
        .args(["recipe", "guidance", "deliver"])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let text = text.trim_end();
            (!text.trim().is_empty()).then(|| text.to_string())
        }
        Ok(output) => {
            eprintln!(
                "amplihack-hooks: operator guidance not delivered: `amplihack recipe guidance deliver` ended with {}",
                output.status
            );
            None
        }
        Err(error) => {
            eprintln!(
                "amplihack-hooks: operator guidance not delivered: cannot run {}: {error}",
                binary.display()
            );
            None
        }
    }
}

/// `output` with `block` added to `hookSpecificOutput.additionalContext`,
/// after any context already there and a blank line.
pub(crate) fn merge_context(event: &str, output: Value, block: &str) -> Value {
    let mut map = match output {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    let specific = map
        .entry("hookSpecificOutput")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !specific.is_object() {
        *specific = Value::Object(serde_json::Map::new());
    }
    let specific = specific
        .as_object_mut()
        .expect("hookSpecificOutput was just made an object");
    let context = match specific
        .get("additionalContext")
        .and_then(Value::as_str)
        .filter(|c| !c.trim().is_empty())
    {
        Some(existing) => format!("{existing}\n\n{block}"),
        None => block.to_string(),
    };
    specific.insert("hookEventName".to_string(), Value::String(event.into()));
    specific.insert("additionalContext".to_string(), Value::String(context));
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_block_follows_context_already_there() {
        let out = merge_context(
            "UserPromptSubmit",
            json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": "prefs"}, "systemMessage": "m"}),
            "BLOCK",
        );
        assert_eq!(
            out["hookSpecificOutput"]["additionalContext"],
            "prefs\n\nBLOCK"
        );
        assert_eq!(out["systemMessage"], "m");
    }

    #[test]
    fn the_block_is_the_whole_context_when_there_was_none() {
        let out = merge_context("PostToolUse", json!({"warnings": ["w"]}), "BLOCK");
        assert_eq!(out["hookSpecificOutput"]["additionalContext"], "BLOCK");
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert_eq!(out["warnings"], json!(["w"]));
        let out = merge_context("PostToolUse", json!(null), "BLOCK");
        assert_eq!(out["hookSpecificOutput"]["additionalContext"], "BLOCK");
    }

    #[test]
    fn other_events_and_unnamed_steps_start_nothing() {
        let _lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _step = crate::test_support::EnvVarGuard::set(STEP_ID_ENV, "run-1-abc");
        let _dirs = crate::test_support::EnvVarGuard::unset(DIRS_ENV);
        let _bin = crate::test_support::EnvVarGuard::set(
            BINARY_OVERRIDE_ENV,
            "/nonexistent/amplihack-for-this-test",
        );
        assert_eq!(new_guidance("PostToolUse"), None);
        assert_eq!(new_guidance("PreToolUse"), None);
    }
}
