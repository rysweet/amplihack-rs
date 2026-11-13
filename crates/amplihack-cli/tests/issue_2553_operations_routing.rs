//! Deterministic routing evidence; no claim about model interpretation quality.

use crate::issue_2553_fixture::{Fixture, describe, leaf, plan};
use amplihack_cli::commands::orch::{normalise_type, reclassify_task_type};
use recipe_runner_rs::context::RecipeContext;
use serde_json::{Value, json};
use std::collections::HashMap;

#[test]
fn project_management_aliases_normalise_to_operations() {
    for label in [
        "Operations",
        "Ops",
        "PM",
        "Project Management",
        "project-management",
        " pM ",
        " PROJECT MANAGEMENT ",
    ] {
        assert_eq!(normalise_type(label), "Operations", "label={label:?}");
    }
}

#[test]
fn administrative_checklists_are_not_promoted_by_complexity_or_source_mentions() {
    for task in [
        "Triage these issues.",
        "Open an issue for X and assign Y.",
        "Create an issue about src/router.rs and assign the owner.",
        "Write up the answers to these questions in chat.",
        "(1) Create a triage issue. (2) Assign Y. (3) Report status in chat.",
        "Research the question set and write the answers in an issue.",
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

#[test]
fn tracked_documentation_edits_are_development_regardless_of_verb() {
    for task in [
        "Rename term X across the docs of repos A and B.",
        "Correct the wording in tracked docs/routing.md.",
        "Write the research answers into README.md and commit them.",
        "Remove the programme name from tracked documentation files.",
    ] {
        assert_eq!(
            reclassify_task_type("Operations", task),
            "Development",
            "{task}"
        );
    }
}

#[test]
fn operations_route_is_authoritative_over_recipe_and_development_override() {
    let f = Fixture::new();
    for supplied in [
        "default-workflow",
        "quick-change",
        "investigation-workflow",
        "../../untrusted.yaml",
    ] {
        let mut item = leaf("ops", "PM", "Report status in chat.");
        item["recipe"] = json!(supplied);
        for override_value in ["", "default-workflow", "quick-change"] {
            let out = f.normalise_output(&plan(vec![item.clone()]), override_value);
            assert!(out.status.success(), "{}", describe(&out));
            let config: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(config[0]["recipe"], "operations-direct", "{config:#}");
            assert!(
                config[0].get("branch").is_none(),
                "Operations has no generated branch"
            );
        }
    }
}

#[test]
fn mislabelled_repository_edit_is_rejected_before_direct_execution() {
    let f = Fixture::new();
    let out = f.normalise_output(
        &plan(vec![leaf(
            "mislabelled",
            "Operations",
            "Update tracked docs/routing.md and commit the wording correction.",
        )]),
        "",
    );
    assert!(!out.status.success(), "{}", describe(&out));
    assert!(f.events().is_empty());
}

#[test]
fn motivating_mixed_request_keeps_only_repository_edits_on_development_route() {
    let f = Fixture::new();
    let mut cross_check = leaf(
        "cross-check",
        "Operations",
        "Open a cross-check issue for the colleague.",
    );
    cross_check["depends_on"] = json!(["edits", "research"]);
    cross_check["required_outputs"] = json!({"created_issue": "issue"});
    let input = plan(vec![
        leaf(
            "edits",
            "Development",
            "Remove the programme name from tracked source and documentation.",
        ),
        leaf(
            "research",
            "Operations",
            "Answer the research question set in chat.",
        ),
        cross_check,
    ]);
    let config = f.normalise(&input);
    assert_eq!(config.as_array().unwrap().len(), 3);
    assert_eq!(config[0]["recipe"], "default-workflow");
    assert_eq!(config[1]["recipe"], "operations-direct");
    assert_eq!(config[2]["recipe"], "operations-direct");
    assert_eq!(
        config[2]["depends_on"],
        json!([config[0]["workstream_id"], config[1]["workstream_id"]])
    );
}

#[test]
fn issue_less_decomposition_reaches_execution_with_scoped_context_and_no_stale_issue() {
    let f = Fixture::new();
    f.normalise(&plan(vec![leaf(
        "answer",
        "Operations",
        "Answer the research questions in chat.",
    )]));
    let out = f
        .command("execute")
        .arg(&f.config)
        .arg("recipe")
        .env("RECIPE_VAR_issue_number", "9999")
        .env("RECIPE_VAR_issue_url", "https://invalid/9999")
        .env("AMPLIHACK_WORKSTREAM_ISSUE", "9999")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    f.assert_direct(1);
    let call = &f.calls("execute")[0];
    assert_eq!(
        call["context"]["task_description"],
        "Answer the research questions in chat."
    );
    for key in ["issue_number", "issue_url"] {
        assert!(call["context"].get(key).is_none(), "{call:#}");
    }
    for key in ["inherited_issue", "inherited_url", "workstream_issue"] {
        assert!(call[key].is_null(), "{call:#}");
    }
    assert!(
        f.calls("gh").is_empty(),
        "missing issues do not opt in to tracking"
    );
}

#[test]
fn maximum_valid_issue_reaches_child_context_without_signed_truncation() {
    let f = Fixture::new();
    let mut item = leaf("status", "Operations", "Report project status in chat.");
    item["issue"] = json!(u64::MAX);
    f.normalise(&plan(vec![item]));
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    f.assert_direct(1);
    assert_eq!(
        f.calls("execute")[0]["context"]["issue_number"].as_u64(),
        Some(u64::MAX)
    );
    assert!(f.calls("gh").is_empty());
}

#[test]
fn dirty_initial_repository_still_executes_operations_directly() {
    let f = Fixture::new();
    std::fs::write(
        f.repo.join("README.md"),
        "# Pre-existing uncommitted wording\n",
    )
    .unwrap();
    let before = f.git(&["diff"]);
    f.normalise(&plan(vec![leaf(
        "status",
        "Operations",
        "Report project status in chat.",
    )]));
    let out = f.execute("recipe");
    assert!(out.status.success(), "{}", describe(&out));
    f.assert_direct(1);
    assert_eq!(f.git(&["diff"]), before);
}

#[test]
fn classic_operations_launches_one_agent_with_only_its_administrative_task() {
    let f = Fixture::new();
    let task = "Report the programme status in chat.";
    f.normalise(&plan(vec![leaf("status", "Operations", task)]));
    let out = f.execute("classic");
    assert!(out.status.success(), "{}", describe(&out));
    f.assert_direct(1);
    let input = f.calls("execute")[0]["raw_input"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(input.contains(task), "{input}");
    for instruction in [
        "default-workflow",
        "smart-orchestrator",
        "Create PR",
        "dev-orchestrator",
    ] {
        assert!(
            !input.contains(instruction),
            "administrative child received development instruction: {input}"
        );
    }
}

/// Use the actual pinned evaluator on the actual routing conditions. This is
/// a condition-boundary test, distinct from the process-launch fixtures above.
#[test]
fn pinned_runner_does_not_select_the_whole_request_operations_agent_for_mixed_work() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/recipes/smart-execute-routing.yaml");
    let recipe = recipe_runner_rs::parser::RecipeParser::new()
        .parse_file(&path)
        .unwrap();
    for count in [json!(3), json!("3")] {
        for serial in [json!(false), json!(true), json!("true")] {
            let mut values = HashMap::new();
            for (key, value) in [
                ("task_type", json!("Operations")),
                ("workstream_count", count.clone()),
                ("force_single_workstream", serial),
                (
                    "decomposition_json",
                    json!({"workstreams": [
                    {"classification": "Operations"}, {"classification": "Development"}, {"classification": "Operations"}]}),
                ),
            ] {
                values.insert(key.to_owned(), value);
            }
            let context = RecipeContext::new(values);
            if let Some(step) = recipe.steps.iter().find(|s| s.id == "handle-ops-agent") {
                let selected = context
                    .evaluate(step.condition.as_deref().unwrap_or("true"))
                    .unwrap();
                assert!(
                    !selected,
                    "mixed work must dispatch leaves, not execute the whole task twice"
                );
            }
        }
    }
}

#[test]
fn pinned_runner_executes_operations_leaves_once_even_in_forced_sequential_mode() {
    for serial in [false, true] {
        let f = Fixture::new();
        let leaves = plan(vec![
            leaf("research", "Operations", "Answer the question set in chat."),
            leaf("status", "Operations", "Report project status in chat."),
        ]);
        let context = json!({"task_type": "Operations", "workstream_count": 2,
            "force_single_workstream": serial, "decomposition_json": leaves.to_string(),
            "task_description": "WHOLE REQUEST: research and report status", "repo_path": f.repo,
            "session_info": "{\"status\":\"ok\"}", "round_1_result": "", "selected_workflow": "none"});
        let path = f.root.join("phase-context.json");
        std::fs::write(&path, serde_json::to_vec(&context).unwrap()).unwrap();
        let out = f
            .command("phase")
            .arg(&path)
            .env("REPO_PATH", &f.repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", describe(&out));
        assert!(
            f.calls("phase_agent")
                .iter()
                .all(|e| !e["prompt"].as_str().unwrap().contains("WHOLE REQUEST")),
            "the original request must not be executed as well as its leaves: {:?}",
            f.events()
        );
        f.assert_direct(2);
    }
}

#[test]
fn actual_operations_recipe_uses_one_agent_in_the_repository_without_development_actions() {
    let f = Fixture::new();
    let task = "Answer the research questions in chat.";
    let context = json!({"workstream_id": crate::issue_2553_fixture::FIRST,
        "classification": "Operations", "repository": crate::issue_2553_fixture::REPOSITORY,
        "task_description": task, "repo_path": f.repo, "required_outputs": {"answer": "answer"},
        "prerequisite_results": []});
    let path = f.tmp.join("operations-context.json");
    std::fs::write(&path, serde_json::to_vec(&context).unwrap()).unwrap();
    let out = f
        .command("operations-recipe")
        .arg(&path)
        .env("REPO_PATH", &f.repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let agents = f.calls("phase_agent");
    assert_eq!(
        agents.len(),
        1,
        "one administrative agent: {:?}",
        f.events()
    );
    assert_eq!(agents[0]["cwd"], f.repo.to_str().unwrap());
    assert!(agents[0]["prompt"].as_str().unwrap().contains(task));
    for kind in ["forbidden_git", "development", "pr", "gh"] {
        assert!(
            f.calls(kind).is_empty(),
            "read-only Operations recipe invoked {kind}"
        );
    }
}

#[test]
fn cli_override_applies_only_to_development_leaves_in_operations_led_mixed_work() {
    let f = Fixture::new();
    let input = plan(vec![
        leaf("status", "Operations", "Report status in chat."),
        leaf("docs", "Development", "Update tracked README.md."),
    ]);
    let path = f.root.join("mixed-input.json");
    std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let out = f
        .command("cli")
        .args(["orch", "helper", "build-workstreams-config"])
        .env("RECIPE_VAR_task_type", "Operations")
        .env("RECIPE_VAR_workflow", "quick-change")
        .stdin(std::fs::File::open(path).unwrap())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let config_file = String::from_utf8(out.stdout).unwrap();
    let config: Value =
        serde_json::from_slice(&std::fs::read(config_file.trim()).unwrap()).unwrap();
    assert_eq!(config[0]["recipe"], "operations-direct");
    assert_eq!(config[1]["recipe"], "quick-change");
    assert!(f.events().is_empty(), "normalisation must not launch work");
}
