//! Issue #1337: reusable agentic loop-health evaluation contract.
//!
//! The motivating run: a default-workflow invocation spent 2h47m and produced
//! ZERO commits. Seven consecutive steps each reported exactly `10m 0s` with
//! no artifacts, one child returned `BLOCKED_TERMINAL` at depth 4/3 with exit
//! 79, and the run itself said "The review workflow is still running; I'm
//! waiting for its structured findings." Every one of those was a visible
//! signal and nothing acted on any of them.
//!
//! What is pinned here:
//!
//! - the `loop-health-evaluator` brick exists, parses, and stays inside the
//!   400-line brick budget;
//! - it has exactly one agentic judgement step, gated off on a terminal
//!   policy refusal so the run never retries into the exit-79 guard;
//! - it declares NO numeric iteration cap — not even as a backstop — and NO
//!   per-step timeout;
//! - the executable contract test (STUCK path + malformed-verdict path)
//!   passes against a binary built from this tree.

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BRICK_LINE_BUDGET: usize = 400;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

fn recipe_path() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/loop-health-evaluator.yaml")
}

fn recipe_text() -> String {
    let path = recipe_path();
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn recipe_yaml() -> Value {
    serde_yaml::from_str(&recipe_text())
        .unwrap_or_else(|e| panic!("parse {}: {e}", recipe_path().display()))
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("loop-health-evaluator must declare top-level steps")
}

fn step<'a>(recipe: &'a Value, id: &str) -> &'a Value {
    steps(recipe)
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("missing step `{id}`"))
}

fn field<'a>(step: &'a Value, name: &str) -> &'a str {
    step.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("step is missing a `{name}` field"))
}

#[test]
fn loop_health_evaluator_recipe_exists_and_fits_the_brick_budget() {
    let text = recipe_text();
    let lines = text.lines().count();
    assert!(
        lines <= BRICK_LINE_BUDGET,
        "loop-health-evaluator.yaml is {lines} lines; the brick budget is {BRICK_LINE_BUDGET}"
    );
    let recipe = recipe_yaml();
    assert_eq!(
        recipe.get("name").and_then(Value::as_str),
        Some("loop-health-evaluator"),
        "the recipe's `name:` must match its filename stem — `recipe run` resolves by stem \
         while `recipe list` keys on `name:`"
    );
}

#[test]
fn verdict_contract_has_exactly_three_outcomes() {
    let recipe = recipe_yaml();
    let prompt = field(step(&recipe, "step-02-evaluate-loop-health"), "prompt");
    for token in ["CONTINUE", "DONE", "STUCK"] {
        assert!(
            prompt.contains(token),
            "the evaluator prompt must define the `{token}` outcome"
        );
    }
    assert!(
        prompt.contains("loop_verdict"),
        "the evaluator must emit its verdict under the `loop_verdict` field"
    );
    // The fail-safe direction has to be stated to the model as well as
    // enforced in the gate.
    assert!(
        prompt.contains("never as `CONTINUE`") || prompt.contains("never as CONTINUE"),
        "the prompt must state that a malformed verdict is STUCK, never CONTINUE"
    );
}

#[test]
fn evaluator_receives_real_evidence_not_prose_impressions() {
    let recipe = recipe_yaml();
    let collect = field(step(&recipe, "step-01-collect-loop-evidence"), "command");
    let prompt = field(step(&recipe, "step-02-evaluate-loop-health"), "prompt");

    // Every evidence channel required by issue #1337 must be both computed
    // and surfaced to the judge.
    for computed in [
        "commits_since_baseline",
        "diff_lines",
        "repeated_duration_count",
        "repeated_output",
        "findings_new",
        "findings_recurring",
        "findings_resolved",
        "tests_moved",
        "ci_moved",
        "waiting_on_output",
    ] {
        assert!(
            collect.contains(computed),
            "step-01 must compute `{computed}` from real evidence"
        );
        assert!(
            prompt.contains(computed),
            "the evaluator prompt must surface `{computed}` to the judge"
        );
    }
    // `terminal_refusal` is computed but deliberately NOT surfaced to the
    // judge: on a terminal policy refusal the judge is never asked at all.
    assert!(
        collect.contains("terminal_refusal"),
        "step-01 must compute `terminal_refusal`"
    );
    assert!(
        prompt.contains("{{loop_history}}") && prompt.contains("{{loop_last_round_output}}"),
        "the judge must see the accumulated round verdicts and the last round's raw output"
    );
}

#[test]
fn no_numeric_iteration_cap_anywhere() {
    // The core design decision of #1337: the loop terminator is absence of
    // progress, not attempt count. A max-iterations integer cuts off work
    // that was about to converge AND lets a genuinely stuck loop burn the
    // whole budget first. Host safety is structural, one layer down
    // (#1327 sealed depth ceiling, #1332 width cap + memory floor).
    let text = recipe_text();
    for banned in [
        "max_iterations",
        "max_iteration",
        "max_rounds",
        "max_attempts",
        "iteration_limit",
        "MAX_LOOPS",
        "max_retries",
    ] {
        for (n, line) in text.lines().enumerate() {
            // The prose that explains WHY there is no counter is allowed to
            // name the thing it rejects.
            if line.trim_start().starts_with('#') {
                continue;
            }
            assert!(
                !line.contains(banned),
                "line {}: loop-health-evaluator.yaml introduces a numeric iteration cap \
                 (`{banned}`), which issue #1337 rejects outright:\n  {line}",
                n + 1
            );
        }
    }
    // `recursion.max_depth` / `max_total_steps` are the runner's own
    // structural guard rails, not the loop terminator, and must stay.
    let recipe = recipe_yaml();
    assert!(
        recipe.get("recursion").is_some(),
        "the structural recursion guard rails must remain declared"
    );
}

#[test]
fn no_per_step_timeout_at_any_scale() {
    // Issue #439 plus the #1337 constraint: nothing here may be bounded at
    // seconds or single-digit-minute scale. The runner owns the ceiling.
    let recipe = recipe_yaml();
    for s in steps(&recipe) {
        let id = s.get("id").and_then(Value::as_str).unwrap_or("<unnamed>");
        for key in ["timeout", "timeout_seconds"] {
            assert!(
                s.get(key).is_none(),
                "step `{id}` declares `{key}` — no step in this brick may carry a per-step timeout"
            );
        }
    }
    assert!(
        recipe.get("default_step_timeout").is_none(),
        "loop-health-evaluator must not pin a recipe-level step timeout"
    );
}

#[test]
fn exit_79_is_terminal_and_never_retried_into() {
    let recipe = recipe_yaml();
    let collect = field(step(&recipe, "step-01-collect-loop-evidence"), "command");
    assert!(
        collect.contains("BLOCKED_TERMINAL"),
        "step-01 must detect a BLOCKED_TERMINAL child"
    );
    assert!(
        collect.contains("79"),
        "step-01 must detect exit code 79 (the #1327/#1332 policy refusal)"
    );

    // The agentic step is gated OFF on a terminal refusal: exit 79 is already
    // a final answer from a structural guard, so not even a model call is
    // spent deciding whether to re-enter it.
    let evaluate = step(&recipe, "step-02-evaluate-loop-health");
    assert_eq!(
        evaluate.get("condition").and_then(Value::as_str),
        Some("loop_evidence.terminal_refusal == 'false'"),
        "the evaluator agent step must be skipped on a terminal policy refusal"
    );

    let resolve = field(step(&recipe, "step-03-resolve-loop-verdict"), "command");
    assert!(
        resolve.contains("terminal_policy_refusal"),
        "step-03 must attribute a forced STUCK to the terminal policy refusal"
    );
}

#[test]
fn verdict_resolution_uses_the_canonical_orch_helper_pipeline() {
    // The verdict contract must fit docs/reference/structured-verdict-parsing.md,
    // not compete with it: extract-json | extract-field --default | normalise.
    let recipe = recipe_yaml();
    for id in [
        "step-03-resolve-loop-verdict",
        "step-04-enforce-loop-verdict",
    ] {
        let cmd = field(step(&recipe, id), "command");
        assert!(
            cmd.contains("amplihack orch helper extract-json"),
            "{id} must route agent output through `orch helper extract-json`"
        );
        assert!(
            cmd.contains("--field loop_verdict --default STUCK"),
            "{id} must default a missing `loop_verdict` to STUCK, never CONTINUE"
        );
        assert!(
            cmd.contains("amplihack orch helper normalise-loop-verdict"),
            "{id} must normalise the token with `orch helper normalise-loop-verdict`"
        );
        // Agent output is untrusted data: it arrives via env + stdin and is
        // never interpolated into a command position.
        assert!(
            !cmd.contains("{{loop_health_assessment}}"),
            "{id} must not interpolate agent output into the command line"
        );
    }
}

#[test]
fn normalise_loop_verdict_helper_is_wired_into_the_cli() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_amplihack"));
    // The whole point of the default: garbage in, STUCK out.
    for (input, want) in [
        ("CONTINUE", "CONTINUE"),
        ("DONE", "DONE"),
        ("STUCK", "STUCK"),
        ("", "STUCK"),
        ("banana", "STUCK"),
        // Negation-adjacent tokens that CONTAIN a permissive token. A
        // `str::contains` implementation would fail OPEN on every one.
        ("DISCONTINUE", "STUCK"),
        ("CANNOT_CONTINUE", "STUCK"),
        ("NOT_DONE", "STUCK"),
    ] {
        let out = crate_run(&bin, input);
        assert_eq!(
            out.trim(),
            want,
            "`normalise-loop-verdict` on {input:?} must print {want}"
        );
    }
}

fn crate_run(bin: &Path, stdin: &str) -> String {
    use std::io::Write;
    let mut child = Command::new(bin)
        .args(["orch", "helper", "normalise-loop-verdict"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn amplihack");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "normalise-loop-verdict exited {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Runs the executable contract test — the STUCK path and the
/// malformed-verdict path exercised against the real extracted step bodies.
///
/// Wired here because `.github/workflows/ci.yml` lists the bash recipe tests
/// one by one; running it from `cargo test` gets the same coverage without
/// touching that file.
#[test]
fn loop_health_contract_shell_test_passes() {
    let root = workspace_root();
    let script =
        root.join("amplifier-bundle/recipes/tests/test-issue-1337-loop-health-evaluator.sh");
    assert!(script.is_file(), "missing {}", script.display());

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_amplihack"));
    let bin_dir = bin.parent().expect("binary parent dir");
    let path = match std::env::var("PATH") {
        Ok(p) => format!("{}:{p}", bin_dir.display()),
        Err(_) => bin_dir.display().to_string(),
    };

    let out = Command::new("bash")
        .arg(&script)
        .current_dir(&root)
        .env("PATH", path)
        .output()
        .expect("run the loop-health contract shell test");

    assert!(
        out.status.success(),
        "test-issue-1337-loop-health-evaluator.sh failed ({:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
