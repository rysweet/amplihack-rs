use serde::{Deserialize, Deserializer, Serialize};

macro_rules! record {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $($(#[$meta])* pub $field: $ty),* }
    };
}

/// #2556: an agent writer may emit `null` (or omit the key) for a string it
/// could not observe. The listed string fields read that as `""`, the
/// internal unknown marker; `validation::unknown_fields` names each one as a
/// blocker, and `validate_binding` still rejects empty identity, so an
/// unknown value is never accepted. Serialisation is unchanged.
pub(super) fn null_as_unknown<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Option::<String>::deserialize(d).map(Option::unwrap_or_default)
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
    #[serde(default, deserialize_with = "null_as_unknown")]
    run_id: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    round_id: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    platform: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    host: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    repository: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    source_repository: String,
    pr_number: u64,
    #[serde(default, deserialize_with = "null_as_unknown")]
    pr_url: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    head_branch: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    base_branch: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    head_sha: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    base_sha: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    merge_base_sha: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    policy_provenance: String,
});
record!(EvidenceReference {
    file: String,
    sha256: String
});
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    #[serde(default, deserialize_with = "null_as_unknown")]
    pub source: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    pub observation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<EvidenceReference>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    #[serde(default, deserialize_with = "null_as_unknown")]
    pub interface: String,
    pub complete: bool,
    #[serde(default, deserialize_with = "null_as_unknown")]
    pub evidence: String,
    #[serde(default)]
    pub observed_before_retry: bool,
}
record!(Attempt {
    #[serde(default, deserialize_with = "null_as_unknown")] operation: String,
    outcome: AttemptOutcome,
    #[serde(default, deserialize_with = "null_as_unknown")] cause: String,
    recovery: Option<Recovery>,
});
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
    #[serde(default, deserialize_with = "null_as_unknown")]
    reviewed_head_sha: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    verified_head_sha: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    base_sha: String,
    #[serde(default, deserialize_with = "null_as_unknown")]
    observation: String,
});
record!(CandidateRecord {
    schema_version: u32, binding: Binding,
    #[serde(default, deserialize_with = "null_as_unknown")] reviewed_head_sha: String,
    #[serde(default, deserialize_with = "null_as_unknown")] verified_head_sha: String, readiness: Readiness, blockers: Vec<String>,
    criteria: Vec<Criterion>, attempts: Vec<Attempt>, review_range: Option<ReviewRange>,
});
record!(FinalizationObservation {
    binding: Binding, requested_action: RequestedAction,
    #[serde(default, deserialize_with = "null_as_unknown")] expected_head_sha: String,
    #[serde(default, deserialize_with = "null_as_unknown")] refreshed_head_sha: String, request_expected_head_sha: Option<String>,
    mutable_facts_refreshed: bool, merge_requests: u32, request_outcome: RequestOutcome,
    platform_state_at_entry: PlatformState, platform_state_before_action: PlatformState,
    platform_state: PlatformState,
    #[serde(default, deserialize_with = "null_as_unknown")] confirmation_evidence: String, failure_cause: Option<String>,
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
