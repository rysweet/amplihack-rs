//! Unit tests for `targets.rs` (issue #1808, C6, AC6).
//!
//! Wire with, at the end of `targets.rs`:
//! `#[cfg(test)] #[path = "tests_targets.rs"] mod tests;`
//!
//! Contract:
//! - `pub(crate) const CACHEDIR_TAG: &str` starts with the standard signature
//!   line and mentions amplihack;
//! - `pub(crate) fn create_tagged_target_dir(path: &Path) -> std::io::Result<()>`
//!   creates the folder (and parents) and writes `CACHEDIR.TAG`; calling it
//!   again is harmless and never truncates other content;
//! - `pub(crate) fn is_tagged(path: &Path) -> bool` is true only when
//!   `CACHEDIR.TAG` is a regular file starting with the signature.

use super::*;
use std::fs;

const SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

#[test]
fn tag_constant_starts_with_standard_signature() {
    assert!(CACHEDIR_TAG.starts_with(SIGNATURE));
    assert!(CACHEDIR_TAG.contains("amplihack"));
    assert!(CACHEDIR_TAG.ends_with('\n'));
}

#[test]
fn create_tagged_target_dir_writes_standard_signature() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("a/b/target");
    create_tagged_target_dir(&target).unwrap();
    let tag = fs::read_to_string(target.join("CACHEDIR.TAG")).unwrap();
    assert_eq!(tag, CACHEDIR_TAG);
    assert!(is_tagged(&target));
}

#[test]
fn create_tagged_target_dir_is_idempotent_and_keeps_contents() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    create_tagged_target_dir(&target).unwrap();
    fs::write(target.join("keep"), "x").unwrap();
    create_tagged_target_dir(&target).unwrap();
    assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "x");
    assert!(is_tagged(&target));
}

#[test]
fn is_tagged_rejects_missing_wrong_or_symlinked_tags() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing");
    fs::create_dir(&missing).unwrap();
    assert!(!is_tagged(&missing));

    let wrong = tmp.path().join("wrong");
    fs::create_dir(&wrong).unwrap();
    fs::write(wrong.join("CACHEDIR.TAG"), "Signature: nope\n").unwrap();
    assert!(!is_tagged(&wrong));

    let real = tmp.path().join("real-tag");
    fs::write(&real, CACHEDIR_TAG).unwrap();
    let linked = tmp.path().join("linked");
    fs::create_dir(&linked).unwrap();
    std::os::unix::fs::symlink(&real, linked.join("CACHEDIR.TAG")).unwrap();
    assert!(!is_tagged(&linked), "a symlinked tag is not trusted");
}

// ----- issue #2039: target identity ------------------------------------------
//
// Contract (D1, D7(f)):
// - `pub(crate) fn is_cargo_target(dir: &Path) -> bool`: true when `dir` is a
//   real folder (not a link) holding a valid `CACHEDIR.TAG`, a regular
//   `.rustc_info.json`, or a `<profile>/.fingerprint` folder or
//   `<profile>/.cargo-lock` file. Never follows a link, at any level.
// - `pub(crate) fn checkout_hash(checkout: &Path) -> Option<String>`: the first
//   12 lowercase hex characters of sha256 over the canonical checkout path,
//   with no trailing newline (the hook's `printf '%s' "$top" | sha256sum`).
//   For a checkout that no longer exists the input is
//   `canonicalize(parent).join(file_name)`. `None` when the parent cannot be
//   resolved or the path has no file name.
// - `pub(crate) fn target_name(checkout: &Path) -> Option<String>`:
//   `amplihack-precommit-target-<checkout_hash>`, matching
//   `^amplihack-precommit-target-[0-9a-f]{12}$`.

use std::os::unix::fs::symlink;
use std::path::PathBuf;

/// The hook's own computation, run through the same coreutils tool.
fn shell_hash(path: &Path) -> String {
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("printf '%s' \"$1\" | sha256sum | cut -c1-12")
        .arg("sh")
        .arg(path)
        .output()
        .expect("run sha256sum");
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn canonical_tmp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    (tmp, root)
}

fn is_target_name(name: &str) -> bool {
    name.strip_prefix("amplihack-precommit-target-")
        .is_some_and(|h| h.len() == 12 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

#[test]
fn is_cargo_target_accepts_each_cargo_marker() {
    let (_tmp, root) = canonical_tmp();

    let tagged = root.join("tagged");
    create_tagged_target_dir(&tagged).unwrap();
    assert!(is_cargo_target(&tagged), "CACHEDIR.TAG");

    let info = root.join("info");
    fs::create_dir(&info).unwrap();
    fs::write(info.join(".rustc_info.json"), "{}").unwrap();
    assert!(is_cargo_target(&info), ".rustc_info.json");

    let fp = root.join("fp");
    fs::create_dir_all(fp.join("debug/.fingerprint")).unwrap();
    assert!(is_cargo_target(&fp), "<profile>/.fingerprint");

    let lock = root.join("lock");
    fs::create_dir_all(lock.join("release")).unwrap();
    fs::write(lock.join("release/.cargo-lock"), "").unwrap();
    assert!(is_cargo_target(&lock), "<profile>/.cargo-lock");
}

#[test]
fn is_cargo_target_rejects_plain_folders_and_files() {
    let (_tmp, root) = canonical_tmp();
    let empty = root.join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(!is_cargo_target(&empty));

    let docs = root.join("docs");
    fs::create_dir_all(docs.join("src")).unwrap();
    fs::write(docs.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(docs.join("Cargo.toml"), "[package]").unwrap();
    assert!(!is_cargo_target(&docs), "a crate is not a target folder");

    let file = root.join("file");
    fs::write(&file, "x").unwrap();
    assert!(!is_cargo_target(&file));
    assert!(!is_cargo_target(&root.join("missing")));

    let wrong_tag = root.join("wrong-tag");
    fs::create_dir(&wrong_tag).unwrap();
    fs::write(wrong_tag.join("CACHEDIR.TAG"), "Signature: nope\n").unwrap();
    assert!(!is_cargo_target(&wrong_tag));
}

#[test]
fn is_cargo_target_never_follows_links() {
    let (_tmp, root) = canonical_tmp();
    let real = root.join("real");
    fs::create_dir_all(real.join("debug/.fingerprint")).unwrap();
    fs::write(real.join(".rustc_info.json"), "{}").unwrap();
    create_tagged_target_dir(&real).unwrap();

    let link = root.join("link");
    symlink(&real, &link).unwrap();
    assert!(!is_cargo_target(&link), "the folder itself is a link");

    // Each marker present only as a link.
    let linked_info = root.join("linked-info");
    fs::create_dir(&linked_info).unwrap();
    symlink(
        real.join(".rustc_info.json"),
        linked_info.join(".rustc_info.json"),
    )
    .unwrap();
    assert!(!is_cargo_target(&linked_info), ".rustc_info.json link");

    let linked_profile = root.join("linked-profile");
    fs::create_dir(&linked_profile).unwrap();
    symlink(real.join("debug"), linked_profile.join("debug")).unwrap();
    assert!(!is_cargo_target(&linked_profile), "profile folder link");

    let linked_fp = root.join("linked-fp");
    fs::create_dir_all(linked_fp.join("debug")).unwrap();
    symlink(
        real.join("debug/.fingerprint"),
        linked_fp.join("debug/.fingerprint"),
    )
    .unwrap();
    assert!(!is_cargo_target(&linked_fp), ".fingerprint link");

    let linked_lock = root.join("linked-lock");
    fs::create_dir_all(linked_lock.join("debug")).unwrap();
    fs::write(root.join("lockfile"), "").unwrap();
    symlink(root.join("lockfile"), linked_lock.join("debug/.cargo-lock")).unwrap();
    assert!(!is_cargo_target(&linked_lock), ".cargo-lock link");

    let linked_tag = root.join("linked-tag");
    fs::create_dir(&linked_tag).unwrap();
    symlink(real.join("CACHEDIR.TAG"), linked_tag.join("CACHEDIR.TAG")).unwrap();
    assert!(!is_cargo_target(&linked_tag), "CACHEDIR.TAG link");
}

#[test]
fn checkout_hash_matches_the_hook_for_an_existing_checkout() {
    let (_tmp, root) = canonical_tmp();
    let checkout = root.join("repo");
    fs::create_dir(&checkout).unwrap();
    let hash = checkout_hash(&checkout).expect("hash");
    assert_eq!(hash, shell_hash(&checkout));
    assert_eq!(hash.len(), 12);

    // Reached through a link, the canonical path is hashed, as git prints it.
    let link = root.join("via-link");
    symlink(&checkout, &link).unwrap();
    assert_eq!(checkout_hash(&link).as_deref(), Some(hash.as_str()));
}

#[test]
fn checkout_hash_of_a_removed_checkout_uses_the_canonical_parent() {
    let (_tmp, root) = canonical_tmp();
    let parent = root.join("worktrees/repo-1a2b3c4d");
    fs::create_dir_all(&parent).unwrap();
    let gone = parent.join("feat-x");
    fs::create_dir(&gone).unwrap();
    let before = checkout_hash(&gone).unwrap();
    fs::remove_dir(&gone).unwrap();

    assert_eq!(
        checkout_hash(&gone).as_deref(),
        Some(before.as_str()),
        "the hash must not change when the checkout is removed"
    );
    assert_eq!(checkout_hash(&gone).unwrap(), shell_hash(&gone));

    let parent_link = root.join("wt-link");
    symlink(&parent, &parent_link).unwrap();
    assert_eq!(
        checkout_hash(&parent_link.join("feat-x")).as_deref(),
        Some(before.as_str()),
        "a linked parent resolves to the same canonical input"
    );
}

#[test]
fn checkout_hash_is_none_when_the_parent_does_not_resolve() {
    let (_tmp, root) = canonical_tmp();
    assert_eq!(checkout_hash(&root.join("no-such-parent/checkout")), None);
    let dangling = root.join("dangling-parent");
    symlink(root.join("nowhere"), &dangling).unwrap();
    assert_eq!(checkout_hash(&dangling.join("checkout")), None);
    assert_eq!(checkout_hash(Path::new("/")), None, "no file name");
}

#[test]
fn target_name_is_prefix_plus_checkout_hash() {
    let (_tmp, root) = canonical_tmp();
    let checkout = root.join("repo");
    fs::create_dir(&checkout).unwrap();
    let name = target_name(&checkout).unwrap();
    assert!(is_target_name(&name), "{name}");
    assert_eq!(
        name,
        format!("amplihack-precommit-target-{}", shell_hash(&checkout))
    );
    assert!(!name.contains("-run-"), "no run suffix: {name}");
    assert_eq!(target_name(&root.join("missing/x")), None);
}

#[test]
fn checkout_hash_handles_unusual_path_bytes() {
    let (_tmp, root) = canonical_tmp();
    for name in ["sp ace", "q'uote", "$(touch pwned)", "-e", "%s"] {
        let checkout = root.join(name);
        fs::create_dir(&checkout).unwrap();
        assert_eq!(
            checkout_hash(&checkout).unwrap(),
            shell_hash(&checkout),
            "{name}"
        );
        assert!(is_target_name(&target_name(&checkout).unwrap()), "{name}");
    }
    assert!(!root.join("pwned").exists());
}
