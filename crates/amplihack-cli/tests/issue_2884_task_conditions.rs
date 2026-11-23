//! Issue #2884: the step that decides on merging sees the task text and
//! judges each condition the task states on merging.
//!
//! #3129 removed the merge-ready round that made this decision. The goal
//! loop's turn (`autodrive-goal-turn.yaml`) makes it now: its one agent step
//! reads the task text and names the next action, and its bash step
//! (`save-decision`, the real `autodrive_goal_turn.sh decision`) records that
//! action in the turn folder for the driver.
//!
//! Behaviour tests. The real `autodrive-goal-turn.yaml` runs through the real
//! runner library with its real bash step. Only the agent and the hosting CLI
//! are stand-ins.
//!
//! The stand-in for the `decide` step is a compliant agent. Its only inputs
//! are the prompt it is handed and a fixture world of facts. It looks for the
//! condition in the prompt; if the prompt does not carry the caller's task
//! text it cannot see the condition and chooses to merge, which is the
//! behaviour before #2884. It never reads recipe files and never matches on
//! the recipe's own wording.
//!
//! No assertion here inspects recipe YAML or prompt wording.
// The 2808 fixture is shared by path rather than copied or made more visible.
#[allow(dead_code, clippy::duplicate_mod)]
#[path = "issue_2808/support.rs"]
mod fixture;

use fixture::{Fixture, private, workspace};
use recipe_runner_rs::adapters::Adapter;
use recipe_runner_rs::agent_resolver::AgentResolver;
use recipe_runner_rs::models::{Recipe, RecipeResult, StepType};
use recipe_runner_rs::parser::RecipeParser;
use recipe_runner_rs::runner::{ExecutionListener, RecipeRunner};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const CONDITION: &str = "a release of acme-infra carrying seed pack 2.3.0 exists";
/// The turn's one agent step, which decides on merging.
const DECIDE_STEP: &str = "decide";

fn task_with_condition() -> String {
    format!(
        "Review and merge the PR. Merge only if {CONDITION}; otherwise hand off to the release owner."
    )
}

/// What is true of the outside world in a drive.
#[derive(Clone, Copy)]
struct World {
    release_exists: bool,
}

#[derive(Default)]
struct Seen {
    active: String,
    /// Step id and prompt for every agent step handed out.
    prompts: Vec<(String, String)>,
}

struct Listener(Arc<Mutex<Seen>>);
impl ExecutionListener for Listener {
    fn on_step_start(&self, id: &str, _kind: StepType) {
        self.0.lock().unwrap().active = id.to_owned();
    }
}

struct Agents<'a> {
    fixture: &'a Fixture,
    seen: Arc<Mutex<Seen>>,
    world: World,
    task: String,
    trusted: PathBuf,
}

impl Agents<'_> {
    /// The compliant merge-deciding agent. Sees only its prompt and the world.
    fn decide(&self, prompt: &str) -> String {
        let head = &self.fixture.observed_head;
        let decision = if self.task.contains(CONDITION) && prompt.contains(&self.task) {
            if self.world.release_exists {
                json!({"action": "merge", "args": {"head": head},
                    "why": format!("Condition: {CONDITION}. `gh release list --repo example-org/acme-infra` listed a release carrying seed pack 2.3.0.")})
            } else {
                json!({"action": "handoff",
                    "args": {"owner": "the release owner",
                        "decision": format!("whether to merge while the condition that {CONDITION} does not hold"),
                        "text": format!("The condition that {CONDITION} does not hold: `gh release list --repo example-org/acme-infra` listed no release carrying seed pack 2.3.0. Only the release owner can clear it.")},
                    "why": "The task's merge condition does not hold."})
            }
        } else {
            json!({"action": "merge", "args": {"head": head}, "why": "The checks are green and the review is clean."})
        };
        format!("I read the reading and the task text.\n{decision}")
    }
}

impl Adapter for Agents<'_> {
    fn execute_agent_step(
        &self,
        prompt: &str,
        _agent: Option<&str>,
        _system: Option<&str>,
        _mode: Option<&str>,
        _cwd: &str,
        _model: Option<&str>,
        _timeout: Option<u64>,
    ) -> anyhow::Result<String> {
        let id = {
            let mut seen = self.seen.lock().unwrap();
            let active = seen.active.clone();
            seen.prompts.push((active.clone(), prompt.to_owned()));
            active
        };
        anyhow::ensure!(id == DECIDE_STEP, "unexpected agent step {id}");
        Ok(self.decide(prompt))
    }

    fn execute_bash_step(
        &self,
        command: &str,
        cwd: &str,
        _timeout: Option<u64>,
        env: &HashMap<String, String>,
    ) -> anyhow::Result<String> {
        let f = self.fixture;
        let out = f
            .command("bash")
            .current_dir(cwd)
            .env("AMPLIHACK_HOME", &self.trusted)
            .env("AUTODRIVE_STATE_DIR", &f.state)
            .envs(env)
            .args(["-c", command])
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "step {}: {out:?}",
            self.seen.lock().unwrap().active
        );
        Ok(String::from_utf8(out.stdout)?)
    }

    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "issue-2884-scripted-agents"
    }
}

fn copy_tools(from: &Path, to: &Path) {
    private(to);
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tools(&entry.path(), &target);
        } else if entry.file_type().unwrap().is_symlink() {
            symlink(fs::read_link(entry.path()).unwrap(), &target).unwrap();
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The real turn recipe.
fn goal_turn() -> Recipe {
    let path = workspace().join("amplifier-bundle/recipes/autodrive-goal-turn.yaml");
    RecipeParser::new()
        .parse(&fs::read_to_string(path).unwrap())
        .unwrap()
}

/// A logging stand-in for the hosting CLI: it records its arguments and never
/// reaches a real host.
fn install_logging_gh(f: &Fixture) -> PathBuf {
    let log = f.temp.path().join("gh.log");
    fs::write(&log, "").unwrap();
    let gh = f.bin.join("gh");
    fs::remove_file(&gh).unwrap();
    crate::support::exec_stub::write_executable(
        &gh,
        format!("#!/bin/sh\necho \"$*\" >> '{}'\nexit 0\n", log.display()),
        0o700,
    );
    log
}

struct Drive {
    result: RecipeResult,
    seen: Arc<Mutex<Seen>>,
    gh_log: String,
    /// The turn's decision.json, as `save-decision` wrote it.
    decision: Value,
}

fn drive(task: &str, world: World) -> Drive {
    let f = Fixture::new();
    let gh_log_path = install_logging_gh(&f);
    let trusted = f.temp.path().join("trusted");
    let tools = trusted.join("amplifier-bundle/tools");
    copy_tools(&workspace().join("amplifier-bundle/tools"), &tools);
    let agents = f.temp.path().join("agents");
    private(&agents.join("core"));
    fs::write(
        agents.join("core/builder.md"),
        "---\nname: fixture\ndescription: scripted test agent\n---\nReturn fixture facts.\n",
    )
    .unwrap();
    // The driver makes each turn folder private, under its state folder.
    let turn_dir = f.state.join("turns").join("001");
    private(&f.state.join("turns"));
    private(&turn_dir);
    let goal_text = fs::read_to_string(
        workspace().join("amplifier-bundle/skills/auto-drive-to-merge/goal-merge-pr.md"),
    )
    .unwrap();
    let reading = json!({"schema_version": 1, "turn": 1,
        "goal": {"summary": "PR #2884 is open", "pr": {"head": f.observed_head}},
        "actions": {"goal": ["review", "wait", "merge"],
            "shared": ["continue", "wait-owner", "handoff", "ready", "stop"]},
        "merging_allowed": true});
    let seen = Arc::new(Mutex::new(Seen::default()));
    let runner = RecipeRunner::new(Agents {
        fixture: &f,
        seen: Arc::clone(&seen),
        world,
        task: task.to_owned(),
        trusted,
    })
    .with_working_dir(f.pr.to_str().unwrap())
    .with_auto_stage(false)
    .with_default_search(false)
    .with_agent_resolver(AgentResolver::new(Some(vec![agents])))
    .with_listener(Box::new(Listener(Arc::clone(&seen))));
    let context: HashMap<String, Value> = HashMap::from([
        ("turn_dir".into(), json!(turn_dir)),
        ("repo_path".into(), json!(f.pr)),
        ("goal_text".into(), json!(goal_text)),
        ("task_description".into(), json!(task)),
        ("goal_notes".into(), json!("")),
        ("turn_reading".into(), json!(reading.to_string())),
        ("previous_record".into(), json!("")),
    ]);
    let result = runner.execute(&goal_turn(), Some(context));
    let gh_log = fs::read_to_string(gh_log_path).unwrap();
    let decision = fs::read_to_string(turn_dir.join("decision.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    Drive {
        result,
        seen,
        gh_log,
        decision,
    }
}

/// The action the turn recorded, from a valid decision only.
fn action(d: &Drive) -> &str {
    assert_eq!(d.decision["valid"], json!(true), "{}", d.decision);
    d.decision["action"].as_str().unwrap_or_default()
}

/// Case A: an unmet external condition ends in a hand-off naming it, with no merge.
#[test]
fn unmet_external_condition_ends_in_hand_off_naming_it_and_does_not_merge() {
    let d = drive(
        &task_with_condition(),
        World {
            release_exists: false,
        },
    );
    assert!(d.result.success, "{:?}", d.result);
    assert_eq!(action(&d), "handoff", "{}", d.decision);
    let named = [&d.decision["args"]["decision"], &d.decision["args"]["text"]]
        .iter()
        .any(|v| v.as_str().unwrap_or_default().contains(CONDITION));
    assert!(
        named,
        "the recorded hand-off must name the condition: {}",
        d.decision
    );
    assert!(
        !d.gh_log.contains("pr merge"),
        "a drive with an unmet condition must not merge; gh log: {}",
        d.gh_log
    );
}

/// Case B: a met condition leaves the decision to merge, as before.
#[test]
fn met_condition_still_ends_ready() {
    let d = drive(
        &task_with_condition(),
        World {
            release_exists: true,
        },
    );
    assert!(d.result.success, "{:?}", d.result);
    assert_eq!(action(&d), "merge", "{}", d.decision);
}

/// Case C: task text that states no merge condition, or none at all, changes nothing.
#[test]
fn task_text_without_a_condition_is_a_no_op() {
    for task in ["", "Review and merge the PR."] {
        let d = drive(
            task,
            World {
                release_exists: false,
            },
        );
        assert!(d.result.success, "{task:?}: {:?}", d.result);
        assert_eq!(action(&d), "merge", "task {task:?}: {}", d.decision);
        assert!(!d.gh_log.contains("pr merge"));
    }
}

/// Case D: every agent step of the turn receives the caller's task text, once.
#[test]
fn the_turn_agent_receives_the_task_text() {
    let task = task_with_condition();
    let d = drive(
        &task,
        World {
            release_exists: true,
        },
    );
    assert!(d.result.success, "{:?}", d.result);
    let seen = d.seen.lock().unwrap();
    let ids: Vec<&str> = seen.prompts.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, [DECIDE_STEP], "the turn has one agent step, run once");
    assert!(
        seen.prompts[0].1.contains(&task),
        "{DECIDE_STEP} did not carry the caller's task text"
    );
}
