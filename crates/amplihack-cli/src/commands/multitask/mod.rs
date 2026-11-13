//! Native parallel workstream orchestrator.
//!
//! Executes multiple independent development workstreams in parallel, each in
//! its own subprocess with isolated working directories. Supports recipe-based
//! and classic execution modes, per-workstream timeout budgets, state
//! persistence for resumption, and automatic cleanup of completed workstreams.

mod cleanup;
// Historic launcher/state adapters are retained for migration regression tests.
#[cfg(test)]
#[allow(dead_code)]
mod command_builder;
#[cfg(test)]
mod command_builder_tests;
mod completion;
// Historic launcher/state adapters are retained for migration regression tests.
#[cfg(test)]
#[allow(dead_code)]
mod default_branch;
mod delivery;
mod dispatch;
mod execution;
mod github;
pub(crate) mod identity;
pub(crate) mod issue;
#[cfg(test)]
mod issue_1570_launcher;
#[cfg(test)]
mod issue_1642_repo_identity;
#[cfg(test)]
mod issue_1966_criticality;
#[cfg(test)]
mod issue_2014_launcher_env;
#[cfg(test)]
mod issue_2553_contracts;
// Historic launcher/state adapters are retained for migration regression tests.
#[cfg(test)]
#[allow(dead_code)]
mod launcher;
#[cfg(test)]
mod launcher_tests;
// Historic launcher/state adapters are retained for migration regression tests.
#[cfg(test)]
#[allow(dead_code)]
mod log_output;
mod migration;
pub(crate) mod models;
mod operations;
// Historic launcher/state adapters are retained for migration regression tests.
#[cfg(test)]
#[allow(dead_code)]
mod orchestrator;
// Historic launcher/state adapters are retained for migration regression tests.
#[cfg(test)]
#[allow(dead_code)]
mod persistence;
pub(crate) mod plan;
mod process_scope;
mod reconcile;
mod runtime;
// Historic launcher/state adapters are retained for migration regression tests.
#[cfg(test)]
#[allow(dead_code)]
mod state;
#[cfg(test)]
mod state_tests;
mod tracking;
mod trusted;
mod utils;
pub(crate) mod waves;

use anyhow::Result;
use std::path::Path;

/// Run parallel workstreams from a JSON config file.
pub fn run_multitask(
    config_path: &str,
    mode: &str,
    recipe: &str,
    max_runtime: Option<u64>,
    timeout_policy: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    // Retained CLI options describe legacy runtime policy; readiness and completion
    // are event driven and do not create wall-clock probes.
    let _ = (max_runtime, timeout_policy);
    dispatch::run(Path::new(config_path), mode, recipe, dry_run)
}

pub fn run_reconcile(config: &str, resolution_file: &str) -> Result<()> {
    reconcile::run(Path::new(config), Path::new(resolution_file))
}

pub fn complete(context_file: &str) -> Result<()> {
    use anyhow::Context;
    use std::io::Read;
    let context_path = Path::new(context_file);
    runtime::owned(context_path, &runtime::base_dir())?;
    let context: serde_json::Value = serde_json::from_slice(&std::fs::read(context_path)?)?;
    let state_path = Path::new(
        context["workstream_state_file"]
            .as_str()
            .context("missing state path")?,
    );
    runtime::owned(state_path, &runtime::base_dir().join("state"))?;
    let state: runtime::Runtime = serde_json::from_slice(&std::fs::read(state_path)?)?;
    anyhow::ensure!(
        context["workstream_id"] == serde_json::json!(state.id())
            && context["repository"] == state.config.repository,
        "completion context ownership mismatch"
    );
    let mut text = String::new();
    std::io::stdin()
        .take(16 * 1024 * 1024)
        .read_to_string(&mut text)?;
    let result = super::orch::extract_json_with_field(&text, "schema_version")
        .context("missing structured completion")?;
    completion::validate(&result, &state.config)?;
    let result_path = state.artefact_dir.join("result.json");
    runtime::owned(&result_path, &state.artefact_dir)?;
    runtime::atomic_json(&result_path, &result)
}

pub fn capture_delivery(context_file: &str, recipe: &str) -> Result<()> {
    delivery::capture(Path::new(context_file), recipe)
}

pub fn run_operation(context_file: &str, output_name: &str, request_file: &str) -> Result<()> {
    operations::run(
        Path::new(context_file),
        output_name,
        Path::new(request_file),
    )
}

pub fn capture_answer(context_file: &str) -> Result<()> {
    use anyhow::Context;
    use serde_json::json;
    use std::io::Read;
    let path = Path::new(context_file);
    runtime::owned(path, &runtime::base_dir())?;
    let context: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let state_file = Path::new(
        context["workstream_state_file"]
            .as_str()
            .context("missing state path")?,
    );
    runtime::owned(state_file, &runtime::base_dir().join("state"))?;
    let state: runtime::Runtime = serde_json::from_slice(&std::fs::read(state_file)?)?;
    anyhow::ensure!(
        ["Investigation", "Q&A"].contains(&state.config.classification.as_str()),
        "answer adapter requires Investigation or Q&A"
    );
    let mut text = String::new();
    std::io::stdin()
        .take(16 * 1024 * 1024)
        .read_to_string(&mut text)?;
    anyhow::ensure!(!text.trim().is_empty(), "workflow produced no answer");
    let mut outputs = serde_json::Map::new();
    for (name, kind) in &state.config.required_outputs {
        anyhow::ensure!(
            kind == "answer",
            "answer adapter cannot supply output {name} of kind {kind}"
        );
        outputs.insert(name.clone(), json!({"kind": "answer", "text": text, "destination": "chat", "evidence": [{"source": "workflow-report", "reference": state.config.recipe, "observation": "Completed workflow report"}]}));
    }
    let result = json!({"schema_version": 1, "workstream_id": state.id(), "repository": state.config.repository, "classification": state.config.classification, "effective_recipe": state.config.recipe, "outcome": "succeeded", "outputs": outputs});
    completion::validate(&result, &state.config)?;
    runtime::atomic_json(&state.artefact_dir.join("result.json"), &result)
}

/// Clean up workstreams with merged PRs.
pub fn run_cleanup(config_path: &str, dry_run: bool) -> Result<()> {
    let base = runtime::base_dir();
    cleanup::cleanup_merged(&base, &base.join("state"), config_path, dry_run)
}

/// Display identity, optional tracking and lifecycle from the owned runtime root.
pub fn run_status(base_dir: Option<&str>) -> Result<()> {
    let base = base_dir
        .map(std::path::PathBuf::from)
        .unwrap_or_else(runtime::base_dir);
    let state_dir = base.join("state");
    if !state_dir.exists() {
        println!("No workstream state at {}", state_dir.display());
        return Ok(());
    }
    for entry in std::fs::read_dir(&state_dir)? {
        let path = entry?.path();
        if path.extension().is_none_or(|s| s != "json")
            || path
                .file_name()
                .is_some_and(|s| s.to_string_lossy().contains(".progress."))
        {
            continue;
        }
        runtime::owned(&path, &state_dir)?;
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let id = value["workstream_id"].as_str().unwrap_or("legacy");
        let lifecycle = value
            .get("lifecycle")
            .or_else(|| value.get("lifecycle_state"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let issue = issue::parse_optional(value.get("issue").unwrap_or(&serde_json::Value::Null))?;
        let tracking = issue
            .map(|n| format!(" issue=#{}", n.get()))
            .unwrap_or_default();
        println!("[{id}] {lifecycle}{tracking}");
    }
    Ok(())
}
