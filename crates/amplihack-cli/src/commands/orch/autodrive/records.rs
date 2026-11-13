use serde::{Deserialize, Serialize};

macro_rules! record {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $(pub $field: $ty),* }
    };
}
macro_rules! enumeration {
    ($name:ident { $($variant:ident),* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),* }
    };
}
enumeration!(Readiness {
    MergeReady,
    Incomplete
});
enumeration!(CriterionStatus {
    Verified,
    NotApplicable,
    Failed,
    Pending,
    Unreadable
});
enumeration!(PlatformState {
    Open,
    Merged,
    Closed,
    Unknown
});
enumeration!(RequestedAction {
    VerifyReadiness,
    Handoff,
    Merge
});
enumeration!(RequestOutcome {
    NotRequested,
    Accepted,
    Queued,
    Ambiguous,
    Refused,
    Failed
});
enumeration!(Action {
    NotRequested,
    HandedOff,
    Merged,
    AlreadyMerged,
    WorkflowViolation,
    Pending,
    Unconfirmed,
    Refused,
    Failed
});
enumeration!(StopReason {
    Completed,
    Blockers,
    NoProgress,
    BudgetExhausted,
    UsageLimit,
    Orphaned,
    DependencyFailure,
    TerminalRefusal
});
enumeration!(AttemptOutcome {
    Failed,
    Refused,
    Ambiguous,
    Succeeded
});
record!(Binding {
    run_id: String,
    round_id: String,
    platform: String,
    host: String,
    repository: String,
    source_repository: String,
    pr_number: u64,
    pr_url: String,
    head_branch: String,
    base_branch: String,
    head_sha: String,
    base_sha: String,
    merge_base_sha: String,
    policy_provenance: String,
});
record!(EvidenceReference {
    file: String,
    sha256: String
});
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub source: String,
    pub observation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<EvidenceReference>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    pub interface: String,
    pub complete: bool,
    pub evidence: String,
    #[serde(default)]
    pub observed_before_retry: bool,
}
record!(Attempt { operation: String, outcome: AttemptOutcome, cause: String, recovery: Option<Recovery> });
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub id: String,
    pub status: CriterionStatus,
    pub binding: Binding,
    pub evidence: Vec<Evidence>,
    pub blockers: Vec<String>,
    #[serde(default)]
    pub not_applicable_reason: Option<String>,
}
record!(ReviewRange {
    reviewed_head_sha: String,
    verified_head_sha: String,
    base_sha: String,
    observation: String,
});
record!(CandidateRecord {
    schema_version: u32, binding: Binding, reviewed_head_sha: String,
    verified_head_sha: String, readiness: Readiness, blockers: Vec<String>,
    criteria: Vec<Criterion>, attempts: Vec<Attempt>, review_range: Option<ReviewRange>,
});
record!(FinalizationObservation {
    binding: Binding, requested_action: RequestedAction, expected_head_sha: String,
    refreshed_head_sha: String, request_expected_head_sha: Option<String>,
    mutable_facts_refreshed: bool, merge_requests: u32, request_outcome: RequestOutcome,
    platform_state_at_entry: PlatformState, platform_state_before_action: PlatformState,
    platform_state: PlatformState, confirmation_evidence: String, failure_cause: Option<String>,
    attempts: Vec<Attempt>,
});
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RoundVerdict {
    Ready,
    NotReady,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvaluatorVerdict {
    Done,
    Continue,
    Stuck,
}
record!(LoopCompletion {
    accepted_round_verdict: RoundVerdict,
    evaluator_verdict: EvaluatorVerdict,
    stop_reason: StopReason
});
record!(Outcome {
    schema_version: u32, binding: Binding, readiness: Readiness, action: Action,
    platform_state: PlatformState, stop_reason: StopReason, blockers: Vec<String>,
    attempts: Vec<Attempt>, failure_cause: Option<String>,
});
