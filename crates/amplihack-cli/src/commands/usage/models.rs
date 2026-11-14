//! Public manifest and report contract. Source paths never enter reports.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u8,
    pub workstream: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
    #[serde(default)]
    pub prs: Vec<u64>,
    #[serde(default)]
    pub completed: bool,
    pub sessions: Vec<Session>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// Native provider session ID. Resumes list additional files here.
    pub id: String,
    pub provider: Provider,
    pub run_id: Option<String>,
    pub parent: Option<String>,
    /// Explicit assertion that all expected records for this session are supplied.
    #[serde(default)]
    pub complete: bool,
    pub files: Vec<PathBuf>,
}

/// Adapter names are exact; safe unknown names retain their spelling.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provider {
    Claude,
    Codex,
    Unsupported(String),
}
impl Provider {
    pub fn label(&self) -> &str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Unsupported(name) => name,
        }
    }
}
impl Serialize for Provider {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.label())
    }
}
impl<'de> Deserialize<'de> for Provider {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        if !safe_label(&name) {
            return Err(serde::de::Error::custom("invalid provider label"));
        }
        Ok(match name.as_str() {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            _ => Self::Unsupported(name),
        })
    }
}

/// Direct input validity travels with observations, independently of token lower bounds.
#[derive(Clone, Default)]
pub(super) struct InputEvidence {
    pub reasons: BTreeSet<&'static str>,
}
impl InputEvidence {
    pub fn tokens(&self, provider: &Provider, tokens: Option<Tokens>) -> Option<u64> {
        if !self.reasons.is_empty() {
            return None;
        }
        let t = tokens?;
        if *provider == Provider::Codex {
            Some(t.input)
        } else {
            t.input
                .checked_add(t.cache_read)?
                .checked_add(t.cache_write)
        }
    }
}

#[derive(Serialize)]
pub struct ContextPerRequest {
    pub input_tokens: Option<u64>,
    pub observed_attempts: Option<u64>,
    pub tokens_per_observed_attempt: Option<f64>,
    pub unit: &'static str,
    pub attempt_basis: &'static str,
    pub unknown_reasons: BTreeSet<&'static str>,
}

/// Disjoint token buckets except cache-write duration fields, which partition cache_write.
/// Codex input excludes cache reads; output excludes separately reported reasoning.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub cache_write: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    pub output: u64,
    pub reasoning: u64,
}

impl Tokens {
    pub fn values(self) -> [u64; 7] {
        [
            self.input,
            self.cache_write,
            self.cache_write_5m,
            self.cache_write_1h,
            self.cache_read,
            self.output,
            self.reasoning,
        ]
    }
    pub fn from_values(v: [u64; 7]) -> Self {
        Self {
            input: v[0],
            cache_write: v[1],
            cache_write_5m: v[2],
            cache_write_1h: v[3],
            cache_read: v[4],
            output: v[5],
            reasoning: v[6],
        }
    }
    pub fn add(&mut self, other: Self) {
        *self = Self::from_values(std::array::from_fn(|i| {
            self.values()[i].saturating_add(other.values()[i])
        }));
    }
    pub fn max(&mut self, other: Self) {
        *self = Self::from_values(std::array::from_fn(|i| {
            self.values()[i].max(other.values()[i])
        }));
    }
    pub fn total(self) -> u64 {
        [
            self.input,
            self.cache_write,
            self.cache_read,
            self.output,
            self.reasoning,
        ]
        .into_iter()
        .fold(0u64, u64::saturating_add)
    }
}

#[derive(Clone, Serialize)]
pub struct ModelUsage {
    pub provider: Provider,
    pub model: String,
    pub tokens: Tokens,
    /// Unique observed provider messages / positive cumulative transitions.
    pub observed_attempts: u64,
    /// Native Claude records include unsplit reasoning in output.
    pub reasoning_in_output: bool,
}

#[derive(Serialize)]
pub struct SessionReport {
    pub id: String,
    pub run_id: Option<String>,
    pub parent: Option<String>,
    pub provider: Provider,
    pub identifier_backed_response_count: u64,
    pub context_per_request: ContextPerRequest,
    pub direct: Option<Tokens>,
    pub inclusive: Option<Tokens>,
    pub models: Vec<ModelUsage>,
    pub complete: bool,
    pub first_observed: Option<String>,
    pub last_observed: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    /// Manifest session ordinal; never a path or raw record fragment.
    pub session: Option<usize>,
    pub file: Option<usize>,
    pub line: Option<usize>,
    pub code: String,
}

#[derive(Serialize)]
pub struct UnresolvedUsage {
    pub provider: Provider,
    pub model: String,
    pub tokens: Tokens,
    pub reason: &'static str,
}

#[derive(Serialize)]
pub struct Report {
    pub version: u8,
    pub workstream: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub prs: Vec<u64>,
    pub allocation: &'static str,
    pub identifier_backed_response_count: u64,
    pub total: Option<Tokens>,
    pub unattributed: Option<Tokens>,
    /// Non-additive evidence: stable identity is missing, so deduplication is impossible.
    pub unresolved: Vec<UnresolvedUsage>,
    pub models: Vec<ModelUsage>,
    pub sessions: Vec<SessionReport>,
    pub complete: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub first_observed: Option<String>,
    pub last_observed: Option<String>,
    pub api_price_estimate: Option<f64>,
    pub price_reference: Option<String>,
    pub price_assumptions: &'static str,
    pub attempt_semantics: &'static str,
}

pub(super) fn safe_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 160
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-:@".contains(&b))
}
