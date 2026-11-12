use std::fs;

use serde_json::json;

use super::fixture::{BRANCH, Fixture, SUBJECT, success};
use crate::support::exec_stub::write_executable;

pub(super) fn prepare(f: &mut Fixture, subject: &str) {
    success(&f.select());
    success(&f.qc("record-base"));
    f.builder(subject);
    assert_eq!(success(&f.qc("diff-limits")), "proceed");
    success(&f.qc("run-checks"));
    // Fixture commits and setup pushes must not satisfy publication assertions.
    fs::write(f.home.join("hook-calls"), "").unwrap();
}

pub(super) fn commit(f: &mut Fixture) {
    success(&f.qc("commit"));
}

fn assert_descriptive(title: &str, path: &str) {
    assert!(
        title.contains(path),
        "fallback lost the main changed path: {title:?}"
    );
    let words = title.replace(path, "");
    assert!(
        words
            .split_whitespace()
            .any(|w| w.chars().any(char::is_alphabetic)),
        "path-only title: {title:?}"
    );
    assert!(
        !title.starts_with("Brief:") && !title.chars().any(char::is_control),
        "{title:?}"
    );
    assert!(
        !title.ends_with('…') && !title.ends_with("..."),
        "title cut off mid-word: {title:?}"
    );
}

#[test]
fn continuation_commits_all_applicable_work_pushes_exact_branch_and_never_calls_gh() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    f.write(&f.caller, "notes.md", "staged\n");
    f.git(&f.caller, &["add", "notes.md"]);
    f.write(&f.caller, "notes.md", "staged\nunstaged\n");
    f.write(&f.caller, "untracked.md", "pre-existing untracked work\n");
    prepare(&mut f, SUBJECT);
    let base = f
        .ctx
        .get("quick_change_base")
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    commit(&mut f);
    success(&f.qc("push"));
    let state = f.state();
    let final_step = f.qc("create-pr");
    success(&final_step);
    assert!(
        f.read_home("gh-calls").is_empty(),
        "continuation must perform no PR operation"
    );
    let sha = f.git(&f.selected(), &["rev-parse", "HEAD"]);
    assert_eq!(
        f.git(&f.origin, &["rev-parse", &format!("refs/heads/{BRANCH}")]),
        sha
    );
    assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD^"]), base);
    assert_eq!(
        f.git(&f.selected(), &["show", "HEAD:notes.md"]),
        "staged\nunstaged"
    );
    assert_eq!(
        f.git(&f.selected(), &["show", "HEAD:untracked.md"]),
        "pre-existing untracked work"
    );
    assert_eq!(
        f.git(&f.selected(), &["show", "-s", "--format=%s"]),
        SUBJECT
    );
    assert!(f.git(&f.selected(), &["status", "--porcelain"]).is_empty());
    let hooks = f.read_home("hook-calls");
    for hook in ["pre-commit", "commit-msg", "pre-push"] {
        assert!(
            hooks.lines().any(|s| s == hook),
            "{hook} did not run: {hooks}"
        );
    }
    let trace = f.read_home("git-trace");
    assert!(
        trace.contains(&format!("refs/heads/{BRANCH}:refs/heads/{BRANCH}")),
        "push lacks explicit destination: {trace}"
    );
    assert!(!trace.contains("--force") && !trace.contains("--no-verify"));
    assert!(!state.exists(), "successful continuation left run state");
}

#[test]
fn ordinary_publication_uses_this_runs_subject_and_keeps_issue_reference() {
    let mut f = Fixture::new("");
    prepare(&mut f, SUBJECT);
    commit(&mut f);
    success(&f.qc("push"));
    let state = f.state();
    success(&f.qc("create-pr"));
    assert_eq!(
        f.read_home("pr-title"),
        SUBJECT,
        "brief path leaked into PR title"
    );
    let body = f.read_home("pr-body");
    assert!(
        body.contains("Refs #2240") || body.contains("Fixes #2240"),
        "{body}"
    );
    assert_eq!(f.read_home("pr-base"), "main");
    assert_eq!(
        f.read_home("pr-head"),
        f.git(&f.selected(), &["branch", "--show-current"])
    );
    assert_eq!(
        f.read_home("gh-calls")
            .lines()
            .filter(|s| s.starts_with("pr create "))
            .count(),
        1
    );
    assert!(!state.exists());
}

#[test]
fn change_commit_subject_comes_from_structured_builder_result_not_task_truncation() {
    let mut f = Fixture::new("");
    prepare(&mut f, SUBJECT);
    commit(&mut f);
    assert_eq!(
        f.git(&f.selected(), &["show", "-s", "--format=%s"]),
        SUBJECT
    );
    let message = f.git(&f.selected(), &["show", "-s", "--format=%B"]);
    assert!(
        message.contains("Refs #2240") || message.contains("Fixes #2240"),
        "{message}"
    );
}

#[test]
fn invalid_subject_candidates_get_descriptive_path_aware_fallbacks() {
    for bad in [
        "",
        "/briefs/task.md",
        "README.md",
        "Brief: /briefs/task.md",
        "fix: first\nsecond",
        "fix: trailing newline\n",
        "fix:\tcontrol",
        "fix: \u{7f}control",
        &"x".repeat(100),
    ] {
        let mut f = Fixture::new("");
        prepare(&mut f, bad);
        commit(&mut f);
        let subject = f.git(&f.selected(), &["show", "-s", "--format=%s"]);
        assert_descriptive(&subject, "README.md");
        assert_ne!(subject, bad, "unusable builder subject was accepted");
    }
}

#[test]
fn malformed_builder_result_stops_before_a_commit_or_hook() {
    let mut f = Fixture::new("");
    prepare(&mut f, SUBJECT);
    f.ctx
        .set("quick_change_implementation", json!("not structured JSON"));
    let head = f.git(&f.selected(), &["rev-parse", "HEAD"]);
    let out = f.qc("commit");
    assert!(!out.status.success());
    assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD"]), head);
    assert!(f.read_home("hook-calls").is_empty());
}

#[test]
fn fallback_does_not_truncate_long_path_identity() {
    let mut f = Fixture::new("");
    success(&f.select());
    success(&f.qc("record-base"));
    let path = format!("docs/{}/review-notes.md", "descriptive-path-".repeat(6));
    f.write(&f.selected(), &path, "review guidance\n");
    f.ctx.set(
        "quick_change_implementation",
        json!({"summary": "Document review guidance", "commit_subject": ""}),
    );
    assert_eq!(success(&f.qc("diff-limits")), "proceed");
    success(&f.qc("run-checks"));
    commit(&mut f);
    assert_descriptive(&f.git(&f.selected(), &["show", "-s", "--format=%s"]), &path);
}

#[test]
fn pr_title_prefers_principal_change_subject_over_later_mechanical_repair() {
    let mut f = Fixture::new("");
    prepare(&mut f, SUBJECT);
    commit(&mut f);
    // This arranges a repair commit after the principal change, within this run.
    f.write(&f.selected(), "notes.md", "formatted notes\n");
    f.git(&f.selected(), &["add", "notes.md"]);
    f.git(
        &f.selected(),
        &[
            "commit",
            "-qm",
            "fix: address formatting pre-push rejection",
        ],
    );
    success(&f.qc("push"));
    success(&f.qc("create-pr"));
    assert_eq!(f.read_home("pr-title"), SUBJECT);
}

#[test]
fn pr_without_suitable_run_subject_uses_path_fallback_not_historical_subject() {
    let mut f = Fixture::new("");
    // A real commit-msg hook supplies an unsuitable run subject. Configure it
    // before record-base so publication's hook snapshot remains authoritative.
    write_executable(
        &f.caller.join(".git/hooks/commit-msg"),
        r#"#!/bin/bash
printf 'commit-msg\n' >> "$HOME/hook-calls"
awk 'NR == 1 { print "Brief: /briefs/old.md"; next } { print }' "$1" > "$1.reworded"
mv "$1.reworded" "$1"
"#,
        0o755,
    );
    prepare(&mut f, SUBJECT);
    commit(&mut f);
    success(&f.qc("push"));
    success(&f.qc("create-pr"));
    assert_descriptive(&f.read_home("pr-title"), "README.md");
    assert!(!f.read_home("pr-title").contains("historical"));
}

#[test]
fn divergent_origin_rejects_non_force_publication_and_preserves_local_history() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    let original = f.git(&f.caller, &["rev-parse", "HEAD"]);
    // A second checkout advances origin; the requested local branch then diverges.
    let peer = f.tmp.path().join("peer");
    f.git(
        &f.caller,
        &[
            "clone",
            "-q",
            "--branch",
            BRANCH,
            f.origin.to_str().unwrap(),
            peer.to_str().unwrap(),
        ],
    );
    f.write(&peer, "remote.md", "remote change\n");
    f.git(&peer, &["add", "-A"]);
    f.git(&peer, &["commit", "-qm", "remote branch advance"]);
    f.git(&peer, &["push", "-q", "origin", BRANCH]);
    let remote = f.git(&peer, &["rev-parse", "HEAD"]);
    f.write(&f.caller, "local.md", "local history to retain\n");
    f.git(&f.caller, &["add", "-A"]);
    f.git(&f.caller, &["commit", "-qm", "local branch advance"]);
    let local = f.git(&f.caller, &["rev-parse", "HEAD"]);
    f.git(&f.caller, &["checkout", "-q", "main"]);
    prepare(&mut f, SUBJECT);
    assert_eq!(f.ctx.get("quick_change_base").unwrap(), &json!(local));
    commit(&mut f);
    let out = f.qc("push");
    assert!(!out.status.success(), "divergent publication succeeded");
    assert_eq!(f.git(&f.origin, &["rev-parse", BRANCH]), remote);
    assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD^"]), local);
    assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD^^"]), original);
    assert!(f.read_home("gh-calls").is_empty());
}

#[test]
fn rejecting_remote_hook_is_run_and_publication_fails_visibly() {
    let mut f = Fixture::new("");
    prepare(&mut f, SUBJECT);
    commit(&mut f);
    let branch = f.git(&f.selected(), &["branch", "--show-current"]);
    let before = f.git(&f.origin, &["rev-parse", &branch]);
    write_executable(
        &f.origin.join("hooks/pre-receive"),
        "#!/bin/bash\ncat >/dev/null\nprintf 'pre-receive\\n' >> \"$HOME/hook-calls\"\necho 'review policy refused this publication' >&2\nexit 1\n",
        0o755,
    );
    let out = f.qc("push");
    assert!(!out.status.success());
    assert!(f.read_home("hook-calls").contains("pre-receive"));
    assert_eq!(f.git(&f.origin, &["rev-parse", &branch]), before);
    assert_ne!(f.git(&f.selected(), &["rev-parse", "HEAD"]), before);
    assert!(f.read_home("gh-calls").is_empty());
}
