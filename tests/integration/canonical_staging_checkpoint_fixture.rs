//! Test-only inspection and synthetic owner preparation for the three checkpoints.
//! The phase projection replaces only finite model scope judgement. It provides
//! no native runner, model review or delivery acceptance evidence.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
checkpoint_worktree="$("${AMPLIHACK_BIN:-amplihack}" internal workflow-checkpoint worktree-path)"
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
checkpoint_worktree="$("${AMPLIHACK_BIN:-amplihack}" internal workflow-checkpoint worktree-path)"
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
    assert!(stage.contains("workflow_checkpoint paths >\"$list\""));
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
        // The request travels as JSON, as it did to the reference driver, so
        // the context keeps the key order the reference parsed.
        let request = crate::checkpoint_projection::py_json_loads(request.to_string().as_bytes())
            .expect("the projection request is JSON");
        let request = crate::checkpoint_projection::ProjectionRequest {
            root: root(),
            asset: asset.to_owned(),
            owner_paths: self.paths.iter().map(|p| p.as_bytes().to_vec()).collect(),
            context: request
                .get("context")
                .cloned()
                .expect("the request has a context"),
            branch_key: "local_branch",
        };
        // The projection's environment, which stands for the reference
        // driver's `os.environ`: a fixed isolated base, every value the
        // fixture set on `command` (explicit negative selectors included; this
        // utility never removes one), then the engine and the task values.
        let mut env: std::collections::BTreeMap<OsString, OsString> = [
            ("HOME", evidence.path().as_os_str()),
            ("AMPLIHACK_TEST_HOME_GUARD", OsStr::new("1")),
            ("AMPLIHACK_ROTATE_AUTOSTART", OsStr::new("0")),
            ("GIT_CONFIG_NOSYSTEM", OsStr::new("1")),
            ("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null")),
            ("GIT_ALLOW_PROTOCOL", OsStr::new("file")),
            ("GIT_TERMINAL_PROMPT", OsStr::new("0")),
        ]
        .into_iter()
        .map(|(k, v)| (OsString::from(k), v.to_owned()))
        .collect();
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                env.insert(key.to_owned(), value.to_owned());
            }
        }
        // The checkpoint engine is the real binary under test, whatever
        // `amplihack` stub a fixture puts on PATH (#1554). Only the test
        // targets of the binary package know its path. A fixture that
        // removed AMPLIHACK_BIN would otherwise count as having set it.
        assert!(
            !command
                .get_envs()
                .any(|(key, value)| key == "AMPLIHACK_BIN" && value.is_none()),
            "a fixture must set AMPLIHACK_BIN or leave it alone, never remove it"
        );
        if let Some(engine) = option_env!("CARGO_BIN_EXE_amplihack")
            && !command.get_envs().any(|(key, _)| key == "AMPLIHACK_BIN")
        {
            env.insert("AMPLIHACK_BIN".into(), engine.into());
        }
        env.insert("TASK_DESCRIPTION".into(), self.task.into());
        env.insert("TMPDIR".into(), evidence.path().into());
        let env: Vec<(OsString, Option<OsString>)> =
            env.into_iter().map(|(k, v)| (k, Some(v))).collect();
        let env = crate::checkpoint_projection::with_python_locale_coercion(&env);
        crate::checkpoint_projection::project(&request, &env).into()
    }
}

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
