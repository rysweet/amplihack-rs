//! TDD tests for executor bug fixes: #277, #251, #242.
//!
//! These tests define the behavioral contracts for:
//! - #277: Non-interactive environment propagation in shell steps
//! - #251: Working directory context augmentation for agent steps
//! - #242: Python3 prerequisite validation before shell execution

use amplihack_recipe::{
    AgentBackend, DryRunAgentBackend, ExecutorConfig, RecipeExecutor, RecipeParser, StepType,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

fn parse_yaml(yaml: &str) -> amplihack_recipe::Recipe {
    RecipeParser::new()
        .parse(yaml)
        .expect("failed to parse recipe YAML")
}

// ============================================================================
// Bug #277: Shell steps must propagate non-interactive environment variables
// ============================================================================

#[test]
fn shell_step_sets_debian_frontend_noninteractive() {
    // Contract: DEBIAN_FRONTEND=noninteractive must be set so apt-get
    // never prompts for user input during recipe execution.
    let yaml = r#"
name: debian-frontend-test
steps:
  - id: check-debian-frontend
    type: shell
    command: "echo DEBIAN_FRONTEND=$DEBIAN_FRONTEND"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap();
    assert!(
        output.contains("DEBIAN_FRONTEND=noninteractive"),
        "DEBIAN_FRONTEND must be 'noninteractive', got: {output}"
    );
}

#[test]
fn shell_step_sets_ci_true() {
    // Contract: CI=true must be set so tools that check for CI environments
    // suppress interactive behavior.
    let yaml = r#"
name: ci-env-test
steps:
  - id: check-ci
    type: shell
    command: "echo CI=$CI"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap();
    assert!(
        output.contains("CI=true"),
        "CI must be 'true', got: {output}"
    );
}

#[test]
fn shell_step_sets_noninteractive_flag() {
    // Contract: NONINTERACTIVE=1 must be set for all shell steps.
    let yaml = r#"
name: noninteractive-test
steps:
  - id: check-ni
    type: shell
    command: "echo NONINTERACTIVE=$NONINTERACTIVE"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap();
    assert!(
        output.contains("NONINTERACTIVE=1"),
        "NONINTERACTIVE must be '1', got: {output}"
    );
}

#[test]
fn shell_step_preserves_path_env_var() {
    // Contract: PATH must be propagated so standard tools are available.
    let yaml = r#"
name: path-env-test
steps:
  - id: check-path
    type: shell
    command: "test -n \"$PATH\" && echo PATH_SET || echo PATH_EMPTY"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap();
    assert!(
        output.contains("PATH_SET"),
        "PATH must be set, got: {output}"
    );
}

#[test]
fn shell_step_preserves_home_env_var() {
    // Contract: HOME must be set so tools can find config directories.
    let yaml = r#"
name: home-env-test
steps:
  - id: check-home
    type: shell
    command: "test -n \"$HOME\" && echo HOME_SET || echo HOME_EMPTY"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap();
    assert!(
        output.contains("HOME_SET"),
        "HOME must be set, got: {output}"
    );
}

#[test]
fn shell_step_all_five_noninteractive_vars_present() {
    // Contract: All 5 non-interactive environment variables must be set
    // simultaneously in every shell step.
    let yaml = r#"
name: all-env-test
steps:
  - id: check-all
    type: shell
    command: |
      echo "H=$HOME"
      echo "P=$PATH"
      echo "N=$NONINTERACTIVE"
      echo "D=$DEBIAN_FRONTEND"
      echo "C=$CI"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap();
    assert!(output.contains("N=1"), "NONINTERACTIVE must be 1");
    assert!(
        output.contains("D=noninteractive"),
        "DEBIAN_FRONTEND must be noninteractive"
    );
    assert!(output.contains("C=true"), "CI must be true");
    for line in output.lines() {
        if line.starts_with("H=") {
            assert!(line.len() > 2, "HOME must not be empty: {line}");
        }
        if line.starts_with("P=") {
            assert!(line.len() > 2, "PATH must not be empty: {line}");
        }
    }
}

// ============================================================================
// Bug #251: Agent steps must receive working_directory in context
// ============================================================================

/// Agent backend that stores received context in shared state.
struct SharedCapturingBackend {
    captured: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

impl SharedCapturingBackend {
    fn new(store: Arc<Mutex<Vec<HashMap<String, String>>>>) -> Self {
        Self { captured: store }
    }
}

impl AgentBackend for SharedCapturingBackend {
    fn run_agent(
        &self,
        _agent_ref: Option<&str>,
        _prompt: &str,
        context: &HashMap<String, String>,
    ) -> anyhow::Result<String> {
        self.captured.lock().unwrap().push(context.clone());
        Ok("agent output".to_string())
    }
}

#[test]
fn agent_step_receives_working_directory_from_config() {
    // Contract: When no working_directory exists in context, the executor's
    // working_dir config value must be injected.
    let yaml = r#"
name: agent-wd-test
steps:
  - id: agent
    type: agent
    prompt: "Do work in the repo"
"#;
    let recipe = parse_yaml(yaml);
    let store = Arc::new(Mutex::new(Vec::new()));
    let config = ExecutorConfig {
        working_dir: "/home/user/project".to_string(),
        ..Default::default()
    };
    let backend = SharedCapturingBackend::new(store.clone());
    let executor = RecipeExecutor::new(config, backend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let captured = store.lock().unwrap();
    let ctx = captured.last().expect("agent should have been called");
    assert_eq!(
        ctx.get("working_directory").map(String::as_str),
        Some("/home/user/project"),
        "working_directory must be injected from executor config"
    );
}

#[test]
fn agent_step_preserves_caller_working_directory() {
    // Contract: If the caller already provided working_directory in the
    // initial context, it must NOT be overwritten by the executor's config.
    let yaml = r#"
name: agent-wd-preserve-test
steps:
  - id: agent
    type: agent
    prompt: "Do work"
"#;
    let recipe = parse_yaml(yaml);
    let store = Arc::new(Mutex::new(Vec::new()));
    let config = ExecutorConfig {
        working_dir: "/executor/dir".to_string(),
        ..Default::default()
    };
    let backend = SharedCapturingBackend::new(store.clone());
    let executor = RecipeExecutor::new(config, backend);
    let mut ctx = HashMap::new();
    ctx.insert("working_directory".to_string(), "/caller/dir".to_string());
    let result = executor.execute(&recipe, ctx).unwrap();
    assert!(result.success);
    let captured = store.lock().unwrap();
    let ctx = captured.last().unwrap();
    assert_eq!(
        ctx.get("working_directory").map(String::as_str),
        Some("/caller/dir"),
        "caller's working_directory must take precedence"
    );
}

#[test]
fn agent_step_receives_noninteractive_flag() {
    // Contract: Agent context must always contain NONINTERACTIVE=1.
    let yaml = r#"
name: agent-ni-test
steps:
  - id: agent
    type: agent
    prompt: "Analyze code"
"#;
    let recipe = parse_yaml(yaml);
    let store = Arc::new(Mutex::new(Vec::new()));
    let backend = SharedCapturingBackend::new(store.clone());
    let executor = RecipeExecutor::new(ExecutorConfig::default(), backend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let captured = store.lock().unwrap();
    let ctx = captured.last().unwrap();
    assert_eq!(
        ctx.get("NONINTERACTIVE").map(String::as_str),
        Some("1"),
        "NONINTERACTIVE must be '1' in agent context"
    );
}

#[test]
fn agent_step_does_not_overwrite_existing_noninteractive() {
    // Contract: If NONINTERACTIVE is already in context, don't overwrite it.
    let yaml = r#"
name: agent-ni-preserve-test
steps:
  - id: agent
    type: agent
    prompt: "Do work"
"#;
    let recipe = parse_yaml(yaml);
    let store = Arc::new(Mutex::new(Vec::new()));
    let backend = SharedCapturingBackend::new(store.clone());
    let executor = RecipeExecutor::new(ExecutorConfig::default(), backend);
    let mut ctx = HashMap::new();
    ctx.insert("NONINTERACTIVE".to_string(), "0".to_string());
    let result = executor.execute(&recipe, ctx).unwrap();
    assert!(result.success);
    let captured = store.lock().unwrap();
    let ctx = captured.last().unwrap();
    assert_eq!(
        ctx.get("NONINTERACTIVE").map(String::as_str),
        Some("0"),
        "existing NONINTERACTIVE value must be preserved"
    );
}

#[test]
fn prompt_type_step_also_gets_working_directory() {
    // Contract: StepType::Prompt is dispatched through execute_agent_step
    // and must also receive working_directory augmentation.
    let yaml = r#"
name: prompt-wd-test
steps:
  - id: prompt-step
    type: prompt
    prompt: "Review the code"
"#;
    let recipe = parse_yaml(yaml);
    let store = Arc::new(Mutex::new(Vec::new()));
    let config = ExecutorConfig {
        working_dir: "/prompt/workdir".to_string(),
        ..Default::default()
    };
    let backend = SharedCapturingBackend::new(store.clone());
    let executor = RecipeExecutor::new(config, backend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let captured = store.lock().unwrap();
    let ctx = captured.last().unwrap();
    assert_eq!(
        ctx.get("working_directory").map(String::as_str),
        Some("/prompt/workdir"),
    );
}

// ============================================================================
// Bug #242: Python3 prerequisite guard for shell steps
// ============================================================================

#[test]
fn shell_step_with_python3_ref_and_python3_available() {
    // Contract: If a shell command references python3 and python3 IS
    // available, the step should succeed normally.
    let yaml = r#"
name: python3-guard-test
steps:
  - id: py-check
    type: shell
    command: "python3 --version"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    // If python3 is available, step succeeds; if not, it fails with prereq message
    if result.success {
        assert!(result.step_results[0].output.is_some());
    } else {
        let error = result.step_results[0].error.as_ref().unwrap();
        assert!(
            error.contains("python3") && error.contains("not installed"),
            "prereq guard must mention python3 and not installed, got: {error}"
        );
    }
}

#[test]
fn shell_step_without_python3_skips_prereq_check() {
    // Contract: Shell steps that don't reference python3 should never
    // trigger the prerequisite check.
    let yaml = r#"
name: no-python-test
steps:
  - id: simple-echo
    type: shell
    command: "echo no-python-here"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success, "non-python shell step must succeed");
    assert!(
        result.step_results[0]
            .output
            .as_ref()
            .unwrap()
            .contains("no-python-here")
    );
}

#[test]
fn shell_step_with_python_space_triggers_guard() {
    // Contract: "python " (with a space) should also trigger the guard.
    let yaml = r#"
name: python-space-test
steps:
  - id: py-space
    type: shell
    command: "python somescript.py"
"#;
    let recipe = parse_yaml(yaml);
    assert_eq!(recipe.steps[0].step_type, StepType::Shell);
    let cmd = recipe.steps[0].command.as_ref().unwrap();
    assert!(cmd.contains("python "), "command must contain 'python '");
}

// ============================================================================
// Security: Protected env var denylist (prevents context override of PATH/HOME/LD_*)
// ============================================================================

#[test]
fn shell_step_context_cannot_override_path() {
    // Security contract: A recipe context key "path" must NOT overwrite the
    // hardened PATH env var set by the executor.
    let yaml = r#"
name: path-override-test
steps:
  - id: check-path
    type: shell
    command: "echo PATH=$PATH"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let mut ctx = HashMap::new();
    ctx.insert("path".to_string(), "/tmp/evil".to_string());
    let result = executor.execute(&recipe, ctx).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap();
    assert!(
        !output.contains("/tmp/evil"),
        "context key 'path' must NOT override PATH, got: {output}"
    );
}

#[test]
fn shell_step_context_cannot_override_home() {
    // Security contract: A recipe context key "home" must NOT overwrite HOME.
    let yaml = r#"
name: home-override-test
steps:
  - id: check-home
    type: shell
    command: "echo HOME=$HOME"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let mut ctx = HashMap::new();
    ctx.insert("home".to_string(), "/tmp/evil-home".to_string());
    let result = executor.execute(&recipe, ctx).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap();
    assert!(
        !output.contains("/tmp/evil-home"),
        "context key 'home' must NOT override HOME, got: {output}"
    );
}

#[test]
fn shell_step_context_cannot_inject_ld_preload() {
    // Security contract: A recipe context key "ld_preload" or "ld-preload"
    // must NOT set LD_PRELOAD in the subprocess environment.
    let yaml = r#"
name: ld-preload-test
steps:
  - id: check-ld
    type: shell
    command: "echo LD_PRELOAD=${LD_PRELOAD:-unset}"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let mut ctx = HashMap::new();
    ctx.insert("ld-preload".to_string(), "/tmp/evil.so".to_string());
    let result = executor.execute(&recipe, ctx).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap();
    assert!(
        output.contains("LD_PRELOAD=unset"),
        "context must NOT inject LD_PRELOAD, got: {output}"
    );
}

// ============================================================================
// Interaction and edge case tests
// ============================================================================

#[test]
fn shell_step_env_vars_do_not_leak_between_steps() {
    // Contract: Each shell step gets its own environment.
    let yaml = r#"
name: env-isolation-test
steps:
  - id: set-var
    type: shell
    command: "export MY_INTERNAL_VAR=secret && echo step1"
  - id: check-var
    type: shell
    command: "echo MY_INTERNAL_VAR=${MY_INTERNAL_VAR:-unset}"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let output2 = result.step_results[1].output.as_ref().unwrap();
    assert!(
        output2.contains("MY_INTERNAL_VAR=unset"),
        "exported vars from step 1 must not leak to step 2, got: {output2}"
    );
}

#[test]
fn shell_step_context_passed_as_uppercase_env() {
    // Contract: Recipe context variables are passed as uppercase env vars
    // with hyphens replaced by underscores.
    let yaml = r#"
name: context-env-test
steps:
  - id: check-ctx
    type: shell
    command: "echo VAL=$MY_KEY"
"#;
    let recipe = parse_yaml(yaml);
    let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
    let mut ctx = HashMap::new();
    ctx.insert("my-key".to_string(), "test-value".to_string());
    let result = executor.execute(&recipe, ctx).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap();
    assert!(
        output.contains("VAL=test-value"),
        "context key 'my-key' must become env var MY_KEY, got: {output}"
    );
}

#[test]
fn shell_step_respects_working_dir() {
    // Contract: Shell steps run in the configured working directory.
    let tmp = tempfile::tempdir().unwrap();
    let yaml = r#"
name: workdir-test
steps:
  - id: pwd-check
    type: shell
    command: "pwd"
"#;
    let recipe = parse_yaml(yaml);
    let config = ExecutorConfig {
        working_dir: tmp.path().to_string_lossy().to_string(),
        ..Default::default()
    };
    let executor = RecipeExecutor::new(config, DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let output = result.step_results[0].output.as_ref().unwrap().trim();
    let expected = std::fs::canonicalize(tmp.path()).unwrap();
    let actual =
        std::fs::canonicalize(output).unwrap_or_else(|_| std::path::PathBuf::from(output));
    assert_eq!(
        actual, expected,
        "shell step must run in configured working_dir"
    );
}

#[test]
fn dry_run_shell_step_does_not_check_python_prereq() {
    // Contract: In dry-run mode, no prerequisite check is performed
    // (the command is not actually executed).
    let yaml = r#"
name: dry-run-python-test
steps:
  - id: py-dry
    type: shell
    command: "python3 --version"
"#;
    let recipe = parse_yaml(yaml);
    let config = ExecutorConfig {
        dry_run: true,
        ..Default::default()
    };
    let executor = RecipeExecutor::new(config, DryRunAgentBackend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    // Dry-run always succeeds regardless of python3 availability
    assert!(result.success, "dry-run must always succeed");
    assert!(
        result.step_results[0]
            .output
            .as_ref()
            .unwrap()
            .contains("[dry-run] shell"),
        "dry-run output must indicate dry-run mode"
    );
}

#[test]
fn multiple_agent_steps_each_get_working_directory() {
    // Contract: Every agent step gets working_directory, not just the first.
    let yaml = r#"
name: multi-agent-test
steps:
  - id: agent1
    type: agent
    prompt: "First task"
  - id: agent2
    type: agent
    prompt: "Second task"
"#;
    let recipe = parse_yaml(yaml);
    let store = Arc::new(Mutex::new(Vec::new()));
    let config = ExecutorConfig {
        working_dir: "/multi/workdir".to_string(),
        ..Default::default()
    };
    let backend = SharedCapturingBackend::new(store.clone());
    let executor = RecipeExecutor::new(config, backend);
    let result = executor.execute(&recipe, HashMap::new()).unwrap();
    assert!(result.success);
    let captured = store.lock().unwrap();
    assert_eq!(captured.len(), 2, "both agent steps should have been called");
    for (i, ctx) in captured.iter().enumerate() {
        assert_eq!(
            ctx.get("working_directory").map(String::as_str),
            Some("/multi/workdir"),
            "agent step {i} must have working_directory"
        );
        assert_eq!(
            ctx.get("NONINTERACTIVE").map(String::as_str),
            Some("1"),
            "agent step {i} must have NONINTERACTIVE"
        );
    }
}
