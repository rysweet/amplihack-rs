//! #2556: `assess` and `round` report writer faults and stale evidence as
//! named blockers with exit 0, and never accept either.
//!
//! Contract used here (the CLI arms call these with stdin bytes):
//! - `helper::assess(auth, directory, repo, input) -> Result<Value>`
//! - `helper::round(auth, directory, repo, record, input) -> Result<Value>`
//! - `helper::binding_drift(repo, binding) -> Result<Vec<String>>`
//!
//! Each test uses a temporary Git repository and private state directory.

use super::autodrive_contract_tests::ReportingFixture;
use crate::commands::orch::autodrive::{
    helper::{assess, binding_drift, round},
    records::Binding,
    trusted,
    validation::MAX_RECORD_BYTES,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

fn git(repo: &Path, args: &[&str]) -> String {
    trusted::git(repo, args).unwrap()
}

fn commit(repo: &Path, file: Option<(&str, &str)>, message: &str) -> String {
    let id = [
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.test",
    ];
    match file {
        Some((name, text)) => {
            std::fs::write(repo.join(name), text).unwrap();
            git(repo, &["add", name]);
            git(repo, &[&id[..], &["commit", "-m", message][..]].concat());
        }
        None => {
            git(
                repo,
                &[&id[..], &["commit", "--allow-empty", "-m", message][..]].concat(),
            );
        }
    }
    git(repo, &["rev-parse", "HEAD"])
}

/// Point `origin/main` at a new commit on top of the evidence base.
fn move_base(repo: &Path) -> String {
    let base = git(repo, &["rev-parse", "refs/remotes/origin/main"]);
    let tree = format!("{base}^{{tree}}");
    let newer = git(
        repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit-tree",
            &tree,
            "-p",
            &base,
            "-m",
            "base moved",
        ],
    );
    git(repo, &["update-ref", "refs/remotes/origin/main", &newer]);
    newer
}

fn listing(directory: &Path) -> BTreeSet<String> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect()
}

fn head_of(fixture: &ReportingFixture) -> String {
    fixture.envelope["candidate"]["binding"]["head_sha"]
        .as_str()
        .unwrap()
        .into()
}

/// The exact #2556 input: the fixture candidate with one failed attempt whose
/// `cause` is `null`, written compactly with `attempts` first.
fn null_cause_input(fixture: &ReportingFixture, drop_key: bool) -> Vec<u8> {
    let mut raw = fixture.envelope["candidate"].clone();
    raw["readiness"] = json!("incomplete");
    raw["blockers"] = json!(["checks pending"]);
    // Written by hand so the key order does not depend on serde_json features.
    let attempts = if drop_key {
        r#"[{"operation":"gh pr view","outcome":"failed","recovery":null}]"#
    } else {
        r#"[{"cause":null,"operation":"gh pr view","outcome":"failed","recovery":null}]"#
    };
    raw.as_object_mut().unwrap().remove("attempts");
    let rest = serde_json::to_string(&raw).unwrap();
    let text = format!("{{\"attempts\":{attempts},{}", &rest[1..]);
    if !drop_key {
        assert!(text.starts_with(r#"{"attempts":[{"cause":null"#), "{text}");
    }
    text.into_bytes()
}

fn is_sha_or_empty(v: &Value) -> bool {
    let s = v.as_str().expect("SHA fields are strings");
    s.is_empty() || ([40, 64].contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn assert_safe_blockers(out: &Value) {
    for b in out["blockers"].as_array().expect("blockers array") {
        let b = b.as_str().unwrap();
        assert!(b.len() <= 256, "blocker over 256 bytes: {b:?}");
        assert!(!b.chars().any(char::is_control), "control in {b:?}");
    }
}

fn assert_refresh(out: &Value, blocker: &str) {
    assert_eq!(out["status"], "refresh_required", "{out}");
    assert_eq!(out["accepted"], Value::Null, "{out}");
    assert_eq!(out["readiness"], "incomplete", "{out}");
    assert_eq!(out["refresh_required"], true, "{out}");
    assert!(
        out["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == blocker),
        "{blocker} missing: {out}"
    );
    for key in ["evidence_head", "current_head", "current_base"] {
        assert!(is_sha_or_empty(&out[key]), "{key}: {out}");
    }
    assert_safe_blockers(out);
}

// ---- assess -----------------------------------------------------------------

#[test]
fn assess_exact_null_cause_input_is_a_named_blocker_with_no_writes() {
    for drop_key in [false, true] {
        let fixture = ReportingFixture::new();
        let before = listing(&fixture.directory);
        let out = assess(
            &fixture.auth,
            &fixture.directory,
            &fixture.repo,
            &null_cause_input(&fixture, drop_key),
        )
        .unwrap_or_else(|e| panic!("drop_key={drop_key}: must not fail: {e:#}"));
        assert_refresh(&out, "unknown-field:attempts[0].cause");
        assert_eq!(
            listing(&fixture.directory),
            before,
            "no snapshot or follow-up read"
        );
    }
}

#[test]
fn assess_accepts_current_evidence_with_status_accepted() {
    let fixture = ReportingFixture::new();
    let input = serde_json::to_vec(&fixture.envelope["candidate"]).unwrap();
    let out = assess(&fixture.auth, &fixture.directory, &fixture.repo, &input).unwrap();
    assert_eq!(out["status"], "accepted", "{out}");
    assert_eq!(out["refresh_required"], false, "{out}");
    assert_eq!(out["readiness"], "merge_ready", "{out}");
    assert!(out["accepted"].is_string(), "{out}");
}

#[test]
fn assess_reports_moved_base_without_merge_base() {
    let fixture = ReportingFixture::new();
    let newer = move_base(&fixture.repo);
    let before = listing(&fixture.directory);
    let input = serde_json::to_vec(&fixture.envelope["candidate"]).unwrap();
    let out = assess(&fixture.auth, &fixture.directory, &fixture.repo, &input).unwrap();
    assert_refresh(&out, "stale-evidence-base");
    assert_eq!(
        out["blockers"],
        json!(["stale-evidence-base"]),
        "merge-base skipped: {out}"
    );
    assert_eq!(out["current_base"], newer, "{out}");
    assert_eq!(listing(&fixture.directory), before);
}

#[test]
fn assess_unknown_head_is_echoed_as_empty() {
    let fixture = ReportingFixture::new();
    let mut raw = fixture.envelope["candidate"].clone();
    raw["binding"]["head_sha"] = json!("unknown");
    let out = assess(
        &fixture.auth,
        &fixture.directory,
        &fixture.repo,
        &serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    assert_refresh(&out, "unknown-field:binding.head_sha");
    assert_eq!(out["evidence_head"], "", "{out}");
}

#[test]
fn assess_parse_fault_never_echoes_record_values() {
    let fixture = ReportingFixture::new();
    let mut raw = fixture.envelope["candidate"].clone();
    raw["readiness"] = json!("merge_ready\u{1b}]0;pwned\u{7}");
    let out = assess(
        &fixture.auth,
        &fixture.directory,
        &fixture.repo,
        &serde_json::to_vec(&raw).unwrap(),
    )
    .expect("a parse fault is a blocker, not an error");
    assert_eq!(out["status"], "refresh_required", "{out}");
    let blockers = out["blockers"].as_array().unwrap();
    assert!(
        blockers.iter().any(|b| {
            let b = b.as_str().unwrap();
            b.starts_with("evidence-parse-error:") && b.contains("line 1 column")
        }),
        "{out}"
    );
    assert!(!out.to_string().contains("pwned"), "{out}");
    assert_safe_blockers(&out);
}

#[test]
fn assess_input_over_the_size_limit_stays_a_hard_error() {
    let fixture = ReportingFixture::new();
    let input = vec![b' '; MAX_RECORD_BYTES + 1];
    assert!(assess(&fixture.auth, &fixture.directory, &fixture.repo, &input).is_err());
}

// ---- stale evidence never reaches MergeReady (D7, security test 7) ------------

/// Rebind every occurrence of `from` to `to` in a candidate.
fn rebind(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(s) if s == from => *s = to.into(),
        Value::Array(a) => a.iter_mut().for_each(|v| rebind(v, from, to)),
        Value::Object(o) => o.values_mut().for_each(|v| rebind(v, from, to)),
        _ => (),
    }
}

#[test]
fn remeasured_candidate_with_old_reviewed_head_is_not_accepted() {
    let fixture = ReportingFixture::new();
    let old = head_of(&fixture);
    let new = commit(
        &fixture.repo,
        Some(("code.rs", "fn changed() {}\n")),
        "substantive",
    );
    let mut raw = fixture.envelope["candidate"].clone();
    rebind(&mut raw, &old, &new);
    raw["reviewed_head_sha"] = json!(old);
    let before = listing(&fixture.directory);
    let error = assess(
        &fixture.auth,
        &fixture.directory,
        &fixture.repo,
        &serde_json::to_vec(&raw).unwrap(),
    )
    .expect_err("old review cannot cover a substantive new head");
    assert!(
        format!("{error:#}").contains("crusty-review-required"),
        "{error:#}"
    );
    assert_eq!(listing(&fixture.directory), before);
}

#[test]
fn merge_ready_at_a_new_head_needs_a_new_receipt() {
    let fixture = ReportingFixture::new();
    let old = head_of(&fixture);
    let new = commit(&fixture.repo, None, "head moved");
    let mut raw = fixture.envelope["candidate"].clone();
    rebind(&mut raw, &old, &new);
    let error = assess(
        &fixture.auth,
        &fixture.directory,
        &fixture.repo,
        &serde_json::to_vec(&raw).unwrap(),
    )
    .expect_err("the receipt is bound to the old head");
    assert!(
        format!("{error:#}").contains("stale or belongs to another"),
        "{error:#}"
    );
}

#[test]
fn receipt_for_a_local_ahead_head_is_not_readiness_at_another_pr_head() {
    // The fixture's receipt was recorded for its head, standing in for a
    // local head ahead of the remote PR. The PR head is a different commit.
    let fixture = ReportingFixture::new();
    let local_ahead = head_of(&fixture);
    let base = git(&fixture.repo, &["rev-parse", "refs/remotes/origin/main"]);
    git(&fixture.repo, &["reset", "--hard", &base]);
    let pr_head = commit(
        &fixture.repo,
        Some(("code.rs", "fn remote() {}\n")),
        "remote head",
    );
    let mut raw = fixture.envelope["candidate"].clone();
    rebind(&mut raw, &local_ahead, &pr_head);
    let error = assess(
        &fixture.auth,
        &fixture.directory,
        &fixture.repo,
        &serde_json::to_vec(&raw).unwrap(),
    )
    .expect_err("a receipt for one head is not readiness at another");
    assert!(
        format!("{error:#}").contains("stale or belongs to another"),
        "{error:#}"
    );
}

#[test]
fn reduction_still_rejects_a_null_identity_field() {
    let mut fixture = ReportingFixture::new();
    fixture.envelope["candidate"]["binding"]["head_sha"] = Value::Null;
    fixture.accept_current();
    assert!(
        fixture.reduce().is_err(),
        "unknown head never passes reduction"
    );
}

// ---- binding_drift ----------------------------------------------------------

fn fixture_binding(fixture: &ReportingFixture) -> Binding {
    serde_json::from_value(fixture.envelope["candidate"]["binding"].clone()).unwrap()
}

#[test]
fn binding_drift_names_each_stale_dimension() {
    let fixture = ReportingFixture::new();
    let b = fixture_binding(&fixture);
    assert!(binding_drift(&fixture.repo, &b).unwrap().is_empty());

    let mut wrong_merge_base = b.clone();
    wrong_merge_base.merge_base_sha = b.head_sha.clone();
    assert_eq!(
        binding_drift(&fixture.repo, &wrong_merge_base).unwrap(),
        vec!["stale-evidence-merge-base".to_string()]
    );

    commit(&fixture.repo, None, "head moved");
    assert_eq!(
        binding_drift(&fixture.repo, &wrong_merge_base).unwrap(),
        vec!["stale-evidence-head".to_string()],
        "merge-base is not compared once the head has drifted"
    );
    move_base(&fixture.repo);
    let mut both = binding_drift(&fixture.repo, &b).unwrap();
    both.sort();
    assert_eq!(
        both,
        vec![
            "stale-evidence-base".to_string(),
            "stale-evidence-head".to_string()
        ]
    );
}

#[test]
fn binding_drift_keeps_branch_and_format_faults_hard() {
    let fixture = ReportingFixture::new();
    let mut other_branch = fixture_binding(&fixture);
    other_branch.head_branch = "other".into();
    assert!(binding_drift(&fixture.repo, &other_branch).is_err());
    let mut bad_ref = fixture_binding(&fixture);
    bad_ref.base_branch = "bad..ref".into();
    assert!(binding_drift(&fixture.repo, &bad_ref).is_err());
}

// ---- round ------------------------------------------------------------------

const ROUND_KEYS: [&str; 8] = [
    "accepted_evidence",
    "blocker_count",
    "blockers",
    "ci_signal",
    "head_sha",
    "merge_ready_verdict",
    "round_label",
    "test_signal",
];

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

fn assert_round_label_safe(v: &Value) {
    let label = v["round_label"].as_str().unwrap();
    assert!(
        label.len() <= 128
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "{label:?}"
    );
}

fn assert_not_ready_round(record: &Path, out: &Value, blocker: &str) {
    assert_eq!(keys(out), ROUND_KEYS, "{out}");
    assert_eq!(out["merge_ready_verdict"], "NOT_MERGE_READY", "{out}");
    assert_eq!(out["test_signal"], "BLOCKED", "{out}");
    assert_eq!(out["ci_signal"], "UNVERIFIED", "{out}");
    assert_eq!(out["accepted_evidence"], Value::Null, "{out}");
    let blockers = out["blockers"].as_array().unwrap();
    assert_eq!(out["blocker_count"], blockers.len(), "{out}");
    assert!(blockers.iter().any(|b| b == blocker), "{out}");
    assert!(is_sha_or_empty(&out["head_sha"]), "{out}");
    assert_round_label_safe(out);
    assert_safe_blockers(out);

    let written: Value = serde_json::from_slice(&std::fs::read(record).unwrap()).unwrap();
    assert_eq!(&written, out);
    let findings = std::fs::read_to_string(format!("{}.findings", record.display())).unwrap();
    let lines: Vec<_> = findings.lines().collect();
    let expected: Vec<_> = blockers.iter().map(|b| b.as_str().unwrap()).collect();
    assert_eq!(lines, expected, "findings name the blockers one per line");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [
            record.to_path_buf(),
            format!("{}.findings", record.display()).into(),
        ] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{}", path.display());
        }
    }
}

#[test]
fn round_exact_null_cause_input_writes_not_merge_ready() {
    for drop_key in [false, true] {
        let fixture = ReportingFixture::new();
        let record = fixture.directory.join("merge-ready-round-2.json");
        let out = round(
            &fixture.auth,
            &fixture.directory,
            &fixture.repo,
            &record,
            &null_cause_input(&fixture, drop_key),
        )
        .unwrap_or_else(|e| panic!("drop_key={drop_key}: must not fail: {e:#}"));
        assert_not_ready_round(&record, &out, "unknown-field:attempts[0].cause");
    }
}

#[test]
fn round_stale_head_and_parse_fault_write_not_merge_ready() {
    let fixture = ReportingFixture::new();
    commit(&fixture.repo, None, "head moved");
    let record = fixture.directory.join("merge-ready-round-2.json");
    let input = serde_json::to_vec(&fixture.envelope["candidate"]).unwrap();
    let out = round(
        &fixture.auth,
        &fixture.directory,
        &fixture.repo,
        &record,
        &input,
    )
    .unwrap();
    assert_not_ready_round(&record, &out, "stale-evidence-head");

    let fixture = ReportingFixture::new();
    let record = fixture.directory.join("merge-ready-round-2.json");
    let mut raw = fixture.envelope["candidate"].clone();
    raw["readiness"] = json!("ready");
    let out = round(
        &fixture.auth,
        &fixture.directory,
        &fixture.repo,
        &record,
        &serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    assert_eq!(out["merge_ready_verdict"], "NOT_MERGE_READY", "{out}");
    assert!(
        out["blockers"][0]
            .as_str()
            .unwrap()
            .starts_with("evidence-parse-error:"),
        "{out}"
    );
}

#[test]
fn round_success_has_the_same_keys_as_a_fault() {
    let fixture = ReportingFixture::new();
    let record = fixture.directory.join("merge-ready-round-2.json");
    let input = serde_json::to_vec(&fixture.envelope["candidate"]).unwrap();
    let out = round(
        &fixture.auth,
        &fixture.directory,
        &fixture.repo,
        &record,
        &input,
    )
    .unwrap();
    assert_eq!(keys(&out), ROUND_KEYS, "{out}");
    assert_eq!(out["merge_ready_verdict"], "MERGE_READY", "{out}");
    assert_eq!(out["blocker_count"], 0, "{out}");
    assert_eq!(out["blockers"], json!([]), "{out}");
    assert_round_label_safe(&out);
}

#[test]
fn round_record_outside_state_or_existing_stays_a_hard_error() {
    let fixture = ReportingFixture::new();
    let input = null_cause_input(&fixture, false);
    let outside_dir = fixture.repo.parent().unwrap().join("elsewhere");
    std::fs::create_dir(&outside_dir).unwrap();
    let outside = outside_dir.join("merge-ready-round-2.json");
    assert!(
        round(
            &fixture.auth,
            &fixture.directory,
            &fixture.repo,
            &outside,
            &input
        )
        .is_err()
    );
    assert!(!outside.exists(), "nothing written outside state");
    assert!(!Path::new(&format!("{}.findings", outside.display())).exists());

    let existing = fixture.directory.join("merge-ready-round-3.json");
    std::fs::write(&existing, b"previous").unwrap();
    assert!(
        round(
            &fixture.auth,
            &fixture.directory,
            &fixture.repo,
            &existing,
            &input
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&existing).unwrap(), b"previous");
    assert!(
        !Path::new(&format!("{}.findings", existing.display())).exists(),
        "the record path is checked before anything is written"
    );
}

// ---- an unreadable worktree HEAD is a refresh, not an error (D3) -------------

const HEAD_UNREADABLE: &str = "current-head-unreadable";

/// A repository with no commits on branch `branch`: `symbolic-ref HEAD`
/// names the branch, but `rev-parse --verify HEAD` fails.
fn unborn_repo(fixture: &ReportingFixture, branch: &str) -> std::path::PathBuf {
    let repo = fixture
        .repo
        .parent()
        .unwrap()
        .join(format!("unborn-{branch}"));
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", branch]);
    assert!(
        trusted::git(&repo, &["rev-parse", "--verify", "HEAD"]).is_err(),
        "the fixture HEAD must be unreadable"
    );
    repo
}

fn assert_head_unreadable(out: &Value) {
    assert_refresh(out, HEAD_UNREADABLE);
    assert_eq!(out["status"], "refresh_required", "never accepted: {out}");
    assert_eq!(out["current_head"], "", "{out}");
    assert_eq!(out["current_base"], "", "{out}");
}

#[test]
fn assess_parse_fault_with_unreadable_head_is_a_refresh() {
    let fixture = ReportingFixture::new();
    let repo = unborn_repo(&fixture, "feature");
    let mut raw = fixture.envelope["candidate"].clone();
    raw["readiness"] = json!("ready");
    let before = listing(&fixture.directory);
    let out = assess(
        &fixture.auth,
        &fixture.directory,
        &repo,
        &serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap_or_else(|e| panic!("an unreadable HEAD must not fail: {e:#}"));
    assert_head_unreadable(&out);
    let blockers = out["blockers"].as_array().unwrap();
    assert_eq!(blockers.len(), 2, "{out}");
    assert!(
        blockers[0]
            .as_str()
            .unwrap()
            .starts_with("evidence-parse-error:"),
        "the parse blocker comes first: {out}"
    );
    assert_eq!(blockers[1], HEAD_UNREADABLE, "{out}");
    assert_eq!(listing(&fixture.directory), before);
}

#[test]
fn assess_unknown_binding_field_with_unreadable_head_is_a_refresh() {
    let fixture = ReportingFixture::new();
    let repo = unborn_repo(&fixture, "feature");
    let mut raw = fixture.envelope["candidate"].clone();
    raw["binding"]["head_sha"] = Value::Null;
    let out = assess(
        &fixture.auth,
        &fixture.directory,
        &repo,
        &serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap_or_else(|e| panic!("an unreadable HEAD must not fail: {e:#}"));
    assert_head_unreadable(&out);
    assert_eq!(
        out["blockers"],
        json!(["unknown-field:binding.head_sha", HEAD_UNREADABLE]),
        "unknown fields first, then the HEAD blocker: {out}"
    );
    assert_eq!(out["evidence_head"], "", "{out}");
}

#[test]
fn assess_current_binding_with_unreadable_head_is_a_refresh() {
    let fixture = ReportingFixture::new();
    let repo = unborn_repo(&fixture, "feature");
    let input = serde_json::to_vec(&fixture.envelope["candidate"]).unwrap();
    let before = listing(&fixture.directory);
    let out = assess(&fixture.auth, &fixture.directory, &repo, &input)
        .unwrap_or_else(|e| panic!("an unreadable HEAD must not fail: {e:#}"));
    assert_head_unreadable(&out);
    assert_eq!(
        out["blockers"],
        json!([HEAD_UNREADABLE]),
        "no head, base or merge-base comparison is made: {out}"
    );
    assert_eq!(out["evidence_head"], head_of(&fixture), "{out}");
    assert_eq!(listing(&fixture.directory), before, "nothing is written");
}

#[test]
fn binding_drift_with_unreadable_head_names_only_that_blocker() {
    let fixture = ReportingFixture::new();
    let repo = unborn_repo(&fixture, "feature");
    let b = fixture_binding(&fixture);
    assert_eq!(
        binding_drift(&repo, &b).unwrap(),
        vec![HEAD_UNREADABLE.to_string()]
    );
}

#[test]
fn unreadable_head_does_not_soften_integrity_checks() {
    let fixture = ReportingFixture::new();
    // The branch check runs before HEAD is read, so a mismatch stays an error.
    let repo = unborn_repo(&fixture, "other");
    let b = fixture_binding(&fixture);
    assert!(binding_drift(&repo, &b).is_err(), "branch mismatch is hard");
    let input = serde_json::to_vec(&fixture.envelope["candidate"]).unwrap();
    assert!(
        assess(&fixture.auth, &fixture.directory, &repo, &input).is_err(),
        "branch mismatch is hard through assess"
    );
    // An invalid binding never reaches Git, whatever the HEAD.
    let repo = unborn_repo(&fixture, "feature");
    let mut bad_ref = b.clone();
    bad_ref.base_branch = "bad..ref".into();
    assert!(binding_drift(&repo, &bad_ref).is_err());
    let mut short_sha = b;
    short_sha.head_sha = "abc".into();
    assert!(binding_drift(&repo, &short_sha).is_err());
}

#[test]
fn round_with_unreadable_head_writes_not_merge_ready() {
    let fixture = ReportingFixture::new();
    let repo = unborn_repo(&fixture, "feature");
    let record = fixture.directory.join("merge-ready-round-2.json");
    let input = serde_json::to_vec(&fixture.envelope["candidate"]).unwrap();
    let out = round(&fixture.auth, &fixture.directory, &repo, &record, &input)
        .unwrap_or_else(|e| panic!("an unreadable HEAD must not fail: {e:#}"));
    assert_not_ready_round(&record, &out, HEAD_UNREADABLE);
    assert_eq!(out["blockers"], json!([HEAD_UNREADABLE]), "{out}");
}

// ---- stale evidence is refreshed at the current head (D5) ---------------------

/// Evidence bound to an older commit A, with the worktree at B, gives
/// `refresh_required` naming B. The candidate re-measured at B, which is what
/// `remeasure-readiness` writes, is then accepted, and the accepted snapshot
/// is bound to B.
#[test]
fn evidence_refreshed_at_the_current_head_is_accepted() {
    let fixture = ReportingFixture::new();
    let head_b = head_of(&fixture);
    let commit_a = git(&fixture.repo, &["rev-parse", "HEAD~1"]);
    assert_ne!(commit_a, head_b);

    let mut at_a = fixture.envelope["candidate"].clone();
    rebind(&mut at_a, &head_b, &commit_a);
    let before = listing(&fixture.directory);
    let out = assess(
        &fixture.auth,
        &fixture.directory,
        &fixture.repo,
        &serde_json::to_vec(&at_a).unwrap(),
    )
    .unwrap_or_else(|e| panic!("stale evidence must not fail: {e:#}"));
    assert_refresh(&out, "stale-evidence-head");
    assert_eq!(out["evidence_head"], commit_a, "{out}");
    assert_eq!(out["current_head"], head_b, "{out}");
    assert_eq!(listing(&fixture.directory), before, "nothing is accepted");

    let at_b = serde_json::to_vec(&fixture.envelope["candidate"]).unwrap();
    let out = assess(&fixture.auth, &fixture.directory, &fixture.repo, &at_b).unwrap();
    assert_eq!(out["status"], "accepted", "{out}");
    assert_eq!(out["refresh_required"], false, "{out}");
    let accepted = out["accepted"].as_str().expect("snapshot path");
    let snapshot: Value = serde_json::from_slice(&std::fs::read(accepted).unwrap()).unwrap();
    assert_eq!(snapshot["binding"]["head_sha"], head_b, "{snapshot}");
}
