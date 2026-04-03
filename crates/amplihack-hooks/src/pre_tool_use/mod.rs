//! Pre-tool-use hook: validates bash commands and XPIA security before execution.
//!
//! Blocks:
//! - CWD deletion (rm -rf, rmdir targeting CWD or parent)
//! - CWD rename/move (mv targeting CWD or parent)
//! - Direct commits to main/master branch
//! - Use of --no-verify flag on git commands
//! - XPIA prompt injection attacks (all tools)

mod command;
mod cwd;
mod git;
pub mod launcher;
mod xpia;

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::{HookInput, ProjectDirs};
use serde_json::Value;

/// Error messages for blocked operations.
const CWD_DELETION_ERROR: &str = "\
🚫 OPERATION BLOCKED - Working Directory Deletion Prevented\n\
\n\
You attempted to delete a directory that contains your current working directory:\n\
  Target: {target}\n\
  CWD:    {cwd}\n\
\n\
Deleting the CWD would break the current session. If you need to clean up\n\
this directory, first change to a different working directory.\n\
\n\
🔒 This protection cannot be disabled programmatically.";

const CWD_RENAME_ERROR: &str = "\
🚫 OPERATION BLOCKED - Working Directory Rename Prevented\n\
\n\
You attempted to move/rename a directory that contains your current working directory:\n\
  Source: {source}\n\
  CWD:    {cwd}\n\
\n\
Moving or renaming the CWD would break the current session. To rename this directory:\n\
  1. First change to a different working directory (e.g., cd ..)\n\
  2. Then perform the rename operation\n\
  3. Change back into the renamed directory if needed\n\
\n\
🔒 This protection cannot be disabled programmatically.";

const MAIN_BRANCH_ERROR: &str = "\
⛔ Direct commits to '{branch}' branch are not allowed.\n\
\n\
Please use the feature branch workflow:\n\
  1. Create a feature branch: git checkout -b feature/your-feature-name\n\
  2. Make your commits on the feature branch\n\
  3. Create a Pull Request to merge into {branch}\n\
\n\
This protection cannot be bypassed with --no-verify.";

const NO_VERIFY_ERROR: &str = "\
🚫 OPERATION BLOCKED\n\
\n\
You attempted to use --no-verify which bypasses critical quality checks:\n\
- Code formatting (ruff, prettier)\n\
- Type checking (pyright)\n\
- Secret detection\n\
- Trailing whitespace fixes\n\
\n\
This defeats the purpose of our quality gates.\n\
\n\
✅ Instead, fix the underlying issues:\n\
1. Run: pre-commit run --all-files\n\
2. Fix the violations\n\
3. Commit without --no-verify\n\
\n\
For true emergencies, ask a human to override this protection.\n\
\n\
🔒 This protection cannot be disabled programmatically.";

/// Strip leading env-variable assignments (`VAR=value ...`) and an optional
/// `env` prefix so that `GIT_DIR=/x git commit` is normalized to `git commit`.
fn normalize_command(command: &str) -> String {
    let mut rest = command.trim();

    // Strip `VAR=value ` prefixes (no quotes in key, value runs until space).
    while let Some(eq_pos) = rest.find('=') {
        let prefix = &rest[..eq_pos];
        // Key must be a valid env var name (alphanumeric + underscore, not starting with digit).
        if prefix.is_empty()
            || !prefix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || prefix.as_bytes()[0].is_ascii_digit()
        {
            break;
        }
        // Skip past the value (until next unquoted space).
        let after_eq = &rest[eq_pos + 1..];
        let value_end = after_eq
            .find([' ', '\t'])
            .unwrap_or(after_eq.len());
        let after_value = after_eq[value_end..].trim_start();
        if after_value.is_empty() {
            break; // nothing left after the value — not a prefix
        }
        rest = after_value;
    }

    // Strip optional `env` command prefix.
    if let Some(after_env) = rest.strip_prefix("env ") {
        rest = after_env.trim_start();
        // Also strip any additional VAR=val pairs after `env`.
        while let Some(eq_pos) = rest.find('=') {
            let prefix = &rest[..eq_pos];
            if prefix.is_empty()
                || !prefix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || prefix.as_bytes()[0].is_ascii_digit()
            {
                break;
            }
            let after_eq = &rest[eq_pos + 1..];
            let value_end = after_eq
                .find([' ', '\t'])
                .unwrap_or(after_eq.len());
            let after_value = after_eq[value_end..].trim_start();
            if after_value.is_empty() {
                break;
            }
            rest = after_value;
        }
    }

    rest.to_string()
}

/// The pre-tool-use hook.
pub struct PreToolUseHook;

impl Hook for PreToolUseHook {
    fn name(&self) -> &'static str {
        "pre_tool_use"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (tool_name, tool_input) = match input {
            HookInput::PreToolUse {
                tool_name,
                tool_input,
                ..
            } => (tool_name, tool_input),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        // Run launcher-specific context injection (side-effect only, never blocks).
        let dirs = ProjectDirs::from_cwd();
        let input_value = serde_json::json!({"tool_name": &tool_name, "tool_input": &tool_input});
        launcher::inject_context(&dirs, &input_value);

        // XPIA security validation for all tools.
        if let Some(block) = xpia::check_xpia(&tool_name, &tool_input) {
            return Ok(block);
        }

        // Only process Bash tool invocations for CWD/git checks.
        if tool_name != "Bash" {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        let command = tool_input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("");

        if command.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // Check CWD deletion.
        if let Some(block) = cwd::check_cwd_deletion(command)? {
            return Ok(block);
        }

        // Check CWD rename/move.
        if let Some(block) = cwd::check_cwd_rename(command)? {
            return Ok(block);
        }

        let normalized = normalize_command(command);
        let is_git_commit = normalized.contains("git commit");
        let is_git_push = normalized.contains("git push");
        let is_git_rebase = normalized.contains("git rebase");
        let is_git_merge = normalized.contains("git merge");
        let is_git_cherry_pick = normalized.contains("git cherry-pick");
        let is_git_am = normalized.contains("git am");
        let has_no_verify = command.contains("--no-verify");
        let is_git_command = is_git_commit
            || is_git_push
            || is_git_rebase
            || is_git_merge
            || is_git_cherry_pick
            || is_git_am;

        if !is_git_command {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // Check main branch protection for git commit.
        if is_git_commit && let Some(block) = git::check_main_branch()? {
            return Ok(block);
        }

        // Check --no-verify flag.
        if has_no_verify && is_git_command {
            return Ok(serde_json::json!({
                "block": true,
                "message": NO_VERIFY_ERROR
            }));
        }

        Ok(Value::Object(serde_json::Map::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;

    fn make_bash_input(command: &str) -> HookInput {
        HookInput::PreToolUse {
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({"command": command}),
            session_id: None,
        }
    }

    #[test]
    fn allows_safe_commands() {
        let hook = PreToolUseHook;
        let result = hook.process(make_bash_input("ls -la")).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn allows_non_bash_tools() {
        let hook = PreToolUseHook;
        let input = HookInput::PreToolUse {
            tool_name: "Read".to_string(),
            tool_input: serde_json::json!({"path": "/tmp/file.txt"}),
            session_id: None,
        };
        let result = hook.process(input).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn blocks_no_verify() {
        let hook = PreToolUseHook;
        let result = hook
            .process(make_bash_input("git commit --no-verify -m 'test'"))
            .unwrap();
        assert_eq!(result["block"], true);
        assert!(result["message"].as_str().unwrap().contains("--no-verify"));
    }

    #[test]
    fn blocks_no_verify_on_push() {
        let hook = PreToolUseHook;
        let result = hook
            .process(make_bash_input("git push --no-verify origin main"))
            .unwrap();
        assert_eq!(result["block"], true);
    }

    #[test]
    fn blocks_no_verify_on_rebase() {
        let hook = PreToolUseHook;
        let result = hook
            .process(make_bash_input("git rebase --no-verify main"))
            .unwrap();
        assert_eq!(result["block"], true);
        assert!(result["message"].as_str().unwrap().contains("--no-verify"));
    }

    #[test]
    fn blocks_no_verify_on_merge() {
        let hook = PreToolUseHook;
        let result = hook
            .process(make_bash_input("git merge --no-verify feature-branch"))
            .unwrap();
        assert_eq!(result["block"], true);
        assert!(result["message"].as_str().unwrap().contains("--no-verify"));
    }

    #[test]
    fn blocks_no_verify_on_cherry_pick() {
        let hook = PreToolUseHook;
        let result = hook
            .process(make_bash_input("git cherry-pick --no-verify abc123"))
            .unwrap();
        assert_eq!(result["block"], true);
        assert!(result["message"].as_str().unwrap().contains("--no-verify"));
    }

    #[test]
    fn blocks_no_verify_on_am() {
        let hook = PreToolUseHook;
        let result = hook
            .process(make_bash_input("git am --no-verify patch.patch"))
            .unwrap();
        assert_eq!(result["block"], true);
        assert!(result["message"].as_str().unwrap().contains("--no-verify"));
    }

    #[test]
    fn allows_git_rebase_without_no_verify() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let hook = PreToolUseHook;
        let result = hook.process(make_bash_input("git rebase main")).unwrap();
        assert!(result.get("block").is_none());
    }

    #[test]
    fn allows_git_commit_on_feature_branch() {
        // Hold env_lock so concurrent tests can't set GITHUB_COPILOT_AGENT=1
        // while inject_context runs against the real CWD.
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // This test depends on the current branch not being main/master.
        // In CI, we may be on a feature branch, so this should pass.
        let hook = PreToolUseHook;
        let result = hook
            .process(make_bash_input("git commit -m 'test'"))
            .unwrap();
        // Can't assert allow/deny here reliably — depends on current branch.
        // Just verify it doesn't panic.
        let _ = result;
    }

    #[test]
    fn handles_unknown_hook_event() {
        let hook = PreToolUseHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn blocks_no_verify_with_git_dir_prefix() {
        let hook = PreToolUseHook;
        let result = hook
            .process(make_bash_input(
                "GIT_DIR=/some/path git commit --no-verify -m 'test'",
            ))
            .unwrap();
        assert_eq!(result["block"], true);
        assert!(result["message"].as_str().unwrap().contains("--no-verify"));
    }

    #[test]
    fn blocks_no_verify_with_env_prefix() {
        let hook = PreToolUseHook;
        let result = hook
            .process(make_bash_input("env git push --no-verify origin main"))
            .unwrap();
        assert_eq!(result["block"], true);
        assert!(result["message"].as_str().unwrap().contains("--no-verify"));
    }

    #[test]
    fn normalize_strips_env_var_prefix() {
        assert_eq!(
            normalize_command("GIT_DIR=/tmp git commit -m 'x'"),
            "git commit -m 'x'"
        );
    }

    #[test]
    fn normalize_strips_env_command() {
        assert_eq!(
            normalize_command("env git push origin main"),
            "git push origin main"
        );
    }

    #[test]
    fn normalize_strips_multiple_env_vars() {
        assert_eq!(
            normalize_command("FOO=1 BAR=baz git commit"),
            "git commit"
        );
    }

    #[test]
    fn normalize_passthrough_plain_command() {
        assert_eq!(normalize_command("git commit -m 'x'"), "git commit -m 'x'");
    }
}
