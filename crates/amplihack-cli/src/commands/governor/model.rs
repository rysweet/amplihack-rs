//! The governor's plain data types. Budgets are in percentage points of a
//! usage window. Every type that is written to `.governor.d` rejects unknown
//! fields, so a file from a newer amplihack is never half-read.

use super::config::Settings;
use super::rate::RateSignal;
use serde::{Deserialize, Serialize};

/// A request's priority class, highest first. See "Reserves and classes" in
/// docs/reference/capacity-governor.md.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Coordination,
    Recovery,
    Review,
    Ordinary,
}

impl Class {
    /// The queue rank: lower is admitted first.
    pub fn rank(self) -> u8 {
        match self {
            Self::Coordination => 0,
            Self::Recovery => 1,
            Self::Review => 2,
            Self::Ordinary => 3,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Coordination => "coordination",
            Self::Recovery => "recovery",
            Self::Review => "review",
            Self::Ordinary => "ordinary",
        }
    }

    /// The exact lowercase name; anything else is `None`.
    pub fn parse(raw: &str) -> Option<Self> {
        [
            Self::Coordination,
            Self::Recovery,
            Self::Review,
            Self::Ordinary,
        ]
        .into_iter()
        .find(|c| c.as_str() == raw)
    }

    /// Classes that may spend the protected reserve.
    pub fn protected(self) -> bool {
        self != Self::Ordinary
    }
}

/// One usage window of one profile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowBudget {
    pub used_pct: f64,
    pub resets_at: Option<i64>,
}

/// One profile's budget. `session` is `None` for a weekly-only profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Budget {
    pub name: String,
    pub active: bool,
    pub eligible: bool,
    pub session: Option<WindowBudget>,
    pub weekly: WindowBudget,
}

/// A slot held by a running top-level process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub pid: u32,
    pub start_time: Option<u64>,
    pub workstream: String,
    pub class: Class,
    pub tree_id: String,
    pub admitted_at: i64,
    /// Nested launches currently running under this slot.
    pub nested_children: u32,
}

/// A waiting request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueEntry {
    pub seq: u64,
    pub workstream: String,
    pub class: Class,
    pub pid: u32,
    pub start_time: Option<u64>,
    pub enqueued_at: i64,
}

/// One admission in the fairness window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    pub workstream: String,
    pub at: i64,
}

/// One reading of one window, kept for the burn rate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub observed_at: i64,
    pub used_pct: f64,
    pub resets_at: Option<i64>,
}

/// The request being decided.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub class: Class,
    pub workstream: String,
    /// Runs under a live parent slot (D2).
    pub nested: bool,
    /// The request's queue ticket, once it has one.
    pub seq: Option<u64>,
    /// The latest time the request may still start.
    pub deadline: i64,
}

/// Everything [`super::policy::decide`] reads.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub budgets: Vec<Budget>,
    pub signal: RateSignal,
    pub limit: u32,
    pub last_evaluated_at: Option<i64>,
    pub slots: Vec<Slot>,
    pub queue: Vec<QueueEntry>,
    pub admissions: Vec<Admission>,
    pub settings: Settings,
    pub request: Request,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    Admit,
    Wait,
    Deny,
}

/// Why a decision was made. The spelling of [`ReasonCode::as_str`] is
/// stable: scripts may match it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonCode {
    NoReadings,
    Room,
    Nested,
    WeeklyExhausted,
    SessionExhausted,
    Reserve,
    Concurrency,
    Queue,
    WaitExpired,
}

impl ReasonCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoReadings => "no-readings",
            Self::Room => "room",
            Self::Nested => "nested",
            Self::WeeklyExhausted => "weekly-exhausted",
            Self::SessionExhausted => "session-exhausted",
            Self::Reserve => "reserve",
            Self::Concurrency => "concurrency",
            Self::Queue => "queue",
            Self::WaitExpired => "wait-expired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub code: ReasonCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub kind: DecisionKind,
    pub reason: Reason,
    /// The concurrency limit after this decision's rate evaluation.
    pub limit_after: u32,
    /// Rate feedback ran (at most once per interval).
    pub evaluated: bool,
    /// When a wait may end on its own (a window reset), if known.
    pub retry_after: Option<i64>,
    /// Say on stderr that the room is on a profile other than the active one.
    pub announce: bool,
}

/// Why the governor admitted without deciding (D11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailOpenCode {
    NoRoot,
    StateDir,
    LockTimeout,
    CorruptState,
    StateVersion,
    NoReadings,
    QueueFull,
}

impl FailOpenCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoRoot => "no-root",
            Self::StateDir => "state-dir",
            Self::LockTimeout => "lock-timeout",
            Self::CorruptState => "corrupt-state",
            Self::StateVersion => "state-version",
            Self::NoReadings => "no-readings",
            Self::QueueFull => "queue-full",
        }
    }
}

/// A fault that admits. The message never holds a path, an account key or
/// task text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailOpen {
    pub code: FailOpenCode,
    pub message: String,
}

impl FailOpen {
    pub fn new(code: FailOpenCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// The last fail-open admission, kept in `state.json` for `governor status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailOpenRecord {
    pub code: FailOpenCode,
    pub message: String,
    pub at: i64,
}
