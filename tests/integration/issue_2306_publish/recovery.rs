//! #2306 metadata contracts. Fault injection proves recovery, not a macOS cause.
use super::fixture::{BRANCH, ISSUE_TITLE, PublishFixture, describe, json};

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
