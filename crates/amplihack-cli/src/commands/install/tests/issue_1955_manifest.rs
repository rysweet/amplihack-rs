//! Issue #1955: a staged bundle must match its source, file for file, before it
//! replaces the installed bundle.

use super::super::bundle_manifest::{
    BundleManifest, ManifestDiff, compare, of_source, of_staged, render_mismatch,
};
use super::super::directories::{copy_amplifier_bundle, staging_hook::with_staging_hook};
use super::super::filesystem::{WalkEntryKind, WalkScope, copy_dir_recursive, walk_tree};
use super::helpers::*;
use super::issue_1526_implicit_downgrade::snapshot;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn manifest(files: &[(&str, &str)]) -> BundleManifest {
    BundleManifest {
        files: files
            .iter()
            .map(|(path, hash)| (path.to_string(), hash.to_string()))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// A tree holding everything the walker has to tell apart.
fn mixed_tree(root: &Path) {
    fs::create_dir_all(root.join("recipes")).unwrap();
    fs::create_dir_all(root.join("skills/deep/er")).unwrap();
    fs::create_dir_all(root.join("__pycache__")).unwrap();
    fs::create_dir_all(root.join("skills/.pytest_cache")).unwrap();
    fs::create_dir_all(root.join("tools/node_modules/x")).unwrap();
    fs::write(root.join("recipes/a.yaml"), "a\n").unwrap();
    fs::write(root.join("skills/deep/er/b.md"), "b\n").unwrap();
    fs::write(root.join("__pycache__/c.pyc"), "c").unwrap();
    fs::write(root.join("skills/.pytest_cache/d"), "d").unwrap();
    fs::write(root.join("tools/node_modules/x/e.js"), "e").unwrap();
    fs::write(root.join("tools/f.pyc"), "f").unwrap();
    fs::write(root.join("tools/g.pyo"), "g").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("a.yaml", root.join("recipes/link.yaml")).unwrap();
        std::os::unix::fs::symlink("../recipes", root.join("skills/dirlink")).unwrap();
        std::os::unix::fs::symlink("/nonexistent/target", root.join("broken")).unwrap();
    }
}

const COPIED: [&str; 2] = ["recipes/a.yaml", "skills/deep/er/b.md"];

fn keys(manifest: &BundleManifest) -> Vec<&str> {
    manifest.files.keys().map(String::as_str).collect()
}

// ── compare is pure ──

#[test]
fn identical_manifests_are_clean() {
    let side = manifest(&[("a", "1"), ("b/c", "2")]);
    assert!(compare(&side, &side.clone()).is_clean());
    assert!(!compare(&side, &manifest(&[("a", "1")])).is_clean());
    assert!(compare(&manifest(&[]), &manifest(&[])).is_clean());
}

#[test]
fn a_file_only_in_the_source_is_missing() {
    let diff = compare(
        &manifest(&[("a", "1"), ("b", "2")]),
        &manifest(&[("a", "1")]),
    );
    assert_eq!(diff.missing, ["b"]);
    assert!(diff.extra.is_empty() && diff.changed.is_empty());
    assert!(!diff.is_clean());
}

#[test]
fn a_file_only_in_staging_is_extra() {
    let diff = compare(
        &manifest(&[("a", "1")]),
        &manifest(&[("a", "1"), ("z", "9")]),
    );
    assert_eq!(diff.extra, ["z"]);
    assert!(diff.missing.is_empty() && diff.changed.is_empty());
}

#[test]
fn a_file_with_another_hash_is_changed() {
    let diff = compare(&manifest(&[("a", "1")]), &manifest(&[("a", "2")]));
    assert_eq!(diff.changed, ["a"]);
    assert!(diff.missing.is_empty() && diff.extra.is_empty());
}

#[test]
fn all_three_findings_are_reported_together() {
    let diff = compare(
        &manifest(&[("gone", "1"), ("same", "2"), ("edited", "3")]),
        &manifest(&[("same", "2"), ("edited", "4"), ("new", "5")]),
    );
    assert_eq!(
        diff,
        ManifestDiff {
            missing: vec!["gone".to_string()],
            extra: vec!["new".to_string()],
            changed: vec!["edited".to_string()],
        }
    );
}

// ── Building a manifest ──

#[test]
fn a_manifest_holds_relative_paths_and_sha256() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    fs::create_dir_all(temp.path().join("sub")).unwrap();
    fs::write(temp.path().join("sub/abc.txt"), "abc").unwrap();

    let built = of_source(temp.path()).unwrap();

    assert_eq!(
        built.files,
        BTreeMap::from([(
            "sub/abc.txt".to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string()
        )])
    );
}

#[test]
fn the_source_manifest_leaves_out_what_the_copy_leaves_out() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    mixed_tree(temp.path());

    assert_eq!(keys(&of_source(temp.path()).unwrap()), COPIED);
}

#[test]
fn the_staged_manifest_sees_everything_so_an_excluded_name_counts_as_extra() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    mixed_tree(temp.path());

    let staged = of_staged(temp.path()).unwrap();

    for path in [
        "__pycache__/c.pyc",
        "skills/.pytest_cache/d",
        "tools/node_modules/x/e.js",
        "tools/f.pyc",
        "tools/g.pyo",
    ] {
        assert!(staged.files.contains_key(path), "{path} must be listed");
    }
    assert!(
        !staged.files.contains_key("recipes/link.yaml"),
        "only regular files are listed"
    );
}

#[test]
fn a_copied_tree_matches_its_source() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let src = temp.path().join("src");
    mixed_tree(&src);
    let dst = temp.path().join("dst");

    copy_dir_recursive(&src, &dst).unwrap();

    let diff = compare(&of_source(&src).unwrap(), &of_staged(&dst).unwrap());
    assert!(diff.is_clean(), "{diff:?}");
}

#[test]
fn a_missing_root_is_an_error_not_an_empty_manifest() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    assert!(
        of_source(temp.path()).is_ok(),
        "an empty root is a valid walk"
    );
    let missing = temp.path().join("missing");
    assert!(of_source(&missing).is_err());
    assert!(of_staged(&missing).is_err());
}

// ── The shared walker ──

#[test]
fn walk_tree_copy_set_matches_what_the_copy_takes() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    mixed_tree(temp.path());

    let entries = walk_tree(temp.path(), WalkScope::CopySet).unwrap();

    let files: BTreeSet<PathBuf> = entries
        .iter()
        .filter(|entry| entry.kind == WalkEntryKind::File)
        .map(|entry| entry.rel.clone())
        .collect();
    assert_eq!(files, COPIED.iter().map(PathBuf::from).collect());
    assert!(
        entries
            .iter()
            .all(|entry| matches!(entry.kind, WalkEntryKind::File | WalkEntryKind::Dir)),
        "the copy set holds only files and directories: {entries:?}"
    );
    let dirs: Vec<&PathBuf> = entries
        .iter()
        .filter(|entry| entry.kind == WalkEntryKind::Dir)
        .map(|entry| &entry.rel)
        .collect();
    assert!(
        !dirs.iter().any(|dir| {
            let dir = dir.to_string_lossy();
            dir.contains("__pycache__")
                || dir.contains(".pytest_cache")
                || dir.contains("node_modules")
        }),
        "{dirs:?}"
    );
}

#[cfg(unix)]
#[test]
fn walk_tree_everything_reports_symlinks_without_following_them() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    mixed_tree(temp.path());

    let entries = walk_tree(temp.path(), WalkScope::Everything).unwrap();

    let kind_of = |rel: &str| {
        entries
            .iter()
            .find(|entry| entry.rel == Path::new(rel))
            .map(|entry| entry.kind)
    };
    assert_eq!(kind_of("skills/dirlink"), Some(WalkEntryKind::Symlink));
    assert_eq!(kind_of("broken"), Some(WalkEntryKind::Symlink));
    assert_eq!(kind_of("__pycache__/c.pyc"), Some(WalkEntryKind::File));
    assert_eq!(
        kind_of("skills/dirlink/a.yaml"),
        None,
        "a symlinked directory must not be walked"
    );
    assert_eq!(kind_of(""), None, "the root itself isn't an entry");
    let entry = entries
        .iter()
        .find(|entry| entry.rel == Path::new("recipes/a.yaml"))
        .unwrap();
    assert_eq!(entry.path, temp.path().join("recipes/a.yaml"));
}

// ── The failure message (SEC-14) ──

#[test]
fn the_mismatch_message_gives_counts_and_paths() {
    let diff = ManifestDiff {
        missing: vec!["recipes/default-workflow.yaml".to_string()],
        extra: vec![],
        changed: vec!["tools/ordinary-tool.sh".to_string()],
    };

    assert_eq!(
        render_mismatch(Path::new("/home/alice/src/amplihack-rs"), &diff),
        "the staged bundle does not match its source /home/alice/src/amplihack-rs (1 missing, 0 \
         extra, 1 changed):\n  missing: recipes/default-workflow.yaml\n  changed: \
         tools/ordinary-tool.sh"
    );
}

#[test]
fn the_mismatch_message_lists_at_most_twenty_paths_per_category() {
    let many = |prefix: &str, count: usize| -> Vec<String> {
        (0..count).map(|i| format!("{prefix}-{i:03}")).collect()
    };
    let diff = ManifestDiff {
        missing: many("gone", 25),
        extra: many("new", 21),
        changed: many("edited", 20),
    };

    let text = render_mismatch(Path::new("/src"), &diff);

    assert!(
        text.contains("(25 missing, 21 extra, 20 changed)"),
        "{text}"
    );
    for (prefix, listed) in [("gone-", 20), ("new-", 20), ("edited-", 20)] {
        assert_eq!(text.matches(prefix).count(), listed, "{prefix}: {text}");
    }
}

#[test]
fn control_characters_in_a_path_are_escaped_in_the_message() {
    let diff = ManifestDiff {
        missing: vec!["evil\n  changed: forged\u{1b}[2J.yaml".to_string()],
        ..ManifestDiff::default()
    };

    let text = render_mismatch(Path::new("/src\u{1b}[31m"), &diff);

    assert!(!text.contains('\u{1b}'), "{text:?}");
    assert_eq!(
        text.lines().count(),
        2,
        "a path must not be able to add a line: {text:?}"
    );
}

// ── Staging ──

struct Staging {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    claude_dir: PathBuf,
    target: PathBuf,
    staging: PathBuf,
}

/// A source repo plus an existing installed bundle with a sentinel in it.
fn staging_with_installed_bundle() -> Staging {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let repo = temp.path().join("repo");
    create_source_repo(&repo);
    fs::create_dir_all(repo.join("amplifier-bundle/skills/deep")).unwrap();
    fs::write(repo.join("amplifier-bundle/skills/deep/skill.md"), "s\n").unwrap();
    let claude_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&claude_dir).unwrap();
    let target = temp.path().join(".amplihack/amplifier-bundle");
    let staging = temp.path().join(".amplihack/amplifier-bundle.staging");
    copy_amplifier_bundle(&repo, &claude_dir).unwrap();
    fs::write(target.join("previous-install-sentinel"), "keep me\n").unwrap();
    Staging {
        _temp: temp,
        repo,
        claude_dir,
        target,
        staging,
    }
}

/// Tampering with staging fails the install, names `expected_path`, leaves the
/// previous bundle byte-identical and removes the staging directory.
fn assert_tampering_fails(tamper: impl FnMut(&Path) + 'static, expected_path: &str) {
    let fixture = staging_with_installed_bundle();
    let before = snapshot(&fixture.target);

    let result = with_staging_hook(tamper, || {
        copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir)
    });

    let err = result.expect_err("a staged bundle that differs from its source must fail");
    assert!(
        format!("{err:#}").contains(expected_path),
        "the error must name {expected_path}: {err:#}"
    );
    assert_eq!(
        snapshot(&fixture.target),
        before,
        "the previous bundle must stay byte-identical"
    );
    assert!(
        !fixture.staging.exists(),
        "staging must be removed on failure"
    );
}

#[test]
fn a_file_missing_from_staging_fails_the_install() {
    assert_tampering_fails(
        |staging| fs::remove_file(staging.join("skills/deep/skill.md")).unwrap(),
        "skills/deep/skill.md",
    );
}

#[test]
fn an_extra_file_in_staging_fails_the_install() {
    assert_tampering_fails(
        |staging| fs::write(staging.join("tools/smuggled.sh"), "x").unwrap(),
        "tools/smuggled.sh",
    );
}

#[test]
fn an_extra_excluded_name_in_staging_fails_the_install() {
    assert_tampering_fails(
        |staging| {
            fs::create_dir_all(staging.join("tools/__pycache__")).unwrap();
            fs::write(staging.join("tools/__pycache__/x.pyc"), "x").unwrap();
        },
        "tools/__pycache__/x.pyc",
    );
}

#[test]
fn a_changed_file_in_staging_fails_the_install() {
    assert_tampering_fails(
        |staging| fs::write(staging.join("recipes/default-workflow.yaml"), "edited\n").unwrap(),
        "recipes/default-workflow.yaml",
    );
}

#[test]
fn the_check_covers_files_outside_recipes() {
    // #1955's scope is the whole bundle, not the recipes the compatibility
    // check reads.
    for rel in [
        "agents/marker.txt",
        "behaviors/marker.txt",
        "tools/statusline.sh",
    ] {
        assert_tampering_fails(
            move |staging| fs::remove_file(staging.join(rel)).unwrap(),
            rel,
        );
    }
}

#[test]
fn a_clean_copy_is_swapped_in_and_leaves_no_staging() {
    let fixture = staging_with_installed_bundle();

    copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir).unwrap();

    assert!(
        !fixture.target.join("previous-install-sentinel").exists(),
        "a successful stage replaces the previous bundle"
    );
    assert!(fixture.target.join("skills/deep/skill.md").is_file());
    assert!(!fixture.staging.exists());
    let diff = compare(
        &of_source(&fixture.repo.join("amplifier-bundle")).unwrap(),
        &of_staged(&fixture.target).unwrap(),
    );
    assert!(diff.is_clean(), "{diff:?}");
}

#[cfg(unix)]
#[test]
fn symlinks_in_the_source_are_not_staged_and_do_not_fail_the_check() {
    // The real bundle has 6 symlinks. They are skipped by the copy and left out
    // of the source manifest, so they must not count as missing.
    let fixture = staging_with_installed_bundle();
    let bundle = fixture.repo.join("amplifier-bundle");
    std::os::unix::fs::symlink("marker.txt", bundle.join("agents/link")).unwrap();
    std::os::unix::fs::symlink("agents", bundle.join("dirlink")).unwrap();
    fs::create_dir_all(bundle.join("tools/__pycache__")).unwrap();
    fs::write(bundle.join("tools/__pycache__/x.pyc"), "x").unwrap();

    copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir).unwrap();

    assert!(fs::symlink_metadata(fixture.target.join("agents/link")).is_err());
    assert!(fs::symlink_metadata(fixture.target.join("dirlink")).is_err());
    assert!(!fixture.target.join("tools/__pycache__").exists());
    assert!(
        !fixture.target.join("previous-install-sentinel").exists(),
        "the stage must have completed"
    );
}

#[test]
fn a_leftover_staging_directory_is_removed_first() {
    // The leftover must not count as an extra file, or one interrupted install
    // would fail every later one.
    let fixture = staging_with_installed_bundle();
    fs::create_dir_all(&fixture.staging).unwrap();
    fs::write(fixture.staging.join("leftover"), "x").unwrap();

    copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir).unwrap();

    assert!(!fixture.target.join("leftover").exists());
    assert!(
        !fixture.target.join("previous-install-sentinel").exists(),
        "the stage must have completed"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_bundle_root_fails_and_creates_no_staging() {
    // SEC-12.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let real = temp.path().join("real");
    create_source_repo(&real);
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    std::os::unix::fs::symlink(real.join("amplifier-bundle"), repo.join("amplifier-bundle"))
        .unwrap();
    let claude_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&claude_dir).unwrap();

    let err = copy_amplifier_bundle(&repo, &claude_dir).expect_err("must fail");

    assert!(format!("{err:#}").contains("symlink"), "{err:#}");
    assert!(
        !temp
            .path()
            .join(".amplihack/amplifier-bundle.staging")
            .exists()
    );
    assert!(!temp.path().join(".amplihack/amplifier-bundle").exists());
}

// The message comes from the implementation; this catches the stub's silence.
#[test]
fn a_non_clean_diff_never_renders_to_an_empty_message() {
    let diff = ManifestDiff {
        changed: vec!["a".to_string()],
        ..ManifestDiff::default()
    };
    assert!(!render_mismatch(Path::new("/src"), &diff).is_empty());
}
