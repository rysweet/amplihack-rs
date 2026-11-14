//! The terminating exit status of a failed step, read from the pinned runner's
//! error envelope (issue #1437).
//!
//! `structural_refusal` rests on this decoder alone, so it reads only text the
//! runner composed itself, anchored where the runner put it. Captured output
//! can quote any status; it never reaches the decoder.
//!
//! # The envelopes recipe-runner-rs `90286bc` writes
//!
//! The pin is in `claude-plugin/recipe-runner.rev` and `Cargo.toml`.
//!
//! - A bash or agent step (`StepExecutionError`, `models.rs`):
//!   `Step '<id>' failed: bash step failed: Command failed (exit N): <stderr>`
//!   or `Step '<id>' failed: agent step failed: amplihack <cli> failed (exit N)`
//!   followed by diagnostics (`adapters/cli_subprocess.rs`). `<id>` goes
//!   through `diagnostic_identifier`, which is `str::escape_debug`; `4d5187e`,
//!   the pin before, wrote the raw id. For an ordinary id such as
//!   `step-04-run` the two are the same, so both are accepted.
//! - An in-process `type: recipe` step (`runner/mod.rs`, the
//!   `StepType::Recipe` branch of step execution, `execute_sub_recipe` and
//!   `describe_sub_recipe_failure`):
//!
//!   ```text
//!   Step "<id>" failed: Sub-recipe '<name>' failed.
//!   Failed steps:
//!   - <inner id> [error]: <the inner step's own error, trimmed>
//!   - <inner id> [error]: ...
//!   Completed steps: ...
//!   ```
//!
//!   The outer id is in Rust `Debug` form (double quotes). When the step has
//!   `recovery_on_failure` and recovery fails, the header reads `failed and
//!   agentic recovery was unsuccessful.` instead of `failed.`.
//!
//! # Only the first failed entry counts
//!
//! The first entry starts on the line right after `Failed steps:`, so its
//! position is fixed by the runner. Its detail runs on to later lines, and
//! for a bash step those lines are the step's stderr: a later line that looks
//! like `- x [error]: Step 'x' failed: … (exit 79)` can be anything the step
//! printed. So no later entry is read. An entry whose detail came from
//! `[output tail]` is captured output and is not read either. The inner error
//! is decoded with the same rules, so `auto-drive-light` → `auto-drive-to-merge`
//! → `autodrive-merge-loop` → a bash step's `exit 79` decodes as `79`.
//!
//! A sub-recipe whose first failed step was non-fatal (`continue_on_error` or
//! `fatal: false`) and whose exit 79 came from a later step decodes to the
//! first step's status, not `79`. That is a known miss (see "Sub-recipe
//! steps" in `docs/reference/transient-failure-retry.md`, and issue #2490).
//! The guard's own phrase in a failed step's evidence still vetoes a
//! mechanical retry (`failure_class::classify_run_failure`).
//!
//! Anything else decodes to `None`, which is never a refusal. Keep this
//! compatibility decoder until the runner exposes a numeric exit-code field.

use super::super::{RecipeRunResult, RecipeRunStepResult};
use crate::commands::orch::STEP_RECOVERY_HANDOFF_EXIT;

/// The exit status of a failed run: 81 when a failed step ended with a
/// recorded step-recovery handoff (issue #2539), so the handoff reaches the
/// caller instead of a plain 1; otherwise 1.
pub(crate) fn failed_run_exit_code(result: &RecipeRunResult) -> i32 {
    let handed_off = result.step_results.iter().any(|step| {
        !super::failure_evidence::is_completed_status(&step.status)
            && step.termination_exit_code() == Some(STEP_RECOVERY_HANDOFF_EXIT)
    });
    if handed_off {
        STEP_RECOVERY_HANDOFF_EXIT
    } else {
        1
    }
}

impl RecipeRunStepResult {
    /// Decode the terminating process status from the runner's error envelope.
    ///
    /// See the module documentation for the shapes decoded and why only those.
    pub(crate) fn termination_exit_code(&self) -> Option<i32> {
        terminating_exit_code(&self.step_id, &self.error)
    }
}

/// The exit status recorded at the start of `error`, for the step `step_id`.
///
/// Iterative, not recursive: each sub-recipe level consumes its header, so the
/// loop ends, and no input can exhaust the stack.
fn terminating_exit_code(step_id: &str, error: &str) -> Option<i32> {
    let mut id = step_id.to_string();
    let mut text = error;
    while let Some((inner_id, inner_error)) = first_sub_recipe_failure(&id, text) {
        id = inner_id;
        text = inner_error;
    }
    leaf_exit_code(&id, text)
}

/// For a failed `type: recipe` step, the escaped-then-decoded id and the error
/// of the FIRST failed step inside it. `None` when `error` is not that shape.
fn first_sub_recipe_failure<'a>(step_id: &str, error: &'a str) -> Option<(String, &'a str)> {
    let rest = error.strip_prefix(&format!("Step {step_id:?} failed: Sub-recipe '"))?;
    let name_len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    if name_len == 0 {
        return None;
    }
    let rest = rest[name_len..].strip_prefix("' failed.\n").or_else(|| {
        rest[name_len..].strip_prefix("' failed and agentic recovery was unsuccessful.\n")
    })?;
    let entry = rest.strip_prefix("Failed steps:\n- ")?;
    // The id is escaped, so it holds no newline: the separator has to be on
    // the entry's own first line. Found later, it would be the step's output.
    let first_line = entry.split('\n').next().unwrap_or_default();
    let at = first_line.find(" [error]: ")?;
    let inner_id = unescape_debug(&first_line[..at])?;
    Some((inner_id, &entry[at + " [error]: ".len()..]))
}

/// A bash or agent step's status, enveloped or bare.
fn leaf_exit_code(step_id: &str, error: &str) -> Option<i32> {
    let escaped: String = step_id.escape_debug().collect();
    let enveloped = error
        .strip_prefix(&format!("Step '{escaped}' failed: "))
        .or_else(|| error.strip_prefix(&format!("Step '{step_id}' failed: ")));
    let error = if let Some(message) = enveloped {
        message
            .strip_prefix("bash step failed: ")
            .or_else(|| message.strip_prefix("agent step failed: "))?
    } else {
        error
    };
    if let Some(rest) = error.strip_prefix("Command failed (exit ") {
        // `Command failed (exit N): <stderr>`. A sub-recipe entry is trimmed,
        // so with empty stderr the header ends at `):`.
        let (code, after) = rest.split_once(')')?;
        let after = after.strip_prefix(':')?;
        if !(after.is_empty() || after.starts_with([' ', '\n'])) {
            return None;
        }
        return code.parse().ok();
    }
    let header = error.lines().next()?;
    let rest = header.strip_prefix("amplihack ")?;
    let (cli, rest) = rest.split_once(" failed (exit ")?;
    if cli.is_empty() || cli.chars().any(char::is_whitespace) {
        return None;
    }
    rest.strip_suffix(')')?.parse().ok()
}

/// Invert `str::escape_debug`, the runner's `diagnostic_identifier`.
///
/// Returns `None` unless re-escaping the result gives back exactly
/// `escaped`, so a malformed or non-canonical id never matches anything.
fn unescape_debug(escaped: &str) -> Option<String> {
    let mut raw = String::new();
    let mut chars = escaped.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            raw.push(c);
            continue;
        }
        let unescaped = match chars.next()? {
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '0' => '\0',
            'u' => {
                if chars.next()? != '{' {
                    return None;
                }
                let hex: String = chars.by_ref().take_while(|&h| h != '}').collect();
                char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
            }
            _ => return None,
        };
        raw.push(unescaped);
    }
    let canonical: String = raw.escape_debug().collect();
    (canonical == escaped).then_some(raw)
}

#[cfg(test)]
pub(super) fn unescape_debug_for_test(escaped: &str) -> Option<String> {
    unescape_debug(escaped)
}
