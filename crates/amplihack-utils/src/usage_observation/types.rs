//! Versioned, non-secret evidence and its captured login identity.
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 2;

pub const MAX_RECORD_BYTES: usize = 16 * 1024;
#[cfg(unix)]
pub(crate) const MAX_TIME: i64 = 253_402_300_799;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub profile: String,
    pub account_id: String,
    pub generation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub binding: Binding,
    pub observation_id: String,
    pub ticket: u64,
    pub captured_at: i64,
}

/// Availability is explicit: an omitted window never proves no session limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionWindow {
    Present,
    Absent,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    #[serde(default)]
    pub session_window: SessionWindow,
    pub session_used: Option<f64>,
    pub week_used: Option<f64>,
    pub session_reset_at: Option<i64>,
    pub week_reset_at: Option<i64>,
    pub limit_reached: bool,
}
#[cfg(unix)]
impl Evidence {
    pub(crate) fn valid(&self) -> bool {
        (match self.session_window {
            SessionWindow::Present => self.session_used.is_some() || self.limit_reached,
            SessionWindow::Absent | SessionWindow::Unknown => {
                self.session_used.is_none() && self.session_reset_at.is_none()
            }
        }) && (self.limit_reached
            || (self.week_used.is_some()
                && match self.session_window {
                    SessionWindow::Present => self.session_used.is_some(),
                    SessionWindow::Absent => true,
                    SessionWindow::Unknown => false,
                }))
            && [self.session_used, self.week_used]
                .into_iter()
                .flatten()
                .all(|v| v.is_finite() && (0.0..=100.0).contains(&v))
            && [self.session_reset_at, self.week_reset_at]
                .into_iter()
                .flatten()
                .all(|v| (0..=MAX_TIME).contains(&v))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub schema_version: u32,
    pub capture: Capture,
    pub evidence: Evidence,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReadOutcome {
    Usable(Record),
    Missing,
    StaleGeneration,
    UnsupportedSchema,
    Invalid,
    Unsafe,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    Accepted { revision: u64 },
    Duplicate,
    Superseded,
    RetiredGeneration,
    InvalidAttribution,
    InvalidEvidence,
    Unsafe,
}

#[cfg(unix)]
pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
#[cfg(unix)]
pub(crate) fn profile(value: &str) -> bool {
    identifier(value)
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-..".contains(&b))
}
#[cfg(unix)]
pub(crate) fn valid_binding(b: &Binding) -> bool {
    profile(&b.profile) && identifier(&b.account_id) && identifier(&b.generation)
}
#[cfg(unix)]
pub(crate) fn valid_capture(c: &Capture) -> bool {
    valid_binding(&c.binding)
        && identifier(&c.observation_id)
        && c.ticket > 0
        && (0..=MAX_TIME).contains(&c.captured_at)
}
