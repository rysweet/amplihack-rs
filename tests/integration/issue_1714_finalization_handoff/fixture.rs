//! Shared fixture for the #1714 tests.
//!
//! - [`validate`] runs the real `workflow_agentic_finalization.sh` against the
//!   #1641 fixture (private git repo, stub `gh` that refuses every mutation).
//! - [`Step03`] runs autodrive-build's `step-03-resolve-pr` exactly as the
//!   recipe holds it, with `AMPLIHACK_HOME` at this worktree, a stub `gh`
//!   answering `pr view <n> --json state,mergedAt|url` from canned files, and
//!   a stub `amplihack` for the two `orch helper` calls.
//! - [`brick`] sources `recipe_var.sh` and the new brick and runs a snippet.
//!
//! Every `bash` child starts from `isolated(..)` (via `bash_in` or
//! [`fixture_bash`]), so it gets `HOME_GUARD_ENV` and `ROTATE_AUTOSTART_ENV`
//! and no ambient `RECIPE_VAR_*` or credential from an enclosing run;
//! `static_guards.rs` refuses a bash `Command` built any other way here.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use serde_yaml::Value as Yaml;
use tempfile::TempDir;

use crate::isolated_amplihack::isolated;
use crate::issue_1641_finalization_open_pr::fixture::LEAK_MARKER;
pub use crate::issue_1641_finalization_open_pr::fixture::{
    BRANCH, Fixture, Outcome, PR_URL, as_refs, check_run, green_rollup, open_pr, resumed_env,
};

pub const BRICK: &str = "workflow_finalization_handoff.sh";
pub const HANDOFF_INFO: &str = "INFO: #1714: BLOCKED_CI handed to the merge-ready loop";
pub const FI_HANDOFF_INFO: &str =
    "INFO: #1714: FAILED_IMPLEMENTATION handed to the merge-ready loop";
pub const PR134_URL: &str = "https://github.com/acme/widget/pull/134";

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

pub fn tool_path(name: &str) -> PathBuf {
    workspace_root().join("amplifier-bundle/tools").join(name)
}

pub fn recipe_path(name: &str) -> PathBuf {
    workspace_root().join(format!("amplifier-bundle/recipes/{name}.yaml"))
}

pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

pub fn recipe_yaml(name: &str) -> Yaml {
    serde_yaml::from_str(&read(&recipe_path(name)))
        .unwrap_or_else(|e| panic!("parse {}: {e}", recipe_path(name).display()))
}

pub fn step<'a>(recipe: &'a Yaml, id: &str) -> &'a Yaml {
    recipe["steps"]
        .as_sequence()
        .expect("recipe must declare steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing step `{id}`"))
}

fn outcome(command: &mut Command) -> Outcome {
    let output = command.output().expect("spawn bash");
    Outcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        output,
    }
}

pub fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).expect("write stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
    }
}

fn parent_path() -> String {
    std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
}

/// `bash` from `isolated(..)` with HOME, TMPDIR and PATH under `tmp`.
fn bash_in(tmp: &Path) -> Command {
    let mut command = isolated("bash");
    command
        .env("HOME", tmp.join("home"))
        .env("TMPDIR", tmp.join("scratch"))
        .env("XDG_CONFIG_HOME", tmp.join("home/.config"))
        .env("GH_CONFIG_DIR", tmp.join("home/.config/gh"))
        .env("AMPLIHACK_STATE_DIR", tmp.join("state"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        // github.com fetches (the open-PR check's, #1714) stay in the fixture.
        .env("GIT_CONFIG_COUNT", "1")
        .env(
            "GIT_CONFIG_KEY_0",
            format!("url.{}/.insteadOf", tmp.join("remotes").display()),
        )
        .env("GIT_CONFIG_VALUE_0", "https://github.com/")
        .env("GH_FIXTURE_DIR", tmp)
        .env(
            "PATH",
            format!("{}:{}", tmp.join("bin").display(), parent_path()),
        );
    command
}

/// `bash` from `isolated(..)` with the #1641 fixture's environment: its
/// private HOME, stub `gh`, github.com rewritten to its private remotes, and
/// the leak marker its `gh` stub echoes on failure. Runs in the fixture repo.
pub fn fixture_bash(fx: &Fixture) -> Command {
    let mut command = bash_in(fx.tmp.path());
    command
        .current_dir(&fx.repo)
        .env("GH_LEAK_MARKER", LEAK_MARKER);
    command
}

/// Run the finalization dispatcher in `mode` with exactly `envs`.
pub fn dispatch(fx: &Fixture, mode: &str, envs: &[(&str, &str)]) -> Outcome {
    let mut command = bash_in(fx.tmp.path());
    command
        .current_dir(&fx.repo)
        .env("GH_LEAK_MARKER", "unused-1714")
        .arg(tool_path("workflow_agentic_finalization.sh"))
        .arg(mode);
    for (key, value) in envs {
        command.env(key, value);
    }
    outcome(&mut command)
}

pub fn validate(fx: &Fixture, envs: &[(&str, &str)]) -> Outcome {
    dispatch(fx, "validate", envs)
}

/// Source `recipe_var.sh` and the brick, then run `script` with `envs`.
pub fn brick(script: &str, envs: &[(&str, &str)]) -> Outcome {
    let tmp = TempDir::new().expect("tempdir");
    for dir in ["home", "scratch", "bin", "state"] {
        fs::create_dir_all(tmp.path().join(dir)).expect("fixture dir");
    }
    let mut command = bash_in(tmp.path());
    command
        .current_dir(tmp.path())
        .arg("-c")
        .arg(format!(r#". "$1"; . "$2"; shift 2; {script}"#))
        .arg("brick")
        .arg(tool_path("recipe_var.sh"))
        .arg(tool_path(BRICK));
    for (key, value) in envs {
        command.env(key, value);
    }
    outcome(&mut command)
}

const GH_STUB: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH_FIXTURE_DIR/gh-calls.log"
case "$*" in
  "pr view "[0-9]*" --json state,mergedAt"*)
    f="$GH_FIXTURE_DIR/state-$3"; s=OPEN
    if [ -f "$f" ]; then
      s="$(head -n 1 "$f")"; rest="$(tail -n +2 "$f")"
      [ -z "$rest" ] || printf '%s\n' "$rest" > "$f"
    fi
    [ "$s" != FAIL ] || exit 1
    printf '{"state":"%s","mergedAt":null}\n' "$s" ;;
  "pr view "[0-9]*" --json url"*) printf 'https://github.com/acme/widget/pull/%s\n' "$3" ;;
  "pr list "*) printf '' ;;
  *) exit 1 ;;
esac
"#;

const AMPLIHACK_STUB: &str = r#"#!/usr/bin/env bash
[ "${1:-} ${2:-}" = "orch helper" ] || { echo "ampstub: unsupported: $*" >&2; exit 2; }
sub="${3:-}"; shift 3; field=""; def=""
while [ $# -gt 0 ]; do
  case "$1" in --require-field|--field) field="${2:-}"; shift 2 ;; --default) def="${2:-}"; shift 2 ;; *) shift ;; esac
done
in="$(cat)"
case "$sub" in
  extract-json) printf '%s' "$in" | jq -e --arg f "$field" 'has($f)' >/dev/null 2>&1 || exit 1; printf '%s\n' "$in" ;;
  extract-field) printf '%s' "$in" | jq -r --arg f "$field" --arg d "$def" '(.[$f] // $d) | tostring' ;;
esac
"#;

/// autodrive-build step-03's command body, as the runner would run it.
pub fn step03_command() -> String {
    step(&recipe_yaml("autodrive-build"), "step-03-resolve-pr")["command"]
        .as_str()
        .expect("step-03 is a bash step with a command")
        .to_owned()
}

/// A private directory for one step-03 run.
pub struct Step03 {
    pub tmp: TempDir,
    pub amplihack_home: PathBuf,
}

impl Step03 {
    pub fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        for dir in ["home", "scratch", "bin", "state", "repo"] {
            fs::create_dir_all(tmp.path().join(dir)).expect("fixture dir");
        }
        write_exec(&tmp.path().join("bin/gh"), GH_STUB);
        write_exec(&tmp.path().join("bin/amplihack"), AMPLIHACK_STUB);
        Self {
            tmp,
            amplihack_home: workspace_root(),
        }
    }

    pub fn repo(&self) -> PathBuf {
        self.tmp.path().join("repo")
    }

    /// Canned `gh pr view <pr> --json state,mergedAt` answers, one per call;
    /// the last one repeats. `FAIL` makes gh exit 1.
    pub fn pr_states(&self, pr: &str, states: &[&str]) {
        fs::write(
            self.tmp.path().join(format!("state-{pr}")),
            states.join("\n") + "\n",
        )
        .expect("state file");
    }

    pub fn gh_calls(&self) -> Vec<String> {
        fs::read_to_string(self.tmp.path().join("gh-calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Run step-03 with a preflight that already knows `pr` ("" for none) and
    /// `workflow_result` (None leaves RECIPE_VAR_workflow_result unset).
    pub fn run(
        &self,
        pr: &str,
        workflow_result: Option<&Value>,
        extra: &[(&str, &str)],
    ) -> Outcome {
        let preflight = serde_json::json!({
            "should_run": "true", "pr": pr, "pr_url": "", "pr_state": if pr.is_empty() { "" } else { "OPEN" },
            "branch": "", "state_dir": "", "reason": ""
        })
        .to_string();
        let mut command = bash_in(self.tmp.path());
        command
            .current_dir(self.repo())
            .env("AMPLIHACK_HOME", &self.amplihack_home)
            .env("REPO_PATH", self.repo())
            .env("RECIPE_VAR_build_preflight", &preflight)
            .env("RECIPE_VAR_no_merge", "true")
            .arg("-c")
            .arg(step03_command());
        if let Some(result) = workflow_result {
            command.env("RECIPE_VAR_workflow_result", result.to_string());
        }
        for (key, value) in extra {
            command.env(key, value);
        }
        outcome(&mut command)
    }
}

/// The validator's hand-off result for PR 134, as workflow-finalize outputs it.
pub fn handoff_result(state: &str, pr: &str) -> Value {
    serde_json::json!({
        "terminal_success": "false", "terminal_failure": "true",
        "terminal_state": state, "pr_number": pr, "pr_url": PR134_URL,
        "finalizer_output_valid": "false", "handoff": "merge_ready_loop"
    })
}

pub fn assert_no_handoff_key(outcome: &Outcome, why: &str) {
    assert!(
        outcome.json().get("handoff").is_none(),
        "{why}: a result that is not handed off must have no `handoff` key\n{}",
        outcome.describe()
    );
}

/// Lines of `text` that are not shell comments.
pub fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|l| !l.trim_start().starts_with('#'))
}
