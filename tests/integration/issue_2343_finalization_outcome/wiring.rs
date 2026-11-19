//! The two steps' wiring, by running them (#2343, #2809, #2819).
//!
//! These cases used to read recipe text and shell text: they grepped the
//! finalize recipe for `--guard` and `condition:`, and the validator for
//! forbidden words. `docs/howto/keep-tests-passing-under-host-load.md` forbids
//! that, and for a reason the repo had already hit -- a test that pins how
//! code is written fails when the wording moves and passes when the behaviour
//! moves. Each case below runs the real thing instead:
//!
//! - the shipped `assess-finalization` step, through the real pinned recipe
//!   runner, with the shipped sub-recipe beside it, so what is asserted is
//!   that the step is attempted on every outcome and that its failure -- the
//!   agent's, or the sub-recipe's own -- never ends the run;
//! - the shipped `validate-agentic-finalization` step's own `command:`, with a
//!   stub `amplihack` in its path, so what is asserted is the call that
//!   actually reaches `step-recovery run` and the exit the body actually gave
//!   it.
//!
//! Nothing here asserts the presence or absence of a word in a file.

use std::fs;
use std::path::PathBuf;

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::as_refs;
use crate::issue_1641_finalization_open_pr::fixture::*;
use serde_json::{Value, json};

fn bundle_recipe(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle/recipes")
        .join(format!("{name}.yaml"))
}

/// The `<id>` step of a shipped recipe, parsed as the document it is, so the
/// step that runs below is the shipped one and not a copy of it.
fn shipped_step(recipe: &str, id: &str) -> serde_yaml::Value {
    let text = fs::read_to_string(bundle_recipe(recipe))
        .unwrap_or_else(|e| panic!("read {recipe}.yaml: {e}"));
    let document: serde_yaml::Value =
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {recipe}.yaml: {e}"));
    document["steps"]
        .as_sequence()
        .unwrap_or_else(|| panic!("{recipe}.yaml has no steps"))
        .iter()
        .find(|step| step["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("{recipe}.yaml has no step {id}"))
        .clone()
}

/// `recipe-runner-rs` at the revision in `claude-plugin/recipe-runner.rev`
/// (#2658): never the host's installed runner.
fn pinned_runner() -> PathBuf {
    crate::pinned_runner::pinned_runner(&workspace_root())
}

struct Run {
    dir: tempfile::TempDir,
    result: Value,
}

impl Run {
    fn status(&self, step_id: &str) -> String {
        self.result["step_results"]
            .as_array()
            .expect("step results")
            .iter()
            .find(|step| step["step_id"] == step_id)
            .unwrap_or_else(|| panic!("no result for {step_id}: {}", self.result))
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }
}

/// Run the shipped `assess-finalization` step followed by a marker step that
/// stands for everything after it, with `evidence` as the run's outcome.
/// `with_sub_recipe` decides whether the shipped sub-recipe is reachable.
fn run_assessment_chain(evidence: &str, with_sub_recipe: bool) -> Run {
    let dir = tempfile::TempDir::new().expect("tempdir");
    if with_sub_recipe {
        let name = "workflow-finalization-assessment";
        fs::copy(bundle_recipe(name), dir.path().join(format!("{name}.yaml")))
            .expect("stage the shipped sub-recipe beside the run");
    }
    let marker = dir.path().join("reached-the-validator");
    let recipe = json!({
        "name": "issue-2819-assessment-chain",
        "version": "1.0.0",
        "description": "the shipped assess-finalization step, then what follows it",
        "context": {
            "finalization_evidence": evidence,
            "finalizer_step_status": r#"{"status":"ok","reporting_failure":"false"}"#,
            "finalization_assessment": "",
        },
        "steps": [
            serde_yaml::from_value::<Value>(shipped_step("workflow-finalize", "assess-finalization"))
                .expect("the shipped step is representable as JSON"),
            {
                "id": "after-the-assessment",
                "type": "bash",
                "command": format!("touch {}", marker.display()),
            },
        ],
    });
    let path = dir.path().join("chain.yaml");
    fs::write(&path, serde_yaml::to_string(&recipe).expect("serialize")).expect("write recipe");

    let output = crate::isolated_amplihack::isolated(pinned_runner().to_str().expect("runner"))
        .arg(&path)
        .arg("--working-dir")
        .arg(dir.path())
        .args(["--no-auto-stage", "--output-format", "json"])
        .env("HOME", dir.path())
        .env("TMPDIR", dir.path())
        .output()
        .expect("run the pinned runner");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "the runner must serialize its result: {e}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert!(
        marker.exists(),
        "the steps after the assessment must still run: {result}"
    );
    Run { dir, result }
}

// The assessment runs on every outcome and never ends the run by itself. It
// once carried a `condition:` that skipped it for the verdicts the bash
// classifier exited on; both are gone. Asserted by running the shipped step
// over two opposite outcomes with no agent on the host, so the agent step
// inside it fails every time: the step is still attempted, and the run still
// finishes.
#[test]
fn the_assessment_is_attempted_on_every_outcome_and_never_ends_the_run() {
    for (what, evidence) in [
        (
            "a finished run",
            json!({"schema_version": 1, "outcome": {"pr_number": "7", "pr_state": "OPEN"}}),
        ),
        (
            "a run that published nothing",
            json!({"schema_version": 1, "outcome": {"pr_number": "", "remote_head": ""}}),
        ),
    ] {
        let run = run_assessment_chain(&evidence.to_string(), true);

        assert_eq!(
            run.result["success"], true,
            "{what}: a declined assessment must not end the run: {}",
            run.result
        );
        assert_ne!(
            run.status("assess-finalization"),
            "skipped",
            "{what}: the assessment decides every run's terminal state, so no outcome skips it: {}",
            run.result
        );
        assert_eq!(
            run.status("after-the-assessment"),
            "completed",
            "{what}: {}",
            run.result
        );
        drop(run.dir);
    }
}

// The sub-recipe's own failure -- resolution, the recursion limit -- is the
// other half of the same guarantee, and it is a different `continue_on_error`
// from the one above: the outer step's.
#[test]
fn an_assessment_the_run_cannot_even_resolve_still_lets_the_run_continue() {
    let run = run_assessment_chain(r#"{"schema_version":1}"#, false);

    assert_eq!(
        run.result["success"], true,
        "an unresolvable assessment must not end the run: {}",
        run.result
    );
    assert_eq!(
        run.status("assess-finalization"),
        "failed",
        "the step really did fail, rather than being skipped: {}",
        run.result
    );
    assert_eq!(run.status("after-the-assessment"), "completed");
}

/// The shipped `validate-agentic-finalization` step's own `command:`.
fn validator_step_command() -> String {
    shipped_step("workflow-finalize", "validate-agentic-finalization")["command"]
        .as_str()
        .expect("the validator step has a bash command")
        .to_owned()
}

// Recovery on the validator step must offer repair, not only a stop. Asserted
// by running the step with a stub in `amplihack`'s place and reading the call
// it made: `--guard` and `--no-repair` both withhold `retry_after_repair`
// (docs/reference/step-failure-recovery.md#the-four-choices), which would
// leave an agent that can only give up on a gap it could close.
#[test]
fn a_no_decision_exit_reaches_a_recovery_agent_that_may_repair_and_validate_again() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
    fx.stub_step_recovery_driver();
    let mut envs = finished_env(&fx.repo_path());
    envs.push(("RECIPE_VAR_step_recovery_dir", fx.repo_path()));

    let outcome = fx.run_step(&validator_step_command(), &as_refs(&envs));

    let calls = fx.driver_calls();
    let call = calls
        .iter()
        .find(|call| call.contains("step-recovery run"))
        .unwrap_or_else(|| panic!("the step must run under step recovery, got {calls:?}"));
    assert!(
        call.contains("--step-id validate-agentic-finalization"),
        "recovery must be recorded against this step: {call}"
    );
    for required in ["--essential", "--stop-on-handoff"] {
        assert!(call.contains(required), "{required} is missing: {call}");
    }
    for withholding in ["--guard", "--no-repair"] {
        assert!(
            !call.contains(withholding),
            "`{withholding}` withholds retry_after_repair, so the agent could only stop: {call}"
        );
    }
    // And what reached it is the gap, not a verdict: the body the driver ran
    // reported that nothing decided, and failed.
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "the body must fail for recovery to see it\n{}",
        outcome.describe()
    );
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_MISSING_TERMINAL_EVIDENCE",
        "{}",
        outcome.describe()
    );
    assert_ne!(
        outcome.field("terminal_state"),
        "FAILED_IMPLEMENTATION",
        "the validator must not reach a verdict of its own"
    );
}

// With recovery switched off the step still runs its body, and nothing is
// handed to a driver that is not there.
#[test]
fn without_a_recovery_directory_the_validator_runs_its_body_directly() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
    fx.stub_step_recovery_driver();
    let envs = decided(&fx, "EXISTING_OPEN_PR", "7");

    let outcome = fx.run_step(&validator_step_command(), &as_refs(&envs));

    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    assert_eq!(outcome.field("pr_number"), "7");
    assert!(
        fx.driver_calls().is_empty(),
        "with no recovery directory nothing is handed to the driver: {:?}",
        fx.driver_calls()
    );
}

/// Every terminal state finalization itself decides, as `assess-finalization`
/// may return it. The deterministic blockers (`FAILED_DIRTY_WORKTREE`,
/// `FAILED_MISSING_TOOLING`) and the states other producers own are not here:
/// no assessment reaches them.
const DECIDABLE: [(&str, bool); 11] = [
    ("EXISTING_OPEN_PR", true),
    ("IMPLEMENTED_VERIFIED", true),
    ("ALLOW_NO_OP", true),
    ("NO_DIFF_SUCCESS", true),
    ("BLOCKED_CI", false),
    ("FAILED_IMPLEMENTATION", false),
    ("FAILED_REPORTING", false),
    ("HOLLOW_SUCCESS", false),
    ("FAILED_MEANINGFUL_DIFF", false),
    ("FAILED_CLOSED_UNMERGED", false),
    ("FAILED_PR_METADATA_UNAVAILABLE", false),
];

// The validator and `workflow-complete` must agree on the vocabulary, and
// this used to be asserted by parsing the predicate bodies out of the shell
// source. Asserted here by running both: every state the validator publishes
// is reported by the completion step and appears in the vocabulary it
// publishes, and the states it calls a success are exactly the four.
#[test]
fn every_state_the_validator_publishes_is_one_workflow_complete_reports() {
    for (state, is_success) in DECIDABLE {
        let fx = Fixture::new();
        fx.with_remote();
        fx.push();
        fx.gh_pr_at(&fx.head(), green_rollup_for_tests());

        let validated = fx.validate(&as_refs(&decided(&fx, state, "7")));

        assert_eq!(
            validated.field("terminal_state"),
            state,
            "the validator must report the state the assessment decided\n{}",
            validated.describe()
        );
        assert_eq!(
            validated.field("terminal_success"),
            is_success.to_string(),
            "{state}: the published vocabulary decides whether it is a success\n{}",
            validated.describe()
        );
        assert_eq!(validated.success(), is_success, "{}", validated.describe());

        let completed = fx.run_dispatcher(
            "complete",
            &[("RECIPE_VAR_workflow_result", validated.stdout.trim())],
        );

        assert!(completed.success(), "{state}: {}", completed.describe());
        let envelope = completed.json();
        assert_eq!(
            envelope["terminal_outcome"], state,
            "{state}: completion must carry the state through: {envelope}"
        );
        let vocabulary = envelope["terminal_vocabulary"]
            .as_array()
            .expect("terminal_vocabulary is a list");
        assert!(
            vocabulary.iter().any(|known| known == state),
            "`{state}` is reported but is not in the vocabulary workflow-complete publishes: \
             {vocabulary:?}"
        );
    }
}

// A state outside that vocabulary is no decision at all, whatever it looks
// like, and the run goes to recovery rather than publishing it.
#[test]
fn a_state_the_vocabulary_does_not_carry_is_never_published() {
    for invented in ["MERGED_BY_HAND", "EXISTING_OPEN_PR_", "ok", ""] {
        let fx = Fixture::new();
        fx.with_remote();
        fx.push();
        fx.gh_pr_at(&fx.head(), green_rollup_for_tests());

        let outcome = fx.validate(&as_refs(&decided(&fx, invented, "7")));

        assert!(!outcome.success(), "`{invented}`: {}", outcome.describe());
        assert_eq!(
            outcome.field("terminal_state"),
            "FAILED_MISSING_TERMINAL_EVIDENCE",
            "`{invented}` must not be published\n{}",
            outcome.describe()
        );
    }
}
