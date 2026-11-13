//! D7/D8: `amplihack orch helper autodrive-round-waits`, the pure classifier
//! that splits a round's blockers into waits and fixable findings.
//! Contract: docs/reference/autodrive-github-reads.md#waits-and-fixable-findings.

use std::io::Write;
use std::process::{Output, Stdio};

use serde_json::{Value, json};

use crate::support::auto_drive_light::show;

const SHA: &str = "3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d";
const EPOCH: i64 = 1_794_528_000;

fn classify_raw(input: &str) -> Output {
    let mut child =
        crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin())
            .args(["orch", "helper", "autodrive-round-waits"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// A round with nothing wrong, overridden by `patch`.
fn input(patch: Value) -> Value {
    let mut v = json!({
        "head_sha": SHA, "ci_signal": "PASS", "merge_state": "CLEAN", "mergeable": "MERGEABLE",
        "unavailable": [], "findings": [], "measured_downgrade": []
    });
    for (k, val) in patch.as_object().unwrap() {
        v[k] = val.clone();
    }
    v
}

fn classify(patch: Value) -> Value {
    let o = classify_raw(&input(patch).to_string());
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    crate::support::autodrive_gh_fake::one_json(&o)
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("expected a list: {v}"))
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

fn assert_wait_only(r: &Value, target: &str) {
    assert_eq!(r["round_waiting"], true, "{r}");
    assert!(strs(&r["fixable"]).is_empty(), "{r}");
    assert!(!strs(&r["waits"]).is_empty(), "{r}");
    assert_eq!(r["wait_target"], target, "{r}");
}

// ── output shape ─────────────────────────────────────────────────────────────

#[test]
fn output_has_exactly_the_documented_fields() {
    let r = classify(json!({}));
    let mut keys: Vec<_> = r.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["fixable", "round_waiting", "wait_target", "waits"]);
    assert_eq!(r["round_waiting"], false, "no blockers is not waiting: {r}");
    assert_eq!(r["wait_target"], "", "{r}");
}

#[test]
fn bad_json_and_unknown_fields_exit_2() {
    for bad in [
        "not json".to_string(),
        "[]".to_string(),
        {
            let mut v = input(json!({}));
            v["ci_sigal"] = json!("PASS"); // a renamed field is refused, not ignored
            v.to_string()
        },
        input(json!({"findings": "ci-pending"})).to_string(),
    ] {
        let o = classify_raw(&bad);
        assert_eq!(o.status.code(), Some(2), "{bad}: {}", show(&o));
    }
}

// ── each wait source ─────────────────────────────────────────────────────────

#[test]
fn ci_pending_or_queued_at_the_head_is_a_ci_wait() {
    for sig in ["PENDING", "QUEUED"] {
        let r = classify(json!({"ci_signal": sig, "findings": ["ci-pending"]}));
        assert_wait_only(&r, &format!("ci@{SHA}"));
    }
}

#[test]
fn ci_pending_with_no_agent_finding_is_still_a_measured_wait() {
    let r = classify(json!({"ci_signal": "PENDING"}));
    assert_wait_only(&r, &format!("ci@{SHA}"));
}

#[test]
fn unreadable_ci_is_a_read_wait() {
    let r =
        classify(json!({"ci_signal": "UNREADABLE", "findings": ["expected-checks-unreadable"]}));
    assert_wait_only(&r, "read@checks");
}

#[test]
fn unknown_merge_state_is_a_mergeability_wait() {
    for (ms, m) in [("UNKNOWN", "MERGEABLE"), ("BLOCKED", "UNKNOWN")] {
        let r = classify(json!({"merge_state": ms, "mergeable": m}));
        assert_wait_only(&r, &format!("mergeability@{SHA}"));
    }
}

#[test]
fn a_rate_limited_read_waits_for_the_stated_reset() {
    let r = classify(json!({
        "unavailable": [{"read": "reviews", "rate_limited": true, "reset_epoch": EPOCH}],
        "findings": ["approval-facts-unreadable"]
    }));
    assert_wait_only(&r, &format!("ratelimit-reset@{EPOCH}"));
}

#[test]
fn a_rate_limit_with_no_reset_waits_on_unknown() {
    let r = classify(json!({
        "unavailable": [{"read": "pr", "rate_limited": true, "reset_epoch": null}]
    }));
    assert_wait_only(&r, "ratelimit-reset@unknown");
}

#[test]
fn an_unavailable_read_without_a_rate_limit_is_a_read_wait() {
    let r = classify(json!({
        "unavailable": [{"read": "head", "rate_limited": false, "reset_epoch": null}],
        "findings": ["crusty-pr-head-unreadable"]
    }));
    assert_wait_only(&r, "read@pr-head");
}

#[test]
fn each_unreadable_wait_id_is_a_wait() {
    for id in [
        "approval-facts-unreadable",
        "expected-checks-unreadable",
        "crusty-pr-head-unreadable",
    ] {
        let r = classify(json!({"findings": [id]}));
        assert!(strs(&r["waits"]).contains(&id.to_string()), "{id}: {r}");
        assert!(strs(&r["fixable"]).is_empty(), "{id}: {r}");
        assert_eq!(r["round_waiting"], true, "{id}: {r}");
    }
}

// ── fixable findings ─────────────────────────────────────────────────────────

#[test]
fn real_findings_are_fixable() {
    for id in [
        "ci-failing",
        "merge-conflict",
        "crusty-concern-docs-typo",
        "changes-requested",
        "unresolved-review-threads",
        "required-check-missing",
        "qa-failed",
        "something-no-one-has-seen-before",
    ] {
        let r = classify(json!({"findings": [id]}));
        assert_eq!(strs(&r["fixable"]), [id], "{id}: {r}");
        assert_eq!(r["round_waiting"], false, "{id}: {r}");
    }
}

#[test]
fn conflicts_and_failing_ci_are_never_waits() {
    let r = classify(
        json!({"ci_signal": "FAIL", "merge_state": "DIRTY", "mergeable": "CONFLICTING",
                            "findings": ["ci-failing", "merge-conflict"]}),
    );
    assert_eq!(r["round_waiting"], false, "{r}");
    assert_eq!(strs(&r["fixable"]).len(), 2, "{r}");
}

#[test]
fn an_agent_id_that_only_contains_a_wait_id_stays_fixable() {
    // Waits come from the fixed list matched on the whole id, so an agent
    // cannot turn a real failure into a wait by naming it creatively.
    for id in [
        "ci-pending-but-actually-red",
        "not-approval-facts-unreadable-really-x",
    ] {
        let r = classify(json!({"findings": [id]}));
        assert_eq!(strs(&r["fixable"]), [id], "{id}: {r}");
    }
}

#[test]
fn a_mixed_round_is_not_waiting_and_keeps_both_lists() {
    let r = classify(json!({"ci_signal": "PENDING", "findings": ["ci-pending", "qa-failed"]}));
    assert_eq!(r["round_waiting"], false, "{r}");
    assert_eq!(strs(&r["fixable"]), ["qa-failed"], "{r}");
    assert!(strs(&r["waits"]).contains(&"ci-pending".to_string()), "{r}");
    assert_eq!(
        r["wait_target"],
        format!("ci@{SHA}"),
        "the target is still named: {r}"
    );
}

// ── measured-evidence-disagrees and the downgrade token table ───────────────

fn downgrade(tokens: &[&str], extra_findings: &[&str]) -> Value {
    let mut findings: Vec<&str> = extra_findings.to_vec();
    findings.push("measured-evidence-disagrees");
    classify(json!({"findings": findings, "measured_downgrade": tokens, "ci_signal": "PASS"}))
}

fn mead_is_fixable(r: &Value) -> bool {
    strs(&r["fixable"]).contains(&"measured-evidence-disagrees".to_string())
}

#[test]
fn every_downgrade_token_is_classed_one_at_a_time() {
    let wait_tokens = [
        "ci_status=PENDING",
        "ci_status=UNREADABLE",
        "conflict=unknown",
        "unresolved_threads=unreadable",
    ];
    for t in wait_tokens {
        let r = downgrade(&[t], &[]);
        assert!(!mead_is_fixable(&r), "{t} is a wait token: {r}");
    }
    let fixable_tokens = [
        "ci_status=RED",
        "ci_status=MISSING",
        "conflict=true",
        "unresolved_threads=2",
        "approval_status=NOT_MET",
        "approval_status=MISSING",
        "approval_status=OTHER",
        "qa_status=FAIL",
        "qa_status=MISSING",
        "crusty_status=NOT_CLEAN",
        "qa_evidence=modified",
        "measured_evidence=unreadable",
        "brand_new=token",
    ];
    for t in fixable_tokens {
        let r = downgrade(&[t], &[]);
        assert!(mead_is_fixable(&r), "{t} is fixable: {r}");
        assert_eq!(r["round_waiting"], false, "{t}: {r}");
    }
}

#[test]
fn ci_running_with_approval_hidden_is_wait_only() {
    // The doc's worked example: approval_status=PENDING is neutral.
    let r = downgrade(
        &["ci_status=PENDING", "approval_status=PENDING"],
        &["ci-pending"],
    );
    assert!(!mead_is_fixable(&r), "{r}");
    assert_eq!(r["round_waiting"], true, "{r}");
}

#[test]
fn approval_pending_alone_stays_fixable() {
    let r = downgrade(&["approval_status=PENDING"], &[]);
    assert!(
        mead_is_fixable(&r),
        "only neutral tokens leave nothing to wait on: {r}"
    );
}

#[test]
fn an_empty_downgrade_keeps_the_id_fixable() {
    let r = downgrade(&[], &[]);
    assert!(mead_is_fixable(&r), "{r}");
}

#[test]
fn approval_unreadable_is_a_wait_only_with_the_failed_read() {
    let r = downgrade(
        &["approval_status=UNREADABLE"],
        &["approval-facts-unreadable"],
    );
    assert!(!mead_is_fixable(&r), "{r}");
    assert_eq!(r["round_waiting"], true, "{r}");
    let r = downgrade(&["approval_status=UNREADABLE"], &[]);
    assert!(
        mead_is_fixable(&r),
        "approval-state-unreadable is not a failed read: {r}"
    );
}

#[test]
fn one_fixable_token_keeps_the_id_fixable() {
    let r = downgrade(&["ci_status=PENDING", "qa_status=FAIL"], &["ci-pending"]);
    assert!(mead_is_fixable(&r), "{r}");
    assert_eq!(r["round_waiting"], false, "{r}");
}

// ── wait_target order ────────────────────────────────────────────────────────

#[test]
fn wait_target_prefers_ratelimit_then_ci_then_mergeability_then_read() {
    let rl = json!([{"read": "checks", "rate_limited": true, "reset_epoch": EPOCH}]);
    let plain = json!([{"read": "threads", "rate_limited": false, "reset_epoch": null}]);
    let all = classify(json!({"ci_signal": "PENDING", "merge_state": "UNKNOWN",
                               "unavailable": [plain[0].clone(), rl[0].clone()]}));
    assert_eq!(
        all["wait_target"],
        format!("ratelimit-reset@{EPOCH}"),
        "{all}"
    );
    let r =
        classify(json!({"ci_signal": "PENDING", "merge_state": "UNKNOWN", "unavailable": plain}));
    assert_eq!(r["wait_target"], format!("ci@{SHA}"), "{r}");
    let r = classify(json!({"merge_state": "UNKNOWN", "unavailable": plain}));
    assert_eq!(r["wait_target"], format!("mergeability@{SHA}"), "{r}");
    let r = classify(json!({"unavailable": plain}));
    assert_eq!(r["wait_target"], "read@threads", "{r}");
}

#[test]
fn wait_target_never_carries_input_text() {
    // An unknown read name or a forged SHA must not reach the closed patterns.
    let r = classify(json!({
        "head_sha": "$(touch pwned)",
        "ci_signal": "PENDING",
        "unavailable": [{"read": "x\"; rm -rf /", "rate_limited": false, "reset_epoch": null}]
    }));
    let t = r["wait_target"].as_str().unwrap();
    let ok = t.is_empty()
        || t == "ratelimit-reset@unknown"
        || t.strip_prefix("ratelimit-reset@")
            .is_some_and(|e| e.bytes().all(|b| b.is_ascii_digit()))
        || ["ci@", "mergeability@"].iter().any(|p| {
            t.strip_prefix(p)
                .is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        })
        || [
            "read@pr",
            "read@pr-head",
            "read@pr-number",
            "read@checks",
            "read@reviews",
            "read@threads",
        ]
        .contains(&t);
    assert!(ok, "wait_target must match a closed pattern: {t}");
    assert!(
        !r.to_string().contains("pwned") && !r.to_string().contains("rm -rf"),
        "{r}"
    );
}
