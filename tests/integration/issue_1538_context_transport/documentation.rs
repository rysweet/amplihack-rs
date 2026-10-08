use super::fixtures::*;
use serde_json::json;

fn checkpoint(f: Fixture, expected: &str) {
    let r = f.doc();
    r.exit(0);
    assert!(
        r.stdout
            .starts_with(&format!("DOC_REVIEW_CHECKPOINT: {expected}\n")),
        "{}",
        r.stdout
    );
    assert_eq!(r.stderr.contains("WARNING:"), expected == "NEEDS_ATTENTION");
}

#[test]
fn standalone_canonical_doc_object_passes() {
    checkpoint(
        Fixture::new().env(
            "RECIPE_VAR_doc_review_feedback",
            json!({"status":"OK","feedback":"checked"}).to_string(),
        ),
        "OK",
    );
}

#[test]
fn standalone_file_doc_positive_beats_stale_negative() {
    checkpoint(
        Fixture::new()
            .file(json!({"doc_review_feedback":{"status":"OK"}}))
            .env(
                "DOC_REVIEW_FEEDBACK",
                json!({"status":"NEEDS_ATTENTION"}).to_string(),
            ),
        "OK",
    );
}

#[test]
fn standalone_canonical_doc_negative_beats_stale_ok() {
    checkpoint(
        Fixture::new()
            .env(
                "RECIPE_VAR_doc_review_feedback",
                json!({"status":"NEEDS_ATTENTION"}).to_string(),
            )
            .env("DOC_REVIEW_FEEDBACK", json!({"status":"OK"}).to_string()),
        "NEEDS_ATTENTION",
    );
}

#[test]
fn standalone_file_doc_missing_never_revives_stale_ok() {
    checkpoint(
        Fixture::new()
            .file(json!({}))
            .env("DOC_REVIEW_FEEDBACK", json!({"status":"OK"}).to_string()),
        "NEEDS_ATTENTION",
    );
}

#[test]
fn standalone_invalid_doc_root_never_revives_stale_ok() {
    for raw in ["", "null", "[]", "broken {\"status\":\"OK\"}"] {
        checkpoint(
            Fixture::new()
                .env("RECIPE_VAR_doc_review_feedback", raw)
                .env("DOC_REVIEW_FEEDBACK", json!({"status":"OK"}).to_string()),
            "NEEDS_ATTENTION",
        );
    }
}

#[test]
fn standalone_nested_only_doc_status_is_not_reconstructed() {
    checkpoint(
        Fixture::new()
            .env("RECIPE_VAR_doc_review_feedback__status", "OK")
            .env("DOC_REVIEW_FEEDBACK", json!({"status":"OK"}).to_string()),
        "NEEDS_ATTENTION",
    );
}

#[test]
fn legacy_doc_status_normalization_and_nonfatal_failure_are_preserved() {
    checkpoint(
        Fixture::new().env(
            "DOC_REVIEW_FEEDBACK",
            json!({"status":" o k \n"}).to_string(),
        ),
        "OK",
    );
    for status in ["NOT_OK", "NEEDS_ATTENTION", ""] {
        checkpoint(
            Fixture::new().env("DOC_REVIEW_FEEDBACK", json!({"status":status}).to_string()),
            "NEEDS_ATTENTION",
        );
    }
}

#[test]
fn doc_feedback_is_data_and_only_allowlisted_durable_refs_are_emitted() {
    let f = Fixture::new();
    let marker = f.temp.path().join("executed");
    let untrusted = format!("$(touch '{}'); SECRET_MARKER_1538", marker.display());
    let f = f
        .env(
            "DOC_REVIEW_FEEDBACK",
            json!({"status":"NEEDS_ATTENTION","feedback":untrusted}).to_string(),
        )
        .env("BRANCH_NAME", "feat/1538")
        .env("PR_NUMBER", "1540")
        .env(
            "PR_URL",
            "https://github.com/rysweet/amplihack-rs/pull/1540",
        )
        .env("COMMIT_SHA", "abc123")
        .env("REVIEW_THREAD_ID", "thread123")
        .env("SECRET_TEST_VALUE", "SECRET_MARKER_1538");
    let r = f.doc();
    r.exit(0);
    assert!(!marker.exists(), "feedback must not execute");
    assert!(!r.stdout.contains("SECRET_MARKER_1538") && !r.stderr.contains("SECRET_MARKER_1538"));
    for reference in [
        "branch: feat/1538",
        "pr_number: 1540",
        "commit_sha: abc123",
        "review_thread: thread123",
    ] {
        assert!(
            r.stdout.contains(reference),
            "lost durable reference {reference}: {}",
            r.stdout
        );
    }
    assert_eq!(
        step("workflow-design", "step-06b-documentation-review")["continue_on_error"],
        true
    );
    assert_eq!(
        step("workflow-design", "step-06c-documentation-refinement")["continue_on_error"],
        true
    );
}
