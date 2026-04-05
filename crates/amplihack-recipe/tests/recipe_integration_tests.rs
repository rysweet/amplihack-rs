//! Integration tests for recipe parsing and step execution.
//!
//! These tests verify that recipes can be parsed from YAML, steps are
//! correctly typed, conditions evaluate properly, and bash commands
//! embedded in steps produce valid shell syntax.

use amplihack_recipe::{Recipe, RecipeParser, Step, StepType};
use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

fn parse_yaml(yaml: &str) -> Recipe {
    let parser = RecipeParser::new();
    parser.parse(yaml).expect("failed to parse recipe YAML")
}

// ── Parse & validate real-world recipe ──

#[test]
fn parse_default_workflow_recipe() {
    let yaml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../amplifier-bundle/recipes/default-workflow.yaml"
    ))
    .expect("default-workflow.yaml not found");

    let recipe = parse_yaml(&yaml);
    assert!(!recipe.name.is_empty(), "recipe must have a name");
    assert!(
        recipe.steps.len() > 10,
        "default-workflow should have many steps"
    );

    // Verify all steps have non-empty IDs
    for step in &recipe.steps {
        assert!(!step.id.is_empty(), "step must have an id");
    }
}

#[test]
fn parse_smart_orchestrator_recipe() {
    let yaml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../amplifier-bundle/recipes/smart-orchestrator.yaml"
    ))
    .expect("smart-orchestrator.yaml not found");

    let recipe = parse_yaml(&yaml);
    assert!(!recipe.name.is_empty());
    assert!(!recipe.steps.is_empty());
}

// ── Step type inference ──

#[test]
fn step_type_inferred_from_command() {
    let yaml = r#"
name: test-recipe
steps:
  - id: bash-step
    command: echo hello
  - id: agent-step
    prompt: do something
    agent: claude
  - id: explicit-bash
    type: shell
    command: ls -la
"#;
    let recipe = parse_yaml(yaml);
    assert_eq!(recipe.steps[0].step_type, StepType::Shell);
    assert_eq!(recipe.steps[1].step_type, StepType::Agent);
    assert_eq!(recipe.steps[2].step_type, StepType::Shell);
}

// ── Bash syntax validation of recipe steps ──

fn validate_bash_syntax(script: &str) -> bool {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(script.as_bytes()).unwrap();
    tmp.flush().unwrap();

    Command::new("bash")
        .arg("-n")
        .arg(tmp.path())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn all_bash_steps_have_valid_syntax() {
    let yaml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../amplifier-bundle/recipes/default-workflow.yaml"
    ))
    .unwrap();

    let recipe = parse_yaml(&yaml);
    let bash_steps: Vec<&Step> = recipe
        .steps
        .iter()
        .filter(|s| s.step_type == StepType::Shell && s.command.is_some())
        .collect();

    assert!(!bash_steps.is_empty(), "should have bash steps");

    for step in &bash_steps {
        let cmd = step.command.as_ref().unwrap();
        assert!(
            validate_bash_syntax(cmd),
            "Bash syntax error in step '{}' (first 100 chars: {})",
            step.id,
            &cmd[..cmd.len().min(100)]
        );
    }
}

// ── Step execution: actually run simple bash steps ──

#[test]
fn execute_simple_bash_step() {
    let yaml = r#"
name: exec-test
steps:
  - id: echo-step
    type: shell
    command: echo "AMPLIHACK_TEST_OUTPUT"
"#;
    let recipe = parse_yaml(yaml);
    let step = &recipe.steps[0];
    let cmd = step.command.as_ref().unwrap();

    let output = Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .output()
        .expect("failed to execute bash step");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("AMPLIHACK_TEST_OUTPUT"));
}

#[test]
fn execute_bash_step_with_env_vars() {
    let yaml = r#"
name: env-test
steps:
  - id: env-step
    type: shell
    command: |
      echo "RESULT=${RECIPE_VAR_test_input}"
"#;
    let recipe = parse_yaml(yaml);
    let cmd = recipe.steps[0].command.as_ref().unwrap();

    let output = Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .env("RECIPE_VAR_test_input", "hello_world")
        .output()
        .expect("failed to execute");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("RESULT=hello_world"));
}

#[test]
fn execute_bash_step_captures_output() {
    let yaml = r#"
name: capture-test
steps:
  - id: capture
    type: shell
    command: |
      echo "line1"
      echo "line2"
      echo "line3"
"#;
    let recipe = parse_yaml(yaml);
    let cmd = recipe.steps[0].command.as_ref().unwrap();

    let output = Command::new("bash").arg("-c").arg(cmd).output().unwrap();

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout_str.lines().collect();
    assert_eq!(lines.len(), 3);
}

// ── max_env_value_bytes parsing ──

#[test]
fn max_env_value_bytes_parsed_from_yaml() {
    let yaml = r#"
name: env-limit-test
steps:
  - id: limited
    type: shell
    command: echo ok
    max_env_value_bytes: 65536
  - id: unlimited
    type: shell
    command: echo ok
"#;
    let recipe = parse_yaml(yaml);
    assert_eq!(recipe.steps[0].max_env_value_bytes, Some(65536));
    assert_eq!(recipe.steps[0].effective_max_env_bytes(), 65536);
    assert_eq!(recipe.steps[1].max_env_value_bytes, None);
    assert_eq!(
        recipe.steps[1].effective_max_env_bytes(),
        Step::DEFAULT_MAX_ENV_BYTES
    );
}

// ── Condition field parsing ──

#[test]
fn condition_field_parsed() {
    let yaml = r#"
name: cond-test
steps:
  - id: conditional
    type: shell
    command: echo yes
    condition: "{{task_type}} == 'development'"
"#;
    let recipe = parse_yaml(yaml);
    assert_eq!(
        recipe.steps[0].condition.as_deref(),
        Some("{{task_type}} == 'development'")
    );
}

// ── Timeout and retry parsing ──

#[test]
fn timeout_and_retry_parsed() {
    let yaml = r#"
name: timeout-test
steps:
  - id: slow
    type: shell
    command: sleep 1
    timeout_seconds: 300
    retry_count: 3
    allow_failure: true
"#;
    let recipe = parse_yaml(yaml);
    assert_eq!(recipe.steps[0].timeout_seconds, Some(300));
    assert_eq!(recipe.steps[0].retry_count, Some(3));
    assert!(recipe.steps[0].allow_failure);
}
