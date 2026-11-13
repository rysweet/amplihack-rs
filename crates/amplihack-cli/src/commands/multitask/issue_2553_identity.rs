//! Issue #2553 — a workstream's identity is separate from the issue it tracks.
//!
//! Fault 2 was that a decomposition minted `issue: "TBD"`, which read back as
//! `0`, which tripped the `Invalid issue number` bail. Fixing it by letting
//! the issue be absent forces the structural change these tests pin: identity
//! (`key`, always present, unique within a run) stops being the tracking
//! number (`issue`, which may be absent).
//!
//! That change also moves an attacker-influenceable string into four path and
//! shell sinks that an `i64` had been closing by construction. The tests
//! marked SR* below are the replacement for that accident.
//!
//! Nothing here sleeps, polls, reads the clock, spawns a workstream, touches
//! the network, or writes outside a `TempDir`. `bash -n` parses a script
//! without running it.

use super::command_builder::build_resume_context_with;
use super::issue::{IssueNumber, parse_optional};
use super::models::{
    PersistedState, Workstream, WorkstreamConfig, WorkstreamKey, derive_key, work_dir_for,
};
use super::orchestrator::ParallelOrchestrator;
use serde_json::{Value, json};
use std::path::{Component, Path};

// ---- fixtures -------------------------------------------------------------

/// An injected environment in which nothing is set, so no test in this module
/// reads the process environment that its neighbours may be changing. The
/// context builder and the script builder read through different seams.
fn no_env(_key: &str) -> Option<String> {
    None
}

fn config(fields: Value) -> WorkstreamConfig {
    serde_json::from_value(fields).expect("a well-formed workstream config")
}

fn untracked_config(description: &str) -> WorkstreamConfig {
    config(json!({
        "branch": "feat/orch-1-triage",
        "description": description,
        "task": "Report the lane status in chat.",
        "recipe": "operations-direct",
    }))
}

fn workstream(dir: &Path, key: &str, issue: Option<u64>) -> Workstream {
    Workstream::new(
        WorkstreamKey::new(key).expect("a valid key"),
        issue,
        "feat/orch-1-triage".to_string(),
        "Triage the open issues".to_string(),
        "Report the lane status in chat.".to_string(),
        "operations-direct".to_string(),
        dir,
        dir,
    )
}

// ---- R6b: one parser, total over well-formed input ------------------------

#[test]
fn every_placeholder_form_reads_as_absence_rather_than_failure() {
    for raw in [
        Value::Null,
        json!(""),
        json!("   "),
        json!("TBD"),
        json!(" tbd "),
        json!("TBC"),
        json!("todo"),
        json!("N/A"),
        json!("none"),
        json!("null"),
        json!("Unknown"),
        json!("unassigned"),
        json!("pending"),
    ] {
        assert_eq!(
            parse_optional(&raw).expect("a placeholder is not an error"),
            None,
            "{raw}"
        );
    }
}

#[test]
fn a_missing_field_reads_as_absence() {
    let cfg = untracked_config("Triage the open issues");
    assert_eq!(cfg.issue().expect("absence is not an error"), None);
}

#[test]
fn every_accepted_present_form_yields_the_same_number() {
    for raw in [
        json!(2553),
        json!("2553"),
        json!("002553"),
        json!("#2553"),
        json!("https://github.com/rysweet/amplihack-rs/issues/2553"),
    ] {
        assert_eq!(
            parse_optional(&raw)
                .expect("an accepted form parses")
                .map(IssueNumber::get),
            Some(2553),
            "{raw}"
        );
    }
}

/// SR4 — the URL form is parsed lexically, with no network, no DNS and no
/// redirect following, and only for `github.com`. Without the host check,
/// `https://attacker.example/.../issues/1` yields `1`, and an agent told to
/// comment on or close the tracked issue acts on issue 1 of the current
/// repository: a confused deputy produced by a value the operator believed
/// pointed somewhere else.
#[test]
fn an_issue_url_is_accepted_only_for_github_com() {
    for hostile in [
        json!("https://attacker.example/rysweet/amplihack-rs/issues/1"),
        json!("https://github.com.attacker.example/o/r/issues/1"),
        json!("https://evil.github.com.co/o/r/issues/1"),
        json!("http://attacker.example/issues/1"),
        json!("https://github.com/rysweet/amplihack-rs/pull/2553"),
        json!("https://github.com/rysweet/amplihack-rs/issues/not-a-number"),
    ] {
        assert!(
            parse_optional(&hostile).is_err(),
            "{hostile} must not resolve to an issue number"
        );
    }
}

/// The accept set for absence widens; the accept set for presence does not.
#[test]
fn a_malformed_value_stays_an_error() {
    for raw in [
        json!("abc123"),
        json!("12x"),
        json!("#"),
        json!("#0"),
        json!(-5),
        json!(0),
        json!("0"),
        json!(2.5),
        json!(true),
        json!([2553]),
        json!({"number": 2553}),
    ] {
        assert!(parse_optional(&raw).is_err(), "{raw}");
    }
}

/// SR7, second half — in configuration `0` is a producer mistake worth
/// surfacing; the legacy coercion to absence is confined to persisted state.
#[test]
fn zero_is_an_error_in_configuration_and_absence_only_in_persisted_state() {
    assert!(parse_optional(&json!(0)).is_err());

    let legacy: PersistedState =
        serde_json::from_str(r#"{"issue": 0, "branch": "feat/x"}"#).expect("legacy state parses");
    assert_eq!(legacy.issue, None);
}

// ---- SR1/SR2: identity derivation and the paths it feeds ------------------

/// With an issue present the key is the issue number, so every path a running
/// workstream already owns is byte-identical to today's and resume needs no
/// migration.
#[test]
fn a_tracked_workstream_keeps_todays_paths_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base");
    let state = dir.path().join("state");
    let ws = Workstream::new(
        WorkstreamKey::new("123").unwrap(),
        Some(123),
        "feat/x".to_string(),
        "d".to_string(),
        "t".to_string(),
        "default-workflow".to_string(),
        &base,
        &state,
    );

    assert_eq!(ws.work_dir, base.join("ws-123"));
    assert_eq!(ws.log_file, base.join("log-123.txt"));
    assert_eq!(ws.state_file, state.join("ws-123.json"));
    assert_eq!(ws.progress_file, state.join("ws-123.progress.json"));
}

#[test]
fn a_tracked_config_derives_its_key_from_the_issue_number() {
    let cfg = config(json!({
        "issue": "#2553",
        "branch": "feat/orch-1-triage",
        "task": "t",
    }));
    assert_eq!(derive_key(&cfg, 1).unwrap().as_str(), "2553");
}

/// An untracked workstream is named from its description, or from its branch
/// when it has no description, plus the one-based index.
#[test]
fn an_untracked_config_derives_its_key_from_the_description_and_index() {
    let cfg = untracked_config("Triage");
    assert_eq!(derive_key(&cfg, 1).unwrap().as_str(), "triage-1");

    let no_description = config(json!({"branch": "feat/orch-2-triage", "task": "t"}));
    assert_eq!(
        derive_key(&no_description, 2).unwrap().as_str(),
        "feat-orch-2-triage-2"
    );
}

/// SR6 — two untracked workstreams in one decomposition may carry the same
/// description. If the index did not enter the key they would share a work
/// directory, a log, a state file and a process-map slot.
#[test]
fn two_untracked_workstreams_never_share_an_identity() {
    let cfg = untracked_config("Triage");
    let first = derive_key(&cfg, 1).unwrap();
    let second = derive_key(&cfg, 2).unwrap();
    assert_ne!(first.as_str(), second.as_str());

    let base = Path::new("/tmp/amplihack-workstreams");
    assert_ne!(work_dir_for(base, &first), work_dir_for(base, &second));
}

/// SR1 — the untracked key derives from a model-produced field whose input
/// may include issue bodies, PR comments or repository content. The `i64` it
/// replaces could not carry a separator, a parent reference or a quote; the
/// charset is what keeps that true.
#[test]
fn an_adversarial_description_cannot_escape_the_base_directory() {
    let base = Path::new("/tmp/amplihack-workstreams");
    for hostile in [
        "../../etc",
        "../../../root/.ssh",
        "a/b/c",
        "it's; rm -rf /",
        "$(touch pwned)",
        "`touch pwned2`",
        "x\ny",
        "..",
    ] {
        let cfg = untracked_config(hostile);
        let key = derive_key(&cfg, 1).expect("derivation never fails on hostile input");
        assert!(
            key.as_str()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "key {key} from {hostile:?} left the charset"
        );
        let dir = work_dir_for(base, &key);
        assert!(dir.starts_with(base), "{dir:?} escaped {base:?}");
        assert_eq!(
            dir.components()
                .filter(|c| matches!(c, Component::ParentDir))
                .count(),
            0,
            "{dir:?} contains a parent reference"
        );
    }
}

/// SR1 — the constructor fails closed. A fallback key would silently merge
/// two workstreams into one identity, which is the outcome SR6 exists to
/// prevent.
#[test]
fn the_key_constructor_rejects_everything_outside_the_charset() {
    for hostile in [
        "",
        "..",
        "../../etc",
        "a/b",
        "a b",
        "a'b",
        "a;b",
        "a\nb",
        "a$b",
        "a`b",
        "ws-\u{00e9}",
    ] {
        assert!(WorkstreamKey::new(hostile).is_err(), "{hostile:?}");
    }
    assert!(WorkstreamKey::new("2553").is_ok());
    assert!(WorkstreamKey::new("triage-1").is_ok());
    assert!(WorkstreamKey::new("a_b-1").is_ok());
    assert!(WorkstreamKey::new(&"x".repeat(64)).is_ok());
    assert!(
        WorkstreamKey::new(&"x".repeat(65)).is_err(),
        "the length bound is enforced"
    );
}

/// SR2 — `Workstream::new` and `cleanup_merged` derived the work directory
/// independently, and only one of them sanitised. One derivation, used by
/// both.
#[test]
fn the_work_directory_has_exactly_one_derivation() {
    let dir = tempfile::tempdir().unwrap();
    let key = WorkstreamKey::new("triage-1").unwrap();
    let ws = workstream(dir.path(), key.as_str(), None);
    assert_eq!(ws.work_dir, work_dir_for(dir.path(), &key));
}

// ---- R6c: duplicate identity is rejected, not overwritten -----------------

#[test]
fn a_duplicate_identity_is_rejected_rather_than_overwriting_the_first() {
    let mut orch = ParallelOrchestrator::new("https://github.com/o/r", "recipe");
    let cfg = config(json!({"issue": 2553, "branch": "feat/a", "task": "t"}));

    orch.register(1, &cfg, "default-workflow")
        .expect("the first workstream registers");
    let err = orch
        .register(2, &cfg, "default-workflow")
        .expect_err("the second must not silently take the first's directory")
        .to_string()
        .to_lowercase();
    assert!(err.contains("duplicate"), "{err}");
    assert!(err.contains("2553"), "the error names the identity: {err}");
}

#[test]
fn distinct_untracked_workstreams_both_register() {
    let mut orch = ParallelOrchestrator::new("https://github.com/o/r", "recipe");
    let cfg = untracked_config("Triage");

    let first = orch.register(1, &cfg, "default-workflow").unwrap();
    let first_dir = first.artefact_dir.clone();
    let second = orch.register(2, &cfg, "default-workflow").unwrap();
    assert_ne!(first_dir, second.artefact_dir);
}

/// R6 — the end of the path #2553 reported: a decomposition that yields an
/// issue-less workstream reaches the orchestrator without the bail, and the
/// context handed to the child carries no fabricated number.
#[test]
fn an_issue_less_decomposition_reaches_execution_without_a_fabricated_number() {
    let plan = crate::commands::orch::build_workstreams_config_to_tempfile(
        &json!({
            "task_type": "Operations",
            "workstreams": [{"name": "Triage", "description": "Triage the open issues."}]
        })
        .to_string(),
    )
    .expect("the plan is written");
    let body = std::fs::read_to_string(&plan).unwrap();
    std::fs::remove_file(&plan).ok();
    let items: Vec<WorkstreamConfig> = serde_json::from_str(&body).expect("the plan deserialises");

    let mut orch = ParallelOrchestrator::new("https://github.com/o/r", "recipe");
    let ws = orch
        .register(1, &items[0], "operations-direct")
        .expect("an absent issue is not an error");

    assert_eq!(ws.issue, None);
    let ctx = build_resume_context_with(ws, no_env);
    assert!(!ctx.contains_key("issue_number"), "{ctx:?}");
    assert!(
        !ctx.values().any(|v| v == &json!(0) || v == &json!("TBD")),
        "no sentinel reaches the child context: {ctx:?}"
    );
}

/// The retired diagnostic. Its absence on the issue-less path is the whole
/// of fault 2.
#[test]
fn the_invalid_issue_number_bail_is_unreachable_without_an_issue() {
    let mut orch = ParallelOrchestrator::new("https://github.com/o/r", "recipe");
    for description in ["Triage", "Report status", "Open a cross-check issue"] {
        let cfg = untracked_config(description);
        let outcome = orch.register(1, &cfg, "operations-direct");
        let text = outcome.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            !text.contains("Invalid issue number"),
            "{description}: {text}"
        );
    }
}
