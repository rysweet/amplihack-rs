//! Warning publication follows the existing transaction journal, rather than
//! the latest resume signal. Lock order: swap lock, then history lock.
use amplihack_utils::{organisation_affinity::Affinity, swap_warning_history::WarningHistory};
use std::path::Path;

pub fn rotator_key(pid: u32, started: i64, live: u64, target: u64) -> String {
    format!("rotator-{pid}-{started}-{live}-{target}")
}
pub fn auto_key(pid: u32, started: i64, host: &str, from: &str, to: &str) -> String {
    format!("auto-{pid}-{started}-{host}-{from}-{to}")
}
/// Advisory: the warning journal never decides whether a swap happens.
/// The swap was the job; the warning tells live sessions about it. A
/// failure here is reported on stderr and the swap goes ahead (or, after
/// the move, finishes) without the warning. The swap record and the rotate
/// log still record the organisation outcome.
fn report(producer: &str, step: &str, error: &dyn std::fmt::Display) {
    eprintln!("{producer}: session warning {step} skipped; the swap is unaffected: {error}");
}

/// Write the durable warning intent for `tx` before the credential move.
/// Returns `Some(tx)` when the intent was written, so the swap journal
/// references it, or `None` (reported) when the warning journal could not
/// take it: full, unsafe, or an unreadable transaction journal.
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
) -> Option<String> {
    let source = source.filter(|id| amplihack_utils::organisation_affinity::valid_id(id));
    let target = target.filter(|id| amplihack_utils::organisation_affinity::valid_id(id));
    let written = (|| -> std::io::Result<()> {
        let mut history = WarningHistory::open(root)?;
        retire_unreferenced(root, &mut history)?;
        history.prepare(&serde_json::json!({
            "version": 1, "transaction_id": tx, "producer": producer,
            "reason": reason, "host": host, "from_login": from, "to_login": to,
            "from_org": source, "to_org": target,
            "affinity": Affinity::between(source, target),
        }))?;
        Ok(())
    })();
    match written {
        Ok(()) => Some(tx.to_owned()),
        Err(e) => {
            report(producer, "intent", &e);
            None
        }
    }
}

/// Publish the warning for a completed swap. Called after credentials
/// moved: a failure is reported and the transaction journal is still
/// removed, because the swap itself is complete.
pub fn complete(root: &Path, tx: &str, producer: &str, host: &str, from: &str, to: &str) {
    let published = WarningHistory::open(root)
        .and_then(|mut history| history.complete_transaction(tx, producer, host, from, to));
    match published {
        Ok(true) => {}
        Ok(false) => report(producer, "publication", &"no durable warning intent"),
        Err(e) => report(producer, "publication", &e),
    }
}

/// Drop the warning intent of a swap that did not happen. Reported only on
/// failure: a stale intent is never delivered, since only completed
/// transactions publish events.
pub fn abort(root: &Path, tx: &str, producer: &str, host: &str, from: &str, to: &str) {
    if let Err(e) = WarningHistory::open(root)
        .and_then(|mut history| history.abort_transaction(tx, producer, host, from, to))
    {
        report(producer, "abort", &e);
    }
}

/// Called under the swap lock (`.auto-switch.d/lock`, shared by the rotator
/// and auto-switch) before creating a new intent. Both authoritative
/// journals are inspected, so a crash after journal deletion cannot leak
/// completion evidence, and a crash between writing an intent and writing
/// its journal cannot leave a pending intent that nothing will ever settle.
/// An unreadable journal holds retention rather than guessing that recovery
/// has finished.
fn retire_unreferenced(root: &Path, history: &mut WarningHistory) -> std::io::Result<()> {
    history.retire_unreferenced_with(|| {
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

    /// A producer killed after writing its intent but before writing its swap
    /// journal leaves an intent no recovery path can reach. The next
    /// `prepare` under the swap lock clears it, and a referenced intent (a
    /// swap whose journal recovery will settle) is kept.
    #[test]
    fn issue_2136_orphaned_pending_intent_is_cleared_by_the_next_prepare() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = tmp.path();
        let intent = |tx: &str| {
            prepare(
                root,
                tx,
                "rotator",
                "threshold",
                "home",
                "work",
                "spare",
                Some("a"),
                Some("b"),
            )
        };
        let settle = |tx: &str| {
            WarningHistory::open(root)
                .unwrap()
                .complete_transaction(tx, "rotator", "home", "work", "spare")
                .unwrap()
        };
        assert!(intent("orphan").is_some());
        // Killed here: no swap journal refers to "orphan".
        assert!(intent("next").is_some());
        assert!(!settle("orphan"), "the orphaned intent was cleared");
        // "next" is referenced by a swap journal, so recovery still owns it.
        let dir = root.join(".auto-switch.d/rotate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("journal.json"),
            br#"{"warning_transaction":"next"}"#,
        )
        .unwrap();
        assert!(intent("third").is_some());
        assert!(settle("next"), "a referenced intent is kept");
    }
}
