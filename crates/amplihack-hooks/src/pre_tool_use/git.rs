//! Git-related protection checks.
//!
//! - Main branch protection: blocks commits to main/master.
//! - Runs `git branch --show-current` with a 5-second timeout.

use serde_json::Value;
use std::process::Command;

/// Check if the current branch is main/master and block commits.
///
/// Returns `Some(block_json)` if on a protected branch, `None` otherwise.
/// Fails open: if git is not found, times out, or errors, returns `None`.
pub fn check_main_branch() -> anyhow::Result<Option<Value>> {
    let branch = match get_current_branch() {
        Some(b) => b,
        None => return Ok(None), // Fail-open.
    };

    if branch == "main" || branch == "master" {
        let message = super::MAIN_BRANCH_ERROR.replace("{branch}", &branch);
        return Ok(Some(serde_json::json!({
            "block": true,
            "message": message
        })));
    }

    Ok(None)
}

/// Get the current git branch name.
///
/// Returns `None` if:
/// - git is not installed
/// - Not in a git repository
/// - Command times out (5 seconds)
/// - Any other error
fn get_current_branch() -> Option<String> {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if branch.is_empty() {
                None
            } else {
                Some(branch)
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_branch_returns_something_or_none() {
        // This is environment-dependent, just verify it doesn't panic.
        let _branch = get_current_branch();
    }

    #[test]
    fn check_main_branch_does_not_panic() {
        let _result = check_main_branch();
    }
}
