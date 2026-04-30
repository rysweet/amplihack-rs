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
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use fs4::fs_std::FileExt;

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

/// Filename of the inter-process advisory lock guarding the installer
/// critical section (issue #502 R5). Sits next to the install stamp under
/// `~/.amplihack/`.
const INSTALL_LOCK_FILE: &str = ".install.lock";

/// Public entrypoint, called from `bins/amplihack/src/main.rs` immediately
/// after the update-notice check and before `commands::dispatch`.
///
/// Returns `Ok(())` when no install was needed or after a successful one.
/// Returns `Err` if the install fails — callers are expected to surface
/// the error and abort.
pub fn ensure_assets_match_binary_version(args: &[OsString]) -> Result<()> {
    // Issue #502 R7: narrow HOME-unset carve-out. Probe `HOME` directly
    // (not via `paths::home_dir()`) so we can distinguish "no home" from
    // any other path-helper error and skip gracefully without swallowing
    // unrelated failures. This is the ONLY intentionally silent path in
    // this module.
    if std::env::var_os("HOME")
        .map(|v| v.is_empty())
        .unwrap_or(true)
    {
        tracing::debug!("self_heal: HOME unset; skipping (graceful carve-out)");
        return Ok(());
    }

    ensure_assets_match_binary_version_with(args, &mut std::io::stderr(), || {
        crate::commands::install::run_install(None, false)
    })
}

/// Resolve the path to the install advisory lock file. Mirrors
/// `version_stamp::installed_version_path` so both files live side-by-side.
fn install_lock_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .context("HOME is not set")?;
    Ok(home.join(".amplihack").join(INSTALL_LOCK_FILE))
}

/// Run `body` while holding an exclusive advisory `flock` on the install
/// lock file (issue #502 R5). The lock serialises concurrent
/// `ensure_assets_match_binary_version` callers within a host so two
/// processes never run the installer body at the same time.
///
/// The lock file is created with default permissions if missing — its
/// contents are never inspected, only its inode is used as the lock token,
/// so it does not need 0o600 like the stamp.
fn with_install_lock<T, F: FnOnce() -> Result<T>>(body: F) -> Result<T> {
    let lock_path = install_lock_path()?;
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open install lock {}", lock_path.display()))?;
    FileExt::lock_exclusive(&lock_file)
        .with_context(|| format!("failed to acquire install lock {}", lock_path.display()))?;
    let result = body();
    // Best-effort unlock; the kernel will release on drop regardless.
    let _ = FileExt::unlock(&lock_file);
    result
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
    if args_should_skip(args) {
        tracing::debug!("self_heal: skipped (subcommand or flag in skip list)");
        return Ok(());
    }

    // Issue #502 R7: if `HOME` is unset (or empty), skip gracefully. The
    // public entrypoint already performs this check; we re-check here so
    // unit tests that drive the `_with` variant directly receive the same
    // behaviour. This is the only intentionally silent path in the module.
    if std::env::var_os("HOME")
        .map(|v| v.is_empty())
        .unwrap_or(true)
    {
        tracing::debug!("self_heal: HOME unset; skipping (graceful carve-out)");
        return Ok(());
    }

    let stamp = version_stamp::read_installed_version().context("reading install version stamp")?;
    let expected = crate::VERSION;

    // Issue #502 R6: when bypass is set AND there is a real skew, emit a
    // single-line stderr diagnostic so CI logs surface the drift. Both
    // values are constant or regex-validated semver, so plain formatting
    // is safe (no control-char injection surface).
    if env_bypass_set() {
        let stamp_match = stamp.as_deref() == Some(expected);
        if !stamp_match {
            let stamp_str = stamp.as_deref().unwrap_or("<missing>");
            writeln!(
                notice,
                "amplihack: AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage \
                 (stamp={stamp_str} current={expected})"
            )
            .context("emitting bypass diagnostic")?;
        }
        tracing::debug!("self_heal: skipped via {SKIP_ENV}");
        return Ok(());
    }

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

    // Issue #502 R5: serialise installer + stamp write across processes
    // via an advisory `flock` on `~/.amplihack/.install.lock`. The second
    // waiter re-reads the stamp inside the critical section; if the first
    // winner already wrote the up-to-date stamp, the waiter exits without
    // re-running the installer.
    with_install_lock(|| {
        // Re-check after acquiring the lock — the previous holder may
        // have already brought the stamp current.
        let stamp_now = version_stamp::read_installed_version()
            .context("re-reading install stamp under lock")?;
        if stamp_now.as_deref() == Some(expected) {
            tracing::debug!(
                "self_heal: stamp brought current by concurrent installer; nothing to do"
            );
            return Ok(());
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
    })
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
        // Issue #502 R6: bypass-with-mismatch emits a single-line diagnostic.
        let line = String::from_utf8(buf).unwrap();
        assert!(
            line.contains("AMPLIHACK_SKIP_AUTO_INSTALL")
                && line.contains("stamp=0.0.0")
                && line.contains(&format!("current={}", crate::VERSION)),
            "expected bypass diagnostic; got: {line}"
        );
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

    // ----- TDD tests for issue #502 hardening -----
    //
    // These tests pin the contract for behaviour deferred from PR #500.
    // They fail until the matching implementation lands in self_heal.

    /// R6: bypass with a stamp/binary mismatch must emit a single-line
    /// stderr diagnostic via the injected `notice` writer. Format must
    /// include both `stamp=<v>` and `current=<V>` so CI logs make the
    /// skew obvious. Plain (non-`{:?}`) formatting is safe because both
    /// values pass the semver regex first.
    #[test]
    fn bypass_with_mismatch_emits_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), Some("1"));
        version_stamp::write_installed_version("0.0.1").unwrap();

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 0, "bypass must skip installer");
        let line = String::from_utf8(buf).unwrap();
        assert!(
            line.contains("AMPLIHACK_SKIP_AUTO_INSTALL"),
            "diagnostic must mention the env var; got: {line}"
        );
        assert!(
            line.contains("stamp=0.0.1"),
            "diagnostic must include stamp value; got: {line}"
        );
        assert!(
            line.contains(&format!("current={}", crate::VERSION)),
            "diagnostic must include current version; got: {line}"
        );
        // Single line.
        assert_eq!(
            line.trim_end_matches('\n').lines().count(),
            1,
            "diagnostic must be a single line; got: {line:?}"
        );
    }

    /// R6: bypass without a stamp mismatch must NOT emit the diagnostic
    /// (no skew = nothing interesting to report).
    #[test]
    fn bypass_with_match_emits_no_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), Some("1"));
        version_stamp::write_installed_version(crate::VERSION).unwrap();

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 0);
        assert!(
            buf.is_empty(),
            "no diagnostic when bypass and no skew; got: {:?}",
            String::from_utf8_lossy(&buf)
        );
    }

    /// R7: with `HOME` unset, `ensure_assets_match_binary_version`
    /// must return `Ok(())` (graceful skip) rather than propagating
    /// the home_dir() error. This is the documented carve-out — the
    /// only intentionally silent path. Implemented by probing
    /// `std::env::var_os("HOME").is_none()` BEFORE calling
    /// `paths::home_dir()`.
    #[test]
    fn missing_home_skips_gracefully() {
        // Acquire the env lock and unset HOME for the duration.
        let lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prior_home = std::env::var_os("HOME");
        let prior_skip = std::env::var_os(SKIP_ENV);
        // SAFETY: serialized via env_lock.
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var(SKIP_ENV);
        }

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        let result = ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        );

        // Restore env BEFORE asserting so a panic doesn't poison other tests.
        // SAFETY: serialized via env_lock held in `lock`.
        unsafe {
            match prior_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prior_skip {
                Some(v) => std::env::set_var(SKIP_ENV, v),
                None => std::env::remove_var(SKIP_ENV),
            }
        }
        drop(lock);

        result.expect("HOME-unset must be a graceful skip, not an error");
        assert_eq!(calls.get(), 0, "installer must not run when HOME unset");
        assert!(buf.is_empty(), "no notice when HOME unset");
    }

    /// R5: concurrent installs must serialize via the advisory file
    /// lock on `~/.amplihack/.install.lock`. Two threads that both
    /// trigger a re-stage must NOT execute the installer body
    /// concurrently — their critical sections must not temporally
    /// overlap.
    #[test]
    fn concurrent_installs_serialize() {
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let tmp = TempDir::new().unwrap();
        // Hold the env_lock for the whole test so the two threads share
        // a stable HOME. EnvGuard takes the same lock; we set HOME
        // manually to avoid double-locking.
        let env_lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prior_home = std::env::var_os("HOME");
        let prior_skip = std::env::var_os(SKIP_ENV);
        // SAFETY: serialized via env_lock above.
        unsafe {
            std::env::set_var("HOME", tmp.path());
            std::env::remove_var(SKIP_ENV);
        }

        // Capture (start, end) of each installer critical section.
        let timings: Arc<Mutex<Vec<(Instant, Instant)>>> = Arc::new(Mutex::new(Vec::new()));

        let spawn = || {
            let timings = Arc::clone(&timings);
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                ensure_assets_match_binary_version_with(
                    &args(&["amplihack", "launch"]),
                    &mut buf,
                    move || {
                        let start = Instant::now();
                        std::thread::sleep(Duration::from_millis(50));
                        let end = Instant::now();
                        timings.lock().unwrap().push((start, end));
                        version_stamp::write_installed_version(crate::VERSION)?;
                        Ok(())
                    },
                )
            })
        };

        let h1 = spawn();
        let h2 = spawn();
        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();

        // Restore env.
        // SAFETY: serialized via env_lock held above.
        unsafe {
            match prior_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prior_skip {
                Some(v) => std::env::set_var(SKIP_ENV, v),
                None => std::env::remove_var(SKIP_ENV),
            }
        }
        drop(env_lock);

        r1.expect("thread 1 ok");
        r2.expect("thread 2 ok");

        let runs = timings.lock().unwrap();
        // Either:
        //  - both threads ran the installer and their critical sections
        //    are disjoint (lock serialized them), OR
        //  - the second thread, after taking the lock, re-read the stamp
        //    and saw a match, so only one installer call happened.
        // Both outcomes prove the lock is doing its job.
        match runs.len() {
            1 => { /* second waiter saw the match after first finished; ok */ }
            2 => {
                let (a_start, a_end) = runs[0];
                let (b_start, b_end) = runs[1];
                let overlap = a_start < b_end && b_start < a_end;
                assert!(
                    !overlap,
                    "installer critical sections must not overlap: \
                     a=[{a_start:?},{a_end:?}] b=[{b_start:?},{b_end:?}]"
                );
            }
            n => panic!("unexpected installer run count: {n}"),
        }
    }
}
