//! Issue #1955: which source an install stages from, and how a source's
//! version is read. The table these follow is the "Source rules" section of
//! `docs/reference/framework-source-selection.md`.
//!
//! The decision is pure: `acquire_framework_source` takes every input as a
//! parameter, so these tests build directory layouts and never read the real
//! `HOME`, `AMPLIHACK_HOME` or working directory.

use super::super::InstallMode;
use super::super::clone::{FrameworkRootOrigin, FrameworkRootQuery};
use super::super::framework_source::*;
use super::super::tag_fetch::SourceFetchFailed;
use super::issue_1955_support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BINARY: &str = "1.2.99";
const OLDER: &str = "1.2.0";
const NEWER: &str = "1.3.0";

fn binary() -> BinaryIdentity {
    BinaryIdentity {
        version: BINARY.to_string(),
        may_fetch: true,
    }
}

fn policy(mode: InstallMode) -> SourcePolicy {
    SourcePolicy {
        local: None,
        force_refresh: false,
        mode,
        fetch_backoff: None,
    }
}

fn explicit() -> SourcePolicy {
    policy(InstallMode::Explicit)
}

fn implicit() -> SourcePolicy {
    policy(InstallMode::Implicit)
}

fn force_refresh() -> SourcePolicy {
    SourcePolicy {
        force_refresh: true,
        ..explicit()
    }
}

fn local(path: &Path) -> SourcePolicy {
    SourcePolicy {
        local: Some(path.to_path_buf()),
        ..explicit()
    }
}

fn home_query(path: &Path) -> FrameworkRootQuery {
    FrameworkRootQuery {
        amplihack_home: Some(path.to_path_buf()),
        ..FrameworkRootQuery::default()
    }
}

fn cwd_query(path: &Path) -> FrameworkRootQuery {
    FrameworkRootQuery {
        current_dir: Some(path.to_path_buf()),
        ..FrameworkRootQuery::default()
    }
}

fn acquire(
    policy: &SourcePolicy,
    query: &FrameworkRootQuery,
    fetcher: &FakeFetcher,
) -> anyhow::Result<AcquiredSource> {
    acquire_framework_source(policy, query, &binary(), fetcher, &accept_all)
}

/// The notes `acquire` prints for this policy and query, as text.
fn notes(policy: &SourcePolicy, query: &FrameworkRootQuery) -> Vec<String> {
    resolve_for_policy(policy, query, BINARY, &accept_all)
        .notes
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn expect_refused(result: anyhow::Result<AcquiredSource>) -> SourceRefused {
    let err = result.expect_err("the source must be refused");
    err.downcast_ref::<SourceRefused>()
        .unwrap_or_else(|| panic!("expected SourceRefused, got: {err:#}"))
        .clone()
}

fn known(version: &str) -> SourceVersion {
    SourceVersion::Known(semver::Version::parse(version).unwrap())
}

// ── Plain install ──

#[test]
fn amplihack_home_at_the_same_version_is_staged_without_a_fetch() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("src"), BINARY);
    let fetcher = FakeFetcher::failing("must not be called");

    let source = acquire(&explicit(), &home_query(&tree), &fetcher).unwrap();

    assert_eq!(source.root, tree);
    assert_eq!(
        source.origin,
        SourceOrigin::Resolved(FrameworkRootOrigin::AmplihackHome)
    );
    assert_eq!(source.version, known(BINARY));
    assert_eq!(fetcher.call_count(), 0, "a matching source needs no fetch");
}

#[test]
fn older_amplihack_home_is_refused_not_staged_silently() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("src"), OLDER);
    let fetcher = FakeFetcher::serving(BINARY);

    let refused = expect_refused(acquire(&explicit(), &home_query(&tree), &fetcher));

    assert_eq!(refused.path, tree);
    assert_eq!(refused.source_version, known(OLDER));
    assert_eq!(refused.binary_version, BINARY);
    assert_eq!(fetcher.call_count(), 0, "a refusal must not fall through");
}

#[test]
fn newer_amplihack_home_is_refused() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("src"), NEWER);

    let refused = expect_refused(acquire(
        &explicit(),
        &home_query(&tree),
        &FakeFetcher::serving(BINARY),
    ));

    assert_eq!(refused.source_version, known(NEWER));
}

#[test]
fn older_current_directory_is_refused() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("checkout"), OLDER);
    mark_repository(&tree);
    let nested = tree.join("crates/amplihack-cli");
    fs::create_dir_all(&nested).unwrap();

    let refused = expect_refused(acquire(
        &explicit(),
        &cwd_query(&nested),
        &FakeFetcher::serving(BINARY),
    ));

    assert_eq!(refused.path, tree);
}

#[test]
fn a_source_with_no_readable_version_is_refused() {
    let _path = path_lock();
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = unversioned_tree(&temp.path().join("src"));

    let refused = expect_refused(acquire(
        &explicit(),
        &home_query(&tree),
        &FakeFetcher::serving(BINARY),
    ));

    assert_eq!(refused.source_version, SourceVersion::Unknown);
}

#[test]
fn the_first_valid_candidate_decides() {
    // A mismatched AMPLIHACK_HOME refuses even though the cwd would match.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let old_home = versioned_tree(&temp.path().join("old"), OLDER);
    let matching_cwd = versioned_tree(&temp.path().join("cwd"), BINARY);
    let query = FrameworkRootQuery {
        amplihack_home: Some(old_home.clone()),
        current_dir: Some(matching_cwd),
        ..FrameworkRootQuery::default()
    };

    let refused = expect_refused(acquire(&explicit(), &query, &FakeFetcher::serving(BINARY)));

    assert_eq!(refused.path, old_home);
}

#[test]
fn an_incompatible_source_is_skipped_before_its_version_is_considered() {
    // Validation runs first (D4), which keeps issue #734's workflow working: an
    // incompatible AMPLIHACK_HOME falls through to a compatible checkout.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let bad_home = versioned_tree(&temp.path().join("bad"), OLDER);
    let matching_cwd = versioned_tree(&temp.path().join("cwd"), BINARY);
    let query = FrameworkRootQuery {
        amplihack_home: Some(bad_home.clone()),
        current_dir: Some(matching_cwd.clone()),
        ..FrameworkRootQuery::default()
    };
    let validate = |candidate: &Path| {
        if candidate == bad_home {
            Err("missing required recipe".to_string())
        } else {
            Ok(())
        }
    };

    let source = acquire_framework_source(
        &explicit(),
        &query,
        &binary(),
        &FakeFetcher::serving(BINARY),
        &validate,
    )
    .unwrap();

    assert_eq!(source.root, matching_cwd);
}

#[test]
fn executable_and_compile_time_mismatches_are_notes_then_a_fetch() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let exe_repo = versioned_tree(&temp.path().join("exe-repo"), OLDER);
    mark_repository(&exe_repo);
    let exe = exe_repo.join("target/debug/amplihack");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let compile_root = versioned_tree(&temp.path().join("compile-root"), NEWER);
    let query = FrameworkRootQuery {
        current_exe: Some(exe),
        compile_time_workspace_root: Some(compile_root.clone()),
        ..FrameworkRootQuery::default()
    };
    let fetcher = FakeFetcher::serving(BINARY);

    let source = acquire(&explicit(), &query, &fetcher).unwrap();

    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
    assert_eq!(fetcher.call_count(), 1);
    let notes = notes(&explicit(), &query).join("\n");
    assert!(
        notes.contains(&exe_repo.display().to_string())
            && notes.contains(&compile_root.display().to_string()),
        "each candidate turned down needs a note: {notes}"
    );
}

#[test]
fn a_prior_install_qualifies_only_when_its_stamp_equals_the_binary() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    let staged = staged_install(&home, Some(BINARY));
    let query = FrameworkRootQuery {
        home: Some(home),
        ..FrameworkRootQuery::default()
    };
    let fetcher = FakeFetcher::failing("must not be called");

    let source = acquire(&explicit(), &query, &fetcher).unwrap();

    assert_eq!(source.root, staged);
    assert_eq!(fetcher.call_count(), 0);
}

#[test]
fn a_prior_install_with_any_other_stamp_means_a_note_and_a_fetch() {
    for stamp in [Some(OLDER), Some(NEWER), Some("garbage"), None] {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let home = temp.path().join("home");
        staged_install(&home, stamp);
        let query = FrameworkRootQuery {
            home: Some(home),
            ..FrameworkRootQuery::default()
        };
        let fetcher = FakeFetcher::serving(BINARY);

        let source = acquire(&explicit(), &query, &fetcher)
            .unwrap_or_else(|e| panic!("stamp {stamp:?}: {e:#}"));

        assert_eq!(source.origin, SourceOrigin::ReleaseTag, "stamp {stamp:?}");
        assert!(
            !notes(&explicit(), &query).is_empty(),
            "stamp {stamp:?}: a note is needed"
        );
    }
}

// ── D5: AMPLIHACK_HOME at ~/.amplihack ──

#[test]
fn amplihack_home_at_the_staged_install_never_refuses() {
    // Sessions set this variable themselves; a refusal would name a directory
    // the user never chose.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    let staged = staged_install(&home, Some(OLDER));
    let query = FrameworkRootQuery {
        amplihack_home: Some(staged),
        home: Some(home),
        ..FrameworkRootQuery::default()
    };
    let fetcher = FakeFetcher::serving(BINARY);

    let source = acquire(&explicit(), &query, &fetcher).unwrap();

    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
    assert_eq!(fetcher.call_count(), 1);
}

#[cfg(unix)]
#[test]
fn amplihack_home_through_a_symlink_to_the_staged_install_never_refuses() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    let staged = staged_install(&home, Some(OLDER));
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&staged, &link).unwrap();
    let query = FrameworkRootQuery {
        amplihack_home: Some(link),
        home: Some(home),
        ..FrameworkRootQuery::default()
    };

    let source = acquire(&explicit(), &query, &FakeFetcher::serving(BINARY)).unwrap();

    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
}

#[test]
fn a_different_amplihack_home_with_the_same_name_still_refuses() {
    // Only a canonical-path match is treated as the staged install.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    staged_install(&home, Some(BINARY));
    let other = versioned_tree(&temp.path().join("elsewhere/.amplihack"), OLDER);
    let query = FrameworkRootQuery {
        amplihack_home: Some(other.clone()),
        home: Some(home),
        ..FrameworkRootQuery::default()
    };

    let refused = expect_refused(acquire(&explicit(), &query, &FakeFetcher::serving(BINARY)));

    assert_eq!(refused.path, other);
}

// ── Implicit self-heal and bootstrap ──

#[test]
fn implicit_install_notes_every_mismatch_and_then_fetches() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let old_home = versioned_tree(&temp.path().join("old-home"), OLDER);
    let old_cwd = versioned_tree(&temp.path().join("old-cwd"), NEWER);
    let query = FrameworkRootQuery {
        amplihack_home: Some(old_home.clone()),
        current_dir: Some(old_cwd.clone()),
        ..FrameworkRootQuery::default()
    };
    let fetcher = FakeFetcher::serving(BINARY);

    let source = acquire(&implicit(), &query, &fetcher).unwrap();

    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
    let notes = notes(&implicit(), &query).join("\n");
    assert!(
        notes.contains(&old_home.display().to_string())
            && notes.contains(&old_cwd.display().to_string()),
        "{notes}"
    );
}

#[test]
fn implicit_install_uses_a_later_matching_candidate() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let old_home = versioned_tree(&temp.path().join("old-home"), OLDER);
    let matching_cwd = versioned_tree(&temp.path().join("cwd"), BINARY);
    let query = FrameworkRootQuery {
        amplihack_home: Some(old_home),
        current_dir: Some(matching_cwd.clone()),
        ..FrameworkRootQuery::default()
    };
    let fetcher = FakeFetcher::failing("must not be called");

    let source = acquire(&implicit(), &query, &fetcher).unwrap();

    assert_eq!(source.root, matching_cwd);
    assert_eq!(fetcher.call_count(), 0);
}

// ── --force-refresh (D6) ──

#[test]
fn force_refresh_uses_a_matching_amplihack_home_without_a_fetch() {
    // The reported bug: `update` with AMPLIHACK_HOME at a valid tag worktree
    // printed "Bundled framework source not found" and went to the network.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("tag-worktree"), BINARY);
    let fetcher = FakeFetcher::failing("must not be called");

    let source = acquire(&force_refresh(), &home_query(&tree), &fetcher).unwrap();

    assert_eq!(source.root, tree);
    assert_eq!(fetcher.call_count(), 0);
}

#[test]
fn force_refresh_with_a_mismatched_amplihack_home_notes_it_and_fetches() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("old"), OLDER);
    let fetcher = FakeFetcher::serving(BINARY);

    let source = acquire(&force_refresh(), &home_query(&tree), &fetcher).unwrap();

    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
    assert_eq!(
        notes(&force_refresh(), &home_query(&tree)),
        vec![format!(
            "ℹ️  Ignoring the framework source at {} (AMPLIHACK_HOME): its version is v{OLDER} \
             and this binary is v{BINARY}.",
            tree.display()
        )]
    );
}

#[test]
fn force_refresh_consults_nothing_but_amplihack_home_and_the_tag() {
    // Every other source would match, and none may be used.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let cwd = versioned_tree(&temp.path().join("cwd"), BINARY);
    let exe_repo = versioned_tree(&temp.path().join("exe"), BINARY);
    mark_repository(&exe_repo);
    let compile_root = versioned_tree(&temp.path().join("compile"), BINARY);
    let home = temp.path().join("home");
    staged_install(&home, Some(BINARY));
    let query = FrameworkRootQuery {
        amplihack_home: None,
        current_dir: Some(cwd),
        current_exe: Some(exe_repo.join("amplihack")),
        compile_time_workspace_root: Some(compile_root),
        home: Some(home),
    };
    let fetcher = FakeFetcher::serving(BINARY);

    let source = acquire(&force_refresh(), &query, &fetcher).unwrap();

    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
    assert_eq!(fetcher.call_count(), 1);
}

#[test]
fn force_refresh_does_not_use_the_staged_install_through_amplihack_home() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    let staged = staged_install(&home, Some(BINARY));
    let query = FrameworkRootQuery {
        amplihack_home: Some(staged),
        home: Some(home),
        ..FrameworkRootQuery::default()
    };
    let fetcher = FakeFetcher::serving(BINARY);

    let source = acquire(&force_refresh(), &query, &fetcher).unwrap();

    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
}

// ── --local ──

#[test]
fn local_stages_a_tree_of_any_version_with_a_warning_and_no_fetch() {
    for (version, shown) in [
        (Some(OLDER), "v1.2.0"),
        (Some(NEWER), "v1.3.0"),
        (None, "unknown"),
    ] {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let tree = temp.path().join("tree");
        match version {
            Some(version) => versioned_tree(&tree, version),
            None => unversioned_tree(&tree),
        };
        let fetcher = FakeFetcher::failing("must not be called");

        let source = acquire(&local(&tree), &FrameworkRootQuery::default(), &fetcher).unwrap();

        assert_eq!(source.root, tree);
        assert_eq!(source.origin, SourceOrigin::Local);
        assert_eq!(fetcher.call_count(), 0);
        assert_eq!(
            source.describe(BINARY),
            vec![
                format!(
                    "📦 Using framework source {} ({shown}, from --local)",
                    tree.display()
                ),
                format!(
                    "⚠️  This source is {shown} but this binary is v{BINARY}. Staging it because \
                     --local was given."
                ),
            ]
        );
    }
}

#[test]
fn local_at_the_same_version_prints_no_warning() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("tree"), BINARY);

    let source = acquire(
        &local(&tree),
        &FrameworkRootQuery::default(),
        &FakeFetcher::failing("must not be called"),
    )
    .unwrap();

    assert_eq!(
        source.describe(BINARY),
        vec![format!(
            "📦 Using framework source {} (v{BINARY}, from --local)",
            tree.display()
        )]
    );
}

#[test]
fn local_takes_priority_over_force_refresh_and_amplihack_home() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let chosen = versioned_tree(&temp.path().join("chosen"), OLDER);
    let other = versioned_tree(&temp.path().join("other"), BINARY);
    let policy = SourcePolicy {
        force_refresh: true,
        ..local(&chosen)
    };
    let fetcher = FakeFetcher::failing("must not be called");

    let source = acquire(&policy, &home_query(&other), &fetcher).unwrap();

    assert_eq!(source.root, chosen);
    assert_eq!(fetcher.call_count(), 0);
}

// ── Messages ──

#[test]
fn a_matching_source_names_its_path_version_and_origin() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("src"), BINARY);

    let source = acquire(
        &explicit(),
        &home_query(&tree),
        &FakeFetcher::failing("must not be called"),
    )
    .unwrap();

    assert_eq!(
        source.describe(BINARY),
        vec![format!(
            "📦 Using framework source {} (v{BINARY}, from AMPLIHACK_HOME)",
            tree.display()
        )]
    );
}

#[test]
fn the_refusal_names_the_source_both_versions_and_the_way_out() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("src"), OLDER);

    let refused = expect_refused(acquire(
        &explicit(),
        &home_query(&tree),
        &FakeFetcher::serving(BINARY),
    ));

    assert_eq!(
        refused.to_string(),
        format!(
            "refusing to stage the framework bundle from {path} (AMPLIHACK_HOME):\n  \
             source version: v{OLDER}\n  binary version: v{BINARY}\n\
             Run `amplihack install --local {path}` to stage this tree anyway.",
            path = tree.display()
        )
    );
}

#[test]
fn the_local_install_hint_quotes_a_path_with_a_space() {
    let refused = SourceRefused {
        path: PathBuf::from("/home/alice/my src/it's here"),
        label: "AMPLIHACK_HOME".to_string(),
        source_version: SourceVersion::parse(OLDER),
        binary_version: BINARY.to_string(),
    };

    let text = refused.to_string();

    assert!(
        text.contains("from /home/alice/my src/it's here (AMPLIHACK_HOME)"),
        "{text}"
    );
    assert!(
        text.contains(r#"`amplihack install --local '/home/alice/my src/it'\''s here'`"#),
        "{text}"
    );
}

#[test]
fn force_refresh_gives_its_own_reason_for_the_fetch() {
    let version = semver::Version::parse(BINARY).unwrap();

    assert!(fetch_reason(&version, true).ends_with("because --force-refresh was given."));
    assert!(
        fetch_reason(&version, false).ends_with("because no local source matches this binary.")
    );
}

#[test]
fn control_characters_in_a_source_path_do_not_reach_the_terminal() {
    // SEC-14.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("evil\n\u{1b}[2Jdir"), OLDER);

    let refused = expect_refused(acquire(
        &explicit(),
        &home_query(&tree),
        &FakeFetcher::serving(BINARY),
    ));

    let text = refused.to_string();
    assert!(!text.contains('\u{1b}'), "{text:?}");
    assert_eq!(
        text.lines().count(),
        4,
        "a newline in the path must not forge a line: {text:?}"
    );
}

#[test]
fn an_unknown_version_prints_as_the_word_unknown() {
    assert_eq!(SourceVersion::Unknown.to_string(), "unknown");
    assert_eq!(known("1.2.3").to_string(), "v1.2.3");
}

// ── D2 and build metadata ──

#[test]
fn a_build_that_may_not_fetch_makes_no_request_and_names_the_local_route() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    staged_install(&home, Some(OLDER));
    let query = FrameworkRootQuery {
        home: Some(home),
        ..FrameworkRootQuery::default()
    };
    let fetcher = FakeFetcher::serving(BINARY);
    let snapshot = "1.2.0+snapshot.7deb23da4";
    let build = BinaryIdentity {
        version: snapshot.to_string(),
        may_fetch: false,
    };

    let err = acquire_framework_source(&explicit(), &query, &build, &fetcher, &accept_all)
        .expect_err("no source and no fetch must fail");

    assert_eq!(fetcher.call_count(), 0, "this build must make no request");
    assert_eq!(
        err.downcast_ref::<SourceFetchFailed>(),
        Some(&SourceFetchFailed::NotAllowed {
            version: snapshot.to_string()
        })
    );
    assert_eq!(
        err.to_string(),
        format!(
            "this build ({snapshot}) cannot fetch a release tag, and no local source matches \
             it.\nRun `amplihack install --local <PATH>` to install from a checkout."
        )
    );
}

#[test]
fn build_metadata_is_ignored_when_versions_are_compared() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("main"), "1.2.0");
    let build = BinaryIdentity {
        version: "1.2.0+snapshot.7deb23da4".to_string(),
        may_fetch: false,
    };

    let source = acquire_framework_source(
        &explicit(),
        &home_query(&tree),
        &build,
        &FakeFetcher::failing("must not be called"),
        &accept_all,
    )
    .unwrap();

    assert_eq!(source.root, tree);
}

#[test]
fn the_real_binary_identity_follows_the_build() {
    let identity = BinaryIdentity::current();

    assert_eq!(identity.version, crate::VERSION);
    assert_eq!(
        identity.may_fetch,
        super::super::tag_fetch::fetch_allowed(
            crate::VERSION,
            option_env!("AMPLIHACK_RELEASE_VERSION").is_some()
        )
    );
}

// ── Reading a source's version (D3) ──

fn read_without_git(root: &Path) -> SourceVersion {
    read_source_version_with_git(root, None)
}

#[test]
fn version_comes_from_workspace_package_in_an_amplihack_cargo_toml() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    write_workspace_toml(temp.path(), "1.2.99");

    assert_eq!(read_without_git(temp.path()), known("1.2.99"));
}

#[test]
fn a_cargo_toml_that_does_not_name_amplihack_gives_no_version() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    write_workspace_toml(temp.path(), "1.2.99");
    assert_eq!(read_without_git(temp.path()), known("1.2.99"));
    fs::write(
        temp.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/other\"]\n\n[workspace.package]\nversion = \"1.2.99\"\n",
    )
    .unwrap();

    assert_eq!(read_without_git(temp.path()), SourceVersion::Unknown);
}

#[test]
fn a_package_version_outside_workspace_package_is_not_the_source_version() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    write_workspace_toml(temp.path(), "1.2.99");
    assert_eq!(read_without_git(temp.path()), known("1.2.99"));
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"x\"\nversion = \"1.2.99\"\n\n[workspace]\nmembers = \
         [\"crates/amplihack-cli\"]\n",
    )
    .unwrap();

    assert_eq!(read_without_git(temp.path()), SourceVersion::Unknown);
}

#[test]
fn a_hostile_version_string_is_unknown_and_cannot_reach_a_message() {
    // SEC-5: only a successful semver parse may become `Known`.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    write_workspace_toml(temp.path(), "\\u001b[31m1.2.99");

    assert_eq!(read_without_git(temp.path()), SourceVersion::Unknown);
    assert_eq!(
        SourceVersion::parse("1.2.99\u{1b}[31m"),
        SourceVersion::Unknown
    );
    assert_eq!(
        SourceVersion::parse("snapshot-abcdef"),
        SourceVersion::Unknown
    );
    assert_eq!(SourceVersion::parse(""), SourceVersion::Unknown);
    assert_eq!(SourceVersion::parse("1.2.99"), known("1.2.99"));
}

#[test]
fn a_cargo_toml_over_64_kib_gives_no_version() {
    // SEC-11.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    write_workspace_toml(temp.path(), "1.2.99");
    assert_eq!(read_without_git(temp.path()), known("1.2.99"));
    let path = temp.path().join("Cargo.toml");
    let mut body = fs::read_to_string(&path).unwrap();
    body.push_str(&"# padding\n".repeat(7000));
    assert!(body.len() > 64 * 1024);
    fs::write(&path, body).unwrap();

    assert_eq!(read_without_git(temp.path()), SourceVersion::Unknown);
}

#[cfg(unix)]
#[test]
fn a_symlinked_cargo_toml_gives_no_version() {
    // SEC-11: only a regular file counts.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let elsewhere = temp.path().join("elsewhere");
    write_workspace_toml(&elsewhere, "1.2.99");
    let tree = temp.path().join("tree");
    fs::create_dir_all(&tree).unwrap();
    assert_eq!(read_without_git(&elsewhere), known("1.2.99"));
    std::os::unix::fs::symlink(elsewhere.join("Cargo.toml"), tree.join("Cargo.toml")).unwrap();

    assert_eq!(read_without_git(&tree), SourceVersion::Unknown);
}

/// Runs `body` on a thread and fails if it blocks. A FIFO with no writer blocks
/// `open` forever, so a regression must fail here and not hang the suite.
#[cfg(unix)]
fn must_not_block<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(body());
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("a FIFO at Cargo.toml must not block a launch")
}

#[cfg(unix)]
fn mkfifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let raw = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `raw` is a valid NUL-terminated path for the duration of the call.
    assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o644) }, 0);
}

#[cfg(unix)]
#[test]
fn a_fifo_at_cargo_toml_gives_no_version_and_does_not_block() {
    // SEC-11: version reading runs in directories the user only stands in.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    write_workspace_toml(temp.path(), "1.2.99");
    assert_eq!(read_without_git(temp.path()), known("1.2.99"));
    fs::remove_file(temp.path().join("Cargo.toml")).unwrap();
    mkfifo(&temp.path().join("Cargo.toml"));
    let root = temp.path().to_path_buf();

    let version = must_not_block(move || read_source_version_with_git(&root, None));

    assert_eq!(version, SourceVersion::Unknown);
}

#[cfg(unix)]
#[test]
fn a_fifo_at_cargo_toml_does_not_block_the_source_shape_check() {
    // SEC-11 also covers looks_like_amplihack_source_in, reached from the walk-up.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("amplifier-bundle/recipes")).unwrap();
    mark_repository(&repo);
    mkfifo(&repo.join("Cargo.toml"));
    let keep_alive = temp.path().to_path_buf();
    let home = keep_alive.join("empty-home");

    let outcome = must_not_block(move || {
        let query = FrameworkRootQuery {
            current_dir: Some(repo),
            home: Some(home),
            ..FrameworkRootQuery::default()
        };
        let fetcher = FakeFetcher::serving(BINARY);
        let source = acquire(&implicit(), &query, &fetcher);
        source.map(|s| s.origin).map_err(|e| format!("{e:#}"))
    });

    assert_eq!(outcome, Ok(SourceOrigin::ReleaseTag));
}

// ── `git describe`, against real repositories ──

fn real_git() -> PathBuf {
    super::super::clone::which_git_in(&std::env::var_os("PATH").unwrap()).expect("git on PATH")
}

/// `read_source_version` finds git through `PATH`, which `InstallEnv` changes for
/// the whole process, so these tests hold the lock it holds.
fn path_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Runs `git` in `dir` with nothing inherited that could point it elsewhere.
fn git_in(dir: &Path, args: &[&str]) {
    let output = Command::new(real_git())
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo_with_one_commit(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git_in(dir, &["init", "--quiet"]);
    fs::write(dir.join("file"), "x\n").unwrap();
    git_in(dir, &["add", "file"]);
    git_in(dir, &["commit", "--quiet", "--no-gpg-sign", "-m", "c"]);
}

#[test]
fn with_no_cargo_version_the_exact_release_tag_is_used() {
    let _path = path_lock();
    let temp = crate::test_support::tempdir_outside_framework_roots();
    repo_with_one_commit(temp.path());
    git_in(temp.path(), &["tag", "v1.2.99"]);

    assert_eq!(read_source_version(temp.path()), known("1.2.99"));
}

#[test]
fn a_commit_past_the_tag_has_no_version() {
    let _path = path_lock();
    let temp = crate::test_support::tempdir_outside_framework_roots();
    repo_with_one_commit(temp.path());
    git_in(temp.path(), &["tag", "v1.2.99"]);
    assert_eq!(read_source_version(temp.path()), known("1.2.99"));
    fs::write(temp.path().join("file"), "y\n").unwrap();
    git_in(
        temp.path(),
        &["commit", "--quiet", "--no-gpg-sign", "-am", "next"],
    );

    assert_eq!(read_source_version(temp.path()), SourceVersion::Unknown);
}

#[test]
fn a_snapshot_tag_is_not_a_version() {
    let _path = path_lock();
    let temp = crate::test_support::tempdir_outside_framework_roots();
    repo_with_one_commit(temp.path());
    git_in(temp.path(), &["tag", "snapshot-7deb23da4"]);
    git_in(temp.path(), &["tag", "vnext"]);

    assert_eq!(read_source_version(temp.path()), SourceVersion::Unknown);

    git_in(temp.path(), &["tag", "v1.2.99"]);
    assert_eq!(read_source_version(temp.path()), known("1.2.99"));
}

#[test]
fn cargo_toml_outranks_a_tag_of_another_version() {
    let _path = path_lock();
    let temp = crate::test_support::tempdir_outside_framework_roots();
    repo_with_one_commit(temp.path());
    git_in(temp.path(), &["tag", "v1.2.99"]);
    write_workspace_toml(temp.path(), "1.2.0");

    assert_eq!(read_source_version(temp.path()), known("1.2.0"));
}

#[test]
fn a_fetched_tree_is_read_from_cargo_toml_only() {
    let _path = path_lock();
    // SEC-10: a matching tag in a tree with no declared version isn't enough.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    repo_with_one_commit(temp.path());
    git_in(temp.path(), &["tag", "v1.2.99"]);

    assert_eq!(
        read_fetched_tree_version(temp.path()),
        SourceVersion::Unknown
    );

    write_workspace_toml(temp.path(), "1.2.99");
    assert_eq!(read_fetched_tree_version(temp.path()), known("1.2.99"));
}

// ── D9: the same gate for the read-style callers ──

#[test]
fn the_read_style_lookup_is_gated_too() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let old = versioned_tree(&temp.path().join("old"), OLDER);
    mark_repository(&old);
    let query = cwd_query(&old);
    let lookup = super::super::clone::find_bundled_framework_root_for;

    assert_eq!(
        lookup(&query, BINARY),
        None,
        "an older cwd must not be returned"
    );

    let found = lookup(&query, OLDER).expect("the same version must be returned");
    assert_eq!(found.root, old);
}
