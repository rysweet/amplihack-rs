use crate::agent_contract::{AgentFinalizerOutput, validate_finalizer_output};
use crate::workflow_contract::{
    ProviderCapabilities, ProviderCapabilityState, RepositoryProvider, TerminalState,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForbiddenCall(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SimulationStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimulationAssertion {
    TerminalStateMatched,
    TerminalSuccessMatched,
    ForbiddenCallsAbsent,
    AgentContractFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationAssertions {
    pub failed: usize,
    pub items: Vec<SimulationAssertion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationResult {
    pub provider: RepositoryProvider,
    pub status: SimulationStatus,
    pub terminal_state: TerminalState,
    pub terminal_success: bool,
    pub provider_calls: Vec<String>,
    pub assertions: SimulationAssertions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationError {
    pub forbidden_call: Option<ForbiddenCall>,
    pub message: String,
}

impl std::fmt::Display for SimulationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SimulationError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeSimulationScenario {
    pub name: String,
    pub recipe: String,
    pub repo_fixture: Option<String>,
    pub provider: SimulatedProvider,
    #[serde(default)]
    pub tools: Value,
    #[serde(default)]
    pub agents: SimulatedAgents,
    #[serde(default)]
    pub observed_calls: Vec<String>,
    pub expect: SimulationExpect,
}

impl RecipeSimulationScenario {
    pub fn from_json(value: Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatedProvider {
    pub kind: RepositoryProvider,
    pub capabilities: ProviderCapabilities,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimulatedAgents {
    pub finalizer: Option<SimulatedAgent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatedAgent {
    pub output: Option<Value>,
    pub output_text: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimulationExpect {
    pub terminal_state: Option<TerminalState>,
    pub terminal_success: Option<bool>,
    #[serde(default)]
    pub forbidden_calls: Vec<String>,
}

pub struct RecipeSimulation;

impl RecipeSimulation {
    pub fn run(scenario: RecipeSimulationScenario) -> Result<SimulationResult, SimulationError> {
        let mut provider_calls = provider_calls_for(&scenario);
        provider_calls.extend(scenario.observed_calls.iter().cloned());
        for forbidden in &scenario.expect.forbidden_calls {
            if provider_calls.iter().any(|call| call == forbidden) {
                return Err(SimulationError {
                    forbidden_call: Some(ForbiddenCall(forbidden.clone())),
                    message: format!("forbidden provider call observed: {forbidden}"),
                });
            }
        }

        let mut assertions = Vec::new();
        let finalizer = validate_simulated_finalizer(&scenario.agents);
        let (terminal_state, terminal_success) = match finalizer {
            Ok(Some(output)) => (output.terminal_state, output.terminal_success),
            Ok(None) => (
                scenario
                    .expect
                    .terminal_state
                    .unwrap_or(TerminalState::ManualRequired),
                scenario.expect.terminal_success.unwrap_or(false),
            ),
            Err(agent_name) => {
                assertions.push(SimulationAssertion::AgentContractFailed(agent_name));
                (TerminalState::FailedFinalizerOutput, false)
            }
        };

        let mut failed = 0;
        if scenario.expect.terminal_state == Some(terminal_state) {
            assertions.push(SimulationAssertion::TerminalStateMatched);
        } else if scenario.expect.terminal_state.is_some() {
            failed += 1;
        }
        if scenario.expect.terminal_success == Some(terminal_success) {
            assertions.push(SimulationAssertion::TerminalSuccessMatched);
        } else if scenario.expect.terminal_success.is_some() {
            failed += 1;
        }
        assertions.push(SimulationAssertion::ForbiddenCallsAbsent);

        Ok(SimulationResult {
            provider: scenario.provider.kind,
            status: SimulationStatus::Succeeded,
            terminal_state,
            terminal_success,
            provider_calls,
            assertions: SimulationAssertions {
                failed,
                items: assertions,
            },
        })
    }
}

fn provider_calls_for(scenario: &RecipeSimulationScenario) -> Vec<String> {
    let mut calls = Vec::new();
    match scenario.provider.kind {
        RepositoryProvider::GitHub => {
            if scenario.provider.capabilities.tracking_items == ProviderCapabilityState::Automated {
                calls.push("gh.issue.create".into());
            }
            if scenario.provider.capabilities.change_requests == ProviderCapabilityState::Automated
            {
                calls.push("gh.pr.create".into());
            }
        }
        RepositoryProvider::AzureDevOps => {
            if scenario.provider.capabilities.tracking_items == ProviderCapabilityState::Automated {
                calls.push("az.boards.work-item.create".into());
            }
            if scenario.provider.capabilities.change_requests == ProviderCapabilityState::Automated
            {
                calls.push("az.repos.pr.create".into());
            }
        }
        RepositoryProvider::Manual => {}
    }
    calls
}

fn validate_simulated_finalizer(
    agents: &SimulatedAgents,
) -> Result<Option<AgentFinalizerOutput>, String> {
    let Some(finalizer) = &agents.finalizer else {
        return Ok(None);
    };
    if let Some(output) = &finalizer.output {
        return validate_finalizer_output(output.clone())
            .map(Some)
            .map_err(|_| "finalizer".into());
    }
    if let Some(text) = &finalizer.output_text {
        return AgentFinalizerOutput::from_agent_text(text)
            .map(Some)
            .map_err(|_| "finalizer".into());
    }
    Ok(None)
}
