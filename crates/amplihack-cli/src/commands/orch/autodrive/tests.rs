//! #2329: pure Rust authorization, evidence, state, and outcome contracts.
//!
//! These are internal Rust APIs, not commands agents must use. No helper in
//! this suite contacts a hosting service or performs a merge. Platform records
//! below are controlled observations; they do not demonstrate agent recovery.
//!
//! Required API: capture authorization from invocation inputs; deserialize closed
//! records; validate a candidate against runner-owned binding; reduce accepted
//! evidence and observed finalization; atomically accept private local snapshots.

use crate::commands::orch::autodrive::{
    authorization::InvocationAuthorization,
    outcome::reduce_outcome,
    records::{Binding, CandidateRecord, FinalizationObservation, LoopCompletion},
    state::EvidenceStore,
    validation::validate_candidate,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const HEAD: &str = concat!("11111111111111111111", "11111111111111111111");
const BASE: &str = concat!("22222222222222222222", "22222222222222222222");
const OTHER: &str = concat!("33333333333333333333", "33333333333333333333");
const CRITERIA: [&str; 13] = [
    "qa",
    "documentation",
    "audit_convergence",
    "checks",
    "metadata",
    "approvals",
    "conflicts",
    "protection",
    "linked_issues",
    "conversations",
    "final_verification",
    "description_evidence",
    "scope",
];

fn authorization(no_merge: &str, dry_run: &str) -> InvocationAuthorization {
    InvocationAuthorization::capture(
        "auto-drive-to-merge",
        &BTreeMap::from([
            ("no_merge".into(), no_merge.into()),
            ("autodrive_dry_run".into(), dry_run.into()),
        ]),
    )
    .expect("valid invocation")
}

fn binding(auth: &InvocationAuthorization) -> Value {
    json!({
        "run_id": auth.run_id(), "round_id": "merge-1",
        "platform": "github", "host": "github.example.test",
        "repository": "example/project", "source_repository": "example/fork",
        "pr_number": 2329,
        "pr_url": "https://github.example.test/example/project/pull/2329",
        "head_branch": "feature", "base_branch": "main",
        "head_sha": HEAD, "base_sha": BASE, "merge_base_sha": BASE,
        "policy_provenance": "trusted-launch-bundle:fixture-v1"
    })
}

fn candidate(auth: &InvocationAuthorization) -> Value {
    let observations = [
        "cargo test --locked passed; scenario execution not required for this Rust CLI",
        "Changed command documented in reference/cli.md and linked from docs/index.md",
        "Crusty CLEAN on reviewed head; evaluator DONE; no remaining findings",
        "All 4 checks completed SUCCESS on verified head; no missing required checks",
        "Read back title, body, feature -> main; PR open and non-draft",
        "Review decision APPROVED; no outstanding changes requested",
        "Current mergeability MERGEABLE; no conflict markers in full diff",
        "Applicable rules read completely; all required rules satisfied without bypass",
        "Description links Closes #2329; linked issue read back",
        "All review-thread pages read; 0 unresolved including outdated threads",
        "Expected head and refreshed mutable facts match; PR observed OPEN",
        "Platform description read back with concrete evidence for criteria 1-13",
        "Complete 3-file diff reviewed; every change serves issue #2329",
    ];
    json!({
        "schema_version": 1, "binding": binding(auth),
        "reviewed_head_sha": HEAD, "verified_head_sha": HEAD,
        "readiness": "merge_ready", "blockers": [],
        "criteria": CRITERIA.iter().zip(observations).map(|(id, observation)| json!({
            "id": id, "status": "verified", "binding": binding(auth),
            "evidence": [{"source": "controlled-observation", "observation": observation}],
            "blockers": []
        })).collect::<Vec<_>>(),
        "attempts": []
    })
}

fn validate(auth: &InvocationAuthorization, raw: Value) -> anyhow::Result<()> {
    let expected: Binding = serde_json::from_value(binding(auth))?;
    let record: CandidateRecord = serde_json::from_value(raw)?;
    validate_candidate(auth, &expected, &record).map(|_| ())
}

fn finalization(auth: &InvocationAuthorization) -> Value {
    json!({
        "binding": binding(auth), "requested_action": "verify_readiness",
        "expected_head_sha": HEAD, "refreshed_head_sha": HEAD,
        "request_expected_head_sha": null,
        "mutable_facts_refreshed": true, "merge_requests": 0,
        "request_outcome": "not_requested", "platform_state_at_entry": "open",
        "platform_state_before_action": "open", "platform_state": "open",
        "confirmation_evidence": "Read back OPEN from controlled hosting fixture",
        "failure_cause": null, "attempts": []
    })
}

fn completion() -> Value {
    json!({"accepted_round_verdict": "READY", "evaluator_verdict": "DONE", "stop_reason": "completed"})
}

fn reduce(auth: &InvocationAuthorization, raw: Value, finish: Value, done: Value) -> Value {
    let expected: Binding = serde_json::from_value(binding(auth)).unwrap();
    let record: CandidateRecord = serde_json::from_value(raw).unwrap();
    let accepted = validate_candidate(auth, &expected, &record).expect("valid candidate");
    let observed: FinalizationObservation = serde_json::from_value(finish).unwrap();
    let completed: LoopCompletion = serde_json::from_value(done).unwrap();
    serde_json::to_value(reduce_outcome(auth, &accepted, &observed, &completed).unwrap()).unwrap()
}

#[test]
fn invocation_defaults_distinguish_outer_composer_from_direct_consumers() {
    for (recipe, allowed) in [
        ("auto-drive-to-merge", true),
        ("auto-drive-light", false),
        ("autodrive-build", false),
        ("autodrive-crusty-loop", false),
        ("autodrive-merge-loop", false),
        ("autodrive-merge-round", false),
        ("autodrive-finalize", false),
    ] {
        let auth = InvocationAuthorization::capture(recipe, &BTreeMap::new()).unwrap();
        assert_eq!(auth.allows_merge(), allowed, "entrypoint {recipe}");
    }
    let direct = InvocationAuthorization::capture(
        "autodrive-finalize",
        &BTreeMap::from([("no_merge".into(), "false".into())]),
    )
    .unwrap();
    assert!(
        direct.allows_merge(),
        "explicit authorization can enable direct finalization"
    );
}

#[test]
fn malformed_flags_fail_before_child_execution_instead_of_granting_authority() {
    for key in ["no_merge", "autodrive_dry_run"] {
        for bad in [
            "",
            "0",
            "1",
            "yes",
            "FALSE",
            " true",
            "false\n",
            "null",
            "{{no_merge}}",
        ] {
            let result = InvocationAuthorization::capture(
                "auto-drive-to-merge",
                &BTreeMap::from([(key.into(), bad.into())]),
            );
            assert!(result.is_err(), "accepted {key}={bad:?}");
        }
    }
}

#[test]
fn no_merge_and_dry_run_are_independent_restrictions() {
    for (no_merge, dry_run, merge, followups) in [
        ("false", "false", true, true),
        ("true", "false", false, true),
        ("false", "true", false, false),
        ("true", "true", false, false),
    ] {
        let auth = authorization(no_merge, dry_run);
        assert_eq!(
            (auth.allows_merge(), auth.allows_followups()),
            (merge, followups)
        );
    }
}

#[test]
fn each_resume_has_fresh_authority_and_rejects_previous_run_evidence() {
    let earlier = authorization("false", "false");
    let resumed = authorization("true", "false");
    assert_ne!(earlier.run_id(), resumed.run_id());
    assert!(validate(&resumed, candidate(&earlier)).is_err());
}

#[test]
fn complete_concrete_evidence_for_all_thirteen_criteria_is_valid() {
    let auth = authorization("true", "false");
    validate(&auth, candidate(&auth)).unwrap();
}

#[test]
fn criterion_names_without_observations_do_not_establish_readiness() {
    let auth = authorization("true", "false");
    for index in 0..13 {
        let mut raw = candidate(&auth);
        raw["criteria"][index]["evidence"] = json!([]);
        assert!(
            validate(&auth, raw).is_err(),
            "missing evidence for {}",
            CRITERIA[index]
        );
    }
}

#[test]
fn missing_duplicate_unknown_and_unverified_criteria_never_pass() {
    let auth = authorization("true", "false");
    for index in 0..13 {
        let original = candidate(&auth);
        let mut missing = original.clone();
        missing["criteria"].as_array_mut().unwrap().remove(index);
        assert!(validate(&auth, missing).is_err());
        let mut duplicate = original.clone();
        duplicate["criteria"]
            .as_array_mut()
            .unwrap()
            .push(original["criteria"][index].clone());
        assert!(validate(&auth, duplicate).is_err());
        for status in ["failed", "pending", "unreadable", "PASS", "unknown"] {
            let mut raw = original.clone();
            raw["criteria"][index]["status"] = json!(status);
            assert!(validate(&auth, raw).is_err(), "criterion {index}: {status}");
        }
    }
    let mut unknown = candidate(&auth);
    unknown["criteria"][0]["id"] = json!("anything_else");
    assert!(validate(&auth, unknown).is_err());
}

#[test]
fn every_identity_dimension_is_bound_including_each_criterion() {
    let auth = authorization("true", "false");
    for (field, wrong) in [
        ("run_id", json!("old-run")),
        ("round_id", json!("merge-0")),
        ("platform", json!("other")),
        ("host", json!("elsewhere.test")),
        ("repository", json!("attacker/project")),
        ("source_repository", json!("other/fork")),
        ("pr_number", json!(2330)),
        ("pr_url", json!("https://elsewhere.test/pull/2329")),
        ("head_branch", json!("other")),
        ("base_branch", json!("release")),
        ("head_sha", json!(OTHER)),
        ("base_sha", json!(OTHER)),
        ("merge_base_sha", json!(OTHER)),
        ("policy_provenance", json!("pr-authored-policy")),
    ] {
        for criterion in [false, true] {
            let mut raw = candidate(&auth);
            if criterion {
                raw["criteria"][0]["binding"][field] = wrong.clone();
            } else {
                raw["binding"][field] = wrong.clone();
            }
            assert!(
                validate(&auth, raw).is_err(),
                "mismatched {field}, criterion={criterion}"
            );
        }
    }
}

#[test]
fn stale_review_and_contradictory_blockers_do_not_pass() {
    let auth = authorization("true", "false");
    for field in ["reviewed_head_sha", "verified_head_sha"] {
        let mut raw = candidate(&auth);
        raw[field] = json!(OTHER);
        assert!(validate(&auth, raw).is_err());
    }
    for pointer in ["/blockers", "/criteria/3/blockers"] {
        let mut raw = candidate(&auth);
        *raw.pointer_mut(pointer).unwrap() = json!(["required-check-failed"]);
        assert!(validate(&auth, raw).is_err());
    }
}

#[test]
fn missing_tools_and_access_cannot_be_excused_as_not_applicable() {
    let auth = authorization("true", "false");
    for (index, cause) in [
        (0, "cargo missing"),
        (5, "approval API denied"),
        (7, "protection 404"),
        (9, "GraphQL refused"),
    ] {
        let mut raw = candidate(&auth);
        raw["criteria"][index]["status"] = json!("not_applicable");
        raw["criteria"][index]["not_applicable_reason"] = json!(cause);
        assert!(
            validate(&auth, raw).is_err(),
            "waived required criterion: {cause}"
        );
    }
}

#[test]
fn closed_record_format_rejects_duplicate_fields_versions_and_statuses() {
    let auth = authorization("true", "false");
    let mut raw = candidate(&auth);
    raw["schema_version"] = json!(999);
    assert!(validate(&auth, raw).is_err());
    let serialized = serde_json::to_string(&candidate(&auth)).unwrap();
    let duplicate = serialized.replacen('{', "{\"schema_version\":1,", 1);
    assert!(serde_json::from_str::<CandidateRecord>(&duplicate).is_err());
    let mut raw = candidate(&auth);
    raw["readiness"] = json!("probably_ready");
    assert!(serde_json::from_value::<CandidateRecord>(raw).is_err());
}

#[test]
fn verify_only_reports_ready_and_open_without_a_merge_request() {
    let auth = authorization("true", "false");
    let result = reduce(&auth, candidate(&auth), finalization(&auth), completion());
    assert_eq!(
        (
            result["readiness"].as_str(),
            result["action"].as_str(),
            result["platform_state"].as_str()
        ),
        (Some("merge_ready"), Some("not_requested"), Some("open"))
    );
}

#[test]
fn neither_no_merge_nor_dry_run_accepts_an_action_from_candidate_context() {
    for (nm, dry) in [("true", "false"), ("false", "true")] {
        let auth = authorization(nm, dry);
        let mut observed = finalization(&auth);
        observed["requested_action"] = json!("merge");
        observed["merge_requests"] = json!(1);
        observed["request_outcome"] = json!("accepted");
        observed["platform_state"] = json!("merged");
        let result = reduce(&auth, candidate(&auth), observed, completion());
        assert_eq!(result["action"], "workflow_violation");
        assert_ne!(result["stop_reason"], "completed");
    }
}

#[test]
fn controlled_merge_success_requires_platform_confirmation_after_authorized_action() {
    let auth = authorization("false", "false");
    let mut observed = finalization(&auth);
    observed["requested_action"] = json!("merge");
    observed["merge_requests"] = json!(1);
    observed["request_expected_head_sha"] = json!(HEAD);
    observed["request_outcome"] = json!("accepted");
    observed["platform_state"] = json!("merged");
    observed["confirmation_evidence"] = json!("Fixture readback: merged=true, mergeCommit=abc");
    let result = reduce(&auth, candidate(&auth), observed, completion());
    assert_eq!(result["action"], "merged");
    assert_eq!(result["stop_reason"], "completed");
}

#[test]
fn command_success_and_queue_admission_are_not_merge_confirmation() {
    let auth = authorization("false", "false");
    for (request, state, expected) in [
        ("accepted", "open", "unconfirmed"),
        ("queued", "open", "pending"),
        ("ambiguous", "unknown", "unconfirmed"),
    ] {
        let mut observed = finalization(&auth);
        observed["requested_action"] = json!("merge");
        observed["merge_requests"] = json!(1);
        observed["request_expected_head_sha"] = json!(HEAD);
        observed["request_outcome"] = json!(request);
        observed["platform_state"] = json!(state);
        let result = reduce(&auth, candidate(&auth), observed, completion());
        assert_eq!(result["action"], expected);
        assert_ne!(result["stop_reason"], "completed");
    }
}

#[test]
fn refused_and_failed_merges_preserve_the_cause_and_open_state() {
    let auth = authorization("false", "false");
    for outcome in ["refused", "failed"] {
        let mut observed = finalization(&auth);
        observed["requested_action"] = json!("merge");
        observed["merge_requests"] = json!(1);
        observed["request_expected_head_sha"] = json!(HEAD);
        observed["request_outcome"] = json!(outcome);
        observed["failure_cause"] = json!("branch policy rejected expected head");
        let result = reduce(&auth, candidate(&auth), observed, completion());
        assert_eq!(result["action"], outcome);
        assert_eq!(result["platform_state"], "open");
        assert!(
            result
                .to_string()
                .contains("branch policy rejected expected head")
        );
        assert_ne!(result["stop_reason"], "completed");
    }
}

#[test]
fn already_merged_and_unexpected_early_merge_are_distinct() {
    let auth = authorization("true", "false");
    for (entry, expected) in [("merged", "already_merged"), ("open", "workflow_violation")] {
        let mut observed = finalization(&auth);
        observed["platform_state_at_entry"] = json!(entry);
        observed["platform_state_before_action"] = json!("merged");
        observed["platform_state"] = json!("merged");
        let result = reduce(&auth, candidate(&auth), observed, completion());
        assert_eq!(result["action"], expected);
    }
}

#[test]
fn changed_head_or_unrefreshed_mutable_facts_prevents_authorized_completion() {
    let auth = authorization("false", "false");
    for field in [
        "expected_head_sha",
        "refreshed_head_sha",
        "mutable_facts_refreshed",
    ] {
        let mut observed = finalization(&auth);
        observed["requested_action"] = json!("merge");
        observed[field] = if field == "mutable_facts_refreshed" {
            json!(false)
        } else {
            json!(OTHER)
        };
        let result = reduce(&auth, candidate(&auth), observed, completion());
        assert_ne!(result["stop_reason"], "completed", "{field}");
        assert_ne!(
            result["readiness"], "merge_ready",
            "stale finalization: {field}"
        );
    }
}

#[test]
fn merge_interface_without_expected_head_protection_is_never_authorized_completion() {
    let auth = authorization("false", "false");
    let mut observed = finalization(&auth);
    observed["requested_action"] = json!("merge");
    observed["merge_requests"] = json!(1);
    observed["request_outcome"] = json!("accepted");
    observed["platform_state"] = json!("merged");
    observed["request_expected_head_sha"] = Value::Null;
    let result = reduce(&auth, candidate(&auth), observed, completion());
    assert_eq!(result["action"], "workflow_violation");
    assert_ne!(result["stop_reason"], "completed");
}

#[test]
fn ambiguous_request_cannot_be_retried_before_observing_platform_state() {
    let auth = authorization("false", "false");
    for observed_before_retry in [true, false] {
        let mut observed = finalization(&auth);
        observed["requested_action"] = json!("merge");
        observed["merge_requests"] = json!(2);
        observed["request_expected_head_sha"] = json!(HEAD);
        observed["request_outcome"] = json!("accepted");
        observed["platform_state"] = json!("merged");
        observed["attempts"] = json!([{
            "operation":"merge", "outcome":"ambiguous", "cause":"response lost",
            "recovery":{"interface":"configured-authenticated-client", "complete":true,
                "evidence":"Observed OPEN with no pending request before retry",
                "observed_before_retry":observed_before_retry}
        }]);
        let result = reduce(&auth, candidate(&auth), observed, completion());
        assert_eq!(
            result["action"],
            if observed_before_retry {
                "merged"
            } else {
                "workflow_violation"
            }
        );
    }
}

#[test]
fn completion_requires_both_accepted_round_and_evaluator_done() {
    let auth = authorization("true", "false");
    for (round, evaluator) in [
        ("NOT_READY", "DONE"),
        ("READY", "CONTINUE"),
        ("READY", "STUCK"),
    ] {
        let mut done = completion();
        done["accepted_round_verdict"] = json!(round);
        done["evaluator_verdict"] = json!(evaluator);
        let result = reduce(&auth, candidate(&auth), finalization(&auth), done);
        assert_ne!(result["stop_reason"], "completed");
    }
}

#[test]
fn incomplete_stop_reasons_survive_reduction_without_becoming_stuck_or_success() {
    let auth = authorization("true", "false");
    for reason in [
        "no_progress",
        "budget_exhausted",
        "usage_limit",
        "orphaned",
        "dependency_failure",
        "terminal_refusal",
    ] {
        let mut done = completion();
        done["stop_reason"] = json!(reason);
        let result = reduce(&auth, candidate(&auth), finalization(&auth), done);
        assert_eq!(result["stop_reason"], reason);
        assert_ne!(result["readiness"], "merge_ready");
    }
}

#[test]
fn recovered_graphql_or_missing_client_retains_failure_and_equivalent_evidence() {
    let auth = authorization("true", "false");
    for cause in ["GraphQL refused", "gh: executable not found"] {
        let mut raw = candidate(&auth);
        raw["attempts"] = json!([{
            "operation": "collect_platform_facts", "outcome": "failed", "cause": cause,
            "recovery": {"interface": "configured-authenticated-client", "complete": true,
                "evidence": "Equivalent approvals, protection, checks and all thread pages read"}
        }]);
        let result = reduce(&auth, raw, finalization(&auth), completion());
        assert_eq!(result["readiness"], "merge_ready");
        assert!(
            result.to_string().contains(cause),
            "recovery erased original cause"
        );
    }
}

#[test]
fn interface_switching_does_not_waive_missing_evidence_or_failed_criteria() {
    let auth = authorization("true", "false");
    for (index, cause) in [
        (9, "incomplete pagination"),
        (0, "required QA tool missing"),
        (7, "no alternative to denied protection API"),
    ] {
        let mut raw = candidate(&auth);
        raw["attempts"] = json!([{"operation": "collect_platform_facts", "outcome": "failed",
            "cause": cause, "recovery": null}]);
        raw["criteria"][index]["status"] = json!("unreadable");
        raw["criteria"][index]["blockers"] = json!([cause]);
        assert!(validate(&auth, raw).is_err(), "recovery bypassed {cause}");
    }
}

#[test]
fn unavailable_recovery_reports_incomplete_evidence_with_its_original_cause() {
    let auth = authorization("true", "false");
    for (index, cause) in [
        (
            9,
            "GraphQL refused; no client can enumerate remaining pages",
        ),
        (0, "gh missing; no configured authenticated alternative"),
    ] {
        let mut raw = candidate(&auth);
        raw["readiness"] = json!("incomplete");
        raw["blockers"] = json!([cause]);
        raw["criteria"][index]["status"] = json!("unreadable");
        raw["criteria"][index]["blockers"] = json!([cause]);
        raw["attempts"] = json!([{"operation":"collect_platform_facts",
            "outcome":"failed", "cause":cause, "recovery":null}]);
        let mut done = completion();
        done["accepted_round_verdict"] = json!("NOT_READY");
        done["evaluator_verdict"] = json!("STUCK");
        done["stop_reason"] = json!("blockers");
        let result = reduce(&auth, raw, finalization(&auth), done);
        assert_eq!(result["readiness"], "incomplete");
        assert_eq!(result["action"], "not_requested");
        assert!(result.to_string().contains(cause));
    }
}

#[test]
fn permitted_non_applicability_requires_concrete_policy_supported_explanation() {
    let auth = authorization("true", "false");
    let mut raw = candidate(&auth);
    raw["criteria"][1]["status"] = json!("not_applicable");
    raw["criteria"][1]["not_applicable_reason"] = json!(
        "Trusted documentation criterion: complete diff changes only internal tests; no user-facing surface changed"
    );
    validate(&auth, raw.clone()).unwrap();
    raw["criteria"][1]["not_applicable_reason"] = json!("");
    assert!(validate(&auth, raw).is_err());
}

#[cfg(unix)]
#[test]
fn state_is_private_and_accepted_bytes_are_immutable_after_candidate_changes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    let store = EvidenceStore::create(&root).unwrap();
    let auth = authorization("true", "false");
    let expected: Binding = serde_json::from_value(binding(&auth)).unwrap();
    let staging = root.join("candidate.json");
    std::fs::write(&staging, serde_json::to_vec(&candidate(&auth)).unwrap()).unwrap();
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600)).unwrap();
    let accepted = store
        .accept_candidate("candidate.json", &auth, &expected)
        .unwrap();
    let before = std::fs::read(accepted.path()).unwrap();
    std::fs::write(staging, b"{}").unwrap();
    assert_eq!(std::fs::read(accepted.path()).unwrap(), before);
    assert_eq!(
        std::fs::metadata(root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(accepted.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn state_rejects_symlinks_traversal_unsafe_permissions_and_historical_completion() {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("autodrive-state");
    let store = EvidenceStore::create(&root).unwrap();
    let auth = authorization("true", "false");
    let expected: Binding = serde_json::from_value(binding(&auth)).unwrap();
    let outside = dir.path().join("secret.json");
    std::fs::write(&outside, serde_json::to_vec(&candidate(&auth)).unwrap()).unwrap();
    symlink(&outside, root.join("linked.json")).unwrap();
    for path in [
        "../secret.json",
        outside.to_str().unwrap(),
        "linked.json",
        ".",
    ] {
        assert!(
            store.accept_candidate(path, &auth, &expected).is_err(),
            "unsafe evidence path {path}"
        );
    }
    let unsafe_file = root.join("unsafe.json");
    std::fs::write(&unsafe_file, serde_json::to_vec(&candidate(&auth)).unwrap()).unwrap();
    std::fs::set_permissions(&unsafe_file, std::fs::Permissions::from_mode(0o666)).unwrap();
    // #2546: a world-writable file is refused and left unchanged.
    assert!(
        store
            .accept_candidate("unsafe.json", &auth, &expected)
            .is_err(),
        "a world-writable evidence file is refused"
    );
    assert_eq!(
        std::fs::metadata(&unsafe_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o666
    );
    std::fs::set_permissions(&unsafe_file, std::fs::Permissions::from_mode(0o664)).unwrap();
    // #2546: our own file with group bits only is repaired, not refused.
    store
        .accept_candidate("unsafe.json", &auth, &expected)
        .unwrap();
    assert_eq!(
        std::fs::metadata(&unsafe_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    std::fs::hard_link(&unsafe_file, dir.path().join("second-name.json")).unwrap();
    assert!(
        store
            .accept_candidate("unsafe.json", &auth, &expected)
            .is_err(),
        "a hard-linked evidence file is still refused"
    );
    std::fs::write(root.join("merge.completed"), "DONE").unwrap();
    assert!(
        store
            .accept_candidate("merge.completed", &auth, &expected)
            .is_err()
    );
    let alias = dir.path().join("alias");
    symlink(&root, &alias).unwrap();
    assert!(EvidenceStore::create(&alias).is_err());
}

#[cfg(unix)]
#[test]
fn state_rejects_special_files_and_untrusted_existing_directory_modes() {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("autodrive-state");
    let store = EvidenceStore::create(&root).unwrap();
    let auth = authorization("true", "false");
    let expected: Binding = serde_json::from_value(binding(&auth)).unwrap();
    // A one-letter name keeps the socket path within the sun_path limit
    // under a long TMPDIR.
    let _socket = UnixListener::bind(root.join("s")).unwrap();
    assert!(store.accept_candidate("s", &auth, &expected).is_err());
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
    // #2546: a world-writable state folder is refused and left unchanged.
    assert!(EvidenceStore::create(&root).is_err());
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o777
    );
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o775)).unwrap();
    // #2546: our own `autodrive-state` folder with group bits is repaired.
    EvidenceStore::create(&root).unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn oversized_records_cannot_be_silently_truncated_into_accepted_evidence() {
    let auth = authorization("true", "false");
    let mut raw = candidate(&auth);
    raw["criteria"][0]["evidence"][0]["observation"] = json!("x".repeat(16 * 1024 * 1024));
    assert!(validate(&auth, raw).is_err());
}

#[test]
fn missing_final_records_cannot_become_ready_from_child_summary() {
    let auth = authorization("true", "false");
    let context = json!({"autodrive_finalization":{"readiness":"merge_ready","action":"merged"},"merge_loop_result":{"loop_result":"BUDGET_EXHAUSTED"}});
    let result = crate::commands::orch::autodrive::report::report(
        &auth,
        context.as_object().unwrap(),
        None,
        std::path::Path::new("."),
    );
    assert_eq!(result["readiness"], "incomplete");
    assert_eq!(result["action"], "not_requested");
    assert_eq!(result["stop_reason"], "budget_exhausted");
}

#[cfg(unix)]
#[test]
fn altered_or_missing_referenced_artifacts_do_not_establish_readiness() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    // tempfile creates 0777 minus the caller's umask; the store requires a
    // private directory whatever the umask.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = EvidenceStore::create(dir.path()).unwrap();
    let auth = authorization("true", "false");
    let expected: Binding = serde_json::from_value(binding(&auth)).unwrap();
    let artifact = dir.path().join("qa.log");
    std::fs::write(&artifact, b"actual test output").unwrap();
    std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut raw = candidate(&auth);
    raw["criteria"][0]["evidence"][0]["reference"] =
        json!({"file":"qa.log","sha256":format!("{:x}",Sha256::digest(b"actual test output"))});
    let path = dir.path().join("candidate.json");
    std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    store
        .accept_candidate("candidate.json", &auth, &expected)
        .unwrap();
    std::fs::write(&artifact, b"changed").unwrap();
    assert!(
        store
            .accept_candidate("candidate.json", &auth, &expected)
            .is_err()
    );
    std::fs::remove_file(&artifact).unwrap();
    assert!(
        store
            .accept_candidate("candidate.json", &auth, &expected)
            .is_err()
    );
}

#[test]
fn unexpected_earlier_merge_cannot_be_hidden_by_later_request() {
    let auth = authorization("false", "false");
    let mut observed = finalization(&auth);
    observed["platform_state_before_action"] = json!("merged");
    observed["platform_state"] = json!("merged");
    observed["requested_action"] = json!("merge");
    observed["request_expected_head_sha"] = json!(HEAD);
    observed["merge_requests"] = json!(1);
    observed["request_outcome"] = json!("accepted");
    let result = reduce(&auth, candidate(&auth), observed, completion());
    assert_eq!(result["action"], "workflow_violation");
}

#[test]
fn completion_receipt_requires_both_signals_and_cannot_be_replayed() {
    use crate::commands::orch::autodrive::completion as receipts;
    let dir = tempfile::tempdir().unwrap();
    // tempfile creates 0777 minus the caller's umask; the store requires a
    // private directory whatever the umask.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = EvidenceStore::create(dir.path()).unwrap();
    let auth = authorization("true", "false");
    let raw = candidate(&auth);
    let record: CandidateRecord = serde_json::from_value(raw.clone()).unwrap();
    assert!(receipts::verify(&auth, dir.path(), &record).is_err());
    let accepted = store.snapshot(&serde_json::to_vec(&raw).unwrap()).unwrap();
    store
        .write_new(
            "merge-ready-round-1.json",
            &serde_json::to_vec(
                &json!({"merge_ready_verdict":"MERGE_READY","accepted_evidence":accepted}),
            )
            .unwrap(),
        )
        .unwrap();
    store.write_new("merge-ready-result.json", &serde_json::to_vec(&json!({"loop_result":"DONE","round_label":"round-1","round_verdict":"MERGE_READY"})).unwrap()).unwrap();
    receipts::record(&auth, dir.path(), "round-1").unwrap();
    receipts::verify(&auth, dir.path(), &record).unwrap();
    let resumed = authorization("true", "false");
    assert!(receipts::verify(&resumed, dir.path(), &record).is_err());
    let mut changed = record;
    changed.binding.head_sha = OTHER.into();
    assert!(receipts::verify(&auth, dir.path(), &changed).is_err());
    std::fs::write(accepted, b"{}").unwrap();
    assert!(receipts::verify(&auth, dir.path(), &changed).is_err());
}

#[test]
fn already_merged_preparation_requires_fresh_bound_platform_confirmation() {
    let auth = authorization("true", "false");
    let mut context = json!({"build_result":{"merged_before_run":true,"pr_state":"MERGED","entry_evidence":"Observed merged=true on exact PR at invocation entry","binding":binding(&auth)}});
    let result = crate::commands::orch::autodrive::report::report(
        &auth,
        context.as_object().unwrap(),
        Some("2329"),
        std::path::Path::new("."),
    );
    assert_eq!(result["action"], "already_merged");
    assert_eq!(result["readiness"], "incomplete");
    context["build_result"]["entry_evidence"] = json!("");
    let result = crate::commands::orch::autodrive::report::report(
        &auth,
        context.as_object().unwrap(),
        Some("2329"),
        std::path::Path::new("."),
    );
    assert_ne!(result["action"], "already_merged");
}

#[test]
fn complete_agent_labels_without_a_fresh_loop_receipt_cannot_establish_readiness() {
    let auth = authorization("true", "false");
    let context = json!({"finalization_observations":{"candidate":candidate(&auth),"finalization":finalization(&auth),"completion":completion()}});
    let result = crate::commands::orch::autodrive::report::report(
        &auth,
        context.as_object().unwrap(),
        None,
        std::path::Path::new("."),
    );
    assert_eq!(result["readiness"], "incomplete");
    assert_eq!(result["action"], "not_requested");
}

#[test]
fn self_consistent_evidence_cannot_change_captured_operator_selection() {
    for (key, value) in [
        ("pr_number", "2330"),
        (
            "pr_url",
            "https://github.example.test/example/project/pull/2330",
        ),
        ("existing_branch", "different-feature"),
    ] {
        let auth = InvocationAuthorization::capture(
            "auto-drive-to-merge",
            &BTreeMap::from([
                ("no_merge".into(), "true".into()),
                (key.into(), value.into()),
            ]),
        )
        .unwrap();
        assert!(
            validate(&auth, candidate(&auth)).is_err(),
            "candidate changed {key}"
        );
    }
}

#[test]
fn minor_followup_reporting_validates_the_complete_private_mapping() {
    let tmp = tempfile::tempdir().unwrap();
    let directory = tmp.path().join("state");
    let store = EvidenceStore::create(&directory).unwrap();
    assert!(
        super::autodrive::followups::read(&store, &directory, 2329)
            .unwrap()
            .is_empty()
    );
    store
        .write_new("followups.tsv", b"naming-1\t2330\t#2329\t2329\n")
        .unwrap();
    let rows = super::autodrive::followups::read(&store, &directory, 2329).unwrap();
    let value = serde_json::to_value(rows).unwrap();
    assert_eq!(value[0]["finding_id"], "naming-1");
    assert_eq!(value[0]["issue_number"], 2330);
    assert!(super::autodrive::followups::read(&store, &directory, 7).is_err());
}

#[test]
fn malformed_minor_followups_are_reported_instead_of_silently_dropped() {
    for rows in [
        "naming-1\tabc\t#2329\t2329\n",
        "x\t1\t#2329\t2329\nx\t2\t#2329\t2329\n",
        "x\t1\t\t2329\n",
        "x\t1\t#2329\t2329\textra\n",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let directory = tmp.path().join("state");
        let store = EvidenceStore::create(&directory).unwrap();
        store.write_new("followups.tsv", rows.as_bytes()).unwrap();
        assert!(super::autodrive::followups::read(&store, &directory, 2329).is_err());
    }
}

#[test]
fn allowed_post_review_range_requires_bound_concrete_evidence() {
    let auth = authorization("true", "false");
    let mut raw = candidate(&auth);
    raw["reviewed_head_sha"] = json!(OTHER);
    assert!(validate(&auth, raw.clone()).is_err());
    raw["review_range"] = json!({"reviewed_head_sha":OTHER,"verified_head_sha":HEAD,"base_sha":BASE,"observation":"Complete range changes only PR_DESCRIPTION.md with regular file modes"});
    validate(&auth, raw.clone()).unwrap();
    raw["review_range"]["base_sha"] = json!(OTHER);
    assert!(validate(&auth, raw).is_err());
}

#[test]
fn local_post_review_range_accepts_description_and_rejects_substantive_code() {
    use crate::commands::orch::autodrive::{review, trusted};
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let git = |args: &[&str]| trusted::git(repo, args).unwrap();
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Auto-drive contract test"]);
    git(&["config", "user.email", "autodrive@example.test"]);
    std::fs::write(repo.join("code.rs"), "fn main() {}\n").unwrap();
    git(&["add", "code.rs"]);
    git(&["commit", "-m", "base"]);
    let reviewed = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("PR_DESCRIPTION.md"), "Concrete validation\n").unwrap();
    git(&["add", "PR_DESCRIPTION.md"]);
    git(&["commit", "-m", "description evidence"]);
    let head = git(&["rev-parse", "HEAD"]);
    let auth = authorization("true", "false");
    let mut raw = candidate(&auth);
    raw["reviewed_head_sha"] = json!(reviewed);
    raw["binding"]["head_sha"] = json!(head);
    raw["binding"]["base_sha"] = json!(reviewed);
    let mut record: CandidateRecord = serde_json::from_value(raw).unwrap();
    review::verify(repo, &record).unwrap();
    std::fs::write(repo.join("code.rs"), "fn main() { panic!(); }\n").unwrap();
    git(&["add", "code.rs"]);
    git(&["commit", "-m", "substantive change"]);
    record.binding.head_sha = git(&["rev-parse", "HEAD"]);
    assert!(
        review::verify(repo, &record)
            .unwrap_err()
            .to_string()
            .contains("crusty-review-required")
    );
}

#[test]
fn retired_environment_cleanup_respects_explicit_command_configuration() {
    use crate::commands::orch::autodrive::trusted;
    use std::{ffi::OsStr, process::Command};
    let mut command = Command::new("unused");
    command.env(
        "BASH_ENV",
        "/old/amplifier-bundle/tools/gh-merge-guard/bash-env",
    );
    command.env("AMPLIHACK_MERGE_GUARD_USER_BASH_ENV", "/user/config.sh");
    trusted::clean_retired_environment(&mut command).unwrap();
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == "BASH_ENV")
            .unwrap()
            .1,
        Some(OsStr::new("/user/config.sh"))
    );
    command.env("BASH_ENV", "/explicit/config.sh");
    trusted::clean_retired_environment(&mut command).unwrap();
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == "BASH_ENV")
            .unwrap()
            .1,
        Some(OsStr::new("/explicit/config.sh"))
    );
}

/// A real local Git history and private evidence store exercise the same
/// reduction boundary as the helper, without hosting or workflow execution.
struct ReportingFixture {
    _temp: tempfile::TempDir,
    repo: std::path::PathBuf,
    directory: std::path::PathBuf,
    auth: InvocationAuthorization,
    envelope: Value,
    context: Value,
}
impl ReportingFixture {
    fn new() -> Self {
        Self::with_action("true")
    }
    fn with_action(no_merge: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git =
            |args: &[&str]| crate::commands::orch::autodrive::trusted::git(&repo, args).unwrap();
        git(&["init", "-b", "feature"]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "--allow-empty",
            "-m",
            "base",
        ]);
        let base = git(&["rev-parse", "HEAD"]);
        git(&["update-ref", "refs/remotes/origin/main", &base]);
        std::fs::write(repo.join("code.rs"), "fn implemented() {}\n").unwrap();
        git(&["add", "code.rs"]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-m",
            "implementation",
        ]);
        let head = git(&["rev-parse", "HEAD"]);
        let mut auth = authorization(no_merge, "false");
        let mut envelope = json!({"candidate":candidate(&auth), "finalization":finalization(&auth), "completion":completion()});
        fn replace(value: &mut Value, head: &str, base: &str) {
            match value {
                Value::String(s) if s == HEAD => *s = head.into(),
                Value::String(s) if s == BASE => *s = base.into(),
                Value::Array(a) => a.iter_mut().for_each(|v| replace(v, head, base)),
                Value::Object(o) => o.values_mut().for_each(|v| replace(v, head, base)),
                _ => (),
            }
        }
        if no_merge == "false" {
            envelope["finalization"]["requested_action"] = json!("merge");
            envelope["finalization"]["request_expected_head_sha"] = json!(HEAD);
            envelope["finalization"]["merge_requests"] = json!(1);
            envelope["finalization"]["request_outcome"] = json!("accepted");
            envelope["finalization"]["platform_state"] = json!("merged");
            envelope["finalization"]["confirmation_evidence"] =
                json!("Observed merged=true on the exact PR");
        }
        replace(&mut envelope, &head, &base);
        let directory = temp.path().join("state");
        let store = EvidenceStore::create(&directory).unwrap();
        // The runner, not the child context, assigns and binds the directory.
        auth.bind_state_dir(directory.clone());
        let accepted = store
            .snapshot(&serde_json::to_vec(&envelope["candidate"]).unwrap())
            .unwrap();
        store
            .write_new(
                "merge-ready-round-1.json",
                &serde_json::to_vec(
                    &json!({"merge_ready_verdict":"MERGE_READY", "accepted_evidence":accepted}),
                )
                .unwrap(),
            )
            .unwrap();
        store.write_new("merge-ready-result.json", &serde_json::to_vec(&json!({"loop_result":"DONE", "round_label":"round-1", "round_verdict":"MERGE_READY"})).unwrap()).unwrap();
        crate::commands::orch::autodrive::completion::record(&auth, &directory, "round-1").unwrap();
        let context = json!({"autodrive_state_dir":directory, "repo_path":repo, "accepted_readiness":{"accepted":accepted}, "finalization_observations":envelope});
        Self {
            _temp: temp,
            repo,
            directory,
            auth,
            envelope,
            context,
        }
    }
    fn reduce(&self) -> anyhow::Result<crate::commands::orch::autodrive::records::Outcome> {
        let store = EvidenceStore::create(&self.directory)?;
        crate::commands::orch::autodrive::helper::validate_reduction(
            &self.auth,
            &store,
            &self.directory,
            &self.repo,
            std::path::Path::new(
                self.context["accepted_readiness"]["accepted"]
                    .as_str()
                    .unwrap(),
            ),
            &serde_json::from_value(self.envelope.clone())?,
        )
    }
    fn report(&self) -> Value {
        let mut context = self.context.clone();
        // Exercise the raw agent string path used by the actual recipe.
        context["finalization_observations"] = json!(self.envelope.to_string());
        crate::commands::orch::autodrive::report::report(
            &self.auth,
            context.as_object().unwrap(),
            None,
            self._temp.path(),
        )
    }
    fn accept_current(&mut self) {
        let store = EvidenceStore::create(&self.directory).unwrap();
        let path = store
            .snapshot(&serde_json::to_vec(&self.envelope["candidate"]).unwrap())
            .unwrap();
        self.context["accepted_readiness"]["accepted"] = json!(path);
    }
    fn seal_reduction(&mut self) {
        let outcome = self.reduce().unwrap();
        let store = EvidenceStore::create(&self.directory).unwrap();
        let path = store
            .snapshot(&serde_json::to_vec(&outcome).unwrap())
            .unwrap();
        self.context["autodrive_finalization"] = json!({"outcome":outcome, "outcome_snapshot":path, "accepted_snapshot":self.context["accepted_readiness"]["accepted"], "repo":self.repo});
    }
    fn assert_rejected(&self, cause: &str) {
        let error = format!("{:#}", self.reduce().unwrap_err());
        assert!(error.contains(cause), "{error}");
        let report = self.report();
        assert_eq!(report["readiness"], "incomplete", "{report}");
        assert_eq!(report["action"], "not_requested", "{report}");
        assert_ne!(report["stop_reason"], "completed", "{report}");
        assert!(report["blockers"].to_string().contains(cause), "{report}");
    }
}

#[test]
fn reporting_preserves_reducer_missing_and_altered_artifact_rejections() {
    use sha2::{Digest, Sha256};
    for no_merge in ["true", "false"] {
        let mut fixture = ReportingFixture::with_action(no_merge);
        fixture.envelope["candidate"]["criteria"][0]["evidence"][0]["reference"] =
            json!({"file":"qa.log", "sha256":format!("{:x}", Sha256::digest(b"passed"))});
        fixture.accept_current();
        // A failed reducer leaves the recipe's empty output default intact.
        fixture.context["autodrive_finalization"] = json!("");
        fixture.assert_rejected("unreadable evidence artifact qa.log");
        let store = EvidenceStore::create(&fixture.directory).unwrap();
        store.write_new("qa.log", b"altered").unwrap();
        fixture.assert_rejected("missing, stale, or altered evidence artifact qa.log");
    }
}

#[test]
fn reporting_cannot_replace_the_accepted_readiness_snapshot() {
    let mut fixture = ReportingFixture::new();
    fixture.seal_reduction();
    fixture.envelope["candidate"]["criteria"][0]["evidence"][0]["observation"] =
        json!("Changed after acceptance");
    fixture.assert_rejected("finalization changed accepted readiness after the action");
}

#[test]
fn reporting_enforces_local_head_and_post_review_coverage() {
    let mut fixture = ReportingFixture::new();
    let base = fixture.envelope["candidate"]["binding"]["base_sha"].clone();
    fixture.envelope["candidate"]["reviewed_head_sha"] = base.clone();
    fixture.envelope["candidate"]["review_range"] = json!({
        "reviewed_head_sha":base, "verified_head_sha":fixture.envelope["candidate"]["binding"]["head_sha"],
        "base_sha":base, "observation":"Agent claims post-review changes are allowed"
    });
    fixture.accept_current();
    fixture.assert_rejected("crusty-review-required");

    let fixture = ReportingFixture::new();
    crate::commands::orch::autodrive::trusted::git(
        &fixture.repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "--allow-empty",
            "-m",
            "moved head",
        ],
    )
    .unwrap();
    fixture.assert_rejected("evidence head differs from current worktree");
}

#[test]
fn reporting_requires_successful_reduction_and_matching_private_outcome() {
    for no_merge in ["true", "false"] {
        let mut fixture = ReportingFixture::with_action(no_merge);
        fixture.reduce().unwrap();
        let missing = fixture.report();
        assert_eq!(missing["readiness"], "incomplete");
        assert!(
            missing["blockers"]
                .to_string()
                .contains("successful deterministic reduction missing")
        );
        fixture.seal_reduction();
        let report = fixture.report();
        assert_eq!(report["readiness"], "merge_ready", "{report}");
        assert_eq!(
            report["action"],
            if no_merge == "true" {
                "not_requested"
            } else {
                "merged"
            },
            "{report}"
        );
        assert_eq!(report["stop_reason"], "completed", "{report}");
        let mut outcome = fixture.reduce().unwrap();
        outcome.blockers.push("Unrelated reduction".into());
        let store = EvidenceStore::create(&fixture.directory).unwrap();
        fixture.context["autodrive_finalization"]["outcome_snapshot"] = json!(
            store
                .snapshot(&serde_json::to_vec(&outcome).unwrap())
                .unwrap()
        );
        let report = fixture.report();
        assert_eq!(report["readiness"], "incomplete");
        assert!(
            report["blockers"]
                .to_string()
                .contains("reported outcome differs from deterministic reduction")
        );
    }
}

// #2329 Crusty round 2, finding runner-report-bypasses-reducer (C7(b)).
// The runner assigns and binds the private state directory. The report opens
// evidence only from that binding and compares any child-reported directory
// lexically; it never joins a child value onto the working directory.

const STATE_DIR_REFUSAL: &str = "autodrive_state_dir must be an absolute path";
const STATE_DIR_MISMATCH: &str = "runner-assigned state directory";

fn capture_with_state_dir(value: &str) -> anyhow::Result<InvocationAuthorization> {
    InvocationAuthorization::capture(
        "auto-drive-to-merge",
        &BTreeMap::from([
            ("no_merge".into(), "true".into()),
            ("autodrive_state_dir".into(), value.into()),
        ]),
    )
}

fn assert_state_dir_rejected(fixture: &ReportingFixture, label: &str) {
    let report = fixture.report();
    assert_eq!(report["readiness"], "incomplete", "{label}: {report}");
    assert_eq!(report["action"], "not_requested", "{label}: {report}");
    assert_ne!(report["action"], "merged", "{label}: {report}");
    assert_ne!(report["stop_reason"], "completed", "{label}: {report}");
    let cause = report["failure_cause"].as_str().unwrap_or_default();
    assert!(
        cause.contains(STATE_DIR_MISMATCH),
        "{label}: failure_cause must name the state directory mismatch: {report}"
    );
}

#[test]
fn explicit_state_directory_must_be_absolute_and_free_of_traversal() {
    for value in [
        "",
        "state",
        "./state",
        "../x",
        "/srv/autodrive/../escape",
        "/srv/auto\ndrive",
        "/srv/auto\u{1b}[0mdrive",
    ] {
        let error = format!(
            "{:#}",
            capture_with_state_dir(value).expect_err(&format!("accepted {value:?}"))
        );
        assert!(error.contains(STATE_DIR_REFUSAL), "{value:?}: {error}");
        // Control characters must not reach the operator's terminal raw.
        assert!(
            !error.contains('\n') && !error.contains('\u{1b}'),
            "{value:?}: refusal echoed raw control characters: {error:?}"
        );
    }
    let accepted = capture_with_state_dir("/srv/autodrive/state").unwrap();
    assert!(accepted.no_merge());
}

#[test]
fn bound_state_directory_is_serialised_and_survives_private_invocation_read() {
    let dir = tempfile::tempdir().unwrap();
    let store_root = dir.path().join("invocation");
    let store = EvidenceStore::create(&store_root).unwrap();
    let mut auth = authorization("true", "false");
    assert_eq!(
        auth.state_dir(),
        None,
        "capture must not invent a directory"
    );
    let assigned = dir.path().join("autodrive-state");
    auth.bind_state_dir(assigned.clone());
    auth.bind_policy(format!("trusted-launch-bundle:sha256:{}", "a".repeat(64)));
    assert_eq!(auth.state_dir(), Some(assigned.as_path()));
    let bytes = serde_json::to_vec(&auth).unwrap();
    let raw: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(raw["state_dir"], json!(assigned));
    store.write_new("invocation.json", &bytes).unwrap();
    let read = InvocationAuthorization::read_private(&store_root.join("invocation.json")).unwrap();
    assert_eq!(read.state_dir(), Some(assigned.as_path()));
    assert_eq!(read.run_id(), auth.run_id());
}

#[test]
fn reporting_reads_runner_assigned_state_directory_when_context_omits_it() {
    for omitted in [None, Some(json!(""))] {
        let mut fixture = ReportingFixture::new();
        fixture.seal_reduction();
        let context = fixture.context.as_object_mut().unwrap();
        match &omitted {
            None => {
                context.remove("autodrive_state_dir");
            }
            Some(value) => {
                context.insert("autodrive_state_dir".into(), value.clone());
            }
        }
        let report = fixture.report();
        assert_eq!(report["readiness"], "merge_ready", "{omitted:?}: {report}");
        assert_eq!(report["stop_reason"], "completed", "{omitted:?}: {report}");
    }
}

#[test]
fn reporting_without_a_bound_state_directory_is_incomplete() {
    let mut fixture = ReportingFixture::new();
    fixture.seal_reduction();
    // A resumed authority that the runner never bound to a directory: the
    // context still names the correct directory, which must not be enough.
    fixture.auth = authorization("true", "false");
    let report = fixture.report();
    assert_eq!(report["readiness"], "incomplete", "{report}");
    assert_eq!(report["action"], "not_requested", "{report}");
    assert!(
        report["failure_cause"]
            .as_str()
            .is_some_and(|c| c.contains("state directory")),
        "{report}"
    );
}

#[test]
fn reporting_rejects_state_directory_outside_runner_assignment() {
    // The outside directory is itself a complete, valid evidence store for
    // another drive; pointing the child context at it must still fail.
    let mut fixture = ReportingFixture::with_action("false");
    fixture.seal_reduction();
    let mut outside = ReportingFixture::with_action("false");
    outside.seal_reduction();
    fixture.context["autodrive_state_dir"] = json!(outside.directory);
    assert_state_dir_rejected(&fixture, "outside store");
    let report = fixture.report();
    assert_ne!(report["action"], "merged", "{report}");
}

#[test]
fn reporting_rejects_relative_or_altered_state_directory() {
    type Case = fn(&std::path::Path) -> Value;
    let cases: [(&str, Case); 7] = [
        ("parent traversal", |_| json!("../x")),
        // Joined onto the working directory this would resolve to the right
        // store; a relative name is still refused.
        ("relative name", |_| json!("state")),
        ("parent component in absolute path", |dir| {
            json!(dir.join("..").join("state"))
        }),
        ("sibling directory", |dir| {
            let sibling = dir.with_file_name("state-sibling");
            std::fs::create_dir(&sibling).unwrap();
            json!(sibling)
        }),
        ("json object", |dir| json!({"path": dir})),
        ("json number", |_| json!(7)),
        ("json array", |dir| json!([dir])),
    ];
    for (label, value) in cases {
        let mut fixture = ReportingFixture::new();
        fixture.seal_reduction();
        fixture.context["autodrive_state_dir"] = value(&fixture.directory);
        assert_state_dir_rejected(&fixture, label);
    }
}

#[test]
fn reporting_rejects_mismatched_build_result_state_directory() {
    for (label, value) in [
        ("relative", json!("state")),
        ("other absolute", json!("/nonexistent/autodrive-state")),
        ("non-string", json!({"dir": "state"})),
    ] {
        let mut fixture = ReportingFixture::new();
        fixture.seal_reduction();
        fixture.context["build_result"] = json!({"repo": fixture.repo, "state_dir": value});
        assert_state_dir_rejected(&fixture, label);
    }
    // A matching or empty build_result.state_dir does not block readiness.
    for value in [None, Some(json!(""))] {
        let mut fixture = ReportingFixture::new();
        fixture.seal_reduction();
        let value = value.unwrap_or_else(|| json!(fixture.directory));
        fixture.context["build_result"] = json!({"repo": fixture.repo, "state_dir": value});
        let report = fixture.report();
        assert_eq!(report["readiness"], "merge_ready", "{report}");
    }
}

#[cfg(unix)]
#[test]
fn reporting_refuses_a_symlinked_state_directory_leaf() {
    let mut fixture = ReportingFixture::new();
    fixture.seal_reduction();
    let alias = fixture.directory.with_file_name("state-alias");
    std::os::unix::fs::symlink(&fixture.directory, &alias).unwrap();
    // Even when runner binding and context agree, a symlinked leaf is refused
    // rather than followed.
    fixture.auth.bind_state_dir(alias.clone());
    fixture.context["autodrive_state_dir"] = json!(alias);
    let report = fixture.report();
    assert_eq!(report["readiness"], "incomplete", "{report}");
    assert_eq!(report["action"], "not_requested", "{report}");
}

#[test]
fn resumed_run_cannot_replay_prior_finalization_or_receipt() {
    let mut fixture = ReportingFixture::with_action("false");
    fixture.seal_reduction();
    let original = fixture.report();
    assert_eq!(original["readiness"], "merge_ready", "{original}");
    assert_eq!(original["action"], "merged", "{original}");
    // A resume captures new authority (new run_id) and, with an operator's
    // shared directory, is bound to the same store. The prior run's
    // finalization observations, completion receipt and outcome snapshot are
    // all still present and self-consistent.
    let mut resumed = authorization("false", "false");
    assert_ne!(resumed.run_id(), fixture.auth.run_id());
    resumed.bind_state_dir(fixture.directory.clone());
    fixture.auth = resumed;
    let report = fixture.report();
    assert_eq!(report["readiness"], "incomplete", "{report}");
    assert_eq!(report["action"], "not_requested", "{report}");
    assert_ne!(report["stop_reason"], "completed", "{report}");
    assert_eq!(report["run_id"], json!(fixture.auth.run_id()), "{report}");
    assert!(
        report["failure_cause"]
            .as_str()
            .is_some_and(|c| !c.trim().is_empty()),
        "replay rejection must keep its cause: {report}"
    );
}

// #2329 merged with #2345: main's Crusty objective-completion path ends the
// Crusty phase with `loop_result=DONE`, `source=objective-completion` and no
// evaluator call. That result is not a `LoopCompletion` and must neither
// block nor replace the merge-ready requirement: readiness still needs the
// fresh `MERGE_READY` + `DONE` receipt of the merge-ready loop.

const CRUSTY_OBJECTIVE_COMPLETION: &str =
    r#"{"loop_result":"DONE","source":"objective-completion","evaluator_invoked":false}"#;

fn crusty_evaluator_records(directory: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(directory)
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.contains("crusty") && name.contains("evaluat"))
        .collect()
}

#[test]
fn crusty_objective_completion_reaches_merge_ready_reduction() {
    for no_merge in ["true", "false"] {
        let mut fixture = ReportingFixture::with_action(no_merge);
        fixture.context["crusty_loop_result"] = json!(CRUSTY_OBJECTIVE_COMPLETION);
        fixture.seal_reduction();
        assert!(
            crusty_evaluator_records(&fixture.directory).is_empty(),
            "fixture must not contain a Crusty evaluator record"
        );
        let report = fixture.report();
        assert_eq!(report["readiness"], "merge_ready", "{no_merge}: {report}");
        assert_eq!(report["stop_reason"], "completed", "{no_merge}: {report}");
        assert_eq!(
            report["action"],
            if no_merge == "true" {
                "not_requested"
            } else {
                "merged"
            },
            "{no_merge}: {report}"
        );
    }
}

#[test]
fn crusty_objective_completion_without_merge_ready_receipt_stays_incomplete() {
    for no_merge in ["true", "false"] {
        let mut fixture = ReportingFixture::with_action(no_merge);
        fixture.context["crusty_loop_result"] = json!(CRUSTY_OBJECTIVE_COMPLETION);
        // The merge-ready loop never completed: no fresh receipt exists, and
        // the reducer cannot have succeeded, so the recipe default is empty.
        let receipt = fixture
            .directory
            .join(format!("loop-{}-merge-ready.json", fixture.auth.run_id()));
        std::fs::remove_file(&receipt).unwrap();
        fixture.context["autodrive_finalization"] = json!("");
        // The reducer refuses without the receipt and names what is missing.
        let error = fixture
            .reduce()
            .expect_err("reduced without a merge-ready receipt");
        assert!(
            format!("{error:#}").contains("fresh loop completion receipt required"),
            "{no_merge}: {error:#}"
        );
        let report = fixture.report();
        assert_eq!(report["readiness"], "incomplete", "{no_merge}: {report}");
        assert_eq!(report["action"], "not_requested", "{no_merge}: {report}");
        assert_ne!(report["stop_reason"], "completed", "{no_merge}: {report}");
        // A completed Crusty phase is not a stop cause of its own: it must
        // not be mapped to no_progress, usage_limit or another loop failure.
        assert_eq!(
            report["stop_reason"], "dependency_failure",
            "{no_merge}: {report}"
        );
    }
}

// ── #2546: umask 002 and repair of our own state ──
//
// The umask is per process and sibling tests create files concurrently, so the
// umask tests run alone in a child of the test binary. The parent test process
// never changes its umask.

#[cfg(unix)]
const UMASK_2546_CHILD: &str = "AMPLIHACK_ISSUE_2546_UMASK_CHILD";

/// The log line the evidence check writes for each repaired entry.
#[cfg(unix)]
const REPAIR_2546: &str = "removed group and other access";

/// The refusal for an entry with extra bits outside the state tree (row 7).
const OUTSIDE_2546: &str = "evidence outside the auto-drive state tree has group or other access";

/// Runs `body` in a child test process under `mask`, and returns its stderr.
#[cfg(unix)]
fn under_umask_2546(test: &str, mask: libc::mode_t, body: impl FnOnce()) -> Option<String> {
    if std::env::var_os(UMASK_2546_CHILD).is_some() {
        // SAFETY: umask has no failure mode, and this child runs only this test.
        let old = unsafe { libc::umask(mask) };
        body();
        unsafe { libc::umask(old) };
        return None;
    }
    let name = format!("commands::orch::autodrive_contract_tests::{test}");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--test-threads=1", "--nocapture"])
        .env(UMASK_2546_CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "{name} failed under umask {mask:o}:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("test result: ok. 1 passed"),
        "re-running {name} ran no test:\n{stdout}"
    );
    Some(stderr)
}

#[cfg(unix)]
fn mode_2546(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

/// Every folder at or below `root` is 0700 and every file 0600.
#[cfg(unix)]
fn assert_private_tree_2546(root: &std::path::Path) {
    let meta = std::fs::symlink_metadata(root).unwrap();
    if meta.is_dir() {
        assert_eq!(mode_2546(root), 0o700, "folder {}", root.display());
        for entry in std::fs::read_dir(root).unwrap() {
            assert_private_tree_2546(&entry.unwrap().path());
        }
    } else {
        assert!(meta.is_file(), "{} is not a regular file", root.display());
        assert_eq!(mode_2546(root), 0o600, "file {}", root.display());
    }
}

#[cfg(unix)]
fn repair_lines_2546(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.contains(REPAIR_2546)).collect()
}

/// Stand-in for the amplihack binary the completion context script calls. It
/// records the mode of each --context-from-file target and of its folder while
/// they still exist (the script removes them afterwards).
#[cfg(unix)]
const CONTEXT_STUB_2546: &str = r#"#!/usr/bin/env bash
mode() { stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"; }
while [ $# -gt 0 ]; do
  if [ "$1" = --context-from-file ]; then
    p="${2#*=}"
    printf '%s %s %s\n' "$(mode "$p")" "$(mode "$(dirname "$p")")" "${2%%=*}" >> "$STUB_MODES"
    shift
  fi
  shift
done
printf '{}'
"#;

/// Runs the shell writers under `umask 002` against `state`: the phase marker
/// from autodrive_state.sh and the completion_context writer. Returns the
/// recorded "<file mode> <folder mode> <name>" lines from the stub.
#[cfg(unix)]
fn run_shell_writers_2546(state: &std::path::Path) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let tools = concat!(env!("CARGO_MANIFEST_DIR"), "/../../amplifier-bundle/tools");
    let stub_dir = tempfile::tempdir().unwrap();
    let stub = stub_dir.path().join("amplihack-stub");
    std::fs::write(&stub, CONTEXT_STUB_2546).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
    let modes = stub_dir.path().join("modes.txt");
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(
            r#"umask 002
field() { printf 'stub'; }
. "$TOOLS/autodrive_state.sh" || exit 3
autodrive_mark_phase_done "$STATE_DIR" crusty-loop || exit 4
run_context() { . "$TOOLS/autodrive_completion_context.sh"; }
run_context || exit 5
"#,
        )
        .env("TOOLS", tools)
        .env("STATE_DIR", state)
        .env("AMPLIHACK_BIN", &stub)
        .env("STUB_MODES", &modes)
        .env("HISTORY", "history")
        .env("BASELINE", "baseline")
        .env("PREV_FINDINGS", "findings")
        .env("PREV_TEST", "test")
        .env("PREV_CI", "ci")
        .env("TOKEN_SAFE", "token")
        .env("ROUND_LABEL", "round-1")
        .env("RECORD", state.join("record.json"))
        .env("REPO", state)
        .env("REVIEW_REAL", state)
        .env("LOOP_HOME", tools)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "shell writers failed ({:?}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<String> = std::fs::read_to_string(&modes)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(!lines.is_empty(), "the context stub recorded nothing");
    lines
}

/// (a) One round written under umask 002 through every writer of state the
/// evidence check reads, then the next evidence check. The Rust writers and
/// the shell writers need no repair; the one plain `fs::write` file, standing
/// in for a file an agent step wrote, is repaired and logged.
#[cfg(unix)]
#[test]
fn issue_2546_round_written_under_umask_002_passes_next_evidence_check() {
    use crate::commands::orch::autodrive::helper::write_new_private;
    use crate::commands::orch::autodrive_completion::secure_fs::{self, Scope};
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let stderr = under_umask_2546(
        "issue_2546_round_written_under_umask_002_passes_next_evidence_check",
        0o002,
        || {
            let dir = tempfile::tempdir().unwrap();
            // A nested parent that does not exist yet.
            let state = dir.path().join("auto-drive").join("autodrive-state");
            let auth = authorization("true", "false");
            let expected: Binding = serde_json::from_value(binding(&auth)).unwrap();

            // Round N, Rust writers: the evidence store and its records.
            let store = EvidenceStore::create(&state).unwrap();
            let state = state.canonicalize().unwrap();
            store
                .write_new(
                    "candidate.json",
                    &serde_json::to_vec(&candidate(&auth)).unwrap(),
                )
                .unwrap();
            let accepted = store
                .accept_candidate("candidate.json", &auth, &expected)
                .unwrap();
            let snapshot = store.snapshot(b"{}").unwrap();
            write_new_private(&state.join("loop-1.json"), b"{}").unwrap();

            // Round N, Rust writers: the completion tree, laid out as
            // private_state.rs lays it out.
            let completion = tempfile::Builder::new()
                .prefix(".completion-")
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in(&state)
                .unwrap()
                .keep();
            for part in ["driver", "output"] {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(completion.join(part))
                    .unwrap();
            }
            let private = completion.join("driver").join("state.json");
            let decision = completion.join("output").join("decision.json");
            secure_fs::create(Scope::of(&state), &private, b"{}").unwrap();
            secure_fs::output(Scope::of(&state), &decision)
                .unwrap()
                .write_all(b"{}")
                .unwrap();

            // Round N, shell writers.
            let context = run_shell_writers_2546(&state);
            assert_eq!(context.len(), 6, "context files seen: {context:?}");
            for line in &context {
                assert!(
                    line.starts_with("600 700 "),
                    "completion_context wrote a file not private under umask 002: {line}"
                );
            }
            assert_eq!(mode_2546(&state.join("phases.tsv")), 0o600);

            // Round N, a file written without an explicit mode (agent step).
            let agent = state.join("agent-notes.json");
            std::fs::write(&agent, b"{}").unwrap();
            assert_eq!(mode_2546(&agent), 0o664, "umask 002 is in force");

            // Round N+1: the next evidence check reads the same state.
            let next = EvidenceStore::create(&state).unwrap();
            next.accept_candidate("candidate.json", &auth, &expected)
                .unwrap();
            for name in [
                "candidate.json".to_owned(),
                "loop-1.json".to_owned(),
                "phases.tsv".to_owned(),
                "agent-notes.json".to_owned(),
                accepted
                    .path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                snapshot.file_name().unwrap().to_string_lossy().into_owned(),
            ] {
                next.read_private(&name)
                    .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            }
            secure_fs::check_dir(&state).unwrap();
            assert_eq!(
                secure_fs::read(Scope::of(&state), &private, 1 << 20).unwrap(),
                b"{}"
            );
            assert_eq!(
                secure_fs::read(Scope::of(&state), &decision, 1 << 20).unwrap(),
                b"{}"
            );
            assert_private_tree_2546(&state);
        },
    );
    if let Some(stderr) = stderr {
        let lines = repair_lines_2546(&stderr);
        assert_eq!(
            lines.len(),
            1,
            "only the plain-write file needs repair:\n{stderr}"
        );
        let line = lines[0];
        assert!(
            line.starts_with("INFO: ")
                && line.contains("agent-notes.json\"")
                && line.contains("0664")
                && line.contains("(gid ")
                && line.contains("0600"),
            "{line}"
        );
    }
}

/// (b) A record chmod'ed to 0664 is repaired to 0600 and logged.
#[cfg(unix)]
#[test]
fn issue_2546_hand_chmod_0664_is_repaired_and_logged() {
    use std::os::unix::fs::PermissionsExt;
    let stderr = under_umask_2546(
        "issue_2546_hand_chmod_0664_is_repaired_and_logged",
        0o022,
        || {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("autodrive-state");
            let store = EvidenceStore::create(&root).unwrap();
            store.write_new("record.json", b"{}").unwrap();
            let file = root.join("record.json");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o664)).unwrap();
            assert_eq!(store.read_private("record.json").unwrap(), b"{}");
            assert_eq!(mode_2546(&file), 0o600);
            // A second read finds nothing left to repair.
            assert_eq!(store.read_private("record.json").unwrap(), b"{}");
        },
    );
    if let Some(stderr) = stderr {
        let lines = repair_lines_2546(&stderr);
        assert_eq!(lines.len(), 1, "one repair, logged once:\n{stderr}");
        let line = lines[0];
        assert!(
            line.starts_with("INFO: ")
                && line.contains("record.json\"")
                && line.contains("0664")
                && line.contains("(gid ")
                && line.contains("0600"),
            "{line}"
        );
    }
}

/// (b) The state folder chmod'ed to 0775 is repaired to 0700 when the
/// evidence store is opened again.
#[cfg(unix)]
#[test]
fn issue_2546_state_folder_0775_is_repaired_and_logged() {
    use std::os::unix::fs::PermissionsExt;
    let stderr = under_umask_2546(
        "issue_2546_state_folder_0775_is_repaired_and_logged",
        0o022,
        || {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("autodrive-state");
            EvidenceStore::create(&root)
                .unwrap()
                .write_new("record.json", b"{}")
                .unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o775)).unwrap();
            let store = EvidenceStore::create(&root).unwrap();
            assert_eq!(mode_2546(&root), 0o700);
            assert_eq!(store.read_private("record.json").unwrap(), b"{}");
        },
    );
    if let Some(stderr) = stderr {
        let lines = repair_lines_2546(&stderr);
        assert_eq!(lines.len(), 1, "one repair, logged once:\n{stderr}");
        let line = lines[0];
        assert!(
            line.starts_with("INFO: ")
                && line.contains("autodrive-state\"")
                && line.contains("0775")
                && line.contains("0700"),
            "{line}"
        );
    }
}

/// S1: outside the auto-drive state root, the completion checker stays strict
/// and changes nothing, even when the caller passes a repairable root.
#[cfg(unix)]
#[test]
fn issue_2546_outside_state_tree_stays_strict() {
    use crate::commands::orch::autodrive_completion::secure_fs::{self, Scope};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    // A repairable state root next to the folders read: they are outside it.
    let state = dir.path().join("autodrive-state");
    std::fs::create_dir(&state).unwrap();
    let scope = Scope::of(&state);
    assert!(matches!(scope, Scope::StateTree(_)));
    // An open folder holding an open file.
    let open = dir.path().join("repo");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    let file = open.join("notes.txt");
    std::fs::write(&file, b"notes").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let error = secure_fs::read(scope, &file, 1 << 20).expect_err("read outside the state tree");
    assert!(format!("{error:#}").contains(OUTSIDE_2546), "{error:#}");
    assert_eq!(mode_2546(&open), 0o755, "folder outside the tree changed");
    assert_eq!(mode_2546(&file), 0o644, "file outside the tree changed");

    // A private folder holding an open file: the file is still refused.
    let closed = dir.path().join("private");
    std::fs::create_dir(&closed).unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();
    let file = closed.join("notes.txt");
    std::fs::write(&file, b"notes").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let error = secure_fs::read(scope, &file, 1 << 20).expect_err("read outside the state tree");
    assert!(format!("{error:#}").contains(OUTSIDE_2546), "{error:#}");
    assert_eq!(mode_2546(&file), 0o644, "file outside the tree changed");

    // The same file made private by its owner is read.
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(secure_fs::read(scope, &file, 1 << 20).unwrap(), b"notes");
}

/// (c) The pure rule: foreign owner, foreign group, world-writable, symlink,
/// wrong type and hard links are refused; repair happens only where allowed.
#[test]
fn issue_2546_foreign_owner_symlink_and_hard_link_are_still_refused() {
    use crate::commands::orch::autodrive::private_mode::{Access, Entry, Kind, Repair, access};
    const SYMLINK: &str = "evidence is a symlink";
    const TYPE: &str = "evidence is not a regular file or state directory";
    const OWNER: &str = "untrusted evidence ownership or permissions";
    const LINKS: &str = "evidence has unexpected hard links";
    let (me, group) = (1000, 1000);
    // The table below lists (entry, dir, single_link, repair) as booleans;
    // this maps them onto `Kind` and `Repair`.
    let check = |entry: Entry, dir: bool, single_link: bool, repair: bool| {
        let kind = match (dir, single_link) {
            (true, _) => Kind::Dir,
            (false, true) => Kind::SingleLinkFile,
            (false, false) => Kind::File,
        };
        let repair = if repair {
            Repair::Allowed
        } else {
            Repair::Refuse
        };
        access(entry, kind, me, group, repair)
    };
    let file = Entry {
        symlink: false,
        is_dir: false,
        is_file: true,
        uid: me,
        gid: group,
        mode: 0o664,
        nlink: 1,
    };
    let folder = Entry {
        is_dir: true,
        is_file: false,
        mode: 0o775,
        nlink: 3,
        ..file
    };

    // Repaired: this user's entries with extra bits, in the state tree.
    assert_eq!(check(file, false, true, true), Access::Repair(0o600));
    assert_eq!(check(folder, true, true, true), Access::Repair(0o700));
    assert_eq!(
        check(
            Entry {
                mode: 0o640,
                ..file
            },
            false,
            true,
            true
        ),
        Access::Repair(0o600)
    );
    // The setgid bit goes with the group bits.
    assert_eq!(
        check(
            Entry {
                mode: 0o2775,
                ..folder
            },
            true,
            true,
            true
        ),
        Access::Repair(0o700)
    );
    // Other-read only, with a foreign group: no group bit trusts that group.
    assert_eq!(
        check(
            Entry {
                mode: 0o705,
                gid: group + 1,
                ..folder
            },
            true,
            true,
            true
        ),
        Access::Repair(0o700)
    );

    // Already private.
    assert_eq!(
        check(
            Entry {
                mode: 0o600,
                ..file
            },
            false,
            true,
            true
        ),
        Access::Private
    );
    assert_eq!(
        check(
            Entry {
                mode: 0o700,
                ..folder
            },
            true,
            true,
            true
        ),
        Access::Private
    );
    assert_eq!(
        check(
            Entry {
                mode: 0o600,
                ..file
            },
            false,
            true,
            false
        ),
        Access::Private
    );

    // Refused, each with its reason, whether or not repair is allowed.
    let refused = [
        (
            Entry {
                uid: me + 1,
                ..file
            },
            false,
            true,
            OWNER,
        ),
        (
            Entry {
                uid: me + 1,
                mode: 0o600,
                ..file
            },
            false,
            true,
            OWNER,
        ),
        (
            Entry {
                uid: me + 1,
                ..folder
            },
            true,
            true,
            OWNER,
        ),
        (
            Entry {
                mode: 0o666,
                ..file
            },
            false,
            true,
            OWNER,
        ),
        (
            Entry {
                mode: 0o602,
                ..file
            },
            false,
            true,
            OWNER,
        ),
        (
            Entry {
                mode: 0o777,
                ..folder
            },
            true,
            true,
            OWNER,
        ),
        (
            Entry {
                gid: group + 1,
                ..file
            },
            false,
            true,
            OWNER,
        ),
        (
            Entry {
                gid: group + 1,
                mode: 0o750,
                ..folder
            },
            true,
            true,
            OWNER,
        ),
        (
            Entry {
                symlink: true,
                ..file
            },
            false,
            true,
            SYMLINK,
        ),
        (
            Entry {
                symlink: true,
                ..folder
            },
            true,
            true,
            SYMLINK,
        ),
        (file, true, true, TYPE),
        (folder, false, true, TYPE),
        (
            Entry {
                is_file: false,
                ..file
            },
            false,
            true,
            TYPE,
        ),
        (
            Entry {
                nlink: 2,
                mode: 0o600,
                ..file
            },
            false,
            true,
            LINKS,
        ),
        (Entry { nlink: 2, ..file }, false, true, LINKS),
        (Entry { nlink: 2, ..file }, false, false, LINKS),
    ];
    for repair in [true, false] {
        for (entry, dir, single_link, reason) in refused {
            assert_eq!(
                check(entry, dir, single_link, repair),
                Access::Refuse(reason),
                "{entry:?} (dir {dir}, single_link {single_link}, repair {repair})"
            );
        }
    }

    // The completion checker allows a clean hard-linked file but never
    // repairs one.
    assert_eq!(
        check(
            Entry {
                nlink: 2,
                mode: 0o600,
                ..file
            },
            false,
            false,
            true
        ),
        Access::Private
    );

    // Extra bits where repair is not allowed: the old strict refusal, with
    // its own reason.
    assert_eq!(
        check(file, false, true, false),
        Access::Refuse(OUTSIDE_2546)
    );
    assert_eq!(
        check(folder, true, true, false),
        Access::Refuse(OUTSIDE_2546)
    );
    assert_eq!(
        check(
            Entry {
                mode: 0o755,
                ..folder
            },
            true,
            false,
            false
        ),
        Access::Refuse(OUTSIDE_2546)
    );
}

// ── #2546: repairs stay at or below a repairable state root (S1) ──

/// Creates `path` as a folder (or a file holding `bytes`) with `mode`.
#[cfg(unix)]
fn make_2546(path: &std::path::Path, bytes: Option<&[u8]>, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    match bytes {
        Some(bytes) => std::fs::write(path, bytes).unwrap(),
        None => std::fs::create_dir(path).unwrap(),
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Runs `amplihack orch auto-drive-completion prepare` with `args`.
#[cfg(unix)]
fn prepare_2546(args: &[&str]) -> anyhow::Result<()> {
    use crate::commands::orch::autodrive_completion::{Prepare, prepare};
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        prepare: Prepare,
    }
    let cli = Cli::try_parse_from(std::iter::once("prepare").chain(args.iter().copied())).unwrap();
    prepare(cli.prepare)
}

/// (i) A `.completion-*` part does not make its parent folder repairable: a
/// 0755 folder `x` above `x/.completion-y/f` is refused and left at 0755,
/// with no root and with a root that is not `x`.
#[cfg(unix)]
#[test]
fn issue_2546_completion_part_outside_state_root_stays_strict() {
    use crate::commands::orch::autodrive_completion::secure_fs::{self, Scope};
    let dir = tempfile::tempdir().unwrap();
    let x = dir.path().join("x");
    make_2546(&x, None, 0o755);
    make_2546(&x.join(".completion-y"), None, 0o700);
    let file = x.join(".completion-y").join("f");
    make_2546(&file, Some(b"f"), 0o600);
    let other = dir.path().join("autodrive-state");
    make_2546(&other, None, 0o700);
    for scope in [Scope::Strict, Scope::of(&other)] {
        let error = secure_fs::read(scope, &file, 1 << 20).expect_err("x is not repairable");
        assert!(format!("{error:#}").contains(OUTSIDE_2546), "{error:#}");
        assert_eq!(mode_2546(&x), 0o755, "{scope:?} changed x");
    }
}

/// (ii) `check_dir` repairs only a folder named `autodrive-state`; any other
/// folder is checked strictly and left as it was.
#[cfg(unix)]
#[test]
fn issue_2546_check_dir_repairs_only_runner_state_folder() {
    use crate::commands::orch::autodrive_completion::secure_fs;
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain");
    make_2546(&plain, None, 0o755);
    let error = secure_fs::check_dir(&plain).expect_err("an explicit folder stays strict");
    assert!(format!("{error:#}").contains("chmod 700"), "{error:#}");
    assert_eq!(mode_2546(&plain), 0o755);

    let state = dir.path().join("autodrive-state");
    make_2546(&state, None, 0o755);
    secure_fs::check_dir(&state).unwrap();
    assert_eq!(mode_2546(&state), 0o700);
}

/// (iii) `prepare` validates its arguments before it repairs the state
/// folder: a rejected call leaves a 0775 `autodrive-state` folder at 0775.
#[cfg(unix)]
#[test]
fn issue_2546_rejected_prepare_changes_no_modes() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("autodrive-state");
    make_2546(&state, None, 0o775);
    let s = state.to_str().unwrap();
    let record = state.join("crusty-round-x.json");
    let error = prepare_2546(&[
        "--state-dir",
        s,
        "--round-label",
        "round-x",
        "--record",
        record.to_str().unwrap(),
        "--repo",
        s,
        "--review-dir",
        s,
        "--launch-tools",
        s,
    ])
    .expect_err("invalid round label");
    assert!(
        format!("{error:#}").contains("invalid round label"),
        "{error:#}"
    );
    assert_eq!(mode_2546(&state), 0o775);
}

/// (iv) `prepare` refuses context files outside the state folder before it
/// reads them, and leaves them and their folder as they were.
#[cfg(unix)]
#[test]
fn issue_2546_prepare_refuses_context_outside_state_folder() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("autodrive-state");
    make_2546(&state, None, 0o700);
    let outside = dir.path().join("outside");
    make_2546(&outside, None, 0o755);
    let file = outside.join("f");
    make_2546(&file, Some(b"history"), 0o644);
    let s = state.to_str().unwrap();
    let record = state.join("crusty-round-1.json");
    let record = record.to_str().unwrap();
    let dotted = format!("{s}/../outside/f");
    for extra in [
        vec![
            "--context-from-file".to_owned(),
            format!("loop_history={}", file.display()),
        ],
        vec![
            "--context-from-file".to_owned(),
            format!("loop_history={dotted}"),
        ],
        vec![
            "--evaluator-context-file".to_owned(),
            file.display().to_string(),
        ],
    ] {
        let mut args = vec![
            "--state-dir",
            s,
            "--round-label",
            "round-1",
            "--record",
            record,
            "--repo",
            s,
            "--review-dir",
            s,
            "--launch-tools",
            s,
        ];
        args.extend(extra.iter().map(String::as_str));
        let error = prepare_2546(&args).expect_err("context outside the state folder");
        assert!(
            format!("{error:#}").contains("context file must be inside the state directory"),
            "{extra:?}: {error:#}"
        );
        assert_eq!(mode_2546(&outside), 0o755);
        assert_eq!(mode_2546(&file), 0o644);
    }
}

/// (v) `EvidenceStore::create` repairs an existing root only when it is the
/// runner's `autodrive-state` folder; an explicit folder with another name is
/// refused and left as it was.
#[cfg(unix)]
#[test]
fn issue_2546_evidence_store_repairs_only_runner_state_root() {
    let dir = tempfile::tempdir().unwrap();
    let explicit = dir.path().join("explicit-state");
    make_2546(&explicit, None, 0o775);
    let error = match EvidenceStore::create(&explicit) {
        Ok(_) => panic!("an explicit 0775 state folder was accepted"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains(OUTSIDE_2546) && format!("{error:#}").contains("chmod 700"),
        "{error:#}"
    );
    assert_eq!(mode_2546(&explicit), 0o775);

    let runner = dir.path().join("autodrive-state");
    std::fs::rename(&explicit, &runner).unwrap();
    EvidenceStore::create(&runner).unwrap();
    assert_eq!(mode_2546(&runner), 0o700);
}
