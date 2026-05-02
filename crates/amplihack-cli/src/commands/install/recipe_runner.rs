//! Recipe-runner-rs install/probe phase.
//!
//! Issue #527 (BUG 2): the install command previously printed an ❌ marker
//! when `recipe-runner-rs` was absent and continued, returning `Ok(())`.
//! Per the install-completeness invariant in
//! `amplifier-bundle/context/PHILOSOPHY.md`, install must fail loudly when a
//! required component cannot be placed.
//!
//! This module owns the entire phase: probe → optional cargo-install →
//! re-probe → loud bail. It is invoked from
//! [`super::run_install`] via [`ensure_recipe_runner`].
//!
//! Hermetic-test escape hatch: if `AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL=1`
//! is set, the cargo-install branch is skipped (probe + bail still run).
//! Used by `install/tests/issue_527_tests.rs` to exercise the bail path
//! without touching the network.
//!
//! Returns an [`Outcome`] describing what happened so the caller can
//! pretty-print user-facing status lines.

use anyhow::{Result, bail};

use crate::freshness::{install_recipe_runner_from_git, recipe_runner_binary_present};

/// What `ensure_recipe_runner` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Binary was already discoverable on PATH (or via the
    /// `RECIPE_RUNNER_RS_PATH` override / standard cargo/local bin
    /// directories) when we probed.
    AlreadyOnPath,
    /// Binary was missing; we ran `cargo install --git ...` and the
    /// re-probe succeeded.
    InstalledFromGit,
    /// Cargo-install branch was skipped via
    /// `AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL=1` *and* the binary was
    /// present at probe time. (If it was absent, we bail instead — see
    /// [`ensure_recipe_runner`].)
    SkippedByEnv,
}

const REMEDIATION: &str = "recipe-runner-rs is required for `amplihack recipe run` and the dev-orchestrator skill. \
     Install it manually with:\n    \
     cargo install --git https://github.com/rysweet/amplihack-recipe-runner --branch main --locked\n\
     Then re-run `amplihack install`. \
     Override the binary location with RECIPE_RUNNER_RS_PATH=/path/to/recipe-runner-rs.";

/// Ensure `recipe-runner-rs` is reachable on PATH (or via the override env
/// vars probed by [`recipe_runner_binary_present`]). Prints a one-line
/// status banner.
///
/// Behavior:
/// 1. Probe. If present → return `AlreadyOnPath`.
/// 2. If `AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL=1` → bail (env-skip without
///    a present binary is the install-completeness violation).
/// 3. Otherwise run `cargo install --git ...` via
///    [`install_recipe_runner_from_git`].
/// 4. Re-probe. Present → return `InstalledFromGit`. Absent → bail.
pub(super) fn ensure_recipe_runner() -> Result<Outcome> {
    if recipe_runner_binary_present() {
        println!("   ✅ recipe-runner-rs is available");
        return Ok(Outcome::AlreadyOnPath);
    }

    let skip_install = std::env::var_os("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL")
        .map(|v| !v.is_empty())
        .unwrap_or(false);

    if skip_install {
        // Caller (typically a hermetic test) asked us to skip the network
        // path, but the binary is also missing — that violates
        // install-completeness. Surface a clear remediation.
        bail!(
            "recipe-runner-rs not found on PATH and AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL=1 \
             disabled the cargo install fallback. {}",
            REMEDIATION
        );
    }

    println!("   ⏬ recipe-runner-rs missing — installing from git (cargo install --locked)");
    if let Err(err) = install_recipe_runner_from_git() {
        bail!(
            "failed to install recipe-runner-rs via cargo: {err:#}. {}",
            REMEDIATION
        );
    }

    if recipe_runner_binary_present() {
        println!("   ✅ recipe-runner-rs installed from git");
        Ok(Outcome::InstalledFromGit)
    } else {
        // cargo install reported success but the binary still isn't
        // discoverable on PATH (for example, ~/.cargo/bin not in PATH).
        bail!(
            "cargo install for recipe-runner-rs reported success but the binary is still not \
             on PATH. Add ~/.cargo/bin to PATH and re-run `amplihack install`, or set \
             RECIPE_RUNNER_RS_PATH explicitly. {}",
            REMEDIATION
        );
    }
}

// `SkippedByEnv` is reserved for a future code path that allows opting out
// when the binary IS present (currently `AlreadyOnPath` covers that case).
// Suppress dead-code lint on the variant rather than dropping it — the
// ergonomic enum is documented in the design spec.
#[allow(dead_code)]
const _: Outcome = Outcome::SkippedByEnv;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::home_env_lock;
    use std::sync::MutexGuard;

    /// Acquire the global HOME lock (also used by other env-mutating tests
    /// in this crate) so PATH/env mutations here don't race other tests.
    fn lock_env() -> MutexGuard<'static, ()> {
        home_env_lock().lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn ensure_recipe_runner_uses_path_override_when_set() {
        let _guard = lock_env();
        let temp = tempfile::tempdir().unwrap();
        let stub = temp.path().join("recipe-runner-rs");
        std::fs::write(&stub, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let prev = std::env::var_os("RECIPE_RUNNER_RS_PATH");
        let prev_skip = std::env::var_os("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL");
        unsafe {
            std::env::set_var("RECIPE_RUNNER_RS_PATH", &stub);
            // Ensure the env-skip branch is NOT chosen — we want to verify
            // the override path satisfies the present-check.
            std::env::remove_var("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL");
        }
        let outcome = ensure_recipe_runner().expect("present-check must succeed via override");
        assert_eq!(outcome, Outcome::AlreadyOnPath);
        unsafe {
            if let Some(v) = prev {
                std::env::set_var("RECIPE_RUNNER_RS_PATH", v);
            } else {
                std::env::remove_var("RECIPE_RUNNER_RS_PATH");
            }
            if let Some(v) = prev_skip {
                std::env::set_var("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", v);
            }
        }
    }

    #[test]
    fn ensure_recipe_runner_bails_when_missing_with_env_skip() {
        let _guard = lock_env();
        let temp = tempfile::tempdir().unwrap();
        let prev_path = std::env::var_os("PATH");
        let prev_override = std::env::var_os("RECIPE_RUNNER_RS_PATH");
        let prev_skip = std::env::var_os("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL");
        let prev_home = std::env::var_os("HOME");
        unsafe {
            // Empty PATH dir + tempdir HOME so neither $HOME/.cargo/bin nor
            // PATH dirs contain a real recipe-runner-rs.
            std::env::set_var("PATH", temp.path());
            std::env::set_var("HOME", temp.path());
            std::env::remove_var("RECIPE_RUNNER_RS_PATH");
            std::env::set_var("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");
        }
        let result = ensure_recipe_runner();
        unsafe {
            if let Some(v) = prev_path {
                std::env::set_var("PATH", v);
            }
            if let Some(v) = prev_override {
                std::env::set_var("RECIPE_RUNNER_RS_PATH", v);
            }
            if let Some(v) = prev_skip {
                std::env::set_var("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", v);
            } else {
                std::env::remove_var("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL");
            }
            if let Some(v) = prev_home {
                std::env::set_var("HOME", v);
            }
        }
        let err = result.expect_err("must bail when missing + env-skip set");
        let msg = format!("{err:#}").to_ascii_lowercase();
        assert!(msg.contains("recipe-runner-rs"), "msg={msg}");
        assert!(msg.contains("cargo install"), "msg={msg}");
    }
}
