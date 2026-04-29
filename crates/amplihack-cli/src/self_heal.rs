//! Startup self-heal: re-stage framework assets when the binary version
//! disagrees with the on-disk install stamp.
//!
//! ## Why this exists
//!
//! PR #488 added a post-install hook to `amplihack update` so that after the
//! binary is replaced, framework assets in `~/.amplihack` are re-staged.
//! That hook only fires when the **old** running binary already contains the
//! post-install code path. Users on pre-#488 versions doing
//! `amplihack update` get a swapped binary but no asset re-stage on next
//! launch — the new binary has no idea the prior install is stale.
//!
//! This module closes the gap by version-stamping every successful install
//! into `~/.amplihack/.installed-version` and comparing the stamp against
//! [`crate::VERSION`] on every startup. If the stamp is missing or stale,
//! the install procedure is invoked automatically and the user sees a
//! one-line notice on stderr.
//!
//! ## Failure mode
//!
//! Per the project's Zero-BS philosophy, install failures **propagate**
//! and are surfaced loudly by the binary entrypoint. There is no silent
//! fallback to "continue with stale assets" — the user gets a clear error
//! and a non-zero exit.

use std::ffi::OsString;
use std::io::Write;

use anyhow::{Context, Result};

use crate::commands::install::version_stamp;

/// Env var that fully disables the auto-restage check.
///
/// Useful for CI, integration tests, and any context where install
/// side effects are unwanted. Any non-empty value disables.
const SKIP_ENV: &str = "AMPLIHACK_SKIP_AUTO_INSTALL";

/// Subcommands that must NOT trigger auto-install.
///
/// - `install`/`uninstall`/`update`: would recurse or undo the user's intent.
/// - `completions`/`help`/`doctor`: read-only/diagnostic; should stay fast.
const SKIP_SUBCOMMANDS: &[&str] = &[
    "install",
    "uninstall",
    "update",
    "completions",
    "doctor",
    "help",
];

/// Top-level flags that short-circuit clap and should not pay for an
/// install check.
const SKIP_FLAGS: &[&str] = &["--help", "-h", "--version", "-V"];

/// Public entrypoint, called from `bins/amplihack/src/main.rs` immediately
/// after the update-notice check and before `commands::dispatch`.
///
/// Returns `Ok(())` when no install was needed or after a successful one.
/// Returns `Err` if the install fails — callers are expected to surface
/// the error and abort.
pub fn ensure_assets_match_binary_version(args: &[OsString]) -> Result<()> {
    ensure_assets_match_binary_version_with(args, &mut std::io::stderr(), || {
        crate::commands::install::run_install(None, false)
    })
}

/// Decision-logic core, factored out for testability.
///
/// Mirrors the closure-injection pattern used by
/// `crate::update::post_install::run_post_update_install` so unit tests
/// can verify the decision tree without running a real install or touching
/// stderr.
pub(crate) fn ensure_assets_match_binary_version_with<W, F>(
    args: &[OsString],
    notice: &mut W,
    install_fn: F,
) -> Result<()>
where
    W: Write,
    F: FnOnce() -> Result<()>,
{
    if env_bypass_set() {
        tracing::debug!("self_heal: skipped via {SKIP_ENV}");
        return Ok(());
    }
    if args_should_skip(args) {
        tracing::debug!("self_heal: skipped (subcommand or flag in skip list)");
        return Ok(());
    }

    let stamp = version_stamp::read_installed_version().context("reading install version stamp")?;
    let expected = crate::VERSION;

    if stamp.as_deref() == Some(expected) {
        tracing::debug!("self_heal: stamp matches binary version {expected}");
        return Ok(());
    }

    match stamp.as_deref() {
        Some(prior) => {
            tracing::info!(
                "self_heal: install stamp {prior} != binary {expected}; re-staging assets"
            );
        }
        None => {
            tracing::info!("self_heal: install stamp missing; staging assets for {expected}");
        }
    }

    install_fn().context("running install during startup self-heal")?;
    version_stamp::write_installed_version(expected)
        .context("writing install version stamp after self-heal")?;
    writeln!(
        notice,
        "amplihack: framework assets re-staged for v{expected}"
    )
    .context("emitting self-heal notice")?;
    Ok(())
}

/// True when `AMPLIHACK_SKIP_AUTO_INSTALL` is set to any non-empty value.
fn env_bypass_set() -> bool {
    std::env::var_os(SKIP_ENV)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Fast pre-clap scan of `args` to determine whether the user's command
/// should bypass the self-heal check.
///
/// We deliberately avoid calling clap here — clap parsing happens after
/// this function returns, and asking clap to parse twice would risk
/// double-erroring on bad args.
fn args_should_skip(args: &[OsString]) -> bool {
    // No subcommand at all -> clap will print help. Skip.
    if args.len() < 2 {
        return true;
    }

    for arg in args.iter().skip(1) {
        let Some(s) = arg.to_str() else {
            // Non-UTF8 arg: cannot be one of the ASCII skip tokens, but
            // could still be a positional value. Keep scanning.
            continue;
        };
        if SKIP_FLAGS.contains(&s) {
            return true;
        }
    }

    // First non-flag positional is the subcommand candidate.
    for arg in args.iter().skip(1) {
        let Some(s) = arg.to_str() else { continue };
        if s.starts_with('-') {
            continue;
        }
        return SKIP_SUBCOMMANDS.contains(&s);
    }

    // No positional found (only flags). Skip — nothing meaningful to run.
    true
}

/// Test-only: this module's tests share the crate-wide
/// `crate::test_support::env_lock` to coordinate with other env-mutating
/// tests (e.g., `nesting::tests`, `version_stamp::tests`,
/// `copilot_setup::tests`).
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use tempfile::TempDir;

    /// Save/restore guard for `HOME` and `AMPLIHACK_SKIP_AUTO_INSTALL`.
    /// Uses the crate-wide [`crate::test_support::env_lock`] so that this
    /// module's tests cannot race tests in other modules that mutate the
    /// same env vars.
    struct EnvGuard {
        prior_home: Option<OsString>,
        prior_skip: Option<OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new(home: &std::path::Path, skip: Option<&str>) -> Self {
            let lock = crate::test_support::env_lock()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let prior_home = std::env::var_os("HOME");
            let prior_skip = std::env::var_os(SKIP_ENV);
            // SAFETY: edition 2024 requires unsafe; serialized via env_lock.
            unsafe {
                std::env::set_var("HOME", home);
                match skip {
                    Some(v) => std::env::set_var(SKIP_ENV, v),
                    None => std::env::remove_var(SKIP_ENV),
                }
            }
            EnvGuard {
                prior_home,
                prior_skip,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: serialized via ENV_LOCK held in self._lock.
            unsafe {
                match self.prior_home.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
                match self.prior_skip.take() {
                    Some(v) => std::env::set_var(SKIP_ENV, v),
                    None => std::env::remove_var(SKIP_ENV),
                }
            }
        }
    }

    fn args(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(|s| OsString::from(*s)).collect()
    }

    /// Helper: build an installer closure that records invocation count
    /// and writes the given version to the stamp on call.
    fn counting_installer<'a>(
        counter: &'a Cell<u32>,
        version: &'static str,
    ) -> impl FnOnce() -> Result<()> + 'a {
        move || {
            counter.set(counter.get() + 1);
            version_stamp::write_installed_version(version)?;
            Ok(())
        }
    }

    #[test]
    fn stamp_mismatch_triggers_install() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        version_stamp::write_installed_version("0.8.55").unwrap();

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 1, "installer should run on mismatch");
        let notice = String::from_utf8(buf).unwrap();
        assert!(
            notice.contains(&format!(
                "framework assets re-staged for v{}",
                crate::VERSION
            )),
            "unexpected notice: {notice}"
        );
        assert_eq!(
            version_stamp::read_installed_version().unwrap().as_deref(),
            Some(crate::VERSION),
            "stamp should be rewritten to current version"
        );
    }

    #[test]
    fn stamp_match_skips_install() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        version_stamp::write_installed_version(crate::VERSION).unwrap();

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 0, "installer must not run when stamp matches");
        assert!(buf.is_empty(), "no notice when stamp matches");
    }

    #[test]
    fn missing_stamp_triggers_install() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        // Note: no stamp file exists.

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 1, "installer should run when stamp missing");
        let notice = String::from_utf8(buf).unwrap();
        assert!(notice.contains("re-staged"), "notice missing: {notice}");
    }

    #[test]
    fn env_bypass_skips_install_even_with_mismatch() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), Some("1"));
        version_stamp::write_installed_version("0.0.0").unwrap();

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 0, "installer must not run when bypass set");
        assert!(buf.is_empty());
        // Stamp should be unchanged.
        assert_eq!(
            version_stamp::read_installed_version().unwrap().as_deref(),
            Some("0.0.0")
        );
    }

    #[test]
    fn empty_bypass_value_does_not_bypass() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), Some(""));
        // No stamp -> install should run despite bypass var being present-but-empty.

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn install_subcommand_skips_to_avoid_recursion() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        // No stamp -> would normally trigger; subcommand must skip anyway.

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "install"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn update_subcommand_skips() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "update"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn help_flag_skips() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "--help"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn version_flag_skips() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "--version"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn no_args_skips() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn install_failure_propagates_and_stamp_not_written() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        // No stamp -> would trigger install.

        let mut buf = Vec::new();
        let result = ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            || Err(anyhow::anyhow!("simulated install failure")),
        );

        assert!(result.is_err(), "install failure must propagate");
        assert!(
            version_stamp::read_installed_version().unwrap().is_none(),
            "stamp must not be written when install fails"
        );
        assert!(buf.is_empty(), "no notice when install fails");
    }
}
