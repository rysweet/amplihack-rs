//! Issue #2553 — what an issue-less workstream hands to its child, what it
//! persists, and what it may adopt (continued from `issue_2553_identity.rs`).
//!
//! Split only to keep both files inside the 400-line brick limit that
//! `multitask_orchestrator_spec_test` enforces.
//!
//! Nothing here sleeps, polls, reads the clock, spawns a workstream, touches
//! the network, or writes outside a `TempDir`. `bash -n` parses a script
//! without running it.

use super::command_builder::{CREATE_ISSUE_ENV, build_resume_context_with, recipe_run_script};
use super::models::{
    PersistedState, ProcessScope, Workstream, WorkstreamConfig, WorkstreamKey, WorkstreamScope,
    derive_key,
};
use super::process_scope::{
    CurrentWorkflowScope, ProcessScopeConfig, ProcessScopeValidation, ProcessSnapshot,
    validate_process_scope,
};
use super::state::apply_saved_state;
use chrono::Utc;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::Path;

// ---- fixtures -------------------------------------------------------------

/// An injected environment in which nothing is set, so no test in this module
/// reads the process environment that its neighbours may be changing. The
/// context builder and the script builder read through different seams.
fn no_env(_key: &str) -> Option<String> {
    None
}

fn no_os_env(_key: &str) -> Option<OsString> {
    None
}

/// The auto-switch substitution seam with no substitution.
fn keep(_inherited: &str) -> Option<String> {
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

// ---- R6a/SR3: what reaches the child --------------------------------------

#[test]
fn a_tracked_workstream_still_carries_its_issue_number() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workstream(dir.path(), "2553", Some(2553));
    let ctx =
        build_resume_context_with(&ws, |k| (k == CREATE_ISSUE_ENV).then(|| "true".to_string()));
    assert_eq!(ctx.get("issue_number"), Some(&json!(2553)));
    assert_eq!(ctx.get("create_issue"), Some(&json!("true")));
}

/// An Operations leaf must not create tracking issues from a leaked default:
/// `create_issue` propagates only when the parent explicitly set it.
#[test]
fn create_issue_does_not_propagate_from_an_unset_or_blank_parent_value() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workstream(dir.path(), "triage-1", None);
    assert!(!build_resume_context_with(&ws, no_env).contains_key("create_issue"));
    assert!(
        !build_resume_context_with(&ws, |_| Some("   ".to_string())).contains_key("create_issue")
    );
}

/// Omission rather than sentinel: a consumer testing for presence cannot be
/// fooled, while one comparing against `0` would be.
#[test]
fn the_run_script_omits_the_issue_export_when_nothing_is_tracked() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workstream(dir.path(), "triage-1", None);
    let script = recipe_run_script(&ws, "amplihack claude", no_os_env, keep).unwrap();

    assert!(
        !script.contains("AMPLIHACK_WORKSTREAM_ISSUE"),
        "no issue export without an issue: {script}"
    );
    assert!(
        !script.contains("AMPLIHACK_WORKSTREAM_KEY"),
        "the identity key is not exported to the child: {script}"
    );

    let tracked = workstream(dir.path(), "2553", Some(2553));
    let script = recipe_run_script(&tracked, "amplihack claude", no_os_env, keep).unwrap();
    assert!(
        script.contains("export AMPLIHACK_WORKSTREAM_ISSUE='2553'\n"),
        "{script}"
    );
}

/// SR3 — the charset excluding the single quote is what closes
/// `cd '{work_dir}'`. `bash -n` parses the script and runs nothing.
#[test]
fn an_adversarial_description_cannot_break_the_run_script_quoting() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = untracked_config("it's $(touch pwned) `touch pwned2`; rm -rf /");
    let key = derive_key(&cfg, 1).unwrap();
    let ws = workstream(dir.path(), key.as_str(), None);
    let script = recipe_run_script(&ws, "amplihack claude", no_os_env, keep).unwrap();

    let path = dir.path().join("run.sh");
    std::fs::write(&path, &script).unwrap();
    // `bash` is resolved from PATH, which other tests point at an empty
    // tempdir while holding the env lock; hold it so the spawn cannot fail
    // with ENOENT (#1730, #1731).
    let _env = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let out = std::process::Command::new("bash")
        .arg("-n")
        .arg(&path)
        .output()
        .expect("bash parses the script");
    assert!(
        out.status.success(),
        "generated script does not parse: {}\n{script}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !dir.path().join("pwned").exists() && !dir.path().join("pwned2").exists(),
        "parsing must not execute anything"
    );
}

// ---- R6d/SR7: persisted state ---------------------------------------------

#[test]
fn persisted_state_round_trips_an_absent_issue_as_an_absent_field() {
    let state = PersistedState {
        key: "triage-1".to_string(),
        issue: None,
        branch: "feat/orch-1-triage".to_string(),
        lifecycle_state: "completed".to_string(),
        ..Default::default()
    };
    let json = serde_json::to_string(&state).unwrap();
    assert!(
        !json.contains("\"issue\""),
        "an absent issue is not serialised: {json}"
    );

    let parsed: PersistedState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.issue, None);
    assert_eq!(parsed.key, "triage-1");
    assert_eq!(parsed.lifecycle_state, "completed");
}

#[test]
fn a_tracked_issue_round_trips_unchanged_and_legacy_files_still_read() {
    let state = PersistedState {
        key: "2553".to_string(),
        issue: Some(2553),
        ..Default::default()
    };
    let parsed: PersistedState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    assert_eq!(parsed.issue, Some(2553));

    // Written before this change: no `key`, and a real issue number.
    let legacy: PersistedState =
        serde_json::from_str(r#"{"issue": 42, "branch": "feat/x", "lifecycle_state": "running"}"#)
            .expect("a legacy state file still parses");
    assert_eq!(legacy.issue, Some(42));
    assert_eq!(legacy.lifecycle_state, "running");
}

/// SR7 — a state file is untrusted input. The in-memory, config-derived key
/// owns the paths; a key read back from disk never constructs one, so
/// anything with write access to the state directory cannot point a run at
/// another run's work.
#[test]
fn a_key_read_from_disk_never_redirects_the_paths() {
    let dir = tempfile::tempdir().unwrap();
    let mut ws = workstream(dir.path(), "triage-1", None);
    let expected_work_dir = ws.work_dir.clone();
    let expected_state_file = ws.state_file.clone();

    let foreign = PersistedState {
        key: "../../victim".to_string(),
        issue: Some(99),
        branch: "feat/victim".to_string(),
        work_dir: "/tmp/victim".to_string(),
        ..Default::default()
    };
    apply_saved_state(&mut ws, &foreign);

    assert_eq!(ws.key.as_str(), "triage-1");
    assert_eq!(ws.work_dir, expected_work_dir);
    assert_eq!(ws.state_file, expected_state_file);
}

// ---- SR5: process ownership stays fail-closed -----------------------------

fn scopes_for(key: &str, issue_id: &str) -> (CurrentWorkflowScope, WorkstreamScope, ProcessScope) {
    let workstream_id = format!("ws-{key}");
    let current = CurrentWorkflowScope {
        repository: "owner/repo".to_string(),
        repo_root: "/repo".to_string(),
        workdir: format!("/repo/{workstream_id}"),
        branch: "feat/orch-1-triage".to_string(),
        issue_id: issue_id.to_string(),
        work_item_id: key.to_string(),
        recipe_run_id: "run-1".to_string(),
        tree_id: "tree-1".to_string(),
        workstream_id: workstream_id.clone(),
    };
    let workstream_scope = WorkstreamScope {
        repository: current.repository.clone(),
        repo_root: current.repo_root.clone(),
        workdir: current.workdir.clone(),
        branch: current.branch.clone(),
        base_ref: "main".to_string(),
        issue_id: current.issue_id.clone(),
        work_item_id: current.work_item_id.clone(),
        recipe: "operations-direct".to_string(),
        recipe_run_id: current.recipe_run_id.clone(),
        tree_id: current.tree_id.clone(),
        workstream_id: current.workstream_id.clone(),
        expected_title_prefix: "Triage".to_string(),
        started_at: Utc::now().to_rfc3339(),
    };
    let process_scope = ProcessScope {
        pid: Some(4242),
        repository: current.repository.clone(),
        repo_root: current.repo_root.clone(),
        workdir: current.workdir.clone(),
        branch: current.branch.clone(),
        issue_id: current.issue_id.clone(),
        work_item_id: current.work_item_id.clone(),
        recipe_run_id: current.recipe_run_id.clone(),
        tree_id: current.tree_id.clone(),
        workstream_id: current.workstream_id.clone(),
        process_started_at: "start-1".to_string(),
        recorded_at: Utc::now().to_rfc3339(),
    };
    (current, workstream_scope, process_scope)
}

fn snapshot(workdir: &str) -> ProcessSnapshot {
    ProcessSnapshot {
        pid: 4242,
        alive: true,
        workdir: workdir.to_string(),
        process_started_at: "start-1".to_string(),
    }
}

/// The presence gate moves from `issue_id` to `work_item_id`, which carries
/// the key and is never empty. An untracked workstream must still validate
/// against its own process.
#[test]
fn an_untracked_workstream_validates_against_its_own_process() {
    let (current, workstream_scope, process_scope) = scopes_for("triage-1", "");
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot(&current.workdir),
            &ProcessScopeConfig::default(),
        ),
        ProcessScopeValidation::Valid
    );
}

/// The gate keeps its width. `work_item_id` is checked on BOTH scopes; the
/// base checked it on the process scope only.
#[test]
fn an_empty_work_item_id_on_either_scope_is_missing_scope() {
    for blank_on_workstream in [true, false] {
        let (current, mut workstream_scope, mut process_scope) = scopes_for("triage-1", "");
        if blank_on_workstream {
            workstream_scope.work_item_id = String::new();
        } else {
            process_scope.work_item_id = String::new();
        }
        assert_eq!(
            validate_process_scope(
                &current,
                &workstream_scope,
                &process_scope,
                &snapshot(&current.workdir),
                &ProcessScopeConfig::default(),
            ),
            ProcessScopeValidation::MissingScope,
            "blank_on_workstream={blank_on_workstream}"
        );
    }
}

/// SR6's residual risk, pinned: for two untracked, branch-sharing workstreams
/// in one run, repository, repo_root, branch and issue_id are all equal and
/// recipe_run_id and tree_id are shared. Identity collapses to the key, and
/// the key is what must stop one adopting the other's process.
#[test]
fn two_untracked_workstreams_cannot_adopt_each_others_processes() {
    let (current, _, _) = scopes_for("triage-1", "");
    let (_, foreign_workstream, foreign_process) = scopes_for("triage-2", "");

    let verdict = validate_process_scope(
        &current,
        &foreign_workstream,
        &foreign_process,
        &snapshot(&foreign_workstream.workdir),
        &ProcessScopeConfig::default(),
    );
    assert!(!verdict.is_valid(), "{verdict:?}");

    // With the work directory forced equal — the collision SR6 exists to
    // prevent — the key is the only field left to separate them, and it must.
    let (_, mut foreign_workstream, mut foreign_process) = scopes_for("triage-2", "");
    foreign_workstream.workdir = current.workdir.clone();
    foreign_workstream.repo_root = current.repo_root.clone();
    foreign_process.workdir = current.workdir.clone();
    foreign_process.repo_root = current.repo_root.clone();
    assert_eq!(
        validate_process_scope(
            &current,
            &foreign_workstream,
            &foreign_process,
            &snapshot(&current.workdir),
            &ProcessScopeConfig::default(),
        ),
        ProcessScopeValidation::WorkstreamMismatch
    );
}

/// Every equality comparison the base performed is still performed, so the
/// relaxation cannot be read as a general loosening.
#[test]
fn a_tracked_workstream_still_rejects_a_foreign_issue_id() {
    let (current, workstream_scope, mut process_scope) = scopes_for("2553", "2553");
    process_scope.issue_id = "2554".to_string();
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot(&current.workdir),
            &ProcessScopeConfig::default(),
        ),
        ProcessScopeValidation::WorkstreamMismatch
    );
}

#[test]
fn an_untracked_workstream_still_rejects_a_foreign_branch() {
    let (current, mut workstream_scope, mut process_scope) = scopes_for("triage-1", "");
    workstream_scope.branch = "feat/other".to_string();
    process_scope.branch = "feat/other".to_string();
    assert_eq!(
        validate_process_scope(
            &current,
            &workstream_scope,
            &process_scope,
            &snapshot(&current.workdir),
            &ProcessScopeConfig::default(),
        ),
        ProcessScopeValidation::BranchMismatch
    );
}
