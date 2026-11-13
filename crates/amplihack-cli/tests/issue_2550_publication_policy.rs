//! Real step-15 and step-16 execution, real Git commits/pushes and live hooks.
#![cfg(unix)]

use crate::support::issue_2550::{Fixture, describe, succeeds};
use std::fs;

fn matrix(policy: Option<&str>, substantive_keyword: &str) {
    for (path, keyword) in [
        ("src/feature.rs", substantive_keyword),
        ("plans/implementation.rs", "Refs"),
    ] {
        let f = Fixture::new();
        f.change(path);
        let commit = f.commit(policy);
        succeeds(&commit);
        let message = f.git(&["log", "-1", "--format=%B"]);
        assert!(
            message.lines().any(|line| line == format!("{keyword} #42")),
            "{policy:?}, {path}: {message}"
        );
        let publish = f.publication(policy);
        succeeds(&publish);
        assert!(
            f.read("body")
                .lines()
                .any(|line| line == format!("{keyword} #42")),
            "{policy:?}, {path}: {}\n{}",
            f.read("body"),
            describe(&publish)
        );
        let events = f.read("events");
        assert!(
            events.lines().any(|e| e == "pre-commit"),
            "commit hook did not run: {events}"
        );
        assert!(
            events.lines().any(|e| e == "pre-push"),
            "push hook did not run: {events}"
        );
        assert_eq!(
            f.git(&["rev-parse", "HEAD"]),
            f.git(&["rev-parse", "@{u}"]),
            "the actual commit must be published"
        );
    }
}

#[test]
fn omitted_policy_keeps_existing_publication_behaviour() {
    matrix(None, "Closes");
}
#[test]
fn explicit_auto_keeps_existing_publication_behaviour() {
    matrix(Some("auto"), "Closes");
}
#[test]
fn refs_keeps_substantive_and_planning_issues_open_in_commit_and_pr() {
    matrix(Some("refs"), "Refs");
}
#[test]
fn explicit_closes_remains_subject_to_planning_precedence() {
    matrix(Some("closes"), "Closes");
}

#[test]
fn staged_and_branch_classifications_are_independent() {
    let f = Fixture::new();
    f.change("src/previous.rs");
    f.git(&["add", "."]);
    f.git(&["commit", "-m", "earlier substantive work"]);
    f.change("docs/next-step-plan.md");
    succeeds(&f.commit(Some("auto")));
    assert!(
        f.git(&["log", "-1", "--format=%B"])
            .lines()
            .any(|s| s == "Refs #42")
    );
    succeeds(&f.publication(Some("auto")));
    assert!(f.read("body").lines().any(|s| s == "Closes #42"));
}

#[test]
fn failed_classifier_stdout_cannot_authorise_closure() {
    let f = Fixture::new();
    f.change("src/feature.rs");
    f.write("classification", "substantive src/feature.rs\nCloses\n");
    f.write("classifier-fails", "yes");
    // Every caller sees the same real failure, including the helper discovered
    // by step-15 and the explicit adapter accepted by the publication script.
    crate::support::exec_stub::write_executable(
        &f.bundle
            .join("amplifier-bundle/tools/workflow_issue_reference.sh"),
        "#!/bin/sh\nexec \"$ISSUE_2550_CLASSIFIER\" \"$@\"\n",
        0o700,
    );
    succeeds(&f.commit(Some("closes")));
    assert!(
        f.git(&["log", "-1", "--format=%B"])
            .lines()
            .any(|s| s == "Refs #42")
    );
    succeeds(&f.publication(Some("closes")));
    assert!(f.read("body").lines().any(|s| s == "Refs #42"));
    assert_eq!(
        f.read("events")
            .lines()
            .filter(|s| *s == "classifier")
            .count(),
        2,
        "both callers must actually observe the partial-output failure"
    );
}

#[test]
fn invalid_values_stop_commit_and_direct_publication_before_side_effects() {
    for value in ["", "invalid", "Auto"] {
        let f = Fixture::new();
        f.change("src/feature.rs");
        let before = f.git(&["rev-parse", "HEAD"]);
        let out = f.commit(Some(value));
        assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("pr_issue_link"),
            "{}",
            describe(&out)
        );
        assert_eq!(f.git(&["rev-parse", "HEAD"]), before);
        assert!(
            f.git(&["diff", "--cached", "--name-only"]).is_empty(),
            "invalid policy staged files"
        );
        assert!(
            f.read("events").is_empty(),
            "invalid policy reached hooks/services"
        );

        let before_files: std::collections::BTreeSet<_> = fs::read_dir(&f.root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let out = f
            .bash(&format!(
                "exec bash {}",
                crate::support::issue_2550::quote(
                    &f.bundle
                        .join("amplifier-bundle/tools/workflow_publish_pr.sh")
                )
            ))
            .env("RECIPE_VAR_pr_issue_link", value)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{}", describe(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("pr_issue_link"),
            "{}",
            describe(&out)
        );
        assert!(
            f.read("events").is_empty(),
            "invalid script entry called hosting"
        );
        assert!(!f.state.join("created").exists());
        let after_files: std::collections::BTreeSet<_> = fs::read_dir(&f.root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(
            after_files, before_files,
            "invalid script entry wrote publication artefacts"
        );
        assert_eq!(
            fs::read_dir(f.root.join("tmp")).unwrap().count(),
            0,
            "invalid script entry wrote publication artefacts"
        );
    }
}

#[test]
fn required_policy_helper_failure_blocks_publication() {
    let f = Fixture::new();
    f.change("src/feature.rs");
    f.write("policy-helper-fails", "yes");
    let before = f.git(&["rev-parse", "HEAD"]);
    assert!(!f.commit(Some("refs")).status.success());
    assert_eq!(f.git(&["rev-parse", "HEAD"]), before);
    assert!(!f.publication(Some("refs")).status.success());
    assert!(!f.state.join("created").exists());
}

#[test]
fn hooks_run_and_rejection_blocks_publication() {
    for hook in ["pre-commit", "pre-push"] {
        let f = Fixture::new();
        f.change("src/feature.rs");
        f.write(&format!("reject-{hook}"), "yes");
        let upstream = f.git(&["rev-parse", "@{u}"]);
        let out = f.commit(Some("refs"));
        assert!(!out.status.success(), "rejected {hook}: {}", describe(&out));
        assert!(
            f.read("events").lines().any(|s| s == hook),
            "hook never ran"
        );
        assert_eq!(f.git(&["rev-parse", "@{u}"]), upstream);
        assert!(!f.state.join("created").exists());
    }
}

#[test]
fn cross_repository_identity_is_preserved_when_refs_is_selected() {
    let f = Fixture::new();
    f.change("src/feature.rs");
    for id in ["step-15-commit-push", "step-16-create-draft-pr"] {
        let out = f
            .bash(&crate::support::issue_2550::step_command(
                "workflow-publish",
                id,
            ))
            .env("RECIPE_VAR_pr_issue_link", "refs")
            .env("RECIPE_VAR_strict_delivery", "true")
            .env("RECIPE_VAR_repository", "another/issues")
            .env("RECIPE_VAR_pr_repository", "owner/repo")
            .env("RECIPE_VAR_closing_reference", "another/issues#42")
            .output()
            .unwrap();
        succeeds(&out);
    }
    assert!(
        f.git(&["log", "-1", "--format=%B"])
            .lines()
            .any(|s| s == "Refs another/issues#42")
    );
    assert!(
        f.read("body")
            .lines()
            .any(|s| s == "Refs another/issues#42")
    );
}

#[test]
fn local_tracking_and_azure_devops_keep_their_provider_contracts() {
    for policy in ["auto", "refs", "closes"] {
        let f = Fixture::new();
        f.change("src/feature.rs");
        f.git(&["add", "."]);
        f.git(&["commit", "-m", "fixture change"]);
        let out = f
            .bash(&crate::support::issue_2550::step_command(
                "workflow-publish",
                "step-16-create-draft-pr",
            ))
            .env("RECIPE_VAR_pr_issue_link", policy)
            .env("ISSUE_NUMBER", "local-42")
            .output()
            .unwrap();
        succeeds(&out);
        assert!(f.read("body").contains("local-42"));
        assert!(!f.read("body").contains("Closes #42") && !f.read("body").contains("Refs #42"));
        f.write("events", "");
        let out = f
            .bash(&crate::support::issue_2550::step_command(
                "workflow-publish",
                "step-16-create-draft-pr",
            ))
            .env("RECIPE_VAR_pr_issue_link", policy)
            .env("REMOTE_HOST_TYPE", "azdo")
            .output()
            .unwrap();
        succeeds(&out);
        assert!(f.read("az.argv").contains("42"), "work item link lost");
        assert!(
            !f.read("events").contains("pr create"),
            "AzDO routed to GitHub"
        );
    }
}
