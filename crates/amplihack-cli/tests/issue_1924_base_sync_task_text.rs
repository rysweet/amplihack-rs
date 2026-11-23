//! #1924: the base sync obeys the task text and the repository's merge policy.
//!
//! Behaviour tests. The real `autodrive-merge-loop.yaml` runs through the real
//! runner library, cut to start at `step-03b-base-sync`, with a bare `origin`
//! whose `main` moved by one commit from a second clone. Its real bash steps
//! and the real `autodrive-handoff.yaml` run. Only the agents and the hosting
//! CLI are stand-ins.
//!
//! The stand-in for `step-03b-base-sync` is a compliant agent. Its only inputs
//! are the prompt it is handed and a fixture world of facts. In T4 it can obey
//! the operator's instruction only if its prompt carries the caller's task
//! text; without it, it cannot see the instruction and merges the base, which
//! is the behaviour before this change. It never reads recipe files and never
//! matches on wording this change introduced.
//!
//! - T4: the task text forbids a base merge: none is made, the origin head is
//!   unchanged, and the hand-off carries the agent's reason. Fails before the
//!   change, because the prompt did not carry the task text.
//! - T5: the repository refuses a merge commit in the PR: the agent changes
//!   nothing and names the owner and the decision; the hand-off carries them.
//!   This is route evidence (it also passes before the change, since the old
//!   prompt aborted on any conflict); it shows the agent's decision reaches the
//!   hand-off and that no rebase or force-push is made.
//!
//! What these tests cannot show: whether a real agent follows the prompt. That
//! is checked by review. No assertion here inspects recipe YAML or prompt
//! wording.

use super::*;
use recipe_runner_rs::models::{StepResult, StepStatus};

const SYNC_STEP: &str = "step-03b-base-sync";
const ROUND_STEP: &str = "step-03c-sync-round";
const HANDOFF_STEP: &str = "hand-off";
const FORBID: &str = "Do not merge the base branch into this branch for any reason";
const OWNERS: &str =
    "the maintainers of acme-infra must allow a merge commit, or the operator must rebase by hand";

#[derive(Clone, Copy, PartialEq)]
enum World {
    /// The caller's task text forbids a base merge; the repository allows one.
    TaskForbids,
    /// The task text is silent; the repository refuses a merge commit in a PR.
    RepoRefusesMergeCommits,
}

#[derive(Default)]
struct Trace {
    active: String,
    /// Steps that ran. The runner announces every step, a skipped one too,
    /// before it evaluates the step's condition, so a step counts as run only
    /// when it completes with a status other than `Skipped`.
    ran: Vec<String>,
    prompts: Vec<(String, String)>,
}

struct Steps(Arc<Mutex<Trace>>);
impl ExecutionListener for Steps {
    fn on_step_start(&self, id: &str, _kind: StepType) {
        self.0.lock().unwrap().active = id.to_owned();
    }
    fn on_step_complete(&self, result: &StepResult) {
        if result.status != StepStatus::Skipped {
            self.0.lock().unwrap().ran.push(result.step_id.clone());
        }
    }
}

struct Agents<'a> {
    fixture: &'a Fixture,
    trace: Arc<Mutex<Trace>>,
    world: World,
    task: String,
    trusted: PathBuf,
}

impl Agents<'_> {
    fn git(&self, args: &[&str]) -> String {
        self.fixture.git_text(&self.fixture.pr, args)
    }

    /// The compliant base-sync agent. Sees only its prompt and the world.
    fn base_sync(&self, prompt: &str) -> String {
        match self.world {
            World::TaskForbids if prompt.contains(&self.task) => json!({
                "synced": "false",
                "reason": format!(
                    "the operator's task text says \"{FORBID}\", so I made no base merge; \
                     the operator must decide whether to lift that instruction"
                ),
                "head_sha": ""
            })
            .to_string(),
            World::RepoRefusesMergeCommits => json!({
                "synced": "false",
                "reason": format!("the repository refuses a merge commit in a PR: {OWNERS}"),
                "head_sha": ""
            })
            .to_string(),
            // Cannot see the instruction: does what the default does.
            World::TaskForbids => {
                self.git(&["fetch", "-q", "origin", "--", "main"]);
                let merged = self
                    .fixture
                    .command("git")
                    .current_dir(&self.fixture.pr)
                    .args(["merge", "--no-edit", "origin/main"])
                    .output()
                    .unwrap();
                if !merged.status.success() {
                    self.git(&["checkout", "--ours", "--", "tracked"]);
                    self.git(&["add", "tracked"]);
                    self.git(&["commit", "-q", "--no-edit"]);
                }
                self.git(&["push", "-q", "origin", "HEAD:feature/2808"]);
                json!({"synced": "true", "reason": "merged the base",
                       "head_sha": self.git(&["rev-parse", "HEAD"])})
                .to_string()
            }
        }
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
            let mut t = self.trace.lock().unwrap();
            let id = t.active.clone();
            t.prompts.push((id.clone(), prompt.to_owned()));
            id
        };
        match id.as_str() {
            SYNC_STEP => Ok(self.base_sync(prompt)),
            _ => Ok("handed the PR back as a draft".to_owned()),
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
        let dir = if Path::new(cwd).is_dir() {
            Path::new(cwd)
        } else {
            f.pr.as_path()
        };
        let out = f
            .command("bash")
            .current_dir(dir)
            .env("AMPLIHACK_HOME", &self.trusted)
            .env("REPO_PATH", &f.pr)
            .env("AUTODRIVE_STATE_DIR", &f.state)
            .envs(env)
            .args(["-c", command])
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "bash step {}: {out:?}",
            self.trace.lock().unwrap().active
        );
        Ok(String::from_utf8(out.stdout)?)
    }

    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "issue-1924-scripted-agents"
    }
}

/// The real loop recipe, cut to start at the base sync.
fn loop_from_base_sync() -> Recipe {
    let path = workspace().join("amplifier-bundle/recipes/autodrive-merge-loop.yaml");
    let mut recipe = RecipeParser::new()
        .parse(&fs::read_to_string(path).unwrap())
        .unwrap();
    let start = recipe
        .steps
        .iter()
        .position(|s| s.id == SYNC_STEP)
        .expect("the base sync step");
    recipe.steps.drain(..start);
    recipe
}

struct Drive {
    result: RecipeResult,
    trace: Arc<Mutex<Trace>>,
    gh_log: String,
    origin_head: String,
    origin_head_before: String,
    pr_head: String,
    pr_head_before: String,
    pr_reflog: String,
    git_config: String,
    fixture: Fixture,
}

fn drive(task: &str, world: World) -> Drive {
    let f = Fixture::new();
    let gh_log_path = install_logging_gh(&f);
    let trusted = f.temp.path().join("trusted");
    copy_tools(
        &workspace().join("amplifier-bundle/tools"),
        &trusted.join("amplifier-bundle/tools"),
    );
    let origin = f.temp.path().join("origin.git");
    // The PR changes `tracked` too, so a base change to it conflicts.
    fs::write(f.pr.join("tracked"), b"the PR's change\n").unwrap();
    f.git(&f.pr, &["add", "tracked"]);
    f.git(&f.pr, &["commit", "-qm", "the PR changes tracked"]);
    f.git(&f.pr, &["push", "-q", "origin", "HEAD:feature/2808"]);
    let other = f.temp.path().join("other");
    f.git(
        f.temp.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    f.git(&other, &["checkout", "-q", "-B", "main", "origin/main"]);
    fs::write(other.join("tracked"), b"main's change\n").unwrap();
    f.git(&other, &["add", "tracked"]);
    f.git(&other, &["commit", "-qm", "main changes tracked"]);
    f.git(&other, &["push", "-q", "origin", "HEAD:refs/heads/main"]);

    let origin_head_before = f.git_text(&origin, &["rev-parse", "refs/heads/feature/2808"]);
    let pr_head_before = f.git_text(&f.pr, &["rev-parse", "HEAD"]);
    let agents = f.temp.path().join("agents");
    private(&agents.join("core"));
    fs::write(
        agents.join("core/builder.md"),
        "---\nname: fixture\ndescription: scripted test agent\n---\nReturn fixture facts.\n",
    )
    .unwrap();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let runner = RecipeRunner::new(Agents {
        fixture: &f,
        trace: Arc::clone(&trace),
        world,
        task: task.to_owned(),
        trusted: trusted.clone(),
    })
    .with_working_dir(f.pr.to_str().unwrap())
    .with_auto_stage(false)
    .with_default_search(false)
    .with_recipe_search_dirs(vec![workspace().join("amplifier-bundle/recipes")])
    .with_agent_resolver(AgentResolver::new(Some(vec![agents])))
    .with_listener(Box::new(Steps(Arc::clone(&trace))));
    let preflight = json!({
        "should_run": "true", "pr": "2808", "pr_url": "", "pr_state": "OPEN",
        "branch": "feature/2808", "head_sha": pr_head_before, "base_branch": "main",
        "base_sha": "", "state_dir": f.state, "repo": f.pr,
        "reason": "fixture", "blockers": [], "attempts": []
    });
    let finalize = json!({
        "result": "base-behind", "pr": 2808, "expected_head": pr_head_before,
        "handoff": "false",
        "reason": "GitHub declined the merge: the branch conflicts with its base (mergeStateStatus DIRTY)"
    });
    let context: HashMap<String, Value> = HashMap::from([
        ("repo_path".into(), json!(f.pr)),
        ("pr_number".into(), json!("2808")),
        ("task_description".into(), json!(task)),
        ("no_merge".into(), json!("false")),
        ("autodrive_dry_run".into(), json!("false")),
        ("autodrive_state_dir".into(), json!(f.state)),
        ("merge_loop_preflight".into(), preflight),
        (
            "merge_loop_outcome".into(),
            json!({"outcome": "done", "finalise": "true"}),
        ),
        ("finalize_merge".into(), finalize),
    ]);
    let result = runner.execute(&loop_from_base_sync(), Some(context));
    let gh_log = fs::read_to_string(gh_log_path).unwrap();
    Drive {
        origin_head: f.git_text(&origin, &["rev-parse", "refs/heads/feature/2808"]),
        pr_head: f.git_text(&f.pr, &["rev-parse", "HEAD"]),
        pr_reflog: f.git_text(&f.pr, &["reflog", "--format=%gs"]),
        git_config: f.git_text(&f.pr, &["config", "--local", "--list"]),
        result,
        trace,
        gh_log,
        origin_head_before,
        pr_head_before,
        fixture: f,
    }
}

fn handoff_prompt(d: &Drive) -> String {
    d.trace
        .lock()
        .unwrap()
        .prompts
        .iter()
        .find(|(id, _)| id == HANDOFF_STEP)
        .map(|(_, p)| p.clone())
        .unwrap_or_default()
}

fn no_merge_commit_on(d: &Drive) {
    let head = &d.pr_head;
    let parents = d
        .fixture
        .git_text(&d.fixture.pr, &["show", "-s", "--format=%P", head]);
    assert!(
        parents.split_whitespace().count() <= 1,
        "no base-merge commit was made: {parents}"
    );
}

/// T4. The caller's task text forbids a base merge. The base sync must see
/// that text. Without it the agent merges the base, as it did on 23 Nov.
#[test]
fn a_task_text_forbidding_a_base_merge_is_obeyed() {
    let task = format!("Review and merge the PR. {FORBID}; hand off if it conflicts.");
    let d = drive(&task, World::TaskForbids);

    assert_eq!(
        d.origin_head, d.origin_head_before,
        "the PR head on the remote is unchanged"
    );
    assert_eq!(
        d.pr_head, d.pr_head_before,
        "the worktree HEAD is unchanged"
    );
    no_merge_commit_on(&d);
    assert!(
        !d.trace.lock().unwrap().ran.iter().any(|s| s == ROUND_STEP),
        "no sync round runs when the base was not merged"
    );
    assert!(
        !d.gh_log.contains("pr merge"),
        "no merge was requested: {}",
        d.gh_log
    );
    assert!(d.result.success, "{:?}", d.result);
    let prompt = handoff_prompt(&d);
    assert!(
        prompt.contains(FORBID),
        "the hand-off carries the agent's reason, quoting the instruction: {prompt}"
    );
    assert!(
        prompt.contains("the operator must decide"),
        "the hand-off names who decides: {prompt}"
    );
}

/// T5. The repository refuses a merge commit in a PR. The agent changes
/// nothing, names the owner and the decision, and the hand-off carries them.
/// No rebase and no force-push are made, in git or through the host.
#[test]
fn a_repository_that_refuses_merge_commits_is_handed_off_with_its_owner() {
    let d = drive("Review and merge the PR.", World::RepoRefusesMergeCommits);

    assert_eq!(d.origin_head, d.origin_head_before, "origin head unchanged");
    assert_eq!(d.pr_head, d.pr_head_before, "worktree head unchanged");
    no_merge_commit_on(&d);
    assert!(
        !d.pr_reflog.contains("rebase"),
        "no rebase was made: {}",
        d.pr_reflog
    );
    for line in d.gh_log.lines() {
        assert!(
            !line.contains("--force") && !line.contains("+refs"),
            "no force-push through the host: {line}"
        );
    }
    assert!(!d.gh_log.contains("pr merge"), "{}", d.gh_log);
    // `git config --list` prints keys in lower case (`core.hookspath`).
    assert!(
        !d.git_config.to_ascii_lowercase().contains("core.hookspath"),
        "hooks were not bypassed: {}",
        d.git_config
    );
    assert!(d.result.success, "{:?}", d.result);
    let prompt = handoff_prompt(&d);
    assert!(
        prompt.contains(OWNERS),
        "the hand-off names the owner and the decision: {prompt}"
    );
}
