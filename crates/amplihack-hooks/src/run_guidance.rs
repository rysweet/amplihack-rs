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
//! changes the hook's exit status. It is always recorded for the sender, as
//! a `hook_delivery_failed` drop in `guidance.log`, which `recipe guide`
//! prints after an append: by `deliver` when it ran and failed (it then
//! exits 1), and by this side ([`record_failure`]) when `deliver` could not
//! be found or started, or ended any other way. This side also writes one
//! line on stderr.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The variable that names the running agent step.
const STEP_ID_ENV: &str = "AMPLIHACK_GUIDANCE_STEP_ID";
/// The guidance folders of the recipe run.
const DIRS_ENV: &str = "AMPLIHACK_RUN_GUIDANCE_DIRS";
/// The override for the `amplihack` binary, as `BinaryFinder` reads it.
const BINARY_OVERRIDE_ENV: &str = "AMPLIHACK_AMPLIHACK_BINARY_PATH";
/// The exit status with which `deliver` says it recorded its own failure.
const DELIVER_RECORDED_FAILURE: i32 = 1;
/// The most guidance folders read from [`DIRS_ENV`], as `amplihack` reads.
const MAX_DIRS: usize = 16;

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
            record_failure();
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
            if output.status.code() != Some(DELIVER_RECORDED_FAILURE) {
                record_failure();
            }
            None
        }
        Err(error) => {
            eprintln!(
                "amplihack-hooks: operator guidance not delivered: cannot run {}: {error}",
                binary.display()
            );
            record_failure();
            None
        }
    }
}

/// `value` when it is a valid run id (`[A-Za-z0-9._-]`, not starting with
/// `.`, at most 200 bytes), `unknown` otherwise; as `amplihack` writes it.
fn valid_run_id(value: &str) -> &str {
    let valid = !value.is_empty()
        && value.len() <= 200
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if valid { value } else { "unknown" }
}

#[cfg(unix)]
/// Whether `meta` (taken without following links) passes `amplihack`'s
/// trust rule: of the wanted kind, owned by this user, not writable by
/// group or others.
fn trusted(meta: &std::fs::Metadata, want_dir: bool) -> bool {
    use std::os::unix::fs::MetadataExt;
    let kind = meta.file_type();
    let kind_ok = if want_dir {
        kind.is_dir()
    } else {
        kind.is_file()
    };
    // SAFETY: geteuid has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    kind_ok && !kind.is_symlink() && meta.uid() == me && meta.mode() & 0o022 == 0
}

/// The `hook_delivery_failed` record this side writes. Every value is from
/// a closed set or a checked run id; none is guidance, argv or error text.
fn failure_record() -> Value {
    let tool = match std::env::var("AMPLIHACK_AGENT_BINARY").as_deref() {
        Ok(tool @ ("claude" | "codex" | "copilot")) => tool.to_string(),
        _ => "other".to_string(),
    };
    let run_id = std::env::var("AMPLIHACK_RECIPE_RUN_ID").unwrap_or_default();
    serde_json::json!({
        "event": "guidance_dropped",
        "ts": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.9fZ").to_string(),
        "step_run_id": valid_run_id(&run_id),
        "tool": tool,
        "launch_form": "hook of a running step",
        "reason": "hook_delivery_failed",
        "detail": "the hook could not run `amplihack recipe guidance deliver`, so the running step cannot be given new guidance",
    })
}

/// Appends `line` to `guidance.log` beside the guidance folder `dir`, as
/// `amplihack` does: the folder's parent must pass the trust rule, the log
/// is created 0600, never opened through a link or as a FIFO, and must pass
/// the trust rule once open. One `write` of one short line in append mode.
#[cfg(unix)]
fn append_beside(dir: &Path, line: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let bad = |what: &str| std::io::Error::other(what.to_string());
    if !dir.is_absolute() || dir.file_name() != Some(std::ffi::OsStr::new("guidance")) {
        return Err(bad("not an absolute path to a guidance folder"));
    }
    let parent = dir.parent().ok_or_else(|| bad("no parent folder"))?;
    if !trusted(&std::fs::symlink_metadata(parent)?, true) {
        return Err(bad("its folder fails the trust rule"));
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(parent.join("guidance.log"))?;
    if !trusted(&file.metadata()?, false) {
        return Err(bad("guidance.log fails the trust rule"));
    }
    file.write_all(line)
}

/// Guidance logs and their trust rule exist on Unix only.
#[cfg(not(unix))]
fn append_beside(_dir: &Path, _line: &[u8]) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// Records a `hook_delivery_failed` drop in the log beside every listed
/// guidance folder whose parent exists, so that `recipe guide` tells the
/// sender that this running step's hook could not deliver. A failed write
/// is one line on stderr.
pub(crate) fn record_failure() {
    let Some(value) = std::env::var_os(DIRS_ENV).filter(|v| !v.is_empty()) else {
        return;
    };
    let mut line = failure_record().to_string();
    line.push('\n');
    let mut seen: Vec<PathBuf> = Vec::new();
    for dir in std::env::split_paths(&value).take(MAX_DIRS) {
        if dir.as_os_str().is_empty() || seen.contains(&dir) {
            continue;
        }
        if let Err(error) = append_beside(&dir, line.as_bytes()) {
            eprintln!(
                "amplihack-hooks: the failure to deliver operator guidance was not logged beside {}: {error}",
                dir.display()
            );
        }
        seen.push(dir);
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

    /// The `guidance.log` records of a run folder whose step's hook ran an
    /// `amplihack` that exits with `status` from `deliver`.
    #[cfg(unix)]
    fn records_after_deliver_exits(status: i32, folder_mode: u32) -> Vec<Value> {
        use std::os::unix::fs::PermissionsExt;
        let _lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        let run = tmp.path().join("run");
        std::fs::create_dir(&run).unwrap();
        let fake = tmp.path().join("amplihack");
        std::fs::write(&fake, format!("#!/bin/sh\nexit {status}\n")).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(folder_mode)).unwrap();
        let guidance = run.join("guidance");
        let _step = crate::test_support::EnvVarGuard::set(STEP_ID_ENV, "run-1-abc");
        let _dirs = crate::test_support::EnvVarGuard::set(DIRS_ENV, guidance.to_str().unwrap());
        let _bin =
            crate::test_support::EnvVarGuard::set(BINARY_OVERRIDE_ENV, fake.to_str().unwrap());
        let _run_id = crate::test_support::EnvVarGuard::set("AMPLIHACK_RECIPE_RUN_ID", "run-1");
        let _tool = crate::test_support::EnvVarGuard::set("AMPLIHACK_AGENT_BINARY", "claude");
        assert_eq!(new_guidance("PostToolUse"), None);
        std::fs::read_to_string(run.join("guidance.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    #[cfg(unix)]
    fn a_deliver_that_ends_without_recording_is_recorded_here() {
        let records = records_after_deliver_exits(3, 0o700);
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0]["event"], "guidance_dropped");
        assert_eq!(records[0]["reason"], "hook_delivery_failed");
        assert_eq!(records[0]["step_run_id"], "run-1");
        assert_eq!(records[0]["tool"], "claude");
    }

    #[test]
    #[cfg(unix)]
    fn a_deliver_that_recorded_its_own_failure_is_not_recorded_twice() {
        assert!(records_after_deliver_exits(1, 0o700).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn no_record_is_written_beside_a_folder_others_can_write() {
        assert!(records_after_deliver_exits(3, 0o777).is_empty());
    }

    #[test]
    fn a_run_id_that_is_not_valid_is_written_as_unknown() {
        assert_eq!(valid_run_id("run-1.a_b"), "run-1.a_b");
        assert_eq!(valid_run_id(".hidden"), "unknown");
        assert_eq!(valid_run_id("a/b"), "unknown");
        assert_eq!(valid_run_id(""), "unknown");
    }
}
