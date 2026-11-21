//! Regressions found during the PR #2313 code and philosophy reviews.
use super::fixture::{PublishFixture, describe, json};

#[test]
fn followup_canonical_repository_casing_preserves_success_including_recovery() {
    for recover in [false, true] {
        let f = PublishFixture::new();
        f.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/Owner/Repo.git",
        ]);
        if recover {
            f.inject_scope_failure();
        }
        let output = f.publish();
        let result = f.assert_created(&output);
        assert_eq!(result["pr_number"], "99");
    }
}

#[test]
fn followup_created_pr_preserves_leading_zeroes_in_a_repository_named_pull() {
    let f = PublishFixture::new();
    f.git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/pull.git",
    ]);
    let url = "https://github.com/owner/pull/pull/099";
    let output = f
        .publish_command()
        .env("CREATE_OUTPUT", url)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
    let result = json(&output);
    assert_eq!(result["terminal_status"], "success");
    assert_eq!(result["state"], "FOLLOWUP_CREATED");
    assert_eq!(result["pr_url"], url);
    assert_eq!(result["pr_number"], "099");
}

#[test]
fn followup_rejected_create_identity_stays_out_of_results_and_logs() {
    let rejected = "https://github.com/foreign-owner/private-repo/pull/99";
    for partial_lookup in [false, true] {
        let f = PublishFixture::new();
        if partial_lookup {
            let mut pr = f.pr("CLOSED", super::fixture::BRANCH);
            pr["url"] = "".into();
            f.write("prs.json", &serde_json::json!([pr]).to_string());
        }
        let output = f
            .publish_command()
            .env("CREATE_OUTPUT", rejected)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{}", describe(&output));
        let result = json(&output);
        assert_eq!(result["state"], "FAILED_PR_IDENTITY");
        assert_eq!(result["terminal_status"], "failure");
        assert_eq!(result["pr_url"], "", "{result}");
        assert_eq!(result["pr_number"], "", "{result}");
        assert!(
            !describe(&output).contains("foreign-owner/private-repo"),
            "rejected identity leaked: {}",
            describe(&output)
        );
    }
}

#[test]
fn followup_terminal_rejection_survives_long_retry_history_and_is_redacted() {
    let f = PublishFixture::new();
    f.long_transient_create_failure();
    let secret = "sk-ant-api03-ABCDEFGHIJKLMNOPQRST";
    let prefix = "distinctive final creation rejection";
    f.write(
        "create.stderr",
        &format!(
            "{prefix} {secret} {} beyond-diagnostic-limit",
            "x".repeat(640)
        ),
    );
    let output = f
        .publish_command()
        .env("CREATE_STATUS", "23")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23), "{}", describe(&output));
    let result = json(&output);
    assert_eq!(result["state"], "FAILED_PR_CREATE");
    assert_eq!(result["terminal_status"], "failure");
    let message = result["message"].as_str().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("HTTP 503"), "{stderr}");
    assert!(
        message.contains("gh pr create") && message.contains("exit 23"),
        "{message}"
    );
    assert!(!message.contains("HTTP 503"), "{message}");
    assert!(message.contains("REDACTED_TOKEN"), "{message}");
    assert!(message.len() < 600, "{message}");
    for text in [message, stderr.as_ref()] {
        assert!(text.contains(prefix), "{text}");
        assert!(text.contains("23"), "{text}");
        assert!(!text.contains(secret), "{text}");
        assert!(!text.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX"), "{text}");
        assert!(!text.contains("beyond-diagnostic-limit"), "{text}");
    }
    assert_eq!(
        std::fs::read_dir(f.tmp.path().join("private"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn followup_failure_before_a_later_attempt_runs_gh_is_not_reported_as_the_earlier_rejection() {
    let f = PublishFixture::new();
    f.offer_collision_pr(super::fixture::BRANCH);
    // Attempt 1 is rejected with HTTP 503 and retried. Attempt 2 fails to
    // create its private stderr capture, so gh never runs again.
    f.long_transient_create_failure();
    f.shim(
        "mktemp",
        r#"
case "$*" in
  *workflow-gh-retry.*)
    # Earlier gh calls use the same helper; fail once, after create was tried.
    if [ -e "$CASE_ROOT/first-create" ] && [ ! -e "$CASE_ROOT/capture-failed" ]; then
      : > "$CASE_ROOT/capture-failed"
      exit 9
    fi ;;
esac
exec "$REAL_TOOL" "$@"
"#,
    );
    let output = f.publish_command().output().unwrap();
    assert_eq!(output.status.code(), Some(9), "{}", describe(&output));
    assert!(f.tmp.path().join("first-create").exists());
    assert!(f.tmp.path().join("capture-failed").exists());
    f.assert_create_attempts_and_scoped_lookups(1, 1);
    let result = json(&output);
    assert_eq!(result["state"], "FAILED_PR_CREATE");
    assert_eq!(result["pr_url"], "");
    assert_eq!(result["pr_number"], "");
    let message = result["message"].as_str().unwrap();
    assert!(
        message.contains("gh pr create")
            && message.contains("private stderr temp creation failed (exit 9)"),
        "{message}"
    );
    assert!(!message.contains("HTTP 503"), "{message}");
    assert!(!message.contains("collision lookup failed"), "{message}");
    assert!(!describe(&output).contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX"));
    f.assert_private_cleanup();
}

#[test]
fn followup_create_result_capture_failure_has_its_own_status_and_operation() {
    let f = PublishFixture::new();
    f.shim(
        "mktemp",
        r#"
case "${1:-}" in
  *workflow-pr-result.*) exit 79 ;;
esac
exec "$REAL_TOOL" "$@"
"#,
    );
    let output = f.publish();
    assert_eq!(output.status.code(), Some(79), "{}", describe(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("gh pr create")
            && stderr.contains("temp creation")
            && stderr.contains("79"),
        "{stderr}"
    );
    assert!(!f.read("gh.log").contains("pr create"));
    assert_eq!(
        std::fs::read_dir(f.tmp.path().join("private"))
            .unwrap()
            .count(),
        0
    );
    let result = json(&output);
    assert_eq!(result["state"], "FAILED_PR_CREATE");
    let message = result["message"].as_str().unwrap();
    assert!(
        message.contains("gh pr create")
            && message.contains("temp creation")
            && message.contains("exit 79"),
        "{message}"
    );
}

#[test]
fn followup_template_scope_sanitization_failure_warns_without_partial_output() {
    let f = PublishFixture::new();
    f.write("issue.json", r#"{"title":""}"#);
    let scope = "aaa-ghp_ABCDEFGHIJKLMNOPQRSTUVWX@scope-marker.txt";
    f.git(&["mv", "feature.txt", scope]);
    f.git(&["commit", "-m", "rename scope file"]);
    f.shim(
        "awk",
        r#"
case "$*" in
  *'code = 0'*)
    input=$(cat)
    if [ "$input" = "aaa-REDACTED_TOKEN@scope-marker.txt" ]; then
      : > "$CASE_ROOT/scope-sanitizer-failed"
      printf 'ghp_PARTIAL_SECRET_ABCDEFGHIJKLMNOP'
      exit 76
    fi
    printf '%s\n' "$input" | "$REAL_TOOL" "$@"
    exit $? ;;
esac
exec "$REAL_TOOL" "$@"
"#,
    );
    let output = f
        .publish_command()
        .env("RECIPE_VAR_task_description", "")
        .output()
        .unwrap();
    f.assert_created(&output);
    assert!(f.tmp.path().join("scope-sanitizer-failed").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let body = f.read("body.md");
    assert!(
        body.starts_with("## Summary\nConcise workflow-generated PR for branch changes.\n\n"),
        "{body}"
    );
    assert!(!describe(&output).contains("PARTIAL_SECRET"));
    assert!(!body.contains("PARTIAL_SECRET"));
    assert!(!body.contains(scope));
    let warning = stderr
        .lines()
        .find(|line| line.contains("WARNING: optional scope sanitization failed (exit 76)"))
        .unwrap_or_else(|| panic!("scope sanitization warning missing: {stderr}"));
    assert!(!warning.contains("scope-marker"), "{warning}");
}

/// The expected repository comes from `origin` alone. When `origin` does not
/// name a GitHub repository the publisher fails loudly and no provider call is
/// made, so no write can be addressed to an empty or defaulted repository.
#[test]
fn followup_unparseable_origin_stops_with_identity_failure_before_any_provider_call() {
    for origin in [
        "https://evil.example/owner/repo.git",
        "https://evil.example#@github.com/owner/repo",
        "owner/repo",
        "",
    ] {
        let f = PublishFixture::new();
        f.git(&["remote", "set-url", "origin", origin]);
        let output = f
            .publish_command()
            .env("WORKFLOW_PR_LABELS", "needs-attention")
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "origin {origin:?} must fail: {}",
            describe(&output)
        );
        let result = json(&output);
        assert_eq!(
            result["state"], "FAILED_PR_IDENTITY",
            "{origin:?}: {result}"
        );
        assert_eq!(result["terminal_status"], "failure", "{origin:?}: {result}");
        assert_eq!(result["pr_url"], "", "{origin:?}: {result}");
        assert_eq!(result["pr_number"], "", "{origin:?}: {result}");
        assert_eq!(
            f.read("gh.log"),
            "",
            "origin {origin:?} must not reach the provider"
        );
        assert!(!f.tmp.path().join("create.argv").exists(), "{origin:?}");
        f.assert_private_cleanup();
    }
}

/// A host that is not GitHub finishes as a success without a provider call,
/// including when labels were requested.
#[test]
fn followup_non_github_host_finishes_without_a_provider_call() {
    let f = PublishFixture::new();
    let output = f
        .publish_command()
        .env("REMOTE_HOST_TYPE", "other")
        .env("WORKFLOW_PR_LABELS", "needs-attention")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
    let result = json(&output);
    assert_eq!(result["state"], "non-github", "{result}");
    assert_eq!(result["terminal_status"], "success", "{result}");
    assert_eq!(result["pr_url"], "", "{result}");
    assert_eq!(result["pr_number"], "", "{result}");
    assert_eq!(f.read("gh.log"), "", "a non-GitHub host must not call gh");
    assert!(!f.tmp.path().join("create.argv").exists());
    f.assert_private_cleanup();
}
