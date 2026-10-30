//! Issue #1740: how smart-orchestrator's phase recipes select and run
//! `quick-change`. Structural checks on the live YAML only; no recipe runs.
//! The step inventory and the reflection and validation conditions (D1) are
//! checked in `tests/integration/smart_orchestrator_decomposition_test.rs`.

use std::path::PathBuf;

use serde_yaml::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn recipe(name: &str) -> Value {
    let path = repo_root().join(format!("amplifier-bundle/recipes/{name}.yaml"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {path:?}: {e}"))
}

fn steps(r: &Value) -> &Vec<Value> {
    r["steps"].as_sequence().expect("steps")
}

fn step<'a>(r: &'a Value, id: &str) -> &'a Value {
    steps(r)
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("step {id} missing"))
}

#[test]
fn classifier_prompt_asks_for_the_workflow_fields_and_small_task_categories() {
    let r = recipe("smart-classify-route");
    let prompt = step(&r, "classify-and-decompose")["prompt"]
        .as_str()
        .expect("classify prompt")
        .to_lowercase();
    for needle in [
        "recommended_workflow",
        "workflow_reason",
        "quick-change",
        "documentation",
        "lock",
        "typo",
        "when in doubt",
    ] {
        assert!(
            prompt.contains(needle),
            "classify prompt must mention {needle:?}"
        );
    }
}

#[test]
fn select_workflow_step_follows_activate_workflow_and_reads_only_recipe_vars() {
    let r = recipe("smart-classify-route");
    let ids: Vec<&str> = steps(&r).iter().filter_map(|s| s["id"].as_str()).collect();
    let at = ids.iter().position(|i| *i == "activate-workflow").unwrap();
    assert_eq!(ids.get(at + 1), Some(&"select-workflow"), "{ids:?}");
    let s = step(&r, "select-workflow");
    assert_eq!(s["type"].as_str(), Some("bash"));
    assert_eq!(s["output"].as_str(), Some("selected_workflow"));
    let cmd = s["command"].as_str().unwrap();
    for needle in [
        "orch helper select-workflow",
        "--override",
        "RECIPE_VAR_workflow",
        "--existing-branch \"${RECIPE_VAR_existing_branch:-}\"",
        "--pr-number \"${RECIPE_VAR_pr_number:-}\"",
        "mktemp",
    ] {
        assert!(
            cmd.contains(needle),
            "select-workflow command must contain {needle:?}"
        );
    }
    assert!(
        !cmd.contains("{{"),
        "select-workflow must not template values into bash"
    );
}

#[test]
fn orchestrator_and_phase_recipes_declare_the_workflow_context() {
    let ctx = recipe("smart-orchestrator")["context"].clone();
    assert_eq!(ctx["workflow"].as_str(), Some(""));
    assert_eq!(ctx["selected_workflow"].as_str(), Some(""));
    for name in [
        "smart-classify-route",
        "smart-execute-routing",
        "smart-reflect-loop",
        "smart-validate-summarize",
    ] {
        let ctx = recipe(name)["context"].clone();
        assert!(
            ctx.get("selected_workflow").is_some(),
            "{name} must declare selected_workflow in context"
        );
    }
    for name in ["smart-classify-route", "smart-execute-routing"] {
        assert!(
            recipe(name)["context"].get("workflow").is_some(),
            "{name} must declare workflow in context"
        );
    }
}

fn condition(r: &Value, id: &str) -> String {
    step(r, id)["condition"]
        .as_str()
        .unwrap_or_else(|| panic!("step {id} must have a condition"))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn development_round_1_excludes_quick_change() {
    let r = recipe("smart-execute-routing");
    let c = condition(&r, "execute-single-round-1-development");
    assert!(c.contains("selected_workflow != 'quick-change'"), "{c}");
    assert_eq!(
        step(&r, "execute-single-round-1-development")["recipe"].as_str(),
        Some("default-workflow")
    );
}

#[test]
fn quick_change_round_1_follows_development_and_forwards_the_same_context() {
    let r = recipe("smart-execute-routing");
    let ids: Vec<&str> = steps(&r).iter().filter_map(|s| s["id"].as_str()).collect();
    let at = ids
        .iter()
        .position(|i| *i == "execute-single-round-1-development")
        .unwrap();
    assert_eq!(
        ids.get(at + 1),
        Some(&"execute-single-round-1-quick-change"),
        "{ids:?}"
    );
    let qc = step(&r, "execute-single-round-1-quick-change");
    assert_eq!(qc["type"].as_str(), Some("recipe"));
    assert_eq!(qc["recipe"].as_str(), Some("quick-change"));
    assert_eq!(qc["output"].as_str(), Some("round_1_result"));
    let c = condition(&r, "execute-single-round-1-quick-change");
    assert!(c.contains("selected_workflow == 'quick-change'"), "{c}");
    assert!(c.contains("'Development' in task_type"), "{c}");
    let dev = &step(&r, "execute-single-round-1-development")["context"];
    assert_eq!(
        &qc["context"], dev,
        "quick-change must get the same forwarded keys"
    );
}

#[test]
fn create_workstreams_config_reads_the_override_from_the_environment() {
    let r = recipe("smart-execute-routing");
    let cmd = step(&r, "create-workstreams-config")["command"]
        .as_str()
        .unwrap();
    assert!(cmd.contains("build-workstreams-config"), "{cmd}");
    assert!(
        !cmd.contains("{{"),
        "override must reach bash as RECIPE_VAR_workflow"
    );
}

#[test]
fn blocked_fallback_and_adaptive_paths_stay_on_default_workflow() {
    let r = recipe("smart-execute-routing");
    for id in [
        "execute-single-fallback-blocked-development",
        "adaptive-execute-development",
    ] {
        let s = step(&r, id);
        assert_ne!(s["recipe"].as_str(), Some("quick-change"), "{id}");
        assert!(
            !s["condition"]
                .as_str()
                .unwrap_or("")
                .contains("quick-change"),
            "{id} must not route on selected_workflow"
        );
    }
}

#[test]
fn smart_recipes_stay_within_400_lines() {
    for name in [
        "smart-orchestrator",
        "smart-classify-route",
        "smart-execute-routing",
        "smart-reflect-loop",
        "smart-validate-summarize",
    ] {
        let n = std::fs::read_to_string(
            repo_root().join(format!("amplifier-bundle/recipes/{name}.yaml")),
        )
        .unwrap()
        .lines()
        .count();
        assert!(n <= 400, "{name}.yaml is {n} lines");
    }
}
