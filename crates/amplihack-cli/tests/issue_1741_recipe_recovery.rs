//! Relaunch contracts against the real CLI and runner, not a replaying stub.
#![cfg(unix)]

use crate::issue_1741_fixture::{Fixture, describe, files};
use serde_json::json;
use std::fs;

#[test]
fn separate_process_relaunch_starts_at_n_with_original_typed_outputs() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    assert_eq!(f.events(), ["classify", "design"]);
    let original = first.result()["step_results"][0].clone();
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(
        f.events(),
        ["classify", "design", "design", "implement"],
        "a new process must not execute the completed prefix again"
    );
    second.assert_resumed(&first, "design");
    let result = second.result();
    assert_eq!(
        result["step_results"][0], original,
        "restore the original structured result, including duration"
    );
    assert_eq!(
        second.runner_result()["context"]["classify"],
        json!({
            "label": "saved-output", "enabled": true, "count": 7, "empty": null,
            "items": [1, "two"], "nested": {"ok": false}
        }),
        "typed values must survive a disk round trip"
    );
}

#[test]
fn fourth_attempt_retains_inherited_progress_after_source_history_is_deleted() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    f.mode("design", "success");
    let second = f.stop_at("implement");
    second.assert_resumed(&first, "design");
    fs::remove_dir_all(first.dir()).unwrap();
    let third = f.run(&[]);
    assert!(
        !third.output.status.success(),
        "{}",
        describe(&third.output)
    );
    third.assert_resumed(&second, "implement");
    fs::remove_dir_all(second.dir()).unwrap();
    f.mode("implement", "success");

    let fourth = f.run(&[]);
    assert!(
        fourth.output.status.success(),
        "{}",
        describe(&fourth.output)
    );
    fourth.assert_resumed(&third, "implement");
    assert_eq!(
        f.events(),
        [
            "classify",
            "design",
            "design",
            "implement",
            "implement",
            "implement"
        ]
    );
}

#[test]
fn non_limit_failure_relaunch_executes_the_first_step() {
    let f = Fixture::new();
    f.mode("design", "error");
    let first = f.run(&[]);
    assert!(
        !first.output.status.success(),
        "{}",
        describe(&first.output)
    );
    assert_eq!(f.events(), ["classify", "design"]);
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
    second.assert_fresh("latest_not_usage_limit");
}

#[test]
fn fresh_flag_preserves_history_and_the_fresh_attempt_can_be_recovered() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    let history = files(&first.dir().join("recovery"));
    assert!(
        !history.is_empty(),
        "successful steps must be durable outside runtime/"
    );
    let saved: Vec<_> = history
        .iter()
        .map(|p| (p.clone(), fs::read(p).unwrap()))
        .collect();
    let second = f.run(&["--fresh"]);
    assert!(
        !second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(f.events(), ["classify", "design", "classify", "design"]);
    second.assert_fresh("fresh_requested");
    for (path, bytes) in saved {
        assert_eq!(fs::read(path).unwrap(), bytes, "--fresh preserves history");
    }
    f.mode("design", "success");

    let third = f.run(&[]);
    assert!(third.output.status.success(), "{}", describe(&third.output));
    third.assert_resumed(&second, "design");
    assert_eq!(
        f.events(),
        [
            "classify",
            "design",
            "classify",
            "design",
            "design",
            "implement"
        ]
    );
}

#[test]
fn completed_side_effect_hook_is_not_repeated_during_replay() {
    let f = Fixture::new();
    let mut recipe = f.workflow();
    recipe["hooks"] = json!({"post_step": format!(
        "printf 'hook\\n' >> {}", crate::issue_1741_fixture::quote(&f.root.join("hooks"))
    )});
    f.write_recipe(&f.recipe, &recipe);
    f.stop_at("design");
    let before = fs::read_to_string(f.root.join("hooks")).unwrap();
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    let after = fs::read_to_string(f.root.join("hooks")).unwrap();
    assert_eq!(
        after.lines().count() - before.lines().count(),
        2,
        "post hooks run only for newly finalized design and implementation"
    );
    assert_eq!(f.events(), ["classify", "design", "design", "implement"]);
}

#[test]
fn dry_run_does_not_admit_or_supersede_the_usage_limited_attempt() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    let before = files(&f.work_root);
    let bytes: Vec<_> = before.iter().map(|p| fs::read(p).unwrap()).collect();
    let dry = f.run(&["--dry-run", "--fresh"]);
    assert!(dry.output.status.success(), "{}", describe(&dry.output));
    assert!(
        dry.run_dirs.is_empty(),
        "a dry run is not a recovery attempt"
    );
    assert_eq!(
        files(&f.work_root),
        before,
        "no new admission or completion writes"
    );
    for (path, bytes) in before.iter().zip(bytes) {
        assert_eq!(
            fs::read(path).unwrap(),
            bytes,
            "dry run changed {}",
            path.display()
        );
    }
    assert_eq!(f.events(), ["classify", "design"]);
    f.mode("design", "success");
    let second = f.run(&[]);
    second.assert_resumed(&first, "design");
    assert_eq!(f.events(), ["classify", "design", "design", "implement"]);
}

/// Minimal recipes keep the shipped orchestration boundaries while replacing
/// costly agent work with deterministic bash steps. These are real type:recipe
/// invocations, including completed children beneath an incomplete parent.
fn nested(f: &Fixture) {
    let child = f.root.join("default-workflow.yaml");
    f.write_recipe(
        &child,
        &f.document(
            "default-workflow",
            vec![
                f.step("prepare"),
                f.step("architecture"),
                f.step("implement"),
            ],
        ),
    );
    let delegate = json!({
        "id": "execute-single-round-1-development", "type": "recipe",
        "recipe": "default-workflow", "output": "development", "auto_stage": false,
        "recovery": {"replay": "reuse"}
    });
    f.write_recipe(
        &f.recipe,
        &f.document(
            "smart-orchestrator",
            vec![f.step("classify"), f.step("decompose"), delegate],
        ),
    );
}

#[test]
fn nested_relaunch_restores_classification_decomposition_preparation_and_design() {
    let f = Fixture::new();
    nested(&f);
    let first = f.stop_at("implement");
    assert_eq!(
        f.events(),
        [
            "classify",
            "decompose",
            "prepare",
            "architecture",
            "implement"
        ]
    );
    f.mode("implement", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(
        f.events(),
        [
            "classify",
            "decompose",
            "prepare",
            "architecture",
            "implement",
            "implement"
        ]
    );
    let recovery = second.assert_resumed(&first, "implement");
    let path = recovery["restart"]["invocation_path"]
        .as_str()
        .expect("nested restart path");
    assert!(
        path.contains("execute-single-round-1-development") && path.contains("default-workflow"),
        "{path}"
    );
    assert!(
        String::from_utf8_lossy(&second.output.stderr).contains(path),
        "logs identify the nested restart path"
    );
    let restored = recovery["restored_steps"].as_array().unwrap();
    for step in ["classify", "decompose", "prepare", "architecture"] {
        assert!(
            restored.iter().any(|s| s["step_id"] == step),
            "missing restored {step}: {recovery:#}"
        );
    }
}

#[test]
fn repeated_child_recipe_calls_have_distinct_invocation_paths() {
    let f = Fixture::new();
    let child = f.root.join("child.yaml");
    f.write_recipe(&child, &f.document("child", vec![f.step("prepare")]));
    let call = |id| {
        json!({"id": id, "type": "recipe", "recipe": "child",
        "output": id, "auto_stage": false, "recovery": {"replay": "reuse"}})
    };
    f.write_recipe(
        &f.recipe,
        &f.document(
            "smart-orchestrator",
            vec![call("first-call"), call("second-call"), f.step("implement")],
        ),
    );
    let first = f.stop_at("implement");
    assert_eq!(
        f.events(),
        ["prepare", "prepare", "implement"],
        "one execution at each call site"
    );
    f.mode("implement", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(f.events(), ["prepare", "prepare", "implement", "implement"]);
    let recovery = second.assert_resumed(&first, "implement");
    let paths: std::collections::BTreeSet<_> = recovery["restored_steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["step_id"] == "prepare")
        .map(|s| s["invocation_path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths.len(),
        2,
        "child completions cannot alias across call sites: {recovery:#}"
    );
}

#[test]
fn delegated_cli_process_keeps_completed_child_steps_beneath_an_incomplete_parent() {
    let f = Fixture::new();
    let child = f.root.join("default-workflow.yaml");
    f.write_recipe(
        &child,
        &f.document(
            "default-workflow",
            vec![
                f.step("prepare"),
                f.step("architecture"),
                f.step("implement"),
            ],
        ),
    );
    let delegate = json!({
        "id": "delegate", "type": "bash", "auto_stage": false,
        "command": format!(
            "{} recipe run {} --no-detach --format json -w {} -c issue_number=1741 -c repo_path={} -c 'task_description=recover issue'",
            crate::issue_1741_fixture::quote(&crate::support::amplihack_bin::amplihack_bin()),
            crate::issue_1741_fixture::quote(&child),
            crate::issue_1741_fixture::quote(&f.repo),
            crate::issue_1741_fixture::quote(&f.repo)
        ),
        "parse_json": true, "parse_json_required": true, "output": "development",
        "recovery": {"replay": "reuse"}
    });
    f.write_recipe(
        &f.recipe,
        &f.document("smart-orchestrator", vec![f.step("classify"), delegate]),
    );
    let first = f.stop_at("implement");
    assert_eq!(
        f.events(),
        ["classify", "prepare", "architecture", "implement"]
    );
    f.mode("implement", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(
        f.events(),
        [
            "classify",
            "prepare",
            "architecture",
            "implement",
            "implement"
        ]
    );
    let recovery = second.assert_resumed(&first, "implement");
    let path = recovery["restart"]["invocation_path"].as_str().unwrap();
    assert!(
        path.contains("delegate") && path.contains("default-workflow"),
        "{path}"
    );
}

#[test]
fn detached_fresh_launch_admits_once_and_records_progress_for_a_later_relaunch() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    let tools = f.root.join("tools");
    fs::create_dir(&tools).unwrap();
    // A deterministic pane transport. Execute the real detached-child
    // synchronously, then report transport success independently of run status.
    // This exercises the private payload and executing process without tmux
    // servers, background daemons, polling or added timers.
    crate::support::exec_stub::write_executable(
        &tools.join("tmux"),
        r#"#!/bin/sh
case "$1" in
  -V) exit 0 ;;
  has-session) exit 1 ;;
  new-session)
    shift
    while [ "$1" != -- ]; do shift; done
    shift
    "$@"
    exit 0 ;;
  kill-session) exit 0 ;;
esac
exit 1
"#,
        0o700,
    );
    let before = f.run_dirs();
    let output = f
        .command(&f.recipe, &f.repo, "1741", &["--detach", "--fresh"])
        .env("PATH", format!("{}:/usr/bin:/bin", tools.display()))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
    let new: Vec<_> = f.run_dirs().difference(&before).cloned().collect();
    assert_eq!(
        new.len(),
        1,
        "the foreground launcher must not admit a second attempt"
    );
    assert_eq!(f.events(), ["classify", "design", "classify", "design"]);
    assert!(
        first.dir().join("recovery").is_dir(),
        "fresh detached launch preserves history"
    );
    let source_id = new[0].file_name().unwrap().to_str().unwrap();
    f.mode("design", "success");

    let third = f.run(&[]);
    assert!(third.output.status.success(), "{}", describe(&third.output));
    assert_eq!(third.recovery()["source_run_id"], source_id);
    assert_eq!(third.recovery()["restart"]["step_id"], "design");
    assert_eq!(
        f.events(),
        [
            "classify",
            "design",
            "classify",
            "design",
            "design",
            "implement"
        ]
    );
}

#[test]
fn fresh_is_inherited_by_nested_recipe_invocations() {
    let f = Fixture::new();
    nested(&f);
    f.stop_at("implement");
    f.mode("implement", "success");
    let before = f.events().len();

    let second = f.run(&["--fresh"]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    second.assert_fresh("fresh_requested");
    assert_eq!(
        &f.events()[before..],
        [
            "classify",
            "decompose",
            "prepare",
            "architecture",
            "implement"
        ]
    );
}
