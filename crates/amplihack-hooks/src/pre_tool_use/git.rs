//! Git-related protection checks.
//!
//! - Main branch protection: blocks commits to main/master.
//! - Runs `git branch --show-current` with a 5-second timeout.

use serde_json::Value;
use std::process::Command;
use std::time::Duration;

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

/// Git command timeout (matches Python's 5-second timeout).
const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Get the current git branch name.
///
/// Returns `None` if:
/// - git is not installed
/// - Not in a git repository
/// - Command times out (5 seconds)
/// - Any other error
fn get_current_branch() -> Option<String> {
    let mut child = Command::new("git")
        .args(["branch", "--show-current"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Poll for completion with timeout.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let output = child.wait_with_output().ok()?;
                let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
                return if branch.is_empty() {
                    None
                } else {
                    Some(branch)
                };
            }
            Ok(Some(_)) => return None, // Non-zero exit.
            Ok(None) => {
                if start.elapsed() > GIT_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    tracing::warn!("git branch --show-current timed out after 5s");
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_branch_returns_something_or_none() {
        let branch = get_current_branch();
        if let Some(name) = &branch {
            assert!(!name.is_empty(), "branch name should not be empty");
        }
    }

    #[test]
    fn check_main_branch_does_not_panic() {
        let result = check_main_branch();
        assert!(result.is_ok(), "check_main_branch should not error");
        // Result is either Some (on protected branch) or None (not protected / not a repo).
        if let Some(val) = result.unwrap() {
            assert!(
                val.get("block").is_some(),
                "block JSON should have 'block' key"
            );
        }
    }
}
