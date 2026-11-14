//! Required recording failures are run failures, even after step success.
use super::{contract_support as boundary, *};

#[test]
fn unavailable_evidence_storage_rejects_execution_before_the_runner_starts() {
    let f = Fixture::new();
    let blocked = f.root.join("blocked-storage");
    fs::write(&blocked, b"a file, even when tests run as root\n").unwrap();
    let output = boundary::command(&f, "init-failure")
        .env("AMPLIHACK_WORK_ROOT", &blocked)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "mandatory recording fell back to advisory operation: {output:?}"
    );
    assert!(
        !f.root.join("init-failure/cwd").exists(),
        "runner started before storage was established"
    );
}

#[test]
fn final_record_publication_failure_cannot_report_complete_success() {
    let f = Fixture::new();
    let before = f.snapshot();
    let output = boundary::command(&f, "publish-failure")
        .env("ISSUE_2096_MODE", "publication-failure")
        .output()
        .unwrap();
    assert!(
        f.root.join("publish-failure/publication-blocked").exists(),
        "fault injection did not reach the execution boundary: {output:?}"
    );
    assert!(
        !output.status.success(),
        "zero runner exit masked required publication failure: {output:?}"
    );
    let run = boundary::run_dir(&f);
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(run.join("run.json")).unwrap()).unwrap();
    assert_ne!(
        report["outcome"], "success",
        "persisted run status must include recording failure"
    );
    assert!(
        run.join("git-trace2").is_dir(),
        "keep available raw evidence when publication fails"
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn loss_of_the_current_collector_is_a_failure_with_retained_partial_evidence() {
    let f = Fixture::new();
    let output = boundary::command(&f, "collection-failure")
        .env("ISSUE_2096_MODE", "collection-loss")
        .output()
        .unwrap();
    assert!(
        f.root.join("collection-failure/collection-lost").exists(),
        "collector fault was not injected: {output:?}"
    );
    assert!(
        !output.status.success(),
        "collector loss was silently treated as no Git activity: {output:?}"
    );
    assert!(f.root.join("collection-failure/retained-trace").is_dir());
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(boundary::run_dir(&f).join("run.json")).unwrap()).unwrap();
    assert_ne!(report["outcome"], "success");
}
