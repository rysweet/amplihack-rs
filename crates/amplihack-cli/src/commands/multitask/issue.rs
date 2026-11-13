//! One optional-issue parser for the model, configuration and persistence
//! boundaries (issue #2553).
//!
//! A decomposition does not know issue numbers. Before this parser the
//! workstreams-config minted the placeholder `"TBD"`, which read back as `0`,
//! which tripped the `Invalid issue number` bail in the orchestrator. Absence
//! is now a first-class answer: [`parse_optional`] is total over well-formed
//! input and returns `Ok(None)` for every form an absent issue has been seen
//! to take.
//!
//! The accept set for *absence* widens freely; the accept set for *presence*
//! does not. A value that is neither is a producer mistake and stays an error.

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::num::NonZeroU64;

/// A tracked issue number. `NonZeroU64` so that `0` cannot be represented and
/// no sentinel can masquerade as a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IssueNumber(NonZeroU64);

impl IssueNumber {
    pub(crate) fn get(self) -> u64 {
        self.0.get()
    }
}

/// Spellings of "no issue yet", matched case-insensitively after trimming.
const PLACEHOLDERS: &[&str] = &[
    "tbd",
    "tbc",
    "todo",
    "n/a",
    "none",
    "null",
    "unknown",
    "unassigned",
    "pending",
];

/// The only host whose issue URLs are accepted. Security control, not
/// convenience: without it `https://attacker.example/issues/1` yields `1`, and
/// an agent told to comment on or close the tracked issue acts on issue 1 of
/// the *current* repository.
const ISSUE_URL_PREFIXES: &[&str] = &["https://github.com/", "http://github.com/"];

/// Parse an optional issue number from a JSON value.
///
/// Absent: JSON `null`, an empty or whitespace-only string, and any entry of
/// [`PLACEHOLDERS`]. Present: a positive integer, a positive decimal string
/// (leading zeroes allowed), `#N`, and a `github.com` issue URL ending
/// `/issues/<N>`. Everything else is an error.
pub(crate) fn parse_optional(value: &Value) -> Result<Option<IssueNumber>> {
    let number = match value {
        Value::Null => return Ok(None),
        Value::String(raw) => {
            let s = raw.trim();
            if s.is_empty() || PLACEHOLDERS.iter().any(|p| s.eq_ignore_ascii_case(p)) {
                return Ok(None);
            }
            parse_present_string(s)?
        }
        Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("issue must be a positive integer, got {n}"))?,
        other => bail!(
            "issue must be a positive integer, a decimal string, `#N`, a github.com \
             issue URL, or absent; got {}",
            kind_of(other)
        ),
    };
    Ok(Some(IssueNumber(NonZeroU64::new(number).ok_or_else(
        || anyhow::anyhow!("issue must be greater than zero"),
    )?)))
}

/// `PersistedState::issue` reader. A state file written before #2553 carries
/// `"issue": 0` for a workstream that never had one; that legacy zero reads
/// back as absence. The coercion is confined to persisted state — in
/// configuration a `0` is a producer mistake worth surfacing.
pub(crate) fn deserialize_persisted<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<u64>, D::Error> {
    let value = Value::deserialize(d)?;
    if value.as_i64() == Some(0) || value.as_u64() == Some(0) {
        return Ok(None);
    }
    parse_optional(&value)
        .map(|v| v.map(IssueNumber::get))
        .map_err(serde::de::Error::custom)
}

fn parse_present_string(s: &str) -> Result<u64> {
    if let Some(rest) = s.strip_prefix('#') {
        return parse_decimal(rest);
    }
    if s.contains("://") {
        return parse_issue_url(s);
    }
    parse_decimal(s)
}

fn parse_decimal(s: &str) -> Result<u64> {
    ensure!(
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()),
        "issue must be a positive decimal integer, `#N`, a github.com issue URL, \
         or absent; got {s:?}"
    );
    s.parse::<u64>()
        .map_err(|_| anyhow::anyhow!("issue number {s:?} exceeds u64::MAX"))
}

/// Purely lexical: no network, no DNS, no redirect following. The host is
/// pinned by an exact scheme-and-authority prefix, so `github.com.evil.test`
/// and `github.com@evil.test` both fail to match.
fn parse_issue_url(s: &str) -> Result<u64> {
    let lower = s.to_ascii_lowercase();
    let rest = ISSUE_URL_PREFIXES
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix).map(|r| &s[s.len() - r.len()..]))
        .ok_or_else(|| {
            anyhow::anyhow!("an issue URL is accepted only for github.com; got {s:?}")
        })?;
    let path = rest.split(['?', '#']).next().unwrap_or_default();
    let segments: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    ensure!(
        segments.len() >= 2,
        "a github.com issue URL must end with /issues/<number>; got {s:?}"
    );
    let kind = segments[segments.len() - 2];
    ensure!(
        kind == "issues",
        "a github.com issue URL must end with /issues/<number>, not /{kind}/; got {s:?}"
    );
    parse_decimal(segments[segments.len() - 1])
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}
