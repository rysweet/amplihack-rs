//! Issue #2553, fault 1 — an Operations (project-management) request must not
//! be routed down the Development path.
//!
//! Every assertion here is on a returned value or on the contents of a file
//! the library wrote. Nothing scans a prompt or a recipe for wording, nothing
//! spawns a process, nothing sleeps, and nothing reads the clock. The route an
//! Operations workstream takes is read out of the workstreams-config the
//! orchestrator helper actually produces.

use amplihack_cli::commands::orch::{
    build_workstreams_config_to_tempfile, build_workstreams_config_with_override, normalise_type,
    reclassify_task_type,
};
use amplihack_workflows::classifier::{WorkflowClassifier, WorkflowType};
use serde_json::{Value, json};

#[test]
fn requested_actions_bind_their_own_objects_and_destinations() {
    let classifier = WorkflowClassifier::default();
    for task in [
        "Write the research findings into docs/report.md and post an issue comment.",
        "Post an issue comment and write the research findings into docs/report.md.",
        "Create an issue to add tests for src/router.rs and update src/lib.rs.",
        "Add tests for src/router.rs and create an issue about the findings.",
        "Add tests for src/router.rs.",
        "Write the findings into \"docs/report.md\" and post an issue comment.",
        "Post an issue comment and fix \"src/router.rs\".",
        "Report the findings into docs/report.md and create an issue.",
        "Fix issue_router.rs and post an issue comment.",
    ] {
        assert_eq!(
            reclassify_task_type("Operations", task),
            "Development",
            "{task}"
        );
        assert_eq!(
            classifier.classify(task).workflow,
            WorkflowType::Default,
            "{task}"
        );
    }
    for task in [
        "Create an issue to add tests for src/router.rs and assign it to Alex.",
        "Assign it to Alex and create an issue to add tests for src/router.rs.",
        "Create an issue titled \"Add tests for src/router.rs and update docs/report.md\".",
        "Post findings about src/router.rs in an issue comment.",
        "Write the findings about src/router.rs in chat.",
    ] {
        assert_eq!(
            reclassify_task_type("Operations", task),
            "Operations",
            "{task}"
        );
        assert_eq!(
            classifier.classify(task).workflow,
            WorkflowType::Ops,
            "{task}"
        );
    }
}

/// Build the workstreams-config for `decomp` and return it parsed. The
/// tempfile is removed before the assertions run, so a failing assertion
/// cannot leave a file behind.
fn config_for(decomp: &Value, override_value: &str) -> Value {
    let path = build_workstreams_config_with_override(&decomp.to_string(), override_value)
        .expect("the helper writes a workstreams-config");
    let body = std::fs::read_to_string(&path).expect("the config is readable");
    std::fs::remove_file(&path).ok();
    serde_json::from_str(&body).expect("the config is JSON")
}

fn recipes(config: &Value) -> Vec<&str> {
    config
        .as_array()
        .expect("the config is an array")
        .iter()
        .map(|e| e["recipe"].as_str().expect("every entry names a recipe"))
        .collect()
}

// ---- the label the decomposition carries ----------------------------------

/// The deputy's wording for this class is "Ops/PM". `Operations` and `Ops`
/// already normalise; the project-management spellings are what #2553 adds.
/// `pm` matches as a whole label, not as a substring, so that `compile` and
/// `pmap` stay Development.
#[test]
fn project_management_aliases_normalise_to_operations() {
    for label in [
        "Operations",
        "Ops",
        "PM",
        "pm",
        " pM ",
        "Project Management",
        "project-management",
        " PROJECT MANAGEMENT ",
    ] {
        assert_eq!(normalise_type(label), "Operations", "label={label:?}");
    }
}

#[test]
fn project_management_aliases_do_not_match_inside_other_words() {
    for label in ["compile", "pmap the process", "impediment"] {
        assert_eq!(normalise_type(label), "Development", "label={label:?}");
    }
}

// ---- promotion to Development only on a real repository edit --------------

/// Fix #269 promotes an Operations label to Development when the task carries
/// strong development signals. An administrative checklist is not such a
/// signal: neither its length, nor an enumerated structure, nor a source path
/// it merely names may promote it.
#[test]
fn administrative_checklists_are_not_promoted_by_complexity_or_source_mentions() {
    for task in [
        "Triage these issues.",
        "Open an issue for X and assign Y.",
        "Create an issue about src/router.rs and assign the owner.",
        "Write up the answers to these questions in chat.",
        "(1) Create a triage issue. (2) Assign Y. (3) Report status in chat.",
        "Research these issues and write up the answers in an issue.",
        "Report unit test coverage in the status issue.",
        "Coordinate review of the existing pull request.",
    ] {
        assert_eq!(
            reclassify_task_type("Operations", task),
            "Operations",
            "{task}"
        );
    }
}

/// The converse, which keeps the Operations route from swallowing real work:
/// a request to change tracked files is Development whatever verb it uses.
#[test]
fn tracked_file_edits_are_development_regardless_of_verb() {
    for task in [
        "Rename term X across the tracked docs of repos A and B.",
        "Correct the wording in tracked docs/routing.md.",
        "Write the research answers into README.md and commit them.",
        "Remove the programme name from tracked documentation files.",
        "Extend crates/amplihack-cli/src/commands/orch.rs with the new arm.",
        "Open a PR that drops the stale entries.",
    ] {
        assert_eq!(
            reclassify_task_type("Operations", task),
            "Development",
            "{task}"
        );
    }
}

// ---- the route an Operations leaf is given --------------------------------

/// Fault 1 in one line: `non_development_recipe` had no Operations arm, so an
/// Operations leaf fell through to `default-workflow`, which clones a
/// worktree, branches and opens a PR for administrative work.
#[test]
fn an_operations_leaf_routes_to_operations_direct() {
    let config = config_for(
        &json!({
            "task_type": "Operations",
            "workstreams": [
                {"name": "triage", "classification": "Operations",
                 "description": "Triage the open issues and report in chat."}
            ]
        }),
        "",
    );
    assert_eq!(recipes(&config), vec!["operations-direct"]);
}

/// The leaf's own recipe field must not be able to put administrative work
/// back on the Development route, whatever the model wrote there and whatever
/// `-c workflow=` the caller passed.
#[test]
fn the_operations_route_is_authoritative_over_recipe_and_override() {
    for supplied in [
        Value::Null,
        json!(""),
        json!("   "),
        json!("default-workflow"),
        json!("quick-change"),
    ] {
        let mut leaf = json!({
            "name": "status",
            "classification": "Operations",
            "description": "Report the lane status in chat."
        });
        if !supplied.is_null() {
            leaf["recipe"] = supplied.clone();
        }
        for override_value in ["", "default-workflow", "quick-change"] {
            let config = config_for(
                &json!({"task_type": "Operations", "workstreams": [leaf]}),
                override_value,
            );
            assert_eq!(
                recipes(&config),
                vec!["operations-direct"],
                "recipe={supplied} override={override_value:?}"
            );
        }
    }
}

/// Operations routing is authoritative even over a custom recipe. Resolving
/// this after development preparation would already be too late.
#[test]
fn a_custom_operations_recipe_cannot_bypass_operations_direct() {
    let config = config_for(
        &json!({
            "task_type": "Operations",
            "workstreams": [
                {"name": "ops", "classification": "Operations", "description": "Run the sentinel", "recipe": "ops-sentinel-workflow"}
            ]
        }),
        "",
    );
    assert_eq!(recipes(&config), vec!["operations-direct"]);
}

/// Same defect, same fix, one classification over: an Investigation or Q&A
/// leaf that names no recipe was also being handed `default-workflow`.
#[test]
fn a_non_development_leaf_without_a_recipe_gets_its_own_workflow() {
    let config = config_for(
        &json!({
            "task_type": "Investigation",
            "workstreams": [
                {"name": "research", "classification": "Investigation", "description": "Read the code"},
                {"name": "answer", "classification": "Q&A", "description": "Answer the question"},
                {"name": "ops", "classification": "Operations", "description": "Triage the issues"}
            ]
        }),
        "",
    );
    assert_eq!(
        recipes(&config),
        vec!["investigation-workflow", "qa-workflow", "operations-direct"]
    );
}

// ---- mixed requests -------------------------------------------------------

/// The 13 November request: three administrative workstreams and one that
/// genuinely edits tracked files. Only the last may reach `default-workflow`.
#[test]
fn the_motivating_mixed_request_keeps_only_repository_edits_on_the_development_route() {
    let config = config_for(
        &json!({
            "task_type": "Operations",
            "workstreams": [
                {"name": "triage", "classification": "Operations",
                 "description": "Triage the open issues."},
                {"name": "research", "classification": "Operations",
                 "description": "Research these issues and write up the answers in chat."},
                {"name": "edits", "classification": "Development",
                 "description": "Remove the programme name from tracked source and documentation."},
                {"name": "cross-check", "classification": "Operations",
                 "description": "Open a cross-check issue for the colleague."}
            ]
        }),
        "",
    );
    assert_eq!(
        recipes(&config),
        vec![
            "operations-direct",
            "operations-direct",
            "default-workflow",
            "operations-direct",
        ]
    );
}

/// `-c workflow=quick-change` is a Development-route override. It must reach
/// the Development leaf and no other.
#[test]
fn the_cli_override_applies_only_to_development_leaves_in_operations_led_work() {
    let config = config_for(
        &json!({
            "task_type": "Operations",
            "workstreams": [
                {"name": "triage", "classification": "Operations", "description": "Triage the issues"},
                {"name": "typo", "classification": "Development", "description": "Fix the typo in src/lib.rs"}
            ]
        }),
        "quick-change",
    );
    assert_eq!(recipes(&config), vec!["operations-direct", "quick-change"]);
}

// ---- the hook-level classification ----------------------------------------

/// `WorkflowType::Ops` has no recipe, which is what makes a plain
/// administrative prompt run directly: no recipe launch, so no worktree and
/// no PR. Fault 1 is the decomposition arm above; this is the layer that
/// never starts a workflow at all.
#[test]
fn an_operations_classification_starts_no_workflow() {
    assert_eq!(WorkflowType::Ops.recipe_name(), None);

    let classifier = WorkflowClassifier::default();
    for request in [
        "Triage these issues",
        "Open an issue for X and assign Y",
        "Research these issues and write up the answers",
        "Write up the answers to these questions in chat",
        "Coordinate review of the open pull requests",
    ] {
        let result = classifier.classify(request);
        assert_eq!(result.workflow, WorkflowType::Ops, "{request}");
        assert_eq!(result.workflow.recipe_name(), None, "{request}");
    }
}

/// The near neighbours of those requests still reach Development, so the new
/// route cannot steal work that edits tracked files.
#[test]
fn near_neighbour_edit_requests_still_reach_the_development_workflow() {
    let classifier = WorkflowClassifier::default();
    for request in [
        "Remove the stale entries from docs/",
        "Fix the typo in README",
        "Rename the config key across crates/amplihack-workflows/",
        "Add tests for the issue parser",
        "Open a PR for the routing fix",
    ] {
        assert_eq!(
            classifier.classify(request).workflow,
            WorkflowType::Default,
            "{request}"
        );
    }
}

/// A plain Operations decomposition writes one entry per workstream and never
/// a `branch` that points at the Development route's naming scheme being
/// reused as evidence of a worktree. The entry count is the contract here.
#[test]
fn every_decomposition_leaf_produces_exactly_one_config_entry() {
    let path = build_workstreams_config_to_tempfile(
        &json!({
            "task_type": "Operations",
            "workstreams": [
                {"name": "a", "classification": "Operations", "description": "Triage"},
                {"name": "b", "classification": "Operations", "description": "Report status"},
                {"name": "c", "classification": "Development", "description": "Edit src/lib.rs"}
            ]
        })
        .to_string(),
    )
    .expect("the helper writes a workstreams-config");
    let body = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let config: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(config.as_array().unwrap().len(), 3);
}
