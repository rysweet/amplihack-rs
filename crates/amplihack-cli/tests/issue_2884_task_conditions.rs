//! Issue #2884: the merge step sees the task text and judges each condition the
//! task states on merging.
//!
//! Behaviour tests. The real `autodrive-merge-round.yaml` runs through the real
//! runner library with real bash steps, up to and including the outcome word
//! (`step-05a-outcome-word`). Only agents and the hosting CLI are stand-ins.
//!
//! The stand-in for `step-02-merge-ready-assessment` is a compliant agent. Its
//! only inputs are the prompt it is handed and a fixture world of facts. It
//! looks for the condition in the prompt; if the prompt does not carry the
//! caller's task text it cannot see the condition and concludes ready, which
//! is the behaviour before this change. It never reads recipe files and never
//! matches on wording that this change introduced.
//!
//! The stand-in for `step-03-outcome-reader` reduces the write-up to the word
//! its closing sentence states, so it does not decide anything itself.
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
const MERGE_STEP: &str = "step-02-merge-ready-assessment";
const READER_STEP: &str = "step-03-outcome-reader";
const FIRST_ROUND_STEP: &str = "step-01-platform-facts";
/// Every agent step of `autodrive-merge-evidence.yaml`. All three render the
/// task text, so each must be seen exactly once carrying it.
const EVIDENCE_STEPS: [&str; 3] = [
    "step-01-sync-with-base",
    "step-02-qa-team-scenarios",
    "step-03-wait-for-ci",
];
const AFTER_WORD_STEP: &str = "step-04-address-blockers";

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
    /// Step id and prompt for every agent step handed out before the round's
    /// own first step ran, which are the steps of the evidence sub-recipe.
    evidence_prompts: Vec<(String, String)>,
    round_started: bool,
}

struct Listener(Arc<Mutex<Seen>>);
impl ExecutionListener for Listener {
    fn on_step_start(&self, id: &str, _kind: StepType) {
        let mut seen = self.0.lock().unwrap();
        if id == FIRST_ROUND_STEP {
            seen.round_started = true;
        }
        seen.active = id.to_owned();
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
    fn assess(&self, prompt: &str) -> String {
        if self.task.contains(CONDITION) && prompt.contains(&self.task) {
            if self.world.release_exists {
                return format!(
                    "Condition: {CONDITION}. Evidence: `gh release list --repo example-org/acme-infra` \
                     listed a release carrying seed pack 2.3.0.\n\nAll 13 criteria are met; ready to merge."
                );
            }
            return format!(
                "Condition: {CONDITION}. Evidence: `gh release list --repo example-org/acme-infra` \
                 listed no release carrying seed pack 2.3.0.\n\n\
                 Hand-off: the condition that {CONDITION} does not hold, and only the release owner can clear it."
            );
        }
        "All 13 criteria are met; ready to merge.".to_owned()
    }

    /// Reduces a write-up to the word its closing sentence states.
    fn read_outcome(&self, prompt: &str) -> String {
        let marker = "<<<UNTRUSTED-MERGE-READY-WRITEUP";
        let end = "UNTRUSTED-MERGE-READY-WRITEUP>>>";
        let body = prompt
            .split(marker)
            .nth(1)
            .and_then(|rest| rest.split(end).next())
            .unwrap_or("");
        let closing = body
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .unwrap_or("")
            .to_lowercase();
        let word = if closing.contains("hand-off") {
            "hand-off"
        } else if closing.contains("ready to merge") {
            "ready"
        } else {
            "not-ready"
        };
        format!("ci: pass\n{word}")
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
        let f = self.fixture;
        let id = {
            let mut seen = self.seen.lock().unwrap();
            let active = seen.active.clone();
            if !seen.round_started {
                seen.evidence_prompts
                    .push((active.clone(), prompt.to_owned()));
            }
            active
        };
        match id.as_str() {
            MERGE_STEP => Ok(self.assess(prompt)),
            READER_STEP => Ok(self.read_outcome(prompt)),
            _ => Ok(json!({
                "pr": "2884", "head_sha": f.observed_head, "resolved_concerns": "", "blockers": [],
                "crusty_status": "DONE_CLEAN", "crusty_reviewed_head_sha": f.observed_head,
                "qa_status": "PASS", "ci_status": "GREEN", "summary": "fixture"
            })
            .to_string()),
        }
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
            .env(
                "AUTODRIVE_LAUNCH_TOOLS",
                self.trusted.join("amplifier-bundle/tools"),
            )
            .env("REPO_PATH", &f.pr)
            .env("AUTODRIVE_STATE_DIR", &f.state)
            .env("AUTODRIVE_QA_EVIDENCE", f.state.join("qa-evidence.json"))
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

/// The real round recipe, cut so that it ends after the outcome word.
fn round_through_outcome_word() -> Recipe {
    let path = workspace().join("amplifier-bundle/recipes/autodrive-merge-round.yaml");
    let mut recipe = RecipeParser::new()
        .parse(&fs::read_to_string(path).unwrap())
        .unwrap();
    let end = recipe
        .steps
        .iter()
        .position(|s| s.id == AFTER_WORD_STEP)
        .expect("step after the outcome word");
    recipe.steps.truncate(end);
    recipe
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
}

fn drive(task: &str, world: World) -> Drive {
    let f = Fixture::new();
    let gh_log_path = install_logging_gh(&f);
    let trusted = f.temp.path().join("trusted");
    let tools = trusted.join("amplifier-bundle/tools");
    copy_tools(&workspace().join("amplifier-bundle/tools"), &tools);
    let agents = f.temp.path().join("agents");
    private(&agents.join("core"));
    for file in ["core/builder.md", "core/reviewer.md"] {
        fs::write(
            agents.join(file),
            "---\nname: fixture\ndescription: scripted test agent\n---\nReturn fixture facts.\n",
        )
        .unwrap();
    }
    fs::write(f.state.join("qa-evidence.json"), "{}").unwrap();
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
    // The real evidence recipe runs: the task text must reach it from the round.
    .with_recipe_search_dirs(vec![workspace().join("amplifier-bundle/recipes")])
    .with_agent_resolver(AgentResolver::new(Some(vec![agents])))
    .with_listener(Box::new(Listener(Arc::clone(&seen))));
    let context: HashMap<String, Value> = HashMap::from([
        ("repo_path".into(), json!(f.pr)),
        ("pr_number".into(), json!("2884")),
        ("pr_url".into(), json!("https://example.invalid/pull/2884")),
        ("task_description".into(), json!(task)),
        ("no_merge".into(), json!("false")),
        ("reviewed_head_sha".into(), json!(f.observed_head)),
        ("autodrive_state_dir".into(), json!(f.state)),
        (
            "autodrive_qa_evidence".into(),
            json!(f.state.join("qa-evidence.json")),
        ),
        (
            "autodrive_round_record".into(),
            json!(f.state.join("round.json")),
        ),
        ("autodrive_round_label".into(), json!("round-1")),
        ("autodrive_tools_dir".into(), json!(tools)),
        ("autodrive_launch_tools".into(), json!(tools)),
    ]);
    let result = runner.execute(&round_through_outcome_word(), Some(context));
    let gh_log = fs::read_to_string(gh_log_path).unwrap();
    Drive {
        result,
        seen,
        gh_log,
    }
}

/// `step-05a-outcome-word` is a `parse_json` bash step, so its result is stored
/// as parsed JSON.
fn outcome_word(result: &RecipeResult) -> &str {
    result
        .context
        .get("merge_ready_word")
        .and_then(|v| v.get("word"))
        .and_then(Value::as_str)
        .unwrap_or_default()
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
    assert_eq!(
        outcome_word(&d.result),
        "hand-off",
        "{:?}",
        d.result.context
    );
    let writeup = d
        .result
        .context
        .get("merge_ready_writeup")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        writeup.contains(CONDITION),
        "the recorded write-up must name the condition: {writeup}"
    );
    assert!(
        !d.gh_log.contains("pr merge"),
        "a drive with an unmet condition must not merge; gh log: {}",
        d.gh_log
    );
}

/// Case B: a met condition leaves the outcome ready, as before.
#[test]
fn met_condition_still_ends_ready() {
    let d = drive(
        &task_with_condition(),
        World {
            release_exists: true,
        },
    );
    assert!(d.result.success, "{:?}", d.result);
    assert_eq!(outcome_word(&d.result), "ready", "{:?}", d.result.context);
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
        assert_eq!(
            outcome_word(&d.result),
            "ready",
            "task {task:?}: {:?}",
            d.result.context
        );
        assert!(!d.gh_log.contains("pr merge"));
    }
}

/// Case D: the round hands its task text to the evidence recipe, whose agent
/// steps receive it.
#[test]
fn evidence_recipe_agents_receive_the_task_text() {
    let task = task_with_condition();
    let d = drive(
        &task,
        World {
            release_exists: true,
        },
    );
    assert!(d.result.success, "{:?}", d.result);
    let seen = d.seen.lock().unwrap();
    let ids: Vec<&str> = seen
        .evidence_prompts
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    for step in EVIDENCE_STEPS {
        let prompts: Vec<&String> = seen
            .evidence_prompts
            .iter()
            .filter(|(id, _)| id == step)
            .map(|(_, prompt)| prompt)
            .collect();
        assert_eq!(
            prompts.len(),
            1,
            "{step} must run exactly once before the round's own steps; saw {ids:?}"
        );
        assert!(
            prompts[0].contains(&task),
            "{step} did not carry the caller's task text"
        );
    }
}
