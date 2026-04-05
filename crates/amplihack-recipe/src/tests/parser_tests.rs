use super::*;
use crate::models::StepType;

fn parser() -> RecipeParser {
    RecipeParser::new()
}

#[test]
fn parse_minimal_recipe() {
    let yaml = r#"
name: test-recipe
steps:
  - id: s1
    name: First step
    shell: echo hello
"#;
    let recipe = parser().parse(yaml).unwrap();
    assert_eq!(recipe.name, "test-recipe");
    assert_eq!(recipe.step_count(), 1);
    assert_eq!(recipe.steps[0].step_type, StepType::Shell);
    assert_eq!(recipe.steps[0].command.as_deref(), Some("echo hello"));
}

#[test]
fn parse_full_recipe() {
    let yaml = r#"
name: full-recipe
version: "2.0"
description: A fully specified recipe
on_failure: cleanup-step
steps:
  - id: init
    name: Initialize
    type: shell
    command: "cargo check"
    timeout_seconds: 60
    allow_failure: false
  - id: analyze
    name: Analyze code
    type: agent
    prompt: "Analyze the codebase"
    agent: amplihack:analyzer
    retry_count: 2
  - id: verify
    name: Verify result
    shell: "cargo test"
    continue_on_error: true
"#;
    let recipe = parser().parse(yaml).unwrap();
    assert_eq!(recipe.name, "full-recipe");
    assert_eq!(recipe.version, "2.0");
    assert_eq!(
        recipe.description.as_deref(),
        Some("A fully specified recipe")
    );
    assert_eq!(recipe.on_failure.as_deref(), Some("cleanup-step"));
    assert_eq!(recipe.step_count(), 3);

    let init = recipe.get_step("init").unwrap();
    assert_eq!(init.step_type, StepType::Shell);
    assert_eq!(init.timeout_seconds, Some(60));

    let analyze = recipe.get_step("analyze").unwrap();
    assert_eq!(analyze.step_type, StepType::Agent);
    assert_eq!(analyze.agent.as_deref(), Some("amplihack:analyzer"));
    assert_eq!(analyze.retry_count, Some(2));

    let verify = recipe.get_step("verify").unwrap();
    assert_eq!(verify.step_type, StepType::Shell);
    assert!(verify.allow_failure);
}

#[test]
fn parse_rejects_missing_name() {
    let yaml = "steps:\n  - id: s1\n    shell: echo hi\n";
    let result = parser().parse(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("name"));
}

#[test]
fn parse_rejects_missing_steps() {
    let yaml = "name: no-steps\n";
    let result = parser().parse(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("steps"));
}

#[test]
fn parse_rejects_duplicate_step_ids() {
    let yaml = r#"
name: dupes
steps:
  - id: s1
    shell: echo a
  - id: s1
    shell: echo b
"#;
    let result = parser().parse(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Duplicate"));
}

#[test]
fn parse_enforces_size_limit() {
    let small_parser = RecipeParser::with_max_size(50);
    let yaml = "name: big\nsteps:\n  - id: s1\n    shell: echo this is too long for the limit\n";
    let result = small_parser.parse(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("size limit"));
}

#[test]
fn parse_infers_step_type_from_fields() {
    let yaml = r#"
name: inferred
steps:
  - id: cmd
    name: Shell step
    command: "ls"
  - id: ask
    name: Prompt step
    prompt: "Explain this"
  - id: sub
    name: Sub recipe
    recipe: "other-recipe"
"#;
    let recipe = parser().parse(yaml).unwrap();
    assert_eq!(recipe.steps[0].step_type, StepType::Shell);
    assert_eq!(recipe.steps[1].step_type, StepType::Agent);
    assert_eq!(recipe.steps[2].step_type, StepType::SubRecipe);
}

#[test]
fn parse_coerces_bool_strings() {
    let yaml = r#"
name: coerce
steps:
  - id: s1
    shell: echo hi
    allow_failure: "yes"
"#;
    let recipe = parser().parse(yaml).unwrap();
    assert!(recipe.steps[0].allow_failure);
}

#[test]
fn parse_coerces_timeout_from_string() {
    let yaml = r#"
name: coerce-timeout
steps:
  - id: s1
    shell: echo hi
    timeout_seconds: "120"
"#;
    let recipe = parser().parse(yaml).unwrap();
    assert_eq!(recipe.steps[0].timeout_seconds, Some(120));
}

#[test]
fn parse_auto_generates_step_ids() {
    let yaml = r#"
name: auto-ids
steps:
  - name: First
    shell: echo 1
  - name: Second
    shell: echo 2
"#;
    let recipe = parser().parse(yaml).unwrap();
    assert_eq!(recipe.steps[0].id, "step-0");
    assert_eq!(recipe.steps[1].id, "step-1");
}

#[test]
fn parse_step_context() {
    let yaml = r#"
name: with-context
context:
  repo_path: "."
steps:
  - id: s1
    shell: echo hi
    context:
      verbose: true
"#;
    let recipe = parser().parse(yaml).unwrap();
    assert!(recipe.context.contains_key("repo_path"));
    assert!(recipe.steps[0].context.contains_key("verbose"));
}

#[test]
fn parse_file_nonexistent() {
    let result = parser().parse_file(Path::new("/nonexistent/recipe.yaml"));
    assert!(result.is_err());
}

#[test]
fn parse_file_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.yaml");
    std::fs::write(
        &path,
        "name: file-recipe\nsteps:\n  - id: s1\n    shell: echo ok\n",
    )
    .unwrap();
    let recipe = parser().parse_file(&path).unwrap();
    assert_eq!(recipe.name, "file-recipe");
}

#[test]
fn default_version_applied() {
    let yaml = "name: no-version\nsteps:\n  - id: s1\n    shell: echo hi\n";
    let recipe = parser().parse(yaml).unwrap();
    assert_eq!(recipe.version, "1.0.0");
}

#[test]
fn parse_rejects_non_mapping() {
    let result = parser().parse("- just a list\n- not a recipe\n");
    assert!(result.is_err());
}

#[test]
fn step_type_parsing() {
    assert_eq!(parse_step_type("agent"), Some(StepType::Agent));
    assert_eq!(parse_step_type("Shell"), Some(StepType::Shell));
    assert_eq!(parse_step_type("bash"), Some(StepType::Shell));
    assert_eq!(parse_step_type("Bash"), Some(StepType::Shell));
    assert_eq!(parse_step_type("command"), Some(StepType::Shell));
    assert_eq!(parse_step_type("sub_recipe"), Some(StepType::SubRecipe));
    assert_eq!(parse_step_type("subrecipe"), Some(StepType::SubRecipe));
    assert_eq!(parse_step_type("unknown"), None);
}
