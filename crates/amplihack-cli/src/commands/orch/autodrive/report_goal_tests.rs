//! #3129 (D8, D-9): the run report for the goal loop.
//!
//! Wire with `#[cfg(test)] mod report_goal_tests;` in `autodrive/mod.rs`.
//! These replace the loop-line cases of `report_v2_tests.rs` (rewrite that
//! file when the implementation lands; the tests that only check removed
//! behaviour go with it).
//!
//! Contract of `report(auth, context, expected_pr)`:
//!
//! - It reads two records from the runner-bound state folder, and only
//!   there: `merge-result.json` (the merge action's own line) and
//!   `loop-end.json` (written by the driver, and by its `end` actions).
//!   Each must be a regular file (never a symlink) of at most
//!   `MAX_RECORD_BYTES`, and its `run_id` must be this run's.
//! - `loop-end.json` carries `end` (`merged`, `closed`, `handoff`, `ready`,
//!   `stopped`, `failed`, `refused`), `run_id`, `subject`, `detail`; a
//!   `ready` end also carries `verify_readiness` (true when merging is off);
//!   a `merged` end may carry `after_merge` (strings).
//! - `stop_reason` is one of `completed`, `handoff`, `closed`, `stopped`,
//!   `terminal_refusal`, `failed`, plus `usage_limit` and `orphaned`.
//! - `completed` comes only from: already merged at entry, `end == merged`,
//!   or `end == ready` with `verify_readiness`. `merge-result.json` fills the
//!   `merge` field and never makes a run `completed` by itself.
//! - `loop_result` is always null. `loop_end` echoes the end word.
//! - The report makes no network call, never fails, never panics, and never
//!   reads a turn record (`turns/NNN/record.md`).

use crate::commands::orch::autodrive::{
    authorization::InvocationAuthorization, identity::MAX_RECORD_BYTES, report::report,
    state::EvidenceStore,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::os::unix::fs::{OpenOptionsExt, symlink};
use std::path::PathBuf;

const HEAD: &str = concat!("15a7f6f47c1e2d0b9a8f", "7e6d5c4b3a2918070605");

fn auth(no_merge: &str) -> InvocationAuthorization {
    InvocationAuthorization::capture(
        "auto-drive-to-merge",
        &BTreeMap::from([
            ("no_merge".into(), no_merge.into()),
            ("autodrive_dry_run".into(), "false".into()),
        ]),
    )
    .expect("valid invocation")
}

struct World {
    _temp: tempfile::TempDir,
    auth: InvocationAuthorization,
    dir: PathBuf,
}

fn world(no_merge: &str) -> World {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("state");
    EvidenceStore::create(&dir).unwrap();
    let mut auth = auth(no_merge);
    auth.bind_state_dir(dir.clone());
    World {
        _temp: temp,
        auth,
        dir,
    }
}

impl World {
    fn put(&self, name: &str, body: &Value) {
        self.put_raw(name, format!("{body}\n").as_bytes());
    }
    fn put_raw(&self, name: &str, bytes: &[u8]) {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.dir.join(name))
            .unwrap();
        std::io::Write::write_all(&mut f, bytes).unwrap();
    }
    fn end(&self, word: &str) {
        self.put("loop-end.json", &json!({"end": word, "run_id": self.auth.run_id(), "subject": "PR 7", "detail": "fixture"}));
    }
    fn merge_result(&self, result: &str) {
        self.put("merge-result.json", &json!({"result": result, "pr": 7, "expected_head": HEAD, "handoff": "false", "run_id": self.auth.run_id()}));
    }
    fn report(&self) -> Value {
        report(&self.auth, &Map::new(), Some("7"))
    }
}

fn reason(v: &Value) -> &str {
    v["stop_reason"].as_str().unwrap_or("<missing>")
}

#[test]
fn merged_end_is_completed() {
    let w = world("false");
    w.merge_result("merged");
    w.end("merged");
    let r = w.report();
    assert_eq!(reason(&r), "completed", "{r}");
    assert_eq!(r["merge"]["result"], "merged", "{r}");
    assert_eq!(r["loop_end"], "merged", "{r}");
    assert!(r["loop_result"].is_null(), "{r}");
}

#[test]
fn merged_by_someone_else_is_completed_with_a_note_and_no_merge_field() {
    let w = world("false");
    w.end("merged"); // no merge-result.json: not this run's merge action
    let r = w.report();
    assert_eq!(reason(&r), "completed", "{r}");
    assert!(r["merge"].is_null(), "{r}");
    assert!(
        r["note"]
            .as_str()
            .is_some_and(|n| n.contains("merge action")),
        "{r}"
    );
}

#[test]
fn merge_record_alone_never_completes_a_run() {
    let w = world("false");
    w.merge_result("merged"); // no loop-end.json
    let r = w.report();
    assert_eq!(reason(&r), "failed", "{r}");
}

#[test]
fn merge_record_with_a_handoff_end_is_handoff_not_completed() {
    let w = world("false");
    w.merge_result("merged");
    w.end("handoff");
    let r = w.report();
    assert_eq!(reason(&r), "handoff", "{r}");
    assert_eq!(r["merge"]["result"], "merged", "{r}");
}

#[test]
fn ready_with_merging_off_is_completed() {
    let w = world("true");
    w.put("loop-end.json", &json!({"end": "ready", "run_id": w.auth.run_id(), "subject": "PR 7", "detail": "d", "verify_readiness": true}));
    let r = w.report();
    assert_eq!(reason(&r), "completed", "{r}");
    assert_eq!(r["requested_action"], "verify_readiness", "{r}");
}

#[test]
fn ready_with_merging_on_is_handoff() {
    let w = world("false");
    w.end("ready");
    let r = w.report();
    assert_eq!(reason(&r), "handoff", "{r}");
}

#[test]
fn a_ready_record_that_claims_verify_readiness_does_not_complete_a_merging_run() {
    // Authority comes from the captured invocation, not from the record.
    let w = world("false");
    w.put("loop-end.json", &json!({"end": "ready", "run_id": w.auth.run_id(), "subject": "x", "detail": "y", "verify_readiness": true}));
    let r = w.report();
    assert_ne!(reason(&r), "completed", "{r}");
}

#[test]
fn closed_stopped_failed_and_refused_ends_map_to_their_reasons() {
    for (end, want) in [
        ("closed", "closed"),
        ("stopped", "stopped"),
        ("failed", "failed"),
        ("handoff", "handoff"),
        ("refused", "terminal_refusal"),
    ] {
        let w = world("false");
        w.end(end);
        let r = w.report();
        assert_eq!(reason(&r), want, "end {end}: {r}");
        assert_ne!(reason(&r), "completed");
    }
}

#[test]
fn only_completed_counts_as_success_words() {
    for removed in [
        "waiting_for_checks",
        "no_progress",
        "budget_exhausted",
        "unreadable",
        "not_merged",
    ] {
        for end in [
            "merged", "closed", "handoff", "ready", "stopped", "failed", "refused", "mystery",
        ] {
            let w = world("false");
            w.end(end);
            let r = w.report();
            assert_ne!(reason(&r), removed, "{removed} is retired (end {end}): {r}");
        }
    }
}

#[test]
fn an_unknown_end_word_is_failed_with_a_note() {
    let w = world("false");
    w.end("mystery");
    let r = w.report();
    assert_eq!(reason(&r), "failed", "{r}");
    assert!(r.get("note").is_some(), "{r}");
}

#[test]
fn a_missing_loop_end_is_failed_not_unreadable() {
    let w = world("false");
    let r = w.report();
    assert_eq!(reason(&r), "failed", "{r}");
}

#[test]
fn pending_and_base_behind_are_not_merge_results_any_more() {
    for old in ["checks-pending", "base-behind"] {
        let w = world("false");
        w.merge_result(old);
        w.end("handoff");
        let r = w.report();
        assert!(r["merge"].is_null(), "{old} is retired and not echoed: {r}");
    }
}

#[test]
fn checks_not_green_is_still_a_known_merge_result() {
    let w = world("false");
    w.merge_result("checks-not-green");
    w.end("handoff");
    let r = w.report();
    assert_eq!(r["merge"]["result"], "checks-not-green", "{r}");
}

#[test]
fn a_record_for_another_run_is_ignored() {
    let w = world("false");
    w.put(
        "loop-end.json",
        &json!({"end": "merged", "run_id": "someone-else", "subject": "x", "detail": "y"}),
    );
    let r = w.report();
    assert_ne!(reason(&r), "completed", "{r}");
}

#[test]
fn a_symlinked_loop_end_is_never_read() {
    let w = world("false");
    let target = w.dir.join("elsewhere.json");
    w.put_raw(
        "elsewhere.json",
        format!(
            "{}\n",
            json!({"end": "merged", "run_id": w.auth.run_id(), "subject": "x", "detail": "y"})
        )
        .as_bytes(),
    );
    symlink(&target, w.dir.join("loop-end.json")).unwrap();
    let r = w.report();
    assert_ne!(reason(&r), "completed", "{r}");
}

#[test]
fn an_oversized_loop_end_is_never_read() {
    let w = world("false");
    let pad = "x".repeat(MAX_RECORD_BYTES + 16);
    w.put(
        "loop-end.json",
        &json!({"end": "merged", "run_id": w.auth.run_id(), "subject": "x", "detail": pad}),
    );
    let r = w.report();
    assert_ne!(reason(&r), "completed", "{r}");
}

#[test]
fn a_state_folder_that_differs_from_the_assigned_one_is_not_used() {
    let w = world("false");
    w.end("merged");
    let ctx: Map<String, Value> = [("autodrive_state_dir".to_string(), json!("/somewhere/else"))]
        .into_iter()
        .collect();
    let r = report(&w.auth, &ctx, Some("7"));
    assert_ne!(reason(&r), "completed", "{r}");
}

// "Already merged at entry" needs a bound PR identity and is covered by the
// existing cases in `report_v2_tests.rs`, which keep passing.

#[test]
fn merged_end_lists_the_steps_written_for_after_the_merge() {
    let w = world("false");
    w.put("loop-end.json", &json!({"end": "merged", "run_id": w.auth.run_id(), "subject": "PR 7", "detail": "d", "after_merge": ["tag the release", "close the issue"]}));
    let r = w.report();
    assert_eq!(
        r["after_merge_steps"],
        json!(["tag the release", "close the issue"]),
        "{r}"
    );
}

#[test]
fn the_report_never_reads_a_turn_record() {
    let w = world("false");
    let turn = w.dir.join("turns").join("001");
    std::fs::create_dir_all(&turn).unwrap();
    std::fs::write(
        turn.join("record.md"),
        "Result: merged. The PR is merged.\n",
    )
    .unwrap();
    std::fs::write(turn.join("reading.json"), r#"{"pr":{"state":"MERGED"}}"#).unwrap();
    let r = w.report();
    assert_ne!(reason(&r), "completed", "turn records are for people: {r}");
}

#[test]
fn the_report_shape_is_stable_and_never_fails_on_hostile_context() {
    let w = world("false");
    w.end("merged");
    let hostile: Map<String, Value> = [
        (
            "autodrive_finalization".to_string(),
            json!({"nested": [1, 2, 3]}),
        ),
        ("merge_loop_result".to_string(), json!("not json")),
        ("finalize_merge".to_string(), json!(42)),
    ]
    .into_iter()
    .collect();
    let r = report(&w.auth, &hostile, Some("7"));
    assert_eq!(r["schema_version"], 2, "{r}");
    assert!(r["summary"].is_string(), "{r}");
    assert!(r["run_id"].is_string(), "{r}");
}
