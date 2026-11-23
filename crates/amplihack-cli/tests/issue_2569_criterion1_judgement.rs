//! #2569: merge-ready criterion 1 is a recorded judgement an agent makes, not
//! a table of checks that end a drive.
//!
//! The case here pins the measurement the judgement works from: it reports
//! scenarios without running them. It runs something and asserts on what it
//! did; it asserts on no prompt or recipe text.
//!
//! #3129 removed the merge-ready round and its merge-evidence recipe: the
//! goal loop's agent now judges readiness itself in each turn. The four cases
//! that ran one whole merge round (`runner.rs`) and the case that ran the
//! merge-evidence measurement step went with them.

use crate::support::criterion1_world::{World, field};

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
