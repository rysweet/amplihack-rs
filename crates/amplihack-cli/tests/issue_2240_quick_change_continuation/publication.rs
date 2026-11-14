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
    success(&f.qc("push"));
    success(&f.qc("create-pr"));
    assert_descriptive(&f.read_home("pr-title"), &path);
}

#[test]
fn pr_title_uses_most_recent_usable_change_commit_subject() {
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
    assert_eq!(
        f.read_home("pr-title"),
        "fix: address formatting pre-push rejection"
    );
}

#[test]
fn pr_body_keeps_task_issue_reference_without_explicit_issue_number() {
    for reference in ["Fixes #2240", "Refs #2240"] {
        let mut f = Fixture::new("");
        f.ctx.set("issue_number", json!(""));
        f.ctx.set(
            "task_description",
            json!(format!("/briefs/review-findings.md\n{reference}")),
        );
        prepare(&mut f, SUBJECT);
        commit(&mut f);
        success(&f.qc("push"));
        success(&f.qc("create-pr"));
        assert_eq!(f.read_home("pr-title"), SUBJECT);
        assert!(f.read_home("pr-body").contains(reference));
    }
}

#[test]
fn continuation_reports_check_warnings_before_finishing_without_gh() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    prepare(&mut f, SUBJECT);
    let summary = "QUICK_CHANGE_CHECKS=failed ran=1 failed=1 skipped=0";
    f.ctx.set("quick_change_checks", json!(summary));
    fs::write(f.state().join("checks-summary"), summary).unwrap();
    fs::write(
        f.state().join("checks-table.md"),
        "| `review check` | exit 1 |\n",
    )
    .unwrap();
    commit(&mut f);
    success(&f.qc("push"));
    let out = f.qc("create-pr");
    success(&out);
    assert!(super::fixture::stderr(&out).contains("Checks failed"));
    assert!(f.read_home("gh-calls").is_empty());
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
fn long_change_subject_is_shortened_at_a_word_boundary() {
    let mut f = Fixture::new("");
    success(&f.select());
    success(&f.qc("record-base"));
    f.write(&f.selected(), "README.md", "base\nreview fix\n");
    let prefix = "fix: preserve the exact continuation branch and all existing review work";
    let subject = format!("{prefix} before publishing the completed change");
    assert!(prefix.len() <= 72 && subject.len() > 72);
    f.git(&f.selected(), &["add", "README.md"]);
    f.git(&f.selected(), &["commit", "-qm", &subject]);
    assert_eq!(success(&f.common("qc_title")), prefix);
}

#[test]
fn unusable_latest_subject_does_not_hide_a_usable_change_subject() {
    let mut f = Fixture::new("");
    prepare(&mut f, SUBJECT);
    commit(&mut f);
    f.write(&f.selected(), "notes.md", "newer review notes\n");
    f.git(&f.selected(), &["add", "notes.md"]);
    f.git(
        &f.selected(),
        &["commit", "-qm", "Brief: /briefs/review.md"],
    );
    assert_eq!(success(&f.common("qc_title")), SUBJECT);
}

#[test]
fn fallback_selects_largest_change_and_breaks_ties_by_complete_path() {
    for (first, second, expected) in [
        ("small\n", "larger\nchange\n", "z-notes.md"),
        ("equal\n", "equal\n", "a-notes.md"),
    ] {
        let mut f = Fixture::new("");
        success(&f.select());
        success(&f.qc("record-base"));
        f.write(&f.selected(), "a-notes.md", first);
        f.write(&f.selected(), "z-notes.md", second);
        f.git(&f.selected(), &["add", "-N", "."]);
        assert_descriptive(&success(&f.common("qc_fallback_title")), expected);
    }
}

#[test]
fn fallback_supports_binary_deleted_and_unusual_paths_without_executing_them() {
    for path in [
        "diagram.dat",
        "notes with spaces.md",
        "notes\twith\ncontrols.md",
        "$(touch injected).md",
    ] {
        let mut f = Fixture::new("");
        success(&f.select());
        success(&f.qc("record-base"));
        fs::write(f.selected().join(path), b"binary\0content\xff").unwrap();
        f.git(
            &f.selected(),
            &["--literal-pathspecs", "add", "-N", "--", path],
        );
        let title = success(&f.common("qc_fallback_title"));
        let displayed_path = if path.chars().any(char::is_control) {
            serde_json::to_string(path).unwrap()
        } else {
            path.to_owned()
        };
        assert_descriptive(&title, &displayed_path);
        assert!(!f.selected().join("injected").exists());
        assert!(!f.caller.join("injected").exists());
    }
    let mut f = Fixture::new("");
    success(&f.select());
    success(&f.qc("record-base"));
    f.git(&f.selected(), &["rm", "README.md"]);
    assert_descriptive(&success(&f.common("qc_fallback_title")), "README.md");
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

#[test]
fn full_task_references_survive_request_truncation_and_respect_issue_precedence() {
    for (issue, reference, expected) in [
        ("2240", "Fixes #2240", "Fixes #2240"),
        ("2240", "Fixes #2210", "Refs #2240"),
        ("", "Fixes #2240", "Fixes #2240"),
    ] {
        let mut f = Fixture::new("");
        f.ctx.set("issue_number", json!(issue));
        f.ctx.set(
            "task_description",
            json!(format!(
                "Brief: /briefs/review.md\n{}\n{reference}",
                "review ".repeat(700)
            )),
        );
        prepare(&mut f, SUBJECT);
        commit(&mut f);
        let message = f.git(&f.selected(), &["show", "-s", "--format=%B"]);
        assert!(message.lines().any(|line| line == expected), "{message}");
        success(&f.qc("push"));
        success(&f.qc("create-pr"));
        let body = f.read_home("pr-body");
        assert!(body.lines().any(|line| line == expected), "{body}");
    }
}

#[test]
fn fallback_preserves_whitespace_in_path_identity_through_pr_creation() {
    for path in [
        "notes with trailing space.md ",
        " leading space.md",
        "tabs\tand trailing spaces.md ",
    ] {
        let mut f = Fixture::new("");
        success(&f.select());
        success(&f.qc("record-base"));
        f.write(&f.selected(), path, "review guidance\n");
        f.ctx.set(
            "quick_change_implementation",
            json!({"summary": "Document review guidance", "commit_subject": ""}),
        );
        assert_eq!(success(&f.qc("diff-limits")), "proceed");
        success(&f.qc("run-checks"));
        commit(&mut f);
        let displayed = serde_json::to_string(path).unwrap();
        assert_descriptive(
            &f.git(&f.selected(), &["show", "-s", "--format=%s"]),
            &displayed,
        );
        success(&f.qc("push"));
        success(&f.qc("create-pr"));
        assert_descriptive(&f.read_home("pr-title"), &displayed);
    }
}
