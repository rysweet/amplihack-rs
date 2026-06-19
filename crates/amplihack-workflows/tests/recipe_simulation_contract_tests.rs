//! TDD contract tests for deterministic recipe simulation.
//!
//! Simulation uses fake providers, tools, and agents; no live GitHub, Azure
//! DevOps, network, or model calls are allowed.

use amplihack_workflows::simulation::{
    ForbiddenCall, RecipeSimulation, RecipeSimulationScenario, SimulationAssertion,
    SimulationStatus,
};
use amplihack_workflows::workflow_contract::{RepositoryProvider, TerminalState};
use serde_json::json;

#[test]
fn azdo_manual_pr_scenario_returns_manual_required_without_github_or_pr_create_calls() {
    let scenario: RecipeSimulationScenario = serde_yaml::from_str(
        r#"
name: azdo-work-item-manual-pr
recipe: default-workflow
repo_fixture: tests/fixtures/workflows/repos/azdo-repo
provider:
  kind: AzureDevOps
  capabilities:
    tracking_items: Automated
    change_requests: ManualRequired
    stale_cleanup: ManualRequired
tools:
  git:
    status: clean
  az:
    boards_work_item_create:
      id: "456"
      url: "https://dev.azure.com/acme/project/_workitems/edit/456"
agents:
  finalizer:
    output:
      schema_version: 1
      terminal_state: MANUAL_REQUIRED
      terminal_success: false
      confidence: high
      reason: "Azure Boards tracking succeeded and Azure Repos PR creation is manual."
      required_next_action: "Create an Azure Repos pull request from the pushed branch."
      hollow_success_detected: false
      evidence_used:
        - "provider=AzureDevOps"
        - "change_requests=ManualRequired"
expect:
  terminal_state: MANUAL_REQUIRED
  terminal_success: false
  forbidden_calls:
    - gh.issue.create
    - gh.pr.create
    - az.repos.pr.create
"#,
    )
    .expect("scenario fixture should parse");

    let result = RecipeSimulation::run(scenario).expect("simulation should run");

    assert_eq!(result.provider, RepositoryProvider::AzureDevOps);
    assert_eq!(result.status, SimulationStatus::Succeeded);
    assert_eq!(result.terminal_state, TerminalState::ManualRequired);
    assert!(!result.terminal_success);
    assert_eq!(result.assertions.failed, 0);
    assert!(
        result
            .provider_calls
            .contains(&"az.boards.work-item.create".into())
    );
    assert!(!result.provider_calls.contains(&"gh.pr.create".into()));
    assert!(!result.provider_calls.contains(&"az.repos.pr.create".into()));
}

#[test]
fn forbidden_provider_call_fails_simulation_even_if_terminal_state_matches() {
    let scenario = RecipeSimulationScenario::from_json(json!({
        "name": "azdo-forbidden-pr-create",
        "recipe": "default-workflow",
        "provider": {
            "kind": "AzureDevOps",
            "capabilities": {
                "tracking_items": "Automated",
                "change_requests": "ManualRequired",
                "stale_cleanup": "ManualRequired"
            }
        },
        "observed_calls": ["az.repos.pr.create"],
        "expect": {
            "terminal_state": "MANUAL_REQUIRED",
            "terminal_success": false,
            "forbidden_calls": ["az.repos.pr.create"]
        }
    }))
    .expect("scenario should parse");

    let error = RecipeSimulation::run(scenario)
        .expect_err("forbidden calls must fail simulation immediately");

    assert_eq!(
        error.forbidden_call,
        Some(ForbiddenCall("az.repos.pr.create".into()))
    );
}

#[test]
fn invalid_agent_json_fails_closed_as_finalizer_output_error() {
    let scenario = RecipeSimulationScenario::from_json(json!({
        "name": "agent-invalid-json",
        "recipe": "default-workflow",
        "provider": {
            "kind": "GitHub",
            "capabilities": {
                "tracking_items": "Automated",
                "change_requests": "Automated",
                "stale_cleanup": "Automated"
            }
        },
        "agents": {
            "finalizer": {
                "output_text": "done, looks good"
            }
        },
        "expect": {
            "terminal_state": "FAILED_FINALIZER_OUTPUT",
            "terminal_success": false
        }
    }))
    .expect("scenario should parse");

    let result = RecipeSimulation::run(scenario).expect("simulation should complete fail-closed");
    assert_eq!(result.terminal_state, TerminalState::FailedFinalizerOutput);
    assert!(!result.terminal_success);
    assert!(
        result
            .assertions
            .items
            .contains(&SimulationAssertion::AgentContractFailed(
                "finalizer".into()
            ))
    );
}
