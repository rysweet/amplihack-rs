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
