//! Issue #1955: staging sets modes itself. Nothing under the staged bundle is
//! writable by group or others, whatever modes the source had and whatever the
//! umask, and executables stay executable.

use super::super::directories::{
    copy_amplifier_bundle, normalize_bundle_modes, staging_hook::with_staging_hook,
};
use super::super::tag_fetch::{TARBALL_LIMITS, extract_tag_tarball};
use super::helpers::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// The mode of `path`, through a symlink: since issue #1954 the published
/// `~/.amplihack/amplifier-bundle` is a link into its release, and a link's
/// own mode is meaningless.
fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Every entry under and including `root`, symlinks not followed below the
/// root (a linked root, issue #1954, is resolved to its release first).
fn all_entries(root: &Path) -> Vec<PathBuf> {
    let root = fs::canonicalize(root).unwrap();
    let root = root.as_path();
    let mut found = vec![root.to_path_buf()];
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                queue.push(path.clone());
            }
            found.push(path);
        }
    }
    found
}

/// `find <root> -perm /022`, printing nothing on a clean bundle.
fn assert_no_group_or_other_write(root: &Path) {
    let offenders: Vec<String> = all_entries(root)
        .iter()
        .filter(|path| mode_of(path) & 0o022 != 0)
        .map(|path| format!("{:o} {}", mode_of(path), path.display()))
        .collect();
    assert!(
        offenders.is_empty(),
        "writable by group or others:\n{}",
        offenders.join("\n")
    );
}

/// Give a tree the modes a checkout gets under umask 002: 775 directories, 664
/// files, and 775 for the files listed in `executables`.
fn set_loose_modes(root: &Path, executables: &[&str]) {
    for path in all_entries(root) {
        let is_dir = fs::symlink_metadata(&path).unwrap().is_dir();
        chmod(&path, if is_dir { 0o775 } else { 0o664 });
    }
    for rel in executables {
        chmod(&root.join(rel), 0o775);
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    claude_dir: PathBuf,
    target: PathBuf,
}

/// A source repo at 775/664 with a merge guard, a gate file and a script.
fn loose_fixture() -> Fixture {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let repo = temp.path().join("repo");
    create_source_repo(&repo);
    let tools = repo.join("amplifier-bundle/tools");
    fs::create_dir_all(&tools).unwrap();
    fs::write(tools.join("run-me.sh"), "#!/bin/sh\n").unwrap();
    fs::write(tools.join("read-me.md"), "text\n").unwrap();
    set_loose_modes(&repo.join("amplifier-bundle"), &["tools/run-me.sh"]);
    let claude_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&claude_dir).unwrap();
    let target = temp.path().join(".amplihack/amplifier-bundle");
    Fixture {
        _temp: temp,
        repo,
        claude_dir,
        target,
    }
}

// ── The rule, entry by entry ──

#[test]
fn normalize_applies_the_mode_rule() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let root = temp.path().join("bundle");
    fs::create_dir_all(root.join("dir/sub")).unwrap();
    // (file, source mode, expected mode)
    let cases = [
        ("group-write", 0o664, 0o644),
        ("other-write", 0o666, 0o644),
        ("private", 0o600, 0o644),
        ("group-exec-only", 0o674, 0o755),
        ("exec-loose", 0o775, 0o755),
        ("exec-world", 0o777, 0o755),
        ("exec-private", 0o700, 0o755),
        ("exec-owner-read", 0o744, 0o755),
        ("exec-other-only", 0o641, 0o755),
        ("read-only", 0o444, 0o644),
    ];
    for (name, mode, _) in cases {
        let path = root.join("dir/sub").join(name);
        fs::write(&path, "x").unwrap();
        chmod(&path, mode);
    }
    chmod(&root.join("dir/sub"), 0o777);
    chmod(&root.join("dir"), 0o700);
    chmod(&root, 0o775);

    normalize_bundle_modes(&root).unwrap();

    for (name, mode, expected) in cases {
        assert_eq!(
            mode_of(&root.join("dir/sub").join(name)),
            expected,
            "{name} from {mode:o}"
        );
    }
    assert_eq!(mode_of(&root.join("dir/sub")), 0o755);
    assert_eq!(mode_of(&root.join("dir")), 0o755);
    assert_eq!(mode_of(&root), 0o755, "the root is a directory too");
}

#[test]
fn normalize_clears_setuid_setgid_and_sticky_bits() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let root = temp.path().join("bundle");
    fs::create_dir_all(root.join("dir")).unwrap();
    fs::write(root.join("dir/tool"), "x").unwrap();
    chmod(&root.join("dir/tool"), 0o6755);
    chmod(&root.join("dir"), 0o3775);

    normalize_bundle_modes(&root).unwrap();

    assert_eq!(mode_of(&root.join("dir/tool")), 0o755);
    assert_eq!(mode_of(&root.join("dir")), 0o755);
}

#[test]
fn normalize_returns_an_error_on_a_symlink_and_leaves_its_target_alone() {
    // SEC-12: set_permissions follows links, so a symlink in staging could
    // chmod a file outside the bundle.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let outside = temp.path().join("outside");
    fs::write(&outside, "x").unwrap();
    chmod(&outside, 0o600);
    let root = temp.path().join("bundle");
    fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

    let err = normalize_bundle_modes(&root).expect_err("a symlink in staging must fail");

    assert!(format!("{err:#}").contains("link"), "{err:#}");
    assert_eq!(mode_of(&outside), 0o600, "the target must not be changed");
}

#[test]
fn normalize_reaches_a_directory_with_no_search_permission() {
    // A 0o000 source directory copied by a later change must not make the walk
    // skip what is under it. Directories are opened up before they are read.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let root = temp.path().join("bundle");
    fs::create_dir_all(root.join("dir")).unwrap();
    fs::write(root.join("dir/file"), "x").unwrap();
    chmod(&root.join("dir/file"), 0o666);
    chmod(&root.join("dir"), 0o300);

    let result = normalize_bundle_modes(&root);
    chmod(&root.join("dir"), 0o755); // So the temp dir can be removed on failure.

    result.unwrap();
    assert_eq!(mode_of(&root.join("dir/file")), 0o644);
}

// ── Staging ──

#[test]
fn a_loose_source_is_staged_with_no_group_or_other_write() {
    let fixture = loose_fixture();

    copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir).unwrap();

    assert_no_group_or_other_write(&fixture.target);
}

#[test]
fn execute_bits_are_kept_both_ways() {
    let fixture = loose_fixture();

    copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir).unwrap();

    let tools = fixture.target.join("tools");
    assert_eq!(mode_of(&tools.join("run-me.sh")), 0o755, "stays executable");
    assert_eq!(mode_of(&tools.join("read-me.md")), 0o644, "stays plain");
    assert_eq!(
        mode_of(&fixture.target.join("recipes/default-workflow.yaml")),
        0o644
    );
    assert_eq!(mode_of(&fixture.target), 0o755);
}

#[test]
fn the_source_tree_is_not_changed_by_staging() {
    let fixture = loose_fixture();
    let script = fixture.repo.join("amplifier-bundle/tools/run-me.sh");

    copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir).unwrap();

    assert_eq!(mode_of(&script), 0o775, "only the copy gets the mode rule");
}

#[test]
fn staging_stays_private_until_the_copy_has_been_checked() {
    // SEC-4: another local user must not be able to read or swap anything in
    // the staging root while the copy and the manifest comparison run.
    let fixture = loose_fixture();
    let seen = Arc::new(Mutex::new(None));
    let record = Arc::clone(&seen);

    with_staging_hook(
        move |staging| {
            *record.lock().unwrap() = Some(mode_of(staging));
        },
        || copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir),
    )
    .unwrap();

    assert_eq!(*seen.lock().unwrap(), Some(0o700));
}

#[test]
fn a_loose_previous_bundle_is_replaced_by_a_clean_one() {
    // The hosts that motivated this: bundles already staged at 775/664.
    let fixture = loose_fixture();
    copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir).unwrap();
    set_loose_modes(&fixture.target, &[]);

    copy_amplifier_bundle(&fixture.repo, &fixture.claude_dir).unwrap();

    assert_no_group_or_other_write(&fixture.target);
}

#[test]
fn a_tar_extracted_source_is_staged_clean() {
    let fixture = loose_fixture();
    let archive = {
        let mut bytes = Vec::new();
        {
            let encoder = flate2::write::GzEncoder::new(&mut bytes, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            builder
                .append_dir_all("amplihack-rs-1.2.99", &fixture.repo)
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        bytes
    };
    let extracted = crate::test_support::tempdir_outside_framework_roots();

    extract_tag_tarball(&archive, extracted.path(), &TARBALL_LIMITS).unwrap();

    let repo = extracted.path().join("amplihack-rs-1.2.99");
    copy_amplifier_bundle(&repo, &fixture.claude_dir).unwrap();
    assert_no_group_or_other_write(&fixture.target);
    assert_eq!(mode_of(&fixture.target.join("tools/run-me.sh")), 0o755);
    assert_eq!(mode_of(&fixture.target.join("tools/read-me.md")), 0o644);
}

// ── Under a real umask ──
//
// The umask is per process, and the sibling tests in this binary create files
// concurrently, so each of these runs alone in a child of the test binary.

const UMASK_CHILD: &str = "AMPLIHACK_ISSUE_1955_UMASK_CHILD";

fn under_umask(test: &str, mask: libc::mode_t, body: impl FnOnce()) {
    if std::env::var_os(UMASK_CHILD).is_some() {
        // SAFETY: umask has no failure mode, and this child runs only this test.
        unsafe { libc::umask(mask) };
        body();
        return;
    }
    let name = format!("commands::install::tests::issue_1955_modes::{test}");
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--test-threads=1", "--nocapture"])
        .env(UMASK_CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{name} failed under umask {mask:o}:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("test result: ok. 1 passed"),
        "re-running {name} ran no test:\n{stdout}"
    );
}

fn stage_under_current_umask() {
    // The source is created here, so its modes come from this umask too.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let repo = temp.path().join("repo");
    create_source_repo(&repo);
    let script = repo.join("amplifier-bundle/tools/run-me.sh");
    fs::write(&script, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o777)).unwrap();
    let claude_dir = temp.path().join(".amplihack/.claude");
    fs::create_dir_all(&claude_dir).unwrap();

    copy_amplifier_bundle(&repo, &claude_dir).unwrap();

    let target = temp.path().join(".amplihack/amplifier-bundle");
    assert_no_group_or_other_write(&target);
    assert_eq!(mode_of(&target.join("tools/run-me.sh")), 0o755);
    assert_eq!(mode_of(&target.join("agents/marker.txt")), 0o644);
    assert_eq!(mode_of(&target.join("agents")), 0o755);
}

#[test]
fn staging_under_umask_002_is_clean() {
    under_umask(
        "staging_under_umask_002_is_clean",
        0o002,
        stage_under_current_umask,
    );
}

#[test]
fn staging_under_umask_000_is_clean() {
    under_umask(
        "staging_under_umask_000_is_clean",
        0o000,
        stage_under_current_umask,
    );
}

#[test]
fn staging_under_umask_077_is_still_readable_by_others() {
    // The rule is exact, so a strict umask doesn't leave a bundle other users
    // on the host can't read.
    under_umask(
        "staging_under_umask_077_is_still_readable_by_others",
        0o077,
        stage_under_current_umask,
    );
}
