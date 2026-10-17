//! Issue #1526 — a source build of a release tag must report that release.
//!
//! The Auto Release `release` job runs `scripts/stamp-release-version.sh X`
//! on the released `main` SHA, commits the manifests as
//! `chore(release): stamp vX` and tags that child commit, so
//! `cargo install --git … --tag vX` reports X. Nothing is pushed to `main`.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MANIFESTS: [&str; 3] = ["Cargo.toml", "Cargo.lock", "package.json"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A temp directory holding copies of the real workspace manifests.
fn manifests_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for name in MANIFESTS {
        fs::copy(repo_root().join(name), dir.path().join(name)).unwrap();
    }
    dir
}

fn stamp(dir: &Path, args: &[&str]) -> Output {
    Command::new("sh")
        .arg(repo_root().join("scripts/stamp-release-version.sh"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name)).unwrap()
}

#[test]
fn stamps_workspace_members_and_leaves_registry_crates_alone() {
    let dir = manifests_copy();
    let out = stamp(dir.path(), &["9999.0.7"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let toml = read(dir.path(), "Cargo.toml");
    assert!(toml.contains("[workspace.package]\nversion = \"9999.0.7\"\n"));
    assert!(read(dir.path(), "package.json").contains("\n  \"version\": \"9999.0.7\","));

    let lock = read(dir.path(), "Cargo.lock");
    let original = fs::read_to_string(repo_root().join("Cargo.lock")).unwrap();
    for block in lock.split("[[package]]").skip(1) {
        let stamped = block.contains("\nversion = \"9999.0.7\"\n");
        let is_member = !block.contains("\nsource = ");
        assert_eq!(stamped, is_member, "wrong Cargo.lock stamp:{block}");
    }
    assert_eq!(lock.lines().count(), original.lines().count());

    let again = stamp(dir.path(), &["9999.0.7"]);
    assert!(again.status.success());
    assert_eq!(
        read(dir.path(), "Cargo.lock"),
        lock,
        "a second run is a no-op"
    );
}

#[test]
fn bad_arguments_are_usage_errors_and_write_nothing() {
    let dir = manifests_copy();
    let before: Vec<String> = MANIFESTS.iter().map(|n| read(dir.path(), n)).collect();
    for bad in [
        "1.2",
        "01.2.3",
        "v1.2.3",
        "1.2.3-rc.1",
        "1.2.3; rm -rf /",
        "$(id)",
        "1.2.3\n1.2.4",
    ] {
        let out = stamp(dir.path(), &[bad]);
        assert_eq!(out.status.code(), Some(2), "argument {bad:?}");
    }
    assert_eq!(stamp(dir.path(), &[]).status.code(), Some(2));
    let after: Vec<String> = MANIFESTS.iter().map(|n| read(dir.path(), n)).collect();
    assert_eq!(before, after);
}

#[test]
fn release_job_stamps_before_tagging_and_pushes_only_the_tag() {
    let yml = fs::read_to_string(repo_root().join(".github/workflows/release.yml")).unwrap();
    let stamp_at = yml
        .find("scripts/stamp-release-version.sh \"${VERSION}\"")
        .expect("release.yml must run the stamp script");
    let tag_at = yml
        .find("git tag -a \"${TAG}\"")
        .expect("release.yml must tag");
    assert!(
        stamp_at < tag_at,
        "the manifests must be stamped before tagging"
    );
    assert!(yml.contains("git push origin \"refs/tags/${TAG}\"\n"));
    assert!(!yml.contains("git push origin main") && !yml.contains("--force"));
    assert!(
        !yml.contains("|| true\n          git push"),
        "a failed tag push must fail the job"
    );
}
