//! Issue #2589: the Claude Code rendering of the policy list.
//!
//! Claude Code's own mechanism is `permissions.deny` in `settings.json`, whose
//! entries are tool patterns: `Bash(<prefix>:*)` matches a Bash command whose
//! text starts with `<prefix>`. The pattern has no negation, so an entry that
//! must spare some forms of a command is rendered as one prefix per forbidden
//! form; that is why the forced and deleting pushes become five entries and
//! plain `git push` keeps working.

use serde_json::{Map, Value};

/// What [`ensure_deny`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DenyOutcome {
    /// Entries were added.
    Written,
    /// Every entry was already present; nothing was touched.
    AlreadyCurrent,
    /// The file holds a `permissions` value that cannot be merged without
    /// discarding it. Reported, never replaced (SEC-C2).
    Unparseable(String),
}

/// The `permissions.deny` entries for `instance`, in policy order.
pub(crate) fn deny_entries(instance: &[String]) -> Vec<String> {
    super::effective_prefixes(instance)
        .iter()
        .map(|prefix| format!("Bash({}:*)", prefix.as_str()))
        .collect()
}

/// Why `settings` cannot take the deny entries without discarding a value
/// the user set. The reason names the key, never the value, which comes from
/// the file.
fn unmergeable(settings: &Value) -> Option<String> {
    let Value::Object(root) = settings else {
        return Some("the settings root is not a JSON object".to_string());
    };
    match root.get("permissions") {
        None => None,
        Some(Value::Object(permissions)) => match permissions.get("deny") {
            None | Some(Value::Array(_)) => None,
            Some(_) => Some("permissions.deny is not a JSON array".to_string()),
        },
        Some(_) => Some("permissions is not a JSON object".to_string()),
    }
}

/// Adds every policy entry missing from `settings["permissions"]["deny"]`,
/// keeping the entries already there and their order, and creating the
/// `permissions` object or the `deny` array only when neither is present.
///
/// Idempotent: a second call on the same value reports
/// [`DenyOutcome::AlreadyCurrent`] and changes nothing, which is what lets
/// the file writers compare by bytes.
pub(crate) fn ensure_deny(settings: &mut Value, instance: &[String]) -> DenyOutcome {
    if let Some(reason) = unmergeable(settings) {
        return DenyOutcome::Unparseable(reason);
    }
    let entries = deny_entries(instance);
    let root = settings
        .as_object_mut()
        .expect("unmergeable() accepted a non-object root");
    let permissions = root
        .entry("permissions")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("unmergeable() accepted a non-object permissions value");
    let deny = permissions
        .entry("deny")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("unmergeable() accepted a non-array deny value");

    let mut changed = false;
    for entry in entries {
        if !deny
            .iter()
            .any(|value| value.as_str() == Some(entry.as_str()))
        {
            deny.push(Value::String(entry));
            changed = true;
        }
    }
    if changed {
        DenyOutcome::Written
    } else {
        DenyOutcome::AlreadyCurrent
    }
}

// --- the record the Claude writers leave behind -----------------------------
//
// `permissions.deny` is written from three places: the install's own settings
// writer, the per-profile refresh and profile seeding. They run at different
// points of a run and none of them returns to the install's policy section,
// so each records what the file it met came to and the policy section reads
// the record. The alternative, threading a result back through all three, put
// the reason in `tracing::warn!` alone, which an ordinary install never shows.
//
// The record is per thread: the install writes and reads it on one thread,
// and a per-thread record keeps this crate's tests independent of each other
// without a lock.

use std::cell::RefCell;

thread_local! {
    /// One entry per settings file this thread met, in the order it met them:
    /// the label naming the file, and the reason it took no entries, if any.
    static RECORD: RefCell<Vec<(String, Option<String>)>> = const { RefCell::new(Vec::new()) };
}

/// Records what one Claude settings file came to. `label` names the file
/// (`home settings`, `profile work`, `new profile fresh`) and never holds
/// file content. Recording the same label twice describes the same file, so
/// the later outcome replaces the earlier one.
pub(crate) fn record(label: &str, outcome: &DenyOutcome) {
    let reason = match outcome {
        DenyOutcome::Written | DenyOutcome::AlreadyCurrent => None,
        DenyOutcome::Unparseable(reason) => Some(reason.clone()),
    };
    RECORD.with_borrow_mut(
        |entries| match entries.iter_mut().find(|(seen, _)| seen == label) {
            Some(entry) => entry.1 = reason,
            None => entries.push((label.to_string(), reason)),
        },
    );
}

/// What the record adds up to, and one warning line per refused file,
/// clearing the record. An empty record is
/// [`HarnessWrite::NothingToWriteInto`](super::HarnessWrite::NothingToWriteInto):
/// no settings file was met, so nothing was written and nothing failed.
pub(crate) fn take_record() -> (super::HarnessWrite, Vec<String>) {
    use super::HarnessWrite;
    let entries = RECORD.with_borrow_mut(std::mem::take);
    let refused: Vec<String> = entries
        .iter()
        .filter_map(|(label, reason)| {
            reason.as_ref().map(|reason| {
                super::write_failure_warning(&format!("Claude Code ({label})"), reason)
            })
        })
        .collect();
    let outcome = match (entries.len(), refused.len()) {
        (0, _) => HarnessWrite::NothingToWriteInto,
        (met, 0) => {
            debug_assert!(met > 0);
            HarnessWrite::Written
        }
        (met, bad) if met == bad => HarnessWrite::AllRefused,
        _ => HarnessWrite::Partial,
    };
    (outcome, refused)
}
