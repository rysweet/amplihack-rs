//! Issue #1976 (decision 17.113): context keys that were removed.
//!
//! A caller who still passes one gets an error and exit 1 before any work,
//! never a silent no-op. `should_publish` looked like a switch that stopped
//! a run from publishing, but nothing ever read it.
//!
//! The key is the text before the first `=`, trimmed, compared exactly
//! (case-sensitive, like every other context key). Every value is refused.
//! The message names the key only, never the value or the whole argument.

use crate::command_error::exit_error;
use anyhow::Result;
use std::io::Write;

/// `(key, message)` for every removed context key.
pub(crate) const REMOVED_CONTEXT_KEYS: &[(&str, &str)] = &[(
    "should_publish",
    "Error: context key 'should_publish' was removed by #1976 (decision 17.113). \
     It never stopped a run from publishing; do not rely on it. \
     Remove '-c should_publish=…' from the command.",
)];

/// The removed key `arg` (a `KEY=VALUE` pair) passes, if any.
pub(crate) fn removed_key(arg: &str) -> Option<&'static str> {
    let key = arg.split_once('=').map_or(arg, |(key, _)| key).trim();
    REMOVED_CONTEXT_KEYS
        .iter()
        .find(|(removed, _)| *removed == key)
        .map(|(removed, _)| *removed)
}

/// Refuse a run whose resolved context passes a removed key: one message per
/// distinct key on `out`, then exit 1. Writes nothing and returns `Ok` when
/// no removed key is present.
pub(crate) fn reject_removed_context_keys(pairs: &[String], out: &mut impl Write) -> Result<()> {
    let mut found: Vec<&'static str> = Vec::new();
    for pair in pairs {
        if let Some(key) = removed_key(pair)
            && !found.contains(&key)
        {
            found.push(key);
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    for (key, message) in REMOVED_CONTEXT_KEYS {
        if found.contains(key) {
            writeln!(out, "{message}")?;
        }
    }
    Err(exit_error(1))
}
