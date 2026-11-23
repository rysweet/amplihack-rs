//! Issue #2966, crusty rounds 1 to 3: the CI repair lives in one agent
//! session, and a session can end before its work does.
//!
//! Round 3 moved every decision out of the re-entry brick. The runner decides
//! when a pass is needed (`continue-ci-repair`'s condition: no terminal_state,
//! or `CI_REPAIR_UNFINISHED`) and whether the fresh pass decided (the same
//! condition on `reentered-pass-undecided` in the nested run). The step's
//! recovery agent decides whether to start another. What is left in
//! `continue_ci_repair` (`workflow_finalization_reentry.sh`, the dispatcher's
//! `reassess` mode) is mechanics: start the bundle's own assessment recipe
//! once, hand it the last answer as data, print what the pass answered, and
//! pass on the nested run's exit code.
//!
//! These cases pin exactly that, and nothing more: one nested run per call,
//! no field of any answer read, and the exit codes. They run the real
//! dispatcher against the #1641 fixture repository with a stub `amplihack`
//! first on PATH that stands for the nested `recipe run`; its exit code stands
//! for what the nested run's own `reentered-pass-undecided` step decided. The
//! conditions themselves are exercised through the pinned runner and the
//! shipped recipe in `issue_2343_finalization_outcome/wiring.rs`, and the
//! outside-in cases in `tests/gadugi/run-issue-2966-finalize-ci-repair-scenario.sh`.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::issue_1641_finalization_open_pr::fixture::{Fixture, Outcome, write_executable};

/// The nested `amplihack recipe run`. Call `n` answers `answer-<n>` (else
/// `answer-last`) as the output of the nested `assess-finalization` step and
/// exits `rc-<n>` (else 0). When `undecided-<n>` exists, its result also
/// records `reentered-pass-undecided` as failed, as the runner does when that
/// step's condition held.
const AMPLIHACK_STUB: &str = r#"#!/usr/bin/env bash
d="$GH_FIXTURE_DIR"
n=$(( $(cat "$d/amp-count" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$d/amp-count"
printf '%s\n' "$*" >> "$d/amp-calls.log"
compgen -e | grep -c '^RECIPE_VAR_' > "$d/amp-recipe-vars-$n" || :
prev=""
for a in "$@"; do
  if [ "$prev" = --context-from-file ]; then
    case "$a" in ci_repair_reentry=*) cp "${a#*=}" "$d/note-$n.json" ;; esac
  fi
  prev="$a"
done
ans="$(cat "$d/answer-$n" 2>/dev/null || cat "$d/answer-last" 2>/dev/null || :)"
guard='[]'
[ ! -f "$d/undecided-$n" ] || guard='[{"step_id":"reentered-pass-undecided","status":"failed","output":""}]'
jq -nc --arg o "$ans" --argjson g "$guard" '{success: true, step_results: ([{step_id: "assess-finalization", status: "completed", output: $o}] + $g)}'
exit "$(cat "$d/rc-$n" 2>/dev/null || echo 0)"
"#;

struct Reentry {
    fx: Fixture,
}

impl Reentry {
    fn new() -> Self {
        let fx = Fixture::new();
        write_executable(&fx.tmp.path().join("bin/amplihack"), AMPLIHACK_STUB);
        Self { fx }
    }

    fn dir(&self) -> PathBuf {
        self.fx.tmp.path().to_path_buf()
    }

    /// What nested call `n` answers; `n == 0` sets the answer for every call
    /// without its own.
    fn answer(&self, n: usize, answer: &str) {
        let name = if n == 0 {
            "answer-last".to_owned()
        } else {
            format!("answer-{n}")
        };
        fs::write(self.dir().join(name), answer).expect("record the answer");
    }

    fn exit_code(&self, n: usize, rc: i32) {
        fs::write(self.dir().join(format!("rc-{n}")), rc.to_string()).expect("record rc");
    }

    /// Nested call `n` fails on its `reentered-pass-undecided` step.
    fn undecided(&self, n: usize) {
        self.exit_code(n, 1);
        fs::write(self.dir().join(format!("undecided-{n}")), "").expect("record undecided");
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir().join("amp-calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn note(&self, n: usize) -> Value {
        let text = fs::read_to_string(self.dir().join(format!("note-{n}.json")))
            .unwrap_or_else(|e| panic!("nested call {n} was handed no note: {e}"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("note {n} is not JSON ({e}): {text}"))
    }

    /// Run `reassess` with the first pass's `answer` (None: the first pass left
    /// none).
    fn run(&self, first: Option<&str>) -> Outcome {
        let repo = self.fx.repo_path();
        let evidence = json!({"schema_version": 1, "git": {"dirty_worktree": "false"}}).to_string();
        let mut envs = vec![
            ("RECIPE_VAR_repo_path", repo.as_str()),
            ("RECIPE_VAR_finalization_evidence", evidence.as_str()),
            (
                "RECIPE_VAR_finalizer_step_status",
                r#"{"status":"ok","reporting_failure":"false"}"#,
            ),
        ];
        if let Some(first) = first {
            envs.push(("RECIPE_VAR_finalization_assessment", first));
        }
        self.fx.run_dispatcher("reassess", &envs)
    }
}

fn decision() -> String {
    json!({
        "terminal_state": "EXISTING_OPEN_PR",
        "action_taken": "repaired_ci",
        "pr_number": "7",
        "reason": "head abc, 3 green, 0 failing, 0 pending, 0 unknown",
        "next_action": "See pull request #7",
    })
    .to_string()
}

fn unfinished() -> String {
    json!({
        "terminal_state": "CI_REPAIR_UNFINISHED",
        "action_taken": "none",
        "pr_number": "7",
        "reason": "head abc, 2 green, 1 failing, 0 pending, 0 unknown",
        "next_action": "fix the smoke test",
    })
    .to_string()
}

fn printed(outcome: &Outcome) -> Value {
    serde_json::from_str(outcome.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "reassess must print one JSON answer ({e})\n{}",
            outcome.describe()
        )
    })
}

// One pass, from the bundle's own recipe (#1543), with no RECIPE_VAR_* of this
// step (#1561), and its answer printed for the step's output.
#[test]
fn one_pass_is_started_from_the_bundle_recipe_and_its_answer_printed() {
    let r = Reentry::new();
    r.answer(1, &decision());
    let outcome = r.run(None);
    assert!(outcome.success(), "{}", outcome.describe());
    let want: Value = serde_json::from_str(&decision()).unwrap();
    assert_eq!(printed(&outcome), want);
    let calls = r.calls();
    assert_eq!(calls.len(), 1, "exactly one pass: {calls:?}");
    let recipe = crate::issue_1641_finalization_open_pr::fixture::workspace_root()
        .join("amplifier-bundle/recipes/workflow-finalization-assessment.yaml");
    assert!(
        calls[0].starts_with(&format!("recipe run {} ", recipe.display())),
        "the bundle's own assessment recipe is run, never the worktree's: {}",
        calls[0]
    );
    let inherited = fs::read_to_string(r.dir().join("amp-recipe-vars-1")).unwrap();
    assert_eq!(
        inherited.trim(),
        "0",
        "the nested run inherits no RECIPE_VAR_* from this step (#1561)"
    );
    assert!(
        outcome
            .stderr
            .contains("INFO: #2966: assess-finalization did not decide"),
        "{}",
        outcome.describe()
    );
}

// The last answer is handed to the pass as data. An answer the context reader
// cannot read as one object (a crash, prose) is handed on as null, and the
// note is never empty, so the nested run always knows it is a re-entry and
// skips continue-ci-repair.
#[test]
fn the_last_answer_is_handed_to_the_pass_as_data() {
    let r = Reentry::new();
    r.answer(0, &decision());

    let outcome = r.run(Some(&unfinished()));
    assert!(outcome.success(), "{}", outcome.describe());
    let want: Value = serde_json::from_str(&unfinished()).unwrap();
    assert_eq!(r.note(1), json!({"previous_answer": want}));

    for first in [None, Some("I could not finish.")] {
        let r = Reentry::new();
        r.answer(0, &decision());
        let outcome = r.run(first);
        assert!(outcome.success(), "{first:?}: {}", outcome.describe());
        assert_eq!(r.note(1), json!({"previous_answer": null}), "{first:?}");
    }
}

// Crusty round 3, concern 2: the brick does not judge an answer. What the pass
// answered is printed as it is when the nested run succeeded, whatever its
// fields say; whether that pass decided is the nested run's own condition,
// and what the decision means is the validator's.
#[test]
fn the_brick_reads_no_field_of_the_answer() {
    let unowned = json!({
        "terminal_state": "BLOCKED_CI",
        "action_taken": "none",
        "pr_number": "7",
        "reason": "deploy: the workflow reads secrets.DEPLOY_TOKEN, which the repository does not define",
        "next_action": "",
    })
    .to_string();
    let unnamed = json!({"terminal_state": "EXISTING_OPEN_PR", "pr_number": "", "reason": "green"})
        .to_string();
    for answer in [unowned, unnamed, unfinished()] {
        let r = Reentry::new();
        r.answer(1, &answer);
        let outcome = r.run(Some(&unfinished()));
        assert!(outcome.success(), "{answer}: {}", outcome.describe());
        let want: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(printed(&outcome), want, "printed unchanged");
        assert_eq!(r.calls().len(), 1, "{answer}: one pass, no more");
    }
}

// Crusty round 3, concern 1: there is no loop. When the fresh pass did not
// decide either, the nested run fails (its reentered-pass-undecided step) and
// so does this step, after exactly one pass, for its recovery agent. The
// ERROR line carries the pass's answer as a JSON string for that agent to
// read; nothing is printed for the validator.
#[test]
fn a_fresh_pass_that_did_not_decide_fails_the_step_after_one_pass() {
    let r = Reentry::new();
    r.answer(1, &unfinished());
    r.undecided(1);
    let outcome = r.run(Some(&unfinished()));
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert_eq!(r.calls().len(), 1, "one pass, no loop: {:?}", r.calls());
    assert!(outcome.stdout.trim().is_empty(), "{}", outcome.describe());
    let quoted = serde_json::to_string(&unfinished()).unwrap();
    assert!(
        outcome.stderr.contains(&format!(
            "ERROR: #2966: the fresh assess-finalization session did not decide either (nested run exit 1); its answer, as data: {quoted}"
        )),
        "{}",
        outcome.describe()
    );
}

// A nested run that failed before its reentered-pass-undecided step could
// judge it (the runner could not start the agent, say) is reported as that,
// not as a session that did not decide. The distinction is the runner's step
// record, not anything in the answer.
#[test]
fn a_nested_run_that_failed_otherwise_says_so() {
    let r = Reentry::new();
    r.answer(1, "");
    r.exit_code(1, 1);
    let outcome = r.run(Some(&unfinished()));
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert_eq!(r.calls().len(), 1);
    assert!(
        outcome.stderr.contains(
            "ERROR: #2966: the fresh assessment run failed before it reached a decision (nested run exit 1)"
        ),
        "{}",
        outcome.describe()
    );
}

// The answer in the ERROR line is JSON-encoded, so a control character in it
// cannot start a terminal escape on the channel step recovery reads.
#[test]
fn the_error_line_escapes_control_characters() {
    let r = Reentry::new();
    r.answer(1, "half an answer \u{1b}[31mred\u{7}");
    r.undecided(1);
    let outcome = r.run(None);
    assert_eq!(outcome.output.status.code(), Some(1));
    assert!(
        !outcome.stderr.contains('\u{1b}') && !outcome.stderr.contains('\u{7}'),
        "{}",
        outcome.describe()
    );
    assert!(
        outcome
            .stderr
            .contains(r"half an answer \u001b[31mred\u0007"),
        "{}",
        outcome.describe()
    );
}

// A nested run that refuses on policy (79), stops as an orphan (75) or dies on
// a signal passes that code on, never retried here.
#[test]
fn a_policy_refusal_orphan_stop_or_signal_of_the_nested_run_is_passed_on() {
    for rc in [79, 75, 130] {
        let r = Reentry::new();
        r.answer(1, "");
        r.exit_code(1, rc);
        let outcome = r.run(Some(&unfinished()));
        assert_eq!(
            outcome.output.status.code(),
            Some(rc),
            "{}",
            outcome.describe()
        );
        assert_eq!(r.calls().len(), 1, "rc {rc}: never retried here");
        assert!(outcome.stdout.trim().is_empty(), "{}", outcome.describe());
    }
}
