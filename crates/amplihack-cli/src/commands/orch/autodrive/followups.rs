//! Render established minor-finding mappings without treating them as readiness authority.
use super::state::EvidenceStore;
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
pub struct Followup {
    finding_id: String,
    issue_number: u64,
    task: String,
    pr_number: u64,
}
pub fn read(store: &EvidenceStore, directory: &Path, pr: u64) -> Result<Vec<Followup>> {
    match directory.join("followups.tsv").symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("inspect follow-up mappings"),
        Ok(_) => (),
    }
    let text = String::from_utf8(store.read_private("followups.tsv")?)?;
    let mut rows = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for line in text.lines().filter(|s| !s.is_empty()) {
        let fields: Vec<_> = line.split('\t').collect();
        ensure!(
            fields.len() == 4,
            "malformed follow-up mapping: expected id, issue, task, PR"
        );
        ensure!(
            !fields[0].is_empty()
                && fields[0]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                && ids.insert(fields[0]),
            "invalid or duplicate follow-up finding id"
        );
        let issue_number = fields[1].parse::<u64>()?;
        let pr_number = fields[3].parse::<u64>()?;
        ensure!(
            issue_number > 0
                && pr_number == pr
                && !fields[2].is_empty()
                && !fields[2].chars().any(char::is_control),
            "follow-up mapping has invalid task or belongs to another PR"
        );
        rows.push(Followup {
            finding_id: fields[0].into(),
            issue_number,
            task: fields[2].into(),
            pr_number,
        });
    }
    Ok(rows)
}
