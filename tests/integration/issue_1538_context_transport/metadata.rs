//! Completion metadata follows authoritative transport without borrowing flags.
use super::fixtures::*;
use serde_json::json;

#[test]
fn file_metadata_beats_stale_legacy_refs_and_preserves_scalar_types() {
    let f = Fixture::new().file(json!({"terminal_state":{"pr_url":"https://example.test/actual","pr_number":1540,
        "terminal_state":"FOLLOWUP_CREATED","terminal_success":false},"branch_name":"actual-branch"}))
        .env("TERMINAL_STATE_PR_URL", "https://example.test/stale")
        .env("TERMINAL_STATE_PR_NUMBER", "999")
        .env("BRANCH_NAME", "stale-branch");
    let r = f.finalizer("collect");
    r.exit(0);
    let j = r.json();
    assert_eq!(j["pr"]["url"], "https://example.test/actual");
    assert_eq!(j["pr"]["number"], "1540");
    assert_eq!(j["git"]["branch_name"], "actual-branch");
    assert_eq!(j["prior_terminal_state"]["terminal_success"], "false");
}

#[test]
fn authoritative_empty_metadata_suppresses_stale_publishing_refs() {
    let r = Fixture::new()
        .file(json!({}))
        .env("TERMINAL_STATE_PR_URL", "https://example.test/stale")
        .env("TERMINAL_STATE_TERMINAL_SUCCESS", "true")
        .env("PUBLISH_STATE_REACHED", "true")
        .finalizer("collect");
    r.exit(0);
    let j = r.json();
    assert_eq!(j["pr"]["url"], "");
    assert_eq!(j["prior_terminal_state"]["terminal_success"], "false");
    assert_eq!(j["completion"]["publish_state_reached"], "false");
}

#[test]
fn file_publishing_object_is_consumed_without_legacy_aliases() {
    let r = Fixture::new().file(json!({"pr_publish_result":{"pr_url":"https://example.test/pr","pr_number":1540,"state":"FOLLOWUP_CREATED"}}))
        .finalizer("collect");
    r.exit(0);
    assert_eq!(r.json()["pr"]["url"], "https://example.test/pr");
    assert_eq!(r.json()["pr"]["number"], "1540");
    assert_eq!(r.json()["pr"]["publish_status"], "FOLLOWUP_CREATED");
}

#[test]
fn complete_large_file_metadata_is_streamed_and_cannot_override_envelope() {
    let task = "large portable context λ ".repeat(20_000);
    let r = Fixture::new()
        .file(json!({"task_description":task,"issue_number":1538,
        "workflow_result":{"terminal_success":"false","terminal_state":"FAILED_IMPLEMENTATION",
        "workflow":"untrusted override","workflow_result":"untrusted override"}}))
        .finalizer("complete");
    r.exit(0);
    let j = r.json();
    assert_eq!(j["task"], task);
    assert_eq!(j["issue_number"], "1538");
    assert_eq!(j["workflow"], "default-workflow");
    assert_eq!(
        j["workflow_result"]["workflow_result"],
        "untrusted override"
    );
    assert_eq!(j["terminal_success"], "false");
}

#[test]
fn collector_large_file_scalar_metadata_retains_bytes_without_argv_delivery() {
    let branch = "branchλ".repeat(30_000);
    let r = Fixture::new()
        .file(json!({"branch_name":branch}))
        .finalizer("collect");
    r.exit(0);
    assert_eq!(r.json()["git"]["branch_name"], branch);
}
