//! Issue #1972 R9, R10, D10, D13: local candidates and `--local` against the
//! release manifest embedded in the binary. The table is "Release manifest
//! check" in `docs/reference/framework-source-selection.md`.
//!
//! Pure: every input, the manifest included, is a parameter.

use super::super::InstallMode;
use super::super::clone::{
    FrameworkRootOrigin, FrameworkRootQuery, find_bundled_framework_root_for,
};
use super::super::framework_source::*;
use super::super::release_manifest::{ManifestFinding, ReleaseManifest};
use super::issue_1955_support::*;
use super::issue_1972_support::*;
use std::path::{Path, PathBuf};

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

fn acquire(
    policy: &SourcePolicy,
    query: &FrameworkRootQuery,
    release: Option<&ReleaseManifest>,
    fetcher: &FakeFetcher,
) -> anyhow::Result<AcquiredSource> {
    acquire_framework_source_with_manifest(
        policy,
        query,
        &release_binary(),
        release,
        fetcher,
        &accept_all,
    )
}

fn notes(
    policy: &SourcePolicy,
    query: &FrameworkRootQuery,
    release: &ReleaseManifest,
) -> Vec<String> {
    resolve_for_policy_with_manifest(policy, query, BINARY, Some(release), &accept_all)
        .notes
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn expect_manifest_refusal(result: anyhow::Result<AcquiredSource>) -> ReleaseManifestRefused {
    let err = result.expect_err("the source must be refused");
    assert!(
        err.downcast_ref::<SourceRefused>().is_none(),
        "a manifest refusal is not a version refusal: {err:#}"
    );
    err.downcast_ref::<ReleaseManifestRefused>()
        .unwrap_or_else(|| panic!("expected ReleaseManifestRefused, got: {err:#}"))
        .clone()
}

/// A tree at [`BINARY`], the release manifest taken from it while pristine.
struct Fixture {
    _temp: tempfile::TempDir,
    tree: PathBuf,
    release: ReleaseManifest,
}

fn fixture() -> Fixture {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("src"), BINARY);
    let release = release_of(&tree, BINARY);
    Fixture {
        _temp: temp,
        tree,
        release,
    }
}

// ── Matching candidates are accepted as before ──

#[test]
fn issue_1972_a_matching_amplihack_home_is_staged_without_a_fetch() {
    let fx = fixture();
    let fetcher = FakeFetcher::failing("must not be called");

    let source = acquire(
        &explicit(),
        &home_query(&fx.tree),
        Some(&fx.release),
        &fetcher,
    )
    .unwrap();

    assert_eq!(
        source.origin,
        SourceOrigin::Resolved(FrameworkRootOrigin::AmplihackHome)
    );
    assert_eq!(fetcher.call_count(), 0);
    assert!(source.warnings().is_empty());
}

#[test]
fn issue_1972_excluded_names_in_a_candidate_do_not_count_as_extra() {
    // AC5: the walk install copies leaves these out, so the manifest does too.
    let fx = fixture();
    add_file(&fx.tree, "tools/__pycache__/x.cpython-312.pyc");
    add_file(&fx.tree, "tools/.pytest_cache/v/cache");
    add_file(&fx.tree, "tools/node_modules/dep/index.js");
    add_file(&fx.tree, "tools/stale.pyc");
    add_file(&fx.tree, "tools/stale.pyo");
    let fetcher = FakeFetcher::failing("must not be called");

    let source = acquire(
        &explicit(),
        &home_query(&fx.tree),
        Some(&fx.release),
        &fetcher,
    )
    .unwrap();

    assert_eq!(
        source.origin,
        SourceOrigin::Resolved(FrameworkRootOrigin::AmplihackHome)
    );
}

#[test]
fn issue_1972_without_a_manifest_behaviour_is_unchanged() {
    let fx = fixture();
    change_file(&fx.tree, LISTED);
    let fetcher = FakeFetcher::failing("must not be called");

    let source = acquire(&explicit(), &home_query(&fx.tree), None, &fetcher).unwrap();

    assert_eq!(source.root, fx.tree);
}

// ── R9: plain install refuses AMPLIHACK_HOME and the cwd ──

#[test]
fn issue_1972_plain_install_refuses_a_changed_amplihack_home() {
    let fx = fixture();
    change_file(&fx.tree, LISTED);
    let fetcher = FakeFetcher::failing("must not be called");

    let refused = expect_manifest_refusal(acquire(
        &explicit(),
        &home_query(&fx.tree),
        Some(&fx.release),
        &fetcher,
    ));

    assert_eq!(refused.path, fx.tree);
    assert_eq!(refused.label, "AMPLIHACK_HOME");
    assert_eq!(
        fetcher.call_count(),
        0,
        "a refusal does not fall through to a fetch"
    );
    let text = refused.to_string();
    for phrase in [
        "refusing to stage the framework bundle from",
        "(AMPLIHACK_HOME)",
        &format!("does not match the release v{BINARY} manifest embedded in this binary"),
        "(0 missing, 0 extra, 1 changed)",
        &format!("changed: {LISTED}"),
        "Nothing was deployed and the installed bundle is unchanged.",
        "amplihack install --local",
    ] {
        assert!(text.contains(phrase), "missing {phrase:?} in:\n{text}");
    }
}

#[test]
fn issue_1972_plain_install_refuses_a_cwd_with_a_missing_or_extra_file() {
    let remove = |tree: &Path| remove_file(tree, LISTED);
    let add = |tree: &Path| add_file(tree, "agents/new.md");
    for (edit, counts) in [
        (&remove as &dyn Fn(&Path), "(1 missing, 0 extra, 0 changed)"),
        (&add, "(0 missing, 1 extra, 0 changed)"),
    ] {
        let fx = fixture();
        mark_repository(&fx.tree);
        edit(&fx.tree);
        let query = FrameworkRootQuery {
            current_dir: Some(fx.tree.clone()),
            ..FrameworkRootQuery::default()
        };
        let fetcher = FakeFetcher::failing("must not be called");

        let refused =
            expect_manifest_refusal(acquire(&explicit(), &query, Some(&fx.release), &fetcher));

        assert_eq!(refused.label, "current directory");
        assert!(refused.to_string().contains(counts), "{refused}");
    }
}

#[test]
fn issue_1972_the_refusal_travels_through_resolution_as_a_refusal() {
    let fx = fixture();
    change_file(&fx.tree, LISTED);

    let resolution = resolve_for_policy_with_manifest(
        &explicit(),
        &home_query(&fx.tree),
        BINARY,
        Some(&fx.release),
        &accept_all,
    );

    assert!(matches!(
        resolution.refused,
        Some(Refusal::ReleaseManifest(_))
    ));
    assert!(resolution.resolved.is_none());
}

#[test]
fn issue_1972_a_version_mismatch_is_refused_as_before_without_hashing() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("src"), "1.2.0");
    let release = release_of(&tree, BINARY);
    change_file(&tree, LISTED);

    let err = acquire(
        &explicit(),
        &home_query(&tree),
        Some(&release),
        &FakeFetcher::failing("unused"),
    )
    .unwrap_err();

    assert!(err.downcast_ref::<SourceRefused>().is_some(), "{err:#}");
    assert!(err.downcast_ref::<ReleaseManifestRefused>().is_none());
}

#[cfg(unix)]
#[test]
fn issue_1972_an_unreadable_file_fails_closed() {
    let fx = fixture();
    let Some(_guard) = UnreadableFile::new(&fx.tree.join("amplifier-bundle").join(LISTED)) else {
        eprintln!("skipped: running as root");
        return;
    };

    let refused = expect_manifest_refusal(acquire(
        &explicit(),
        &home_query(&fx.tree),
        Some(&fx.release),
        &FakeFetcher::failing("unused"),
    ));

    assert!(matches!(refused.finding, ManifestFinding::Unreadable(_)));
    let text = refused.to_string();
    assert!(
        text.contains(&format!(
            "could not be checked against the release v{BINARY} manifest embedded in this binary"
        )),
        "{text}"
    );
}

#[cfg(unix)]
#[test]
fn issue_1972_a_version_mismatch_is_never_hashed() {
    // An unreadable file would be a finding if hashed; a version mismatch
    // must decide first.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("src"), "1.2.0");
    let release = release_of(&tree, BINARY);
    let Some(_guard) = UnreadableFile::new(&tree.join("amplifier-bundle").join(LISTED)) else {
        eprintln!("skipped: running as root");
        return;
    };

    let notes = notes(&implicit(), &home_query(&tree), &release);

    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("its version is v1.2.0"), "{notes:?}");
}

// ── R9: every other case notes the mismatch and moves on ──

#[test]
fn issue_1972_an_implicit_launch_skips_a_changed_amplihack_home_and_fetches() {
    let fx = fixture();
    change_file(&fx.tree, LISTED);
    let release = fx.release.clone();
    // The tag serves the release as built, which the pristine fixture is.
    let fetcher = FakeFetcher::new(|destination| {
        let root = destination.join("amplihack-rs");
        versioned_tree(&root, BINARY);
        Ok(root)
    });

    let source = acquire(&implicit(), &home_query(&fx.tree), Some(&release), &fetcher).unwrap();

    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
    assert_eq!(fetcher.call_count(), 1);
    let notes = notes(&implicit(), &home_query(&fx.tree), &release);
    assert_eq!(notes.len(), 1, "{notes:?}");
    for phrase in [
        "Ignoring the framework source at",
        "(AMPLIHACK_HOME)",
        &format!("it declares v{BINARY}, but its amplifier-bundle/ does not match the release"),
        &format!("changed: {LISTED}"),
        "Trying the next source.",
    ] {
        assert!(notes[0].contains(phrase), "missing {phrase:?} in {notes:?}");
    }
}

#[test]
fn issue_1972_force_refresh_skips_a_changed_amplihack_home() {
    let fx = fixture();
    change_file(&fx.tree, LISTED);

    let resolution = resolve_for_policy_with_manifest(
        &force_refresh(),
        &home_query(&fx.tree),
        BINARY,
        Some(&fx.release),
        &accept_all,
    );

    assert!(resolution.refused.is_none());
    assert!(resolution.resolved.is_none());
    assert_eq!(resolution.notes.len(), 1);
}

#[test]
fn issue_1972_the_compile_time_root_is_skipped_for_the_next_candidate() {
    let fx = fixture();
    change_file(&fx.tree, LISTED);
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().to_path_buf();
    staged_install(&home, Some(BINARY));
    let staged_release = ReleaseManifest {
        version: BINARY.to_string(),
        files: release_of(&home.join(".amplihack"), BINARY).files,
    };
    // One manifest both can be compared with: the staged copy's.
    let query = FrameworkRootQuery {
        compile_time_workspace_root: Some(fx.tree.clone()),
        home: Some(home.clone()),
        ..FrameworkRootQuery::default()
    };

    let source = acquire(
        &explicit(),
        &query,
        Some(&staged_release),
        &FakeFetcher::failing("unused"),
    )
    .unwrap();

    assert_eq!(
        source.origin,
        SourceOrigin::Resolved(FrameworkRootOrigin::PriorStagedInstall)
    );
    let notes = notes(&explicit(), &query, &staged_release);
    assert!(
        notes
            .iter()
            .any(|n| n.contains("Ignoring the framework source") && n.contains("compile-time")),
        "{notes:?}"
    );
}

#[test]
fn issue_1972_a_changed_staged_install_with_a_matching_stamp_is_skipped() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().to_path_buf();
    let staged = staged_install(&home, Some(BINARY));
    let release = release_of(&staged, BINARY);
    add_file(&staged, "agents/after-release.md");
    let query = FrameworkRootQuery {
        home: Some(home),
        ..FrameworkRootQuery::default()
    };
    let fetcher = FakeFetcher::serving(BINARY);
    let served = served_release(BINARY);
    let release_for_fetch = ReleaseManifest {
        files: served.files,
        ..release.clone()
    };

    // With the staged copy's own manifest it is noted; then the tag is used
    // with a manifest the served tree matches.
    let notes = notes(&explicit(), &query, &release);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("Ignoring the framework source"),
        "{notes:?}"
    );
    let source = acquire(&explicit(), &query, Some(&release_for_fetch), &fetcher).unwrap();
    assert_eq!(source.origin, SourceOrigin::ReleaseTag);
}

#[test]
fn issue_1972_the_codex_lookup_never_hashes() {
    let fx = fixture();
    change_file(&fx.tree, LISTED);

    let found = find_bundled_framework_root_for(&home_query(&fx.tree), BINARY)
        .expect("the Codex lookup gates on version only");

    assert_eq!(found.root, fx.tree);
}

// ── R10, D13: `--local` warns and stages ──

#[test]
fn issue_1972_local_at_the_binarys_version_warns_and_stages() {
    let fx = fixture();
    change_file(&fx.tree, LISTED);

    let source = acquire(
        &local(&fx.tree),
        &FrameworkRootQuery::default(),
        Some(&fx.release),
        &FakeFetcher::failing("unused"),
    )
    .unwrap();

    assert_eq!(source.origin, SourceOrigin::Local);
    let warnings = source.warnings();
    assert_eq!(warnings.len(), 1);
    for phrase in [
        "The --local source",
        &format!("declares v{BINARY}, but it does not match the release v{BINARY} manifest"),
        "(0 missing, 0 extra, 1 changed)",
        "Staging it anyway because --local was given.",
    ] {
        assert!(
            warnings[0].contains(phrase),
            "missing {phrase:?} in {warnings:?}"
        );
    }
}

#[test]
fn issue_1972_local_matching_or_at_another_version_has_no_warning() {
    let fx = fixture();
    let source = acquire(
        &local(&fx.tree),
        &FrameworkRootQuery::default(),
        Some(&fx.release),
        &FakeFetcher::failing("unused"),
    )
    .unwrap();
    assert!(source.warnings().is_empty());

    let temp = crate::test_support::tempdir_outside_framework_roots();
    for tree in [
        versioned_tree(&temp.path().join("older"), "1.2.0"),
        unversioned_tree(&temp.path().join("unknown")),
    ] {
        change_file(&tree, LISTED);
        let source = acquire(
            &local(&tree),
            &FrameworkRootQuery::default(),
            Some(&fx.release),
            &FakeFetcher::failing("unused"),
        )
        .unwrap();
        assert!(source.warnings().is_empty(), "{}", tree.display());
    }
}

// ── Output injection ──

#[cfg(unix)]
#[test]
fn issue_1972_control_characters_never_reach_a_message() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = versioned_tree(&temp.path().join("src\u{1b}[31m\nx"), BINARY);
    let release = release_of(&tree, BINARY);
    add_file(&tree, "agents/evil\u{1b}]0;title\u{7}\nname.md");

    let refused = expect_manifest_refusal(acquire(
        &explicit(),
        &home_query(&tree),
        Some(&release),
        &FakeFetcher::failing("unused"),
    ));
    let text = refused.to_string();
    assert!(only_newlines_are_control(&text), "{text:?}");
    let hint = text
        .lines()
        .find(|line| line.contains("--local"))
        .expect("the hint");
    assert!(hint.contains("'"), "a path with `?` is quoted: {hint}");

    let source = acquire(
        &local(&tree),
        &FrameworkRootQuery::default(),
        Some(&release),
        &FakeFetcher::failing("unused"),
    )
    .unwrap();
    for warning in source.warnings() {
        assert!(only_newlines_are_control(&warning), "{warning:?}");
    }
    for note in notes(&implicit(), &home_query(&tree), &release) {
        assert!(only_newlines_are_control(&note), "{note:?}");
    }
}
