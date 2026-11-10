//! Warning publication follows the existing transaction journal, rather than
//! the latest resume signal. Lock order: swap lock, then history lock.
use super::ProfileError;
use amplihack_utils::{organisation_affinity::Affinity, swap_warning_history::WarningHistory};
use std::path::Path;

pub fn rotator_key(pid: u32, started: i64, live: u64, target: u64) -> String {
    format!("rotator-{pid}-{started}-{live}-{target}")
}
pub fn auto_key(pid: u32, started: i64, host: &str, from: &str, to: &str) -> String {
    format!("auto-{pid}-{started}-{host}-{from}-{to}")
}
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    root: &Path,
    tx: &str,
    producer: &str,
    reason: &str,
    host: &str,
    from: &str,
    to: &str,
    source: Option<&str>,
    target: Option<&str>,
) -> Result<(), ProfileError> {
    let source = source.filter(|id| amplihack_utils::organisation_affinity::valid_id(id));
    let target = target.filter(|id| amplihack_utils::organisation_affinity::valid_id(id));
    let mut history = WarningHistory::open(root)?;
    retire_unreferenced(root, &mut history)?;
    history.prepare(&serde_json::json!({
        "version": 1, "transaction_id": tx, "producer": producer,
        "reason": reason, "host": host, "from_login": from, "to_login": to,
        "from_org": source, "to_org": target,
        "affinity": Affinity::between(source, target),
    }))?;
    Ok(())
}
pub fn complete(
    root: &Path,
    tx: &str,
    producer: &str,
    host: &str,
    from: &str,
    to: &str,
) -> Result<(), ProfileError> {
    WarningHistory::open(root)
        .and_then(|mut history| history.complete_transaction(tx, producer, host, from, to))
        .and_then(|present| {
            if present {
                Ok(())
            } else {
                Err(std::io::Error::other(
                    "completed swap has no durable warning intent",
                ))
            }
        })
        .map_err(|error| {
            ProfileError::Io(std::io::Error::other(format!(
                "swap completed; warning publication pending; transaction journal retained: {error}"
            )))
        })?;
    Ok(())
}

/// Called under the producer swap lock before creating a new intent. Both
/// authoritative journals are inspected, so a crash after journal deletion
/// cannot leak completion evidence indefinitely. An unreadable journal holds
/// retention rather than guessing that recovery has finished.
fn retire_unreferenced(root: &Path, history: &mut WarningHistory) -> std::io::Result<()> {
    history.retire_completed_with(|| {
        let mut referenced = Vec::new();
        for path in [
            ".auto-switch.d/journal.json",
            ".auto-switch.d/rotate/journal.json",
        ] {
            if let Some((raw, _)) = amplihack_utils::secure_files::read_bounded_nofollow(
                &root.join(path),
                8 * 1024 * 1024,
            )? {
                let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| {
                    std::io::Error::other("transaction retention journal unreadable")
                })?;
                if let Some(tx) = value.get("warning_transaction").and_then(|v| v.as_str()) {
                    referenced.push(tx.to_owned());
                }
            }
        }
        Ok(referenced)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use amplihack_utils::swap_warning_history::Completion;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn issue_2136_completion_evidence_follows_authoritative_reference() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = tmp.path();
        let mut h = WarningHistory::open(root).unwrap();
        let session = h.register_current_process("s", "home").unwrap();
        let intent = h
            .prepare(&serde_json::json!({
                "version":1,"transaction_id":"tx","producer":"rotator",
                "reason":"threshold","host":"home","from_login":"work","to_login":"spare",
                "from_org":"a","to_org":"b","affinity":"crossing"
            }))
            .unwrap();
        h.complete(&intent, Completion::Committed).unwrap();
        let dir = root.join(".auto-switch.d/rotate");
        std::fs::create_dir_all(&dir).unwrap();
        let journal = dir.join("journal.json");
        std::fs::write(&journal, br#"{"warning_transaction":"tx"}"#).unwrap();
        retire_unreferenced(root, &mut h).unwrap();
        h.complete(&intent, Completion::Committed).unwrap();
        std::fs::remove_file(journal).unwrap();
        retire_unreferenced(root, &mut h).unwrap();
        assert!(h.complete(&intent, Completion::Committed).is_err());
        assert_eq!(h.pending(&session).unwrap().len(), 1);
    }
}
