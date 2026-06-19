use crate::workflow_contract::{ChangeRequestKind, ChangeRequestStatus, RepositoryProvider};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum CleanupMode {
    DryRun,
    Apply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupPolicy {
    pub provider: RepositoryProvider,
    pub mode: CleanupMode,
    pub workflow_label: String,
    pub superseded_by_label_prefix: String,
    pub minimum_age_hours: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaleChangeRequest {
    pub kind: ChangeRequestKind,
    pub id: String,
    pub title: String,
    pub state: ChangeRequestStatus,
    pub labels: Vec<String>,
    pub age_hours: u64,
    pub has_unmerged_meaningful_diff: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum CleanupAction {
    WouldCloseAsSuperseded,
    CloseAsSuperseded,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupPlanAction {
    pub change_request_id: String,
    pub action: CleanupAction,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupPlan {
    pub provider: RepositoryProvider,
    pub mode: CleanupMode,
    pub actions: Vec<CleanupPlanAction>,
    pub mutations_executed: usize,
}

impl CleanupPlan {
    pub fn build(
        policy: CleanupPolicy,
        candidates: Vec<StaleChangeRequest>,
    ) -> Result<Self, String> {
        if policy.workflow_label.trim().is_empty() {
            return Err("workflow_label is required".into());
        }
        if policy.superseded_by_label_prefix.trim().is_empty() {
            return Err("superseded_by_label_prefix is required".into());
        }

        let mut mutations_executed = 0;
        let actions = candidates
            .into_iter()
            .map(|candidate| {
                let eligible = candidate.state == ChangeRequestStatus::Open
                    && candidate.age_hours >= policy.minimum_age_hours
                    && !candidate.has_unmerged_meaningful_diff
                    && candidate
                        .labels
                        .iter()
                        .any(|label| label == &policy.workflow_label)
                    && candidate
                        .labels
                        .iter()
                        .any(|label| label.starts_with(&policy.superseded_by_label_prefix));

                let action = if eligible {
                    match policy.mode {
                        CleanupMode::DryRun => CleanupAction::WouldCloseAsSuperseded,
                        CleanupMode::Apply => {
                            mutations_executed += 1;
                            CleanupAction::CloseAsSuperseded
                        }
                    }
                } else {
                    CleanupAction::Skip
                };

                CleanupPlanAction {
                    change_request_id: candidate.id,
                    action,
                    reason: match action {
                        CleanupAction::WouldCloseAsSuperseded => {
                            "dry-run: workflow-owned superseded change request is eligible".into()
                        }
                        CleanupAction::CloseAsSuperseded => {
                            "workflow-owned superseded change request is eligible".into()
                        }
                        CleanupAction::Skip => {
                            "candidate is not workflow-owned, superseded, old enough, or diff-free"
                                .into()
                        }
                    },
                }
            })
            .collect();

        Ok(Self {
            provider: policy.provider,
            mode: policy.mode,
            actions,
            mutations_executed,
        })
    }
}
