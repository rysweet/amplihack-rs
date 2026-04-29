//! Post-update install orchestration.
//!
//! After a successful binary self-update, we re-run the `install` step to
//! ensure framework assets in `~/.amplihack/.claude` are in sync with the
//! freshly-installed binary. Users who want the legacy binary-only update
//! can opt out via `--skip-install` (alias `--no-install`).
//!
//! This module exposes a single helper, [`run_post_update_install`], that
//! takes a closure for the actual install step. Closure injection keeps
//! tests pure — no network, no filesystem, no real install side effects.

use anyhow::Result;

/// Run the post-update install step unless the user opted out.
///
/// * When `skip_install` is `true`, log and return `Ok(())` without invoking
///   the installer.
/// * Otherwise, invoke `installer()` and propagate its result via `?`.
pub(super) fn run_post_update_install<F>(skip_install: bool, installer: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    if skip_install {
        tracing::info!("skipping post-update install (--skip-install)");
        return Ok(());
    }
    tracing::info!("Running post-update install...");
    installer()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn invokes_installer_when_skip_install_is_false() {
        let called = Cell::new(0u32);
        let result = run_post_update_install(false, || {
            called.set(called.get() + 1);
            Ok(())
        });
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(called.get(), 1, "installer must be invoked exactly once");
    }

    #[test]
    fn does_not_invoke_installer_when_skip_install_is_true() {
        let called = Cell::new(0u32);
        let result = run_post_update_install(true, || {
            called.set(called.get() + 1);
            Ok(())
        });
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(
            called.get(),
            0,
            "installer must NOT be invoked when skip_install=true"
        );
    }

    #[test]
    fn propagates_installer_error() {
        let result = run_post_update_install(false, || {
            Err(anyhow::anyhow!("install failed: synthetic test error"))
        });
        let err = result.expect_err("expected installer error to propagate");
        assert!(
            err.to_string().contains("synthetic test error"),
            "error message should propagate verbatim, got: {err}"
        );
    }

    #[test]
    fn skip_install_short_circuits_before_installer_error() {
        // Structural guarantee: when skip_install=true, the installer closure
        // is never invoked — even if it would have errored. This mirrors the
        // real call site's expectation that download_and_replace failures
        // short-circuit before reaching the installer (`?` propagation).
        let result = run_post_update_install(true, || {
            panic!("installer must not be called when skip_install=true");
        });
        assert!(result.is_ok());
    }
}
