//! Called by the real local-install test after assets have been published.
//! The pinned runner executes the installed recipe with an isolated agent
//! adapter. Any shell/development preparation step fails instead of being
//! accepted by a text-only recipe assertion.

use recipe_runner_rs::adapters::Adapter;
use recipe_runner_rs::agent_resolver::AgentResolver;
use recipe_runner_rs::runner::RecipeRunner;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Agent {
    directory: PathBuf,
    calls: AtomicUsize,
}

impl Adapter for &Agent {
    fn execute_agent_step(
        &self,
        _prompt: &str,
        agent: Option<&str>,
        _system: Option<&str>,
        _mode: Option<&str>,
        working_dir: &str,
        _model: Option<&str>,
        timeout: Option<u64>,
    ) -> anyhow::Result<String> {
        assert_eq!(agent, Some("amplihack:core:builder"));
        assert_eq!(Path::new(working_dir), self.directory);
        assert!(
            timeout.is_none(),
            "Operations has no wall-clock execution budget"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::fs::write(
            self.directory.join("fixture-answer.txt"),
            b"Research delivered in chat\n",
        )?;
        Ok("OPERATIONS_RESULT: completed — fixture answer in chat".into())
    }

    fn execute_bash_step(
        &self,
        _command: &str,
        _working_dir: &str,
        _timeout: Option<u64>,
        _extra_env: &HashMap<String, String>,
    ) -> anyhow::Result<String> {
        anyhow::bail!("installed Operations recipe attempted an unexpected shell action")
    }

    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "issue-2553-isolated-agent"
    }
}

pub(super) fn execute(bundle: &Path, directory: &Path) {
    let agent = Agent {
        directory: directory.to_path_buf(),
        calls: AtomicUsize::new(0),
    };
    let yaml = std::fs::read_to_string(bundle.join("recipes/operations-direct.yaml")).unwrap();
    let recipe = recipe_runner_rs::parse_recipe(&yaml).unwrap();
    let runner = RecipeRunner::new(&agent)
        .with_default_search(false)
        .with_agent_resolver(AgentResolver::from_dirs(vec![bundle.join("agents")], false))
        .with_working_dir(directory.to_str().unwrap());
    let result = runner.execute(
        &recipe,
        Some(HashMap::from([
            (
                "task_description".into(),
                json!("Research the routing questions and report in chat"),
            ),
            ("repo_path".into(), json!(directory)),
        ])),
    );
    assert!(result.success, "installed recipe failed: {result}");
    assert_eq!(agent.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        result.context["operations_result"],
        "OPERATIONS_RESULT: completed — fixture answer in chat"
    );
    assert_eq!(
        std::fs::read(directory.join("fixture-answer.txt")).unwrap(),
        b"Research delivered in chat\n"
    );
}
