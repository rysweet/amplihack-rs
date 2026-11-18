//! Execute the installed round prefixes with the real runner library and real
//! shell steps. Only hosting evidence and agents are scripted. The assessor
//! reports uncertain changes and leaves them in place: this exercises the
//! permitted reporting continuation, not automatic restoration or model quality.
use super::fixture::{Fixture, files_under, private, workspace};
use recipe_runner_rs::adapters::Adapter;
use recipe_runner_rs::agent_resolver::AgentResolver;
use recipe_runner_rs::models::{Recipe, RecipeCheckpoint, RecipeResult, StepStatus, StepType};
use recipe_runner_rs::parser::RecipeParser;
use recipe_runner_rs::runner::{ExecutionListener, RecipeRunner};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const WORKING: &[u8] = b"prior unstaged content 2808\n";
const STAGED: &[u8] = b"prior staged content 2808\n";
const UNTRACKED: &[u8] = b"prior untracked content 2808\n";
const BINARY: &[u8] = b"\0\xff\x01binary working content 2808\0";
const OUTSIDE: &[u8] = b"external symlink content must never be collected 2808\n";
const ROUNDS: [(&str, &str, &str); 2] = [
    (
        "autodrive-crusty-round",
        "step-02-crusty-review",
        "step-03-extract-crusty-verdict",
    ),
    (
        "autodrive-merge-round",
        "step-02-merge-ready-assessment",
        "step-03-outcome-reader",
    ),
];

#[derive(Clone, Copy, Debug)]
enum Mutation {
    None,
    Unstaged,
    Staged,
    AlreadyDirty,
    Untracked,
    Head,
    Interrupted,
}

#[derive(Default)]
struct Calls {
    active: String,
    reviewed: bool,
    assessed: bool,
    // Ordinary artifacts captured by the real pre-review steps, before the
    // reviewer runs. No fixture-defined manifest or new production API.
    baseline: Vec<(PathBuf, Vec<u8>)>,
}

struct Listener(Arc<Mutex<Calls>>);
impl ExecutionListener for Listener {
    fn on_step_start(&self, id: &str, _kind: StepType) {
        self.0.lock().unwrap().active = id.to_owned();
    }
}

struct Agents<'a> {
    fixture: &'a Fixture,
    calls: Arc<Mutex<Calls>>,
    reviewer: &'a str,
    mutation: Mutation,
    trusted: PathBuf,
}

fn artifact_bytes(path: &Path) -> Vec<u8> {
    if fs::symlink_metadata(path).unwrap().is_symlink() {
        use std::os::unix::ffi::OsStrExt;
        fs::read_link(path).unwrap().as_os_str().as_bytes().to_vec()
    } else {
        fs::read(path).unwrap()
    }
}

fn contains_blob(artifacts: &[(PathBuf, Vec<u8>)], value: &[u8]) -> bool {
    !value.is_empty()
        && artifacts
            .iter()
            .any(|(_, blob)| blob.windows(value.len()).any(|part| part == value))
}

impl Adapter for Agents<'_> {
    fn execute_agent_step(
        &self,
        _prompt: &str,
        _agent: Option<&str>,
        _system: Option<&str>,
        _mode: Option<&str>,
        cwd: &str,
        _model: Option<&str>,
        _timeout: Option<u64>,
    ) -> anyhow::Result<String> {
        let f = self.fixture;
        assert_eq!(
            Path::new(cwd).canonicalize().unwrap(),
            f.pr,
            "review-context, review and assessor agents must use the actual PR worktree"
        );
        let mut calls = self.calls.lock().unwrap();
        let id = calls.active.clone();
        if id == self.reviewer {
            calls.reviewed = true;
            assert_eq!(f.git_text(&f.pr, &["rev-parse", "HEAD"]), f.observed_head);
            calls.baseline = files_under(&f.state)
                .into_iter()
                .filter(|path| path.file_name().unwrap() != "qa-evidence.json")
                .map(|path| {
                    let bytes = artifact_bytes(&path);
                    (path, bytes)
                })
                .collect();
            for (_, bytes) in &calls.baseline {
                assert!(
                    !bytes.windows(OUTSIDE.len()).any(|part| part == OUTSIDE),
                    "evidence followed a symlink outside the worktree"
                );
            }
            match self.mutation {
                Mutation::None => {}
                Mutation::Unstaged => fs::write(f.pr.join("pr-only"), b"reviewer unstaged edit\n")?,
                Mutation::Staged => {
                    fs::write(f.pr.join("pr-only"), b"reviewer staged edit\n")?;
                    f.git(&f.pr, &["add", "pr-only"]);
                }
                Mutation::AlreadyDirty => {
                    fs::write(f.pr.join("tracked"), b"reviewer overlapped prior work\n")?
                }
                Mutation::Untracked => fs::write(
                    f.pr.join("prior untracked"),
                    b"reviewer changed existing untracked file\n",
                )?,
                Mutation::Head => {
                    f.git(
                        &f.pr,
                        &[
                            "commit",
                            "--allow-empty",
                            "--only",
                            "-qm",
                            "reviewer moved head",
                        ],
                    );
                }
                Mutation::Interrupted => {
                    fs::write(f.pr.join("tracked"), b"interrupted reviewer edit\n")?;
                    anyhow::bail!(
                        "reviewer interrupted after writing; preserve this invocation's baseline"
                    );
                }
            }
            return Ok(json!({"crusty_verdict":"CLEAN", "concerns":[], "reviewed_head_sha":f.observed_head,
                "summary":"review complete; ready to merge"}).to_string());
        }
        if calls.reviewed {
            calls.assessed = true;
            // A report is a valid agentic handling decision. Do not implement
            // a fake deterministic repair in the fixture and claim it as product behavior.
            let status = f.git(
                &f.pr,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            );
            let index = f.git(&f.pr, &["ls-files", "--stage", "-z"]);
            return Ok(json!({"assessment":"report changes; preserve uncertain edits and continue evaluation",
                "status_bytes":status.len(), "index_bytes":index.len(),
                "reviewed_head_sha":f.observed_head, "head_sha":f.git_text(&f.pr, &["rev-parse", "HEAD"])}).to_string());
        }
        // Pre-review context/platform agents return their declared facts. Step
        // identity comes from the listener, never from matching prompt phrases.
        Ok(
            json!({"pr":"2808", "head_sha":f.observed_head, "resolved_concerns":"", "blockers":[],
            "crusty_status":"DONE_CLEAN", "crusty_reviewed_head_sha":f.observed_head,
            "qa_status":"PASS", "ci_status":"GREEN"})
            .to_string(),
        )
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
            self.calls.lock().unwrap().active
        );
        Ok(String::from_utf8(out.stdout)?)
    }
    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "issue-2808-scripted-agents"
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

fn round_prefix(name: &str, stop: &str) -> Recipe {
    let recipe_path = workspace()
        .join("amplifier-bundle/recipes")
        .join(format!("{name}.yaml"));
    let mut recipe = RecipeParser::new()
        .parse(&fs::read_to_string(recipe_path).unwrap())
        .unwrap();
    let end = recipe
        .steps
        .iter()
        .position(|step| step.id == stop)
        .expect("round outcome boundary");
    recipe.steps.truncate(end);
    recipe
}

fn run(
    f: &Fixture,
    spec: (&str, &str, &str),
    mutation: Mutation,
    checkpoint: Option<&RecipeCheckpoint>,
) -> (RecipeResult, Arc<Mutex<Calls>>) {
    let (name, reviewer, stop) = spec;
    let trusted = f.temp.path().join("trusted");
    let tools = trusted.join("amplifier-bundle/tools");
    if !tools.exists() {
        copy_tools(&workspace().join("amplifier-bundle/tools"), &tools);
    }
    let agents = f.temp.path().join("agents");
    private(&agents.join("core"));
    for file in [
        "builder.md",
        "reviewer.md",
        "core/builder.md",
        "core/reviewer.md",
    ] {
        fs::write(
            agents.join(file),
            "---\nname: fixture\ndescription: scripted test agent\n---\nReturn fixture facts.\n",
        )
        .unwrap();
    }
    // Only hosting/QA evidence is replaced. The pre-review collection and the
    // post-review handling steps themselves are always the shipped recipe's.
    let recipes = f.temp.path().join("recipes");
    private(&recipes);
    fs::write(recipes.join("autodrive-merge-evidence.yaml"), "name: autodrive-merge-evidence\nversion: 1.0.0\nsteps:\n  - id: fixture-evidence\n    type: bash\n    command: \"echo '{}'\"\n").unwrap();
    fs::write(f.state.join("qa-evidence.json"), "{}").unwrap();
    let calls = Arc::new(Mutex::new(Calls::default()));
    let mut runner = RecipeRunner::new(Agents {
        fixture: f,
        calls: Arc::clone(&calls),
        reviewer,
        mutation,
        trusted: trusted.clone(),
    })
    .with_working_dir(f.pr.to_str().unwrap())
    .with_auto_stage(false)
    .with_default_search(false)
    .with_recipe_search_dirs(vec![recipes])
    .with_agent_resolver(AgentResolver::new(Some(vec![agents])))
    .with_listener(Box::new(Listener(Arc::clone(&calls))));
    if let Some(checkpoint) = checkpoint {
        runner = runner.with_resume_from(checkpoint);
    }
    let context = checkpoint.map(|c| c.context.clone()).unwrap_or_else(|| {
        HashMap::from([
            ("repo_path".into(), json!(f.pr)),
            ("pr_number".into(), json!("2808")),
            ("pr_url".into(), json!("https://example.invalid/pull/2808")),
            ("reviewed_head_sha".into(), json!(f.observed_head)),
            ("autodrive_state_dir".into(), json!(f.state)),
            (
                "autodrive_round_record".into(),
                json!(f.state.join("round.json")),
            ),
            ("autodrive_round_label".into(), json!("round-1")),
            ("autodrive_tools_dir".into(), json!(tools)),
            ("autodrive_launch_tools".into(), json!(tools)),
        ])
    });
    (
        runner.execute(&round_prefix(name, stop), Some(context)),
        calls,
    )
}

fn dirty(f: &Fixture) {
    fs::write(f.pr.join("tracked"), STAGED).unwrap();
    f.git(&f.pr, &["add", "tracked"]);
    fs::write(f.pr.join("tracked"), WORKING).unwrap();
    fs::set_permissions(f.pr.join("tracked"), fs::Permissions::from_mode(0o751)).unwrap();
    fs::write(f.pr.join("prior untracked"), UNTRACKED).unwrap();
    fs::write(
        f.pr.join(std::ffi::OsString::from_vec(b"binary\n\t\xff".to_vec())),
        BINARY,
    )
    .unwrap();
    fs::write(f.temp.path().join("outside"), OUTSIDE).unwrap();
    symlink("../../../outside", f.pr.join("external-link")).unwrap();
    f.git(&f.pr, &["update-index", "--split-index"]);
}

#[test]
fn unchanged_reviews_in_both_rounds_use_pr_worktree_and_preserve_git_state() {
    for spec in ROUNDS {
        for has_prior_work in [false, true] {
            let f = Fixture::new();
            if has_prior_work {
                dirty(&f);
            }
            let before_status = f.git(
                &f.pr,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            );
            let before_index = f.git(&f.pr, &["ls-files", "--stage", "-z"]);
            let (result, calls) = run(&f, spec, Mutation::None, None);
            assert!(result.success, "{}: {result:?}", spec.0);
            let calls = calls.lock().unwrap();
            assert!(
                calls.reviewed && calls.assessed,
                "review must be followed by an agentic assessment: {result:?}"
            );
            assert_eq!(
                f.git(
                    &f.pr,
                    &["status", "--porcelain=v1", "-z", "--untracked-files=all"]
                ),
                before_status
            );
            assert_eq!(f.git(&f.pr, &["ls-files", "--stage", "-z"]), before_index);
            assert_eq!(
                fs::read(f.pr.join("tracked")).unwrap(),
                if has_prior_work {
                    WORKING
                } else {
                    b"committed baseline\n"
                }
            );
            assert_eq!(f.git_text(&f.pr, &["rev-parse", "HEAD"]), f.observed_head);
            f.assert_no_private_repositories();
        }
    }
}

#[test]
fn reviewer_changes_are_assessed_without_diff_failure_and_prior_layers_remain_recoverable() {
    for spec in ROUNDS {
        for mutation in [
            Mutation::Unstaged,
            Mutation::Staged,
            Mutation::AlreadyDirty,
            Mutation::Untracked,
            Mutation::Head,
        ] {
            let f = Fixture::new();
            dirty(&f);
            let before_status = f.git(
                &f.pr,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            );
            let before_entries = f.git(&f.pr, &["ls-files", "--stage", "-z"]);
            let index_path = PathBuf::from(f.git_text(
                &f.pr,
                &["rev-parse", "--path-format=absolute", "--git-path", "index"],
            ));
            let index = fs::read(&index_path).unwrap();
            let shared_indexes: Vec<Vec<u8>> = fs::read_dir(index_path.parent().unwrap())
                .unwrap()
                .map(Result::unwrap)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("sharedindex.")
                })
                .map(|entry| fs::read(entry.path()).unwrap())
                .collect();
            assert!(!shared_indexes.is_empty(), "fixture must use a split index");
            let (result, calls) = run(&f, spec, mutation, None);
            assert!(
                result.success,
                "{} {mutation:?}: differences must permit reporting and continuation: {result:?}",
                spec.0
            );
            let calls = calls.lock().unwrap();
            assert!(
                calls.reviewed && calls.assessed,
                "no agent assessed {mutation:?}"
            );
            for value in [
                WORKING,
                STAGED,
                UNTRACKED,
                BINARY,
                before_status.as_slice(),
                before_entries.as_slice(),
                index.as_slice(),
            ] {
                assert!(
                    contains_blob(&calls.baseline, value),
                    "pre-review evidence lost a prior content/index layer ({mutation:?})"
                );
            }
            for shared in &shared_indexes {
                assert!(
                    contains_blob(&calls.baseline, shared),
                    "split-index dependency was not captured"
                );
            }
            assert!(
                contains_blob(&calls.baseline, b"../../../outside"),
                "symlink target was not retained as evidence"
            );
            for (path, bytes) in &calls.baseline {
                assert_eq!(
                    artifact_bytes(path),
                    *bytes,
                    "baseline overwritten after reviewer mutation"
                );
                if !fs::symlink_metadata(path).unwrap().is_symlink() {
                    assert_eq!(
                        fs::metadata(path).unwrap().permissions().mode() & 0o022,
                        0,
                        "evidence under private state must not be writable by other users: {path:?}"
                    );
                }
            }
            assert_eq!(f.git(&f.pr, &["show", ":tracked"]), STAGED);
            assert_eq!(
                fs::metadata(f.pr.join("tracked"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o751
            );
            assert_eq!(
                fs::read(f.pr.join(std::ffi::OsString::from_vec(b"binary\n\t\xff".to_vec())))
                    .unwrap(),
                BINARY
            );
            assert_eq!(
                fs::read(f.repo.join("tracked")).unwrap(),
                b"committed baseline\n"
            );
        }
    }
}

#[test]
fn interrupted_reviewer_resume_keeps_the_original_baseline() {
    for spec in ROUNDS {
        let f = Fixture::new();
        dirty(&f);
        let (first, first_calls) = run(&f, spec, Mutation::Interrupted, None);
        assert!(!first.success, "fixture interruption must be visible");
        let original = first_calls.lock().unwrap().baseline.clone();
        assert!(
            first_calls.lock().unwrap().reviewed,
            "interruption must happen inside reviewer, not an obsolete entry guard: {first:?}"
        );
        assert!(
            contains_blob(&original, WORKING),
            "original dirty work needs recoverable evidence"
        );
        let checkpoint = RecipeCheckpoint {
            recipe_name: spec.0.into(),
            timestamp: "2026-11-18T00:00:00Z".into(),
            completed_steps: first
                .step_results
                .iter()
                .filter(|step| step.status == StepStatus::Completed)
                .map(|step| step.step_id.clone())
                .collect(),
            context: first.context,
        };
        let (resumed, calls) = run(&f, spec, Mutation::None, Some(&checkpoint));
        assert!(resumed.success, "{resumed:?}");
        assert!(calls.lock().unwrap().assessed);
        for (path, bytes) in &original {
            assert_eq!(
                artifact_bytes(path),
                *bytes,
                "resume replaced the interrupted invocation's baseline"
            );
        }
        assert_eq!(
            fs::read(f.pr.join("tracked")).unwrap(),
            b"interrupted reviewer edit\n"
        );
        assert_eq!(f.git(&f.pr, &["show", ":tracked"]), STAGED);
    }
}
