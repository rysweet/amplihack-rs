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

use crate::freshness::{
    InstallResult, RunnerTarget, install_recipe_runner_from_git_with,
    managed_runner_needs_reconcile_with, recipe_runner_binary_present,
};
use crate::runner_ancestry::{RunnerOrder, skip_message};

/// What `ensure_recipe_runner` did.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The installed runner is newer than, diverged from, or not comparable
    /// with the pin, so it was kept rather than replaced (issue #1593).
    KeptInstalled {
        installed: String,
        pinned: String,
        class: RunnerOrder,
    },
}

fn remediation() -> String {
    format!(
        "recipe-runner-rs is required for `amplihack recipe run` and the dev-orchestrator skill. \
     Install it manually with:\n    \
     cargo install --git https://github.com/rysweet/amplihack-recipe-runner --rev {} --locked\n\
     Then re-run `amplihack install`. \
     Override the binary location with RECIPE_RUNNER_RS_PATH=/path/to/recipe-runner-rs.",
        crate::freshness::RECIPE_RUNNER_REV.trim()
    )
}

/// Ensure `recipe-runner-rs` is reachable on PATH (or via the override env
/// vars probed by [`recipe_runner_binary_present`]). Prints a one-line
/// status banner.
///
/// Behavior:
/// 1. Probe. If present → return `AlreadyOnPath`.
/// 2. If `AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL=1` → bail (env-skip without
///    a present binary is the install-completeness violation).
/// 3. Otherwise run `cargo install --git ...` via
///    [`install_recipe_runner_from_git_with`], which keeps a runner newer
///    than the pin and returns `KeptInstalled`.
/// 4. Re-probe. Present → return `InstalledFromGit`. Absent → bail.
pub(super) fn ensure_recipe_runner() -> Result<Outcome> {
    if std::env::var_os("RECIPE_RUNNER_RS_PATH").is_some_and(|v| !v.is_empty()) {
        return ensure_recipe_runner_with_override(&remediation());
    }
    ensure_recipe_runner_with(&RunnerTarget::pinned()?)
}

fn ensure_recipe_runner_with_override(remediation: &str) -> Result<Outcome> {
    crate::freshness::probe_recipe_runner().map_err(|e| anyhow::anyhow!(
        "RECIPE_RUNNER_RS_PATH selects an incompatible runner: {e:#}. Preserve this binary; select a compatible build or unset the override. {remediation}"))?;
    Ok(Outcome::AlreadyOnPath)
}

pub(super) fn ensure_recipe_runner_with(target: &RunnerTarget) -> Result<Outcome> {
    let remediation = remediation();
    if recipe_runner_binary_present() && !managed_runner_needs_reconcile_with(target) {
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
             disabled the cargo install fallback. {remediation}"
        );
    }

    println!(
        "   ⏬ recipe-runner-rs missing or stale — reconciling with pinned revision (cargo install --locked)"
    );
    // Only the user-typed `amplihack install` may bootstrap a Rust toolchain;
    // self-heal and ensure_framework_installed reach this same function.
    let bootstrap = crate::rust_toolchain::bootstrap_permitted();
    match install_recipe_runner_from_git_with(target, bootstrap) {
        Ok(InstallResult::Installed) => {}
        Ok(InstallResult::Kept(skip)) => {
            println!(
                "   ⚠️  {}",
                skip_message(short(&skip.installed), short(&skip.pin), skip.class)
            );
            return Ok(Outcome::KeptInstalled {
                installed: skip.installed,
                pinned: skip.pin,
                class: skip.class,
            });
        }
        Err(err) => {
            bail!("failed to install recipe-runner-rs via cargo: {err:#}. {remediation}");
        }
    }

    if recipe_runner_binary_present() {
        println!("   ✅ recipe-runner-rs installed from git");
        Ok(Outcome::InstalledFromGit)
    } else {
        // cargo install reported success but the binary is in none of the
        // places `rust_toolchain::find_recipe_runner` probes. `$CARGO_HOME/bin`
        // is always probed, so cargo must have installed somewhere else
        // (`CARGO_INSTALL_ROOT`, cargo's `install.root` setting, or `--root`).
        bail!(
            "cargo install for recipe-runner-rs reported success but the binary is still not \
             discoverable. cargo installed it outside $CARGO_HOME/bin (check CARGO_INSTALL_ROOT \
             or cargo's `install.root` setting; `cargo install --list` shows the root). Set \
             RECIPE_RUNNER_RS_PATH to the installed binary and re-run `amplihack install`. \
             {remediation}"
        );
    }
}

/// The first 12 characters of a SHA, for terminal output.
fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
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
        std::fs::write(&stub, "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\"]}'\n").unwrap();
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
        let prev_cargo_home = std::env::var_os("CARGO_HOME");
        unsafe {
            // Empty PATH dir + tempdir HOME, no CARGO_HOME, so neither
            // $CARGO_HOME/bin, $HOME/.cargo/bin nor PATH dirs contain a real
            // recipe-runner-rs.
            std::env::set_var("PATH", temp.path());
            std::env::set_var("HOME", temp.path());
            std::env::remove_var("CARGO_HOME");
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
            if let Some(v) = prev_cargo_home {
                std::env::set_var("CARGO_HOME", v);
            }
        }
        let err = result.expect_err("must bail when missing + env-skip set");
        let msg = format!("{err:#}").to_ascii_lowercase();
        assert!(msg.contains("recipe-runner-rs"), "msg={msg}");
        assert!(msg.contains("cargo install"), "msg={msg}");
    }

    /// Issue: startup self-heal and `ensure_framework_installed` reach
    /// `ensure_recipe_runner` too. Outside the explicit `amplihack install`
    /// scope a missing toolchain must fail with the manual hint, never
    /// download rustup or run apt as root.
    #[test]
    fn implicit_install_never_bootstraps_a_toolchain() {
        let _guard = lock_env();
        let temp = tempfile::tempdir().unwrap();
        let saved: Vec<(&str, Option<std::ffi::OsString>)> = [
            "PATH",
            "HOME",
            "CARGO_HOME",
            "RECIPE_RUNNER_RS_PATH",
            "AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL",
            "AMPLIHACK_NO_RUST_BOOTSTRAP",
        ]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect();
        unsafe {
            // No cargo, no curl/wget, no recipe-runner-rs anywhere: were
            // bootstrapping attempted, the error would name curl/wget.
            std::env::set_var("PATH", temp.path());
            std::env::set_var("HOME", temp.path());
            for key in [
                "CARGO_HOME",
                "RECIPE_RUNNER_RS_PATH",
                "AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL",
                "AMPLIHACK_NO_RUST_BOOTSTRAP",
            ] {
                std::env::remove_var(key);
            }
        }
        let result = ensure_recipe_runner();
        unsafe {
            for (key, value) in saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        let msg = format!("{:#}", result.expect_err("no toolchain, no bootstrap"));
        assert!(msg.contains("cargo is required"), "msg={msg}");
        assert!(
            !msg.contains("curl or wget"),
            "must not try to download rustup: {msg}"
        );
        assert!(!temp.path().join(".cargo").exists());
    }
}

/// Installer downgrade guard (issue #1593): `amplihack install` keeps a runner
/// newer than the pin and reports it as `KeptInstalled`.
#[cfg(all(test, unix))]
mod downgrade_guard_tests {
    use super::*;
    use crate::freshness::RunnerTarget;
    use crate::runner_ancestry::{AncestrySource, FORCE_ENV, RunnerOrder, fixture};
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    const URL: &str = "https://github.com/rysweet/amplihack-recipe-runner";

    fn executable(path: &Path, script: &str) {
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn receipt(cargo: &Path, sha: &str) {
        std::fs::write(
            cargo.join(".crates2.json"),
            format!(
                r#"{{"installs":{{"recipe-runner-rs 0.5.0 (git+{URL}#{sha})":{{"bins":["recipe-runner-rs"]}}}}}}"#
            ),
        )
        .unwrap();
    }

    /// Runs `ensure_recipe_runner_with` with the runner fixture, a receipt
    /// for `installed`, a managed state record for `installed`, and a stub
    /// cargo. Returns the outcome and the cargo argument log.
    fn run(installed: &str, force: bool) -> (Result<Outcome>, String, fixture::Runner) {
        let dir = tempfile::tempdir().unwrap();
        let fix = fixture::build(dir.path());
        let installed = match installed {
            "A" => fix.a.clone(),
            "C" => fix.c.clone(),
            other => panic!("{other}"),
        };
        let home = dir.path().join("home");
        let cargo = home.join(".cargo");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(cargo.join("bin")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(home.join(".amplihack/state")).unwrap();
        executable(
            &cargo.join("bin/recipe-runner-rs"),
            "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\"]}'\n",
        );
        receipt(&cargo, &installed);
        std::fs::write(
            home.join(".amplihack/state/recipe_runner.json"),
            format!(r#"{{"installed_sha":"{installed}","checked_at":0}}"#),
        )
        .unwrap();
        let log = dir.path().join("cargo-args");
        executable(
            &bin.join("cargo"),
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{}'\n\
                 rev=''; prev=''\nfor a in \"$@\"; do [ \"$prev\" = --rev ] && rev=$a; prev=$a; done\n\
                 printf '{{\"installs\":{{\"recipe-runner-rs 0.5.0 (git+{URL}#%s)\":{{\"bins\":[\"recipe-runner-rs\"]}}}}}}' \"$rev\" > '{}'\n",
                log.display(),
                cargo.join(".crates2.json").display()
            ),
        );
        executable(&bin.join("cc"), "#!/bin/sh\nexit 0\n");
        let target = RunnerTarget {
            rev: fix.b.clone(),
            url: URL.to_string(),
            ancestry: AncestrySource::Fixed(fix.path.clone()),
        };
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let outcome = crate::test_support::with_env(
            &[
                ("PATH", Some(OsStr::new(&path))),
                ("HOME", Some(home.as_os_str())),
                ("CARGO_HOME", Some(cargo.as_os_str())),
                ("AMPLIHACK_AGENT_BINARY", Some(OsStr::new("claude"))),
                (FORCE_ENV, force.then_some(OsStr::new("1"))),
                ("RECIPE_RUNNER_RS_PATH", None),
                ("RECIPE_RUNNER_ANCESTRY_REPO", None),
                ("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", None),
                ("AMPLIHACK_NO_RUST_BOOTSTRAP", None),
            ],
            || ensure_recipe_runner_with(&target),
        );
        let args = std::fs::read_to_string(&log).unwrap_or_default();
        (outcome, args, fix)
    }

    #[test]
    fn a_newer_runner_is_kept_and_reported() {
        let (outcome, args, fix) = run("C", false);
        assert!(args.is_empty(), "cargo must not run: {args}");
        assert_eq!(
            outcome.expect("keeping a newer runner is not an error"),
            Outcome::KeptInstalled {
                installed: fix.c.clone(),
                pinned: fix.b.clone(),
                class: RunnerOrder::Newer,
            }
        );
    }

    #[test]
    fn an_older_runner_is_upgraded() {
        let (outcome, args, fix) = run("A", false);
        assert!(
            args.lines()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|p| p == ["--rev", fix.b.as_str()]),
            "{args}"
        );
        assert_eq!(outcome.unwrap(), Outcome::InstalledFromGit);
    }

    #[test]
    fn force_installs_the_pin_over_a_newer_runner() {
        let (outcome, args, fix) = run("C", true);
        assert!(
            args.lines()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|p| p == ["--rev", fix.b.as_str()]),
            "{args}"
        );
        assert_eq!(outcome.unwrap(), Outcome::InstalledFromGit);
    }

    #[test]
    fn kept_outcome_is_cloneable_for_callers() {
        let kept = Outcome::KeptInstalled {
            installed: "c".repeat(40),
            pinned: "b".repeat(40),
            class: RunnerOrder::Diverged,
        };
        assert_eq!(kept.clone(), kept);
    }
}
