//! Issue #1619 (TDD): the bundled recipes declare a model tier per agent step.
//!
//! These are pure structural assertions on the shipped YAML. They need no
//! recipe runner, no network and no harness, and they are the contract the
//! recipe edits of this issue are held to:
//!
//! - every agent step in the tiered set declares `model:` with one of the
//!   three words `fast`, `standard`, `strong` (requirement F1);
//! - no bundled recipe anywhere names a concrete model id in `model:`;
//! - the tier assignment rule of the issue's 16 November comment is applied:
//!   design, implementation and debugging stay on `strong`, the mechanical
//!   steps named in the "first cut" go on `fast`;
//! - the recipes that were deliberately left untiered still carry no
//!   `model:` at all, so their behaviour is unchanged;
//! - the brick budget is still met, which is requirement F12: room is made
//!   by extracting bash bodies, never by raising the limit;
//! - the small-diff combined review of `workflow-refactor-review.yaml` is
//!   wired as requirement F9 describes, and is off by default.
//!
//! The documented contract these assert is `docs/reference/model-tiers.md`.
//!
//! A step is an agent step when it has an `agent:` key and its `type:` is
//! either absent or `agent` — the bundled recipes use both spellings.
//! `step-01-prepare-workspace` is deliberately NOT asserted here: it is a
//! `bash` step, and a bash step takes no model.

use serde_yaml::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The only three tier words. There is no alias and no fourth word.
const TIER_WORDS: [&str; 3] = ["fast", "standard", "strong"];

/// The brick budget, unchanged by this issue (requirement F12).
const BRICK_LIMIT: usize = 400;

/// Every bundled recipe reachable from `default-workflow`,
/// `investigation-workflow`, `smart-orchestrator` or the auto-drive family
/// that contains at least one agent step. Every agent step in each of these
/// declares a tier.
const TIERED_RECIPES: &[&str] = &[
    "auto-drive-light",
    "auto-drive-to-merge",
    "autodrive-build",
    "autodrive-crusty-loop",
    "autodrive-crusty-round",
    "autodrive-finalize",
    "autodrive-handoff",
    "autodrive-merge-evidence",
    "autodrive-merge-loop",
    "autodrive-merge-round",
    "investigation-explore",
    "investigation-prep",
    "investigation-report",
    "investigation-verify",
    "push-failure-recovery",
    "quick-change",
    "smart-classify-route",
    "smart-execute-routing",
    "smart-reflect-loop",
    "smart-validate-summarize",
    "workflow-design",
    "workflow-finalize",
    "workflow-pr-review",
    "workflow-precommit-test",
    "workflow-prep",
    "workflow-publish",
    "workflow-refactor-review",
    "workflow-tdd",
    "workflow-terminal-adjudication",
];

/// Recipes with agent steps that this issue deliberately leaves alone. They
/// keep today's behaviour exactly: no `model:` field anywhere, so amplihack's
/// existing default chain decides as it always did.
const UNTIERED_RECIPES: &[&str] = &[
    "oxidizer-workflow",
    "n-version-workflow",
    "debate-workflow",
    "code-atlas",
    "cascade-workflow",
    "auto-workflow",
    "quality-audit-cycle",
    "consensus-workflow",
    "consensus-design",
    "consensus-tdd-impl",
    "consensus-pr-review",
];

fn recipes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .join("amplifier-bundle/recipes")
}

fn recipe_path(name: &str) -> PathBuf {
    recipes_dir().join(format!("{name}.yaml"))
}

fn read(name: &str) -> String {
    let path = recipe_path(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn parse(name: &str) -> Value {
    serde_yaml::from_str(&read(name))
        .unwrap_or_else(|e| panic!("parse {}: {e}", recipe_path(name).display()))
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .map(|s| s.as_slice())
        .unwrap_or(&[])
}

fn step_id(step: &Value) -> &str {
    step.get("id").and_then(Value::as_str).unwrap_or("<no id>")
}

/// An agent step: an `agent:` key, and a `type:` that is absent or `agent`.
fn is_agent_step(step: &Value) -> bool {
    step.get("agent").is_some()
        && matches!(
            step.get("type").and_then(Value::as_str),
            None | Some("agent")
        )
}

fn declared_model(step: &Value) -> Option<&str> {
    step.get("model").and_then(Value::as_str)
}

/// Every `(recipe, step id, model)` of a tiered recipe's agent steps.
fn tiered_agent_steps() -> Vec<(&'static str, String, Option<String>)> {
    TIERED_RECIPES
        .iter()
        .flat_map(|&name| {
            let recipe = parse(name);
            steps(&recipe)
                .iter()
                .filter(|s| is_agent_step(s))
                .map(|s| {
                    (
                        name,
                        step_id(s).to_string(),
                        declared_model(s).map(str::to_string),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The tier a named step must declare, from the rule in the issue's
/// 16 November comment and in `docs/reference/model-tiers.md`.
///
/// Spot checks, not the whole inventory: a full inventory would have to be
/// rewritten every time a step is added, and the blanket assertions above
/// already cover every agent step.
const REQUIRED_TIERS: &[(&str, &str, &str)] = &[
    // strong — design, implementation, debugging, recovery, planning
    ("workflow-design", "step-05-architecture", "strong"),
    ("workflow-tdd", "step-08-implement", "strong"),
    ("workflow-tdd", "step-08b-integration", "strong"),
    ("workflow-refactor-review", "step-09-refactor", "strong"),
    (
        "workflow-refactor-review",
        "step-11b-implement-feedback",
        "strong",
    ),
    (
        "workflow-pr-review",
        "step-18b-implement-feedback",
        "strong",
    ),
    ("smart-classify-route", "classify-and-decompose", "strong"),
    ("push-failure-recovery", "push_recovery", "strong"),
    // standard — reviews, documentation, PR body writing, test authoring
    ("workflow-tdd", "step-07-write-tests", "standard"),
    ("workflow-design", "step-06-documentation", "standard"),
    (
        "workflow-refactor-review",
        "step-10-pre-commit-review",
        "standard",
    ),
    (
        "workflow-refactor-review",
        "step-10b-security-review",
        "standard",
    ),
    (
        "workflow-refactor-review",
        "step-10c-philosophy-check",
        "standard",
    ),
    ("workflow-pr-review", "step-17c-security-review", "standard"),
    // fast — mechanical steps named in the issue's "first cut"
    ("workflow-finalize", "step-20-final-cleanup", "fast"),
    ("auto-drive-to-merge", "autodrive-prerequisites", "fast"),
    ("auto-drive-to-merge", "autodrive-check-unmerged", "fast"),
    ("auto-drive-light", "light-check-unmerged", "fast"),
    ("smart-validate-summarize", "summarize", "fast"),
];

// -------------------------------------------------------------------------
// F1 — every agent step of the tiered set declares one of the three words
// -------------------------------------------------------------------------

#[test]
fn every_tiered_agent_step_declares_a_tier() {
    let mut missing = Vec::new();
    for (recipe, id, model) in tiered_agent_steps() {
        match model {
            None => missing.push(format!("{recipe}: {id} declares no model")),
            Some(value) if !TIER_WORDS.contains(&value.as_str()) => {
                missing.push(format!("{recipe}: {id} declares `{value}`"))
            }
            Some(_) => {}
        }
    }
    assert!(
        missing.is_empty(),
        "every agent step in the tiered recipes must declare model: fast | standard | strong \
         (issue #1619, requirement F1). Offending steps:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn the_tiered_set_actually_has_agent_steps() {
    // Guards the assertion above against passing vacuously if a recipe is
    // renamed and the filter silently finds nothing.
    for name in TIERED_RECIPES {
        let recipe = parse(name);
        assert!(
            steps(&recipe).iter().any(is_agent_step),
            "{name}.yaml is in TIERED_RECIPES but has no agent step; \
             update the list in the same change that moved the steps"
        );
    }
}

// -------------------------------------------------------------------------
// F1 — no bundled recipe names a concrete model
// -------------------------------------------------------------------------

#[test]
fn no_bundled_recipe_names_a_concrete_model() {
    let mut offenders = Vec::new();
    let dir = recipes_dir();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".yaml").map(str::to_string)
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no recipes found in {}", dir.display());

    for name in &names {
        let Ok(recipe) = serde_yaml::from_str::<Value>(&read(name)) else {
            continue; // parse failures are another test's business
        };
        for step in steps(&recipe) {
            if let Some(model) = declared_model(step)
                && !TIER_WORDS.contains(&model)
            {
                offenders.push(format!("{name}: {}: `{model}`", step_id(step)));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a bundled recipe step must declare a tier word, never a concrete model id: \
         a concrete id in a shipped recipe cannot be right for Claude Code, Codex and \
         Copilot CLI at once (issue #1619). Offending steps:\n  {}",
        offenders.join("\n  ")
    );
}

// -------------------------------------------------------------------------
// The tier assignment rule
// -------------------------------------------------------------------------

#[test]
fn named_steps_carry_the_tier_the_rule_gives_them() {
    for (recipe_name, id, expected) in REQUIRED_TIERS {
        let recipe = parse(recipe_name);
        let step = steps(&recipe)
            .iter()
            .find(|s| step_id(s) == *id)
            .unwrap_or_else(|| panic!("no step `{id}` in {recipe_name}.yaml"));
        assert!(
            is_agent_step(step),
            "{recipe_name}: {id} is not an agent step, so it takes no model tier"
        );
        assert_eq!(
            declared_model(step),
            Some(*expected),
            "{recipe_name}: {id} must be on the `{expected}` tier \
             (issue #1619, 16 November comment)"
        );
    }
}

#[test]
fn implementation_design_and_debugging_stay_strong() {
    // The owner's condition, stated twice in the issue: the saving comes from
    // the mechanical and review steps, never from the steps that build.
    for (recipe_name, id, expected) in REQUIRED_TIERS {
        if *expected != "strong" {
            continue;
        }
        let recipe = parse(recipe_name);
        let step = steps(&recipe)
            .iter()
            .find(|s| step_id(s) == *id)
            .unwrap_or_else(|| panic!("no step `{id}` in {recipe_name}.yaml"));
        assert_eq!(
            declared_model(step),
            Some("strong"),
            "{recipe_name}: {id} builds or designs and must stay on `strong`"
        );
    }
}

// -------------------------------------------------------------------------
// The recipes deliberately left alone
// -------------------------------------------------------------------------

#[test]
fn untiered_recipes_declare_no_model_at_all() {
    for name in UNTIERED_RECIPES {
        let recipe = parse(name);
        let declared: Vec<String> = steps(&recipe)
            .iter()
            .filter(|s| declared_model(s).is_some())
            .map(|s| step_id(s).to_string())
            .collect();
        assert!(
            declared.is_empty(),
            "{name}.yaml is outside the tiered set and must keep today's behaviour \
             exactly — no model: field. Steps that now declare one: {declared:?}. \
             Tiering it is a separate, deliberate edit to that recipe."
        );
    }
}

#[test]
fn the_tiered_and_untiered_sets_do_not_overlap() {
    let tiered: BTreeSet<&str> = TIERED_RECIPES.iter().copied().collect();
    let untiered: BTreeSet<&str> = UNTIERED_RECIPES.iter().copied().collect();
    let both: Vec<&&str> = tiered.intersection(&untiered).collect();
    assert!(
        both.is_empty(),
        "listed as both tiered and untiered: {both:?}"
    );
}

// -------------------------------------------------------------------------
// F12 — the brick budget is met by extraction, not by raising the limit
// -------------------------------------------------------------------------

/// Recipes that were already over the brick budget before this issue. They
/// are pre-existing debt, not something adding a `model:` line created, and
/// paying it down is a separate change. Listed here so the guard below stays
/// a regression guard rather than a demand to refactor an unrelated recipe.
const ALREADY_OVER_BUDGET: &[&str] = &["auto-drive-light", "push-failure-recovery"];

#[test]
fn adding_a_tier_line_does_not_push_a_recipe_over_the_brick_budget() {
    let mut over = Vec::new();
    for name in TIERED_RECIPES {
        if ALREADY_OVER_BUDGET.contains(name) {
            continue;
        }
        let lines = read(name).lines().count();
        if lines > BRICK_LIMIT {
            over.push(format!("{name}.yaml: {lines} lines"));
        }
    }
    assert!(
        over.is_empty(),
        "adding one `model:` line per agent step must not push a recipe past the \
         {BRICK_LIMIT}-line brick budget. Make room by extracting a bash step body into \
         amplifier-bundle/tools/ and calling out to it, in the same commit as the tier \
         lines — never by raising the limit and never by reflowing YAML to compress it \
         (issue #1619, requirement F12). Over budget:\n  {}",
        over.join("\n  ")
    );
}

#[test]
fn the_already_over_budget_list_names_only_recipes_that_are_really_over() {
    // So the carve-out cannot quietly become a place to hide a new overrun.
    for name in ALREADY_OVER_BUDGET {
        let lines = read(name).lines().count();
        assert!(
            lines > BRICK_LIMIT,
            "{name}.yaml is {lines} lines, within the budget — remove it from \
             ALREADY_OVER_BUDGET so the guard covers it again"
        );
    }
}

// -------------------------------------------------------------------------
// F9 — combined review for a small diff, off by default
// -------------------------------------------------------------------------

/// The step inventory of `workflow-refactor-review.yaml` after the combined
/// review lands: the three separate passes stay, and one combined pass and
/// one bash step that decides between them are added.
const REFACTOR_REVIEW_INVENTORY: &[&str] = &[
    "step-09-refactor",
    "step-09b-optimize",
    "review-mode",
    "step-10-pre-commit-review",
    "step-10b-security-review",
    "step-10c-philosophy-check",
    "step-10d-combined-review",
    "step-11-incorporate-feedback",
    "step-11b-implement-feedback",
    "checkpoint-after-review-feedback",
];

#[test]
fn refactor_review_inventory_is_pinned() {
    let recipe = parse("workflow-refactor-review");
    let actual: Vec<&str> = steps(&recipe).iter().map(step_id).collect();
    assert_eq!(
        actual, REFACTOR_REVIEW_INVENTORY,
        "the combined-review change adds exactly the mode step and the combined pass, \
         in order, and removes nothing (issue #1619, requirement F9)"
    );
}

#[test]
fn the_three_passes_stand_down_only_for_a_combined_review() {
    let recipe = parse("workflow-refactor-review");
    for id in [
        "step-10-pre-commit-review",
        "step-10b-security-review",
        "step-10c-philosophy-check",
    ] {
        let step = steps(&recipe)
            .iter()
            .find(|s| step_id(s) == id)
            .unwrap_or_else(|| panic!("no step `{id}`"));
        let condition = step
            .get("condition")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{id} must keep a condition"));
        assert!(
            condition.contains("goal_already_met != 'true'"),
            "{id} must keep its existing condition; the combined review adds a clause, \
             it does not replace one. Found: {condition}"
        );
        assert!(
            condition.contains("review_combined") && condition.contains("!= 'true'"),
            "{id} must also stand down when the combined pass runs. Found: {condition}"
        );
    }
}

#[test]
fn the_combined_pass_runs_only_when_the_mode_step_chose_it() {
    let recipe = parse("workflow-refactor-review");
    let combined = steps(&recipe)
        .iter()
        .find(|s| step_id(s) == "step-10d-combined-review")
        .expect("no step-10d-combined-review");
    assert!(
        is_agent_step(combined),
        "the combined review is one agent step covering code, security and philosophy"
    );
    assert_eq!(
        declared_model(combined),
        Some("standard"),
        "the combined review is a review, so it runs on the `standard` tier"
    );
    let condition = combined
        .get("condition")
        .and_then(Value::as_str)
        .expect("the combined pass must carry the inverse condition");
    assert!(
        condition.contains("review_combined") && condition.contains("== 'true'"),
        "the combined pass runs only when the mode step chose it. Found: {condition}"
    );
    let prompt = combined
        .get("prompt")
        .and_then(Value::as_str)
        .expect("the combined pass must have a prompt");
    for topic in ["security", "philosophy"] {
        assert!(
            prompt.to_lowercase().contains(topic),
            "the combined pass must still cover {topic}: collapsing three passes into \
             one must not drop what they looked for"
        );
    }
}

#[test]
fn the_mode_step_reads_both_settings_and_defaults_to_separate_passes() {
    let recipe = parse("workflow-refactor-review");
    let mode = steps(&recipe)
        .iter()
        .find(|s| step_id(s) == "review-mode")
        .expect("no review-mode step");
    assert_eq!(
        mode.get("type").and_then(Value::as_str),
        Some("bash"),
        "deciding the review mode is a computation, not an agent step"
    );
    let command = mode
        .get("command")
        .and_then(Value::as_str)
        .expect("review-mode must have a command");
    for key in [
        "review.combined_small_diff",
        "review.combined_diff_threshold",
    ] {
        assert!(
            command.contains(key),
            "review-mode must read {key} through `amplihack config get`, so one documented \
             setting drives the behaviour. Command:\n{command}"
        );
    }
    assert!(
        mode.get("output").and_then(Value::as_str) == Some("review_combined"),
        "review-mode must publish `review_combined`, which is the variable the four \
         review steps condition on"
    );
}
