use crate::workflow_contract::{TerminalState, validate_terminal_transition_ref};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FinalizerConfidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFinalizerOutput {
    pub schema_version: u32,
    pub terminal_state: TerminalState,
    pub terminal_success: bool,
    pub confidence: FinalizerConfidence,
    pub reason: String,
    pub required_next_action: String,
    pub hollow_success_detected: bool,
    pub evidence_used: Vec<String>,
}

impl AgentFinalizerOutput {
    pub fn from_agent_text(text: &str) -> Result<Self, AgentContractError> {
        let value: Value =
            serde_json::from_str(text).map_err(|_| AgentContractError::MissingOrInvalidJson)?;
        validate_finalizer_output(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentContractError {
    MissingOrInvalidJson,
    SuccessRequiresHighConfidence,
    FailedInvalidEvidence,
}

impl AgentContractError {
    pub fn terminal_state(&self) -> TerminalState {
        match self {
            Self::MissingOrInvalidJson => TerminalState::FailedFinalizerOutput,
            Self::SuccessRequiresHighConfidence | Self::FailedInvalidEvidence => {
                TerminalState::FailedInvalidEvidence
            }
        }
    }
}

pub fn validate_finalizer_output(value: Value) -> Result<AgentFinalizerOutput, AgentContractError> {
    let output = AgentFinalizerOutput::deserialize(&value)
        .map_err(|_| AgentContractError::MissingOrInvalidJson)?;

    if output.terminal_success && output.confidence != FinalizerConfidence::High {
        return Err(AgentContractError::SuccessRequiresHighConfidence);
    }
    if output.hollow_success_detected && output.terminal_success {
        return Err(AgentContractError::FailedInvalidEvidence);
    }

    let transition = validate_terminal_transition_ref(&value);
    if transition.terminal_state == TerminalState::FailedInvalidEvidence {
        return Err(AgentContractError::FailedInvalidEvidence);
    }
    if output.terminal_success != output.terminal_state.is_success() {
        return Err(AgentContractError::FailedInvalidEvidence);
    }

    Ok(output)
}
