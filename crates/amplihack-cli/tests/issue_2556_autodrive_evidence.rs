//! #2556: the Crusty loop runs for an open PR whose Crusty round is not
//! complete at the current head.
//!
//! The preflight used to set `should_run` to `"false"` for a worktree that was
//! merely behind the PR head. #2556 also made merge-ready `assess` and `round`
//! tolerate `null` evidence; #2575 retired that parsed record, so only the
//! Crusty preflight contract is tested here. No agent, hosting service, timer
//! or sleep is used.

use recipe_runner_rs::context::RecipeContext;
use serde_json::{Value, json};
use serde_yaml::Value as Yaml;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn recipe_text(name: &str) -> String {
    std::fs::read_to_string(
        root()
            .join("amplifier-bundle/recipes")
            .join(format!("{name}.yaml")),
    )
    .unwrap()
}

fn recipe(name: &str) -> Yaml {
    let text = recipe_text(name);
    recipe_runner_rs::parser::RecipeParser::new()
        .parse(&text)
        .unwrap_or_else(|e| panic!("{name} must parse with the recipe runner: {e:?}"));
    serde_yaml::from_str(&text).unwrap()
}

fn steps(doc: &Yaml) -> Vec<Yaml> {
    doc["steps"].as_sequence().unwrap().clone()
}

fn step<'a>(steps: &'a [Yaml], id: &str) -> &'a Yaml {
    steps
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing step {id}"))
}

fn text_of(s: &Yaml, key: &str) -> String {
    s[key].as_str().unwrap_or("").to_string()
}

#[test]
fn crusty_loop_runs_for_an_open_pr_with_an_incomplete_round() {
    let all = steps(&recipe("autodrive-crusty-loop"));
    for id in ["step-02-crusty-loop", "step-03-record-crusty-phase"] {
        assert_eq!(
            text_of(step(&all, id), "condition"),
            "crusty_loop_preflight.should_run == 'true'",
            "{id} keeps its condition"
        );
    }
    let condition = text_of(step(&all, "step-02-crusty-loop"), "condition");
    let decide = |preflight: Value| {
        RecipeContext::new(HashMap::from([("crusty_loop_preflight".into(), preflight)]))
            .evaluate(&condition)
            .unwrap()
    };
    // Open PR, worktree repaired to the PR head, no complete round there.
    assert!(decide(
        json!({"should_run":"true","skip_reason":"","pr_state":"OPEN"})
    ));
    for reason in ["merged", "complete-at-head", "worktree-diverged"] {
        assert!(
            !decide(json!({"should_run":"false","skip_reason":reason})),
            "{reason}"
        );
    }
}
