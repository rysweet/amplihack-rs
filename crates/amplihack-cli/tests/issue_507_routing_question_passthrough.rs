//! Regression tests for issue #507: when smart-orchestrator routes a
//! task to investigation-workflow via `smart-execute-routing.yaml`, the
//! sub-recipe receives `task_description` (the parent context name) but
//! investigation-workflow's downstream prompts read
//! `investigation_question`. Without an explicit context mapping the
//! nested recipe runs with an empty question and produces hollow
//! success.
//!
//! Per Decision 3 in the requirements doc the fix is at the routing
//! layer: every step in `smart-execute-routing.yaml` that invokes
//! `investigation-workflow` must carry an explicit `context:` block
//! that maps `investigation_question: "{{task_description}}"` (and
//! preserves `task_description` itself for `context_validation`).
//!
//! These tests parse the live YAML files in `amplifier-bundle/recipes/`
//! and assert structural invariants. They are intentionally
//! parser-tolerant — we only check the keys/values that the contract
//! depends on, not the surrounding step ordering.
//!
//! TDD note: the first test is expected to FAIL until the routing YAML
//! is updated. The second test is a guardrail asserting the
//! defense-in-depth `normalize-question` step in
//! `investigation-prep.yaml` is preserved (per Decision 3 it stays as a
//! belt-and-braces fallback alongside the explicit routing context).

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_recipes_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .find(|p| p.join("amplifier-bundle/recipes").is_dir())
        .map(|p| p.join("amplifier-bundle/recipes"))
        .expect("workspace must contain amplifier-bundle/recipes/")
}

fn load_yaml(path: &Path) -> Value {
    let body = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    serde_yaml::from_str(&body)
        .unwrap_or_else(|e| panic!("failed to parse {} as YAML: {e}", path.display()))
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .map(Vec::as_slice)
        .expect("recipe must have a top-level `steps:` sequence")
}

/// Returns every step in the recipe whose `recipe:` key resolves to
/// `investigation-workflow` — there may be more than one (single vs.
/// multitask routes both invoke it).
fn investigation_invocations(recipe: &Value) -> Vec<&Value> {
    steps(recipe)
        .iter()
        .filter(|step| {
            step.get("type").and_then(Value::as_str) == Some("recipe")
                && step.get("recipe").and_then(Value::as_str) == Some("investigation-workflow")
        })
        .collect()
}

#[test]
fn smart_execute_routing_passes_investigation_question_to_investigation_workflow() {
    let routing = load_yaml(&workspace_recipes_dir().join("smart-execute-routing.yaml"));
    let invocations = investigation_invocations(&routing);
    assert!(
        !invocations.is_empty(),
        "smart-execute-routing.yaml must invoke investigation-workflow at least once"
    );

    for (idx, step) in invocations.iter().enumerate() {
        let step_id = step
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let context = step.get("context").unwrap_or_else(|| {
            panic!(
                "issue #507: investigation-workflow invocation '{step_id}' (index {idx}) \
                 must declare an explicit `context:` block that forwards \
                 investigation_question — got step without context: {step:?}"
            )
        });
        let context_map = context.as_mapping().unwrap_or_else(|| {
            panic!("step '{step_id}': `context:` must be a mapping, got {context:?}")
        });

        let investigation_question = context_map
            .get(Value::String("investigation_question".to_string()))
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "issue #507: step '{step_id}' must set \
                     `investigation_question: \"{{{{task_description}}}}\"` so the \
                     nested recipe receives the question; got context={context_map:?}"
                )
            });
        assert!(
            investigation_question.contains("{{task_description}}")
                || investigation_question.contains("{{ task_description }}"),
            "issue #507: step '{step_id}' must derive investigation_question from \
             task_description (template `{{{{task_description}}}}`); got value {investigation_question:?}"
        );

        // task_description must remain populated for context_validation —
        // either explicitly forwarded or inherited via parent merging.
        // If it's explicitly set we assert it's also `{{task_description}}`.
        if let Some(task_desc) = context_map
            .get(Value::String("task_description".to_string()))
            .and_then(Value::as_str)
        {
            assert!(
                task_desc.contains("{{task_description}}")
                    || task_desc.contains("{{ task_description }}"),
                "step '{step_id}': if task_description is explicitly forwarded it must \
                 reference the parent context, not be hardcoded; got {task_desc:?}"
            );
        }
    }
}

#[test]
fn investigation_prep_keeps_normalize_question_as_defense_in_depth() {
    // Decision 3 explicitly preserves the `normalize-question` step as a
    // belt-and-braces fallback — neither change is load-bearing on the
    // other. If a future refactor removes it, that change must come with
    // an updated test (and a separate audit of every caller of
    // investigation-prep), so this guardrail is intentional.
    let prep = load_yaml(&workspace_recipes_dir().join("investigation-prep.yaml"));
    let has_normalize = steps(&prep).iter().any(|step| {
        step.get("id").and_then(Value::as_str) == Some("normalize-question")
            && step.get("output").and_then(Value::as_str) == Some("investigation_question")
    });
    assert!(
        has_normalize,
        "issue #507 (Decision 3): investigation-prep.yaml must keep the \
         `normalize-question` step (output: investigation_question) as a \
         defense-in-depth fallback for callers that don't pass \
         investigation_question explicitly"
    );
}
