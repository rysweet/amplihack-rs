//! The environment variables an agent entry may record (R2.4, D7, S21): a
//! closed list of exact names. Anything else, credentials above all, is
//! never recorded and never mentioned, because a name off the list may itself
//! be a secret typed in the wrong place.

use super::validate;
use std::collections::BTreeMap;

/// Any change to this list needs a security review (S21).
pub const ALLOWLIST: &[&str] = &[
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_EFFORT_LEVEL",
    "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
    "MAX_THINKING_TOKENS",
    "AMPLIHACK_DEFAULT_MODEL",
    "AMPLIHACK_AGENT_BINARY",
];

const AGENT_BINARY: &str = "AMPLIHACK_AGENT_BINARY";
const BINARY_RULE: &str = "value must be one of claude, copilot, codex or amplifier";

pub fn allowed(name: &str) -> bool {
    ALLOWLIST.contains(&name)
}

/// The rule `value` breaks for the allowlisted `name`, if any (S8b for the
/// agent binary, `env_value` for the rest).
fn value_rule(name: &str, value: &str) -> Result<(), String> {
    if name == AGENT_BINARY {
        let canonical = amplihack_utils::agent_binary::validate_binary_name(value);
        return match canonical {
            Some(c) if c == value => Ok(()),
            _ => Err(BINARY_RULE.to_string()),
        };
    }
    validate::env_value(value)
}

/// Whether `name=value` may be stored: the name is listed and the value
/// passes its rule. Used by the whole-entry check.
pub fn value_ok(name: &str, value: &str) -> bool {
    allowed(name) && value_rule(name, value).is_ok()
}

/// The listed variables among `pairs`, and `(name, rule)` for each listed
/// variable whose value was refused. Unlisted names are dropped silently.
pub fn record(
    pairs: impl IntoIterator<Item = (String, String)>,
) -> (BTreeMap<String, String>, Vec<(String, String)>) {
    let mut kept = BTreeMap::new();
    let mut refused = Vec::new();
    for (name, value) in pairs {
        if !allowed(&name) {
            continue;
        }
        match value_rule(&name, &value) {
            Ok(()) => {
                kept.insert(name, value);
            }
            Err(rule) => refused.push((name, rule)),
        }
    }
    (kept, refused)
}

/// The listed variables of this process. Values that are not UTF-8 are
/// refused like any other bad value.
pub fn from_current_env() -> (BTreeMap<String, String>, Vec<(String, String)>) {
    let mut refused = Vec::new();
    let mut pairs = Vec::new();
    for name in ALLOWLIST {
        match std::env::var(name) {
            Ok(v) => pairs.push((name.to_string(), v)),
            Err(std::env::VarError::NotUnicode(_)) => {
                refused.push((name.to_string(), "value must be UTF-8".to_string()));
            }
            Err(std::env::VarError::NotPresent) => {}
        }
    }
    let (kept, mut bad) = record(pairs);
    refused.append(&mut bad);
    (kept, refused)
}

/// One `--env KEY=VALUE` argument. The error is the whole message and never
/// repeats the argument (B10).
pub fn parse_flag(kv: &str) -> Result<(String, String), String> {
    let Some((name, value)) = kv.split_once('=') else {
        return Err("--env: expected KEY=VALUE".to_string());
    };
    if !allowed(name) {
        return Err(format!(
            "--env: only these variables can be recorded: {}",
            ALLOWLIST.join(", ")
        ));
    }
    value_rule(name, value).map_err(|rule| format!("--env: {rule}"))?;
    Ok((name.to_string(), value.to_string()))
}
