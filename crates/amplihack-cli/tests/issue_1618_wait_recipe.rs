//! Issue #1618, AC2: in a multi-step recipe run, only the step that hit the
//! usage limit is run again; the steps before and after it run once, and
//! the run succeeds with no `usage_limit` classification.
//!
//! The recipe runner is a stub (through `RECIPE_RUNNER_RS_PATH`, as in
//! `issue_1570_recipe_context_file`) that launches three agent steps with
//! `amplihack claude -p`, the way `recipe-runner-rs` does, and reports
//! failure if any step fails. Step 2's stub `claude` is limited once. Each
//! test runs in its own process (`in_quiet_process`). See
//! `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, executable, text};
use crate::support::quiet_process::in_quiet_process;
use std::fs;
use std::process::Output;

const STUB_RUNNER: &str = r#"#!/bin/sh
case "$1" in
  --version) echo 'recipe-runner-rs 0.0.0 (stub)'; exit 0 ;;
esac
export AMPLIHACK_NONINTERACTIVE=1
for s in 1 2 3; do
  if ! STEP=$s "$AMPLIHACK_UNDER_TEST" claude --subprocess-safe -- -p "step $s" </dev/null >> "$RECORD_DIR/runner.out"; then
    printf '{"recipe_name":"issue-1618-wait","success":false,"step_results":[{"step_id":"step-%s","status":"failed","output":"","error":"agent step failed"}],"context":{}}\n' "$s"
    exit 1
  fi
done
printf '%s\n' '{"recipe_name":"issue-1618-wait","success":true,"step_results":[],"context":{}}'
"#;

const RECIPE: &str = r#"name: issue-1618-wait
description: Three agent steps; the second hits a usage limit once
version: 1.0.0
context: {}
steps:
  - id: step-1
    type: bash
    command: "true"
  - id: step-2
    type: bash
    command: "true"
  - id: step-3
    type: bash
    command: "true"
"#;

fn run_recipe(f: &Fixture, limited_step: &str) -> Output {
    run_recipe_with(f, limited_step, &[])
}

fn run_recipe_with(f: &Fixture, limited_step: &str, env: &[(&str, &str)]) -> Output {
    let runner = f.tools.join("recipe-runner-rs");
    executable(&runner, STUB_RUNNER);
    let recipe = f.root.join("recipe.yaml");
    fs::write(&recipe, RECIPE).unwrap();
    let mut c = f.cmd(&["recipe", "run"]);
    c.arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_HOME", f.home.join(".amplihack"))
        .env("LIMIT_STEP", limited_step);
    for (k, v) in env {
        c.env(k, v);
    }
    f.run(&mut c)
}

fn usage_limit_class_lines(out: &Output) -> Vec<String> {
    text(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("amplihack.recipe.failure_class") && l.contains("usage_limit"))
        .map(str::to_string)
        .collect()
}

#[test]
fn ac2_only_the_limited_step_runs_again_and_the_run_succeeds() {
    in_quiet_process(
        module_path!(),
        "ac2_only_the_limited_step_runs_again_and_the_run_succeeds",
        || {
            let f = Fixture::new();
            let out = run_recipe(&f, "2");
            assert!(out.status.success(), "{}", describe(&out));
            assert_eq!(
                [f.count("1"), f.count("2"), f.count("3")],
                [1, 2, 1],
                "completed steps are not repeated"
            );
            assert!(
                usage_limit_class_lines(&out).is_empty(),
                "the run must not be classified usage_limit: {:?}",
                usage_limit_class_lines(&out)
            );
            // Steps 1 and 3 print their only attempt; step 2 prints only its
            // retry, never the limited attempt's output.
            let runner_out = fs::read_to_string(f.records.join("runner.out")).unwrap();
            let printed: Vec<&str> = runner_out
                .lines()
                .filter(|l| l.starts_with("attempt "))
                .collect();
            assert_eq!(
                printed,
                ["attempt 1 output", "attempt 2 output", "attempt 1 output"],
                "{runner_out}"
            );
            for step in ["1", "2", "3"] {
                f.assert_isolated(step);
            }
        },
    );
}

#[test]
fn disabled_the_limited_step_ends_the_run_as_before() {
    in_quiet_process(
        module_path!(),
        "disabled_the_limited_step_ends_the_run_as_before",
        || {
            let f = Fixture::new();
            // Under `in_quiet_process` the test guard keeps the user
            // config.toml unread, so the variable turns the wait off.
            let out = run_recipe_with(&f, "2", &[("AMPLIHACK_USAGE_LIMIT_WAIT", "0")]);
            assert!(!out.status.success(), "{}", describe(&out));
            assert_eq!([f.count("1"), f.count("2"), f.count("3")], [1, 1, 0]);
        },
    );
}
