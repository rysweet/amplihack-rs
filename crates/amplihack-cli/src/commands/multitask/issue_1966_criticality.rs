//! Issue #1966 (D6): the parallel launcher copies an optional
//! `is_critical_code` field from the workstreams file into `context.json`
//! unchanged. Validation stays in the recipe; the launcher never trims,
//! case-folds, converts or defaults the value.
//!
//! Contract assumed by these tests:
//!
//!   WorkstreamConfig { .., #[serde(default)] pub is_critical_code: Option<String> }
//!   Workstream       { .., pub is_critical_code: Option<String> }
//!
//! `Workstream::new` keeps its signature and leaves the field `None`;
//! `ParallelOrchestrator::add` assigns it from the config afterwards.
//! `build_resume_context` inserts `Value::String(v.clone())` only when the
//! field is `Some`.

use super::command_builder::build_resume_context;
use super::command_builder_tests::{keep, no_env, workstream};
use super::launcher::write_recipe_launcher_with;
use super::models::WorkstreamConfig;
use serde_json::Value;

const KEY: &str = "is_critical_code";

fn config(json: &str) -> serde_json::Result<WorkstreamConfig> {
    serde_json::from_str(json)
}

fn written_context(value: Option<&str>) -> (tempfile::TempDir, Value) {
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = workstream(tmp.path());
    ws.is_critical_code = value.map(str::to_string);
    std::fs::create_dir_all(&ws.work_dir).unwrap();
    write_recipe_launcher_with(&ws, "amplihack claude", no_env, keep).unwrap();
    let raw = std::fs::read(ws.work_dir.join("context.json")).unwrap();
    let parsed = serde_json::from_slice(&raw).unwrap();
    (tmp, parsed)
}

// --- WorkstreamConfig parsing ------------------------------------------------

#[test]
fn config_without_field_parses_to_none() {
    let c = config(r#"{"issue":1,"branch":"b","task":"t"}"#).unwrap();
    assert_eq!(c.is_critical_code, None);
}

#[test]
fn config_string_field_is_kept_byte_exact() {
    for v in ["true", "false", " True", "maybe", "", "tr\u{1b}[31mue"] {
        let json = serde_json::json!({"issue":1,"branch":"b","task":"t", KEY: v}).to_string();
        let c = config(&json).unwrap();
        assert_eq!(c.is_critical_code.as_deref(), Some(v), "value {v:?}");
    }
}

#[test]
fn config_boolean_field_fails_to_parse() {
    // Documented: in a workstreams file the field must be a JSON string.
    for v in ["true", "false", "1", "{}", "[]"] {
        let json = format!(r#"{{"issue":1,"branch":"b","task":"t","{KEY}":{v}}}"#);
        assert!(
            config(&json).is_err(),
            "a non-string is_critical_code ({v}) must not parse"
        );
    }
}

#[test]
fn workstreams_file_with_one_boolean_field_fails_as_a_whole() {
    let file = r#"[
        {"issue":1,"branch":"a","task":"t","is_critical_code":"true"},
        {"issue":2,"branch":"b","task":"t","is_critical_code":true}
    ]"#;
    let parsed: serde_json::Result<Vec<WorkstreamConfig>> = serde_json::from_str(file);
    assert!(
        parsed.is_err(),
        "one Boolean field must fail the whole file so no workstream starts"
    );
}

#[test]
fn workstream_new_leaves_field_unset() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(workstream(tmp.path()).is_critical_code, None);
}

// --- build_resume_context ----------------------------------------------------

#[test]
fn resume_context_has_no_key_when_field_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workstream(tmp.path());
    let ctx = build_resume_context(&ws);
    assert!(
        !ctx.contains_key(KEY),
        "no default may be supplied; the recipe reports the missing input"
    );
}

#[test]
fn resume_context_copies_value_without_normalising() {
    let tmp = tempfile::tempdir().unwrap();
    for v in ["true", "false", " True", "FALSE", "maybe", ""] {
        let mut ws = workstream(tmp.path());
        ws.is_critical_code = Some(v.to_string());
        let ctx = build_resume_context(&ws);
        assert_eq!(
            ctx.get(KEY),
            Some(&Value::String(v.to_string())),
            "value {v:?} must arrive as the same JSON string"
        );
    }
}

// --- context.json ------------------------------------------------------------

#[test]
fn context_json_has_no_key_when_field_absent() {
    let (_tmp, ctx) = written_context(None);
    assert!(ctx.get(KEY).is_none(), "context.json: {ctx}");
}

#[test]
fn context_json_carries_value_byte_exact() {
    let (_tmp, ctx) = written_context(Some(" True"));
    assert_eq!(ctx[KEY], Value::String(" True".to_string()));
    let (_tmp, ctx) = written_context(Some("false"));
    assert_eq!(ctx[KEY], Value::String("false".to_string()));
}

#[cfg(unix)]
#[test]
fn context_json_with_field_is_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = workstream(tmp.path());
    ws.is_critical_code = Some("true".to_string());
    std::fs::create_dir_all(&ws.work_dir).unwrap();
    write_recipe_launcher_with(&ws, "amplihack claude", no_env, keep).unwrap();
    let mode = std::fs::metadata(ws.work_dir.join("context.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "context.json mode {mode:o}");
}

#[test]
fn value_never_reaches_launcher_or_run_script_text() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = workstream(tmp.path());
    let marker = "crit-$(touch pwned)-MARKER-1966";
    ws.is_critical_code = Some(marker.to_string());
    std::fs::create_dir_all(&ws.work_dir).unwrap();
    write_recipe_launcher_with(&ws, "amplihack claude", no_env, keep).unwrap();
    for script in ["launcher.sh", "run.sh"] {
        let text = std::fs::read_to_string(ws.work_dir.join(script)).unwrap();
        assert!(
            !text.contains("MARKER-1966"),
            "{script} must not embed the value: {text}"
        );
    }
}

// --- orchestrator copy-through -----------------------------------------------

/// `add` clones a repository, so the assignment is checked in the source:
/// the field is copied from the config after `Workstream::new`, unchanged.
#[test]
fn orchestrator_add_copies_field_from_config() {
    let source = include_str!("orchestrator.rs");
    let new_at = source
        .find("Workstream::new(")
        .expect("orchestrator calls Workstream::new");
    let assign = "ws.is_critical_code = config.is_critical_code.clone();";
    let assign_at = source
        .find(assign)
        .unwrap_or_else(|| panic!("orchestrator.rs must contain `{assign}`"));
    assert!(assign_at > new_at, "assignment must follow Workstream::new");
}
