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
    match probe("gh copilot", &["copilot", "--version"]) {
        Availability::Usable => {
            debug!("gh copilot is already available");
            return true;
        }
        // Issue #1424: a probe that could not run says nothing about whether
        // the extension is installed. Reporting "not found" and spending an
        // install on it names a cause that was never established — and on a
        // host that is out of processes, the install cannot run either.
        Availability::Undetermined(reason) => {
            warn!(
                %reason,
                "could not determine whether gh copilot is available; not \
                 attempting an install"
            );
            return false;
        }
        Availability::Unusable => {}
    }

    info!("gh copilot not found, attempting auto-install");

    match probe("gh", &["--version"]) {
        Availability::Usable => {}
        Availability::Unusable => {
            warn!("GitHub CLI (gh) is not installed — cannot auto-install copilot extension");
            return false;
        }
        Availability::Undetermined(reason) => {
            warn!(%reason, "could not run the GitHub CLI (gh) to check it; not \
                 attempting an install");
            return false;
        }
    }

    if install_copilot_extension() {
        info!("gh copilot extension installed successfully");
        matches!(
            probe("gh copilot", &["copilot", "--version"]),
            Availability::Usable
        )
    } else {
        warn!("Failed to install gh copilot extension");
        false
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// What a `--version` probe actually established.
///
/// Three answers, not two (issue #1424). `status().is_ok_and(|s| s.success())`
/// collapses "it answered no" with "it never ran", and only the first is
/// evidence that something is not installed.
#[derive(Debug)]
enum Availability {
    /// It ran and exited 0.
    Usable,
    /// It ran, or is genuinely absent, and the answer is no.
    Unusable,
    /// Nothing was established: the probe could not be run, or was killed.
    Undetermined(String),
}

/// Run `gh <args> --version`-shaped probe and say what it established.
fn probe(label: &str, args: &[&str]) -> Availability {
    match Command::new("gh")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Availability::Usable,
        // A clean non-zero exit is an answer: `gh` ran and said no.
        Ok(status) if status.code().is_some() => Availability::Unusable,
        // Killed by a signal: it was there, it started, and something took it
        // down. That is not "not installed".
        Ok(status) => Availability::Undetermined(format!("{label} was killed ({status})")),
        // Absence is the ONE spawn error that proves the tool is not there.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Availability::Unusable,
        Err(error) => Availability::Undetermined(format!("{label} could not be run: {error}")),
    }
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
    fn probe_returns_an_answer_without_panicking() {
        // Should not panic regardless of environment.
        let _ = probe("gh copilot", &["copilot", "--version"]);
    }

    #[test]
    fn a_missing_gh_is_an_answer_not_an_unknown() {
        // Absence is the one spawn error that establishes anything: a tool
        // that is not on `$PATH` really is not installed.
        assert!(matches!(
            probe("nope", &["definitely-not-a-gh-subcommand-xyz"]),
            Availability::Usable | Availability::Unusable
        ));
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
        let _ = probe("gh", &["--version"]);
        let _ = probe("gh copilot", &["copilot", "--version"]);
    }
}
