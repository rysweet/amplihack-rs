use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum RepositoryProvider {
    GitHub,
    AzureDevOps,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ProviderCapabilityState {
    Automated,
    ManualRequired,
    BlockedManualProvider,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub tracking_items: ProviderCapabilityState,
    pub change_requests: ProviderCapabilityState,
    pub stale_cleanup: ProviderCapabilityState,
}

pub fn provider_capabilities(provider: RepositoryProvider) -> ProviderCapabilities {
    match provider {
        RepositoryProvider::GitHub => ProviderCapabilities {
            tracking_items: ProviderCapabilityState::Automated,
            change_requests: ProviderCapabilityState::Automated,
            stale_cleanup: ProviderCapabilityState::Automated,
        },
        RepositoryProvider::AzureDevOps => ProviderCapabilities {
            tracking_items: ProviderCapabilityState::Automated,
            change_requests: ProviderCapabilityState::Automated,
            stale_cleanup: ProviderCapabilityState::ManualRequired,
        },
        RepositoryProvider::Manual => ProviderCapabilities {
            tracking_items: ProviderCapabilityState::ManualRequired,
            change_requests: ProviderCapabilityState::ManualRequired,
            stale_cleanup: ProviderCapabilityState::ManualRequired,
        },
    }
}

pub fn provider_default_next_action(provider: RepositoryProvider) -> &'static str {
    match provider {
        RepositoryProvider::GitHub => "No further provider setup is required.",
        RepositoryProvider::AzureDevOps => {
            "Use Azure Boards and Azure Repos automation where configured."
        }
        RepositoryProvider::Manual => "Run provider-specific change request steps manually.",
    }
}

pub use crate::remote_repository::{
    provider_from_remote_url, redact_remote_url, repository_identity_from_remote_url,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryIdentity {
    pub remote_url: Option<String>,
    pub owner: String,
    pub name: String,
    pub default_base: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ProviderOperationStatus {
    Succeeded,
    ManualRequired,
    BlockedManualProvider,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderContext {
    pub schema_version: u32,
    pub provider: RepositoryProvider,
    pub repository: RepositoryIdentity,
    pub capabilities: ProviderCapabilities,
    pub status: ProviderOperationStatus,
    pub next_action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalState {
    FollowupCreated,
    ManualRequired,
    BlockedManualProvider,
    HollowSuccess,
    FailedInvalidEvidence,
    FailedFinalizerOutput,
    Failed,
}

impl TerminalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FollowupCreated => "FOLLOWUP_CREATED",
            Self::ManualRequired => "MANUAL_REQUIRED",
            Self::BlockedManualProvider => "BLOCKED_MANUAL_PROVIDER",
            Self::HollowSuccess => "HOLLOW_SUCCESS",
            Self::FailedInvalidEvidence => "FAILED_INVALID_EVIDENCE",
            Self::FailedFinalizerOutput => "FAILED_FINALIZER_OUTPUT",
            Self::Failed => "FAILED",
        }
    }

    pub fn is_success(self) -> bool {
        matches!(self, Self::FollowupCreated)
    }
}

impl Serialize for TerminalState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TerminalState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        parse_terminal_state(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown terminal state '{raw}'")))
    }
}

pub fn parse_terminal_state(raw: &str) -> Option<TerminalState> {
    if matches_identifier(raw, "followupcreated") {
        Some(TerminalState::FollowupCreated)
    } else if matches_identifier(raw, "manualrequired") {
        Some(TerminalState::ManualRequired)
    } else if matches_identifier(raw, "blockedmanualprovider") {
        Some(TerminalState::BlockedManualProvider)
    } else if matches_identifier(raw, "hollowsuccess") {
        Some(TerminalState::HollowSuccess)
    } else if matches_identifier(raw, "failedinvalidevidence") {
        Some(TerminalState::FailedInvalidEvidence)
    } else if matches_identifier(raw, "failedfinalizeroutput") {
        Some(TerminalState::FailedFinalizerOutput)
    } else if matches_identifier(raw, "failed") {
        Some(TerminalState::Failed)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HelperOperation {
    DetectProvider,
    PublishChangeRequest,
    SimulateRecipe,
    CleanupStale,
    TerminalState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelperEnvelope {
    pub schema_version: u32,
    pub provider: RepositoryProvider,
    pub operation: HelperOperation,
    pub status: ProviderOperationStatus,
    pub next_action: String,
    pub warnings: Vec<String>,
    pub data: Value,
}

impl HelperEnvelope {
    pub fn succeeded(
        provider: RepositoryProvider,
        operation: HelperOperation,
        next_action: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            schema_version: 1,
            provider,
            operation,
            status: ProviderOperationStatus::Succeeded,
            next_action: next_action.into(),
            warnings: Vec::new(),
            data,
        }
    }

    pub fn manual_required(
        provider: RepositoryProvider,
        operation: HelperOperation,
        next_action: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            schema_version: 1,
            provider,
            operation,
            status: ProviderOperationStatus::ManualRequired,
            next_action: next_action.into(),
            warnings: Vec::new(),
            data,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ChangeRequestKind {
    PullRequest,
    MergeRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ChangeRequestStatus {
    Open,
    Closed,
    Merged,
    Draft,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRequest {
    pub kind: ChangeRequestKind,
    pub id: String,
    pub url: String,
    pub state: ChangeRequestStatus,
    pub source_branch: String,
    pub base_branch: String,
    pub head_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualAction {
    pub action: String,
    pub instructions: String,
    pub required_inputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalValidationResult {
    pub terminal_state: TerminalState,
    pub terminal_success: bool,
    pub terminal_reason: String,
    pub required_next_action: String,
    pub evidence_used: Vec<String>,
}

pub fn validate_terminal_transition(value: Value) -> TerminalValidationResult {
    validate_terminal_transition_ref(&value)
}

pub fn validate_terminal_transition_ref(value: &Value) -> TerminalValidationResult {
    let requested_state = value
        .get("terminal_state")
        .and_then(Value::as_str)
        .and_then(parse_terminal_state)
        .unwrap_or(TerminalState::FailedInvalidEvidence);
    let requested_success = value
        .get("terminal_success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let terminal_reason = value
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let required_next_action = value
        .get("required_next_action")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let evidence_used = value
        .get("evidence_used")
        .and_then(Value::as_array)
        .map(|items| {
            let mut evidence = Vec::with_capacity(items.len());
            for item in items {
                if let Some(item) = item.as_str().map(str::trim)
                    && !item.is_empty()
                {
                    evidence.push(item.to_owned());
                }
            }
            evidence
        })
        .unwrap_or_default();
    let invalid_next_action = if required_next_action.is_empty() {
        "Inspect workflow evidence and rerun finalization.".to_string()
    } else {
        required_next_action.clone()
    };

    if terminal_reason.is_empty() {
        return invalid_terminal_transition(
            "terminal transition requires non-empty reason",
            invalid_next_action,
            evidence_used,
        );
    }
    if required_next_action.is_empty() {
        return invalid_terminal_transition(
            "terminal transition requires non-empty required_next_action",
            invalid_next_action,
            evidence_used,
        );
    }
    if evidence_used.is_empty() {
        return invalid_terminal_transition(
            "terminal transition requires non-empty evidence_used",
            required_next_action,
            evidence_used,
        );
    }

    if requested_success && !requested_state.is_success() {
        return invalid_terminal_transition(
            format!(
                "{} cannot be terminal_success=true",
                requested_state.as_str()
            ),
            required_next_action,
            evidence_used,
        );
    }
    if !requested_success && requested_state.is_success() {
        return invalid_terminal_transition(
            format!(
                "{} requires terminal_success=true",
                requested_state.as_str()
            ),
            required_next_action,
            evidence_used,
        );
    }
    if requested_success
        && value
            .get("hollow_success_detected")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return invalid_terminal_transition(
            "hollow_success_detected cannot be terminal_success=true",
            required_next_action,
            evidence_used,
        );
    }
    if requested_success && provider_from_value(value).is_none() {
        return invalid_terminal_transition(
            "terminal_success=true requires explicit provider evidence",
            required_next_action,
            evidence_used,
        );
    }
    if requested_success && provider_from_value(value) == Some(RepositoryProvider::Manual) {
        return invalid_terminal_transition(
            "Manual provider cannot be terminal_success=true",
            required_next_action,
            evidence_used,
        );
    }
    if requested_success {
        match change_request_capability_from_value(value) {
            Some(ProviderCapabilityState::Automated) => {}
            Some(change_requests) => {
                return invalid_terminal_transition(
                    format!("change_requests={change_requests:?} cannot be terminal_success=true"),
                    required_next_action,
                    evidence_used,
                );
            }
            None => {
                return invalid_terminal_transition(
                    "terminal_success=true requires explicit change_requests capability evidence",
                    required_next_action,
                    evidence_used,
                );
            }
        }
    }

    TerminalValidationResult {
        terminal_state: requested_state,
        terminal_success: requested_success && requested_state.is_success(),
        terminal_reason,
        required_next_action,
        evidence_used,
    }
}

fn invalid_terminal_transition(
    terminal_reason: impl Into<String>,
    required_next_action: String,
    evidence_used: Vec<String>,
) -> TerminalValidationResult {
    TerminalValidationResult {
        terminal_state: TerminalState::FailedInvalidEvidence,
        terminal_success: false,
        terminal_reason: terminal_reason.into(),
        required_next_action,
        evidence_used,
    }
}

fn provider_from_value(value: &Value) -> Option<RepositoryProvider> {
    value
        .get("provider")
        .and_then(Value::as_str)
        .and_then(parse_repository_provider)
}

fn change_request_capability_from_value(value: &Value) -> Option<ProviderCapabilityState> {
    value
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("change_requests"))
        .and_then(Value::as_str)
        .and_then(parse_provider_capability_state)
}

fn parse_repository_provider(raw: &str) -> Option<RepositoryProvider> {
    if matches_identifier(raw, "github") {
        Some(RepositoryProvider::GitHub)
    } else if matches_identifier(raw, "azuredevops") {
        Some(RepositoryProvider::AzureDevOps)
    } else if matches_identifier(raw, "manual") {
        Some(RepositoryProvider::Manual)
    } else {
        None
    }
}

fn parse_provider_capability_state(raw: &str) -> Option<ProviderCapabilityState> {
    if matches_identifier(raw, "automated") {
        Some(ProviderCapabilityState::Automated)
    } else if matches_identifier(raw, "manualrequired") {
        Some(ProviderCapabilityState::ManualRequired)
    } else if matches_identifier(raw, "blockedmanualprovider") {
        Some(ProviderCapabilityState::BlockedManualProvider)
    } else if matches_identifier(raw, "unsupported") {
        Some(ProviderCapabilityState::Unsupported)
    } else {
        None
    }
}

fn matches_identifier(raw: &str, expected: &str) -> bool {
    let mut chars = raw
        .trim()
        .chars()
        .filter(|ch| !matches!(ch, '_' | '-' | ' '));
    for expected in expected.chars() {
        match chars.next() {
            Some(ch) if ch.eq_ignore_ascii_case(&expected) => {}
            _ => return false,
        }
    }
    chars.next().is_none()
}
