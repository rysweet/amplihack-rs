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
//!   the run log has a capacity start line and `end reason=rotator capacity`,
//!   and the run is never classed `usage_limit`.
//! - expiry: with a zero maximum, step-02 runs once, prints no verdict, and
//!   the run fails with the `usage_limit` failure class, which auto-drive
//!   reports as USAGE_LIMIT (`test-auto-drive-to-merge.sh`).
//!
//! Both stubs are written to the test's temporary directory and run by
//! absolute path. HOME, the profile root and the config dir all lie under
//! that directory, and no rotator or tmux server is started
//! (`AMPLIHACK_ROTATE_AUTOSTART=0` in the fixture).

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, WEEKLY_TEXT, describe, events, executable, text};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{assert_no_account_detail, real_now, write_cycle, x_is_live};
use crate::support::quiet_process::in_quiet_process;
use std::fs;
use std::process::Output;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const VERDICT: &str = "LOOP_HEALTH: CONTINUE — the round made progress";
const STEP: &str = "2";

const STUB_RUNNER: &str = r#"#!/bin/sh
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

fn run_evaluator(f: &Fixture, real_clock: bool, env: &[(&str, &str)]) -> Output {
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
    if real_clock {
        // The fixture's virtual clock would put the wait's start before any
        // real-time cycle. Sped up as in `issue_1801_capacity_wait`.
        c.env_remove("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "1000");
    }
    for (k, v) in env {
        c.env(k, v);
    }
    f.run(&mut c)
}

/// Writes a fresh rotator cycle once step-02 has made its first call and
/// the second has turned over.
fn rotator_cycle_during_wait(f: &Fixture) -> JoinHandle<()> {
    let count = f.records.join(format!("count.{STEP}"));
    let profiles = f.profiles.clone();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let seen = fs::read_to_string(&count)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            if seen == Some(1) {
                std::thread::sleep(Duration::from_millis(1200));
                write_cycle(&profiles, "Y", real_now());
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })
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

#[test]
fn g1_a_limited_evaluator_step_is_retried_and_returns_its_verdict() {
    in_quiet_process(
        module_path!(),
        "g1_a_limited_evaluator_step_is_retried_and_returns_its_verdict",
        || {
            let f = with_profiles();
            x_is_live(&f);
            let writer = rotator_cycle_during_wait(&f);
            let out = run_evaluator(&f, true, &[]);
            writer.join().unwrap();

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
                ev.first()
                    .is_some_and(|l| l.starts_with("start reason=usage_limit wait=capacity ")),
                "{ev:?}"
            );
            assert!(
                ev.iter().any(|l| l == "end reason=rotator capacity"),
                "{ev:?}"
            );
            assert!(
                !text(&out.stderr).contains("reason=reset_too_far"),
                "AC3: no reset_too_far skip while another login could take over"
            );
            f.assert_isolated(STEP);
            assert_no_account_detail(&f, &out);
        },
    );
}

#[test]
fn g1_an_evaluator_wait_that_expires_fails_the_step_as_usage_limit() {
    in_quiet_process(
        module_path!(),
        "g1_an_evaluator_wait_that_expires_fails_the_step_as_usage_limit",
        || {
            let f = with_profiles();
            x_is_live(&f);
            let out = run_evaluator(&f, false, &[("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "0")]);

            assert!(!out.status.success(), "{}", describe(&out));
            assert_eq!(f.count(STEP), 1, "no retry once the wait expired");
            assert_eq!(verdicts(&f), 0, "no verdict to misread");
            let classed = usage_limit_class_lines(&out);
            assert!(
                classed
                    .iter()
                    .any(|l| l.starts_with("amplihack.recipe.failure_class {")),
                "R5: the run is classed usage_limit with the runner's marker: {}",
                describe(&out)
            );
            assert!(
                events(&out)
                    .iter()
                    .any(|l| l == "end reason=capacity expired"),
                "{:?}",
                events(&out)
            );
            f.assert_isolated(STEP);
            assert_no_account_detail(&f, &out);
        },
    );
}
