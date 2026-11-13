//! Unit tests for `autodrive_waits` (issue #2472, waits slice). Included from
//! `autodrive_waits.rs` with:
//!
//! ```ignore
//! #[cfg(test)]
//! #[path = "autodrive_waits_tests.rs"]
//! mod tests;
//! ```
//!
//! `Input` deserialises the `autodrive-round-waits` stdin with
//! `deny_unknown_fields`; `classify` is pure; `Output` serialises to the
//! documented fields. `recompute_target` gives the wait target after an R7
//! merge-state recompute.

use super::{Input, Output, classify, recompute_target};
use serde_json::{Value, json};

const SHA: &str = "3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d";
const PUSHED: &str = "0123456789abcdef0123456789abcdef01234567";

fn input(patch: Value) -> Input {
    let mut v = json!({
        "head_sha": SHA, "ci_signal": "PASS", "merge_state": "CLEAN", "mergeable": "MERGEABLE",
        "unavailable": [], "findings": [], "measured_downgrade": []
    });
    for (k, val) in patch.as_object().unwrap() {
        v[k] = val.clone();
    }
    serde_json::from_value(v).expect("valid input")
}

fn run(patch: Value) -> Output {
    classify(&input(patch))
}

// ── Field names are pinned ───────────────────────────────────────────────────

#[test]
fn input_fields_are_pinned_and_unknown_fields_refused() {
    let ok = json!({
        "head_sha": SHA, "ci_signal": "", "merge_state": "", "mergeable": "",
        "unavailable": [{"read": "pr", "rate_limited": false, "reset_epoch": null}],
        "findings": [], "measured_downgrade": []
    });
    assert!(serde_json::from_value::<Input>(ok.clone()).is_ok());
    let mut extra = ok.clone();
    extra["surprise"] = json!(1);
    assert!(
        serde_json::from_value::<Input>(extra).is_err(),
        "unknown field refused"
    );
    let mut extra = ok;
    extra["unavailable"][0]["why"] = json!("x");
    assert!(
        serde_json::from_value::<Input>(extra).is_err(),
        "unknown nested field refused"
    );
}

#[test]
fn output_fields_are_pinned() {
    let v = serde_json::to_value(run(json!({}))).unwrap();
    let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["fixable", "round_waiting", "wait_target", "waits"]);
}

// ── Wait sources ─────────────────────────────────────────────────────────────

#[test]
fn ci_signal_mapping() {
    for (sig, target) in [
        ("PENDING", format!("ci@{SHA}")),
        ("QUEUED", format!("ci@{SHA}")),
        ("UNREADABLE", "read@checks".to_string()),
    ] {
        let o = run(json!({"ci_signal": sig}));
        assert!(o.round_waiting, "{sig}");
        assert_eq!(o.wait_target, target, "{sig}");
    }
    for sig in ["PASS", "FAIL", "GREEN", "RED", "", "NO_CHECKS"] {
        let o = run(json!({"ci_signal": sig}));
        assert!(o.waits.is_empty(), "{sig} adds no wait: {:?}", o.waits);
    }
}

#[test]
fn mergeability_unknown_is_a_wait_at_the_head() {
    let o = run(json!({"merge_state": "UNKNOWN"}));
    assert!(o.round_waiting);
    assert_eq!(o.wait_target, format!("mergeability@{SHA}"));
    let o = run(json!({"mergeable": "UNKNOWN"}));
    assert_eq!(o.wait_target, format!("mergeability@{SHA}"));
}

#[test]
fn unavailable_reads_map_to_read_targets() {
    for (read, target) in [
        ("pr", "read@pr"),
        ("head", "read@pr-head"),
        ("pr-for-branch", "read@pr-number"),
        ("checks", "read@checks"),
        ("reviews", "read@reviews"),
        ("threads", "read@threads"),
    ] {
        let o = run(
            json!({"unavailable": [{"read": read, "rate_limited": false, "reset_epoch": null}]}),
        );
        assert!(o.round_waiting, "{read}");
        assert_eq!(o.wait_target, target, "{read}");
    }
}

#[test]
fn rate_limited_reads_wait_for_the_reset() {
    let o = run(
        json!({"unavailable": [{"read": "pr", "rate_limited": true, "reset_epoch": 1_794_528_000}]}),
    );
    assert_eq!(o.wait_target, "ratelimit-reset@1794528000");
    let o =
        run(json!({"unavailable": [{"read": "pr", "rate_limited": true, "reset_epoch": null}]}));
    assert_eq!(o.wait_target, "ratelimit-reset@unknown");
}

#[test]
fn the_earliest_reset_wins_among_several_rate_limited_reads() {
    let o = run(json!({"unavailable": [
        {"read": "pr", "rate_limited": true, "reset_epoch": 1_794_528_900},
        {"read": "checks", "rate_limited": true, "reset_epoch": 1_794_528_000}
    ]}));
    // Resuming at the later reset would also be correct; the earliest is the
    // first moment any read can succeed, and the next round re-reads.
    assert_eq!(o.wait_target, "ratelimit-reset@1794528000");
}

#[test]
fn an_invalid_head_sha_never_reaches_a_target() {
    let o = run(json!({"head_sha": "not-a-sha", "ci_signal": "PENDING"}));
    assert!(!o.wait_target.contains("not-a-sha"), "{}", o.wait_target);
}

// ── Fixed wait ids vs fixable ────────────────────────────────────────────────

#[test]
fn fixed_wait_ids_are_waits_on_the_whole_id_only() {
    for id in [
        "ci-pending",
        "approval-facts-unreadable",
        "expected-checks-unreadable",
        "crusty-pr-head-unreadable",
    ] {
        let o = run(json!({"findings": [id]}));
        assert_eq!(o.waits, [id], "{id}");
        assert!(o.fixable.is_empty(), "{id}");
    }
    for id in ["ci-pending-x", "x-ci-pending-y", "CI-PENDING", "ci_pending"] {
        let o = run(json!({"findings": [id]}));
        assert_eq!(o.fixable, [id], "{id}");
    }
}

#[test]
fn no_findings_and_no_waits_is_not_waiting() {
    let o = run(json!({}));
    assert!(!o.round_waiting);
    assert!(o.waits.is_empty() && o.fixable.is_empty());
    assert_eq!(o.wait_target, "");
}

#[test]
fn fixable_findings_keep_their_order_and_are_deduplicated() {
    let o = run(json!({"findings": ["b", "a", "b", "ci-pending"]}));
    assert_eq!(o.fixable, ["b", "a"]);
    assert!(!o.round_waiting);
}

// ── measured-evidence-disagrees and the token table ─────────────────────────

fn mead(tokens: &[&str], extra: &[&str]) -> Output {
    let mut f: Vec<&str> = extra.to_vec();
    f.push("measured-evidence-disagrees");
    run(json!({"findings": f, "measured_downgrade": tokens}))
}

fn mead_fixable(o: &Output) -> bool {
    o.fixable.iter().any(|f| f == "measured-evidence-disagrees")
}

#[test]
fn wait_tokens_drop_the_id_with_their_targets() {
    for (t, target) in [
        ("ci_status=PENDING", format!("ci@{SHA}")),
        ("ci_status=UNREADABLE", "read@checks".into()),
        ("conflict=unknown", format!("mergeability@{SHA}")),
        ("unresolved_threads=unreadable", "read@threads".into()),
    ] {
        let o = mead(&[t], &[]);
        assert!(!mead_fixable(&o), "{t}");
        assert!(o.round_waiting, "{t}");
        assert_eq!(o.wait_target, target, "{t}");
    }
}

#[test]
fn fixable_tokens_keep_the_id() {
    for t in [
        "ci_status=RED",
        "ci_status=MISSING",
        "conflict=true",
        "unresolved_threads=3",
        "approval_status=NOT_MET",
        "approval_status=MISSING",
        "approval_status=OTHER",
        "qa_status=FAIL",
        "crusty_status=UNTRUSTED",
        "qa_evidence=modified",
        "measured_evidence=unreadable",
        "",
        "=",
        "ci_status=",
        "nonsense",
    ] {
        assert!(mead_fixable(&mead(&[t], &[])), "{t:?}");
    }
}

#[test]
fn approval_pending_is_neutral() {
    assert!(!mead_fixable(&mead(
        &["ci_status=PENDING", "approval_status=PENDING"],
        &[]
    )));
    assert!(
        mead_fixable(&mead(&["approval_status=PENDING"], &[])),
        "only neutral tokens"
    );
    assert!(mead_fixable(&mead(
        &["approval_status=PENDING", "qa_status=FAIL"],
        &[]
    )));
}

#[test]
fn approval_unreadable_needs_the_failed_read() {
    assert!(!mead_fixable(&mead(
        &["approval_status=UNREADABLE"],
        &["approval-facts-unreadable"]
    )));
    assert!(mead_fixable(&mead(&["approval_status=UNREADABLE"], &[])));
}

#[test]
fn an_empty_downgrade_keeps_the_id() {
    assert!(mead_fixable(&mead(&[], &[])));
}

// ── wait_target precedence ───────────────────────────────────────────────────

#[test]
fn precedence_is_ratelimit_ci_mergeability_read() {
    let rl = json!({"read": "pr", "rate_limited": true, "reset_epoch": null});
    let rd = json!({"read": "reviews", "rate_limited": false, "reset_epoch": null});
    let all =
        json!({"ci_signal": "PENDING", "merge_state": "UNKNOWN", "unavailable": [rd.clone(), rl]});
    assert_eq!(run(all).wait_target, "ratelimit-reset@unknown");
    assert_eq!(
        run(json!({"ci_signal": "QUEUED", "merge_state": "UNKNOWN", "unavailable": [rd.clone()]}))
            .wait_target,
        format!("ci@{SHA}")
    );
    assert_eq!(
        run(json!({"merge_state": "UNKNOWN", "unavailable": [rd.clone()]})).wait_target,
        format!("mergeability@{SHA}")
    );
    assert_eq!(
        run(json!({"unavailable": [rd]})).wait_target,
        "read@reviews"
    );
}

// ── R7 recompute wait targets ────────────────────────────────────────────────

#[test]
fn recompute_targets_follow_the_documented_table() {
    assert_eq!(
        recompute_target("merge-push", "ok", SHA, Some(PUSHED), false, None),
        format!("mergeability@{PUSHED}")
    );
    assert_eq!(
        recompute_target("update-branch", "ok", SHA, None, false, None),
        format!("mergeability@{SHA}")
    );
    assert_eq!(
        recompute_target("noop-up-to-date", "ok", SHA, None, false, None),
        format!("mergeability@{SHA}")
    );
    assert_eq!(
        recompute_target(
            "update-branch",
            "unavailable",
            SHA,
            None,
            true,
            Some(1_794_528_000)
        ),
        "ratelimit-reset@1794528000"
    );
    assert_eq!(
        recompute_target("update-branch", "unavailable", SHA, None, true, None),
        "ratelimit-reset@unknown"
    );
    assert_eq!(
        recompute_target("update-branch", "unavailable", SHA, None, false, None),
        "read@pr"
    );
    assert_eq!(
        recompute_target("update-branch", "refused", SHA, None, false, None),
        "read@pr-head"
    );
}
