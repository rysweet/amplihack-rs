//! Issue #2336 fixtures. All helpers except recipe/agent execution and gh are real.
//!
//! The proposed CLI seam is `orch helper autodrive-completion-{prepare,write,dispatch}`.
//! Prepare returns {run_id, private_state, decision_file}; write accepts only an
//! action/reason and computes identity/hash itself. Dispatch takes child status
//! separately. Keeping that seam here permits renaming without rewriting cases.
//! The recipe boundary double executes extracted bash bodies; it does not mock
//! classification, record writing, dispatch, follow-up tools, or phase markers.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::auto_drive_light::{
    Fakes, fake_bin, private_dir, private_file, repo_root, show, stdout, step_command, tools_dir,
};

pub const ROUND: &str = "autodrive-crusty-round";
#[path = "autodrive_completion_boundary.rs"]
mod boundary;

pub struct Drive {
    pub tmp: tempfile::TempDir,
    pub state: PathBuf,
    pub repo: PathBuf,
    pub review: PathBuf,
    pub fakes: Fakes,
    pub task: PathBuf,
}

impl Drive {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
        // macOS may spell the global temp ancestor through /var -> /private/var.
        // Private-state helpers require physical paths for no-follow access.
        let physical = tmp.path().canonicalize().unwrap();
        let state = physical.join("state");
        let review = physical.join("review");
        let repo = physical.join("pr");
        private_dir(&state);
        private_dir(&review);
        let fakes = fake_bin(tmp.path());
        private_file(&fakes.log.join("gh.log"), "");
        crate::support::exec_stub::write_executable(
            &fakes.bin.join("amplihack"),
            boundary::SHIM,
            0o755,
        );
        let task = tmp.path().join("task.txt");
        private_file(&task, "Fix the issue\n");
        let d = Self {
            tmp,
            state,
            repo,
            review,
            fakes,
            task,
        };
        let origin = d.tmp.path().join("origin.git");
        d.git(
            d.tmp.path(),
            &["init", "-q", "--bare", origin.to_str().unwrap()],
        );
        d.git(&d.review, &["init", "-q", "-b", "main"]);
        d.git(
            &d.review,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        d.git(&d.review, &["commit", "--allow-empty", "-qm", "baseline"]);
        d.git(
            &d.review,
            &[
                "worktree",
                "add",
                "-qb",
                "quick/demo",
                d.repo.to_str().unwrap(),
            ],
        );
        d.set("eval-verdict", "STUCK");
        d.set("review.json", &json!({"crusty_verdict":"CONCERNS", "concerns":[{
            "id":"docs-typo", "severity":"minor", "summary":"Typo in example", "evidence":"README.md:4"
        }]}).to_string());
        for (file, id) in [
            ("guard.sh", "step-00-review-dir-guard"),
            ("context.sh", "step-01-round-context"),
            ("guard-check.sh", "step-02b-review-dir-unchanged"),
            ("classify.sh", "step-03-extract-crusty-verdict"),
            ("record.sh", "step-06-write-round-record"),
        ] {
            d.set(file, &step_command(ROUND, id));
        }
        // Agent boundary: review prose and next-action judgement are supplied by
        // the fixture. The prompt itself is checked by the regression module.
        d.set("round.sh", boundary::ROUND_SCRIPT);
        d.set("action", "complete");
        d.set("round-rc", "0");
        d
    }

    pub fn set(&self, name: &str, text: &str) {
        private_file(&self.tmp.path().join(name), text);
    }

    pub fn git(&self, dir: &Path, args: &[&str]) -> String {
        let o = amplihack_git::command_in(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {}", show(&o));
        stdout(&o).trim().to_string()
    }

    pub fn cmd(&self, program: impl AsRef<OsStr>) -> Command {
        let mut c = crate::isolated_amplihack::isolated(program);
        c.current_dir(&self.review)
            .env("PATH", self.fakes.path_with_fakes())
            .env("REAL_AMPLIHACK", amplihack_bin())
            .env("AMPLIHACK_BIN", self.fakes.bin.join("amplihack"))
            .env("AMPLIHACK_HOME", repo_root())
            .env("FIXTURE", self.tmp.path())
            .env("FAKE_LOG", &self.fakes.log)
            .env("FAKE_GH_HEAD", self.head())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("AMPLIHACK_SESSION_DEPTH", "2")
            .env("AMPLIHACK_MAX_DEPTH", "6");
        c
    }

    pub fn head(&self) -> String {
        self.git(&self.repo, &["rev-parse", "HEAD"])
    }
    pub fn p(&self, name: &str) -> PathBuf {
        self.state.join(name)
    }
    pub fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.p(name)).unwrap_or_default()
    }
    pub fn evaluators(&self) -> Vec<Value> {
        std::fs::read_to_string(self.fakes.log.join("evaluator.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    pub fn rounds(&self) -> Vec<String> {
        std::fs::read_to_string(self.fakes.log.join("rounds"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
    pub fn marked(&self) -> bool {
        self.read("phases.tsv")
            .lines()
            .any(|s| s.starts_with("crusty-loop\t"))
    }
    pub fn helper(&self, op: &str, args: &[&OsStr]) -> Output {
        self.cmd(amplihack_bin())
            .args(["orch", "helper", &format!("autodrive-completion-{op}")])
            .args(args)
            .output()
            .unwrap()
    }
    pub fn script(&self, script: &str) -> Command {
        let mut c = self.cmd("bash");
        c.args(["-c", script]);
        c
    }

    /// Construct authentic round evidence through the YAML commands, then do
    /// exactly the preserved-copy/manifest work owned by the loop driver.
    pub fn record(&self, n: usize) -> PathBuf {
        let label = format!("round-{n}");
        let record = self.p(&format!("crusty-{label}.json"));
        let generation = self.review_generation();
        let o = self
            .cmd("bash")
            .arg(self.tmp.path().join("round.sh"))
            .env("AUTODRIVE_ROUND_LABEL", &label)
            .env("AUTODRIVE_ROUND_RECORD", &record)
            .env("AUTODRIVE_LAUNCH_TOOLS", tools_dir())
            .env("REVIEW_DIR", &generation)
            .env("REVIEWED_HEAD_SHA", self.head())
            .env("REPO_PATH", &self.repo)
            .env("PR_NUMBER", "7")
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "fixture must produce valid evidence: {}",
            show(&o)
        );
        let body = std::fs::read_to_string(&record).unwrap();
        private_file(&self.p("crusty-latest.json"), &body);
        private_file(
            &self.p("crusty-latest.json.findings"),
            &std::fs::read_to_string(record.with_extension("json.findings")).unwrap(),
        );
        let hash = self.git(
            &self.state,
            &["hash-object", "--no-filters", record.to_str().unwrap()],
        );
        let mut manifest = self.read("crusty-records.tsv");
        manifest.push_str(&format!("{label}\tcrusty-{label}.json\t{hash}\n"));
        private_file(&self.p("crusty-records.tsv"), &manifest);
        record
    }

    /// A private review generation of the PR worktree at its current head, as
    /// the loop allocates for a round (#2096); step-06 binds the record to it.
    pub fn review_generation(&self) -> PathBuf {
        let parent = self.tmp.path().join("review-generations");
        std::fs::create_dir_all(&parent).unwrap();
        let dest = parent.join(format!("g{}", std::fs::read_dir(&parent).unwrap().count()));
        let o = self
            .cmd(amplihack_bin())
            .args(["internal", "review-checkout"])
            .arg(&self.repo)
            .arg(&dest)
            .arg(self.head())
            .output()
            .unwrap();
        assert!(o.status.success(), "review-checkout: {}", show(&o));
        PathBuf::from(stdout(&o).trim())
    }

    pub fn prepare(&self, n: usize, history: &str) -> Value {
        let record = self.p(&format!("crusty-round-{n}.json"));
        let context = self.p("evaluator-context.json");
        private_file(&context, &json!({
            "loop_name":"auto-drive:crusty", "loop_round_label":format!("round-{n}"),
            "loop_history":history, "loop_repo_path":self.repo, "loop_agent_working_dir":self.review,
            "loop_baseline_ref":self.head(), "loop_child_exit_code":"0",
            "loop_findings_current":self.read(&format!("crusty-round-{n}.json.findings")),
            "loop_findings_previous":"", "loop_test_signal":"", "loop_test_signal_previous":"",
            "loop_ci_signal":"", "loop_ci_signal_previous":"", "loop_clean_token":"CLEAN",
            "loop_round_verdict":"CONCERNS", "loop_last_round_output":"fixture review"
        }).to_string());
        let label = format!("round-{n}");
        let o = self.helper(
            "prepare",
            &[
                OsStr::new("--state-dir"),
                self.state.as_os_str(),
                OsStr::new("--round-label"),
                OsStr::new(&label),
                OsStr::new("--record"),
                record.as_os_str(),
                OsStr::new("--repo"),
                self.repo.as_os_str(),
                OsStr::new("--review-dir"),
                self.review.as_os_str(),
                OsStr::new("--launch-tools"),
                tools_dir().as_os_str(),
                OsStr::new("--evaluator-context-file"),
                context.as_os_str(),
            ],
        );
        assert!(
            o.status.success(),
            "Rust prepare helper is required: {}",
            show(&o)
        );
        json_output(&o)
    }

    pub fn write(&self, prepared: &Value, action: &str, reason: &str) -> Output {
        self.helper(
            "write",
            &[
                OsStr::new("--private-state"),
                OsStr::new(prepared["private_state"].as_str().unwrap()),
                OsStr::new("--action"),
                OsStr::new(action),
                OsStr::new("--reason"),
                OsStr::new(reason),
            ],
        )
    }
    pub fn dispatch(&self, prepared: &Value, rc: i32) -> Output {
        self.helper(
            "dispatch",
            &[
                OsStr::new("--private-state"),
                OsStr::new(prepared["private_state"].as_str().unwrap()),
                OsStr::new("--child-exit-code"),
                OsStr::new(&rc.to_string()),
            ],
        )
    }
    pub fn decide(&self, prepared: &Value, action: &str) {
        let o = self.write(
            prepared,
            action,
            "Current review evidence determines the next action",
        );
        assert!(o.status.success(), "Rust writer: {}", show(&o));
    }

    pub fn run_loop(&self) -> Output {
        let script = step_command("autodrive-crusty-loop", "step-02-crusty-loop");
        self.script(&script)
            .env("REPO_PATH", &self.review)
            .env(
                "RECIPE_VAR_crusty_loop_preflight",
                json!({
                    "should_run":"true", "repo":self.repo, "pr":"7", "state_dir":self.state
                })
                .to_string(),
            )
            .env("RECIPE_VAR_task_description", "Fix issue #2336")
            .env("PR_URL", "https://github.com/o/r/pull/7")
            .output()
            .unwrap()
    }

    pub fn finalize(&self, dry_run: bool, fail: bool) -> Output {
        let script = step_command("autodrive-crusty-loop", "step-03-record-crusty-phase");
        let mut c = self.script(&script);
        c.env("REPO_PATH", &self.repo)
            .env(
                "RECIPE_VAR_crusty_loop_preflight",
                json!({"should_run":"true","pr":"7","state_dir":self.state}).to_string(),
            )
            .env(
                "RECIPE_VAR_crusty_loop_result",
                self.read("crusty-result.json"),
            )
            .env("RECIPE_VAR_plan_task_id", "#2336")
            .env("RECIPE_VAR_task_description", "Fix issue #2336")
            .env("AUTODRIVE_DRY_RUN", if dry_run { "true" } else { "false" });
        if fail {
            c.env("FAKE_GH_FAIL", "issue create");
        }
        c.output().unwrap()
    }

    pub fn rereview(&self, fail: bool) -> Output {
        let mut c = self.cmd("bash");
        c.arg(tools_dir().join("autodrive_crusty_rereview.sh"))
            .arg("--repo")
            .arg(&self.repo)
            .arg("--review-dir")
            .arg(&self.review)
            .arg("--state-dir")
            .arg(&self.state)
            .args(["-c", "pr_number=7", "-c", "plan_task_id=#2336"])
            .arg("--context-from-file")
            .arg(format!("task_description={}", self.task.display()));
        if fail {
            c.env("FAKE_GH_FAIL", "issue create");
        }
        c.output().unwrap()
    }

    pub fn enable_rereview(&self) {
        self.set("review.json", r#"{"crusty_verdict":"CLEAN","concerns":[]}"#);
        self.record(1);
        private_file(
            &self.p("crusty-result.json"),
            r#"{"loop":"crusty","loop_result":"DONE","round_label":"round-1","round_verdict":"CLEAN"}"#,
        );
        let o = self.finalize(false, false);
        assert!(
            o.status.success() && self.marked(),
            "baseline completion: {}",
            show(&o)
        );
        std::fs::write(self.repo.join("code.rs"), "fn new_code() {}\n").unwrap();
        self.git(&self.repo, &["add", "code.rs"]);
        self.git(&self.repo, &["commit", "-qm", "code needs re-review"]);
        self.set("review.json", r#"{"crusty_verdict":"CONCERNS","concerns":[{"id":"docs-typo","severity":"minor","summary":"Typo","evidence":"README.md:4"}]}"#);
    }
}

pub fn json_output(o: &Output) -> Value {
    stdout(o)
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str(l).ok())
        .unwrap_or_else(|| panic!("expected structured output: {}", show(o)))
}

pub fn decision_file(p: &Value) -> PathBuf {
    PathBuf::from(p["decision_file"].as_str().unwrap())
}

pub fn assert_complete(d: &Drive, o: &Output) {
    assert!(
        o.status.success(),
        "completed objective must advance: {}",
        show(o)
    );
    let v = json_output(o);
    assert_eq!(v["loop_result"], "DONE", "{}", show(o));
    assert_eq!(v["source"], "objective-completion", "{}", show(o));
    assert_eq!(v["evaluator_invoked"], false, "{}", show(o));
    assert!(
        d.evaluators().is_empty(),
        "completion must not launch an evaluator"
    );
    assert!(
        !d.marked(),
        "dispatch ends iteration; follow-up finalization marks the phase"
    );
}
