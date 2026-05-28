//! code-philosophy-audit recipe structural tests.
//!
//! Validates the `code-philosophy-audit.yaml` recipe that orchestrates
//! code-smell-detector, philosophy-compliance-workflow, and the 3-pass
//! structured audit into a 5-layer pipeline.
//!
//! Contracts tested:
//!   - Recipe parses as valid YAML with correct metadata
//!   - Exactly 5 steps in the expected order (layer-1 through layer-5)
//!   - All steps use the `amplihack:core:reviewer` agent (read-only)
//!   - Output variables are correctly assigned per step
//!   - Required context variables are defined
//!   - Recursion guards (max_depth, max_total_steps) are set
//!   - Layer 4 and Layer 5 have condition gates
//!   - Brick rule: recipe is ≤400 physical lines
//!   - No duplicate step IDs
//!   - Manifest registration present
//!   - SKILL.md has recipe: frontmatter field
//!   - SKILL.md has mermaid diagram
//!   - reference.md documents recipe architecture
//!
//! These tests are pure structural assertions on YAML/JSON/Markdown content.
//! They are fast and have no external dependencies.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const BRICK_LIMIT: usize = 400;

#[derive(Debug, Deserialize)]
struct Recipe {
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    context: HashMap<String, serde_yaml::Value>,
    #[serde(default)]
    recursion: Option<RecursionConfig>,
    #[serde(default)]
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct RecursionConfig {
    #[serde(default)]
    max_depth: Option<u32>,
    #[serde(default)]
    max_total_steps: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    condition: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

fn bundle_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("amplifier-bundle")
}

fn recipes_dir() -> PathBuf {
    bundle_dir().join("recipes")
}

fn skills_dir() -> PathBuf {
    bundle_dir().join("skills")
}

fn recipe_path() -> PathBuf {
    recipes_dir().join("code-philosophy-audit.yaml")
}

fn recipe_text() -> String {
    let path = recipe_path();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_recipe() -> Recipe {
    let path = recipe_path();
    let text = recipe_text();
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn skill_md() -> String {
    let path = skills_dir().join("code-philosophy").join("SKILL.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn reference_md() -> String {
    let path = skills_dir().join("code-philosophy").join("reference.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn manifest() -> HashMap<String, String> {
    let path = recipes_dir().join("_recipe_manifest.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse manifest: {e}"))
}

// ─── Recipe structural tests ────────────────────────────────────────────────

const EXPECTED_STEPS: &[&str] = &[
    "layer-1-anti-patterns",
    "layer-2-architecture",
    "layer-3-three-pass-audit",
    "layer-4-consolidation",
    "layer-5-reassessment",
];

const EXPECTED_OUTPUTS: &[(&str, &str)] = &[
    ("layer-1-anti-patterns", "layer_1_findings"),
    ("layer-2-architecture", "layer_2_findings"),
    ("layer-3-three-pass-audit", "layer_3_findings"),
    ("layer-4-consolidation", "consolidation_report"),
    ("layer-5-reassessment", "reassessment_report"),
];

const REQUIRED_CONTEXT_VARS: &[&str] = &[
    "repo_path",
    "target_path",
    "task_description",
    "layer_1_findings",
    "layer_2_findings",
    "layer_3_findings",
    "consolidation_report",
    "fix_results",
    "reassessment_report",
];

#[test]
fn recipe_parses_with_correct_name_and_version() {
    let recipe = load_recipe();
    assert_eq!(recipe.name, "code-philosophy-audit");
    assert!(recipe.version.is_some(), "recipe must have a version field");
    assert!(
        recipe.description.is_some(),
        "recipe must have a description field"
    );
}

#[test]
fn recipe_has_exactly_five_steps_in_order() {
    let recipe = load_recipe();
    assert_eq!(
        recipe.steps.len(),
        5,
        "recipe must have exactly 5 steps (layers); found {}",
        recipe.steps.len()
    );
    let actual_ids: Vec<&str> = recipe.steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        actual_ids, EXPECTED_STEPS,
        "step IDs must match expected layer order"
    );
}

#[test]
fn no_duplicate_step_ids() {
    let recipe = load_recipe();
    let mut seen = HashSet::new();
    let mut dups = Vec::new();
    for step in &recipe.steps {
        if !seen.insert(&step.id) {
            dups.push(step.id.clone());
        }
    }
    assert!(dups.is_empty(), "duplicate step IDs: {dups:?}");
}

#[test]
fn all_steps_use_reviewer_agent() {
    let recipe = load_recipe();
    for step in &recipe.steps {
        let agent = step.agent.as_deref().unwrap_or("");
        assert!(
            agent.contains("reviewer"),
            "step '{}' must use reviewer agent for read-only enforcement; got '{}'",
            step.id,
            agent
        );
        assert!(
            !agent.contains("builder"),
            "step '{}' must NOT use builder agent; got '{}'",
            step.id,
            agent
        );
    }
}

#[test]
fn output_variables_assigned_correctly() {
    let recipe = load_recipe();
    for (step_id, expected_output) in EXPECTED_OUTPUTS {
        let step = recipe
            .steps
            .iter()
            .find(|s| s.id == *step_id)
            .unwrap_or_else(|| panic!("step '{step_id}' not found"));
        assert_eq!(
            step.output.as_deref(),
            Some(*expected_output),
            "step '{step_id}' output must be '{expected_output}'"
        );
    }
}

#[test]
fn required_context_variables_defined() {
    let recipe = load_recipe();
    let missing: Vec<&&str> = REQUIRED_CONTEXT_VARS
        .iter()
        .filter(|v| !recipe.context.contains_key(**v))
        .collect();
    assert!(
        missing.is_empty(),
        "missing required context variables: {missing:?}"
    );
}

#[test]
fn no_audit_mode_context_variable() {
    let recipe = load_recipe();
    assert!(
        !recipe.context.contains_key("audit_mode"),
        "audit_mode context variable should not exist (removed as future-proofing)"
    );
}

#[test]
fn recursion_guards_set() {
    let recipe = load_recipe();
    let recursion = recipe
        .recursion
        .as_ref()
        .expect("recipe must have a recursion block");
    assert!(
        recursion.max_depth.is_some(),
        "recursion.max_depth must be set"
    );
    assert!(
        recursion.max_total_steps.is_some(),
        "recursion.max_total_steps must be set"
    );
    let max_depth = recursion.max_depth.unwrap();
    assert!(
        max_depth <= 10,
        "max_depth should be reasonable (≤10); got {max_depth}"
    );
}

#[test]
fn layer_4_has_condition_on_findings() {
    let recipe = load_recipe();
    let l4 = recipe
        .steps
        .iter()
        .find(|s| s.id == "layer-4-consolidation")
        .expect("layer-4-consolidation step must exist");
    let condition = l4
        .condition
        .as_deref()
        .expect("layer-4-consolidation must have a condition");
    let has_finding_ref = condition.contains("layer_1_findings")
        || condition.contains("layer_2_findings")
        || condition.contains("layer_3_findings");
    assert!(
        has_finding_ref,
        "layer-4 condition must reference at least one layer findings variable; got: {condition}"
    );
}

#[test]
fn layer_5_conditional_on_fix_results() {
    let recipe = load_recipe();
    let l5 = recipe
        .steps
        .iter()
        .find(|s| s.id == "layer-5-reassessment")
        .expect("layer-5-reassessment step must exist");
    let condition = l5
        .condition
        .as_deref()
        .expect("layer-5-reassessment must have a condition");
    assert!(
        condition.contains("fix_results"),
        "layer-5 condition must reference fix_results; got: {condition}"
    );
}

#[test]
fn recipe_under_brick_limit() {
    let lines = recipe_text().lines().count();
    assert!(
        lines <= BRICK_LIMIT,
        "recipe is {lines} lines — exceeds {BRICK_LIMIT} LOC brick limit"
    );
    assert!(
        lines >= 100,
        "recipe is only {lines} lines — likely incomplete for 5-layer pipeline"
    );
}

#[test]
fn layer_1_prompt_references_code_smell_detector() {
    let recipe = load_recipe();
    let l1 = recipe
        .steps
        .iter()
        .find(|s| s.id == "layer-1-anti-patterns")
        .expect("layer-1 step must exist");
    let prompt = l1.prompt.as_deref().unwrap_or("");
    assert!(
        prompt.contains("code-smell-detector"),
        "layer-1 prompt must reference code-smell-detector skill"
    );
}

#[test]
fn layer_2_prompt_references_philosophy_compliance_workflow() {
    let recipe = load_recipe();
    let l2 = recipe
        .steps
        .iter()
        .find(|s| s.id == "layer-2-architecture")
        .expect("layer-2 step must exist");
    let prompt = l2.prompt.as_deref().unwrap_or("");
    assert!(
        prompt.contains("philosophy-compliance-workflow"),
        "layer-2 prompt must reference philosophy-compliance-workflow skill"
    );
}

#[test]
fn layer_2_receives_layer_1_findings() {
    let recipe = load_recipe();
    let l2 = recipe
        .steps
        .iter()
        .find(|s| s.id == "layer-2-architecture")
        .expect("layer-2 step must exist");
    let prompt = l2.prompt.as_deref().unwrap_or("");
    assert!(
        prompt.contains("layer_1_findings"),
        "layer-2 prompt must reference layer_1_findings for deduplication"
    );
}

#[test]
fn layer_3_receives_both_prior_layer_findings() {
    let recipe = load_recipe();
    let l3 = recipe
        .steps
        .iter()
        .find(|s| s.id == "layer-3-three-pass-audit")
        .expect("layer-3 step must exist");
    let prompt = l3.prompt.as_deref().unwrap_or("");
    assert!(
        prompt.contains("layer_1_findings"),
        "layer-3 prompt must reference layer_1_findings"
    );
    assert!(
        prompt.contains("layer_2_findings"),
        "layer-3 prompt must reference layer_2_findings"
    );
}

#[test]
fn layer_3_prompt_includes_all_three_passes() {
    let recipe = load_recipe();
    let l3 = recipe
        .steps
        .iter()
        .find(|s| s.id == "layer-3-three-pass-audit")
        .expect("layer-3 step must exist");
    let prompt = l3.prompt.as_deref().unwrap_or("").to_lowercase();
    assert!(
        prompt.contains("brick rule") || prompt.contains("pass 1"),
        "layer-3 prompt must reference Pass 1 / BRICK RULE"
    );
    assert!(
        prompt.contains("quality invariant") || prompt.contains("pass 2"),
        "layer-3 prompt must reference Pass 2 / QUALITY INVARIANTS"
    );
    assert!(
        prompt.contains("philosophy spirit") || prompt.contains("pass 3"),
        "layer-3 prompt must reference Pass 3 / PHILOSOPHY SPIRIT"
    );
}

#[test]
fn layer_4_prompt_references_all_three_layer_findings() {
    let recipe = load_recipe();
    let l4 = recipe
        .steps
        .iter()
        .find(|s| s.id == "layer-4-consolidation")
        .expect("layer-4 step must exist");
    let prompt = l4.prompt.as_deref().unwrap_or("");
    for var in &["layer_1_findings", "layer_2_findings", "layer_3_findings"] {
        assert!(
            prompt.contains(var),
            "layer-4 prompt must reference {var} for consolidation"
        );
    }
}

#[test]
fn layer_4_prompt_includes_dedup_rules() {
    let recipe = load_recipe();
    let l4 = recipe
        .steps
        .iter()
        .find(|s| s.id == "layer-4-consolidation")
        .expect("layer-4 step must exist");
    let prompt = l4.prompt.as_deref().unwrap_or("").to_lowercase();
    assert!(
        prompt.contains("dedup") || prompt.contains("de-dup") || prompt.contains("deduplic"),
        "layer-4 prompt must include deduplication rules"
    );
    assert!(
        prompt.contains("severity"),
        "layer-4 prompt must reference severity for resolution"
    );
    assert!(
        prompt.contains("verdict"),
        "layer-4 prompt must specify verdict rules"
    );
}

#[test]
fn layer_5_prompt_limits_reassessment() {
    let recipe = load_recipe();
    let l5 = recipe
        .steps
        .iter()
        .find(|s| s.id == "layer-5-reassessment")
        .expect("layer-5 step must exist");
    let prompt = l5.prompt.as_deref().unwrap_or("").to_lowercase();
    assert!(
        prompt.contains("changed file") || prompt.contains("only"),
        "layer-5 must scope re-assessment to changed files"
    );
    assert!(
        prompt.contains("not trigger another")
            || prompt.contains("no recurs")
            || prompt.contains("max 1")
            || prompt.contains("final layer"),
        "layer-5 must explicitly limit re-assessment passes"
    );
}

#[test]
fn finding_id_patterns_present_in_recipe() {
    let text = recipe_text();
    for pattern in &["L1-", "L2-", "L3-", "C-", "R-"] {
        assert!(
            text.contains(pattern),
            "recipe must include finding ID pattern '{pattern}'"
        );
    }
}

// ─── Manifest tests ─────────────────────────────────────────────────────────

#[test]
fn recipe_registered_in_manifest() {
    let m = manifest();
    assert!(
        m.contains_key("code-philosophy-audit"),
        "code-philosophy-audit must be registered in _recipe_manifest.json"
    );
    let val = &m["code-philosophy-audit"];
    assert!(
        val.len() >= 8,
        "manifest value should be a meaningful hash, got: {val}"
    );
}

// ─── SKILL.md tests ─────────────────────────────────────────────────────────

#[test]
fn skill_md_has_recipe_frontmatter() {
    let text = skill_md();
    assert!(
        text.contains("recipe: code-philosophy-audit"),
        "SKILL.md must have recipe: code-philosophy-audit in frontmatter"
    );
}

#[test]
fn skill_md_has_mermaid_diagram() {
    let text = skill_md();
    assert!(
        text.contains("```mermaid"),
        "SKILL.md must contain a mermaid architecture diagram"
    );
    let lower = text.to_lowercase();
    for label in &["layer 1", "layer 2", "layer 3", "layer 4", "layer 5"] {
        assert!(
            lower.contains(label),
            "SKILL.md mermaid diagram must reference '{label}'"
        );
    }
}

#[test]
fn skill_md_references_recipe() {
    let text = skill_md();
    assert!(
        text.contains("code-philosophy-audit"),
        "SKILL.md must reference the code-philosophy-audit recipe"
    );
    assert!(
        text.contains("amplihack recipe run code-philosophy-audit"),
        "SKILL.md must include recipe run command example"
    );
}

#[test]
fn skill_md_references_composed_skills() {
    let text = skill_md();
    assert!(
        text.contains("code-smell-detector"),
        "SKILL.md must reference code-smell-detector"
    );
    assert!(
        text.contains("philosophy-compliance-workflow"),
        "SKILL.md must reference philosophy-compliance-workflow"
    );
}

#[test]
fn skill_md_documents_read_only_enforcement() {
    let lower = skill_md().to_lowercase();
    assert!(
        lower.contains("read-only") || lower.contains("read only"),
        "SKILL.md must document read-only enforcement"
    );
    assert!(
        lower.contains("reviewer"),
        "SKILL.md must reference reviewer agent"
    );
}

#[test]
fn skill_md_under_reasonable_size() {
    let lines = skill_md().lines().count();
    assert!(
        lines >= 150,
        "SKILL.md has only {lines} lines — likely incomplete"
    );
    assert!(
        lines <= 500,
        "SKILL.md has {lines} lines — too large for skill definition"
    );
}

// ─── reference.md tests ─────────────────────────────────────────────────────

#[test]
fn reference_md_documents_recipe_architecture() {
    let lower = reference_md().to_lowercase();
    assert!(
        lower.contains("recipe architecture"),
        "reference.md must have a Recipe Architecture section"
    );
    assert!(
        lower.contains("layer interaction") || lower.contains("findings passed forward"),
        "reference.md must document layer interactions"
    );
}

#[test]
fn reference_md_documents_dedup_logic() {
    let lower = reference_md().to_lowercase();
    assert!(
        lower.contains("dedup") || lower.contains("de-dup") || lower.contains("deduplic"),
        "reference.md must document deduplication logic"
    );
    // Must have an overlap mapping table
    assert!(
        lower.contains("over-abstraction") && lower.contains("large-function"),
        "reference.md must document known category overlaps"
    );
}

#[test]
fn reference_md_documents_individual_vs_full_audit() {
    let text = reference_md();
    assert!(
        text.contains("Individual") || text.contains("individual") || text.contains("Layer 3 only"),
        "reference.md must document how to run individual layers"
    );
    assert!(
        text.contains("Full audit") || text.contains("full audit"),
        "reference.md must document how to run the full audit"
    );
}

#[test]
fn reference_md_documents_context_variables() {
    let text = reference_md();
    for var in &["repo_path", "target_path", "fix_results"] {
        assert!(
            text.contains(var),
            "reference.md must document context variable '{var}'"
        );
    }
}

// ─── Composed skills not modified ───────────────────────────────────────────

#[test]
fn code_smell_detector_not_modified() {
    let path = skills_dir().join("code-smell-detector").join("SKILL.md");
    if path.exists() {
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("code-philosophy-audit"),
            "code-smell-detector SKILL.md should NOT reference code-philosophy-audit (compose, don't modify)"
        );
    }
}

#[test]
fn philosophy_compliance_workflow_not_modified() {
    let path = skills_dir()
        .join("philosophy-compliance-workflow")
        .join("SKILL.md");
    if path.exists() {
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("code-philosophy-audit"),
            "philosophy-compliance-workflow SKILL.md should NOT reference code-philosophy-audit"
        );
    }
}
