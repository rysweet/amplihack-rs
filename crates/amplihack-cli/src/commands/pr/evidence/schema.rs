//! Version 1 file protocol. Requests are proposals, never authorization.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef { pub artifact_id: String, pub sha256: String }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target { pub host: String, pub repository_id: u64, pub owner: String, pub repo: String, pub pr_number: u32, pub head_repository_id: u64 }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Revision { pub head_sha: String, pub base_ref: String, pub base_sha: String, pub worktree_sha: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request { pub schema_version: u32, pub request_id: String, pub operation: String, pub run_id: String, pub round_id: String, pub target: Target, pub revision: Revision, pub predecessor_ref: Option<ArtifactRef>, pub resume_ref: Option<ArtifactRef>, pub payload: Value }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collect { pub operations: Vec<Operation> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation { pub operation_id: String, pub api: Api, pub resource: Resource, pub parameters: Parameters, pub pagination: Pagination }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum Api { Rest, Graphql }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum Resource { PrMetadata, CheckRuns, CommitStatuses, CheckSuites, SuiteCheckRuns, BranchProtection, BranchMetadata, ActiveRules, Reviews, ReviewComments, ReviewThreads, LocalEvidence }
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Parameters { pub sha: Option<String>, pub suite_id: Option<u64>, pub base_ref: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pagination { pub scope: Scope, pub per_page: u32, pub max_pages: u32 }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum Scope { Single, All }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Validate { pub assessment_ref: ArtifactRef, pub receipt_refs: Vec<ArtifactRef>, pub supersessions: Vec<Supersession> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Supersession { pub prior_receipt_ref: ArtifactRef, pub replacement_receipt_ref: ArtifactRef, pub fact_ids: Vec<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assessment { pub merge_ready_verdict: Readiness, pub criteria: Vec<Criterion>, pub next_action: Action }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum Readiness { MergeReady, NotMergeReady }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion { pub criterion_id: String, pub disposition: Disposition, pub explanation: String, pub fact_refs: Vec<FactRef>, pub applicability_fact_refs: Vec<FactRef> }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum Disposition { Satisfied, Blocked, Unknown, NotApplicable }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactRef { pub receipt_ref: ArtifactRef, pub fact_id: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action { pub kind: ActionKind, pub reason: String, pub request_ref: Option<ArtifactRef> }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
pub enum ActionKind { Collect, Wait, Remediate, Advance, Stop }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wait { pub wake: Wake, pub reason: String, pub recollect_operation_ids: Vec<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag="kind", deny_unknown_fields)]
pub enum Wake { #[serde(rename="DEADLINE")] Deadline { at: String }, #[serde(rename="EVENT")] Event { event_ref: ArtifactRef } }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Execute { pub bundle_ref: ArtifactRef, pub evaluator_ref: ArtifactRef, pub revalidation_plan: Plan }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan { pub operations: Vec<Operation>, pub coverage: Vec<Coverage>, pub reuse_refs: Vec<ArtifactRef> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage { pub criterion_id: String, pub operation_ids: Vec<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inspect { pub attempt_ref: ArtifactRef }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error { pub code: String, pub scope: String, pub message: String, pub operation_id: Option<String>, pub artifact_ref: Option<ArtifactRef> }
impl Error { pub fn new(code: &str, scope: &str, message: &str) -> Self { Self {code:code.into(),scope:scope.into(),message:message.chars().filter(|c| !c.is_control()).take(400).collect(),operation_id:None,artifact_ref:None} } }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record { pub schema_version: u32, pub kind: String, pub run_id: String, pub round_id: String, pub target: Target, pub revision: Revision, pub predecessor_ref: Option<ArtifactRef>, pub payload: Value }
impl Request { pub fn record(&self, kind: &str, payload: Value) -> Record { Record { schema_version:1,kind:kind.into(),run_id:self.run_id.clone(),round_id:self.round_id.clone(),target:self.target.clone(),revision:self.revision.clone(),predecessor_ref:self.predecessor_ref.clone(),payload } } }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt { pub operation: Operation, pub collected_at: String, pub pages: Vec<Page>, pub complete: bool, pub errors: Vec<Error> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page { pub response_ref: Option<ArtifactRef>, pub http_status: Option<u16>, pub response_headers: std::collections::BTreeMap<String,String>, pub process_exit_code: Option<i32> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle { pub assessment_ref: ArtifactRef, pub receipt_refs: Vec<ArtifactRef>, pub supersessions: Vec<Supersession>, pub assessment: Assessment }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evaluator { pub loop_verdict: String, pub assessment_ref: ArtifactRef, pub explanation: String, pub next_action: Action, pub recommended_intervention: Option<String> }

pub fn identifier(s:&str) -> bool { !s.is_empty() && s.len()<=128 && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)) }
pub fn git_sha(s:&str) -> bool { matches!(s.len(),40|64) && hex(s) }
pub fn hex(s:&str) -> bool { s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) }
pub fn reference(r:&ArtifactRef)->bool { identifier(&r.artifact_id) && r.sha256.len()==64 && hex(&r.sha256) }
