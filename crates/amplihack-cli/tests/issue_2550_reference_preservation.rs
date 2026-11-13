//! Behavioural preservation tests. Assertions inspect the body accepted by a
//! stateful GitHub adapter, not strings in implementation source.
#![cfg(unix)]

use crate::support::issue_2550::{Fixture, describe, quote, source_root, step, succeeds};
use std::fs;

#[test]
fn omitted_references_are_restored_above_both_readiness_boundaries() {
    for boundary in [
        "<!-- merge-ready-report: evidence -->",
        "## Merge readiness",
        "## MERGE READINESS\r",
    ] {
        let f = Fixture::new();
        let candidate = format!("Updated description\n\n{boundary}\nVerdict: ready\n");
        let out = f.edit(
            "Original description\n\nRefs #42\nCloses other/repo#7\n",
            &candidate,
        );
        succeeds(&out);
        let body = f.read("body");
        let description = body.split(boundary).next().unwrap();
        assert!(
            description.contains("Refs #42") && description.contains("Closes other/repo#7"),
            "{body}"
        );
        assert!(
            body.contains("Updated description") && body.contains("Verdict: ready"),
            "unrelated updates lost: {body}"
        );
        assert_eq!(f.read("submitted"), body);
        assert_eq!(
            f.read("events").lines().collect::<Vec<_>>(),
            ["pr view", "pr edit", "pr view"]
        );
        let argv = fs::read(f.state.join("gh.argv")).unwrap();
        for operation in argv
            .split(|b| *b == 0)
            .collect::<Vec<_>>()
            .split(|a| a.is_empty())
            .filter(|a| !a.is_empty())
        {
            assert!(
                operation.contains(&b"--repo".as_slice())
                    && operation.contains(&b"owner/repo".as_slice()),
                "GitHub operation lacks explicit repository: {operation:?}"
            );
            assert!(
                operation.contains(&b"99".as_slice()),
                "GitHub operation lacks explicit PR: {operation:?}"
            );
            if operation.contains(&b"edit".as_slice()) {
                assert!(
                    operation.contains(&b"--body-file".as_slice())
                        && operation.contains(&b"-".as_slice()),
                    "body must be sent on stdin"
                );
                assert!(!operation.contains(&b"--body".as_slice()));
            }
        }
    }
}

#[test]
fn qualified_and_url_aliases_restore_the_published_keyword_and_identity_text() {
    for alias in [
        "#42",
        "OWNER/REPO#42",
        "https://github.com/owner/repo/issues/42",
    ] {
        let f = Fixture::new();
        let out = f.edit(
            "Refs owner/repo#42\n",
            &format!("New implementation\nCloses {alias}\n"),
        );
        succeeds(&out);
        assert!(
            f.read("body").lines().any(|s| s == "Refs owner/repo#42"),
            "{}",
            f.read("body")
        );
        assert!(
            !f.read("body").contains("Closes"),
            "conflicting closing meaning retained: {}",
            f.read("body")
        );
    }
}

#[test]
fn all_published_github_closing_keyword_spellings_survive_candidate_edits() {
    for keyword in [
        "close", "Closes", "CLOSED", "fix", "Fixes", "fixed", "resolve", "Resolves", "RESOLVED",
        "Refs",
    ] {
        let f = Fixture::new();
        let published = format!("{keyword} #42");
        succeeds(&f.edit(
            &format!("{published}\n"),
            "Replacement description\nRefs owner/repo#42\n",
        ));
        assert!(
            f.read("body").lines().any(|line| line == published),
            "{}",
            f.read("body")
        );
    }
}

#[test]
fn several_existing_references_keep_independent_meanings() {
    let f = Fixture::new();
    succeeds(&f.edit(
        "Refs #42\nFixes other/repo#7\nRefs https://github.com/team/project/issues/9\n",
        "New summary\nCloses owner/repo#42\nRefs OTHER/REPO#7\nCloses team/project#9\n",
    ));
    let body = f.read("body");
    for expected in [
        "Refs #42",
        "Fixes other/repo#7",
        "Refs https://github.com/team/project/issues/9",
    ] {
        assert!(
            body.lines().any(|s| s == expected),
            "missing {expected}: {body}"
        );
    }
    assert!(
        !body.contains("Closes"),
        "candidate changed published meaning: {body}"
    );
}

#[test]
fn examples_and_noncanonical_url_hosts_are_not_operative_references() {
    let f = Fixture::new();
    let baseline = "Refs #42\n\n```text\nCloses #81\n```\n~~~text\nFixes #82\n~~~\nInline `Closes #83` and ``Fixes #84``.\nRefs https://github.com.evil/owner/repo/issues/85\nRefs https://github.com/owner/repo/pull/86\nCloses `example` #87\nCloses\n```text\nexample\n```\n#88\n";
    succeeds(&f.edit(baseline, "Updated summary\n"));
    let body = f.read("body");
    assert!(body.contains("Refs #42"), "{body}");
    for excluded in ["#81", "#82", "#83", "#84", "/85", "/86", "#87", "#88"] {
        assert!(
            !body.contains(excluded),
            "example/non-issue became operative: {body}"
        );
    }
}

#[test]
fn candidate_code_examples_are_kept_without_changing_the_preserved_reference() {
    let f = Fixture::new();
    let candidate = "Summary\n\n```text\nCloses #42\n```\nUse `Fixes #42` in this example.\n";
    succeeds(&f.edit("Refs #42\n", candidate));
    let body = f.read("body");
    assert!(
        body.contains(candidate.trim()),
        "example text was rewritten: {body}"
    );
    assert!(
        body.lines().any(|s| s == "Refs #42"),
        "operative reference missing: {body}"
    );
}

#[test]
fn unmatched_and_escaped_backticks_do_not_hide_published_references() {
    for baseline in [
        "Use a ` character. Refs #42\n",
        "Use \\` to quote. Refs #42\n",
    ] {
        let f = Fixture::new();
        succeeds(&f.edit(baseline, "New summary\nCloses #42\n"));
        assert!(f.read("body").lines().any(|s| s == "Refs #42"));
        assert!(!f.read("body").contains("Closes #42"));
    }
}

#[test]
fn candidate_markdown_cannot_hide_restored_references_in_an_open_fence() {
    let f = Fixture::new();
    let out = f.edit("Refs #42\n", "New summary\n```text\nunfinished example\n");
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.read("body"), "Refs #42\n");
    assert!(!f.read("events").contains("pr edit"));
}

#[test]
fn malformed_references_are_outside_the_preservation_contract() {
    let f = Fixture::new();
    succeeds(&f.edit(
        "Refs #abc\nCloses #0\nFixes owner/repo#-1\nRefs #42\n",
        "Replacement description\n",
    ));
    assert!(f.read("body").contains("Refs #42"));
    for malformed in ["#abc", "#0", "#-1"] {
        assert!(
            !f.read("body").contains(malformed),
            "malformed identity was promoted: {}",
            f.read("body")
        );
    }
}

#[test]
fn oversized_hosting_body_is_rejected_before_editing() {
    let f = Fixture::new();
    let out = f.edit(
        &format!("Refs #42\n{}", "x".repeat(65_536)),
        "Replacement description\n",
    );
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(!f.read("events").contains("pr edit"));
    assert!(!out.stderr.is_empty());
}

#[test]
fn ambiguous_baseline_is_rejected_without_editing() {
    let f = Fixture::new();
    let baseline = "Refs #42\nFixes OWNER/REPO#42\n";
    let out = f.edit(baseline, "Updated description\n");
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .to_lowercase()
            .contains("ambig"),
        "{}",
        describe(&out)
    );
    assert_eq!(f.read("body"), baseline);
    assert_eq!(f.read("events").lines().collect::<Vec<_>>(), ["pr view"]);
}

#[test]
fn invalid_arguments_are_rejected_before_contacting_github() {
    let f = Fixture::new();
    f.write("candidate.md", "candidate");
    for (repo, pr) in [
        ("", "99"),
        ("owner", "99"),
        ("owner/repo/extra", "99"),
        ("--repo", "99"),
        ("owner/repo", "0"),
        ("owner/repo", "-1"),
        ("owner/repo", "abc"),
    ] {
        let out = f
            .workflow("edit-body")
            .args(["--repo", repo, "--pr", pr, "--candidate-file"])
            .arg(f.state.join("candidate.md"))
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(2),
            "{repo:?}/{pr}: {}",
            describe(&out)
        );
        assert!(out.stdout.is_empty());
        assert!(
            f.read("events").is_empty(),
            "invalid invocation reached GitHub"
        );
    }
}

#[test]
fn unreadable_and_invalid_utf8_candidates_fail_before_editing() {
    for candidate in [None, Some(vec![0xff, 0xfe])] {
        let f = Fixture::new();
        if let Some(bytes) = candidate {
            f.write("candidate.md", bytes);
        }
        let out = f
            .workflow("edit-body")
            .args(["--repo", "owner/repo", "--pr", "99", "--candidate-file"])
            .arg(f.state.join("candidate.md"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
        assert!(!f.read("events").contains("pr edit"));
        assert!(!out.stderr.is_empty());
    }
}

#[test]
fn hosting_failures_are_visible_without_retries_or_rollback() {
    for (failure, expected_events, may_have_changed) in [
        ("fail-pr-view", vec!["pr view"], false),
        ("fail-pr-edit", vec!["pr view", "pr edit"], false),
        ("fail-readback", vec!["pr view", "pr edit", "pr view"], true),
        (
            "wrong-readback",
            vec!["pr view", "pr edit", "pr view"],
            true,
        ),
    ] {
        let f = Fixture::new();
        f.write(failure, "yes");
        let out = f.edit("Refs #42\n", "New summary\nCloses #42\n");
        assert_eq!(out.status.code(), Some(1), "{failure}: {}", describe(&out));
        assert_eq!(
            f.read("events").lines().collect::<Vec<_>>(),
            expected_events,
            "must not retry/roll back"
        );
        let error = String::from_utf8_lossy(&out.stderr).to_lowercase();
        assert!(!error.is_empty());
        if may_have_changed {
            assert!(
                error.contains("may")
                    && (error.contains("updated")
                        || error.contains("occurred")
                        || error.contains("changed")),
                "uncertain write not disclosed: {error}"
            );
            assert!(f.read("body").contains("New summary"));
        }
    }
}

fn stalled_github_operation(failure: &str, events: &[&str], accepted: bool) {
    let f = Fixture::new();
    f.write(failure, "yes");
    let started = std::time::Instant::now();
    // A stalled edit must also bound a write larger than the stdin pipe buffer.
    let candidate = if failure == "stall-pr-edit" {
        format!("Updated description\n{}\nCloses #42\n", "x".repeat(60_000))
    } else {
        "Updated description\nCloses #42\n".to_owned()
    };
    let out = f.edit("Refs #42\n", &candidate);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(out.stdout.is_empty(), "{}", describe(&out));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "GitHub operation exceeded its deadline"
    );
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains("timed out"), "{error}");
    if failure != "stall-pr-view" {
        assert!(
            error.contains("update may already have occurred"),
            "{error}"
        );
    }
    assert_eq!(
        f.read("events").lines().collect::<Vec<_>>(),
        events,
        "must not retry or roll back a timed-out operation"
    );
    assert_eq!(f.read("body").contains("Updated description"), accepted);
}

#[test]
fn stalled_baseline_read_times_out_without_editing() {
    stalled_github_operation("stall-pr-view", &["pr view"], false);
}

#[test]
fn stalled_edit_times_out_and_reports_uncertain_write() {
    stalled_github_operation("stall-pr-edit", &["pr view", "pr edit"], false);
}

#[test]
fn stalled_readback_times_out_without_retrying_accepted_edit() {
    stalled_github_operation("stall-readback", &["pr view", "pr edit", "pr view"], true);
}

/// Execute supported body-edit examples from lifecycle instructions. This
/// does not run an LLM or pretend to verify the other merge criteria.
pub fn edit_example(text: &str) -> String {
    let fenced = text
        .split("```bash")
        .skip(1)
        .filter_map(|b| b.split("```").next())
        .find(|b| b.contains("issue-reference edit-body") || b.contains("gh pr edit"));
    fenced
        .unwrap_or_else(|| {
            panic!("lifecycle instructions need an executable supported GitHub body-edit example")
        })
        .to_owned()
}

#[test]
fn merge_ready_supported_edit_preserves_the_published_refs_keyword() {
    let f = Fixture::new();
    f.write("body", "Refs #42\n");
    fs::write(
        f.worktree.join("pr-description.md"),
        "Updated evidence\nCloses #42\n<!-- merge-ready-report: evidence -->\nReady\n",
    )
    .unwrap();
    let skill =
        fs::read_to_string(source_root().join("amplifier-bundle/skills/merge-ready/SKILL.md"))
            .unwrap();
    let command = edit_example(&skill);
    let out = f
        .bash(&command)
        .env("PR", "99")
        .env("PR_NUMBER", "99")
        .env("REPO", "owner/repo")
        .env("REPOSITORY", "owner/repo")
        .env("CANDIDATE_FILE", f.worktree.join("pr-description.md"))
        .env("RECIPE_VAR_pr_issue_link", "closes")
        .output()
        .unwrap();
    succeeds(&out);
    assert!(
        f.read("body").contains("Refs #42") && !f.read("body").contains("Closes #42"),
        "{}",
        f.read("body")
    );
}

#[test]
fn finalise_and_merge_round_supported_edits_do_not_reclassify_published_references() {
    for name in ["workflow-finalize", "autodrive-merge-round"] {
        let f = Fixture::new();
        let doc = crate::support::issue_2550::recipe(name);
        let prompts = doc["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|s| s["prompt"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let command = edit_example(&prompts);
        f.write("body", "Refs #42\n");
        fs::write(
            f.worktree.join("pr-description.md"),
            "Revised description\nCloses #42\n",
        )
        .unwrap();
        let out = f
            .bash(&command)
            .env("PR", "99")
            .env("PR_NUMBER", "99")
            .env("REPO", "owner/repo")
            .env("REPOSITORY", "owner/repo")
            .env("CANDIDATE_FILE", f.worktree.join("pr-description.md"))
            .env("RECIPE_VAR_pr_issue_link", "auto")
            .output()
            .unwrap();
        succeeds(&out);
        assert!(
            f.read("body").contains("Refs #42") && !f.read("body").contains("Closes #42"),
            "{name}: {}",
            f.read("body")
        );
    }
}

/// Use the trusted helper loading and guarded merge operation published by
/// the production finalisation recipe, with placeholders bound to this fixture.
pub fn squash(f: &Fixture) -> std::process::Output {
    let prompt = step("autodrive-finalize", "finalize-observed-action")["prompt"]
        .as_str()
        .unwrap()
        .to_owned();
    let snippets: Vec<_> = prompt
        .split('`')
        .enumerate()
        .filter_map(|(i, s)| (i % 2 == 1).then_some(s))
        .collect();
    let load = snippets
        .iter()
        .find(|s| s.contains("MM=\"$AMPLIHACK_HOME/"))
        .expect("trusted launch helper loading command");
    let merge = snippets
        .iter()
        .find(|s| s.contains("| gh pr merge") && s.contains("--body-file -"))
        .expect("production guarded squash command")
        .replace("<number>", "99")
        .replace("<verified-head>", "\"$EXPECTED_HEAD\"");
    let json = f.state.join("pr.json");
    fs::write(
        &json,
        serde_json::json!({"title":"fix: select reference", "body": f.read("body")}).to_string(),
    )
    .unwrap();
    let command = format!(
        "{load}\njson=$(cat {})\nsubject=$(autodrive_merge_subject \"$json\" 99)\nautodrive_merge_body \"$json\"\n{merge}",
        quote(&json)
    );
    f.bash(&command)
        .env("EXPECTED_HEAD", f.read("expected-head"))
        .output()
        .unwrap()
}
