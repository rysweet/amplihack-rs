//! Capture existing Development delivery gates without interpreting an exit as success.
use super::{
    completion,
    runtime::{self, Runtime},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn capture(context_file: &Path, recipe: &str) -> Result<()> {
    runtime::owned(context_file, &runtime::base_dir())?;
    let context: Value = serde_json::from_slice(&fs::read(context_file)?)?;
    let state_file = Path::new(
        context["workstream_state_file"]
            .as_str()
            .context("missing state path")?,
    );
    runtime::owned(state_file, &runtime::base_dir().join("state"))?;
    let state: Runtime = serde_json::from_slice(&fs::read(state_file)?)?;
    ensure!(
        ["Development", "Consensus"].contains(&state.config.classification.as_str())
            && context["workstream_id"] == json!(state.id()),
        "delivery context ownership mismatch"
    );
    let selected = state.config.recipe.as_deref().context("missing route")?;
    ensure!(
        selected == recipe || (selected == "quick-change" && recipe == "default-workflow"),
        "untrusted delivery hand-over"
    );
    let directory = std::env::var_os("RECIPE_VAR_worktree_path")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::env::current_dir()?);
    let directory = directory.canonicalize()?;
    runtime::owned(&directory, &state.artefact_dir)?;
    let git = |args: &[&str]| -> Result<String> {
        let out = amplihack_git::command()
            .args(args)
            .current_dir(&directory)
            .output()?;
        ensure!(
            out.status.success(),
            "delivery git command failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(String::from_utf8(out.stdout)?.trim().to_owned())
    };
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    let commit = git(&["rev-parse", "HEAD"])?;
    ensure!(
        git(&["status", "--porcelain"])?.is_empty(),
        "delivery worktree has uncommitted changes"
    );
    let terminal = std::env::var("RECIPE_VAR_terminal_success").unwrap_or_default();
    let terminal_state = std::env::var("RECIPE_VAR_terminal_state").unwrap_or_default();
    let output = if terminal == "true"
        && ["no_diff", "no_change", "obsolete", "NO_DIFF", "OBSOLETE"]
            .contains(&terminal_state.as_str())
    {
        let reason = std::env::var("RECIPE_VAR_terminal_reason")
            .context("terminal adjudication reason missing")?;
        ensure!(
            !reason.trim().is_empty(),
            "terminal adjudication reason is blank"
        );
        json!({"kind": "delivery", "terminal_outcome": "no_change", "evidence": [{"source": "workflow-terminal-state", "reference": commit, "observation": reason}]})
    } else {
        let out = Command::new("gh")
            .args([
                "pr",
                "view",
                &branch,
                "--repo",
                &state.config.repository,
                "--json",
                "number,headRefName,headRefOid,url,state",
            ])
            .current_dir(&directory)
            .output()?;
        ensure!(
            out.status.success(),
            "delivery PR verification failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let pr: Value = serde_json::from_slice(&out.stdout)?;
        ensure!(
            pr["headRefName"] == branch && pr["headRefOid"] == commit,
            "PR head does not match the delivered branch and commit"
        );
        let reference = completion::issue_ref(
            &json!({"repository": state.config.repository, "number": pr["number"]}),
            &state.config.repository,
        )?;
        json!({"kind": "delivery", "branch": branch, "commit": commit, "pull_request": reference,
            "evidence": [{"source": "github", "reference": pr["url"], "observation": format!("{recipe} completed its delivery gates; verified PR head {commit}")}]})
    };
    completion::output(&output, "delivery", &state.config.repository)?;
    let mut outputs = serde_json::Map::new();
    for (name, kind) in &state.config.required_outputs {
        ensure!(
            kind == "delivery",
            "Development adapter cannot invent non-delivery output {name}"
        );
        outputs.insert(name.clone(), output.clone());
    }
    let result = json!({"schema_version": 1, "workstream_id": state.id(), "repository": state.config.repository,
        "classification": state.config.classification, "effective_recipe": recipe, "handover_from": if recipe == selected { Value::Null } else { json!(selected) },
        "outcome": "succeeded", "outputs": outputs});
    completion::validate(&result, &state.config)?;
    runtime::atomic_json(&state.artefact_dir.join("result.json"), &result)
}
