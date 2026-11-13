//! Behavioural contract for #2550: policy, real commit/publication and recipe contexts.
//! Run: cargo test -p amplihack --test integration workflow_issue_link::
use super::workflow_issue_link_fixture::{Fixture, command, describe, executable, root};
use std::fs;

#[test]
fn selector_cli_obeys_every_mode_and_successful_classification() {
    let f = Fixture::new();
    for mode in [None, Some("auto"), Some("refs"), Some("closes")] {
        for classification in [None, Some("Closes"), Some("Refs")] {
            let mut args = vec!["workflow", "issue-keyword"];
            if let Some(mode) = mode {
                args.extend(["--mode", mode]);
            }
            if let Some(classification) = classification {
                args.extend(["--classification", classification]);
            }
            let out = f.stdin(&args, "");
            assert!(out.status.success(), "{args:?}: {}", describe(&out));
            let expected = if classification == Some("Closes") && mode != Some("refs") {
                "Closes\n"
            } else {
                "Refs\n"
            };
            assert_eq!(String::from_utf8(out.stdout).unwrap(), expected, "{args:?}");
        }
    }
    assert!(
        !f.path().join("home/.amplihack/.installed-version").exists(),
        "the pure keyword command must not install framework assets"
    );
}

#[test]
fn selector_cli_rejects_unsupported_modes_and_untrusted_verdicts_without_output() {
    let f = Fixture::new();
    for (flag, value) in [
        ("--mode", "force"),
        ("--mode", "REFS"),
        ("--mode", ""),
        ("--classification", "closes"),
        ("--classification", "Closes\nRefs"),
        ("--classification", "Closes #42"),
        ("--classification", ""),
    ] {
        let out = f.stdin(&["workflow", "issue-keyword", flag, value], "");
        assert!(
            !out.status.success(),
            "{flag}={value:?}: {}",
            describe(&out)
        );
        assert!(
            out.stdout.is_empty(),
            "invalid input must not emit a keyword"
        );
    }
}

#[test]
fn substantive_publication_honours_mode_and_omission() {
    for (mode, expected) in [
        (Some("refs"), "Refs #2550"),
        (Some("auto"), "Closes #2550"),
        (None, "Closes #2550"),
        (Some("closes"), "Closes #2550"),
    ] {
        let f = Fixture::new();
        f.commit_change("src/feature.rs");
        let out = f.publish(mode);
        f.assert_published(&out, expected);
    }
}

#[test]
fn planning_and_generated_changes_never_close_under_any_mode() {
    for path in ["docs/issue-2550-plan.md", "Cargo.lock"] {
        for mode in [None, Some("auto"), Some("refs"), Some("closes")] {
            let f = Fixture::new();
            f.commit_change(path);
            let out = f.publish(mode);
            f.assert_published(&out, "Refs #2550");
        }
    }
}

#[test]
fn existing_classifier_empty_input_is_reference_only_in_every_mode() {
    let f = Fixture::new();
    let out = f
        .child("bash")
        .arg(root().join("amplifier-bundle/tools/workflow_issue_reference.sh"))
        .arg("--keyword")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(out.stdout, b"Refs\n");
    for mode in ["auto", "refs", "closes"] {
        let out = f.stdin(
            &[
                "workflow",
                "issue-keyword",
                "--mode",
                mode,
                "--classification",
                "Refs",
            ],
            "",
        );
        assert!(out.status.success(), "{}", describe(&out));
        assert_eq!(out.stdout, b"Refs\n");
    }
}

#[test]
fn empty_changes_do_not_create_a_pr_in_any_mode() {
    for mode in [None, Some("auto"), Some("refs"), Some("closes")] {
        let f = Fixture::new();
        let out = f.publish(mode);
        assert!(out.status.success(), "{}", describe(&out));
        let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(result["state"], "NO_DIFF_SUCCESS", "{}", describe(&out));
        assert!(
            !f.read("operations")
                .lines()
                .any(|line| line.starts_with("pr create "))
        );
        assert!(f.read("body.md").is_empty());
    }
}

#[test]
fn actual_commit_step_uses_staged_paths_and_mode_and_runs_hooks() {
    for path in ["src/feature.rs", "docs/issue-2550-plan.md", "Cargo.lock"] {
        for mode in [None, Some("auto"), Some("refs"), Some("closes")] {
            let f = Fixture::new();
            f.change(path);
            let mut cmd = f.bash(&command("workflow-publish", "step-15-commit-push"));
            if let Some(mode) = mode {
                cmd.env("RECIPE_VAR_pr_issue_link", mode);
            }
            let out = cmd.output().unwrap();
            assert!(out.status.success(), "{path}, {mode:?}: {}", describe(&out));
            let message = f.git(&["log", "-1", "--format=%B"]);
            assert_ne!(
                message,
                "fixture base",
                "step must create a commit: {}\n{}",
                describe(&out),
                f.git(&["status", "--short"])
            );
            let keyword = if path == "src/feature.rs" && mode != Some("refs") {
                "Closes"
            } else {
                "Refs"
            };
            assert_eq!(
                message
                    .lines()
                    .filter(|line| *line == format!("{keyword} #2550"))
                    .count(),
                1,
                "{message}"
            );
            let hooks = f.read("hooks.log");
            for hook in ["pre-commit", "commit-msg", "pre-push"] {
                assert!(
                    hooks.lines().any(|line| line == hook),
                    "{hook} did not run: {hooks}"
                );
            }
            assert_eq!(f.git(&["rev-parse", "HEAD"]), f.git(&["rev-parse", "@{u}"]));
        }
    }
}

#[test]
fn pr_classification_covers_complete_diff_even_after_a_planning_only_commit() {
    let f = Fixture::new();
    f.commit_change("src/feature.rs");
    f.change("docs/issue-2550-plan.md");
    let out = f
        .bash(&command("workflow-publish", "step-15-commit-push"))
        .env("RECIPE_VAR_pr_issue_link", "auto")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        f.git(&["log", "-1", "--format=%B"])
            .lines()
            .any(|line| line == "Refs #2550")
    );
    f.assert_published(&f.publish(Some("auto")), "Closes #2550");
}

#[test]
fn classifier_failure_or_absence_keeps_conservative_refs_even_for_closes() {
    for failure in ["absent", "silent", "misleading"] {
        let f = Fixture::new();
        f.commit_change("src/feature.rs");
        let classifier = f
            .bundle
            .join("amplifier-bundle/tools/workflow_issue_reference.sh");
        match failure {
            "absent" => fs::remove_file(&classifier).unwrap(),
            "silent" => executable(&classifier, "#!/bin/sh\nexit 73\n"),
            _ => executable(&classifier, "#!/bin/sh\nprintf 'Closes\\n'\nexit 73\n"),
        }
        // The publisher's sibling helper resolution must use this private bundle.
        f.assert_published(&f.publish(Some("closes")), "Refs #2550");
        f.change("src/next.rs");
        let out = f
            .bash(&command("workflow-publish", "step-15-commit-push"))
            .env("RECIPE_VAR_pr_issue_link", "closes")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", describe(&out));
        assert!(
            f.git(&["log", "-1", "--format=%B"])
                .lines()
                .any(|line| line == "Refs #2550")
        );
    }
}

#[test]
fn failed_selector_output_is_never_used_for_commits_or_publication() {
    let f = Fixture::new();
    f.write("selector-fails", "");
    f.change("src/feature.rs");
    let out = f
        .bash(&command("workflow-publish", "step-15-commit-push"))
        .env("RECIPE_VAR_pr_issue_link", "closes")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        f.git(&["log", "-1", "--format=%B"])
            .lines()
            .any(|line| line == "Refs #2550")
    );
    f.assert_published(&f.publish(Some("closes")), "Refs #2550");
}

#[test]
fn successful_selector_with_an_invalid_keyword_is_also_reference_only() {
    for output in ["", "Closes #2550", "Closes\nRefs", "unknown"] {
        let f = Fixture::new();
        f.write("selector-output", output);
        f.change("src/feature.rs");
        let out = f
            .bash(&command("workflow-publish", "step-15-commit-push"))
            .env("RECIPE_VAR_pr_issue_link", "closes")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", describe(&out));
        assert!(
            f.git(&["log", "-1", "--format=%B"])
                .lines()
                .any(|line| line == "Refs #2550")
        );
        f.assert_published(&f.publish(Some("closes")), "Refs #2550");
    }
}

#[test]
fn rich_body_failure_reuses_the_mode_aware_reference() {
    for (mode, expected) in [
        ("auto", "Closes #2550"),
        ("refs", "Refs #2550"),
        ("closes", "Closes #2550"),
    ] {
        let f = Fixture::new();
        f.commit_change("src/feature.rs");
        // Fault only the rich-body printf. The production fallback remains intact.
        let script = format!(
            "{}\n{}",
            r#"
printf() {
  case "${1:-}" in '## Summary\n%s\n\n## Issue\n%s'*) return 74 ;; esac
  builtin printf "$@"
}
export -f printf
"#,
            command("workflow-publish", "step-16-create-draft-pr")
        );
        let out = f
            .bash(&script)
            .env("RECIPE_VAR_pr_issue_link", mode)
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("rich PR body construction failed"),
            "fault must fire: {}",
            describe(&out)
        );
        f.assert_published(&out, expected);
    }
}

#[test]
fn stale_bare_aliases_cannot_override_the_current_context() {
    for (context, stale, expected) in [
        (Some("refs"), "closes", "Refs #2550"),
        (Some("closes"), "refs", "Closes #2550"),
        (None, "refs", "Closes #2550"),
    ] {
        let f = Fixture::new();
        f.commit_change("src/feature.rs");
        let mut cmd = f.bash(&command("workflow-publish", "step-16-create-draft-pr"));
        cmd.env("PR_ISSUE_LINK", stale).env("pr_issue_link", stale);
        if let Some(context) = context {
            cmd.env("RECIPE_VAR_pr_issue_link", context);
        }
        f.assert_published(&cmd.output().unwrap(), expected);
    }
}

#[test]
fn cross_repository_reference_identity_is_kept() {
    let f = Fixture::new();
    f.commit_change("src/feature.rs");
    let out = f
        .bash(&command("workflow-publish", "step-16-create-draft-pr"))
        .env("RECIPE_VAR_strict_delivery", "true")
        .env("RECIPE_VAR_repository", "plan/tasks")
        .env("RECIPE_VAR_pr_repository", "owner/repo")
        .env("RECIPE_VAR_closing_reference", "plan/tasks#2550")
        .env("RECIPE_VAR_pr_issue_link", "refs")
        .output()
        .unwrap();
    f.assert_published(&out, "Refs plan/tasks#2550");
}

#[test]
fn absent_issue_discovery_never_invents_an_issue() {
    for mode in ["auto", "refs", "closes"] {
        let f = Fixture::new();
        f.commit_change("src/feature.rs");
        let out = f
            .bash(&command("workflow-publish", "step-16-create-draft-pr"))
            .env("ISSUE_NUMBER", "local-private")
            .env("RECIPE_VAR_issue_number", "local-private")
            .env("RECIPE_VAR_pr_issue_link", mode)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", describe(&out));
        let body = f.read("body.md");
        assert!(body.contains("No linked issue"), "{body}");
        assert!(
            !body
                .lines()
                .any(|line| line.starts_with("Closes ") || line.starts_with("Refs ")),
            "{body}"
        );
    }
}

#[test]
fn azure_devops_publication_keeps_work_item_linkage_for_every_mode() {
    for mode in [None, Some("auto"), Some("refs"), Some("closes")] {
        let f = Fixture::new();
        f.commit_change("src/feature.rs");
        executable(
            &f.bin.join("az"),
            r#"#!/bin/bash
set -eu
printf '%s\n' "$*" >> "$CASE_ROOT/azure-operations"
case "${1:-} ${2:-} ${3:-}" in
  'repos pr list') printf '[]\n' ;;
  'repos pr create')
    while [ "$#" -gt 0 ]; do
      case "$1" in --description) printf '%s' "$2" > "$CASE_ROOT/azure-body.md"; shift ;; esac
      shift
    done
    printf '{"pullRequestId":99,"url":"https://dev.azure.com/org/project/_git/repo/pullrequest/99"}\n' ;;
  *) echo 'unexpected Azure operation' >&2; exit 96 ;;
esac
"#,
        );
        let mut cmd = f.bash(&command("workflow-publish", "step-16-create-draft-pr"));
        cmd.env("REMOTE_HOST_TYPE", "azdo");
        if let Some(mode) = mode {
            cmd.env("RECIPE_VAR_pr_issue_link", mode);
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{}", describe(&out));
        let body = f.read("azure-body.md");
        assert!(body.contains("AB#2550"), "{body}");
        assert!(
            !body.contains("Closes #") && !body.contains("Refs #"),
            "{body}"
        );
        assert!(
            !f.read("operations")
                .lines()
                .any(|line| line.starts_with("pr create "))
        );
    }
}

#[test]
fn azure_devops_commit_references_remain_ab_references() {
    let f = Fixture::new();
    f.change("src/feature.rs");
    let out = f
        .bash(&command("workflow-publish", "step-15-commit-push"))
        .env("REMOTE_HOST_TYPE", "azdo")
        .env("RECIPE_VAR_pr_issue_link", "refs")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let message = f.git(&["log", "-1", "--format=%B"]);
    assert!(message.lines().any(|line| line == "AB#2550"), "{message}");
}

#[path = "workflow_issue_link_runner.rs"]
mod runner;
