//! Runs the live execution phase with the pinned engine and fixture agents.
//! The unrelated global temporary-file sweep is excluded from this fixture.

use anyhow::{Result, ensure};
use recipe_runner_rs::adapters::Adapter;
use recipe_runner_rs::agent_resolver::AgentResolver;
use recipe_runner_rs::parser::RecipeParser;
use recipe_runner_rs::runner::RecipeRunner;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

struct FixtureAgent {
    response: String,
}

impl Adapter for FixtureAgent {
    fn execute_agent_step(
        &self,
        prompt: &str,
        _agent: Option<&str>,
        _system: Option<&str>,
        _mode: Option<&str>,
        working_dir: &str,
        _model: Option<&str>,
        _timeout: Option<u64>,
    ) -> Result<String> {
        super::record(json!({"kind": "phase_agent", "prompt": prompt, "cwd": working_dir}))?;
        Ok(self.response.clone())
    }

    fn execute_bash_step(
        &self,
        command: &str,
        working_dir: &str,
        _timeout: Option<u64>,
        extra_env: &HashMap<String, String>,
    ) -> Result<String> {
        let output = Command::new("bash")
            .args(["-c", command])
            .envs(extra_env)
            .current_dir(working_dir)
            .stdin(Stdio::null())
            .output()?;
        ensure!(
            output.status.success(),
            "phase bash failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8(output.stdout)?)
    }

    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "isolated-rust-agent"
    }
}

pub(super) fn run(context_file: &str, recipe_name: &str) -> Result<()> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let recipes = source.join("amplifier-bundle/recipes");
    let mut recipe =
        RecipeParser::new().parse_file(&recipes.join(format!("{recipe_name}.yaml")))?;
    recipe.steps.retain(|s| s.id != "cleanup-helper");
    let context: HashMap<String, Value> = serde_json::from_slice(&std::fs::read(context_file)?)?;
    let response = if recipe_name == "operations-direct" {
        super::result(&serde_json::to_value(&context)?).to_string()
    } else {
        "Fixture answer. STATUS: COMPLETE".into()
    };
    let working_dir = std::env::current_dir()?;
    let runner = RecipeRunner::new(FixtureAgent { response })
        .with_working_dir(working_dir.to_str().expect("UTF-8 fixture path"))
        .with_auto_stage(false)
        .with_agent_resolver(AgentResolver::from_dirs(
            vec![source.join("amplifier-bundle/agents")],
            false,
        ))
        .with_default_search(false)
        .with_recipe_search_dirs(vec![recipes]);
    let result = runner.execute(&recipe, Some(context));
    println!("{}", serde_json::to_string(&result)?);
    ensure!(result.success, "live execution phase failed");
    Ok(())
}
