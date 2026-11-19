//! Local bare "GitHub" remote on top of the #1641 fixture.

use std::fs;

use serde_json::{Value, json};

use crate::issue_1641_finalization_open_pr::builders::{check_run, open_pr};
use crate::issue_1641_finalization_open_pr::fixture::{
    BRANCH, Fixture, Outcome, PR_URL, workspace_root, write_executable,
};

/// Phrase the old transport emitted for absence. Its presence on stderr means
/// absence was still being treated as invalidity.
pub const INVALID_CONTEXT_WARNING: &str = "invalid authoritative workflow context";

impl Fixture {
    /// Create the bare repo that `https://github.com/rysweet/fixture.git`
    /// resolves to. Does not push.
    pub fn with_remote(&self) {
        let bare = self.remotes().join("rysweet/fixture.git");
        fs::create_dir_all(&bare).expect("bare dir");
        let mut command = amplihack_git::command();
        self.isolate(&mut command);
        let out = command
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&bare)
            .output()
            .expect("git init --bare");
        assert!(out.status.success(), "bare init failed");
    }

    /// Push the run branch to the fake remote at the current local head.
    pub fn push(&self) {
        self.git(&["push", "-q", "origin", BRANCH]);
    }

    /// Commit a new file so local HEAD moves ahead of whatever was pushed.
    pub fn commit_more(&self, name: &str) -> String {
        fs::write(self.repo.join(name), "more\n").expect("write file");
        self.git(&["add", name]);
        self.git(&["commit", "-q", "-m", name]);
        self.head()
    }

    /// A commit object that is neither the local head nor related to it.
    pub fn unrelated_commit(&self) -> String {
        self.git(&["commit-tree", "HEAD^{tree}", "-m", "unrelated"])
            .trim()
            .to_owned()
    }

    /// Canned `gh pr view` for PR #7 with an explicit head and rollup, plus the
    /// matching `gh pr list` answer. The list is what the check falls back to
    /// when the run carries no PR reference of its own, as it does when the
    /// context file is unreadable and the legacy `PR_URL` alias is suppressed.
    pub fn gh_pr_at(&self, head: &str, rollup: Value) {
        let mut body = open_pr(rollup);
        body["headRefOid"] = json!(head);
        body["isDraft"] = json!(false);
        self.gh_view(&body);
        self.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
    }

    pub fn gh_no_pr(&self) {
        self.gh_list(&json!([]));
        self.gh_fail("view", 1);
    }
}

pub fn pending_rollup() -> Value {
    json!([check_run("IN_PROGRESS", Value::Null)])
}

/// `green_rollup` under a name the local modules can import without pulling
/// the whole #1641 builder surface into scope.
pub fn green_rollup_for_tests() -> Value {
    crate::issue_1641_finalization_open_pr::builders::green_rollup()
}

pub fn failing_rollup() -> Value {
    json!([check_run("COMPLETED", json!("FAILURE"))])
}

/// A run's env with a PR URL and no implementation/verification outputs and no
/// context file selected.
pub fn finished_env(repo: &str) -> Vec<(&'static str, String)> {
    let mut envs = crate::issue_1641_finalization_open_pr::builders::resumed_env(repo);
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs
}

/// A credential-bearing origin URL. `parse_github_repo_identity` accepts
/// `https://<userinfo>@github.com/<owner>/<repo>`, so the secret reaches the
/// outcome reader and must be discarded there rather than reported.
pub const ORIGIN_USERINFO: &str = "ci-bot:ghp-s3cr3t-2343";

impl Fixture {
    /// Point `origin` at `url` without touching anything else.
    pub fn set_origin(&self, url: &str) {
        self.git(&["remote", "set-url", "origin", url]);
    }

    /// `origin` as a github.com URL carrying userinfo.
    pub fn origin_with_userinfo(&self) {
        self.set_origin(&format!(
            "https://{ORIGIN_USERINFO}@github.com/rysweet/fixture.git"
        ));
    }

    /// Make every `gh pr view`/`pr list` fail with credential-bearing stderr.
    pub fn gh_unreachable(&self) {
        self.gh_fail("view", 1);
        self.gh_fail("list", 1);
    }
}

/// A run whose assessment decided `terminal_state` for pull request `pr`.
pub fn decided(fx: &Fixture, terminal_state: &str, pr: &str) -> Vec<(&'static str, String)> {
    let mut envs = finished_env(&fx.repo_path());
    envs.push(
        crate::issue_1641_finalization_open_pr::builders::assessment(terminal_state, "none", pr),
    );
    envs
}

/// The same, for a run that recorded no pull request of its own.
pub fn decided_resumed(
    fx: &Fixture,
    terminal_state: &str,
    pr: &str,
) -> Vec<(&'static str, String)> {
    let mut envs = crate::issue_1641_finalization_open_pr::builders::resumed_env(&fx.repo_path());
    envs.push(
        crate::issue_1641_finalization_open_pr::builders::assessment(terminal_state, "none", pr),
    );
    envs
}

/// Where `stub_step_recovery_driver` records the calls it was given.
pub const DRIVER_LOG: &str = "driver-calls.log";

/// A stand-in `amplihack` for the fixture's PATH. It records every argv it was
/// called with, then runs whatever followed `--`, the way `step-recovery run`
/// runs the body it was handed. With it in place a step's real `command:` can
/// be executed and the call it makes to the driver observed.
const DRIVER_STUB: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH_FIXTURE_DIR/driver-calls.log"
for ((i = 1; i <= $#; i++)); do
  if [ "${!i}" = "--" ]; then shift "$i"; exec "$@"; fi
done
exit 0
"#;

impl Fixture {
    pub fn stub_step_recovery_driver(&self) {
        write_executable(&self.tmp.path().join("bin/amplihack"), DRIVER_STUB);
    }

    /// Every argv the stub recorded, in call order.
    pub fn driver_calls(&self) -> Vec<String> {
        fs::read_to_string(self.tmp.path().join(DRIVER_LOG))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Run `command` under bash with the fixture's isolation and `envs`, the
    /// way the recipe runner runs a step's `command:`.
    pub fn run_step(&self, command: &str, envs: &[(&str, &str)]) -> Outcome {
        let mut process = std::process::Command::new("bash");
        self.isolate(&mut process);
        process
            .arg("-c")
            .arg(command)
            .arg("step")
            .env("AMPLIHACK_HOME", workspace_root());
        for (key, value) in envs {
            process.env(key, value);
        }
        let output = process.output().expect("run the step command");
        Outcome {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            output,
        }
    }
}

impl Fixture {
    /// Run `workflow_agentic_finalization.sh collect [--base <base>]` with
    /// exactly `envs`, and parse the one evidence object it writes to stdout.
    /// This is the step whose output `assess-finalization` reads, so a case
    /// that asks what the deciding agent is told drives this rather than
    /// writing the evidence by hand.
    pub fn collect(&self, envs: &[(&str, &str)], base: Option<&str>) -> (Value, Outcome) {
        let mut command = std::process::Command::new("bash");
        self.isolate(&mut command);
        command
            .arg(crate::issue_1641_finalization_open_pr::fixture::tool_path(
                "workflow_agentic_finalization.sh",
            ))
            .arg("collect");
        if let Some(base) = base {
            command.arg("--base").arg(base);
        }
        for (key, value) in envs {
            command.env(key, value);
        }
        let output = command.output().expect("run collect-finalization-evidence");
        let outcome = Outcome {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            output,
        };
        let value: Value = serde_json::from_str(outcome.stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "collect-finalization-evidence must print one JSON object: {e}\n{}",
                outcome.describe()
            )
        });
        (value, outcome)
    }

    /// Give `origin` a default branch of `name`: create it, push it, and point
    /// `refs/remotes/origin/HEAD` at it, the way a clone of a repository whose
    /// default branch is `name` looks.
    pub fn with_default_branch(&self, name: &str) {
        let here = self
            .git(&["rev-parse", "--abbrev-ref", "HEAD"])
            .trim()
            .to_owned();
        self.git(&["checkout", "-q", "-B", name]);
        self.git(&["push", "-q", "origin", name]);
        self.git(&["checkout", "-q", &here]);
        self.git(&["remote", "set-head", "origin", name]);
    }
}
