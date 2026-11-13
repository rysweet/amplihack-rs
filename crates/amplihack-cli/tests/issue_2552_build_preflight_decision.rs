//! Issue #2552: the auto-drive build phase decides whether to build from the
//! preflight agent's reported facts, not from the agent's reading of a rule.
//!
//! The real `autodrive-build.yaml` runs in process under the real recipe
//! runner library. A fake adapter plays every agent step; its preflight agent
//! reports what an in-memory PR table holds, in place of a `gh` lookup. No
//! `gh` binary is run. The `default-workflow` child is a one-step stub in a
//! private directory, so a build is visible as that stub's bash command.
//!
//! Isolation: recipes are read only from a temporary directory
//! (`with_default_search(false)` plus `with_recipe_search_dirs`), agents only
//! from a temporary directory (`AgentResolver::new(Some(..))`), and no audit
//! directory or listener is attached. `HOME` is not changed: `set_var` is
//! unsafe in edition 2024 and would race with the other tests in this binary.
//! No model, network, home directory or amplihack installation is touched.

use recipe_runner_rs::adapters::Adapter;
use recipe_runner_rs::agent_resolver::AgentResolver;
use recipe_runner_rs::models::StepStatus;
use recipe_runner_rs::parser::RecipeParser;
use recipe_runner_rs::runner::RecipeRunner;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const BUILD_MARKER: &str = "issue-2552-default-workflow-ran";

/// The PRs a `gh pr list --head <branch> --state all` lookup would show, or a
/// refused lookup. Held in memory; no `gh` binary is involved.
enum PrTable {
    Prs(Vec<Value>),
    Refused,
}

/// Plays the agents of `autodrive-build`. The preflight agent reports only
/// what the PR table holds. It also returns `should_run=false` in every
/// case, as the agents in #2552 did, to show the recipe no longer reads it.
struct FakeAgents {
    prs: PrTable,
    bash: Arc<Mutex<Vec<String>>>,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl FakeAgents {
    fn preflight(&self) -> Value {
        let base = json!({
            "should_run": "false",
            "branch": "feat/issue-2552",
            "state_dir": "",
            "repo": "",
            "review_dir": "",
            "attempts": ["gh pr list --head feat/issue-2552 --state all"],
        });
        let facts = match &self.prs {
            PrTable::Refused => json!({
                "pr_found": "unknown", "pr": "", "pr_url": "", "pr_state": "UNKNOWN",
                "blockers": ["gh pr list refused: no access"],
            }),
            PrTable::Prs(prs) => match prs
                .iter()
                .find(|pr| matches!(pr["state"].as_str(), Some("OPEN" | "MERGED")))
            {
                Some(pr) => json!({
                    "pr_found": "true",
                    "pr": pr["number"].to_string(),
                    "pr_url": pr["url"],
                    "pr_state": pr["state"],
                    "blockers": [],
                }),
                None => json!({
                    "pr_found": "false", "pr": "", "pr_url": "", "pr_state": "NONE",
                    "blockers": [],
                }),
            },
        };
        let mut out = base;
        out.as_object_mut()
            .unwrap()
            .extend(facts.as_object().unwrap().clone());
        out
    }
}

impl Adapter for FakeAgents {
    fn execute_agent_step(
        &self,
        prompt: &str,
        _agent_name: Option<&str>,
        _system_prompt: Option<&str>,
        _mode: Option<&str>,
        _working_dir: &str,
        _model: Option<&str>,
        _timeout: Option<u64>,
    ) -> Result<String, anyhow::Error> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        let reply = if prompt.contains("[amplihack:no-merge]") {
            json!({"task_description": "Fix issue #2552", "requirements": ""})
        // Step-01 and step-03 both begin "Establish the PR identity"; only step-03
        // reads build_preflight, so it is matched first. An unrecognised prompt
        // reaches bail!.
        } else if prompt.contains("Read build_preflight") {
            json!({"pr": "7", "terminal_state": "BUILT", "handed_off": false, "blockers": []})
        } else if prompt.contains("Establish the PR identity") {
            self.preflight()
        } else {
            anyhow::bail!("unexpected agent step: {}", &prompt[..prompt.len().min(80)]);
        };
        Ok(reply.to_string())
    }

    fn execute_bash_step(
        &self,
        command: &str,
        _working_dir: &str,
        _timeout: Option<u64>,
        _extra_env: &HashMap<String, String>,
    ) -> Result<String, anyhow::Error> {
        self.bash.lock().unwrap().push(command.to_string());
        Ok("ok".to_string())
    }

    fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "issue-2552-fake"
    }
}

fn recipe_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/recipes/autodrive-build.yaml")
}

/// What one run of `autodrive-build` showed.
struct Run {
    statuses: HashMap<String, StepStatus>,
    /// Bash commands run by the stub `default-workflow`.
    built: Vec<String>,
    /// Agent prompts in the order the steps sent them.
    prompts: Vec<String>,
}

impl Run {
    fn status(&self, id: &str) -> StepStatus {
        self.statuses
            .get(id)
            .copied()
            .unwrap_or_else(|| panic!("{id} has no result: {:?}", self.statuses))
    }

    /// The step-03 prompt, which carries the rendered `{{build_preflight}}`.
    fn resolve_prompt(&self) -> &str {
        self.prompts
            .iter()
            .find(|p| p.contains("Read build_preflight"))
            .unwrap_or_else(|| panic!("step-03 sent no prompt: {:?}", self.statuses))
    }
}

/// Runs `autodrive-build` with `issue_number` set against the given PR table.
fn run(prs: PrTable) -> Run {
    let tmp = tempfile::tempdir().unwrap();
    let recipes = tmp.path().join("recipes");
    let agents = tmp.path().join("agents");
    fs::create_dir_all(&recipes).unwrap();
    fs::create_dir_all(agents.join("core")).unwrap();
    fs::write(agents.join("core/builder.md"), "fake builder\n").unwrap();
    fs::write(
        recipes.join("default-workflow.yaml"),
        format!(
            "name: default-workflow\nversion: 1.0.0\nsteps:\n  - id: build\n    type: bash\n    command: \"echo {BUILD_MARKER}\"\n"
        ),
    )
    .unwrap();

    let text = fs::read_to_string(recipe_path()).unwrap();
    let recipe = RecipeParser::new()
        .parse(&text)
        .expect("autodrive-build parses");
    let bash = Arc::new(Mutex::new(Vec::new()));
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let agents_impl = FakeAgents {
        prs,
        bash: Arc::clone(&bash),
        prompts: Arc::clone(&prompts),
    };
    let runner = RecipeRunner::new(agents_impl)
        .with_working_dir(tmp.path().to_str().unwrap())
        .with_auto_stage(false)
        .with_default_search(false)
        .with_recipe_search_dirs(vec![recipes])
        .with_agent_resolver(AgentResolver::new(Some(vec![agents])));
    let context = HashMap::from([
        ("issue_number".to_string(), json!("2552")),
        ("task_description".to_string(), json!("Fix issue #2552")),
    ]);
    let result = runner.execute(&recipe, Some(context));
    let statuses = result
        .step_results
        .iter()
        .map(|step| (step.step_id.clone(), step.status))
        .collect();
    let built = bash.lock().unwrap().clone();
    let prompts = prompts.lock().unwrap().clone();
    Run {
        statuses,
        built,
        prompts,
    }
}

#[test]
fn a_fresh_lane_with_no_pr_runs_default_workflow() {
    let run = run(PrTable::Prs(vec![]));
    assert_eq!(
        run.status("step-02-default-workflow"),
        StepStatus::Completed,
        "no PR after a complete search must build: {:?}",
        run.statuses
    );
    assert_eq!(
        run.built,
        vec![format!("echo {BUILD_MARKER}")],
        "the stub default-workflow ran once"
    );
    assert_eq!(run.status("step-03-resolve-pr"), StepStatus::Completed);
}

#[test]
fn a_closed_unmerged_pr_does_not_stop_the_build() {
    let run = run(PrTable::Prs(vec![json!({
        "number": 5, "state": "CLOSED", "url": "https://example.invalid/pull/5"
    })]));
    assert_eq!(
        run.status("step-02-default-workflow"),
        StepStatus::Completed
    );
    assert_eq!(run.built.len(), 1, "{:?}", run.built);
}

#[test]
fn a_lane_with_an_open_pr_reuses_it_without_building() {
    for state in ["OPEN", "MERGED"] {
        let run = run(PrTable::Prs(vec![json!({
            "number": 7, "state": state, "url": "https://example.invalid/pull/7"
        })]));
        assert_eq!(
            run.status("step-02-default-workflow"),
            StepStatus::Skipped,
            "an existing {state} PR is reused, not rebuilt"
        );
        assert!(run.built.is_empty(), "{:?}", run.built);
        assert_eq!(
            run.status("step-03-resolve-pr"),
            StepStatus::Completed,
            "the reused {state} PR is still resolved"
        );
        let resolve = run.resolve_prompt();
        assert!(
            resolve.contains("\"pr\":\"7\"") || resolve.contains("pull/7"),
            "step-03 must be handed the {state} PR 7 to reuse: {resolve}"
        );
    }
}

#[test]
fn a_lookup_that_could_not_complete_does_not_build() {
    let run = run(PrTable::Refused);
    assert_eq!(
        run.status("step-02-default-workflow"),
        StepStatus::Skipped,
        "missing access is not absence of a PR"
    );
    assert!(run.built.is_empty(), "{:?}", run.built);
    let resolve = run.resolve_prompt();
    assert!(
        resolve.contains("gh pr list refused"),
        "the named blocker must reach step-03: {resolve}"
    );
}
