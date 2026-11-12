//! Fresh completion receipts connect the unchanged convergence decision to finalization.
use super::{authorization::InvocationAuthorization, records::*, state::EvidenceStore};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    run_id: String,
    round_id: String,
    candidate: String,
    candidate_sha256: String,
}
fn name(auth: &InvocationAuthorization) -> String {
    format!("loop-{}-merge-ready.json", auth.run_id())
}
fn basename(path: &Path, directory: &Path) -> Result<String> {
    ensure!(
        path.parent()
            .context("accepted evidence has no parent")?
            .canonicalize()?
            == directory.canonicalize()?,
        "accepted evidence is outside private state"
    );
    Ok(path
        .file_name()
        .and_then(|s| s.to_str())
        .context("invalid accepted evidence filename")?
        .to_owned())
}
/// Called by the loop driver only in its accepted clean + evaluator DONE branch.
pub fn record(auth: &InvocationAuthorization, directory: &Path, round: &str) -> Result<()> {
    ensure!(
        round.starts_with("round-")
            && round[6..].bytes().all(|b| b.is_ascii_digit())
            && round.len() > 6,
        "invalid loop round label"
    );
    let store = EvidenceStore::create(directory)?;
    let result: serde_json::Value =
        serde_json::from_slice(&store.read_private("merge-ready-result.json")?)?;
    ensure!(
        result["loop_result"] == "DONE"
            && result["round_label"] == round
            && result["round_verdict"] == "MERGE_READY",
        "loop completion requires accepted MERGE_READY and evaluator DONE"
    );
    let raw: serde_json::Value =
        serde_json::from_slice(&store.read_private(&format!("merge-ready-{round}.json"))?)?;
    ensure!(
        raw["merge_ready_verdict"] == "MERGE_READY",
        "loop round is not accepted ready"
    );
    let path = raw["accepted_evidence"]
        .as_str()
        .context("loop round has no accepted evidence snapshot")?;
    let candidate = basename(Path::new(path), directory)?;
    let bytes = store.read_private(&candidate)?;
    let record: CandidateRecord = serde_json::from_slice(&bytes)?;
    ensure!(
        record.readiness == Readiness::MergeReady,
        "loop evidence is incomplete"
    );
    let receipt = Receipt {
        schema_version: 1,
        run_id: auth.run_id().into(),
        round_id: round.into(),
        candidate,
        candidate_sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    store.write_new(&name(auth), &serde_json::to_vec(&receipt)?)?;
    Ok(())
}
/// Historical markers and an agent's completion labels cannot establish current readiness.
pub fn verify(
    auth: &InvocationAuthorization,
    directory: &Path,
    current: &CandidateRecord,
) -> Result<()> {
    let store = EvidenceStore::create(directory)?;
    let receipt_bytes = store
        .read_private(&name(auth))
        .context("fresh loop completion receipt required")?;
    let receipt: Receipt =
        serde_json::from_slice(&receipt_bytes).context("fresh loop completion receipt required")?;
    ensure!(
        receipt.schema_version == 1 && receipt.run_id == auth.run_id(),
        "completion belongs to another invocation"
    );
    let bytes = store.read_private(&receipt.candidate)?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == receipt.candidate_sha256,
        "accepted loop evidence was altered"
    );
    let earlier: CandidateRecord = serde_json::from_slice(&bytes)?;
    let mut binding = earlier.binding.clone();
    binding.run_id.clone_from(&current.binding.run_id);
    binding.round_id.clone_from(&current.binding.round_id);
    binding
        .policy_provenance
        .clone_from(&current.binding.policy_provenance);
    ensure!(
        earlier.readiness == Readiness::MergeReady && binding == current.binding,
        "accepted loop evidence is stale or belongs to another PR/head/base"
    );
    Ok(())
}
