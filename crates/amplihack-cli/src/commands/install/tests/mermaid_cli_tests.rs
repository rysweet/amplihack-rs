//! TDD red-phase tests for issue #828:
//! "Best-effort installation of the mermaid CLI (mmdc) during `amplihack install`".
//!
//! Contract under test: `install::mermaid_cli::ensure_mermaid_cli()`, driven
//! through `ensure_mermaid_cli_with` so each child's environment can be set
//! (issue #1765).
//!
//! The mermaid CLI (`@mermaid-js/mermaid-cli`, binary `mmdc`) lets the
//! `pr-guide` skill render mermaid diagrams to images locally for Azure
//! DevOps (where mermaid does not render in PR descriptions/comments),
//! avoiding the third-party `mermaid.ink` service. It pulls in puppeteer +
//! a headless Chromium download (hundreds of MB) and requires Node/npm, so
//! it is an *optional / best-effort* component.
//!
//! Per the install-completeness invariant in
//! `amplifier-bundle/context/PHILOSOPHY.md`, `amplihack install` must fail
//! loudly ONLY for REQUIRED components. mmdc is NOT required, therefore a
//! failed mmdc install must warn-and-continue — never abort the install.
//! This is encoded as: `ensure_mermaid_cli()` ALWAYS returns `Ok`, and the
//! `Outcome` it returns describes what happened.
//!
//! These are *failing* tests in the TDD red phase: they specify the contract
//! for the upcoming `install/mermaid_cli.rs` module and are expected to fail
//! to compile (the module does not exist yet) and then pass once the
//! implementation lands.
//!
//! Expected contract:
//!   pub(super) enum Outcome {
//!       AlreadyPresent,   // mmdc already on PATH -> skipped
//!       Installed,        // npm install -g succeeded and re-probe found mmdc
//!       SkippedByEnv,     // AMPLIHACK_SKIP_MMDC set (any non-empty value)
//!       SkippedNoNpm,     // npm absent -> skipped with an informative message
//!       Failed,           // npm install attempted but failed / mmdc still absent
//!   }
//!   pub(super) fn ensure_mermaid_cli() -> anyhow::Result<Outcome>;  // always Ok
//!   pub(super) fn ensure_mermaid_cli_with(
//!       configure: &dyn Fn(&mut Command),
//!   ) -> anyhow::Result<Outcome>;  // the same, with a hook on every child
//!
//! All tests are hermetic without changing the process environment (issue
//! #1765): every child the flow starts gets PATH set to a single controlled
//! bin directory and HOME set to a temporary directory, and does not inherit
//! `AMPLIHACK_CLAUDE_PROFILE_ROOT`. Neither a real `npm` nor a real `mmdc` on
//! the developer/CI host can be reached, and stubs use an absolute `/bin/sh`
//! shebang so they exec regardless of the pinned PATH. The only `npm install
//! -g` any test runs is the stub's. No test contacts the network.

use super::super::mermaid_cli::{Outcome, ensure_mermaid_cli_with};
use crate::commands::profile::ROOT_ENV;
use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::process::Command;

// ---------------------------------------------------------------------------
// Hermetic test scaffolding
// ---------------------------------------------------------------------------

/// Create an executable shell-script stub at `dir/name` with `body`.
///
/// Uses an absolute `#!/bin/sh` shebang on purpose: tests pin each child's
/// PATH to a single controlled directory, so a relative shebang interpreter
/// (e.g. `#!/usr/bin/env bash`) would fail to resolve `bash` via that PATH.
/// `/bin/sh` is exec'd by the kernel directly without a PATH lookup.
fn create_script_stub(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// An `mmdc` stub that succeeds for any args (so `mmdc --version` exits 0)
/// — i.e. a mermaid CLI that is "already installed".
fn stub_mmdc_present(bin_dir: &Path) {
    create_script_stub(bin_dir, "mmdc", "#!/bin/sh\nexit 0\n");
}

/// An `npm` stub that reports a version (exit 0 for `--version`) but FAILS
/// any `install` subcommand (exit 1). This lets us drive the failure path
/// (`npm` present, `mmdc` missing, `npm install -g ...` fails) hermetically
/// without touching the network.
fn stub_npm_present_install_fails(bin_dir: &Path) {
    create_script_stub(
        bin_dir,
        "npm",
        "#!/bin/sh\nif [ \"$1\" = \"install\" ]; then\n  echo 'npm ERR! simulated failure' 1>&2\n  exit 1\nfi\nexit 0\n",
    );
}

/// An `npm` stub that reports a version and whose `install` subcommand puts a
/// working `mmdc` stub into `bin_dir`, so the re-probe finds it.
fn stub_npm_install_provides_mmdc(bin_dir: &Path) {
    let mmdc = sh_quoted(&bin_dir.join("mmdc"));
    create_script_stub(
        bin_dir,
        "npm",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"install\" ]; then\n  printf '#!/bin/sh\\nexit 0\\n' > {mmdc}\n  /bin/chmod 755 {mmdc}\nfi\nexit 0\n"
        ),
    );
}

/// `path` in single quotes for a `/bin/sh` script.
fn sh_quoted(path: &Path) -> String {
    let text = path.to_str().expect("temporary paths are UTF-8");
    assert!(
        !text.contains(['\'', '\n']),
        "cannot single-quote {text:?} for /bin/sh"
    );
    format!("'{text}'")
}

/// The hook every test passes to [`ensure_mermaid_cli_with`]: each child
/// gets PATH set to `bin_dir` alone (so no real `npm`/`mmdc` on the host can
/// be reached), HOME set to `home`, and no `AMPLIHACK_CLAUDE_PROFILE_ROOT`.
fn hermetic<'a>(bin_dir: &'a Path, home: &'a Path) -> impl Fn(&mut Command) + 'a {
    move |cmd: &mut Command| {
        cmd.env("PATH", bin_dir)
            .env("HOME", home)
            .env_remove(ROOT_ENV);
    }
}

/// Run `f` with a fresh `bin/` directory and temporary home, and with
/// `AMPLIHACK_SKIP_MMDC` cleared (individual tests set it explicitly).
///
/// PATH and HOME are never changed in the process: tests pass them to each
/// child through [`hermetic`]. `AMPLIHACK_SKIP_MMDC` is read from the process
/// environment, so the lock is held for it and it is restored on exit.
fn with_mermaid_env<R>(f: impl FnOnce(&Path, &Path) -> R) -> R {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let temp = tempfile::tempdir().unwrap();
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let prev_skip = std::env::var_os("AMPLIHACK_SKIP_MMDC");
    unsafe {
        std::env::remove_var("AMPLIHACK_SKIP_MMDC");
    }

    let result = f(&bin_dir, temp.path());

    unsafe {
        match prev_skip {
            Some(v) => std::env::set_var("AMPLIHACK_SKIP_MMDC", v),
            None => std::env::remove_var("AMPLIHACK_SKIP_MMDC"),
        }
    }
    result
}

// ---------------------------------------------------------------------------
// (1) Opt-out env var short-circuits everything -> SkippedByEnv
// ---------------------------------------------------------------------------

#[test]
fn ensure_mermaid_cli_skips_when_opt_out_env_set() {
    // Requirement #4: an opt-out env var (AMPLIHACK_SKIP_MMDC) disables the
    // attempt entirely for minimal/offline environments. Truthiness is "any
    // non-empty value" (matches the recipe_runner var_os(..).map(!is_empty)
    // convention), so a bare "1" must trigger the skip.
    //
    // The skip must take precedence over every other probe: even if neither
    // mmdc nor npm is present (or both are), setting the env var yields
    // SkippedByEnv with no install attempt.
    with_mermaid_env(|bin_dir, home| {
        // SAFETY: env is serialized by with_mermaid_env's lock; restored below.
        unsafe {
            std::env::set_var("AMPLIHACK_SKIP_MMDC", "1");
        }

        let outcome = ensure_mermaid_cli_with(&hermetic(bin_dir, home))
            .expect("issue #828: ensure_mermaid_cli must never return Err (best-effort)");

        assert!(
            matches!(outcome, Outcome::SkippedByEnv),
            "issue #828: AMPLIHACK_SKIP_MMDC set must short-circuit to \
             Outcome::SkippedByEnv, got {outcome:?}"
        );

        unsafe {
            std::env::remove_var("AMPLIHACK_SKIP_MMDC");
        }
    });
}

#[test]
fn ensure_mermaid_cli_opt_out_is_any_nonempty_value() {
    // Requirement #4 (truthiness semantics): the opt-out is a presence flag,
    // not strictly "=1". Any non-empty value disables the attempt.
    with_mermaid_env(|bin_dir, home| {
        unsafe {
            std::env::set_var("AMPLIHACK_SKIP_MMDC", "yes");
        }

        let outcome = ensure_mermaid_cli_with(&hermetic(bin_dir, home))
            .expect("issue #828: must never return Err (best-effort)");

        assert!(
            matches!(outcome, Outcome::SkippedByEnv),
            "issue #828: AMPLIHACK_SKIP_MMDC=<any non-empty> must skip via \
             Outcome::SkippedByEnv, got {outcome:?}"
        );

        unsafe {
            std::env::remove_var("AMPLIHACK_SKIP_MMDC");
        }
    });
}

// ---------------------------------------------------------------------------
// (2) Already-installed mmdc is detected and skipped -> AlreadyPresent
// ---------------------------------------------------------------------------

#[test]
fn ensure_mermaid_cli_skips_when_mmdc_already_present() {
    // Requirement #2 (preflight): if mmdc is already installed (discoverable
    // on PATH / responds to `mmdc --version`), skip without invoking npm.
    //
    // We place an `mmdc` stub on PATH but deliberately place NO `npm` stub.
    // If the impl wrongly tried to install, it would have to find npm — and
    // it can't here — proving the present-check short-circuited before npm.
    with_mermaid_env(|bin_dir, home| {
        stub_mmdc_present(bin_dir);

        let outcome = ensure_mermaid_cli_with(&hermetic(bin_dir, home))
            .expect("issue #828: ensure_mermaid_cli must never return Err (best-effort)");

        assert!(
            matches!(outcome, Outcome::AlreadyPresent),
            "issue #828: an mmdc already on PATH must yield \
             Outcome::AlreadyPresent (skip install), got {outcome:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// (3) Missing npm is handled gracefully -> SkippedNoNpm (no error)
// ---------------------------------------------------------------------------

#[test]
fn ensure_mermaid_cli_skips_when_npm_absent() {
    // Requirement #2 + #1: when npm is unavailable, skip with an informative
    // message and DO NOT error. PATH is pinned to an empty bin dir, so
    // neither npm nor mmdc exists.
    with_mermaid_env(|bin_dir, home| {
        // No mmdc stub, no npm stub: both absent on the pinned PATH.
        let outcome = ensure_mermaid_cli_with(&hermetic(bin_dir, home))
            .expect("issue #828: a missing npm must be handled gracefully, not error");

        assert!(
            matches!(outcome, Outcome::SkippedNoNpm),
            "issue #828: npm absent must yield Outcome::SkippedNoNpm \
             (skip, no error), got {outcome:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// (4) Install failure does NOT propagate as an install error -> Failed (Ok)
// ---------------------------------------------------------------------------

#[test]
fn ensure_mermaid_cli_install_failure_does_not_propagate() {
    // Requirement #1 + #3 (CRITICAL): mmdc is optional. When npm is present
    // but `npm install -g @mermaid-js/mermaid-cli` fails (network/permission/
    // Chromium-download failures), ensure_mermaid_cli must capture the failure
    // and return Ok(Outcome::Failed) — it must NEVER return Err, because that
    // would propagate up and abort the overall `amplihack install`.
    //
    // We stub an npm that reports a version (probe sees it as present) but
    // fails any `install` subcommand, and we never create an `mmdc`, so the
    // post-install re-probe also fails -> Outcome::Failed.
    with_mermaid_env(|bin_dir, home| {
        stub_npm_present_install_fails(bin_dir);

        let result = ensure_mermaid_cli_with(&hermetic(bin_dir, home));

        assert!(
            result.is_ok(),
            "issue #828: a failed mmdc install must be captured and must NOT \
             propagate as an Err (best-effort / install-completeness invariant), \
             but got: {:?}",
            result.err()
        );

        let outcome = result.unwrap();
        assert!(
            matches!(outcome, Outcome::Failed),
            "issue #828: npm-present + install-fails + mmdc-still-absent must \
             yield Outcome::Failed, got {outcome:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// (5) Always-Ok guarantee across representative scenarios
// ---------------------------------------------------------------------------

#[test]
fn ensure_mermaid_cli_never_returns_err() {
    // The best-effort contract distilled: across every probe branch
    // (env-skip, already-present, npm-absent, install-failure),
    // ensure_mermaid_cli must always return Ok. This is the single most
    // important property — it is what keeps a missing/broken mmdc from ever
    // blocking `amplihack install`.

    // env-skip branch
    with_mermaid_env(|bin_dir, home| {
        unsafe {
            std::env::set_var("AMPLIHACK_SKIP_MMDC", "1");
        }
        assert!(
            ensure_mermaid_cli_with(&hermetic(bin_dir, home)).is_ok(),
            "issue #828: env-skip branch must return Ok"
        );
        unsafe {
            std::env::remove_var("AMPLIHACK_SKIP_MMDC");
        }
    });

    // already-present branch
    with_mermaid_env(|bin_dir, home| {
        stub_mmdc_present(bin_dir);
        assert!(
            ensure_mermaid_cli_with(&hermetic(bin_dir, home)).is_ok(),
            "issue #828: already-present branch must return Ok"
        );
    });

    // npm-absent branch
    with_mermaid_env(|bin_dir, home| {
        assert!(
            ensure_mermaid_cli_with(&hermetic(bin_dir, home)).is_ok(),
            "issue #828: npm-absent branch must return Ok"
        );
    });

    // install-failure branch
    with_mermaid_env(|bin_dir, home| {
        stub_npm_present_install_fails(bin_dir);
        assert!(
            ensure_mermaid_cli_with(&hermetic(bin_dir, home)).is_ok(),
            "issue #828: install-failure branch must return Ok"
        );
    });
}

// ---------------------------------------------------------------------------
// (6) The hook reaches every child the flow starts (issue #1765)
// ---------------------------------------------------------------------------

#[test]
fn ensure_mermaid_cli_with_hooks_every_child_through_install_and_reprobe() {
    // The flow starts four children on the install path: the `mmdc` probe,
    // the `npm` probe, `npm install -g` and the re-probe. Each must get the
    // hook, or it would run with the process PATH and HOME. `Installed` can
    // only come back if the stub `npm` (reachable through the hook's PATH
    // alone) ran the install and the re-probe found the `mmdc` it wrote.
    with_mermaid_env(|bin_dir, home| {
        stub_npm_install_provides_mmdc(bin_dir);
        let isolate = hermetic(bin_dir, home);
        let hooked = Cell::new(0);

        let outcome = ensure_mermaid_cli_with(&|cmd: &mut Command| {
            hooked.set(hooked.get() + 1);
            isolate(cmd);
        })
        .expect("issue #828: ensure_mermaid_cli must never return Err (best-effort)");

        assert!(
            matches!(outcome, Outcome::Installed),
            "issue #1765: an install through the hooked stub npm that provides \
             mmdc must yield Outcome::Installed, got {outcome:?}"
        );
        assert_eq!(
            hooked.get(),
            4,
            "issue #1765: the hook must be applied to the mmdc probe, the npm \
             probe, npm install -g and the re-probe"
        );
    });
}
