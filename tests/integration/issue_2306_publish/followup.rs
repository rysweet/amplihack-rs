//! Regressions for the #2306 follow-up: mixed-case repository identity, the
//! terminal create rejection after retry warnings, and the scope warning.
use super::fixture::{PublishFixture, describe, executable, json};
use std::fs;

fn assert_private_empty(f: &PublishFixture) {
    assert_eq!(
        fs::read_dir(f.tmp.path().join("private")).unwrap().count(),
        0,
        "owned captures must be removed"
    );
}

#[test]
fn followup_canonical_repository_casing_preserves_success_with_and_without_scope_failure() {
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
    let f = PublishFixture::new();
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

/// Only one exact PR URL for the expected repository with a positive decimal
/// number is accepted. Each other shape is a loud identity failure with no
/// PR identity in the structured result.
#[test]
fn followup_malformed_created_identity_is_rejected() {
    for created in [
        "https://github.com/owner/repo/pull/0",
        "https://github.com/owner/repo/pull/00",
        "https://github.com/owner/repo/pull/1x",
        "https://github.com/owner/repo/pull/1/files",
        "https://github.com/owner/repo/pull/5?x=1",
        "https://github.com/owner/repo/pull/5/pull/9",
        "https://github.com/owner/repo/pull/",
        "https://github.com/owner/repo/pull/12\nextra text",
        "https://github.com/owner/repo\n/pull/5",
        "https://github.com/owner/repository/pull/5",
        "https://github.com/owner/repo/issues/12",
        "https://github.com/owner/repo/pull/-1",
        "http://github.com/owner/repo/pull/12",
        "owner/repo/pull/12",
    ] {
        let f = PublishFixture::new();
        let output = f
            .publish_command()
            .env("CREATE_OUTPUT", created)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{created:?}: {}",
            describe(&output)
        );
        let result = json(&output);
        assert_eq!(
            result["state"], "FAILED_PR_IDENTITY",
            "{created:?}: {result}"
        );
        assert_eq!(
            result["terminal_status"], "failure",
            "{created:?}: {result}"
        );
        assert_eq!(result["pr_url"], "", "{created:?}: {result}");
        assert_eq!(result["pr_number"], "", "{created:?}: {result}");
        let message = result["message"].as_str().unwrap();
        assert!(
            message.contains("a draft PR may exist for this branch"),
            "a rejected identity must tell the reader where to look: {message}"
        );
        assert_private_empty(&f);
    }
}

/// Two transient 503 attempts write retry warnings into the publisher's
/// capture before the third attempt's terminal rejection. Each warning carries
/// a diagnostic at the helper's 500-byte bound, so the history alone exceeds
/// the bound the publisher applies to what it reports. It must report the
/// terminal rejection, not the head of the warning history. The real retry
/// helper runs; only its backoff sleep is replaced by a recorder, so the test
/// checks the delays without waiting.
#[test]
fn followup_terminal_rejection_survives_long_retry_history_and_is_redacted() {
    let f = PublishFixture::new();
    let real_gh = f.tmp.path().join("gh-original");
    fs::copy(f.bin.join("gh"), &real_gh).unwrap();
    executable(
        &f.bin.join("gh"),
        &format!(
            r#"#!/bin/bash
if [ "${{1:-}} ${{2:-}}" = "pr create" ]; then
  n=$(( $(cat "$CASE_ROOT/creates" 2>/dev/null || echo 0) + 1 ))
  printf '%s' "$n" > "$CASE_ROOT/creates"
  if [ "$n" -lt 3 ]; then printf 'HTTP 503 Service Unavailable on attempt %s %0600d\n' "$n" 0 >&2; exit 23; fi
  cat "$CASE_ROOT/final.stderr" >&2; exit 23
fi
exec '{}' "$@"
"#,
            real_gh.display()
        ),
    );
    executable(
        &f.bin.join("sleep"),
        "#!/bin/bash\nprintf '%s\\n' \"$1\" >> \"$CASE_ROOT/sleeps\"\n",
    );
    let secret = "sk-ant-api03-ABCDEFGHIJKLMNOPQRST";
    let prefix = "distinctive final creation rejection";
    f.write(
        "final.stderr",
        &format!(
            "{prefix} {secret} {} beyond-diagnostic-limit",
            "x".repeat(640)
        ),
    );
    let output = f.publish();
    assert_eq!(output.status.code(), Some(23), "{}", describe(&output));
    assert_eq!(f.read("creates"), "3");
    assert_eq!(f.read("sleeps"), "1\n2\n");
    let result = json(&output);
    assert_eq!(result["state"], "FAILED_PR_CREATE");
    assert_eq!(result["terminal_status"], "failure");
    let message = result["message"].as_str().unwrap();
    assert!(
        message.contains("gh pr create") && message.contains("exit 23"),
        "{message}"
    );
    assert!(message.contains(prefix), "{message}");
    assert!(message.contains("REDACTED_TOKEN"), "{message}");
    assert!(!message.contains("HTTP 503"), "{message}");
    assert!(!message.contains(secret), "{message}");
    assert!(!message.contains("beyond-diagnostic-limit"), "{message}");
    assert!(message.len() < 600, "{message}");
    assert!(!describe(&output).contains(secret));
    assert_private_empty(&f);
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
        assert_private_empty(&f);
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
    assert_private_empty(&f);
}

/// The publisher reads its own private creation capture with `tail`. A read
/// that fails is not a capture `gh` left empty, so reporting it as one would
/// name the wrong cause. The step must say the capture was unavailable, keep
/// the original creation status, and still remove the capture.
#[test]
fn followup_unreadable_creation_capture_is_not_reported_as_empty_stderr() {
    let f = PublishFixture::new();
    f.write("create.stderr", "distinctive publish rejection");
    f.shim(
        "tail",
        r#"
case "$*" in
  *workflow-pr-create.*) exit 71 ;;
esac
exec "$REAL_TOOL" "$@"
"#,
    );
    let output = f
        .publish_command()
        .env("CREATE_STATUS", "23")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23), "{}", describe(&output));
    let result = json(&output);
    assert_eq!(result["state"], "FAILED_PR_CREATE", "{result}");
    assert_eq!(result["terminal_status"], "failure", "{result}");
    let message = result["message"].as_str().unwrap();
    assert!(
        message.contains("gh pr create") && message.contains("exit 23"),
        "{message}"
    );
    assert!(message.contains("stderr capture unavailable"), "{message}");
    assert!(
        !message.contains("empty stderr"),
        "an unread capture is not an empty one: {message}"
    );
    assert!(
        !message.contains("distinctive publish rejection"),
        "an unread capture cannot supply a diagnostic: {message}"
    );
    assert_private_empty(&f);
}
