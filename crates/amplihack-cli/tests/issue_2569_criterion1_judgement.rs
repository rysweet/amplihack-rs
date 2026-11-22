//! #2569: merge-ready criterion 1 is a recorded judgement an agent step
//! makes inside the loop, not a table of checks that end a drive.
//!
//! The four cases the issue names run one whole merge round through the
//! pinned recipe runner, with a fake agent and fake tools (`runner.rs`). The
//! cases here pin the two paths around them: the measurement reports
//! scenarios without running them, and a measurement that cannot be made is a
//! named blocker rather than a stop. Every case runs something and asserts on
//! what it did; none asserts on prompt or recipe text alone.

use crate::support::criterion1_world::{GadugiCall, SCENARIO_NAME, World, field, last_line, show};
use serde_json::Value;

#[path = "issue_2569_criterion1_judgement/runner.rs"]
mod runner;

/// Each scenario is validated before it is run, from its own directory
/// outside the repository holding that one file, and named by its `name:`
/// value (gadugi-agentic-test #207).
fn assert_staging_rule(calls: &[GadugiCall]) {
    assert!(
        calls.iter().any(|c| c.subcommand == "run"),
        "no scenario was run: {calls:?}"
    );
    let mut validated = None;
    for c in calls {
        assert!(!c.dir_in_repo, "staged inside the repository: {c:?}");
        assert_eq!(
            c.staged_files, 1,
            "one scenario per staging directory: {c:?}"
        );
        match c.subcommand.as_str() {
            "validate" => validated = Some(c.dir.as_str()),
            "run" => {
                assert_eq!(
                    validated.take(),
                    Some(c.dir.as_str()),
                    "run before validate: {c:?}"
                );
                assert_eq!(c.scenario, SCENARIO_NAME, "named by its name: value: {c:?}");
            }
            other => panic!("unexpected gadugi-test subcommand {other:?}: {c:?}"),
        }
    }
}

/// The measurement lists a Node repository's scenarios and runs none of
/// them. Before #2569 the tool reached a complete record only by running
/// `gadugi-test` itself; `FOUND` is now complete, so a Node repository with
/// scenarios is not held as incomplete.
#[test]
fn a_node_repository_with_scenarios_measures_found_and_runs_nothing() {
    let w = World::new();
    w.node_repo(&[(
        "tests/agentic/checkout.yaml",
        "name: shopper places an order\nsteps: []\n",
    )]);
    w.feature_pr();

    let rec = w.record();
    assert_eq!(field(&rec, "gadugi_required"), "true");
    assert_eq!(field(&rec, "gadugi_status"), "FOUND");
    assert_eq!(field(&rec, "qa_scenarios"), "tests/agentic/checkout.yaml");
    assert_eq!(field(&rec, "gadugi_scenarios_run"), "0");
    assert_eq!(field(&rec, "qa_status"), "PASS", "{rec}");
    assert!(
        w.gadugi_calls().is_empty(),
        "the measurement ran gadugi-test: {:?}",
        w.gadugi_calls()
    );
}

/// A tool that prints nothing, prints only blank lines, or fails without
/// printing reaches the judgement as the named blocker
/// `qa-measurement-unavailable`, and the step exits 0 so the round carries on.
#[test]
fn an_unusable_measurement_is_a_named_blocker_and_not_a_stop() {
    let w = World::new();
    w.node_repo(&[]);
    w.feature_pr();

    for (name, body) in [
        ("silent", "#!/bin/sh\nexit 0\n"),
        ("blank", "#!/bin/sh\nprintf '  \\n\\n'\n"),
        ("failing", "#!/bin/sh\nexit 3\n"),
    ] {
        let tools = w.private_bundle(name, body);
        let o = w.run_step(
            "autodrive-merge-evidence.yaml",
            "step-02a-qa-inputs",
            &[(
                "RECIPE_VAR_autodrive_tools_dir",
                tools.to_str().expect("utf-8 path"),
            )],
        );
        assert_eq!(o.status.code(), Some(0), "{name}: {}", show(&o));
        let line = last_line(&o);
        let v: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("{name}: the last line is one JSON object; {e}: {line}"));
        assert_eq!(
            field(&v, "qa_reason"),
            "qa-measurement-unavailable",
            "{name}: {v}"
        );
        assert_eq!(field(&v, "qa_status"), "BLOCKED", "{name}: {v}");
    }
}
