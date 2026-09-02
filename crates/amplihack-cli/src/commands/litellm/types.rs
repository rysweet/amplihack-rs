use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RunStatus {
    Passed,
}

#[derive(Debug, Serialize)]
pub(crate) struct NegativeCaseSummaryV1 {
    pub case: &'static str,
    pub stage: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct ClientSummaryV1 {
    pub client: &'static str,
    pub version: Option<String>,
    pub binary_sha256: Option<String>,
    pub package_name: Option<&'static str>,
    pub package_integrity_sha256: Option<String>,
    pub status: &'static str,
    pub correlation_id: Option<String>,
    pub requested_alias: Option<String>,
    pub observed_provider: Option<String>,
    pub observed_model: Option<String>,
    pub gateway_identity: Option<String>,
    pub cache_status: Option<String>,
    pub backend_dispatch_id: Option<String>,
    pub result_sha256: Option<String>,
    pub failure_case: Option<String>,
    pub failure_stage: Option<String>,
    pub rustyclawd_source: Option<&'static str>,
    pub rustyclawd_revision: Option<&'static str>,
    pub rustyclawd_package: Option<&'static str>,
    pub executable_path: Option<String>,
    pub tools_disabled: Option<bool>,
    pub negative_cases: Vec<NegativeCaseSummaryV1>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RunSummaryV1 {
    pub schema: &'static str,
    pub schema_version: u8,
    pub created_at: String,
    pub run_id: String,
    pub execution_context: &'static str,
    pub repository_commit: String,
    pub repository_context_sha256: String,
    pub pr_number: u64,
    pub status: RunStatus,
    pub exit_code: u8,
    pub clients: Vec<ClientSummaryV1>,
    pub negative_cases_passed: u16,
    pub negative_cases_failed: u16,
    pub evidence_path: Option<String>,
    pub evidence_sha256: Option<String>,
    pub credentials_read: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GatewayTelemetryV1 {
    pub schema_version: u8,
    pub correlation_id: String,
    pub requested_alias: String,
    pub observed_provider: String,
    pub observed_model: String,
    pub gateway_identity: String,
    pub cache_status: String,
    pub backend_dispatch_id: String,
    pub result_sha256: String,
    pub signature_sha256: String,
}
