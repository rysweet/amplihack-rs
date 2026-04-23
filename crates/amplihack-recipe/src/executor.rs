//! Recipe executor — runs recipe steps with env propagation and error recovery.
//!
//! Provides a `RecipeExecutor` that walks a parsed `Recipe`, expands templates,
//! evaluates conditions, runs shell/agent/sub-recipe/checkpoint/parallel steps,
//! and collects `RecipeResult`.

use crate::condition_eval::evaluate_condition;
use crate::models::{Recipe, RecipeResult, Step, StepResult, StepStatus, StepType};
use crate::sub_recipe_recovery::{FailureContext, SubRecipeRecovery};
use crate::template::expand_template;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Write;
use std::time::Instant;
use tracing::{debug, info, warn};

/// Threshold above which a bash command body is spilled to a tempfile and
/// executed as a script (`/bin/bash <path>`) instead of passed inline via
/// `bash -c <body>`. Mirrors recipe-runner #80/#90: argv + env must fit in
/// the kernel's `ARG_MAX` budget (currently 2 MiB on Linux ≥ 2.6.23, but as
/// little as 128 KiB on older kernels and some other Unix variants). When
/// smart-orchestrator final steps accumulate round results from many parallel
/// workstreams, the inlined script body alone can exceed this and fail with
/// `Argument list too long (os error 7)`. 64 KiB is a conservative threshold
/// that leaves headroom for the env vector + binary args even on lower-bound
/// systems, and is well clear of the per-step env budget defined inside
/// `execute_shell_step`.
const BASH_INLINE_LIMIT: usize = 64 * 1024;

/// Wrap `body` in a `bash` `Command`, spilling to a tempfile when the body
/// exceeds [`BASH_INLINE_LIMIT`]. The returned `NamedTempFile` (when
/// present) MUST outlive the spawned process — drop it only after the
/// process has exited, otherwise the script file disappears mid-execution.
fn build_bash_command(
    body: &str,
) -> Result<(std::process::Command, Option<tempfile::NamedTempFile>)> {
    if body.len() > BASH_INLINE_LIMIT {
        let mut tf = tempfile::Builder::new()
            .prefix("recipe-bash-step-")
            .suffix(".sh")
            .tempfile()
            .with_context(|| "Failed to create tempfile for large bash step")?;
        tf.write_all(body.as_bytes())
            .with_context(|| "Failed to write large bash step to tempfile")?;
        // Explicit flush so any I/O errors (disk full, quota exceeded, etc.)
        // surface BEFORE we spawn bash against a possibly-truncated script.
        tf.flush()
            .with_context(|| "Failed to flush large bash step tempfile")?;
        let mut cmd = std::process::Command::new("/bin/bash");
        cmd.arg(tf.path());
        Ok((cmd, Some(tf)))
    } else {
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(body);
        Ok((cmd, None))
    }
}

/// Trait for executing agent steps. Implementors provide the actual agent
/// invocation logic (e.g. spawning a Claude session).
pub trait AgentBackend: Send + Sync {
    /// Execute an agent step, returning the agent's output or an error.
    fn run_agent(
        &self,
        agent_ref: Option<&str>,
        prompt: &str,
        context: &HashMap<String, String>,
    ) -> Result<String>;
}

/// No-op agent backend that returns a placeholder message.
/// Used for dry-run mode and testing.
pub struct DryRunAgentBackend;

impl AgentBackend for DryRunAgentBackend {
    fn run_agent(
        &self,
        agent_ref: Option<&str>,
        prompt: &str,
        _context: &HashMap<String, String>,
    ) -> Result<String> {
        let agent_label = agent_ref.unwrap_or("default");
        Ok(format!(
            "[dry-run] agent={agent_label} prompt_len={}",
            prompt.len()
        ))
    }
}

/// Configuration for recipe execution.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Default step timeout in seconds (overridden by recipe/step settings).
    pub default_timeout_secs: u64,
    /// Whether to run in dry-run mode (no actual shell/agent execution).
    pub dry_run: bool,
    /// Current recursion depth for nested recipe invocations.
    pub recursion_depth: u32,
    /// Maximum recursion depth.
    pub max_recursion_depth: u32,
    /// Working directory for shell commands.
    pub working_dir: String,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: 300,
            dry_run: false,
            recursion_depth: 0,
            max_recursion_depth: 3,
            working_dir: ".".to_string(),
        }
    }
}

/// Executes a parsed recipe step-by-step.
pub struct RecipeExecutor<A: AgentBackend> {
    config: ExecutorConfig,
    agent_backend: A,
    recovery: SubRecipeRecovery,
}

impl<A: AgentBackend> RecipeExecutor<A> {
    pub fn new(config: ExecutorConfig, agent_backend: A) -> Self {
        Self {
            config,
            agent_backend,
            recovery: SubRecipeRecovery::new(),
        }
    }

    /// Execute a recipe with the given initial context.
    pub fn execute(
        &self,
        recipe: &Recipe,
        initial_context: HashMap<String, String>,
    ) -> Result<RecipeResult> {
        info!(recipe = %recipe.name, steps = recipe.step_count(), "Starting recipe execution");

        // Check recursion depth
        if let Some(ref rc) = recipe.recursion
            && self.config.recursion_depth >= rc.max_depth
        {
            let mut result = RecipeResult::new(&recipe.name);
            result.add_step(StepResult::failure(
                "recursion-guard",
                format!(
                    "Recursion depth {} exceeds max_depth {}",
                    self.config.recursion_depth, rc.max_depth
                ),
            ));
            return Ok(result);
        }
        if self.config.recursion_depth >= self.config.max_recursion_depth {
            let mut result = RecipeResult::new(&recipe.name);
            result.add_step(StepResult::failure(
                "recursion-guard",
                format!(
                    "Recursion depth {} exceeds executor max {}",
                    self.config.recursion_depth, self.config.max_recursion_depth
                ),
            ));
            return Ok(result);
        }

        // Merge recipe context defaults with provided context
        let mut context = HashMap::new();
        for (k, v) in &recipe.context {
            if let Some(s) = v.as_str() {
                context.insert(k.clone(), s.to_string());
            } else {
                context.insert(k.clone(), v.to_string());
            }
        }
        for (k, v) in initial_context {
            context.insert(k, v);
        }

        let start = Instant::now();
        let mut result = RecipeResult::new(&recipe.name);

        for step in &recipe.steps {
            let step_result = self.execute_step(step, &mut context, recipe)?;

            let failed = step_result.status == StepStatus::Failed;
            context.insert(
                format!("{}_status", step.id),
                step_result.status.to_string(),
            );
            if let Some(ref output) = step_result.output {
                // Store under the YAML-specified output key if present (fix #226),
                // otherwise use the default `{step_id}_output` key.
                let key = step
                    .output_key
                    .clone()
                    .unwrap_or_else(|| format!("{}_output", step.id));
                context.insert(key, output.clone());
            }

            result.add_step(step_result);

            if failed && !step.allow_failure {
                warn!(step_id = %step.id, "Step failed, aborting recipe");
                // Run on_failure step if configured
                if let Some(ref failure_step_id) = recipe.on_failure
                    && let Some(failure_step) = recipe.get_step(failure_step_id)
                {
                    info!(step_id = %failure_step_id, "Running on_failure handler");
                    let failure_result = self.execute_step(failure_step, &mut context, recipe)?;
                    result.add_step(failure_result);
                }
                break;
            }
        }

        result.total_duration_seconds = start.elapsed().as_secs_f64();
        info!(
            recipe = %recipe.name,
            success = result.success,
            duration = format!("{:.1}s", result.total_duration_seconds),
            "Recipe execution complete"
        );
        Ok(result)
    }

    fn execute_step(
        &self,
        step: &Step,
        context: &mut HashMap<String, String>,
        recipe: &Recipe,
    ) -> Result<StepResult> {
        // Evaluate condition
        if let Some(ref condition) = step.condition {
            let expanded_condition = expand_template(condition, context);
            match evaluate_condition(&expanded_condition, context) {
                Ok(true) => {}
                Ok(false) => {
                    debug!(step_id = %step.id, condition = %expanded_condition, "Condition false, skipping");
                    return Ok(StepResult::skipped(&step.id));
                }
                Err(e) => {
                    warn!(step_id = %step.id, error = %e, "Condition evaluation error, skipping");
                    return Ok(StepResult::skipped(&step.id));
                }
            }
        }

        let timeout = step.effective_timeout(
            recipe
                .default_step_timeout
                .or(Some(self.config.default_timeout_secs)),
        );
        let start = Instant::now();

        debug!(
            step_id = %step.id,
            step_type = %step.step_type,
            timeout = ?timeout,
            "Executing step"
        );

        let result = match step.step_type {
            StepType::Shell => self.execute_shell_step(step, context),
            StepType::Agent | StepType::Prompt => self.execute_agent_step(step, context),
            StepType::SubRecipe => self.execute_sub_recipe_step(step, context),
            StepType::Checkpoint => self.execute_checkpoint_step(step, context),
            StepType::Parallel => self.execute_parallel_step(step, context),
        };

        let mut step_result = match result {
            Ok(output) => {
                let mut sr = StepResult::success(&step.id, &output);
                sr.duration_seconds = start.elapsed().as_secs_f64();
                sr
            }
            Err(e) => {
                let err_msg = e.to_string();
                // Attempt recovery for sub-recipe failures
                if step.step_type == StepType::SubRecipe {
                    let failure_class = self.recovery.classify_failure(&err_msg, None);
                    let fc = FailureContext {
                        recipe_name: recipe.name.clone(),
                        step_id: step.id.clone(),
                        error_message: err_msg.clone(),
                        exit_code: None,
                        failure_class,
                        attempt: 0,
                    };
                    if self.recovery.should_attempt_recovery(&fc) {
                        info!(step_id = %step.id, "Attempting sub-recipe recovery");
                        let prompt = self.recovery.build_recovery_prompt(&fc);
                        if let Ok(recovery_output) =
                            self.agent_backend.run_agent(None, &prompt, context)
                        {
                            let rr = self.recovery.parse_recovery_response(&recovery_output, 1);
                            if rr.recovered {
                                let mut sr = StepResult::success(&step.id, &rr.output);
                                sr.duration_seconds = start.elapsed().as_secs_f64();
                                sr.metadata.insert(
                                    "recovery".to_string(),
                                    serde_json::Value::String(rr.strategy),
                                );
                                return Ok(sr);
                            }
                        }
                    }
                }

                let mut sr = StepResult::failure(&step.id, &err_msg);
                sr.duration_seconds = start.elapsed().as_secs_f64();

                // Handle retry logic
                if let Some(retries) = step.retry_count {
                    for attempt in 1..=retries {
                        warn!(step_id = %step.id, attempt, "Retrying step");
                        let retry_result = match step.step_type {
                            StepType::Shell => self.execute_shell_step(step, context),
                            StepType::Agent | StepType::Prompt => {
                                self.execute_agent_step(step, context)
                            }
                            _ => break,
                        };
                        match retry_result {
                            Ok(output) => {
                                sr = StepResult::success(&step.id, &output);
                                sr.duration_seconds = start.elapsed().as_secs_f64();
                                sr.metadata.insert(
                                    "retry_attempt".to_string(),
                                    serde_json::Value::Number(attempt.into()),
                                );
                                break;
                            }
                            Err(e) => {
                                sr = StepResult::failure(&step.id, e.to_string());
                                sr.duration_seconds = start.elapsed().as_secs_f64();
                            }
                        }
                    }
                }
                sr
            }
        };

        // Propagate step metadata from context
        for (k, v) in &step.context {
            if let Some(s) = v.as_str() {
                step_result
                    .metadata
                    .insert(k.clone(), serde_json::Value::String(s.to_string()));
            }
        }

        Ok(step_result)
    }

    fn execute_shell_step(&self, step: &Step, context: &HashMap<String, String>) -> Result<String> {
        let command = step
            .command
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Shell step '{}' has no command", step.id))?;

        let expanded = expand_template(command, context);

        // Prereq guard: fail fast when a shell step requires python3 but it is
        // not on PATH (fix #242). This prevents 2-hour runs from dying at the
        // last step because a Python sidecar script is missing.
        if (expanded.contains("python3") || expanded.contains("python "))
            && std::process::Command::new("python3")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_err()
        {
            return Err(anyhow::anyhow!(
                "Shell step '{}' requires python3 but it is not installed or not on PATH. \
                 Recipe steps should use deterministic Rust tools instead of Python sidecars.",
                step.id
            ));
        }

        if self.config.dry_run {
            return Ok(format!(
                "[dry-run] shell: {}",
                expanded.chars().take(200).collect::<String>()
            ));
        }

        let (mut cmd, _script_file) = build_bash_command(&expanded)?;
        cmd.current_dir(&self.config.working_dir);

        // Ensure a sane non-interactive environment (fix #277).
        // Many tools (npm, apt, git credential helpers) try interactive prompts
        // unless told otherwise; in recipe execution there is no TTY to respond.
        cmd.env(
            "HOME",
            std::env::var("HOME").unwrap_or_else(|_| "/root".into()),
        );
        cmd.env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()),
        );
        cmd.env("NONINTERACTIVE", "1");
        cmd.env("DEBIAN_FRONTEND", "noninteractive");
        cmd.env("CI", "true");

        // Pass context as environment variables, respecting per-value and
        // cumulative size limits to prevent E2BIG (fix #224).
        let max_env_bytes = step.effective_max_env_bytes();
        // Reserve headroom for existing process env + the command itself.
        const TOTAL_ENV_BUDGET: usize = 1_500_000; // ~1.5MB of ~2MB ARG_MAX
        let mut cumulative_env_bytes: usize = 0;
        // Deny context keys from overriding safety-critical env vars set above.
        // Without this guard, a recipe context key like `path` or `ld_preload`
        // would overwrite the hardened defaults via the to_uppercase transform.
        const PROTECTED_ENV: &[&str] = &[
            "PATH",
            "HOME",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
        ];
        for (k, v) in context {
            let env_key = k.to_uppercase().replace('-', "_");
            if PROTECTED_ENV.contains(&env_key.as_str()) {
                debug!(
                    step_id = %step.id,
                    key = %env_key,
                    "Skipping protected env var from context"
                );
                continue;
            }
            let entry_size = env_key.len() + v.len() + 1; // key=value\0
            if v.len() > max_env_bytes {
                debug!(
                    step_id = %step.id,
                    key = %env_key,
                    size = v.len(),
                    "Env value exceeds per-value limit, skipping"
                );
                continue;
            }
            if cumulative_env_bytes + entry_size > TOTAL_ENV_BUDGET {
                debug!(
                    step_id = %step.id,
                    key = %env_key,
                    cumulative = cumulative_env_bytes,
                    "Env budget exhausted, skipping remaining vars"
                );
                break;
            }
            cumulative_env_bytes += entry_size;
            cmd.env(&env_key, v);
        }

        let output = cmd
            .output()
            .with_context(|| format!("Failed to execute shell step '{}'", step.id))?;
        // _script_file (if Some) is dropped here, after bash has exited —
        // moving the drop earlier would yank the script file mid-execution.
        drop(_script_file);

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok(stdout)
        } else {
            let code = output.status.code().unwrap_or(-1);
            Err(anyhow::anyhow!(
                "Shell step '{}' exited with code {}: {}",
                step.id,
                code,
                if stderr.is_empty() { &stdout } else { &stderr }
            ))
        }
    }

    fn execute_agent_step(&self, step: &Step, context: &HashMap<String, String>) -> Result<String> {
        let prompt = step
            .prompt
            .as_deref()
            .or(step.description.as_deref())
            .unwrap_or("");

        let expanded = expand_template(prompt, context);
        let agent_ref = step.agent.as_deref();

        // Augment context with working directory and non-interactive flag so
        // the agent knows where to read/write files (fix #251).
        let mut augmented = context.clone();
        augmented
            .entry("working_directory".to_string())
            .or_insert_with(|| self.config.working_dir.clone());
        augmented
            .entry("NONINTERACTIVE".to_string())
            .or_insert_with(|| "1".to_string());

        self.agent_backend
            .run_agent(agent_ref, &expanded, &augmented)
    }

    fn execute_sub_recipe_step(
        &self,
        step: &Step,
        _context: &HashMap<String, String>,
    ) -> Result<String> {
        // Sub-recipe execution requires the recipe runner binary — we signal
        // this as a special step type that the caller should handle.
        if self.config.dry_run {
            let recipe_ref = step
                .context
                .get("recipe")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Ok(format!("[dry-run] sub_recipe: {recipe_ref}"));
        }

        // In the native executor, sub-recipe steps delegate to the recipe
        // runner binary for full isolation. This is a placeholder that signals
        // the step type for external orchestration.
        Ok(format!(
            "[sub-recipe] step '{}' — delegated to recipe runner",
            step.id
        ))
    }

    fn execute_checkpoint_step(
        &self,
        step: &Step,
        context: &HashMap<String, String>,
    ) -> Result<String> {
        if self.config.dry_run {
            return Ok(format!("[dry-run] checkpoint: {}", step.id));
        }

        // Checkpoint steps run their command (typically git commit) if present
        if let Some(ref command) = step.command {
            let expanded = expand_template(command, context);
            let (mut cmd, _script_file) = build_bash_command(&expanded)?;
            let output = cmd
                .current_dir(&self.config.working_dir)
                .output()
                .with_context(|| format!("Checkpoint step '{}' failed", step.id))?;
            drop(_script_file);

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if output.status.success() {
                Ok(format!("[checkpoint] {}: {}", step.id, stdout.trim()))
            } else {
                // Checkpoint failures are typically non-fatal
                Ok(format!("[checkpoint] {}: no changes to commit", step.id))
            }
        } else {
            Ok(format!("[checkpoint] {}: marker", step.id))
        }
    }

    fn execute_parallel_step(
        &self,
        step: &Step,
        context: &HashMap<String, String>,
    ) -> Result<String> {
        if self.config.dry_run {
            return Ok(format!("[dry-run] parallel: {}", step.id));
        }

        // Parallel steps contain sub-steps that should run concurrently.
        // The native executor signals this for external orchestration.
        let _ = context;
        Ok(format!(
            "[parallel] step '{}' — orchestrated by recipe runner",
            step.id
        ))
    }
}

/// Build a context map from environment variables with a given prefix.
pub fn context_from_env(prefix: &str) -> HashMap<String, String> {
    let mut ctx = HashMap::new();
    let prefix_upper = prefix.to_uppercase();
    for (key, value) in std::env::vars() {
        if let Some(stripped) = key.strip_prefix(&prefix_upper) {
            let ctx_key = stripped.trim_start_matches('_').to_lowercase();
            if !ctx_key.is_empty() {
                ctx.insert(ctx_key, value);
            }
        }
    }
    ctx
}

/// Merge two context maps, with `overrides` taking precedence.
pub fn merge_context(
    base: &HashMap<String, String>,
    overrides: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = base.clone();
    for (k, v) in overrides {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_recipe() -> Recipe {
        let yaml = r#"
name: test-recipe
steps:
  - id: greet
    type: shell
    command: "echo hello"
  - id: agent-step
    type: agent
    prompt: "Analyze {{task_description}}"
    agent: "builder"
"#;
        crate::parser::RecipeParser::new().parse(yaml).unwrap()
    }

    #[test]
    fn dry_run_executes_all_steps() {
        let recipe = test_recipe();
        let config = ExecutorConfig {
            dry_run: true,
            ..Default::default()
        };
        let executor = RecipeExecutor::new(config, DryRunAgentBackend);
        let result = executor.execute(&recipe, HashMap::new()).unwrap();
        assert!(result.success);
        assert_eq!(result.step_count(), 2);
        assert!(
            result.step_results[0]
                .output
                .as_ref()
                .unwrap()
                .contains("[dry-run] shell")
        );
        assert!(
            result.step_results[1]
                .output
                .as_ref()
                .unwrap()
                .contains("[dry-run] agent")
        );
    }

    #[test]
    fn shell_step_runs_command() {
        let yaml = r#"
name: shell-test
steps:
  - id: echo
    type: shell
    command: "echo test-output-42"
"#;
        let recipe = crate::parser::RecipeParser::new().parse(yaml).unwrap();
        let config = ExecutorConfig::default();
        let executor = RecipeExecutor::new(config, DryRunAgentBackend);
        let result = executor.execute(&recipe, HashMap::new()).unwrap();
        assert!(result.success);
        assert!(
            result.step_results[0]
                .output
                .as_ref()
                .unwrap()
                .contains("test-output-42")
        );
    }

    #[test]
    fn condition_skips_step() {
        let yaml = r#"
name: condition-test
steps:
  - id: always-run
    type: shell
    command: "echo yes"
  - id: never-run
    type: shell
    command: "echo no"
    condition: "'skip' in task_type"
"#;
        let recipe = crate::parser::RecipeParser::new().parse(yaml).unwrap();
        let config = ExecutorConfig::default();
        let executor = RecipeExecutor::new(config, DryRunAgentBackend);
        let result = executor.execute(&recipe, HashMap::new()).unwrap();
        assert!(result.success);
        assert_eq!(result.step_results[1].status, StepStatus::Skipped);
    }

    #[test]
    fn template_expansion_in_command() {
        let yaml = r#"
name: template-test
steps:
  - id: greet
    type: shell
    command: "echo {{greeting}}"
"#;
        let recipe = crate::parser::RecipeParser::new().parse(yaml).unwrap();
        let config = ExecutorConfig::default();
        let executor = RecipeExecutor::new(config, DryRunAgentBackend);
        let mut ctx = HashMap::new();
        ctx.insert("greeting".to_string(), "hello-world".to_string());
        let result = executor.execute(&recipe, ctx).unwrap();
        assert!(result.success);
        assert!(
            result.step_results[0]
                .output
                .as_ref()
                .unwrap()
                .contains("hello-world")
        );
    }

    #[test]
    fn on_failure_handler_runs() {
        let yaml = r#"
name: failure-test
on_failure: cleanup
steps:
  - id: bad-step
    type: shell
    command: "exit 1"
  - id: cleanup
    type: shell
    command: "echo cleaned-up"
"#;
        let recipe = crate::parser::RecipeParser::new().parse(yaml).unwrap();
        let config = ExecutorConfig::default();
        let executor = RecipeExecutor::new(config, DryRunAgentBackend);
        let result = executor.execute(&recipe, HashMap::new()).unwrap();
        assert!(!result.success);
        // Should have bad-step (failed) + cleanup (from on_failure)
        assert_eq!(result.step_count(), 2);
        assert_eq!(result.step_results[0].status, StepStatus::Failed);
        assert_eq!(result.step_results[1].status, StepStatus::Succeeded);
    }

    #[test]
    fn recursion_guard_prevents_deep_nesting() {
        let yaml = r#"
name: recursive-test
recursion:
  max_depth: 2
  max_total_steps: 10
steps:
  - id: step1
    type: shell
    command: "echo ok"
"#;
        let recipe = crate::parser::RecipeParser::new().parse(yaml).unwrap();
        let config = ExecutorConfig {
            recursion_depth: 5,
            max_recursion_depth: 10,
            ..Default::default()
        };
        let executor = RecipeExecutor::new(config, DryRunAgentBackend);
        let result = executor.execute(&recipe, HashMap::new()).unwrap();
        assert!(!result.success);
        assert!(
            result.step_results[0]
                .error
                .as_ref()
                .unwrap()
                .contains("Recursion depth")
        );
    }

    #[test]
    fn allow_failure_continues() {
        let yaml = r#"
name: allow-failure-test
steps:
  - id: may-fail
    type: shell
    command: "exit 1"
    allow_failure: true
  - id: continues
    type: shell
    command: "echo still-running"
"#;
        let recipe = crate::parser::RecipeParser::new().parse(yaml).unwrap();
        let config = ExecutorConfig::default();
        let executor = RecipeExecutor::new(config, DryRunAgentBackend);
        let result = executor.execute(&recipe, HashMap::new()).unwrap();
        // allow_failure means recipe can still succeed overall
        assert_eq!(result.step_count(), 2);
        assert_eq!(result.step_results[0].status, StepStatus::Failed);
        assert_eq!(result.step_results[1].status, StepStatus::Succeeded);
    }

    #[test]
    fn context_from_env_extracts_prefixed_vars() {
        // SAFETY: Single-threaded test — no concurrent env var access.
        unsafe {
            std::env::set_var("AMPLIHACK_TEST_KEY_123", "test_val");
        }
        let ctx = context_from_env("AMPLIHACK_TEST_");
        assert_eq!(ctx.get("key_123").map(|s| s.as_str()), Some("test_val"));
        unsafe {
            std::env::remove_var("AMPLIHACK_TEST_KEY_123");
        }
    }

    #[test]
    fn shell_step_inherits_noninteractive_env() {
        // Verify that shell steps always get HOME, PATH, and NONINTERACTIVE
        // environment variables (fix #277).
        let yaml = r#"
name: env-test
steps:
  - id: check-env
    type: shell
    command: "echo HOME=$HOME NONINTERACTIVE=$NONINTERACTIVE CI=$CI"
"#;
        let recipe = crate::parser::RecipeParser::new().parse(yaml).unwrap();
        let config = ExecutorConfig::default();
        let executor = RecipeExecutor::new(config, DryRunAgentBackend);
        let result = executor.execute(&recipe, HashMap::new()).unwrap();
        assert!(result.success);
        let output = result.step_results[0].output.as_ref().unwrap();
        assert!(
            output.contains("NONINTERACTIVE=1"),
            "missing NONINTERACTIVE"
        );
        assert!(output.contains("CI=true"), "missing CI");
    }

    /// Agent backend that records the context it receives for test assertions.
    struct CapturingAgentBackend {
        captured: std::sync::Mutex<Option<HashMap<String, String>>>,
    }

    impl CapturingAgentBackend {
        fn new() -> Self {
            Self {
                captured: std::sync::Mutex::new(None),
            }
        }
    }

    impl AgentBackend for CapturingAgentBackend {
        fn run_agent(
            &self,
            _agent_ref: Option<&str>,
            _prompt: &str,
            context: &HashMap<String, String>,
        ) -> Result<String> {
            *self.captured.lock().unwrap() = Some(context.clone());
            Ok("ok".to_string())
        }
    }

    #[test]
    fn agent_step_receives_working_directory() {
        // Verify that agent steps get working_directory in their context (fix #251).
        let yaml = r#"
name: agent-context-test
steps:
  - id: agent
    type: agent
    prompt: "do work"
"#;
        let recipe = crate::parser::RecipeParser::new().parse(yaml).unwrap();
        let config = ExecutorConfig {
            working_dir: "/tmp/test-workdir".to_string(),
            ..Default::default()
        };
        let backend = CapturingAgentBackend::new();
        let executor = RecipeExecutor::new(config, backend);
        let result = executor.execute(&recipe, HashMap::new()).unwrap();
        assert!(result.success);
        let captured = executor.agent_backend.captured.lock().unwrap();
        let ctx = captured.as_ref().expect("agent should have been called");
        assert_eq!(
            ctx.get("working_directory").map(|s| s.as_str()),
            Some("/tmp/test-workdir")
        );
        assert_eq!(ctx.get("NONINTERACTIVE").map(|s| s.as_str()), Some("1"));
    }

    #[test]
    fn agent_step_does_not_overwrite_existing_working_directory() {
        // If context already has working_directory, don't overwrite it.
        let yaml = r#"
name: agent-context-preserve-test
steps:
  - id: agent
    type: agent
    prompt: "do work"
"#;
        let recipe = crate::parser::RecipeParser::new().parse(yaml).unwrap();
        let config = ExecutorConfig {
            working_dir: "/tmp/executor-dir".to_string(),
            ..Default::default()
        };
        let backend = CapturingAgentBackend::new();
        let executor = RecipeExecutor::new(config, backend);
        let mut ctx = HashMap::new();
        ctx.insert(
            "working_directory".to_string(),
            "/tmp/caller-dir".to_string(),
        );
        let result = executor.execute(&recipe, ctx).unwrap();
        assert!(result.success);
        let captured = executor.agent_backend.captured.lock().unwrap();
        let ctx = captured.as_ref().expect("agent should have been called");
        // Should keep the caller's working_directory, not overwrite with executor's
        assert_eq!(
            ctx.get("working_directory").map(|s| s.as_str()),
            Some("/tmp/caller-dir")
        );
    }

    #[test]
    fn merge_context_overrides() {
        let mut base = HashMap::new();
        base.insert("a".to_string(), "1".to_string());
        base.insert("b".to_string(), "2".to_string());
        let mut over = HashMap::new();
        over.insert("b".to_string(), "3".to_string());
        over.insert("c".to_string(), "4".to_string());
        let merged = merge_context(&base, &over);
        assert_eq!(merged.get("a").unwrap(), "1");
        assert_eq!(merged.get("b").unwrap(), "3");
        assert_eq!(merged.get("c").unwrap(), "4");
    }

    /// Regression for #268 (closed not-reproducible): the recipe runner must
    /// inherit the parent process environment when launching `bash` steps, so
    /// users can pass things like `PYTHONPATH` or arbitrary custom env vars
    /// from their shell into the recipe. Rust's `Command` inherits by default;
    /// this test pins that behavior so we don't accidentally introduce
    /// `env_clear()` / `env_remove()` in `execute_shell_step`.
    #[test]
    fn shell_step_inherits_parent_env_vars() {
        // RAII guard so the env var is removed even if an assertion or
        // executor call panics — env vars are process-global and cargo runs
        // tests in parallel, so a leak would contaminate sibling tests.
        struct EnvGuard(&'static str);
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                // SAFETY: removing a var we set; no other thread observes
                // this unique key.
                unsafe {
                    std::env::remove_var(self.0);
                }
            }
        }

        let key = "AMPLIHACK_INHERIT_PROBE_268";
        let value = "propagated-from-parent";
        // SAFETY: unique key, no concurrent reader of this name.
        unsafe {
            std::env::set_var(key, value);
        }
        let _guard = EnvGuard(key);

        let yaml = format!(
            r#"
name: env-inherit-probe
steps:
  - id: probe
    type: shell
    command: 'printf "%s" "${}"'
"#,
            key
        );
        let recipe = crate::parser::RecipeParser::new().parse(&yaml).unwrap();
        let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
        let result = executor.execute(&recipe, HashMap::new()).unwrap();

        assert!(result.success, "recipe should succeed");
        assert_eq!(result.step_results[0].status, StepStatus::Succeeded);
        assert_eq!(
            result.step_results[0].output.as_deref(),
            Some(value),
            "shell step did not inherit parent env var (regression for #268)"
        );
    }

    // ---- BASH_INLINE_LIMIT spill (mirrors recipe-runner #80/#90) ----

    #[test]
    fn build_bash_command_inline_for_small_body() {
        let body = "echo hi";
        let (cmd, tf) = build_bash_command(body).unwrap();
        assert!(tf.is_none(), "small body should NOT spill to tempfile");
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        assert_eq!(args[1], body);
    }

    #[test]
    fn build_bash_command_spills_for_oversized_body() {
        // Body > BASH_INLINE_LIMIT must use the file-backed form.
        let big = "a".repeat(64 * 1024 + 1);
        let (cmd, tf) = build_bash_command(&big).unwrap();
        let tf = tf.expect("oversized body must spill to tempfile");
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        // Single arg = the script path; no `-c` form.
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], tf.path().as_os_str());
        // Tempfile actually contains the body.
        let on_disk = std::fs::read_to_string(tf.path()).unwrap();
        assert_eq!(on_disk.len(), big.len());
    }

    #[test]
    fn shell_step_executes_oversized_body_via_tempfile() {
        // End-to-end: a shell step whose body is > 64 KiB must execute
        // successfully (would fail with `Argument list too long` if we
        // still used `bash -c`).
        let mut padding = String::with_capacity(80 * 1024);
        for i in 0..1300 {
            padding.push_str(&format!(
                "# padding {i:04} aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
            ));
        }
        padding.push_str("printf SPILL_OK\n");
        assert!(padding.len() > 64 * 1024);

        let yaml = "
name: spill-test
steps:
  - id: big
    type: shell
    command: 'placeholder'
"
        .to_string();
        let mut recipe = crate::parser::RecipeParser::new().parse(&yaml).unwrap();
        recipe.steps[0].command = Some(padding);

        let executor = RecipeExecutor::new(ExecutorConfig::default(), DryRunAgentBackend);
        let result = executor.execute(&recipe, HashMap::new()).unwrap();
        assert!(result.success, "oversized shell step failed: {result:?}");
        let out = result.step_results[0].output.as_deref().unwrap_or("");
        assert!(
            out.contains("SPILL_OK"),
            "oversized shell step did not run; output={out:?}"
        );
    }
}
