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

/// Adds every policy entry missing from `settings["permissions"]["deny"]`,
/// keeping the entries already there and their order, and creating the
/// `permissions` object or the `deny` array only when neither is present.
///
/// A value on that path that is not the shape the entries go into is
/// reported, never replaced (SEC-C2): each refusal below returns before
/// anything is inserted, so the file is left exactly as it was found. The
/// reason names the key, never the value, which comes from the file.
///
/// Idempotent: a second call on the same value reports
/// [`DenyOutcome::AlreadyCurrent`] and changes nothing, which is what lets
/// the file writers compare by bytes.
pub(crate) fn ensure_deny(settings: &mut Value, instance: &[String]) -> DenyOutcome {
    let Some(root) = settings.as_object_mut() else {
        return DenyOutcome::Unparseable("the settings root is not a JSON object".to_string());
    };
    let permissions = root
        .entry("permissions")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(permissions) = permissions.as_object_mut() else {
        return DenyOutcome::Unparseable("permissions is not a JSON object".to_string());
    };
    let deny = permissions
        .entry("deny")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(deny) = deny.as_array_mut() else {
        return DenyOutcome::Unparseable("permissions.deny is not a JSON array".to_string());
    };

    let mut changed = false;
    for entry in deny_entries(instance) {
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
// the record.
//
// The record is per thread: the install writes and reads it on one thread,
// and a per-thread record keeps this crate's tests independent of each other
// without a lock.
//
// This is the wrong shape and is being replaced. Its correctness rests on
// four call-site rules nothing enforces: clear the record where each write
// phase starts, record from every writer including the arms that return
// before the merge closure runs, read the record after every writer, and
// clear it in any test that reads it. Rule 2 was broken at two of the four
// sites when they were written. The explicit channel already exists:
// `refresh_profile_settings` returns the skipped profiles with their
// reasons, and `apply_deny_policy` returns `Option<String>` beside
// recording, so the same fact travels two channels. Passing those into
// `install_report_lines` deletes `record`, `clear_record`, `take_record`,
// the three clear-sites and 46 test calls, and makes the bug class
// unrepresentable. Tracked as issue #2751, which also moves
// `apply_deny_policy` out of the install module into this one.

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

/// Empties the record, so that what is reported next describes the writes
/// that come after this call alone.
///
/// [`take_record`] already empties the record on its way out, which covers a
/// phase that reaches its policy section. A phase that does not reach one --
/// the launch bootstrap, which writes `settings.json` after it has read the
/// record, profile seeding, or another test on this thread -- leaves its
/// outcomes behind for whatever runs next in the same process. Reporting one
/// of those as this run's work would print a positive line about a file this
/// run never met, so each install entry point clears the record where its
/// own write phase starts (SEC-C3).
///
/// Clearing an already-empty record is not an error, so the call needs no
/// condition around it.
pub(crate) fn clear_record() {
    RECORD.with_borrow_mut(Vec::clear);
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
        (_, 0) => HarnessWrite::Written,
        (met, bad) if met == bad => HarnessWrite::AllRefused,
        _ => HarnessWrite::Partial,
    };
    (outcome, refused)
}
