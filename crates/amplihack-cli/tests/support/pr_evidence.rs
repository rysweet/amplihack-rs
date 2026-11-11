//! Issue #2333 black-box fixtures. Only external processes are controlled;
//! readiness, convergence and authorization remain production decisions.
//!
//! The loop fixture replaces the recipe invocation boundary, not the loop.
//! It does NOT exercise receipt registration or the final merge executor.

use super::auto_drive_light::{private_dir, private_file, repo_root, show};
use super::exec_stub::write_executable;
use super::wait::output_with_ceiling;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

pub const HEAD: &str = "1111111111111111111111111111111111111111";
pub const BASE: &str = "2222222222222222222222222222222222222222";

pub fn request(operation: &str, payload: Value) -> Value {
    json!({
        "schema_version": 1, "request_id": "request-1", "operation": operation,
        "run_id": "run-2333", "round_id": "round-1",
        "target": {"host": "github.com", "repository_id": 2333001,
            "owner": "fixture", "repo": "fixture", "pr_number": 7,
            "head_repository_id": 2333001},
        "revision": {"head_sha": HEAD, "base_ref": "main", "base_sha": BASE,
            "worktree_sha": null},
        "predecessor_ref": null, "resume_ref": null, "payload": payload
    })
}

pub fn collect_request() -> Value {
    request(
        "collect",
        json!({"operations": [{
            "operation_id": "read-pr", "api": "REST", "resource": "PR_METADATA",
            "parameters": {}, "pagination": {"scope": "SINGLE", "per_page": 1, "max_pages": 1}
        }]}),
    )
}

pub fn artifact(id: &str) -> Value {
    json!({"artifact_id": id, "sha256": "a".repeat(64)})
}

/// No trusted launch binding is minted here. Tests using this fixture can
/// assert input rejection and missing authorization, never successful merging.
pub struct CommandFixture {
    pub tmp: tempfile::TempDir,
    pub input: PathBuf,
    pub result: PathBuf,
    pub calls: PathBuf,
    bin: PathBuf,
}

impl CommandFixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("request.json");
        let result = tmp.path().join("result.json");
        let calls = tmp.path().join("gh.calls");
        let bin = tmp.path().join("bin");
        private_dir(&bin);
        // Unexpected GitHub access is recorded and refused. No real gh is
        // reachable through the child's fixed PATH; there are no credentials.
        write_executable(
            &bin.join("gh"),
            "#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"$GH_CALLS\"\nexit 97\n",
            0o700,
        );
        Self {
            tmp,
            input,
            result,
            calls,
            bin,
        }
    }

    pub fn write(&self, input: &Value) {
        private_file(&self.input, &serde_json::to_string(input).unwrap());
    }

    pub fn run(&self, group: &str, operation: &str) -> Output {
        self.run_extra(group, operation, &[])
    }

    pub fn run_extra(&self, group: &str, operation: &str, env: &[(&str, &str)]) -> Output {
        let mut cmd = crate::isolated_amplihack::isolated(super::amplihack_bin::amplihack_bin());
        cmd.args(["pr", group, operation, "--request"])
            .arg(&self.input)
            .arg("--result")
            .arg(&self.result)
            .current_dir(self.tmp.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("GH_CALLS", &self.calls)
            .env("HOME", self.tmp.path())
            .env("AMPLIHACK_HOME", self.tmp.path().join(".amplihack"));
        for (key, value) in env {
            cmd.env(key, value);
        }
        output_with_ceiling("PR evidence command", &mut cmd, Duration::from_secs(20))
    }

    /// Normal domain outcomes have a result; invalid requests instead have
    /// a structured diagnostic. Merely rejecting an unknown subcommand is
    /// deliberately insufficient: these tests must fail on today's baseline.
    pub fn diagnostic(&self, output: &Output) -> Value {
        for stream in [&output.stdout, &output.stderr] {
            if let Ok(value) = serde_json::from_slice::<Value>(stream)
                && value.get("error").is_some()
            {
                return value;
            }
        }
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        text.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|value| value.get("error").is_some())
            .unwrap_or_else(|| {
                panic!(
                    "expected a v1 structured diagnostic, not a CLI parse failure:\n{}",
                    show(output)
                )
            })
    }

    pub fn assert_no_github(&self) {
        assert!(!self.calls.exists(), "invalid/unbound input reached GitHub");
    }
}

/// Fixture evaluator responses, not a competing convergence implementation.
/// The finite plan is a harness tripwire: exhausting it exits 98, never STUCK.
const RUNNER: &str = r#"#!/usr/bin/env bash
set -eu
case "${1:-} ${2:-}" in
  'recipe show') exit 0 ;;
  'orch helper') exec "$REAL_AMPLIHACK" "$@" ;;
  'recipe run') ;;
  *) echo "unexpected fixture invocation" >&2; exit 98 ;;
esac
name=$(basename "$3" .yaml)
shift 3
label=''; record=''; field=''; prev=''
for arg in "$@"; do
  if [ "$prev" = '-c' ]; then
    case "$arg" in
      autodrive_round_label=*|loop_round_label=*) label=${arg#*=} ;;
      autodrive_round_record=*) record=${arg#*=} ;;
    esac
  fi
  prev=$arg
done
n=${label#round-}
case "$n" in ''|*[!0-9]*) exit 98 ;; esac
row=$(jq -ce --argjson i "$((n - 1))" '.[$i]' "$FIXTURE/plan.json") || exit 98
if [ "$name" = loop-health-evaluator ]; then
  printf '%s\0' "$@" > "$FIXTURE/evaluator-$n.argv"
  printf '%s\n' "$label" >> "$FIXTURE/evaluators"
  # Record the *delivered* context, including any registered file payloads.
  for arg in "$@"; do
    if [ -f "${arg#*=}" ]; then
      cat -- "${arg#*=}" >> "$FIXTURE/evaluator-$n.files"
    fi
  done
  mode=$(printf '%s' "$row" | jq -r '.evaluation // "CONTINUE"')
  case "$mode" in
    MALFORMED) printf 'agent output was truncated\n'; exit 0 ;;
    FAILED) echo 'fixture evaluator invocation failed' >&2; exit 42 ;;
    POLICY_REFUSED) exit 79 ;;
    STUCK)
      printf '{"loop_verdict":"STUCK","verdict_source":"evaluator","not_converging":["fixture requires operator action"]}\n' > "$FIXTURE/explicit-stuck.json"
      printf 'LOOP_HEALTH: STUCK — fixture requires operator action (source=evaluator)\n' >&2
      exit 1 ;;
    CONTINUE|DONE)
      printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: %s\n' "$mode" ;;
    *) exit 98 ;;
  esac
  exit 0
fi
case "$name" in
  autodrive-crusty-round) field=crusty_verdict ;;
  autodrive-merge-round) field=merge_ready_verdict ;;
  *) exit 98 ;;
esac
printf '%s\n' "$label" >> "$FIXTURE/rounds"
printf '%s' "$row" | jq --arg f "$field" '.record + {($f): .verdict}' > "$record"
printf '%s' "$row" | jq -r '.findings[]?' > "$record.findings"
echo 'fixture round completed without changing HEAD'
"#;

pub struct LoopFixture {
    tmp: tempfile::TempDir,
    pub state: PathBuf,
    pub fixture: PathBuf,
    main: PathBuf,
    worktree: PathBuf,
    bin: PathBuf,
    head: String,
}

impl LoopFixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir_in(super::scratch_base::scratch_base()).unwrap();
        let main = tmp.path().join("main");
        let worktree = tmp.path().join("pr");
        let state = tmp.path().join("state");
        let fixture = tmp.path().join("fixture");
        let bin = tmp.path().join("bin");
        for dir in [&main, &state, &fixture, &bin] {
            private_dir(dir);
        }
        let git = |args: &[&str]| git_at(&main, args);
        git(&["init", "-q", "-b", "main"]);
        let tree = git(&["write-tree"]);
        let head = git(&["commit-tree", &tree, "-m", "fixture base"]);
        git(&["update-ref", "HEAD", &head]);
        git(&[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            worktree.to_str().unwrap(),
        ]);
        write_executable(&bin.join("amplihack"), RUNNER, 0o700);
        write_executable(
            &bin.join("gh"),
            "#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"$FIXTURE/gh.calls\"\nexit 0\n",
            0o700,
        );
        private_file(
            &fixture.join("task.txt"),
            "issue #2333 isolated loop scenario",
        );
        Self {
            tmp,
            state,
            fixture,
            main,
            worktree,
            bin,
            head,
        }
    }

    pub fn run(&self, loop_name: &str, plan: &[Value], extra: &[&str]) -> Output {
        let plan: Vec<_> = plan
            .iter()
            .cloned()
            .map(|mut entry| {
                entry["record"]["head_sha"] = json!(self.head);
                entry
            })
            .collect();
        private_file(
            &self.fixture.join("plan.json"),
            &serde_json::to_string(&plan).unwrap(),
        );
        let (round, token, field) = if loop_name == "crusty" {
            ("autodrive-crusty-round", "CLEAN", "crusty_verdict")
        } else {
            (
                "autodrive-merge-round",
                "MERGE_READY",
                "merge_ready_verdict",
            )
        };
        let mut cmd = crate::isolated_amplihack::isolated("bash");
        cmd.arg(repo_root().join("amplifier-bundle/tools/autodrive_loop.sh"))
            .args([
                "--loop-name",
                loop_name,
                "--round-recipe",
                round,
                "--clean-token",
                token,
                "--verdict-field",
                field,
            ])
            .arg("--repo")
            .arg(&self.worktree)
            .arg("--review-dir")
            .arg(&self.main)
            .arg("--state-dir")
            .arg(&self.state)
            .arg("--task-file")
            .arg(self.fixture.join("task.txt"))
            .args(extra)
            .current_dir(&self.main)
            .env("HOME", self.tmp.path())
            .env("AMPLIHACK_BIN", self.bin.join("amplihack"))
            .env("REAL_AMPLIHACK", super::amplihack_bin::amplihack_bin())
            .env("FIXTURE", &self.fixture)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()));
        let out = output_with_ceiling(
            "finite auto-drive scenario",
            &mut cmd,
            Duration::from_secs(40),
        );
        assert_eq!(
            git_at(&self.worktree, &["rev-parse", "HEAD"]),
            self.head,
            "the scenario must exercise unchanged-head behavior"
        );
        out
    }

    pub fn evaluations(&self) -> Vec<String> {
        std::fs::read_to_string(self.fixture.join("evaluators"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn result(&self, loop_name: &str) -> Value {
        let path = self.state.join(format!("{loop_name}-result.json"));
        serde_json::from_slice(
            &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        )
        .unwrap()
    }

    pub fn delivered(&self, round: usize) -> String {
        let args = std::fs::read(self.fixture.join(format!("evaluator-{round}.argv"))).unwrap();
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&args),
            std::fs::read_to_string(self.fixture.join(format!("evaluator-{round}.files")))
                .unwrap_or_default()
        )
    }
}

fn git_at(dir: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", show(&out));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

pub fn round(verdict: &str, ci: &str, evaluation: &str) -> Value {
    json!({"verdict": verdict, "evaluation": evaluation, "findings": ["await-ci"],
        "record": {"ci_signal": ci, "head_sha": HEAD,
            "actions": [{"kind": "WAIT", "reason": "external system", "elapsed_ms": 1}],
            "collection_errors": [{"code": "RATE_LIMITED", "api": "GRAPHQL", "http_status": 403}],
            "waiting_history": [{"wake_reason": "DEADLINE", "elapsed_ms": 1}]}})
}
