//! Test-only inspection and synthetic owner preparation for the three checkpoints.
//! The phase projection replaces only finite model scope judgement. It provides
//! no native runner, model review or delivery acceptance evidence.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Follow only the five authorised complete command extractions. A removed or
/// retargeted wrapper fails before the extracted body can satisfy an assertion.
pub fn inspect(id: &str, wrapper: &str) -> String {
    let (helper, invocation) = match id {
        "step-15-commit-push" => (
            "workflow_publish_checkpoint.sh",
            "bash \"$CHECKPOINT_TOOL\"",
        ),
        "step-18c-push-feedback-changes" => {
            ("workflow_review_checkpoint.sh", "bash \"$CHECKPOINT_TOOL\"")
        }
        "step-20b-push-cleanup" => (
            "workflow_cleanup_checkpoint.sh",
            "bash \"$CHECKPOINT_TOOL\"",
        ),
        "step-00-workflow-preparation" => ("workflow_prepare_banner.sh", "bash \"$PREP_TOOL\""),
        "step-01-round-context" => (
            "autodrive_round_context.sh",
            "bash \"/${AUTODRIVE_LAUNCH_TOOLS#/}/autodrive_round_context.sh\"",
        ),
        _ => return wrapper.to_owned(),
    };
    let expected = match id {
        "step-15-commit-push" => {
            r###"set -euo pipefail
CHECKPOINT_TOOL=""
for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do
  if [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/workflow_publish_checkpoint.sh" ]; then CHECKPOINT_TOOL="$c/amplifier-bundle/tools/workflow_publish_checkpoint.sh"; break; fi
done
[ -n "$CHECKPOINT_TOOL" ] || { echo 'ERROR: bounded checkpoint helper not found' >&2; exit 2; }
checkpoint_worktree="$(python3 -I -c 'import json,os; s=json.loads(os.environ["RECIPE_VAR_worktree_setup"]); print(s["worktree_path"])')"
cd -P "${checkpoint_worktree:?missing owned worktree}"
bash "$CHECKPOINT_TOOL"
"###
        }
        "step-18c-push-feedback-changes" => {
            r###"set -euo pipefail
CHECKPOINT_TOOL=""
for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do
  if [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/workflow_review_checkpoint.sh" ]; then CHECKPOINT_TOOL="$c/amplifier-bundle/tools/workflow_review_checkpoint.sh"; break; fi
done
[ -n "$CHECKPOINT_TOOL" ] || { echo 'ERROR: bounded checkpoint helper not found' >&2; exit 2; }
checkpoint_worktree="$(python3 -I -c 'import json,os; s=json.loads(os.environ["RECIPE_VAR_worktree_setup"]); print(s["worktree_path"])')"
cd -P "${checkpoint_worktree:?missing owned worktree}"
bash "$CHECKPOINT_TOOL"
"###
        }
        "step-20b-push-cleanup" => {
            r###"set -euo pipefail
CHECKPOINT_TOOL=""
for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do
  if [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/workflow_cleanup_checkpoint.sh" ]; then CHECKPOINT_TOOL="$c/amplifier-bundle/tools/workflow_cleanup_checkpoint.sh"; break; fi
done
[ -n "$CHECKPOINT_TOOL" ] || { echo 'ERROR: bounded checkpoint helper not found' >&2; exit 2; }
bash "$CHECKPOINT_TOOL"
"###
        }
        "step-00-workflow-preparation" => {
            r###"PREP_TOOL=""
for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do
  if [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/workflow_prepare_banner.sh" ]; then PREP_TOOL="$c/amplifier-bundle/tools/workflow_prepare_banner.sh"; break; fi
done
[ -n "$PREP_TOOL" ] || { echo 'ERROR: workflow preparation helper not found' >&2; exit 2; }
bash "$PREP_TOOL"
"###
        }
        "step-01-round-context" => {
            r###"bash "/${AUTODRIVE_LAUNCH_TOOLS#/}/autodrive_round_context.sh"
"###
        }
        _ => unreachable!(),
    };
    assert_eq!(wrapper, expected, "{id}: exact extraction wrapper changed");
    assert!(
        wrapper.contains(&format!("/{helper}")),
        "{id}: missing exact helper path"
    );
    assert_eq!(
        wrapper
            .lines()
            .filter(|line| line.trim() == invocation)
            .count(),
        1,
        "{id}: missing or altered extraction invocation"
    );
    fs::read_to_string(root().join("amplifier-bundle/tools").join(helper))
        .unwrap_or_else(|e| panic!("read extracted {helper}: {e}"))
}

pub fn inspect_recipe(mut recipe: serde_yaml::Value) -> serde_yaml::Value {
    for step in recipe["steps"].as_sequence_mut().into_iter().flatten() {
        if let (Some(id), Some(body)) = (step["id"].as_str(), step["command"].as_str()) {
            step["command"] = serde_yaml::Value::String(inspect(id, body));
        }
    }
    recipe
}

/// Check the real shared strict call as well as each checkpoint's callsite.
pub fn assert_strict_staging(body: &str) {
    assert!(
        body.lines()
            .any(|line| line.starts_with("workflow_checkpoint_stage; "))
    );
    let common =
        fs::read_to_string(root().join("amplifier-bundle/tools/workflow_publish_checkpoint.sh"))
            .unwrap();
    let stage = common
        .split("workflow_checkpoint_stage() {\n")
        .nth(1)
        .unwrap()
        .split("\n}\n")
        .next()
        .unwrap();
    assert!(stage.contains("workflow_checkpoint_check || return $?"));
    assert!(stage.contains("workflow_checkpoint_python paths >\"$list\""));
    assert!(stage.contains("stage_workflow_changes --strict --path-list \"$list\" || rc=$?"));
    assert!(!stage.contains("git add"));
}

/// Declare this value before creating or editing any task source. No Git
/// observation, staged path or expected outcome contributes path permission.
pub struct Owner {
    pub task: &'static str,
    pub paths: &'static [&'static str],
}

impl Owner {
    pub fn project(
        &self,
        command: &Command,
        asset: &str,
        parent: &Path,
        worktree: &Path,
        branch: &str,
        local_branch: &str,
    ) -> Output {
        let evidence = tempfile::tempdir().expect("private projection evidence");
        let worktree = fs::canonicalize(worktree).expect("canonical fixture worktree");
        let request = serde_json::json!({
            "asset": asset, "fixture_owner_paths": self.paths,
            "root": root(), "evidence": evidence.path(),
            "context": {"task_description": self.task, "final_requirements": self.task,
                "repo_path": fs::canonicalize(parent).unwrap(),
                "worktree_setup": {"worktree_path": worktree, "branch_name": branch, "local_branch": local_branch,
                    "base_ref": "refs/remotes/origin/main"},
                "version_bump": {"change_classification": "PATCH"}}
        });
        let mut projection = Command::new("python3");
        projection
            .env_clear()
            .env("HOME", evidence.path())
            .env("AMPLIHACK_TEST_HOME_GUARD", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null());
        // Preserve explicit negative selectors. Isolated fixtures supply their
        // own environment; this utility never removes an explicit selector.
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                projection.env(key, value);
            }
        }
        projection
            .env("TASK_DESCRIPTION", self.task)
            .env("TMPDIR", evidence.path())
            .arg("-I")
            .arg("-c")
            .arg(PROJECTION)
            .arg(request.to_string())
            .output()
            .expect("run bounded test-only phase projection")
    }
}

const PROJECTION: &str = r#"
import importlib.util, json, os, pathlib, subprocess, sys, unittest
request = json.loads(sys.argv[1])
path = pathlib.Path(request['root']) / 'amplifier-bundle/recipes/tests/test-checkpoint-scope.py'
spec = importlib.util.spec_from_file_location('checkpoint_contract', path)
contract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)
class Fixture:
    case = unittest.TestCase()
    def __init__(self):
        self.context = request['context']
        self.task = pathlib.Path(self.context['worktree_setup']['worktree_path'])
        self.branch = self.context['worktree_setup']['local_branch']
        self.owner = self.context['task_description']
        self.allowed = {os.fsencode(p) for p in request['fixture_owner_paths']}
        self.env = dict(os.environ)
        self.sync_context()
    def sync_context(self):
        for k, v in self.context.items():
            self.env['RECIPE_VAR_' + k] = json.dumps(v) if isinstance(v, (dict, list, bool)) else str(v)
    def git(self, repo, *args):
        r = subprocess.run(['git', '-C', str(repo), *args], env=self.env, capture_output=True)
        self.case.assertEqual(r.returncode, 0, r.stderr)
        return r.stdout
    def shell(self, body):
        return subprocess.run(['bash', '-c', body], cwd=self.task, env=self.env, capture_output=True)
f = Fixture()
phase = next(p for p in contract.PHASES if p[0] == request['asset'])
r = contract.PhaseProjection(f, phase).run()
sys.stdout.buffer.write(r.stdout)
sys.stderr.buffer.write(r.stderr)
sys.exit(r.returncode)
"#;

/// Execute the actual redaction function without running its publication phase.
/// Failure messages deliberately contain no disposable URI userinfo or output.
pub fn assert_uri_redaction(command: &str, name: &str) {
    let heading = format!("{name}() {{\n");
    assert_eq!(
        command.matches(heading.as_str()).count(),
        1,
        "one actual redactor is required"
    );
    let body = command
        .split_once(heading.as_str())
        .unwrap()
        .1
        .split_once("\n}")
        .unwrap()
        .0;
    let invocation = match name {
        "redact_sensitive_file" => r#"redact_sensitive_file "$1""#,
        "redact_sensitive_output" => r#"redact_sensitive_output < "$1""#,
        _ => panic!("unsupported fixture redactor"),
    };
    let script = format!("{heading}{body}\n}}\n{invocation}\n");
    let tmp = tempfile::tempdir().expect("private redaction fixture");
    let input = tmp.path().join("input.txt");
    for scheme in ["http", "https", "ssh", "git+ssh", "ftp", "file"] {
        fs::write(&input, format!("{scheme}://fixture-user:disposable@host.example/path?keep=1\n{scheme}://host.example/plain\n")).unwrap();
        let output = Command::new("/bin/bash")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", tmp.path())
            .env("TMPDIR", tmp.path())
            .current_dir(tmp.path())
            .args(["-c", &script, "redaction-fixture"])
            .arg(&input)
            .output()
            .expect("run actual redactor");
        assert!(output.status.success(), "redactor failed for {scheme}");
        let expected = format!(
            "{scheme}://REDACTED@host.example/path?keep=1\n{scheme}://host.example/plain\n"
        );
        assert!(
            output.stdout.as_slice() == expected.as_bytes(),
            "userinfo redaction or unchanged plain URI failed for {scheme}"
        );
    }
}
