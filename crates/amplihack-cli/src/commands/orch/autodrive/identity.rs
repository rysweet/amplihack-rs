//! PR identity prepared by the runner (build preflight), never by an agent's
//! readiness write-up. `report::already_merged` and
//! `InvocationAuthorization::validate_selection` read it.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Upper bound on any private state file or reported summary.
pub const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;

/// Runner-produced preparation, so unknown fields are still refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub run_id: String,
    pub round_id: String,
    pub platform: String,
    pub host: String,
    pub repository: String,
    pub source_repository: String,
    pub pr_number: u64,
    pub pr_url: String,
    pub head_branch: String,
    pub base_branch: String,
    pub head_sha: String,
    pub base_sha: String,
    pub merge_base_sha: String,
    pub policy_provenance: String,
}

pub fn validate_binding(b: &Binding) -> Result<()> {
    for sha in [&b.head_sha, &b.base_sha, &b.merge_base_sha] {
        ensure!(
            [40, 64].contains(&sha.len()) && sha.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid revision in evidence binding"
        );
    }
    ensure!(
        b.pr_number > 0 && b.platform == "github",
        "unsupported platform or invalid PR number"
    );
    for value in [
        &b.run_id,
        &b.round_id,
        &b.host,
        &b.repository,
        &b.source_repository,
        &b.head_branch,
        &b.base_branch,
        &b.policy_provenance,
    ] {
        ensure!(
            !value.trim().is_empty() && !value.chars().any(char::is_control),
            "invalid empty or control-bearing identity"
        );
    }
    ensure!(
        !b.host.contains(['/', '@', ':'])
            && b.repository.split('/').count() == 2
            && b.source_repository.split('/').count() == 2,
        "invalid repository or host"
    );
    ensure!(
        b.pr_url == format!("https://{}/{}/pull/{}", b.host, b.repository, b.pr_number),
        "PR URL does not match identity"
    );
    Ok(())
}
