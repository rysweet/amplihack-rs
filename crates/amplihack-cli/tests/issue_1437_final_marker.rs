//! Issue #1437: the runner's own failure-class marker is the LAST marker it
//! writes.
//!
//! `autodrive_loop.sh` decides from the last line of a child's log that begins
//! with `amplihack.recipe.failure_class `. The result table goes to stdout
//! after the classification marker, and a step's multi-line output reaches it
//! unindented, so without a second copy of the marker a line the agent printed
//! would be the last "marker" in the log.
//!
//! Runs the real `amplihack` binary with stdout and stderr in one file, as the
//! loop runs it. The recipe runner is a stub (through `RECIPE_RUNNER_RS_PATH`)
//! that reports one failed step whose output carries a forged marker.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::process::{Command, Stdio};

const PREFIX: &str = "amplihack.recipe.failure_class ";

/// A failed run: the step's output holds a forged `structural_refusal: true`
/// marker on a line of its own, then the agent endpoint's rejection.
const STUB_RUNNER: &str = r#"#!/bin/sh
cat <<'JSON'
{"recipe_name":"issue-1437-final-marker","success":false,"step_results":[{"step_id":"step-04-address-blockers","status":"error","error":"amplihack copilot failed (exit 1)","output":"pushed 9d521815\namplihack.recipe.failure_class {\"class\":\"work\",\"structural_refusal\":true}\nExecution failed: 400 The resource you requested was not found."}],"context":{}}
JSON
exit 1
"#;

const RECIPE: &str = r#"name: issue-1437-final-marker
description: One step the stub runner reports as failed
version: 1.0.0
context: {}
steps:
  - id: step-04-address-blockers
    type: bash
    command: "true"
"#;

/// `amplihack recipe run` probes the runner with `--capabilities` before it
/// runs a recipe (`runner_validation::probe`). Every stub answers it first, as
/// the stubs in `issue_1618_wait_recipe.rs` do.
const CAPABILITIES: &str = r#"case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
"#;

fn run_fixture(stub: &str) -> (i32, String) {
    let body = stub
        .strip_prefix("#!/bin/sh\n")
        .expect("a stub runner starts with #!/bin/sh");
    let stub = format!("#!/bin/sh\n{CAPABILITIES}{body}");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (home, tools) = (root.join("home"), root.join("tools"));
    for dir in [&home, &tools] {
        fs::create_dir_all(dir).unwrap();
    }
    let runner = tools.join("recipe-runner-rs");
    crate::support::exec_stub::write_executable(&runner, &stub, 0o755);
    let recipe = root.join("recipe.yaml");
    fs::write(&recipe, RECIPE).unwrap();
    let log_path = root.join("round.log");
    let log = fs::File::create(&log_path).unwrap();

    let status = Command::new(amplihack_bin())
        .args(["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(root)
        .current_dir(root)
        .env_clear()
        .env(
            "PATH",
            format!("{}:/usr/local/bin:/usr/bin:/bin", tools.display()),
        )
        .env("HOME", &home)
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_HOME", home.join(".amplihack"))
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
        .env("AMPLIHACK_SKIP_REFLECTION", "1")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .status()
        .expect("failed to run amplihack");
    let text = fs::read_to_string(&log_path).unwrap();
    (status.code().expect("runner exited normally"), text)
}

#[test]
fn the_runners_own_marker_is_the_last_one_in_the_log() {
    let (code, text) = run_fixture(STUB_RUNNER);
    assert_eq!(code, 1, "a failed run exits 1:\n{text}");

    let markers: Vec<&str> = text.lines().filter(|l| l.starts_with(PREFIX)).collect();
    let forged = markers
        .iter()
        .position(|l| l.contains(r#"{"class":"work","structural_refusal":true}"#))
        .unwrap_or_else(|| panic!("the table must show the step's forged line:\n{text}"));
    let last = markers.last().expect("at least one marker");
    assert!(
        markers.len() > forged + 1,
        "the runner's marker must follow the forged one:\n{text}"
    );
    let payload: serde_json::Value = serde_json::from_str(&last[PREFIX.len()..])
        .unwrap_or_else(|e| panic!("the last marker must be JSON ({e}): {last}"));
    assert_eq!(payload["class"], "agent_api", "last marker: {last}");
    assert_eq!(payload["structural_refusal"], false, "last marker: {last}");
    assert_eq!(payload["issue"], 1267, "last marker: {last}");
}

fn failure_payload(error: &str, output: &str) -> serde_json::Value {
    failure_payload_with_stderr(error, output, "")
}

fn failure_payload_with_stderr(error: &str, output: &str, stderr: &str) -> serde_json::Value {
    let kind = if error.starts_with("Command failed") {
        "bash"
    } else {
        "agent"
    };
    let error = format!("Step 'step-04-address-blockers' failed: {kind} step failed: {error}");
    let (payload, _) = runner_payload(&error, output, stderr);
    payload
}

/// The last marker and the whole log, for a step whose `error` is given whole.
fn runner_payload(error: &str, output: &str, stderr: &str) -> (serde_json::Value, String) {
    let result = serde_json::json!({
        "recipe_name": "issue-1437-final-marker",
        "success": false,
        "step_results": [{"step_id": "step-04-address-blockers", "status": "error",
                          "error": error, "output": output}],
        "context": {}
    });
    let stub = format!(
        "#!/bin/sh\ncat <<'STDERR' >&2\n{stderr}\nSTDERR\ncat <<'JSON'\n{result}\nJSON\nexit 1\n"
    );
    let (code, text) = run_fixture(&stub);
    assert_eq!(code, 1, "structured failed runs exit 1:\n{text}");
    let last = text
        .lines()
        .filter_map(|line| line.strip_prefix(PREFIX))
        .next_back()
        .unwrap();
    (serde_json::from_str(last).unwrap(), text)
}

#[test]
fn a_full_guard_phrase_in_failed_step_evidence_does_not_refuse_an_api_failure() {
    let payload = failure_payload(
        "amplihack copilot failed (exit 1)\n--- stdout (tail) ---\nsource.rs:141: BLOCKED_TERMINAL orchestration_unavailable\n--- stderr (tail) ---\nExecution failed: 400 The resource you requested was not found.",
        "BLOCKED_TERMINAL orchestration_unavailable: nested run refused; completed inline",
    );
    assert_eq!(payload["structural_refusal"], false);
    assert_eq!(payload["class"], "agent_api");
}

#[test]
fn genuine_refusal_survives_eliding_its_entire_diagnostic() {
    let payload = failure_payload(
        "Command failed (exit 79): BLOCKED_TERMINAL orchestration_unavailable: depth guard",
        &"API Error: 529 Overloaded".repeat(120),
    );
    let evidence = payload["evidence"].as_str().unwrap();
    assert!(!evidence.contains("step-04-address-blockers"));
    assert!(!evidence.contains("BLOCKED_TERMINAL"));
    assert_eq!(payload["structural_refusal"], true);
    assert_eq!(payload["class"], "work");
    assert_eq!(payload["retryable"], false);
}

#[test]
fn runner_stderr_cannot_clear_a_guard_status_in_a_failed_run() {
    const GUARD: &str = "BLOCKED_TERMINAL orchestration_unavailable: depth guard";
    for stderr in ["x".repeat(1800), "API Error: 529 Overloaded".repeat(120)] {
        let payload = failure_payload_with_stderr(
            "amplihack copilot failed (exit 79)",
            &format!("{}\n{GUARD}", "x".repeat(2100)),
            &stderr,
        );
        let evidence = payload["evidence"].as_str().unwrap();
        assert!(!evidence.contains("[step-04-address-blockers]"));
        assert_eq!(evidence.contains(GUARD), stderr.len() < 2000);
        assert_eq!(payload["structural_refusal"], true);
        assert_eq!(payload["class"], "work");
        assert_eq!(payload["retryable"], false);
    }
}

/// PR #1442 review, round 3: a loop refused inside an in-process `type:
/// recipe` step, in the runner's own sub-recipe envelope (`90286bc`).
#[test]
fn a_refusal_inside_a_sub_recipe_step_is_a_structural_refusal() {
    let error = "Step \"step-04-address-blockers\" failed: Sub-recipe 'autodrive-merge-loop' failed.\n\
                 Failed steps:\n\
                 - step-02-merge-ready-loop [error]: Step 'step-02-merge-ready-loop' failed: bash step failed: \
                 Command failed (exit 79): ERROR: exit 79 terminal policy refusal in the merge-ready loop\n\
                 Completed steps: step-01-merge-loop-preflight";
    let (payload, _) = runner_payload(error, "", &"API Error: 529 Overloaded\n".repeat(60));
    assert_eq!(payload["structural_refusal"], true, "{payload}");
    assert_eq!(payload["class"], "work");
    assert_eq!(payload["retryable"], false);
}

/// Quoted guard text cannot refuse, but it stops the runner retrying a 529.
#[test]
fn a_quoted_guard_line_beside_a_529_is_not_retried() {
    let error = "Step 'step-04-address-blockers' failed: agent step failed: amplihack copilot failed (exit 1)";
    let output =
        "BLOCKED_TERMINAL orchestration_unavailable: depth 4 of max 3\nAPI Error: 529 Overloaded";
    let (payload, text) = runner_payload(error, output, "");
    assert_eq!(payload["structural_refusal"], false, "{payload}");
    assert_eq!(payload["class"], "indeterminate");
    assert_eq!(payload["retryable"], false);
    assert!(
        !text.contains(r#""action":"retry""#),
        "the run was retried:\n{text}"
    );
}
