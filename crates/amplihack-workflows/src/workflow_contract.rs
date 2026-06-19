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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            change_requests: ProviderCapabilityState::ManualRequired,
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
            "Use Azure Boards automation where configured; create Azure Repos PRs manually."
        }
        RepositoryProvider::Manual => "Run provider-specific change request steps manually.",
    }
}

pub fn provider_from_remote_url(remote_url: Option<&str>) -> RepositoryProvider {
    let Some(remote_url) = remote_url else {
        return RepositoryProvider::Manual;
    };
    let normalized = remote_url.to_ascii_lowercase();
    if normalized.contains("dev.azure.com") || normalized.contains("visualstudio.com") {
        RepositoryProvider::AzureDevOps
    } else if normalized.contains("github.com") {
        RepositoryProvider::GitHub
    } else {
        RepositoryProvider::Manual
    }
}

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
    let trimmed = raw.trim();
    let normalized = if trimmed.contains('_') || trimmed.contains('-') {
        trimmed.replace('-', "_").to_ascii_uppercase()
    } else {
        let mut out = String::new();
        for (index, ch) in trimmed.chars().enumerate() {
            if ch.is_ascii_uppercase() && index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_uppercase());
        }
        out
    };

    match normalized.as_str() {
        "FOLLOWUP_CREATED" => Some(TerminalState::FollowupCreated),
        "MANUAL_REQUIRED" => Some(TerminalState::ManualRequired),
        "BLOCKED_MANUAL_PROVIDER" => Some(TerminalState::BlockedManualProvider),
        "HOLLOW_SUCCESS" => Some(TerminalState::HollowSuccess),
        "FAILED_INVALID_EVIDENCE" => Some(TerminalState::FailedInvalidEvidence),
        "FAILED_FINALIZER_OUTPUT" => Some(TerminalState::FailedFinalizerOutput),
        "FAILED" => Some(TerminalState::Failed),
        _ => None,
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
    let requested_state = value
        .get("terminal_state")
        .and_then(Value::as_str)
        .and_then(parse_terminal_state)
        .unwrap_or(TerminalState::FailedInvalidEvidence);
    let requested_success = value
        .get("terminal_success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let required_next_action = value
        .get("required_next_action")
        .and_then(Value::as_str)
        .unwrap_or("Inspect workflow evidence and rerun finalization.")
        .to_string();
    let evidence_used = value
        .get("evidence_used")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if requested_success && !requested_state.is_success() {
        return TerminalValidationResult {
            terminal_state: TerminalState::FailedInvalidEvidence,
            terminal_success: false,
            terminal_reason: format!(
                "{} cannot be terminal_success=true",
                requested_state.as_str()
            ),
            required_next_action,
            evidence_used,
        };
    }

    TerminalValidationResult {
        terminal_state: requested_state,
        terminal_success: requested_success && requested_state.is_success(),
        terminal_reason: "terminal transition evidence accepted".into(),
        required_next_action,
        evidence_used,
    }
}
