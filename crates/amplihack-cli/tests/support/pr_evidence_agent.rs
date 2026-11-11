//! Run the real evaluator recipe with real bash steps and a controlled agent.
//! The production runner handles conditions, rendering and step outputs.

use crate::support::auto_drive_light::{private_dir, read, repo_root, show};
use crate::support::exec_stub::write_executable;
use crate::support::quiet_process::child_set_var;
use crate::support::wait::output_with_ceiling;
use anyhow::{Result, bail};
use recipe_runner_rs::adapters::Adapter;
use recipe_runner_rs::models::RecipeResult;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
struct ControlledAgent {
    answer: String,
    prompts: Arc<Mutex<Vec<String>>>,
    root: PathBuf,
}

impl Adapter for ControlledAgent {
    fn execute_agent_step(
        &self,
        prompt: &str,
        _agent_name: Option<&str>,
        _system_prompt: Option<&str>,
        _mode: Option<&str>,
        _working_dir: &str,
        _model: Option<&str>,
        _timeout: Option<u64>,
    ) -> Result<String> {
        self.prompts.lock().unwrap().push(prompt.to_owned());
        Ok(self.answer.clone())
    }

    fn execute_bash_step(
        &self,
        command: &str,
        _working_dir: &str,
        _timeout: Option<u64>,
        extra_env: &HashMap<String, String>,
    ) -> Result<String> {
        let mut cmd = crate::isolated_amplihack::isolated("bash");
        cmd.args(["-c", command])
            .current_dir(&self.root)
            .env("HOME", &self.root)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .envs(extra_env);
        let out = output_with_ceiling(
            "real evaluator bash step",
            &mut cmd,
            Duration::from_secs(20),
        );
        if !out.status.success() {
            bail!("{}", show(&out));
        }
        Ok(String::from_utf8(out.stdout)?)
    }

    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "issue-2333-controlled-agent"
    }
}

/// Call only inside in_quiet_process: the runner's default discovery and
/// auditing see a private home, even if they read process-global state.
pub fn evaluate(answer: &str, child_exit: &str) -> (RecipeResult, Vec<String>) {
    // Resolve/build before changing HOME: cargo's registry and toolchain
    // discovery belong to the test host, not the isolated recipe home.
    let amplihack = crate::support::amplihack_bin::amplihack_bin();
    let tmp = tempfile::tempdir().unwrap();
    child_set_var("HOME", tmp.path());
    child_set_var("AMPLIHACK_HOME", tmp.path().join(".amplihack"));
    let bin = tmp.path().join("bin");
    private_dir(&bin);
    // Use the real Rust helpers, with no real GitHub executable available.
    std::os::unix::fs::symlink(amplihack, bin.join("amplihack")).unwrap();
    write_executable(&bin.join("gh"), "#!/bin/sh\nexit 97\n", 0o700);
    let recipe = recipe_runner_rs::parse_recipe(&read(
        "amplifier-bundle/recipes/loop-health-evaluator.yaml",
    ))
    .unwrap();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let adapter = ControlledAgent {
        answer: answer.to_owned(),
        prompts: Arc::clone(&prompts),
        root: tmp.path().to_owned(),
    };
    let context: HashMap<String, Value> = [
        ("loop_name", json!("auto-drive:merge-ready")),
        ("loop_round_label", json!("round-4")),
        ("loop_repo_path", json!(tmp.path())),
        ("loop_agent_working_dir", json!(tmp.path())),
        ("loop_child_exit_code", json!(child_exit)),
        ("loop_round_verdict", json!("NOT_MERGE_READY")),
        ("loop_clean_token", json!("MERGE_READY")),
        ("loop_ci_signal", json!("PENDING:1/2")),
        ("loop_ci_signal_previous", json!("PENDING:1/2")),
        ("loop_history", json!({"head_sha": "1".repeat(40), "collection_errors": [{"code": "RATE_LIMITED", "api": "GRAPHQL"}], "waiting_history": [{"wake_reason": "DEADLINE", "elapsed_ms": 1}]})),
    ].into_iter().map(|(key, value)| (key.to_owned(), value)).collect();
    let result = recipe_runner_rs::runner::RecipeRunner::new(adapter)
        .with_working_dir(tmp.path().to_str().unwrap())
        .with_agent_resolver(recipe_runner_rs::agent_resolver::AgentResolver::from_dirs(
            vec![repo_root().join("amplifier-bundle/agents")],
            false,
        ))
        .with_recipe_search_dirs(vec![repo_root().join("amplifier-bundle/recipes")])
        .with_default_search(false)
        .with_audit_dir(tmp.path().join("audit"))
        .with_checkpoints(false)
        .execute(&recipe, Some(context));
    let prompts = prompts.lock().unwrap().clone();
    (result, prompts)
}
