//! Regression tests for issue #527:
//! "Fresh install verifier reports missing CLAUDE.md and recipe-runner-rs"
//!
//! Two concrete bugs covered here:
//!
//!   BUG 1: `amplihack install` did not stage CLAUDE.md when the source
//!          repo uses the bundle layout and ships CLAUDE.md inside
//!          `amplifier-bundle/CLAUDE.md`. The verifier looks at
//!          `$AMPLIHACK_HOME/CLAUDE.md` and reported it missing.
//!
//!   BUG 2: The "🦀 Ensuring Rust recipe runner" install phase printed an
//!          ❌ status when `recipe-runner-rs` was absent from PATH but
//!          STILL returned `Ok(())`. Per the install-completeness
//!          invariant in `amplifier-bundle/context/PHILOSOPHY.md`, install
//!          must fail loudly when a required component cannot be placed.
//!
//! Each test here is a *failing* test in the TDD red phase: it specifies the
//! contract for the upcoming implementation and is expected to fail against
//! the current `main` (or this branch's pre-fix tip).

use super::helpers;
use super::*;
use std::fs;
use std::path::Path;

// ---------------------------------------------------------------------------
// Test fixture helpers (scoped to this module — do not pollute helpers.rs
// until the implementation lands).
// ---------------------------------------------------------------------------

/// Build a bundle-only source repo for issue #527 where CLAUDE.md lives
/// **only** inside `amplifier-bundle/CLAUDE.md` (no legacy `<repo>/CLAUDE.md`).
///
/// This mirrors the post-fix bundle layout: the canonical CLAUDE.md ships
/// inside the bundle, not at the repo root.
fn create_bundle_only_repo_with_bundle_claude_md(root: &Path) {
    helpers::create_bundle_only_source_repo(root);
    // create_bundle_only_source_repo wrote `<repo>/CLAUDE.md`. Remove it so
    // the only available source is the bundle path.
    let legacy = root.join("CLAUDE.md");
    if legacy.exists() {
        fs::remove_file(&legacy).unwrap();
    }
    // Seed the canonical bundle CLAUDE.md.
    fs::write(
        root.join("amplifier-bundle/CLAUDE.md"),
        "# Amplihack\n\nBundle CLAUDE.md fixture for issue #527.\n",
    )
    .unwrap();
}

/// Build a bundle-only source repo for issue #527 where CLAUDE.md is missing
/// from BOTH the bundle and the legacy parent location. Install must hard
/// error in this state — silently skipping causes the verifier to report
/// CLAUDE.md missing post-install.
fn create_bundle_repo_with_no_claude_md_anywhere(root: &Path) {
    helpers::create_bundle_only_source_repo(root);
    let legacy = root.join("CLAUDE.md");
    if legacy.exists() {
        fs::remove_file(&legacy).unwrap();
    }
    // Do NOT write amplifier-bundle/CLAUDE.md — both locations empty.
    assert!(!root.join("CLAUDE.md").exists());
    assert!(!root.join("amplifier-bundle/CLAUDE.md").exists());
}

/// Stage a `recipe-runner-rs` executable stub inside `bin_dir` so that
/// `find_binary("recipe-runner-rs")` returns Some(...) without going to the
/// network. The stub never runs — its mere presence on PATH must satisfy
/// the install probe.
fn stage_recipe_runner_stub(bin_dir: &Path) -> std::path::PathBuf {
    helpers::create_exe_stub(bin_dir, "recipe-runner-rs")
}

/// Run `f` with HOME pointed at a tempdir AND with PATH containing
/// `extra_path_dir` *first* so any stubs placed there shadow real binaries.
/// Restores HOME and PATH on exit, even on panic.
///
/// Why this duplicates `with_install_env` in install_flow.rs: that helper is
/// `fn`-private to that module and bundles its own python3 + amplihack-hooks
/// stubs. We need the same isolation but with the additional ability to
/// place a `recipe-runner-rs` stub on PATH for some tests and explicitly
/// omit it for others. We also always set `AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL=1`
/// so the cargo-install fallback never reaches the network during tests.
fn with_install_env<R>(f: impl FnOnce(&Path, &Path) -> R) -> R {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous_home = crate::test_support::set_home(temp.path());

    let bin_dir = temp.path().join("stub_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    helpers::create_exe_stub(&bin_dir, "python3");
    let hooks_stub = helpers::create_exe_stub(&bin_dir, "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    let prev_skip = std::env::var_os("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL");
    let prev_rr_path = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    // Locate /usr/bin/sh + /usr/bin (cargo-install needs basic tools); we
    // include them but deliberately exclude any directory that may already
    // contain a real recipe-runner-rs (~/.cargo/bin, ~/.local/bin) so the
    // BUG 2 bail test is reliably hermetic on developer machines.
    let safe_system_dirs = ["/usr/local/bin", "/usr/bin", "/bin"];
    let new_path = {
        let mut entries = vec![bin_dir.display().to_string()];
        entries.extend(safe_system_dirs.iter().map(|s| (*s).to_string()));
        entries.join(":")
    };
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        std::env::set_var("PATH", &new_path);
        // Tests must never touch the network. The env hatch short-circuits
        // the cargo-install branch; the present-check still runs.
        std::env::set_var("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");
        // Make sure no leaked RECIPE_RUNNER_RS_PATH from a prior test
        // accidentally satisfies the present-check.
        std::env::remove_var("RECIPE_RUNNER_RS_PATH");
    }

    let result = f(temp.path(), &bin_dir);

    unsafe {
        if let Some(v) = prev_hooks {
            std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v);
        } else {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        }
        if let Some(v) = prev_path {
            std::env::set_var("PATH", v);
        }
        if let Some(v) = prev_skip {
            std::env::set_var("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", v);
        } else {
            std::env::remove_var("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL");
        }
        if let Some(v) = prev_rr_path {
            std::env::set_var("RECIPE_RUNNER_RS_PATH", v);
        }
    }
    crate::test_support::restore_home(previous_home);
    result
}

// ---------------------------------------------------------------------------
// BUG 1: CLAUDE.md staging
// ---------------------------------------------------------------------------

#[test]
fn install_stages_claude_md_from_bundle_path() {
    // Issue #527 / BUG 1 — RED phase.
    //
    // Pre-fix: `directories.rs` only checks `source_root.parent()/CLAUDE.md`
    // (i.e., `<repo>/CLAUDE.md`). When the source repo ships CLAUDE.md inside
    // the bundle (`<repo>/amplifier-bundle/CLAUDE.md`) and not at the repo
    // root, install silently skips the copy and the verifier reports
    // `$AMPLIHACK_HOME/CLAUDE.md` missing.
    //
    // Post-fix: install must look at `source_root/CLAUDE.md` first (bundle
    // path), fall back to the legacy parent only if the bundle copy is
    // absent, and stage whichever was found to `$AMPLIHACK_HOME/CLAUDE.md`.
    with_install_env(|home, bin_dir| {
        // Recipe-runner stub on PATH so BUG 2 doesn't mask BUG 1.
        stage_recipe_runner_stub(bin_dir);

        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        create_bundle_only_repo_with_bundle_claude_md(&repo);

        local_install(&repo, None).expect(
            "issue #527 BUG 1: install must succeed when CLAUDE.md is only \
             present at amplifier-bundle/CLAUDE.md",
        );

        let staged_claude_md = home.join(".amplihack/CLAUDE.md");
        assert!(
            staged_claude_md.is_file(),
            "issue #527 BUG 1: $AMPLIHACK_HOME/CLAUDE.md must exist after install \
             (verifier checks this exact path); expected file at {}",
            staged_claude_md.display()
        );

        // Content must round-trip from the bundle source, not be an empty
        // placeholder. This guards against a future "touch the file" patch
        // that would silence the verifier without actually staging content.
        let staged = fs::read_to_string(&staged_claude_md).unwrap();
        assert!(
            staged.contains("Bundle CLAUDE.md fixture for issue #527"),
            "issue #527 BUG 1: staged CLAUDE.md must contain the bundle source \
             content; got: {staged:?}"
        );
    });
}

#[test]
fn install_bails_when_claude_md_absent_from_all_known_locations() {
    // Issue #527 / BUG 1 — install-completeness invariant.
    //
    // Pre-fix: when CLAUDE.md is missing at every candidate location, the
    // copy block is silently skipped and install returns Ok(()). The
    // verifier then prints ❌ for CLAUDE.md, contradicting install's
    // success. Per amplifier-bundle/context/PHILOSOPHY.md (Install
    // Completeness Invariant from PR #526), install must fail loudly.
    //
    // Post-fix: install must return Err with a remediation message
    // mentioning CLAUDE.md and the candidate paths it tried.
    with_install_env(|home, bin_dir| {
        stage_recipe_runner_stub(bin_dir);

        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        create_bundle_repo_with_no_claude_md_anywhere(&repo);

        let result = local_install(&repo, None);
        assert!(
            result.is_err(),
            "issue #527 BUG 1: install must hard-error when CLAUDE.md is \
             missing from both bundle and legacy paths, but returned Ok(())"
        );
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.to_ascii_lowercase().contains("claude.md"),
            "issue #527 BUG 1: error must mention CLAUDE.md so users know what \
             to remediate; got: {err}"
        );

        // The staged CLAUDE.md must NOT exist (no silent ghost file).
        assert!(
            !home.join(".amplihack/CLAUDE.md").exists(),
            "issue #527 BUG 1: install must not leave a half-staged or empty \
             CLAUDE.md when the source is missing"
        );
    });
}

// ---------------------------------------------------------------------------
// BUG 2: recipe-runner-rs deployment
// ---------------------------------------------------------------------------

#[test]
fn install_succeeds_when_recipe_runner_stub_is_on_path() {
    // Issue #527 / BUG 2 — happy path.
    //
    // When `recipe-runner-rs` is already on PATH, the recipe-runner phase
    // must short-circuit (no cargo install) and install must succeed.
    // This is the contract that lets CI / hermetic test environments
    // satisfy install without network access.
    with_install_env(|home, bin_dir| {
        stage_recipe_runner_stub(bin_dir);

        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        create_bundle_only_repo_with_bundle_claude_md(&repo);

        local_install(&repo, None).expect(
            "issue #527 BUG 2: install must succeed when recipe-runner-rs is \
             already on PATH (no cargo install required)",
        );

        // Sanity: the stub is still reachable via PATH after install — i.e.,
        // install did not modify PATH in a way that hides it.
        assert!(
            paths::find_binary("recipe-runner-rs").is_some(),
            "issue #527 BUG 2: recipe-runner-rs must remain discoverable on \
             PATH after install"
        );
    });
}

#[test]
fn install_bails_when_recipe_runner_missing_and_install_skipped() {
    // Issue #527 / BUG 2 — install-completeness invariant.
    //
    // Pre-fix: the recipe-runner phase prints "❌ recipe-runner-rs not
    // installed" and continues, so install returns Ok(()) while a required
    // component is absent. This is the exact bug the verifier catches.
    //
    // Post-fix: when (a) the binary is not on PATH AND (b) the cargo-install
    // fallback is disabled (env hatch set, as in this test) AND (c) re-probe
    // still fails, install must return Err with a remediation message.
    //
    // The env hatch (`AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL=1`) is set
    // unconditionally by `with_install_env`. We deliberately do NOT stage a
    // recipe-runner-rs stub on PATH for this test, so the bail path is
    // exercised even though we never touch the network.
    with_install_env(|home, _bin_dir| {
        // NOTE: no stage_recipe_runner_stub() call.

        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        create_bundle_only_repo_with_bundle_claude_md(&repo);

        // Defensive: confirm the binary really is absent in this isolated
        // environment, otherwise the assertion below would be vacuous.
        assert!(
            paths::find_binary("recipe-runner-rs").is_none(),
            "test precondition: recipe-runner-rs must not be on PATH in the \
             tempdir-only stub_bin (other PATH entries may shadow this if a \
             system-wide install exists; in that case this test is a no-op)"
        );

        let result = local_install(&repo, None);
        assert!(
            result.is_err(),
            "issue #527 BUG 2: install must hard-error when recipe-runner-rs \
             is absent and the install fallback is skipped, but returned Ok(())"
        );
        let err = format!("{:#}", result.unwrap_err());
        let lower = err.to_ascii_lowercase();
        assert!(
            lower.contains("recipe-runner-rs") || lower.contains("recipe runner"),
            "issue #527 BUG 2: error must mention recipe-runner-rs so users \
             know what to remediate; got: {err}"
        );
        assert!(
            lower.contains("cargo install") || lower.contains("path"),
            "issue #527 BUG 2: error must include actionable remediation \
             (cargo install command or PATH guidance); got: {err}"
        );
    });
}
