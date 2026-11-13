//! Issue #2528: `copy_bundle` and `LinkPolicy` in `trusted.rs`.
//!
//! Wire with, at the end of `trusted.rs`:
//! `#[cfg(all(test, unix))] #[path = "tests_copy_bundle.rs"] mod tests_copy_bundle;`
//!
//! Contract (docs/reference/recipe-run-bundle.md, "Trust checks"):
//! - `copy_bundle(source, dest, LinkPolicy)` checks and copies each entry in
//!   one pass. `dest` must not exist; it is created 0755.
//! - Files with any execute bit become 0755, others 0644; directories 0755;
//!   setuid, setgid and sticky bits are dropped.
//! - `LinkPolicy::Skip` skips every symlink (auto-drive). `KeepInternal` keeps
//!   relative links whose canonical target stays inside the bundle, recreated
//!   with the same text and never descended through; every other link is
//!   refused with E7.
//! - The two merge-guard names are skipped under both policies.
//! - E6: `untrusted bundle entry <path>: mode 0664 (group or other writable);
//!   fix with chmod go-w <path>`.
//! - E7: `untrusted bundle entry <path>: <reason>`.
//! - `trusted_directory` is `pub(crate)`.

use super::{LinkPolicy, copy_bundle, copy_tree, trusted_directory};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

/// A trusted bundle: `recipes/a.yaml`, `tools/run.sh` (exec), `agents/x.md`.
fn bundle() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let src = root.join("amplifier-bundle");
    for dir in ["recipes", "tools", "agents"] {
        fs::create_dir_all(src.join(dir)).unwrap();
        set_mode(&src.join(dir), 0o755);
    }
    fs::write(src.join("recipes/a.yaml"), "name: a\n").unwrap();
    fs::write(src.join("tools/run.sh"), "#!/bin/sh\n").unwrap();
    fs::write(src.join("agents/x.md"), "# x\n").unwrap();
    set_mode(&src.join("recipes/a.yaml"), 0o644);
    set_mode(&src.join("tools/run.sh"), 0o755);
    set_mode(&src.join("agents/x.md"), 0o644);
    set_mode(&src, 0o755);
    let dest = root.join("copy");
    (tmp, src, dest)
}

fn err_text(result: anyhow::Result<()>) -> String {
    format!("{:#}", result.expect_err("expected the copy to be refused"))
}

#[test]
fn trusted_directory_is_crate_visible_and_checks_modes() {
    let (_tmp, src, _dest) = bundle();
    trusted_directory(&src).unwrap();
    set_mode(&src, 0o775);
    assert!(trusted_directory(&src).is_err());
}

#[test]
fn copy_sets_explicit_modes() {
    let (_tmp, src, dest) = bundle();
    // Group-executable only: any execute bit makes the copy 0755.
    set_mode(&src.join("agents/x.md"), 0o654);
    copy_bundle(&src, &dest, LinkPolicy::KeepInternal).unwrap();
    assert_eq!(mode_of(&dest), 0o755);
    assert_eq!(mode_of(&dest.join("tools")), 0o755);
    assert_eq!(mode_of(&dest.join("tools/run.sh")), 0o755);
    assert_eq!(mode_of(&dest.join("recipes/a.yaml")), 0o644);
    assert_eq!(mode_of(&dest.join("agents/x.md")), 0o755);
    assert_eq!(
        fs::read_to_string(dest.join("recipes/a.yaml")).unwrap(),
        "name: a\n"
    );
}

#[test]
fn copy_strips_setuid_setgid_and_sticky_bits() {
    let (_tmp, src, dest) = bundle();
    set_mode(&src.join("tools/run.sh"), 0o6755);
    set_mode(&src.join("tools"), 0o1755);
    copy_bundle(&src, &dest, LinkPolicy::KeepInternal).unwrap();
    assert_eq!(mode_of(&dest.join("tools/run.sh")), 0o755);
    assert_eq!(mode_of(&dest.join("tools")), 0o755);
}

#[test]
fn group_writable_file_error_names_path_and_mode() {
    let (_tmp, src, dest) = bundle();
    let bad = src.join("recipes/a.yaml");
    set_mode(&bad, 0o664);
    assert_eq!(
        err_text(copy_bundle(&src, &dest, LinkPolicy::KeepInternal)),
        format!(
            "untrusted bundle entry {p}: mode 0664 (group or other writable); \
             fix with chmod go-w {p}",
            p = bad.display()
        )
    );
}

#[test]
fn other_writable_directory_is_refused() {
    let (_tmp, src, dest) = bundle();
    set_mode(&src.join("agents"), 0o757);
    let text = err_text(copy_bundle(&src, &dest, LinkPolicy::Skip));
    assert!(text.contains("mode 0757"), "{text}");
    assert!(
        text.contains(&src.join("agents").display().to_string()),
        "{text}"
    );
}

#[test]
fn a_fifo_is_refused_as_an_unsupported_entry() {
    let (_tmp, src, dest) = bundle();
    let fifo = src.join("tools/pipe");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: a valid C string for a path in our own temp dir.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
    assert_eq!(
        err_text(copy_bundle(&src, &dest, LinkPolicy::KeepInternal)),
        format!(
            "untrusted bundle entry {}: not a regular file, directory or symlink",
            fifo.display()
        )
    );
}

#[test]
fn merge_guard_names_are_skipped_under_both_policies() {
    for policy in [LinkPolicy::Skip, LinkPolicy::KeepInternal] {
        let (_tmp, src, dest) = bundle();
        fs::create_dir(src.join("tools/gh-merge-guard")).unwrap();
        set_mode(&src.join("tools/gh-merge-guard"), 0o755);
        fs::write(src.join("tools/autodrive_merge_gate.sh"), "#!/bin/sh\n").unwrap();
        set_mode(&src.join("tools/autodrive_merge_gate.sh"), 0o755);
        copy_bundle(&src, &dest, policy).unwrap();
        assert!(!dest.join("tools/gh-merge-guard").exists());
        assert!(!dest.join("tools/autodrive_merge_gate.sh").exists());
        assert!(dest.join("tools/run.sh").is_file());
    }
}

// ------------------------------------------------------------- symlinks

#[test]
fn skip_policy_still_skips_every_link() {
    let (_tmp, src, dest) = bundle();
    symlink("a.yaml", src.join("recipes/alias.yaml")).unwrap();
    symlink("/etc/hostname", src.join("tools/abs")).unwrap();
    copy_bundle(&src, &dest, LinkPolicy::Skip).unwrap();
    assert!(fs::symlink_metadata(dest.join("recipes/alias.yaml")).is_err());
    assert!(fs::symlink_metadata(dest.join("tools/abs")).is_err());
}

#[test]
fn copy_tree_keeps_auto_drive_behaviour() {
    let (_tmp, src, dest) = bundle();
    symlink("a.yaml", src.join("recipes/alias.yaml")).unwrap();
    copy_tree(&src, &dest).unwrap();
    assert!(dest.join("recipes/a.yaml").is_file());
    assert!(fs::symlink_metadata(dest.join("recipes/alias.yaml")).is_err());
}

#[test]
fn internal_relative_link_is_kept_with_the_same_text() {
    let (_tmp, src, dest) = bundle();
    symlink("../recipes/a.yaml", src.join("tools/a-link.yaml")).unwrap();
    copy_bundle(&src, &dest, LinkPolicy::KeepInternal).unwrap();
    let link = dest.join("tools/a-link.yaml");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        PathBuf::from("../recipes/a.yaml")
    );
    assert_eq!(fs::read_to_string(&link).unwrap(), "name: a\n");
}

#[test]
fn absolute_link_is_refused() {
    let (_tmp, src, dest) = bundle();
    let link = src.join("tools/abs");
    // Absolute even when it points inside the bundle.
    symlink(src.join("recipes/a.yaml"), &link).unwrap();
    assert_eq!(
        err_text(copy_bundle(&src, &dest, LinkPolicy::KeepInternal)),
        format!(
            "untrusted bundle entry {}: absolute symlink",
            link.display()
        )
    );
}

#[test]
fn dotdot_escape_is_refused() {
    let (_tmp, src, dest) = bundle();
    fs::write(src.parent().unwrap().join("outside.sh"), "#!/bin/sh\n").unwrap();
    let link = src.join("tools/escape.sh");
    symlink("../../outside.sh", &link).unwrap();
    assert_eq!(
        err_text(copy_bundle(&src, &dest, LinkPolicy::KeepInternal)),
        format!(
            "untrusted bundle entry {}: symlink leaves the bundle",
            link.display()
        )
    );
}

#[test]
fn keep_internal_refuses_sibling_prefix_escape() {
    let (_tmp, src, dest) = bundle();
    let sibling = src.parent().unwrap().join("amplifier-bundle-old");
    fs::create_dir(&sibling).unwrap();
    set_mode(&sibling, 0o755);
    fs::write(sibling.join("t.sh"), "#!/bin/sh\n").unwrap();
    set_mode(&sibling.join("t.sh"), 0o755);
    let link = src.join("tools/old.sh");
    symlink("../../amplifier-bundle-old/t.sh", &link).unwrap();
    let text = err_text(copy_bundle(&src, &dest, LinkPolicy::KeepInternal));
    assert!(text.ends_with("symlink leaves the bundle"), "{text}");
}

#[test]
fn keep_internal_refuses_dangling_and_cyclic_links() {
    let (_tmp, src, dest) = bundle();
    let dangling = src.join("tools/gone");
    symlink("missing.sh", &dangling).unwrap();
    assert_eq!(
        err_text(copy_bundle(&src, &dest, LinkPolicy::KeepInternal)),
        format!(
            "untrusted bundle entry {}: dangling symlink",
            dangling.display()
        )
    );
    fs::remove_file(&dangling).unwrap();
    let _ = fs::remove_dir_all(&dest);

    let a = src.join("tools/loop-a");
    symlink("loop-b", &a).unwrap();
    symlink("loop-a", src.join("tools/loop-b")).unwrap();
    let text = err_text(copy_bundle(&src, &dest, LinkPolicy::KeepInternal));
    assert!(text.ends_with(": symlink cycle"), "{text}");
}

#[test]
fn keep_internal_does_not_descend_through_dir_link() {
    let (_tmp, src, dest) = bundle();
    symlink("../recipes", src.join("tools/recipes-alias")).unwrap();
    copy_bundle(&src, &dest, LinkPolicy::KeepInternal).unwrap();
    let alias = dest.join("tools/recipes-alias");
    assert!(
        fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // The real directory is copied once; the alias is a link, not a copy.
    assert!(dest.join("recipes/a.yaml").is_file());
}

#[test]
fn keep_internal_refuses_a_link_whose_target_is_group_writable() {
    let (_tmp, src, dest) = bundle();
    fs::create_dir(src.join("shared")).unwrap();
    set_mode(&src.join("shared"), 0o775);
    symlink("../shared", src.join("tools/shared")).unwrap();
    let text = err_text(copy_bundle(&src, &dest, LinkPolicy::KeepInternal));
    assert!(text.contains("mode 0775"), "{text}");
}

#[test]
fn a_symlinked_bundle_root_is_refused() {
    let (_tmp, src, dest) = bundle();
    let alias = src.parent().unwrap().join("alias");
    symlink(&src, &alias).unwrap();
    assert!(copy_bundle(&alias, &dest, LinkPolicy::KeepInternal).is_err());
}

#[test]
fn an_existing_destination_is_refused() {
    let (_tmp, src, dest) = bundle();
    fs::create_dir(&dest).unwrap();
    assert!(copy_bundle(&src, &dest, LinkPolicy::KeepInternal).is_err());
}
