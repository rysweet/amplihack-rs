//! R2 (AC14): a check passed in the build loop is not run again after it.
//! smart-orchestrator runs smart-reflect-loop and smart-validate-summarize
//! after default-workflow; when the build loop ran (`build_loop_result` is
//! set), reflection and outside-in validation do not run, and the summary
//! still does. Each recipe runs through the real binary and runner with PR 1's
//! harness and a stub agent; the test reads the runner's own step results.

#![cfg(target_os = "linux")]

use crate::issue_3129_goal_loop::support::{Harness, describe};
use serde_json::Value;

fn run(h: &Harness, recipe: &str, extra: &[&str]) -> Value {
    let mut args: Vec<String> = vec!["recipe".into(), "run".into(), recipe.into()];
    for kv in [
        "task_type=Development",
        "round_1_result=PR https://github.com/o/r/pull/7 opened and marked ready",
        "task_description=Build the widget.",
        "decomposition_json={}",
        "session_info={}",
    ]
    .iter()
    .chain(extra.iter())
    {
        args.push("-c".into());
        args.push((*kv).to_string());
    }
    for a in [
        "--format",
        "json",
        "-w",
        h.repo.to_str().unwrap(),
        "--no-detach",
    ] {
        args.push(a.into());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = h.command(&refs).output().expect("amplihack recipe run");
    assert!(out.status.success(), "{recipe}: {}", describe(&out));
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", describe(&out)))
}

/// The runner's status for one step, lower case.
fn status(result: &Value, id: &str) -> String {
    result["step_results"]
        .as_array()
        .unwrap_or_else(|| panic!("no step_results: {result}"))
        .iter()
        .find(|s| s["step_id"] == id)
        .unwrap_or_else(|| panic!("no step {id}: {result}"))["status"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase()
}

#[test]
fn after_the_build_loop_reflection_does_not_run() {
    let h = Harness::new();
    let r = run(
        &h,
        "smart-reflect-loop",
        &["build_loop_result=Goal loop: {\"end\":\"ready\"}"],
    );
    assert_eq!(status(&r, "reflect-round-1"), "skipped", "{r}");
    assert_eq!(status(&r, "reflect-final"), "skipped", "{r}");
}

#[test]
fn without_the_build_loop_reflection_still_runs() {
    let h = Harness::new();
    let r = run(&h, "smart-reflect-loop", &[]);
    assert_ne!(status(&r, "reflect-round-1"), "skipped", "{r}");
}

#[test]
fn after_the_build_loop_validation_does_not_run_and_the_summary_does() {
    let h = Harness::new();
    let r = run(
        &h,
        "smart-validate-summarize",
        &["build_loop_result=Goal loop: {\"end\":\"ready\"}"],
    );
    assert_eq!(status(&r, "validate-outside-in-testing"), "skipped", "{r}");
    assert_ne!(status(&r, "summarize"), "skipped", "{r}");
}
