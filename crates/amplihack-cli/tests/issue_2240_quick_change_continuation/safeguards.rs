use std::fs;

use serde_json::json;

use super::fixture::{BRANCH, Fixture, SUBJECT, stderr, success};
use super::publication::{commit, prepare};
use crate::support::exec_stub::write_executable;

#[test]
fn oversized_commit_hook_changes_stop_continuation_before_push() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    write_executable(
        &f.caller.join(".git/hooks/pre-commit"),
        r#"#!/bin/bash
printf 'pre-commit\n' >> "$HOME/hook-calls"
for ((i=0; i<151; i++)); do printf 'hook-generated review notes\n'; done > notes.md
git add -- notes.md
"#,
        0o755,
    );
    prepare(&mut f, SUBJECT);
    let remote = f.git(&f.origin, &["rev-parse", BRANCH]);
    let out = f.qc("commit");
    assert!(!out.status.success(), "oversized hook change accepted");
    assert!(stderr(&out).contains("lines>150"), "{}", stderr(&out));
    assert_ne!(f.git(&f.selected(), &["rev-parse", "HEAD"]), remote);
    assert_eq!(f.git(&f.origin, &["rev-parse", BRANCH]), remote);
    assert!(f.read_home("hook-calls").contains("pre-commit"));
    assert!(!f.read_home("hook-calls").contains("pre-push"));
    assert!(f.state().join("base").exists());
    assert!(f.read_home("gh-calls").is_empty());
}

#[test]
fn push_remeasures_continuation_before_publishing_changes_added_after_commit() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    prepare(&mut f, SUBJECT);
    commit(&mut f);
    // Another commit after the measured commit must not escape the limit.
    f.write(&f.selected(), "notes.md", &"late review work\n".repeat(151));
    f.git(&f.selected(), &["add", "notes.md"]);
    f.git(
        &f.selected(),
        &["commit", "-qm", "fix: include late review work"],
    );
    let local = f.git(&f.selected(), &["rev-parse", "HEAD"]);
    let remote = f.git(&f.origin, &["rev-parse", BRANCH]);
    fs::write(f.home.join("hook-calls"), "").unwrap();
    let out = f.qc("push");
    assert!(!out.status.success(), "oversized continuation published");
    assert_eq!(f.git(&f.origin, &["rev-parse", BRANCH]), remote);
    assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD"]), local);
    assert!(!f.read_home("hook-calls").contains("pre-push"));
    assert!(f.state().join("base").exists());
    assert!(f.read_home("gh-calls").is_empty());
}

fn install_push_repair_fixture(f: &Fixture, lines: usize) {
    write_executable(
        &f.caller.join(".git/hooks/pre-push"),
        r#"#!/bin/bash
cat >> "$HOME/push-input"
printf 'pre-push\n' >> "$HOME/hook-calls"
if [ ! -f "$HOME/push-rejected" ]; then
  : > "$HOME/push-rejected"
  printf '%s\n' '- hook id: review-policy' 'review findings need repair' >&2
  exit 1
fi
"#,
        0o755,
    );
    // Keep real guards and repair logic. Only the fixing agent is synthetic.
    let bin = f.tmp.path().join("bin");
    fs::write(f.home.join("repair-lines"), format!("{lines}\n")).unwrap();
    fs::rename(bin.join("amplihack"), bin.join("amplihack-real")).unwrap();
    write_executable(
        &bin.join("amplihack"),
        r#"#!/bin/bash
if [ "$1" != recipe ]; then exec "$(dirname "$0")/amplihack-real" "$@"; fi
printf '%s\n' "$*" >> "$HOME/recipe-calls"
if [ "$2" = show ]; then exit 0; fi
case "$3" in
  *hook-rejection-repair*)
    read -r lines < "$HOME/repair-lines"
    for ((i=0; i<lines; i++)); do printf 'repair review notes\n'; done > notes.md
    printf '%s\n' '{"hook_repair":{"status":"fixed"}}'
    ;;
  *) printf '%s\n' '{"verdict":"DONE","reasoning":"synthetic repair complete"}' ;;
esac
"#,
        0o755,
    );
}

#[test]
fn ordinary_push_can_repair_a_rejection_and_publish_with_every_hook() {
    let mut f = Fixture::new("");
    success(&f.select());
    // Ordinary setup publishes the initial branch; install the rejecting hook
    // afterwards, before the immutable configuration snapshot is recorded.
    install_push_repair_fixture(&f, 1);
    success(&f.qc("record-base"));
    f.builder(SUBJECT);
    assert_eq!(success(&f.qc("diff-limits")), "proceed");
    success(&f.qc("run-checks"));
    fs::write(f.home.join("hook-calls"), "").unwrap();
    commit(&mut f);
    assert_eq!(success(&f.qc("push")), "yes");
    let branch = f.git(&f.selected(), &["branch", "--show-current"]);
    assert_eq!(
        f.git(&f.origin, &["rev-parse", &branch]),
        f.git(&f.selected(), &["rev-parse", "HEAD"])
    );
    assert_eq!(
        f.git(&f.selected(), &["show", "HEAD:notes.md"]),
        "repair review notes"
    );
    let hooks = f.read_home("hook-calls");
    for hook in ["pre-commit", "commit-msg", "pre-push"] {
        assert_eq!(
            hooks.lines().filter(|line| *line == hook).count(),
            2,
            "{hooks}"
        );
    }
    success(&f.qc("create-pr"));
    assert_eq!(f.read_home("gh-calls").lines().count(), 1);
}

#[test]
fn rejected_continuation_never_publishes_unmeasured_repair_work() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    install_push_repair_fixture(&f, 151);
    prepare(&mut f, SUBJECT);
    commit(&mut f);
    let measured = f.git(&f.selected(), &["rev-parse", "HEAD"]);
    let remote = f.git(&f.origin, &["rev-parse", BRANCH]);
    let out = f.qc("push");
    assert!(
        !stderr(&out).contains("command not found"),
        "a missing command cannot establish safe rejection: {}",
        stderr(&out)
    );
    if !f.read_home("recipe-calls").is_empty() {
        assert_eq!(
            fs::read_to_string(f.selected().join("notes.md")).unwrap(),
            "repair review notes\n".repeat(151),
            "repair fixture did not reach its agent: {}",
            stderr(&out)
        );
    }
    assert!(
        !out.status.success(),
        "rejected continuation unexpectedly succeeded"
    );
    assert_eq!(
        f.git(&f.origin, &["rev-parse", BRANCH]),
        remote,
        "repair work reached origin before its size was checked"
    );
    assert!(f.read_home("hook-calls").contains("pre-push"));
    // Stopping on rejection or retaining measured repair work are both safe.
    success(
        &f.command("git")
            .current_dir(f.selected())
            .args(["merge-base", "--is-ancestor", &measured, "HEAD"])
            .output()
            .unwrap(),
    );
    assert!(f.state().join("base").exists());
    assert!(f.read_home("gh-calls").is_empty());
}

#[test]
fn changed_git_configuration_stops_push_before_hooks_or_remote_mutation() {
    let mut f = Fixture::new("");
    prepare(&mut f, SUBJECT);
    commit(&mut f);
    let branch = f.git(&f.selected(), &["branch", "--show-current"]);
    let remote = f.git(&f.origin, &["rev-parse", &branch]);
    fs::write(f.home.join("hook-calls"), "").unwrap();
    f.git(&f.selected(), &["config", "push.default", "matching"]);
    let out = f.qc("push");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("changed"), "{}", stderr(&out));
    assert!(!f.read_home("hook-calls").contains("pre-push"));
    assert_eq!(f.git(&f.origin, &["rev-parse", &branch]), remote);
}

#[test]
fn continuation_rechecks_exact_branch_before_commit_and_before_push() {
    for operation in ["commit", "push"] {
        let mut f = Fixture::new(BRANCH);
        f.branch(true);
        prepare(&mut f, SUBJECT);
        if operation == "push" {
            commit(&mut f);
        }
        f.git(&f.selected(), &["checkout", "-q", "-b", "quick/substitute"]);
        let head = f.git(&f.selected(), &["rev-parse", "HEAD"]);
        let remote = f.git(&f.origin, &["show-ref"]);
        fs::write(f.home.join("hook-calls"), "").unwrap();
        let out = f.qc(operation);
        assert!(!out.status.success(), "{operation} accepted another branch");
        assert!(stderr(&out).contains(BRANCH), "{}", stderr(&out));
        assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD"]), head);
        assert_eq!(f.git(&f.origin, &["show-ref"]), remote);
        assert!(f.read_home("hook-calls").is_empty());
    }
}

#[test]
fn matching_but_nonexistent_pinned_sha_is_not_accepted_as_a_base() {
    let mut f = Fixture::new("");
    success(&f.select());
    success(&f.qc("record-base"));
    let missing = "0".repeat(40);
    fs::write(f.state().join("base"), &missing).unwrap();
    f.ctx.set("quick_change_base", json!(missing));
    let out = f.common("qc_base");
    assert!(
        !out.status.success(),
        "unknown SHA accepted as immutable base"
    );
}
