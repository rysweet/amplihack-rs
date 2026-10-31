//! Public manifest and report contract. Source paths never enter reports.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
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
}

#[derive(Serialize)]
pub struct SessionReport {
    pub id: String,
    pub run_id: Option<String>,
    pub parent: Option<String>,
    pub provider: Provider,
    pub direct: Tokens,
    pub inclusive: Tokens,
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
pub struct Report {
    pub version: u8,
    pub workstream: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub prs: Vec<u64>,
    pub allocation: &'static str,
    pub total: Tokens,
    pub unattributed: Tokens,
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
