//! Copilot SDK auto-installation.
//!
//! Checks whether the GitHub Copilot CLI (`gh copilot`) is available and
//! attempts to install the `gh-copilot` extension if missing.

use std::process::Command;
use tracing::{debug, info, warn};

/// Ensure the Copilot SDK (GitHub CLI copilot extension) is available.
///
/// Returns `true` if `gh copilot` is usable after this call (either it was
/// already installed or auto-install succeeded).
pub fn ensure_copilot_sdk_installed() -> bool {
    if is_copilot_available() {
        debug!("gh copilot is already available");
        return true;
    }

    info!("gh copilot not found, attempting auto-install");

    if !is_gh_available() {
        warn!("GitHub CLI (gh) is not installed — cannot auto-install copilot extension");
        return false;
    }

    if install_copilot_extension() {
        info!("gh copilot extension installed successfully");
        is_copilot_available()
    } else {
        warn!("Failed to install gh copilot extension");
        false
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check if `gh copilot` is available by running `gh copilot --version`.
fn is_copilot_available() -> bool {
    Command::new("gh")
        .args(["copilot", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Check if the `gh` CLI itself is on PATH.
fn is_gh_available() -> bool {
    Command::new("gh")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Attempt to install the copilot extension via `gh extension install`.
fn install_copilot_extension() -> bool {
    match Command::new("gh")
        .args(["extension", "install", "github/gh-copilot", "--force"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status()
    {
        Ok(status) => {
            if status.success() {
                true
            } else {
                warn!(code = ?status.code(), "gh extension install exited non-zero");
                false
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to run gh extension install");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_copilot_available_returns_bool() {
        // Should not panic regardless of environment
        let _ = is_copilot_available();
    }

    #[test]
    fn is_gh_available_returns_bool() {
        let _ = is_gh_available();
    }

    #[test]
    fn ensure_copilot_sdk_installed_returns_bool() {
        // In CI without gh, this gracefully returns false
        let result = ensure_copilot_sdk_installed();
        let _ = result;
    }

    #[test]
    fn install_copilot_extension_returns_bool() {
        // Without gh auth, this should fail gracefully
        let _ = install_copilot_extension();
    }

    #[test]
    fn functions_do_not_panic_without_gh() {
        // Smoke test: none of these should panic even if gh is absent
        let _ = is_gh_available();
        let _ = is_copilot_available();
    }
}
