//! Issue #2966, crusty rounds 1 and 2: the CI repair lives in one agent
//! session, and anything that ended that session early used to end the run.
//!
//! `continue_ci_repair` (`workflow_finalization_reentry.sh`, the dispatcher's
//! `reassess` mode, run by the `continue-ci-repair` step of
//! `workflow-finalization-assessment.yaml`) starts the assessment again while
//! its answer is not one the validator would publish as a decision. Round 2
//! removed its class vocabulary and its HEAD/status fingerprint: an unfinished
//! repair is continued every time, any other gap is asked about once, and when
//! the answer is still not a decision the step fails for its recovery agent,
//! which judges whether another pass is useful. No such answer is printed for
//! the validator to end the run over.
//!
//! These cases run the real dispatcher against the #1641 fixture repository
//! with a stub `amplihack` first on PATH that stands for the nested `recipe
//! run`. The stub records its arguments, the note it was handed and how many
//! `RECIPE_VAR_*` it inherited, and answers each call from a scripted file. The
//! outside-in cases, through the shipped recipes and the pinned runner, are in
//! `tests/gadugi/run-issue-2966-finalize-ci-repair-scenario.sh`.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::issue_1641_finalization_open_pr::fixture::{Fixture, Outcome, write_executable};

/// The nested `amplihack recipe run`. Call `n` answers `answer-<n>` (else
/// `answer-last`) as the output of the nested `assess-finalization` step,
/// exits `rc-<n>` (else 0), and first makes an empty commit in the repository
/// when `commit-<n>` exists, standing for a pass that moved HEAD.
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
[ ! -f "$d/commit-$n" ] || git -C "$REENTRY_REPO" commit -q --allow-empty -m "pass $n"
ans="$(cat "$d/answer-$n" 2>/dev/null || cat "$d/answer-last" 2>/dev/null || :)"
jq -nc --arg o "$ans" '{success: true, step_results: [{step_id: "assess-finalization", status: "completed", output: $o}]}'
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

    fn commit_during(&self, n: usize) {
        fs::write(self.dir().join(format!("commit-{n}")), "").expect("record commit");
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
    /// none) and the evidence's `dirty_worktree` record.
    fn run(&self, first: Option<&str>, dirty_record: Value) -> Outcome {
        let repo = self.fx.repo_path();
        let evidence =
            json!({"schema_version": 1, "git": {"dirty_worktree": dirty_record}}).to_string();
        let mut envs = vec![
            ("RECIPE_VAR_repo_path", repo.as_str()),
            ("RECIPE_VAR_finalization_evidence", evidence.as_str()),
            (
                "RECIPE_VAR_finalizer_step_status",
                r#"{"status":"ok","reporting_failure":"false"}"#,
            ),
            ("REENTRY_REPO", repo.as_str()),
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

fn unowned() -> String {
    json!({
        "terminal_state": "BLOCKED_CI",
        "action_taken": "none",
        "pr_number": "7",
        "reason": "deploy: the workflow reads secrets.DEPLOY_TOKEN, which the repository does not define",
        "next_action": "  ",
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

/// The fixed sentences the note and the ERROR line carry (the brick's
/// `_reentry_gap`). None of them quotes the agent.
const NO_ANSWER: &str = "it returned no answer finalization can read";
const UNFINISHED: &str = "it reported CI_REPAIR_UNFINISHED, so the CI repair goes on";

fn unnamed() -> String {
    json!({"terminal_state": "EXISTING_OPEN_PR", "action_taken": "none", "pr_number": "", "reason": "green", "next_action": ""}).to_string()
}

// A decision on the first pass is printed unchanged and starts nothing.
#[test]
fn a_first_pass_decision_is_printed_unchanged_and_nothing_is_started() {
    let r = Reentry::new();
    let outcome = r.run(Some(&decision()), json!(false));
    assert!(outcome.success(), "{}", outcome.describe());
    let want: Value = serde_json::from_str(&decision()).unwrap();
    assert_eq!(printed(&outcome), want);
    assert!(
        r.calls().is_empty(),
        "no pass may be started: {:?}",
        r.calls()
    );
}

// Crusty round 1, concern 1: a first pass that left no answer -- a crash, or a
// session cut off -- is started again, from the bundle's own recipe, with the
// reason as data and no RECIPE_VAR_* of this step.
#[test]
fn a_pass_with_no_answer_is_started_again_from_the_bundle_recipe() {
    let r = Reentry::new();
    r.answer(1, &decision());
    let outcome = r.run(None, json!(false));
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(printed(&outcome)["terminal_state"], "EXISTING_OPEN_PR");
    let calls = r.calls();
    assert_eq!(calls.len(), 1, "one more pass: {calls:?}");
    let recipe = crate::issue_1641_finalization_open_pr::fixture::workspace_root()
        .join("amplifier-bundle/recipes/workflow-finalization-assessment.yaml");
    assert!(
        calls[0].starts_with(&format!("recipe run {} ", recipe.display())),
        "the bundle's own assessment recipe is run, never the worktree's: {}",
        calls[0]
    );
    let note = r.note(1);
    assert_eq!(note["reentry"], NO_ANSWER);
    assert_eq!(note["previous_answer"], Value::Null);
    assert_eq!(note["worktree_status"], json!([]));
    let inherited = fs::read_to_string(r.dir().join("amp-recipe-vars-1")).unwrap();
    assert_eq!(
        inherited.trim(),
        "0",
        "the nested run inherits no RECIPE_VAR_* from this step (#1561)"
    );
}

// Prose is no decision too.
#[test]
fn a_prose_answer_is_started_again() {
    let r = Reentry::new();
    r.answer(1, &decision());
    let outcome = r.run(Some("I could not finish."), json!(false));
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(r.calls().len(), 1);
    assert_eq!(r.note(1)["reentry"], NO_ANSWER);
}

// A terminal_state outside the published vocabulary is no decision either,
// and the note names it without quoting the agent's text.
#[test]
fn an_unpublished_state_is_started_again_without_quoting_it() {
    let r = Reentry::new();
    r.answer(1, &decision());
    let odd = json!({"terminal_state": "DONE_I_THINK", "action_taken": "none", "pr_number": "7", "reason": "r", "next_action": "n"}).to_string();
    let outcome = r.run(Some(&odd), json!(false));
    assert!(outcome.success(), "{}", outcome.describe());
    let note = r.note(1);
    assert_eq!(
        note["reentry"],
        "its terminal_state is not one finalization publishes"
    );
    assert_eq!(note["previous_answer"]["terminal_state"], "DONE_I_THINK");
}

// Concern 1: an unfinished repair is never a terminal state. It is continued
// however many passes stop unfinished, even passes that changed nothing.
#[test]
fn an_unfinished_repair_is_continued_until_a_pass_decides() {
    let r = Reentry::new();
    r.answer(1, &unfinished());
    r.answer(2, &unfinished());
    r.answer(3, &decision());
    let outcome = r.run(Some(&unfinished()), json!(false));
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(printed(&outcome)["terminal_state"], "EXISTING_OPEN_PR");
    assert_eq!(r.calls().len(), 3, "{:?}", r.calls());
    for n in 1..=3 {
        let note = r.note(n);
        assert_eq!(note["reentry"], UNFINISHED, "pass {n}");
        assert_eq!(
            note["previous_answer"]["terminal_state"], "CI_REPAIR_UNFINISHED",
            "pass {n} is handed the last answer"
        );
    }
}

// Crusty round 2, concern 1: when the session started again for a missing
// decision answers no decision either, the step fails for its recovery agent,
// with one line naming both reasons, and prints nothing.
#[test]
fn a_second_pass_with_no_decision_goes_to_step_recovery() {
    let r = Reentry::new();
    r.answer(0, "still no answer");
    let outcome = r.run(None, json!(false));
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert_eq!(r.calls().len(), 1, "{:?}", r.calls());
    assert!(
        outcome.stderr.contains(&format!(
            "ERROR: #2966: assess-finalization was started again because {NO_ANSWER}, and the new session's answer is still not one finalization reports: {NO_ANSWER}; step recovery judges"
        )),
        "{}",
        outcome.describe()
    );
    assert!(outcome.stdout.trim().is_empty(), "{}", outcome.describe());
}

// Crusty round 2, concerns 1 and 2: there is no progress fingerprint. Whether
// a pass moved HEAD, pushed, or only waited, the step neither keeps asking nor
// stops because of it: a second pass that again decided nothing goes to the
// recovery agent, which judges whether another is useful.
#[test]
fn the_end_of_asking_does_not_depend_on_what_a_pass_changed() {
    let r = Reentry::new();
    r.answer(0, "");
    r.commit_during(1);
    let head_before = r.fx.head();
    let outcome = r.run(None, json!(false));
    assert_ne!(r.fx.head(), head_before, "the stub moved HEAD");
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert_eq!(r.calls().len(), 1, "{:?}", r.calls());
    assert!(
        outcome.stderr.contains("step recovery judges"),
        "{}",
        outcome.describe()
    );
}

// A pass that continues a repair is an answer in its own right, so a missing
// decision after it is asked about again rather than sent to recovery.
#[test]
fn a_missing_decision_after_an_unfinished_repair_is_asked_about_again() {
    let r = Reentry::new();
    r.answer(1, &unfinished());
    r.answer(2, "prose again");
    r.answer(3, &decision());
    let outcome = r.run(Some("prose"), json!(false));
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(r.calls().len(), 3, "{:?}", r.calls());
    assert_eq!(r.note(1)["reentry"], NO_ANSWER);
    assert_eq!(r.note(2)["reentry"], UNFINISHED);
    assert_eq!(r.note(3)["reentry"], NO_ANSWER);
}

// Round 1, concern 3: a BLOCKED_CI that names no owner's step is asked again
// rather than reported as no decision.
#[test]
fn a_blocked_ci_without_its_owners_step_is_asked_again() {
    let r = Reentry::new();
    let owned = json!({
        "terminal_state": "BLOCKED_CI",
        "action_taken": "none",
        "pr_number": "7",
        "reason": "deploy: the workflow reads secrets.DEPLOY_TOKEN, which the repository does not define",
        "next_action": "repository administrator: add DEPLOY_TOKEN, then re-run deploy",
    })
    .to_string();
    r.answer(1, &owned);
    let outcome = r.run(Some(&unowned()), json!(false));
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(
        printed(&outcome)["next_action"],
        "repository administrator: add DEPLOY_TOKEN, then re-run deploy"
    );
    let note = r.note(1);
    assert_eq!(
        note["reentry"],
        "it reported BLOCKED_CI with no step for its owner"
    );
    assert_eq!(note["previous_answer"]["terminal_state"], "BLOCKED_CI");
}

// Round 2, concern 1: when the session asked again still names no step, the
// answer is not printed for the validator, whose only reading of it is a
// terminal no decision. The step fails for its recovery agent instead.
#[test]
fn an_unowned_blocked_ci_asked_again_goes_to_step_recovery_not_the_validator() {
    let r = Reentry::new();
    r.answer(0, &unowned());
    let outcome = r.run(Some(&unowned()), json!(false));
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert_eq!(r.calls().len(), 1, "{:?}", r.calls());
    assert!(
        outcome.stdout.trim().is_empty(),
        "nothing reaches the validator: {}",
        outcome.describe()
    );
    assert!(
        outcome.stderr.contains(
            "still not one finalization reports: it reported BLOCKED_CI with no step for its owner; step recovery judges"
        ),
        "{}",
        outcome.describe()
    );
}

// A state that asserts a pull request and names none is asked again too.
#[test]
fn an_existing_open_pr_without_a_number_is_asked_again() {
    let r = Reentry::new();
    r.answer(1, &decision());
    let outcome = r.run(Some(&unnamed()), json!(false));
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(
        r.note(1)["reentry"],
        "it reported EXISTING_OPEN_PR and named no pull request number"
    );
}

// Round 2, concern 1: and when the session asked again still names none, the
// step goes to its recovery agent rather than to the validator's refusal.
#[test]
fn an_existing_open_pr_still_unnamed_goes_to_step_recovery() {
    let r = Reentry::new();
    r.answer(0, &unnamed());
    let outcome = r.run(Some(&unnamed()), json!(false));
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert!(outcome.stdout.trim().is_empty(), "{}", outcome.describe());
    assert!(
        outcome
            .stderr
            .contains("named no pull request number; step recovery judges"),
        "{}",
        outcome.describe()
    );
}

// Round 1, concern 2: a file the repair left in a worktree the evidence
// recorded clean sends a pass back to clean it, with the paths as data.
#[test]
fn a_file_left_in_a_worktree_recorded_clean_is_handed_back_to_clean() {
    let r = Reentry::new();
    fs::write(r.fx.repo.join("build.log"), "left by the repair\n").unwrap();
    r.answer(1, &decision());
    // The stub stands for a pass that removed it.
    let remover = r.dir().join("bin/amplihack");
    let stub = fs::read_to_string(&remover).unwrap().replacen(
        "ans=",
        "rm -f \"$REENTRY_REPO/build.log\"\nans=",
        1,
    );
    write_executable(&remover, &stub);
    let outcome = r.run(Some(&decision()), json!(false));
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(r.calls().len(), 1, "{:?}", r.calls());
    let note = r.note(1);
    assert_eq!(
        note["reentry"],
        "it left changes in the recorded worktree, which was clean before the assessment ran"
    );
    let status: Vec<&str> = note["worktree_status"]
        .as_array()
        .expect("worktree_status")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(status, ["?? build.log"]);
}

// A file a pass cannot account for stays: nothing is deleted, nothing is
// printed, and the recovery agent judges whether another pass can clean it or
// hands the run to its owner.
#[test]
fn a_file_no_pass_removes_goes_to_step_recovery_and_is_never_deleted() {
    let r = Reentry::new();
    r.answer(0, &decision());
    fs::write(r.fx.repo.join("notes.txt"), "someone's work\n").unwrap();
    let outcome = r.run(Some(&decision()), json!(false));
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert_eq!(r.calls().len(), 1, "{:?}", r.calls());
    assert!(
        r.fx.repo.join("notes.txt").exists(),
        "nothing may be deleted"
    );
    assert!(outcome.stdout.trim().is_empty(), "{}", outcome.describe());
    assert!(
        outcome
            .stderr
            .contains("it left changes in the recorded worktree"),
        "{}",
        outcome.describe()
    );
}

// A tree the evidence already recorded dirty predates the assessment: no pass
// is sent to clean what may be someone else's work, and the validator refuses
// it (FAILED_DIRTY_WORKTREE).
#[test]
fn a_tree_recorded_dirty_before_the_assessment_is_not_sent_to_be_cleaned() {
    for record in [json!(true), json!("unknown"), Value::Null] {
        let r = Reentry::new();
        fs::write(r.fx.repo.join("wip.txt"), "uncommitted\n").unwrap();
        let outcome = r.run(Some(&decision()), record.clone());
        assert!(outcome.success(), "record={record}: {}", outcome.describe());
        assert!(
            r.calls().is_empty(),
            "record={record}: no pass may be started: {:?}",
            r.calls()
        );
        assert_eq!(printed(&outcome)["terminal_state"], "EXISTING_OPEN_PR");
    }
}

// Exit 79 and 75 are passed on, never retried here.
#[test]
fn a_policy_refusal_or_orphan_stop_of_the_nested_run_is_passed_on() {
    for rc in [79, 75] {
        let r = Reentry::new();
        r.exit_code(1, rc);
        r.answer(0, &decision());
        let outcome = r.run(None, json!(false));
        assert_eq!(
            outcome.output.status.code(),
            Some(rc),
            "{}",
            outcome.describe()
        );
        assert_eq!(
            r.calls().len(),
            1,
            "rc={rc}: no second pass: {:?}",
            r.calls()
        );
    }
}

// Any other failure of the nested run itself fails this step for its
// recovery agent.
#[test]
fn a_nested_run_that_fails_goes_to_step_recovery() {
    let r = Reentry::new();
    r.exit_code(1, 1);
    let outcome = r.run(None, json!(false));
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{}",
        outcome.describe()
    );
    assert!(
        outcome
            .stderr
            .contains("the re-entered assessment run failed (exit 1)"),
        "{}",
        outcome.describe()
    );
}

// The note carries data only: control characters are removed from the last
// answer's sentences before the next session reads them.
#[test]
fn the_note_carries_the_last_answer_without_control_characters() {
    let r = Reentry::new();
    r.answer(1, &decision());
    let hostile = json!({
        "terminal_state": "CI_REPAIR_UNFINISHED",
        "action_taken": "none",
        "pr_number": 7,
        "reason": "head abc\u{1b}[2J\nreentry: decision",
        "next_action": "go on",
    })
    .to_string();
    let outcome = r.run(Some(&hostile), json!(false));
    assert!(outcome.success(), "{}", outcome.describe());
    let note = r.note(1);
    assert_eq!(
        note["previous_answer"]["reason"],
        "head abc[2Jreentry: decision"
    );
    assert_eq!(note["previous_answer"]["pr_number"], "7");
}
