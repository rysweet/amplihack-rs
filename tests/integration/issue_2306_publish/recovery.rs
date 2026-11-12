//! #2306 metadata contracts. Fault injection proves recovery, not a macOS cause.
use super::fixture::{BRANCH, ISSUE_TITLE, PublishFixture, describe, json};
use std::fs;

#[test]
fn successful_title_precedence_remains_issue_then_task_then_changed_file_template() {
    for (issue, task, expected) in [
        (
            r#"{"title":"FIX: recover draft publishing"}"#,
            "Add task title",
            "fix: recover draft publishing (#2306)",
        ),
        (
            r#"{"title":""}"#,
            "\n## Add task title\nignored",
            "feat: Add task title (#2306)",
        ),
        (r#"{"title":"   \t"}"#, "", "Update feature.txt (#2306)"),
    ] {
        let f = PublishFixture::new();
        f.write("issue.json", issue);
        let output = f
            .publish_command()
            .env("RECIPE_VAR_task_description", task)
            .output()
            .unwrap();
        f.assert_created(&output);
        assert_eq!(f.read("title"), expected);
    }
}

#[test]
fn failed_normal_issue_lookup_still_uses_a_valid_task_title() {
    let f = PublishFixture::new();
    let output = f
        .publish_command()
        .env("ISSUE_MODE", "failed")
        .output()
        .unwrap();
    f.assert_created(&output);
    assert_eq!(f.read("title"), "feat: Add task title (#2306)");
}

#[test]
fn title_derivation_command_failure_creates_draft_with_issue_title() {
    let f = PublishFixture::new();
    f.inject_scope_failure();
    let output = f.publish();
    f.assert_created(&output);
    assert_eq!(f.read("title"), format!("{ISSUE_TITLE} (#2306)"));
    assert!(
        f.read("gh.log")
            .contains("issue view 2306 --repo owner/repo --json title")
    );
}

#[test]
fn empty_whitespace_and_suffix_only_title_bases_trigger_recovery() {
    for replacement in ["", "   \t", "...   "] {
        let f = PublishFixture::new();
        // Cap input is bounded below 200; normalizer input is bounded at 8192.
        // Empty cap input used to produce a title consisting only of (#2306).
        f.shim(
            "head",
            r#"
if [ "${1:-}" = '-c' ] && [ "${2:-8192}" -le 200 ] && [ ! -e "$CASE_ROOT/injected" ]; then
  : > "$CASE_ROOT/injected"
  cat >/dev/null
  printf '%s' "$CAP_REPLACEMENT"
  exit 0
fi
exec "$REAL_TOOL" "$@"
"#,
        );
        let output = f
            .publish_command()
            .env("CAP_REPLACEMENT", replacement)
            .output()
            .unwrap();
        f.assert_created(&output);
        assert_eq!(
            f.read("title"),
            format!("{ISSUE_TITLE} (#2306)"),
            "unusable base {replacement:?}"
        );
        assert!(
            f.read("gh.log")
                .lines()
                .filter(|l| l.starts_with("issue view "))
                .count()
                >= 2,
            "an unusable completed title must cause an explicit recovery lookup"
        );
    }
}

#[test]
fn unusable_recovery_issue_responses_use_branch_fallback() {
    for (mode, response) in [
        ("unavailable", ""),
        ("failed", ""),
        ("ok", "not JSON"),
        ("ok", r#"{"title":"valid prefix"} broken"#),
        ("ok", r#"{"title":"one"} {"title":"two"}"#),
        ("ok", ""),
        ("ok", "{}"),
        ("ok", "null"),
        ("ok", "[]"),
        ("ok", r#"{"title":""}"#),
        ("ok", r#"{"title":" \t\r\n "}"#),
        ("ok", r#"{"title":null}"#),
        ("ok", r#"{"title":123}"#),
        ("ok", r#"{"title":true}"#),
        ("ok", r#"{"title":{"value":"wrong type"}}"#),
    ] {
        let f = PublishFixture::new();
        f.inject_scope_failure();
        f.write("issue.json", response);
        let output = f
            .publish_command()
            .env("ISSUE_MODE", mode)
            .output()
            .unwrap();
        f.assert_created(&output);
        assert_eq!(
            f.read("title"),
            format!("Update {BRANCH} (#2306)"),
            "mode={mode}, response={response:?}"
        );
    }
}

#[test]
fn empty_or_whitespace_completed_titles_without_an_issue_use_branch_fallback() {
    for replacement in ["", "   \t"] {
        let f = PublishFixture::new();
        f.shim(
            "head",
            r#"
if [ "${1:-}" = '-c' ] && [ "${2:-8192}" -le 200 ] && [ ! -e "$CASE_ROOT/injected" ]; then
  : > "$CASE_ROOT/injected"
  cat >/dev/null
  printf '%s' "$CAP_REPLACEMENT"
  exit 0
fi
exec "$REAL_TOOL" "$@"
"#,
        );
        let output = f
            .publish_command()
            .env("ISSUE_NUMBER", "")
            .env("CAP_REPLACEMENT", replacement)
            .output()
            .unwrap();
        f.assert_created(&output);
        assert_eq!(f.read("title"), format!("Update {BRANCH}"));
        assert!(!f.read("gh.log").contains("issue view"));
    }
}

#[test]
fn persistent_title_normalization_failure_has_an_independent_builtin_fallback() {
    let f = PublishFixture::new();
    f.shim(
        "sed",
        r#"
case "$*" in
  *'s/  +/ /g; s/^ //; s/[. ]+$//'*)
    cat >/dev/null; echo 'injected normalizer failure' >&2; exit 72 ;;
esac
exec "$REAL_TOOL" "$@"
"#,
    );
    let output = f.publish();
    f.assert_created(&output);
    assert_eq!(f.read("title"), format!("Update {BRANCH} (#2306)"));
}

#[test]
fn optional_body_formatter_failure_preserves_draft_and_issue_reference() {
    for section in ["changed-files", "validation", "issue-reference"] {
        let f = PublishFixture::new();
        let mut command = f.publish_command();
        match section {
            "changed-files" => f.shim(
                "awk",
                r#"
case "$*" in *'shown[n]'*) cat >/dev/null; exit 73 ;; esac
exec "$REAL_TOOL" "$@"
"#,
            ),
            "validation" => {
                f.shim(
                    "sed",
                    r#"
if [ "${1:-}" = '-n' ] && [ "${2:-}" = '1,12p' ]; then cat >/dev/null; exit 74; fi
exec "$REAL_TOOL" "$@"
"#,
                );
                command.env("RECIPE_VAR_local_testing_gate", "local tests passed");
            }
            _ => {
                let helper = f.tmp.path().join("issue-reference.sh");
                super::fixture::executable(&helper, "#!/bin/bash\nexit 75\n");
                command.env("WORKFLOW_ISSUE_REFERENCE_HELPER", helper);
            }
        }
        let output = command.output().unwrap();
        f.assert_created(&output);
        assert!(
            f.read("body.md").contains("#2306"),
            "{section}: body must retain relevant issue"
        );
        assert!(f.read("body.md").contains("Summary"));
    }
}

#[test]
fn recovery_title_redacts_credentials_defuses_references_and_keeps_shell_text_literal() {
    let f = PublishFixture::new();
    f.inject_scope_failure();
    f.write("issue.json", &serde_json::json!({"title":
        "Fixes #7 ghp_ABCDEFGHIJKLMNOPQRSTUVWX https://user:pass@github.com/x AKIAABCDEFGHIJKLMNOP ASIAABCDEFGHIJKLMNOP $(touch PWNED) `id` %s\u{1b}[31m café"
    }).to_string());
    let output = f.publish();
    f.assert_created(&output);
    let title = f.read("title");
    assert!(title.ends_with(" (#2306)"));
    assert!(title.contains("issue 7") && !title.contains("#7"));
    assert!(title.contains("$(touch PWNED) `id` %s"));
    assert!(title.contains("café"));
    assert!(title.len() <= 200);
    for secret in [
        "ghp_ABCDEFGHIJKLMNOPQRSTUVWX",
        "user:pass",
        "AKIAABCDEFGHIJKLMNOP",
        "ASIAABCDEFGHIJKLMNOP",
        "\u{1b}",
    ] {
        assert!(
            !format!(
                "{title}{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .contains(secret)
        );
    }
    assert!(!f.repo.join("PWNED").exists());
}

#[test]
fn branch_fallback_is_bounded_and_safe_even_with_malicious_branch_text() {
    let f = PublishFixture::new();
    let branch = format!(
        "feat/$(touch${{IFS}}PWNED)-ghp_ABCDEFGHIJKLMNOPQRSTUVWX/{}/{}",
        "x".repeat(120),
        "y".repeat(120)
    );
    f.git(&["branch", "-m", &branch]);
    f.inject_scope_failure();
    let output = f
        .publish_command()
        .env("ISSUE_MODE", "failed")
        .output()
        .unwrap();
    f.assert_created(&output);
    let title = f.read("title");
    assert!(title.starts_with("Update ") && title.ends_with(" (#2306)"));
    assert!(title.len() <= 200);
    assert!(
        !title.contains("ghp_") && !title.contains("$("),
        "unsafe branch must use a neutral fallback component"
    );
    assert!(!f.repo.join("PWNED").exists());
}

#[test]
fn recovery_preserves_no_linked_issue_and_invalid_identity_gates() {
    for issue in ["", "local-2306"] {
        let f = PublishFixture::new();
        f.inject_scope_failure();
        let output = f
            .publish_command()
            .env("ISSUE_NUMBER", issue)
            .output()
            .unwrap();
        f.assert_created(&output);
        assert_eq!(f.read("title"), format!("Update {BRANCH}"));
        assert!(f.read("body.md").contains("No linked issue"));
        assert!(!f.read("gh.log").contains("issue view"));
        assert!(!f.read("body.md").contains("Refs #"));
    }
    let f = PublishFixture::new();
    let output = f
        .publish_command()
        .env("ISSUE_NUMBER", "2306;touch PWNED")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", describe(&output));
    assert_eq!(json(&output)["state"], "FAILED_INVALID_INPUT");
    assert!(f.read("gh.log").is_empty());
}

#[test]
fn metadata_failure_does_not_weaken_auth_identity_or_duplicate_claim_gates() {
    for gate in ["auth", "identity", "duplicate-claim"] {
        let f = PublishFixture::new();
        f.inject_scope_failure();
        let mut command = f.publish_command();
        match gate {
            "auth" => {
                command.env("LOOKUP_STATUS", "24");
            }
            "identity" => f.git(&[
                "remote",
                "set-url",
                "origin",
                "https://example.invalid/owner/repo",
            ]),
            _ => {
                let helper = f.tmp.path().join("claim-check.sh");
                super::fixture::executable(
                    &helper,
                    "#!/bin/bash\necho 'duplicate issue claim' >&2\nexit 81\n",
                );
                command.env("WORKFLOW_ISSUE_CLAIM_CHECK_HELPER", helper);
            }
        }
        let output = command.output().unwrap();
        assert!(
            !output.status.success(),
            "{gate} must remain mandatory: {}",
            describe(&output)
        );
        assert!(
            !f.read("gh.log").contains("pr create"),
            "{gate} allowed creation"
        );
        assert!(
            !f.read("gh.log").contains("issue view"),
            "{gate} allowed metadata recovery before gate rejection"
        );
    }
}

#[test]
fn metadata_failures_preserve_existing_merged_closed_and_empty_branch_outcomes() {
    for (state, expected, succeeds) in [
        ("OPEN", "existing-open-pr", true),
        ("MERGED", "MERGED", true),
        ("CLOSED", "FAILED_CLOSED_UNMERGED", false),
    ] {
        let f = PublishFixture::new();
        f.inject_scope_failure();
        f.write(
            "prs.json",
            &serde_json::json!([f.pr(state, BRANCH)]).to_string(),
        );
        let output = f.publish();
        assert_eq!(output.status.success(), succeeds, "{}", describe(&output));
        assert_eq!(json(&output)["state"], expected);
        assert!(!f.read("gh.log").contains("pr create"));
    }
    let f = PublishFixture::new();
    f.git(&["reset", "--hard", "origin/main"]);
    f.inject_scope_failure();
    let output = f.publish();
    assert!(output.status.success(), "{}", describe(&output));
    assert_eq!(json(&output)["state"], "NO_DIFF_SUCCESS");
    assert!(!f.read("gh.log").contains("pr create"));
}

#[test]
fn recovery_metadata_is_utf8_bounded_without_dropping_the_issue_suffix() {
    let f = PublishFixture::new();
    f.inject_scope_failure();
    f.write(
        "issue.json",
        &serde_json::json!({"title": format!("fix: {}", "é".repeat(180))}).to_string(),
    );
    let output = f.publish();
    f.assert_created(&output);
    let title = f.read("title");
    assert!(title.starts_with("fix: ") && title.ends_with(" (#2306)"));
    assert!(title.len() <= 200);
    assert!(!title.contains('\u{fffd}'));
}

#[test]
fn apple_mktemp_directory_precedence_cannot_override_the_caller_tmpdir() {
    let f = PublishFixture::new();
    // Faithful seam for Apple shell_cmds-278/mktemp/mktemp.c lines 94-115:
    // no template implies -t, which prefers confstr(_CS_DARWIN_USER_TEMP_DIR)
    // over TMPDIR. A positional template skips that lookup entirely.
    // The absent Darwin directory represents an inaccessible host default;
    // the caller's designated private TMPDIR remains writable.
    f.shim(
        "mktemp",
        r#"
if [ "$#" -eq 0 ]; then
  exec "$REAL_TOOL" "$CASE_ROOT/unavailable-darwin-temp/tmp.XXXXXXXX"
fi
exec "$REAL_TOOL" "$@"
"#,
    );
    let output = f.publish();
    f.assert_created(&output);
}

#[test]
fn filename_evidence_is_sanitized_in_template_title_and_multiline_body() {
    let f = PublishFixture::new();
    f.write("issue.json", r#"{"title":""}"#);
    let path = "aaa-ghp_ABCDEFGHIJKLMNOPQRSTUVWX-Fixes #987.txt";
    f.git(&["mv", "feature.txt", path]);
    f.git(&[
        "commit",
        "-m",
        "fix: ghp_ABCDEFGHIJKLMNOPQRSTUVWX Closes #986",
    ]);
    let output = f
        .publish_command()
        .env("RECIPE_VAR_task_description", "")
        .env(
            "RECIPE_VAR_local_testing_gate",
            "line one\nghp_ABCDEFGHIJKLMNOPQRSTUVWX Fixes #985\nline three",
        )
        .output()
        .unwrap();
    f.assert_created(&output);
    let title = f.read("title");
    assert!(
        title.starts_with("Update aaa-REDACTED_TOKEN-Fixes issue 987.txt"),
        "{title}"
    );
    assert!(title.ends_with(" (#2306)"));
    let body = f.read("body.md");
    for text in [&title, &body] {
        for forbidden in ["ghp_ABCDEFGHIJKLMNOPQRSTUVWX", "#987", "#986", "#985"] {
            assert!(!text.contains(forbidden), "{text}");
        }
    }
    assert!(
        body.contains("line one\nREDACTED_TOKEN Fixes issue 985\nline three"),
        "{body}"
    );
    assert!(body.contains("## Issue\nCloses #2306"), "{body}");
}

#[test]
fn metadata_sanitizer_partial_output_never_becomes_a_fallback() {
    let f = PublishFixture::new();
    f.write("issue.json", r#"{"title":""}"#);
    f.shim(
        "sed",
        "cat >/dev/null\nprintf 'ghp_PARTIAL_SECRET_ABCDEFGHIJKLMNOP'\nexit 76\n",
    );
    let output = f
        .publish_command()
        .env("RECIPE_VAR_task_description", "")
        .output()
        .unwrap();
    f.assert_created(&output);
    for text in [f.read("title"), f.read("body.md"), describe(&output)] {
        assert!(!text.contains("PARTIAL_SECRET"), "{text}");
    }
    assert!(f.read("body.md").contains("Evidence unavailable."));
}

#[test]
fn malformed_creation_output_reports_identity_failure_without_recreating() {
    for url in [
        "",
        "99",
        "https://github.com/owner/repo/pull/99/trailing",
        "https://github.com/other/repo/pull/99",
        "https://github.com/owner/repo/pull/99\nwarning",
    ] {
        let f = PublishFixture::new();
        let output = f
            .publish_command()
            .env("CREATE_OUTPUT", url)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{}", describe(&output));
        let result = json(&output);
        assert_eq!(result["state"], "FAILED_PR_IDENTITY");
        assert!(
            result["message"]
                .as_str()
                .unwrap()
                .contains("succeeded but result parsing")
        );
        assert_eq!(
            f.read("gh.log")
                .lines()
                .filter(|s| s.starts_with("pr create "))
                .count(),
            1
        );
    }
}

// base_branch and strict_delivery (docs/reference/workflow-delivery-inputs.md).

fn create_args(f: &PublishFixture) -> Vec<Vec<u8>> {
    fs::read(f.tmp.path().join("create.argv"))
        .unwrap()
        .split(|b| *b == 0)
        .map(<[u8]>::to_vec)
        .collect()
}

fn refused_without_create(f: &PublishFixture, output: &std::process::Output, reason: &str) {
    assert!(!output.status.success(), "{}", describe(output));
    assert!(
        !f.tmp.path().join("create.argv").exists(),
        "no pull request may be created: {}",
        describe(output)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(reason)
            || String::from_utf8_lossy(&output.stdout).contains(reason),
        "expected {reason:?}: {}",
        describe(output)
    );
}

/// The fixture with origin/dev at origin/main, the strict inputs for issue
/// 2306 of `plan/tasks` and a pull request into `owner/repo` (the origin).
fn strict_command(f: &PublishFixture) -> std::process::Command {
    f.git(&[
        "update-ref",
        "refs/remotes/origin/dev",
        "refs/remotes/origin/main",
    ]);
    let mut command = f.publish_command();
    command
        .env("RECIPE_VAR_base_branch", "dev")
        .env("RECIPE_VAR_strict_delivery", "true")
        .env("RECIPE_VAR_repository", "plan/tasks")
        .env("RECIPE_VAR_pr_repository", "owner/repo")
        .env("RECIPE_VAR_closing_reference", "plan/tasks#2306");
    command
}

#[test]
fn ordinary_publish_lets_github_choose_the_base() {
    let f = PublishFixture::new();
    f.assert_created(&f.publish());
    assert!(
        !create_args(&f).iter().any(|a| a.starts_with(b"--base")),
        "without base_branch, gh pr create names no base, as before"
    );
}

#[test]
fn base_branch_is_passed_to_github_without_discovery() {
    let f = PublishFixture::new();
    f.git(&[
        "update-ref",
        "refs/remotes/origin/dev",
        "refs/remotes/origin/main",
    ]);
    let output = f
        .publish_command()
        .env("RECIPE_VAR_base_branch", "dev")
        .output()
        .unwrap();
    f.assert_created(&output);
    assert!(create_args(&f).contains(&b"--base=dev".to_vec()));
    assert_eq!(f.read("title"), format!("{ISSUE_TITLE} (#2306)"));
}

#[test]
fn base_branch_is_never_replaced_when_it_is_absent_forbidden_or_the_head() {
    for (base, forbidden, reason) in [
        ("dev", "", "has no remote-tracking ref origin/dev"),
        (
            "coordination/tasks",
            "release/* coordination/*",
            "matches forbidden_base_branches pattern 'coordination/*'",
        ),
        // The step-16 guard refuses this before the base is resolved.
        (
            BRANCH,
            "",
            "landed on protected branch 'feat/issue-2306-publish'",
        ),
    ] {
        let f = PublishFixture::new();
        if base != "dev" {
            f.git(&[
                "update-ref",
                &format!("refs/remotes/origin/{base}"),
                "refs/remotes/origin/main",
            ]);
        }
        let output = f
            .publish_command()
            .env("RECIPE_VAR_base_branch", base)
            .env("RECIPE_VAR_forbidden_base_branches", forbidden)
            .output()
            .unwrap();
        refused_without_create(&f, &output, reason);
    }
}

#[test]
fn step_16_refuses_main_even_when_base_branch_names_another_branch() {
    let f = PublishFixture::new();
    f.git(&[
        "update-ref",
        "refs/remotes/origin/dev",
        "refs/remotes/origin/main",
    ]);
    f.git(&["checkout", "main"]);
    let output = f
        .publish_command()
        .env("RECIPE_VAR_base_branch", "dev")
        .output()
        .unwrap();
    refused_without_create(&f, &output, "landed on protected branch 'main'");
}

#[test]
fn strict_delivery_names_another_repository_issue_in_full() {
    let f = PublishFixture::new();
    let output = strict_command(&f).output().unwrap();
    f.assert_created(&output);
    let body = f.read("body.md");
    assert!(body.contains("Closes plan/tasks#2306"), "{body}");
    assert!(
        !body.contains(" #2306"),
        "no bare #2306, which GitHub would resolve in owner/repo:\n{body}"
    );
    assert!(
        body.contains("Workflow-generated PR for plan/tasks#2306"),
        "{body}"
    );
    assert_eq!(f.read("title"), format!("{ISSUE_TITLE} (plan/tasks#2306)"));
    let log = f.read("gh.log");
    assert!(
        log.contains("issue view 2306 --repo plan/tasks --json title"),
        "the title is read from the owning repository:\n{log}"
    );
    assert!(
        !log.contains("--repo owner/repo --json state"),
        "the claim check would read owner/repo#2306, an unrelated issue:\n{log}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("issue claim check skipped"),
        "{}",
        describe(&output)
    );
    assert!(create_args(&f).contains(&b"--base=dev".to_vec()));
}

#[test]
fn strict_delivery_in_the_same_repository_keeps_a_bare_reference() {
    let f = PublishFixture::new();
    let output = strict_command(&f)
        .env("RECIPE_VAR_repository", "Owner/Repo")
        .env("RECIPE_VAR_closing_reference", "#2306")
        .output()
        .unwrap();
    f.assert_created(&output);
    let body = f.read("body.md");
    assert!(body.contains("Closes #2306"), "{body}");
    assert_eq!(f.read("title"), format!("{ISSUE_TITLE} (#2306)"));
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("issue claim check skipped"),
        "the claim check runs for an issue in this repository: {}",
        describe(&output)
    );
}

#[test]
fn strict_delivery_refuses_inputs_that_disagree_with_the_checkout() {
    for (key, value, reason) in [
        (
            "RECIPE_VAR_closing_reference",
            "#2306",
            "closing_reference '#2306' must be 'plan/tasks#2306'",
        ),
        (
            "RECIPE_VAR_closing_reference",
            "other/repo#2306",
            "closing_reference 'other/repo#2306' must be 'plan/tasks#2306'",
        ),
        (
            "RECIPE_VAR_pr_repository",
            "plan/tasks",
            "pr_repository 'plan/tasks' is not this checkout's origin repository 'owner/repo'",
        ),
        (
            "RECIPE_VAR_repository",
            "",
            "strict_delivery requires repository",
        ),
        (
            "ISSUE_NUMBER",
            "",
            "strict_delivery requires the caller's numeric issue_number",
        ),
    ] {
        let f = PublishFixture::new();
        let output = strict_command(&f).env(key, value).output().unwrap();
        refused_without_create(&f, &output, reason);
        assert_eq!(
            json(&output)["state"],
            "FAILED_INVALID_INPUT",
            "{key}={value}"
        );
    }
}
