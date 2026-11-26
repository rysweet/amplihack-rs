//! Issue #1801, AC2 (G1): a usage limit on the loop-health evaluator's
//! agent step is retried inside that step, so the recipe returns the
//! evaluator's verdict and auto-drive never reads a missing verdict as STUCK.
//!
//! The recipe runner is a stub (through `RECIPE_RUNNER_RS_PATH`, as in
//! `issue_1618_wait_recipe`) that runs `step-02-evaluate-loop-health` with
//! `amplihack claude -p`, the way `recipe-runner-rs` does. The stub `claude`
//! (`issue_1618_fixture`) hits the weekly limit on its first call and prints
//! the verdict on its second. A second stored login exists, so the far reset
//! becomes a capacity wait; a rotator cycle written during the wait ends it.
//!
//! - retry: the run exits 0, step-02 runs twice, the verdict is printed once,
//!   the run log has a `waiting_for_capacity` line, a capacity start line and
//!   `end reason=rotator capacity`, and the run is never classed
//!   `usage_limit`.
//! - no reset: a wait whose limit states no reset has no deadline (issue
//!   #2911); the login change the test makes ends it and step-02 retries, so
//!   the run still exits 0 with its verdict. This used to end the run: the
//!   wait wrote `failed reason=capacity_expired` and the runner classed it
//!   `usage_limit`. A usage limit is no longer a terminal outcome for the
//!   evaluator; only the wait turned off and output with no limit message in
//!   it are, as docs/reference/loop-health-evaluator.md documents.
//!
//! Both stubs are written to the test's temporary directory and run by
//! absolute path. HOME, the profile root and the config dir all lie under
//! that directory, and no rotator or tmux server is started
//! (`AMPLIHACK_ROTATE_AUTOSTART=0` in the fixture).

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, WEEKLY_TEXT, describe, events, executable, text};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{
    ROTATOR_ENDINGS, assert_no_account_detail, real_clock, rotator_running,
    run_with_rotator_cycle_during_wait, x_is_live,
};
use crate::support::quiet_process::in_quiet_process;
use std::fs;
use std::process::{Command, Output};

const VERDICT: &str = "LOOP_HEALTH: CONTINUE — the round made progress";
const STEP: &str = "2";

const STUB_RUNNER: &str = r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
case "$1" in
  --version) echo 'recipe-runner-rs 0.0.0 (stub)'; exit 0 ;;
esac
export AMPLIHACK_NONINTERACTIVE=1
# Like recipe-runner-rs, a failed agent step's output never reaches the next
# step: only a step that succeeded hands its output to step-03's reader.
if out="$(STEP=2 STDOUT_TEXT="$VERDICT" "$AMPLIHACK_UNDER_TEST" claude --subprocess-safe -- -p "evaluate loop health" </dev/null)"; then
  printf '%s\n' "$out" >> "$RECORD_DIR/runner.out"
else
  printf '%s\n' '{"recipe_name":"loop-health-evaluator","success":false,"step_results":[{"step_id":"step-02-evaluate-loop-health","status":"failed","output":"","error":"agent step failed"}],"context":{}}'
  exit 1
fi
printf '%s\n' '{"recipe_name":"loop-health-evaluator","success":true,"step_results":[],"context":{}}'
"#;

/// The evaluator's shape: evidence, the agent step, then the verdict reader.
const RECIPE: &str = r#"name: loop-health-evaluator
description: Stand-in for the loop-health evaluator; step-02 is the agent step
version: 1.0.0
context: {}
steps:
  - id: step-01-collect-loop-evidence
    type: bash
    command: "true"
  - id: step-02-evaluate-loop-health
    type: bash
    command: "true"
  - id: step-03-resolve-loop-verdict
    type: bash
    command: "true"
"#;

/// The `recipe run` command for the evaluator recipe, with `env` set.
fn evaluator_cmd(f: &Fixture, env: &[(&str, &str)]) -> Command {
    let runner = f.tools.join("recipe-runner-rs");
    executable(&runner, STUB_RUNNER);
    let recipe = f.root.join("loop-health-evaluator.yaml");
    fs::write(&recipe, RECIPE).unwrap();
    // A step on `$HOME/.claude`, limited on its first call with a weekly
    // reset too far away for a reset wait.
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.cmd(&["recipe", "run"]);
    c.arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_HOME", f.home.join(".amplihack"))
        .env("LIMIT_STEP", STEP)
        .env("LIMIT_MESSAGE", WEEKLY_TEXT)
        .env("VERDICT", VERDICT);
    for (k, v) in env {
        c.env(k, v);
    }
    c
}

/// Runs the evaluator on the real clock, sped up as in
/// `issue_1801_capacity_wait` (the fixture's virtual clock would put the
/// wait's start before any real-time cycle), and writes a fresh rotator
/// cycle once the step's wait has printed its `start` line. Issue #2019: the
/// cycle used to be written a fixed 1.2 s after step-02's first call, which
/// on a loaded host could come before the wait started, so the cycle was not
/// fresh and the wait ran to its maximum.
fn run_evaluator_with_cycle_during_wait(f: &Fixture) -> Output {
    let mut c = evaluator_cmd(f, &[]);
    real_clock(&mut c);
    run_with_rotator_cycle_during_wait(f, &mut c)
}

fn usage_limit_class_lines(out: &Output) -> Vec<String> {
    text(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("amplihack.recipe.failure_class") && l.contains("usage_limit"))
        .map(str::to_string)
        .collect()
}

/// Verdict lines handed on to step-03, the evaluator's verdict reader.
fn verdicts(f: &Fixture) -> usize {
    fs::read_to_string(f.records.join("runner.out"))
        .unwrap_or_default()
        .lines()
        .filter(|l| *l == VERDICT)
        .count()
}

/// G1: a usage limit on the evaluator's agent step is retried inside the
/// step, so the recipe returns the evaluator's verdict.
///
/// Issue #2911 added one line to the sequence: the weekly limit's reset is
/// too far away to wait for, so the wait has no reset it can end on and says
/// `waiting_for_capacity reason=reset_too_far` immediately before its own
/// `start`. The reason token used to be logged as a terminal
/// `skipped reason=reset_too_far`, which is what ended the runs the issue
/// reports; it now names a wait.
#[test]
fn g1_a_limited_evaluator_step_is_retried_and_returns_its_verdict() {
    in_quiet_process(
        module_path!(),
        "g1_a_limited_evaluator_step_is_retried_and_returns_its_verdict",
        || {
            let f = with_profiles();
            x_is_live(&f);
            let _rotator = rotator_running(&f);
            let out = run_evaluator_with_cycle_during_wait(&f);

            assert!(out.status.success(), "{}", describe(&out));
            assert_eq!(f.count(STEP), 2, "one limited call, one retry");
            assert_eq!(verdicts(&f), 1, "the verdict is printed once");
            assert!(
                usage_limit_class_lines(&out).is_empty(),
                "a retried step is not a usage_limit failure: {:?}",
                usage_limit_class_lines(&out)
            );
            let ev = events(&out);
            assert!(
                ev.first().is_some_and(|l| l.starts_with(
                    "waiting_for_capacity reason=reset_too_far harness=claude until="
                )),
                "{ev:?}"
            );
            assert!(
                ev.get(1)
                    .is_some_and(|l| l.starts_with("start reason=usage_limit wait=capacity ")),
                "the line comes immediately before the wait's own start: {ev:?}"
            );
            // The rotator's one write ends the wait under either reason, as
            // in `issue_1801_capacity_wait::ac1` (#3170).
            assert!(
                ev.get(2)
                    .is_some_and(|l| ROTATOR_ENDINGS.contains(&l.as_str())),
                "the wait ends on the rotator's write: {ev:?}"
            );
            assert_eq!(
                ev.get(3).map(String::as_str),
                Some("resume attempt=2"),
                "{ev:?}"
            );
            assert!(
                !text(&out.stderr).contains("skipped reason=reset_too_far"),
                "AC3 and issue #2911: a far reset waits and never skips the step"
            );
            f.assert_isolated(STEP);
            assert_no_account_detail(&f, &out);
        },
    );
}

/// Issue #2911: a capacity wait with no reset does not fail the evaluator
/// step. It waits for an event, the step retries, and the verdict arrives.
///
/// This used to assert the opposite: a wait that reached its deadline wrote
/// `failed reason=capacity_expired`, step-02 ran once, produced no verdict,
/// and the runner classed the run `usage_limit`. That reason is gone, and
/// since issue #3037 so is the last one, `step_timeout`: a limit that is
/// still in force when a wait ends means *wait again*, not *stop the run*.
/// The evaluator keeps a terminal usage-limit outcome only for a step whose
/// wait is turned off and for output with no limit message in it; see
/// docs/reference/loop-health-evaluator.md, "When the wait does not lift the
/// limit".
///
/// Nothing on the host ends the wait: the rotator's ledger names a cycle
/// from before the step and no reading shows a login with room, and the
/// limit states no reset, so the wait has no deadline. The test changes the
/// login once the wait is under way, as a person would.
#[test]
fn g1_an_evaluator_wait_with_no_reset_retries_on_an_event_and_returns_its_verdict() {
    in_quiet_process(
        module_path!(),
        "g1_an_evaluator_wait_with_no_reset_retries_on_an_event_and_returns_its_verdict",
        || {
            let f = with_profiles();
            x_is_live(&f);
            let _rotator = rotator_running(&f);
            let out = f.run_with_a_login_change_in(
                &f.home.join(".claude"),
                &mut evaluator_cmd(
                    &f,
                    &[(
                        "LIMIT_MESSAGE",
                        "You've hit your session limit · try again later",
                    )],
                ),
            );

            assert!(out.status.success(), "{}", describe(&out));
            assert_eq!(f.count(STEP), 2, "the wait earned a retry");
            assert_eq!(verdicts(&f), 1, "the verdict is printed once");
            assert!(
                usage_limit_class_lines(&out).is_empty(),
                "a limit that a wait did not lift is not a failure class: {:?}",
                usage_limit_class_lines(&out)
            );
            let ev = events(&out);
            assert!(ev.iter().any(|l| l == "end reason=login changed"), "{ev:?}");
            assert!(
                ev.iter().any(|l| l == "resume attempt=2"),
                "the step retries after the wait: {ev:?}"
            );
            assert!(
                !ev.iter()
                    .any(|l| l.starts_with("failed ") || l.starts_with("skipped ")),
                "nothing refuses the limit or ends the step: {ev:?}"
            );
            f.assert_isolated(STEP);
            assert_no_account_detail(&f, &out);
        },
    );
}
