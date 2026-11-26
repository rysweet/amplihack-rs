//! Issue #2039: when `amplihack cleanup` removes a run's pre-commit target.
//!
//! The specification is "When a target is removed" in
//! `docs/howto/clean-up-after-recipe-runs.md`. A target recorded in the new
//! 5-field form goes only on an event:
//!
//! - (a) run end: the run's own worktree for that checkout is removed in the
//!   same sweep and the origin is `created`;
//! - (b) checkout gone: the recorded checkout no longer exists;
//! - (c) worktree teardown: a worktree removed by cleanup takes the target of
//!   that checkout with it (see also `worktree_target.rs`).
//!
//! It is never removed by age (AC9). Linked targets lose their destination
//! and the link when the destination is under `AMPLIHACK_TARGET_LINK_ROOTS`.

use super::fixture::*;
use std::fs;
use std::path::{Path, PathBuf};

fn reason(entry: &serde_json::Value) -> String {
    entry["reason"].as_str().unwrap_or_default().to_string()
}

fn index_of(report: &serde_json::Value, path: &Path) -> usize {
    let want = path.to_string_lossy();
    entries(report)
        .iter()
        .position(|e| e["path"].as_str() == Some(want.as_ref()))
        .unwrap_or_else(|| panic!("no entry for {want} in {report:#}"))
}

/// The target the hook would use for `checkout`, made cargo-shaped.
fn target_for(fx: &Fx, checkout: &Path) -> PathBuf {
    cargo_target_at(&fx.work.join("targets").join(hook_target_name(checkout)))
}

fn assert_kept(report: &serde_json::Value, target: &Path, want: &str) {
    let e = entry_for(report, target);
    assert_eq!(e["category"], "run-target", "{e:#}");
    assert_eq!(e["action"], "skipped", "{e:#}");
    assert_eq!(reason(&e), want, "{e:#}");
    assert!(target.join("debug/blob").exists(), "{target:?} was deleted");
}

/// D4 (a): at run end the run's merged worktree is removed and its `created`
/// target goes with it, in that order.
#[test]
fn run_end_removes_owned_worktree_then_its_created_target() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-a");
    let tgt = target_for(&fx, &wt);
    fx.gh_closed("feat-a", 2039, 72, true);
    fx.seed_run(
        &SeedRun::ended(72)
            .owns_worktree(&wt, "feat-a")
            .owns_new_target(&tgt, "created", &wt),
    );

    let report = fx.cleanup_json(&["--apply"]);

    assert_eq!(entry_for(&report, &wt)["action"], "removed", "{report:#}");
    let e = entry_for(&report, &tgt);
    assert_eq!(e["category"], "run-target");
    assert_eq!(e["action"], "removed", "{e:#}");
    assert_eq!(reason(&e), "worktree removed by run cleanup");
    assert!(!wt.exists() && !present(&tgt));
    assert!(
        index_of(&report, &wt) < index_of(&report, &tgt),
        "the worktree is processed before its target: {report:#}"
    );
}

/// AC3: a worktree kept for its open PR keeps its target.
#[test]
fn ac3_kept_pr_worktree_keeps_its_target() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-open");
    let tgt = target_for(&fx, &wt);
    fx.gh_open("feat-open", 2039);
    fx.seed_run(
        &SeedRun::ended(72)
            .owns_worktree(&wt, "feat-open")
            .owns_new_target(&tgt, "created", &wt),
    );

    let report = fx.cleanup_json(&["--apply", "--keep-hours", "0"]);

    assert!(wt.exists());
    assert_kept(&report, &tgt, "worktree kept");
}

/// AC4: the main checkout's target is never removed while the checkout is
/// there, whether the run recorded it as `reused` or `created`.
#[test]
fn ac4_reused_and_main_checkout_targets_are_kept() {
    let fx = Fx::new();
    let main_tgt = target_for(&fx, &fx.repo);
    fx.seed_run(&SeedRun::ended(72).owns_new_target(&main_tgt, "reused", &fx.repo));
    let report = fx.cleanup_json(&["--apply", "--keep-hours", "0"]);
    assert_kept(&report, &main_tgt, "reused target; checkout still exists");

    let fx = Fx::new();
    let main_tgt = target_for(&fx, &fx.repo);
    fx.seed_run(&SeedRun::ended(72).owns_new_target(&main_tgt, "created", &fx.repo));
    let report = fx.cleanup_json(&["--apply", "--keep-hours", "0"]);
    assert_kept(&report, &main_tgt, "checkout not owned by this run");
}

/// AC4: a `reused` target of the run's own worktree is not removed by rule
/// (a) while the worktree is kept.
#[test]
fn ac4_reused_target_of_a_kept_worktree_is_kept() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-reuse");
    let tgt = target_for(&fx, &wt);
    fx.gh_open("feat-reuse", 2040);
    fx.seed_run(
        &SeedRun::ended(72)
            .owns_worktree(&wt, "feat-reuse")
            .owns_new_target(&tgt, "reused", &wt),
    );

    let report = fx.cleanup_json(&["--apply", "--keep-hours", "0"]);

    assert!(tgt.join("debug/blob").exists(), "{report:#}");
    assert_eq!(entry_for(&report, &tgt)["action"], "skipped");
}

/// AC5 (D4 b): once the checkout is deleted, the target goes for both
/// origins, without waiting for a PR verdict or a keep period.
#[test]
fn ac5_deleted_checkout_lets_target_go_for_both_origins() {
    let fx = Fx::new();
    let a = fx.worktree("gone-a");
    let b = fx.worktree("gone-b");
    let ta = target_for(&fx, &a);
    let tb = target_for(&fx, &b);
    for wt in [&a, &b] {
        fx.git_ok(
            &fx.repo,
            &["worktree", "remove", "--force", wt.to_str().unwrap()],
        );
    }
    fx.seed_run(
        &SeedRun::ended(0)
            .owns_new_target(&ta, "created", &a)
            .owns_new_target(&tb, "reused", &b),
    );

    let report = fx.cleanup_json(&["--apply", "--keep-hours", "87600"]);

    for t in [&ta, &tb] {
        let e = entry_for(&report, t);
        assert_eq!(e["action"], "removed", "{e:#}");
        assert_eq!(reason(&e), "checkout gone");
        assert!(!present(t));
    }
}

/// AC6 (D5, D7): a linked target whose destination sits on a second disk
/// listed in `AMPLIHACK_TARGET_LINK_ROOTS` loses both destination and link.
#[test]
fn ac6_linked_target_on_a_second_disk_is_removed_with_its_link() {
    let fx = Fx::new();
    let disk = fx.root.join("disk/cargo-targets");
    mkdir_0700(&disk);
    let checkout = fx.root.join("gone-checkout");
    let name = hook_target_name(&checkout);
    let dest = cargo_target_at(&disk.join(&name));
    let link = fx.work.join("targets").join(&name);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&dest, &link).unwrap();
    fx.seed_run(&SeedRun::ended(1).owns_new_target(&link, "created", &checkout));

    // Without the setting the link is reported and left alone: the
    // destination's name is not the host cargo wrapper's layout, so no
    // folder is derived from it.
    let out = fx.cleanup_with(&["--apply", "--format", "json"], |c| {
        c.env_remove("AMPLIHACK_TARGET_LINK_ROOTS");
    });
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let e = entry_for(&report, &link);
    assert_eq!(e["action"], "skipped", "{e:#}");
    assert_eq!(reason(&e), "link not followed: outside allowed roots");
    assert!(dest.join("debug/blob").exists() && present(&link));

    // With it, both go.
    let out = fx.cleanup_with(&["--apply", "--format", "json"], |c| {
        c.env("AMPLIHACK_TARGET_LINK_ROOTS", &disk);
    });
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let e = entry_for(&report, &link);
    assert_eq!(e["action"], "removed", "{e:#}");
    assert_eq!(reason(&e), "checkout gone");
    assert!(!present(&dest), "destination removed");
    assert!(!present(&link), "link removed");
    assert!(disk.is_dir(), "the allowed root itself stays");
}

/// Issue #2039 fix item 1: a link laid out by the host cargo wrapper
/// (`<parent>/<name> -> <disk>/<basename of parent>-<name>-<12 hex>`) is
/// followed with no `AMPLIHACK_TARGET_LINK_ROOTS` set, so the motivating host
/// frees its second-disk targets with default settings.
#[test]
fn wrapper_linked_target_is_removed_without_link_roots() {
    let fx = Fx::new();
    let disk = fx.root.join("d0/cargo-targets");
    mkdir_0700(&disk);
    let checkout = fx.root.join("gone-checkout");
    let name = hook_target_name(&checkout);
    let dest = cargo_target_at(&disk.join(format!("targets-{name}-f5e95e09b016")));
    let link = fx.work.join("targets").join(&name);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&dest, &link).unwrap();
    fx.seed_run(&SeedRun::ended(1).owns_new_target(&link, "created", &checkout));

    let out = fx.cleanup_with(&["--apply", "--format", "json"], |c| {
        c.env_remove("AMPLIHACK_TARGET_LINK_ROOTS");
    });
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let e = entry_for(&report, &link);
    assert_eq!(e["action"], "removed", "{e:#}");
    assert_eq!(reason(&e), "checkout gone");
    assert!(!present(&dest), "destination removed");
    assert!(!present(&link), "link removed");
    assert!(disk.is_dir(), "the folder holding it stays");
}

/// D7: an entry of `AMPLIHACK_TARGET_LINK_ROOTS` that is `$HOME` is ignored,
/// so a link into it is never followed.
#[test]
fn link_roots_entry_equal_to_home_is_ignored() {
    let fx = Fx::new();
    let checkout = fx.root.join("gone-checkout");
    let name = hook_target_name(&checkout);
    let dest = cargo_target_at(&fx.home.join(&name));
    let link = fx.work.join("targets").join(&name);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&dest, &link).unwrap();
    fx.seed_run(&SeedRun::ended(1).owns_new_target(&link, "created", &checkout));

    let out = fx.cleanup_with(&["--apply", "--format", "json"], |c| {
        c.env("AMPLIHACK_TARGET_LINK_ROOTS", &fx.home);
    });
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(dest.join("debug/blob").exists(), "{}", describe(&out));
    assert!(present(&link));
}

/// AC9: a new-format target with no event is kept at any keep period,
/// including zero; age is never the reason.
#[test]
fn ac9_new_format_targets_are_never_removed_by_age() {
    let fx = Fx::new();
    let tgt = target_for(&fx, &fx.repo);
    age_tree(&tgt, 24 * 365);
    fx.seed_run(&SeedRun::ended(24 * 365).owns_new_target(&tgt, "reused", &fx.repo));

    let report = fx.cleanup_json(&["--apply", "--keep-hours", "0"]);

    assert_kept(&report, &tgt, "reused target; checkout still exists");
}

/// D8: a target whose `.cargo-lock` a build holds is reported `in use`.
#[test]
fn held_cargo_lock_keeps_a_run_target() {
    let mut fx = Fx::new();
    let checkout = fx.root.join("gone-checkout");
    let tgt = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&checkout)));
    fx.hold_flock(&tgt.join("debug/.cargo-lock"));
    fx.seed_run(&SeedRun::ended(1).owns_new_target(&tgt, "created", &checkout));

    let report = fx.cleanup_json(&["--apply"]);

    assert_kept(&report, &tgt, "in use: .cargo-lock is held");
}

/// D4: another active run that records the same target keeps it.
#[test]
fn target_recorded_by_an_active_run_is_kept() {
    let fx = Fx::new();
    let checkout = fx.root.join("gone-checkout");
    let tgt = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&checkout)));
    fx.seed_run(&SeedRun::ended(1).owns_new_target(&tgt, "created", &checkout));
    fx.seed_run(&SeedRun::active().owns_new_target(&tgt, "reused", &fx.repo));

    let report = fx.cleanup_json(&["--apply"]);

    assert!(tgt.join("debug/blob").exists(), "{report:#}");
    let want = tgt.to_string_lossy().to_string();
    assert!(
        entries(&report)
            .iter()
            .filter(|e| e["path"].as_str() == Some(want.as_str()))
            .all(|e| e["action"] == "skipped"),
        "{report:#}"
    );
}

/// D4 (c): when cleanup removes a worktree, the target of that checkout goes
/// in the same sweep even when the run never recorded it.
#[test]
fn removing_a_worktree_takes_its_unrecorded_target() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-c");
    let tgt = target_for(&fx, &wt);
    fx.gh_closed("feat-c", 2041, 72, true);
    fx.seed_run(&SeedRun::ended(72).owns_worktree(&wt, "feat-c"));

    let out = fx.cleanup(&["--apply", "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));

    assert!(!wt.exists(), "{}", describe(&out));
    assert!(!present(&tgt), "the removed worktree's target must go too");
}

/// D4 (c): the follow-on removal still checks liveness; a held lock keeps
/// the target and never changes the worktree's own result.
#[test]
fn follow_on_target_removal_respects_a_held_lock() {
    let mut fx = Fx::new();
    let wt = fx.worktree("feat-busy");
    let tgt = target_for(&fx, &wt);
    fx.hold_flock(&tgt.join(".cargo-lock"));
    fx.gh_closed("feat-busy", 2042, 72, true);
    fx.seed_run(&SeedRun::ended(72).owns_worktree(&wt, "feat-busy"));

    let report = fx.cleanup_json(&["--apply"]);

    assert_eq!(entry_for(&report, &wt)["action"], "removed", "{report:#}");
    assert!(tgt.join("debug/blob").exists());
    // The kept target is in the report with its reason, not only in the log.
    assert_kept(&report, &tgt, "in use: .cargo-lock is held");
}

/// Review of PR #2152: a target that rule (c) keeps is shown in the text
/// report too, with its reason.
#[test]
fn follow_on_target_kept_is_in_the_text_report() {
    let mut fx = Fx::new();
    let wt = fx.worktree("feat-busy-text");
    let tgt = target_for(&fx, &wt);
    fx.hold_flock(&tgt.join(".cargo-lock"));
    fx.gh_closed("feat-busy-text", 2043, 72, true);
    fx.seed_run(&SeedRun::ended(72).owns_worktree(&wt, "feat-busy-text"));

    let out = fx.cleanup(&["--apply"]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    let want = tgt.to_string_lossy();
    // The entry line, then its reason on the next line.
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.contains(want.as_ref()))
        .unwrap_or_else(|| panic!("no line for {want}: {}", describe(&out)));
    assert!(lines[at].contains("skipped"), "{}", lines[at]);
    assert_eq!(
        lines.get(at + 1).map(|l| l.trim()),
        Some("reason: in use: .cargo-lock is held"),
        "{text}"
    );
    assert!(tgt.join("debug/blob").exists());
}

/// Review of PR #2152: with `--only run-target` no worktree is removed, so
/// rule (a) cannot apply; the target is kept and the reason says why.
#[test]
fn only_run_target_keeps_a_created_target_and_says_why() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-only-tgt");
    let tgt = target_for(&fx, &wt);
    fx.gh_closed("feat-only-tgt", 2044, 72, true);
    fx.seed_run(
        &SeedRun::ended(72)
            .owns_worktree(&wt, "feat-only-tgt")
            .owns_new_target(&tgt, "created", &wt),
    );

    let report = fx.cleanup_json(&["--apply", "--only", "run-target"]);

    assert!(wt.exists(), "{report:#}");
    assert_kept(&report, &tgt, "worktree kept: run-worktree not selected");
}

/// Review of PR #2152: with `--only run-worktree` there is no rule (a) item,
/// so a `created` record does not stop rule (c) from taking the target of the
/// worktree it removes.
#[test]
fn only_run_worktree_takes_a_created_target_by_rule_c() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-only-wt");
    let tgt = target_for(&fx, &wt);
    fx.gh_closed("feat-only-wt", 2045, 72, true);
    fx.seed_run(
        &SeedRun::ended(72)
            .owns_worktree(&wt, "feat-only-wt")
            .owns_new_target(&tgt, "created", &wt),
    );

    let report = fx.cleanup_json(&["--apply", "--only", "run-worktree"]);

    assert_eq!(entry_for(&report, &wt)["action"], "removed", "{report:#}");
    let e = entry_for(&report, &tgt);
    assert_eq!(e["action"], "removed", "{e:#}");
    assert_eq!(reason(&e), "worktree removed");
    assert!(!present(&tgt));
}

/// Review of PR #2152: a `created` target refused at plan time has no rule
/// (a) item, so rule (c) checks it again after the worktree goes. Here the
/// lock is still held, so it is kept, and it is reported once.
#[test]
fn created_target_refused_at_plan_time_is_checked_by_rule_c_and_reported_once() {
    let mut fx = Fx::new();
    let wt = fx.worktree("feat-plan-refused");
    let tgt = target_for(&fx, &wt);
    fx.hold_flock(&tgt.join("debug/.cargo-lock"));
    fx.gh_closed("feat-plan-refused", 2046, 72, true);
    fx.seed_run(
        &SeedRun::ended(72)
            .owns_worktree(&wt, "feat-plan-refused")
            .owns_new_target(&tgt, "created", &wt),
    );

    let report = fx.cleanup_json(&["--apply"]);

    assert_eq!(entry_for(&report, &wt)["action"], "removed", "{report:#}");
    let want = tgt.to_string_lossy().to_string();
    let found = entries(&report)
        .iter()
        .filter(|e| e["path"].as_str() == Some(want.as_str()))
        .count();
    assert_eq!(found, 1, "one entry for the target: {report:#}");
    assert_kept(&report, &tgt, "in use: .cargo-lock is held");
}

/// Dry run: a target that would go is reported `would-remove`, nothing is
/// deleted.
#[test]
fn dry_run_reports_without_deleting() {
    let fx = Fx::new();
    let checkout = fx.root.join("gone-checkout");
    let tgt = cargo_target_at(&fx.work.join("targets").join(hook_target_name(&checkout)));
    fx.seed_run(&SeedRun::ended(1).owns_new_target(&tgt, "created", &checkout));

    let report = fx.cleanup_json(&[]);

    let e = entry_for(&report, &tgt);
    assert_eq!(e["action"], "would-remove", "{e:#}");
    assert_eq!(reason(&e), "checkout gone");
    assert!(tgt.join("debug/blob").exists());
}
