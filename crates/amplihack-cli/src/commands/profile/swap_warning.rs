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
