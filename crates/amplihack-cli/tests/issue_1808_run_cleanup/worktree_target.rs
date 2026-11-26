//! Issue #2039 (D6): `amplihack cleanup --worktree-target <checkout>`, the
//! single entry point that removes the pre-commit target of a worktree that
//! was just removed.
//!
//! Contract (from "Remove the target of a removed worktree" in
//! `docs/howto/clean-up-after-recipe-runs.md`):
//! - refuses with exit 1 and exactly one stderr line when the path is
//!   relative, still exists (including a dangling link at that path), or has
//!   a parent that cannot be resolved;
//! - otherwise looks for `<work>/targets/<name>` and `<hook base>/<name>`,
//!   where `<name>` is the hook's name for that checkout, and removes each
//!   under the usual checks (owned, cargo-shaped, lock free, D7 for links);
//! - keeps a target another active run records;
//! - reports category `run-target`, reason `worktree removed`;
//! - without `--apply` reports `would-remove` and deletes nothing;
//! - exit 0 unless an entry failed.

use super::fixture::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

fn worktree_target(fx: &Fx, args: &[&str], checkout: &Path) -> Output {
    let mut all: Vec<&str> = args.to_vec();
    all.push("--worktree-target");
    let path = checkout.to_str().unwrap().to_string();
    fx.cleanup_with(&all, |c| {
        c.arg(&path);
    })
}

fn json(out: &Output) -> serde_json::Value {
    assert_eq!(out.status.code(), Some(0), "{}", describe(out));
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("JSON did not parse ({e}): {}", describe(out)))
}

fn assert_refused(out: &Output, want: &str) {
    assert_eq!(out.status.code(), Some(1), "{}", describe(out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines, vec![want], "{}", describe(out));
}

/// A worktree that existed and was removed with `git worktree remove`.
fn removed_worktree(fx: &Fx, branch: &str) -> PathBuf {
    let wt = fx.worktree(branch);
    fx.git_ok(
        &fx.repo,
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
    );
    assert!(!present(&wt));
    wt
}

#[test]
fn refuses_a_relative_path() {
    let fx = Fx::new();
    let out = fx.cleanup(&["--apply", "--worktree-target", "worktrees/feat"]);
    assert_refused(
        &out,
        "amplihack cleanup: --worktree-target needs an absolute path: worktrees/feat",
    );
}

#[test]
fn refuses_a_checkout_that_still_exists() {
    let fx = Fx::new();
    let wt = fx.worktree("still-here");
    let tgt = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&wt)));
    let out = worktree_target(&fx, &["--apply"], &wt);
    assert_refused(
        &out,
        &format!(
            "amplihack cleanup: --worktree-target: checkout still exists: {}",
            wt.display()
        ),
    );
    assert!(tgt.join("debug/blob").exists());

    // The main checkout is an existing path too.
    let main_tgt = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&fx.repo)));
    let out = worktree_target(&fx, &["--apply"], &fx.repo);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(main_tgt.join("debug/blob").exists());
}

#[test]
fn refuses_a_dangling_link_at_the_checkout_path() {
    let fx = Fx::new();
    let at = fx.root.join("dangling-checkout");
    std::os::unix::fs::symlink(fx.root.join("nowhere"), &at).unwrap();
    let out = worktree_target(&fx, &["--apply"], &at);
    assert_refused(
        &out,
        &format!(
            "amplihack cleanup: --worktree-target: checkout still exists: {}",
            at.display()
        ),
    );
}

#[test]
fn refuses_a_path_whose_parent_cannot_be_resolved() {
    let fx = Fx::new();
    let at = fx.root.join("no-such-parent/feat");
    let out = worktree_target(&fx, &["--apply"], &at);
    assert_refused(
        &out,
        &format!(
            "amplihack cleanup: --worktree-target: cannot resolve the parent of {}",
            at.display()
        ),
    );
}

/// Restores a folder's mode on drop, so a failed test can still be cleaned.
struct ModeGuard(PathBuf, u32);

impl Drop for ModeGuard {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(self.1));
    }
}

/// SR6: only "not found" means the checkout is gone. A checkout that cannot
/// be examined, here because its parent is mode 000, is refused and its
/// target is kept.
#[test]
fn refuses_a_checkout_that_cannot_be_examined() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root ignores mode 000");
        return;
    }
    let fx = Fx::new();
    let locked = fx.root.join("locked");
    let wt = locked.join("feat");
    fs::create_dir_all(&wt).unwrap();
    let tgt = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&wt)));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = ModeGuard(locked.clone(), 0o755);

    let out = worktree_target(&fx, &["--apply"], &wt);

    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!(
            "--worktree-target: cannot examine checkout {}",
            wt.display()
        )),
        "{}",
        describe(&out)
    );
    assert!(tgt.join("debug/blob").exists(), "the target must be kept");
}

#[test]
fn removes_the_target_of_a_removed_worktree() {
    let fx = Fx::new();
    let wt = removed_worktree(&fx, "gone-real");
    let tgt = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&wt)));
    let other = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&fx.repo)));

    let report = json(&worktree_target(&fx, &["--apply", "--format", "json"], &wt));

    let e = entry_for(&report, &tgt);
    assert_eq!(e["category"], "run-target", "{e:#}");
    assert_eq!(e["action"], "removed", "{e:#}");
    assert_eq!(e["reason"], "worktree removed", "{e:#}");
    assert!(!present(&tgt));
    assert!(
        other.join("debug/blob").exists(),
        "only that checkout's target"
    );
    assert_eq!(entries(&report).len(), 1, "{report:#}");
}

#[test]
fn removes_the_target_under_the_hook_cache_base() {
    let fx = Fx::new();
    let wt = removed_worktree(&fx, "gone-xdg");
    let tgt = cargo_target_at(&fx.home.join(".cache/amplihack").join(hook_target_name(&wt)));

    let report = json(&worktree_target(&fx, &["--apply", "--format", "json"], &wt));

    assert_eq!(entry_for(&report, &tgt)["action"], "removed", "{report:#}");
    assert!(!present(&tgt));
}

#[test]
fn removes_a_linked_target_and_its_destination() {
    let fx = Fx::new();
    let wt = removed_worktree(&fx, "gone-link");
    let disk = fx.root.join("disk/cargo-targets");
    mkdir_0700(&disk);
    let name = hook_target_name(&wt);
    let dest = cargo_target_at(&disk.join(&name));
    let link = fx.work.join("targets").join(&name);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&dest, &link).unwrap();

    let out = fx.cleanup_with(
        &[
            "--apply",
            "--format",
            "json",
            "--worktree-target",
            wt.to_str().unwrap(),
        ],
        |c| {
            c.env("AMPLIHACK_TARGET_LINK_ROOTS", &disk);
        },
    );
    let report = json(&out);

    assert_eq!(entry_for(&report, &link)["action"], "removed", "{report:#}");
    assert!(!present(&dest) && !present(&link));
}

#[test]
fn linked_target_outside_allowed_roots_is_reported_and_kept() {
    let fx = Fx::new();
    let wt = removed_worktree(&fx, "gone-link-out");
    let name = hook_target_name(&wt);
    let dest = cargo_target_at(&fx.root.join("elsewhere").join(&name));
    let link = fx.work.join("targets").join(&name);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&dest, &link).unwrap();

    let out = fx.cleanup_with(
        &[
            "--apply",
            "--format",
            "json",
            "--worktree-target",
            wt.to_str().unwrap(),
        ],
        |c| {
            c.env_remove("AMPLIHACK_TARGET_LINK_ROOTS");
        },
    );
    let report = json(&out);

    let e = entry_for(&report, &link);
    assert_eq!(e["action"], "skipped", "{e:#}");
    assert_eq!(e["reason"], "link not followed: outside allowed roots");
    assert!(dest.join("debug/blob").exists() && present(&link));
}

#[test]
fn removes_a_dangling_link_at_the_target_path() {
    let fx = Fx::new();
    let wt = removed_worktree(&fx, "gone-dangling");
    let link = fx.work.join("targets").join(hook_target_name(&wt));
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    let neighbour = fx.root.join("neighbour");
    fs::create_dir_all(&neighbour).unwrap();
    fs::write(neighbour.join("keep"), "x").unwrap();
    std::os::unix::fs::symlink(neighbour.join("missing"), &link).unwrap();

    let report = json(&worktree_target(&fx, &["--apply", "--format", "json"], &wt));

    assert_eq!(entry_for(&report, &link)["action"], "removed", "{report:#}");
    assert!(!present(&link));
    assert!(neighbour.join("keep").exists());
}

#[test]
fn keeps_a_target_an_active_run_records() {
    let fx = Fx::new();
    let wt = removed_worktree(&fx, "gone-active");
    let tgt = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&wt)));
    fx.seed_run(&SeedRun::active().owns_new_target(&tgt, "created", &wt));

    let report = json(&worktree_target(&fx, &["--apply", "--format", "json"], &wt));

    assert_eq!(entry_for(&report, &tgt)["action"], "skipped", "{report:#}");
    assert!(tgt.join("debug/blob").exists());
}

#[test]
fn keeps_a_target_whose_cargo_lock_is_held() {
    let mut fx = Fx::new();
    let wt = removed_worktree(&fx, "gone-busy");
    let tgt = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&wt)));
    fx.hold_flock(&tgt.join("debug/.cargo-lock"));

    let report = json(&worktree_target(&fx, &["--apply", "--format", "json"], &wt));

    let e = entry_for(&report, &tgt);
    assert_eq!(e["action"], "skipped", "{e:#}");
    assert_eq!(e["reason"], "in use: .cargo-lock is held");
    assert!(tgt.join("debug/blob").exists());
}

#[test]
fn refuses_a_folder_that_is_not_cargo_shaped() {
    let fx = Fx::new();
    let wt = removed_worktree(&fx, "gone-odd");
    let odd = fx.work.join("targets").join(hook_target_name(&wt));
    fs::create_dir_all(odd.join("notes")).unwrap();
    fs::write(odd.join("notes/keep.txt"), "keep").unwrap();

    let out = worktree_target(&fx, &["--apply", "--format", "json"], &wt);

    assert!(odd.join("notes/keep.txt").exists(), "{}", describe(&out));
}

#[test]
fn dry_run_reports_would_remove_and_deletes_nothing() {
    let fx = Fx::new();
    let wt = removed_worktree(&fx, "gone-dry");
    let tgt = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&wt)));

    let report = json(&worktree_target(&fx, &["--format", "json"], &wt));
    let e = entry_for(&report, &tgt);
    assert_eq!(e["action"], "would-remove", "{e:#}");
    assert_eq!(e["reason"], "worktree removed");
    assert!(tgt.join("debug/blob").exists());

    let out = worktree_target(&fx, &[], &wt);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("run-target") && text.contains("would-remove"),
        "{text}"
    );
    assert!(tgt.join("debug/blob").exists());
}

#[test]
fn nothing_to_remove_is_a_quiet_success() {
    let fx = Fx::new();
    let wt = removed_worktree(&fx, "gone-empty");
    let report = json(&worktree_target(&fx, &["--apply", "--format", "json"], &wt));
    assert!(entries(&report).is_empty(), "{report:#}");
}
