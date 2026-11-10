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
fn followup_terminal_rejection_survives_long_retry_history_and_is_redacted() {
    let f = PublishFixture::new();
    f.long_transient_create_failure();
    let secret = "sk-ant-api03-ABCDEFGHIJKLMNOPQRST";
    f.write(
        "create.stderr",
        &format!("distinctive final creation rejection {secret}"),
    );
    let output = f
        .publish_command()
        .env("CREATE_STATUS", "23")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23), "{}", describe(&output));
    let result = json(&output);
    assert_eq!(result["state"], "FAILED_PR_CREATE");
    let message = result["message"].as_str().unwrap();
    assert!(!message.contains("HTTP 503"), "{message}");
    assert!(message.len() < 600, "{message}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    for text in [message, stderr.as_ref()] {
        assert!(
            text.contains("distinctive final creation rejection"),
            "{text}"
        );
        assert!(text.contains("23"), "{text}");
        assert!(!text.contains(secret), "{text}");
        assert!(!text.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX"), "{text}");
    }
    assert_eq!(
        std::fs::read_dir(f.tmp.path().join("private"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn followup_template_scope_sanitization_failure_warns_without_partial_output() {
    let f = PublishFixture::new();
    f.write("issue.json", r#"{"title":""}"#);
    f.shim(
        "awk",
        r#"
case "$*" in
  *'code = 0'*)
    cat >/dev/null
    printf 'ghp_PARTIAL_SECRET_ABCDEFGHIJKLMNOP'
    exit 76 ;;
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("WARNING: optional scope sanitization failed (exit 76)"),
        "{stderr}"
    );
    assert!(
        f.read("body.md")
            .contains("Concise workflow-generated PR for branch changes.")
    );
    assert!(!describe(&output).contains("PARTIAL_SECRET"));
    assert!(!f.read("body.md").contains("PARTIAL_SECRET"));
}
