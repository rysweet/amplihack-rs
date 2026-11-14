//! Issue #1955, end to end through `run_install_with_fetcher`: the real install
//! path with an injected fetcher and an isolated HOME. No test here reaches the
//! network: see `InstallEnv`.

use super::super::framework_source::{BinaryIdentity, bundle_kept_without_source};
use super::super::run_install_with_fetcher;
use super::super::tag_fetch::{
    EXPLICIT_FETCH_BUDGET, FetchedWrongVersion, IMPLICIT_FETCH_BUDGET, SourceFetchFailed,
};
use super::super::{InstallMode, claude_commands, clone::FrameworkRootQuery, version_stamp};
use super::issue_1526_implicit_downgrade::{changes, snapshot};
use super::issue_1955_support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn release_binary() -> BinaryIdentity {
    BinaryIdentity {
        version: crate::VERSION.to_string(),
        may_fetch: true,
    }
}

fn install(
    local: Option<PathBuf>,
    force_refresh: bool,
    mode: InstallMode,
    binary: &BinaryIdentity,
    fetcher: &FakeFetcher,
) -> anyhow::Result<()> {
    let version = local
        .as_ref()
        .map(|path| super::super::framework_source::read_source_version(path));
    let version = match version {
        Some(super::super::framework_source::SourceVersion::Known(version)) => version.to_string(),
        _ => binary.version.clone(),
    };
    crate::test_support::release_fixture::with_release(&version, || {
        run_install_with_fetcher(local, false, force_refresh, false, mode, binary, fetcher)
    })
}

fn explicit(force_refresh: bool, fetcher: &FakeFetcher) -> anyhow::Result<()> {
    install(
        None,
        force_refresh,
        InstallMode::Explicit,
        &release_binary(),
        fetcher,
    )
}

/// A bundle file only `tree` has, so a test can tell which tree was staged.
fn mark_tree(tree: &Path, sentinel: &str) {
    fs::write(tree.join("amplifier-bundle/context/sentinel.txt"), sentinel).unwrap();
}

fn stamp() -> Option<String> {
    version_stamp::read_installed_version_quiet().unwrap()
}

// ── The reported bug: `update` runs `install --force-refresh` ──

#[test]
fn force_refresh_with_a_same_version_amplihack_home_stages_from_it_without_a_fetch() {
    let mut fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("tag-worktree"), crate::VERSION);
    mark_tree(&tree, "from-tag-worktree");
    fx.env.set("AMPLIHACK_HOME", &tree);
    let fetcher = FakeFetcher::failing("must not be called");

    explicit(true, &fetcher).unwrap_or_else(|e| panic!("{e:#}"));

    assert_eq!(fetcher.call_count(), 0);
    assert_eq!(
        fs::read_to_string(fx.staged_file("context/sentinel.txt")).unwrap(),
        "from-tag-worktree"
    );
    assert_eq!(stamp().as_deref(), Some(crate::VERSION));
}

#[test]
fn the_false_message_and_the_main_download_are_gone_from_the_source() {
    // This text was printed even with AMPLIHACK_HOME at a valid tag worktree. An
    // in-process test can't capture stdout, so `tests/issue_1955_install_cli.rs`
    // checks the printed output and this checks the wording is gone.
    let install_mod = include_str!("../mod.rs");
    let clone = include_str!("../clone.rs");
    let types = include_str!("../types.rs");
    for (name, source) in [("mod.rs", install_mod), ("clone.rs", clone)] {
        assert!(
            !source.contains("Bundled framework source not found"),
            "{name} still prints the false message"
        );
    }
    assert!(
        !install_mod.contains("Forcing fresh framework download from upstream"),
        "mod.rs still promises a download from upstream"
    );
    assert!(
        !types.contains("refs/heads/main"),
        "the install must never fetch main"
    );
    assert!(
        !clone.contains("\"--depth\",\n            \"1\",\n            REPO_GIT_URL"),
        "clone.rs still clones the default branch"
    );
}

#[test]
fn force_refresh_with_an_older_amplihack_home_stages_the_release_tag() {
    let mut fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("old"), &older_than_binary());
    mark_tree(&tree, "from-old-tree");
    fx.env.set("AMPLIHACK_HOME", &tree);
    let fetcher = FakeFetcher::serving(crate::VERSION);

    explicit(true, &fetcher).unwrap_or_else(|e| panic!("{e:#}"));

    assert!(
        !fx.staged_file("context/sentinel.txt").exists(),
        "the older tree must not be staged"
    );
    assert_eq!(
        fs::read_to_string(fx.staged_file("context/fetched-sentinel.txt")).unwrap(),
        crate::VERSION
    );
    assert_eq!(stamp().as_deref(), Some(crate::VERSION));
}

#[test]
fn the_fetch_asks_for_the_binarys_exact_version_with_the_explicit_budget() {
    let _fx = InstallEnv::new();
    let fetcher = FakeFetcher::serving(crate::VERSION);

    explicit(true, &fetcher).unwrap_or_else(|e| panic!("{e:#}"));

    let calls = fetcher.calls.borrow();
    assert_eq!(
        calls.len(),
        1,
        "exactly one fetch, and no older tree after it"
    );
    assert_eq!(calls[0].0.to_string(), crate::VERSION);
    assert_eq!(calls[0].1, EXPLICIT_FETCH_BUDGET);
}

#[test]
fn the_budgets_are_the_agreed_ones() {
    // The update's child install has 300 s, so a longer fetch would be killed.
    assert_eq!(EXPLICIT_FETCH_BUDGET, Duration::from_secs(180));
    assert_eq!(IMPLICIT_FETCH_BUDGET, Duration::from_secs(60));
}

#[test]
fn an_implicit_install_fetches_with_the_shorter_budget() {
    let fx = InstallEnv::new();
    // Forced, because the compile-time root would match this test build.
    let fetcher = FakeFetcher::serving(crate::VERSION);

    install(
        None,
        true,
        InstallMode::Implicit,
        &release_binary(),
        &fetcher,
    )
    .unwrap_or_else(|e| panic!("{e:#}"));

    let calls = fetcher.calls.borrow();
    assert_eq!(calls.len(), 1, "{:?}", fx.home);
    assert_eq!(calls[0].1, IMPLICIT_FETCH_BUDGET);
}

// ── Plain install refuses, and changes nothing ──

/// Runs `action`, expects a refusal, and checks nothing under HOME changed.
fn assert_refused_with_no_change(fx: &InstallEnv, action: impl FnOnce() -> anyhow::Result<()>) {
    let before = snapshot(&fx.home);

    let err = action().expect_err("the older source must be refused");

    assert!(
        err.downcast_ref::<super::super::framework_source::SourceRefused>()
            .is_some(),
        "expected a refusal, got: {err:#}"
    );
    let after = snapshot(&fx.home);
    assert!(
        before == after,
        "a refusal happens before binaries are deployed, so nothing may change:\n{}",
        changes(&before, &after).join("\n")
    );
}

#[test]
fn install_with_an_older_amplihack_home_is_refused_before_anything_changes() {
    let mut fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("old"), &older_than_binary());
    fx.env.set("AMPLIHACK_HOME", &tree);
    let fetcher = FakeFetcher::serving(crate::VERSION);

    assert_refused_with_no_change(&fx, || explicit(false, &fetcher));
    assert_eq!(fetcher.call_count(), 0);
}

#[test]
fn install_from_an_older_current_directory_is_refused_before_anything_changes() {
    let fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("old-checkout"), &older_than_binary());
    mark_repository(&tree);
    let _cwd = crate::test_support::CwdGuard::set(&tree).unwrap();
    let fetcher = FakeFetcher::serving(crate::VERSION);

    assert_refused_with_no_change(&fx, || explicit(false, &fetcher));
}

#[test]
fn install_with_a_newer_amplihack_home_is_refused_before_anything_changes() {
    let mut fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("new"), &newer_than_binary());
    fx.env.set("AMPLIHACK_HOME", &tree);
    let fetcher = FakeFetcher::serving(crate::VERSION);

    assert_refused_with_no_change(&fx, || explicit(false, &fetcher));
}

#[test]
fn a_refusal_comes_before_the_interactive_wizard() {
    // Asked for `--interactive`, a refusal must come before any question.
    let mut fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("old"), &older_than_binary());
    fx.env.set("AMPLIHACK_HOME", &tree);
    fx.env.set("AMPLIHACK_NONINTERACTIVE", "1");
    let fetcher = FakeFetcher::serving(crate::VERSION);
    let binary = release_binary();

    assert_refused_with_no_change(&fx, || {
        run_install_with_fetcher(
            None,
            true,
            false,
            false,
            InstallMode::Explicit,
            &binary,
            &fetcher,
        )
    });
}

// ── --local ──

#[test]
fn local_stages_an_older_release_and_stamps_the_selected_version() {
    let fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("old"), &older_than_binary());
    mark_tree(&tree, "chosen");
    let fetcher = FakeFetcher::failing("must not be called");

    install(
        Some(tree),
        false,
        InstallMode::Explicit,
        &release_binary(),
        &fetcher,
    )
    .unwrap_or_else(|e| panic!("{e:#}"));

    assert_eq!(fetcher.call_count(), 0);
    assert_eq!(
        fs::read_to_string(fx.staged_file("context/sentinel.txt")).unwrap(),
        "chosen"
    );
    // The stamp describes the selected release, including a deliberate downgrade.
    assert_eq!(stamp().as_deref(), Some(older_than_binary().as_str()));
}

#[test]
fn local_wins_over_force_refresh() {
    let fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("chosen"), &older_than_binary());
    mark_tree(&tree, "chosen");
    let fetcher = FakeFetcher::failing("must not be called");

    install(
        Some(tree),
        true,
        InstallMode::Explicit,
        &release_binary(),
        &fetcher,
    )
    .unwrap_or_else(|e| panic!("{e:#}"));

    assert_eq!(fetcher.call_count(), 0);
    assert_eq!(
        fs::read_to_string(fx.staged_file("context/sentinel.txt")).unwrap(),
        "chosen"
    );
}

// ── A failed fetch changes nothing ──

/// Runs `action`, expects `check` to accept the error, and that the installed
/// bundle and stamp are untouched.
fn assert_failure_keeps_install(
    fx: &InstallEnv,
    action: impl FnOnce() -> anyhow::Result<()>,
    check: impl FnOnce(&anyhow::Error),
) {
    let bundle = fx.staged_bundle();
    let stamp_path = fx.home.join(".amplihack/.installed-version");
    let bundle_before = snapshot(&bundle);
    let stamp_before = fs::read(&stamp_path).unwrap();

    let err = action().expect_err("the install must exit non-zero");

    check(&err);
    assert_eq!(
        snapshot(&bundle),
        bundle_before,
        "the installed bundle must not change"
    );
    assert_eq!(
        fs::read(&stamp_path).unwrap(),
        stamp_before,
        "the stamp must not advance"
    );
}

#[test]
fn a_failed_fetch_exits_non_zero_and_keeps_the_bundle_and_stamp() {
    let fx = InstallEnv::new();
    staged_install(&fx.home, Some(&older_than_binary()));
    let fetcher = FakeFetcher::failing("network request timed out");

    assert_failure_keeps_install(
        &fx,
        || explicit(true, &fetcher),
        |err| {
            assert!(
                matches!(
                    err.downcast_ref::<SourceFetchFailed>(),
                    Some(SourceFetchFailed::Failed { .. })
                ),
                "{err:#}"
            );
        },
    );
}

#[test]
fn the_failed_fetch_error_says_what_was_kept_and_how_to_recover() {
    let err = SourceFetchFailed::Failed {
        version: "1.2.99".to_string(),
        reason: "network request timed out".to_string(),
    };

    assert_eq!(
        err.to_string(),
        "could not fetch the framework bundle from release tag v1.2.99: network request timed \
         out\nThe installed bundle and version stamp were not changed.\nRun `amplihack install \
         --local <PATH>` to install from a checkout."
    );
}

#[test]
fn a_build_that_may_not_fetch_fails_without_calling_the_fetcher() {
    let fx = InstallEnv::new();
    staged_install(&fx.home, Some(&older_than_binary()));
    let fetcher = FakeFetcher::serving(crate::VERSION);
    let binary = BinaryIdentity {
        version: crate::VERSION.to_string(),
        may_fetch: false,
    };

    assert_failure_keeps_install(
        &fx,
        || install(None, true, InstallMode::Explicit, &binary, &fetcher),
        |err| {
            assert!(
                matches!(
                    err.downcast_ref::<SourceFetchFailed>(),
                    Some(SourceFetchFailed::NotAllowed { .. })
                ),
                "{err:#}"
            );
        },
    );
    assert_eq!(fetcher.call_count(), 0);
}

#[test]
fn a_fetched_tree_with_the_wrong_version_fails_and_no_older_tree_is_tried() {
    let fx = InstallEnv::new();
    staged_install(&fx.home, Some(&older_than_binary()));
    let fetcher = FakeFetcher::serving(&older_than_binary());

    assert_failure_keeps_install(
        &fx,
        || explicit(true, &fetcher),
        |err| {
            assert!(
                err.downcast_ref::<FetchedWrongVersion>().is_some(),
                "{err:#}"
            );
            assert!(
                err.downcast_ref::<SourceFetchFailed>().is_none(),
                "self-heal must not be able to continue past this: {err:#}"
            );
        },
    );
    assert_eq!(fetcher.call_count(), 1);
}

#[test]
fn a_fetched_tree_with_no_version_fails_the_same_way() {
    let fx = InstallEnv::new();
    staged_install(&fx.home, Some(&older_than_binary()));
    let fetcher = FakeFetcher::new(|destination| {
        let root = destination.join("tree");
        unversioned_tree(&root);
        Ok(root)
    });

    assert_failure_keeps_install(
        &fx,
        || explicit(true, &fetcher),
        |err| {
            assert!(
                err.downcast_ref::<FetchedWrongVersion>().is_some(),
                "{err:#}"
            )
        },
    );
}

#[test]
fn the_fetched_tree_outlives_the_decision_and_is_removed_afterwards() {
    // SEC-13: it has to live until the install returns, in a private temp dir.
    let _fx = InstallEnv::new();
    let seen = std::rc::Rc::new(std::cell::RefCell::new(None::<PathBuf>));
    let record = std::rc::Rc::clone(&seen);
    let fetcher = FakeFetcher::new(move |destination| {
        *record.borrow_mut() = Some(destination.to_path_buf());
        let root = destination.join("tree");
        versioned_tree(&root, crate::VERSION);
        Ok(root)
    });

    explicit(true, &fetcher).unwrap_or_else(|e| panic!("{e:#}"));

    let destination = seen.borrow().clone().expect("the fetcher ran");
    assert!(!destination.exists(), "the fetched tree must be cleaned up");
}

// ── The launch bootstrap's other callers (D9) ──

#[test]
fn slash_commands_are_not_restored_from_an_older_source() {
    let mut fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("old"), "1.2.0");
    fs::create_dir_all(tree.join(".claude/commands/amplihack")).unwrap();
    fs::write(tree.join(".claude/commands/amplihack/x.md"), "x\n").unwrap();
    fx.env.set("AMPLIHACK_HOME", &tree);
    let target = claude_commands::claude_commands_dir().unwrap();
    let query = FrameworkRootQuery {
        amplihack_home: Some(tree),
        ..FrameworkRootQuery::default()
    };

    claude_commands::ensure_claude_commands_staged_for(&query, "1.2.99").unwrap();

    assert!(
        !target.exists(),
        "with no same-version source the commands stay missing, and nothing is fetched"
    );
}

#[test]
fn slash_commands_are_restored_from_a_same_version_source() {
    let fx = InstallEnv::new();
    let tree = versioned_tree(&fx.source_path("same"), "1.2.99");
    fs::create_dir_all(tree.join(".claude/commands/amplihack")).unwrap();
    fs::write(tree.join(".claude/commands/amplihack/x.md"), "x\n").unwrap();
    let target = claude_commands::claude_commands_dir().unwrap();
    let query = FrameworkRootQuery {
        amplihack_home: Some(tree),
        ..FrameworkRootQuery::default()
    };

    claude_commands::ensure_claude_commands_staged_for(&query, "1.2.99").unwrap();

    assert!(target.join("x.md").is_file());
}

#[test]
fn codex_reconcile_follows_the_stamp_when_no_source_is_found() {
    let binary = "1.2.99";
    // A bundle kept under D1: no source, and a stamp that says so.
    assert!(bundle_kept_without_source(Some("1.2.0"), binary));
    assert!(bundle_kept_without_source(None, binary));
    // The stamp says this install is current, so a missing source is a real fault.
    assert!(!bundle_kept_without_source(Some(binary), binary));
}
