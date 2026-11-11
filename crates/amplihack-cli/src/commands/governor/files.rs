//! The three state files of `.governor.d` (D13) and the checks run on every
//! load. Each file has a `version`; a file whose fields or values fail a
//! check is corrupt. Holds profile names and workstream ids only.

use super::identity::parse_workstream;
use super::model::{Admission, FailOpenRecord, QueueEntry, Sample, Slot};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const GOVERNOR_STATE_VERSION: u32 = 1;
/// The highest concurrency limit any file may hold.
pub const GOVERNOR_LIMIT_CAP: u32 = 64;
/// The most samples kept per profile and window.
pub const GOVERNOR_HISTORY_RING: usize = 64;
/// A start waits only when the queue is shorter than this.
pub const GOVERNOR_QUEUE_CAP: usize = 4096;
const MAX_TREE_ID: usize = 128;
/// Timestamps may be at most this far in the future.
const FUTURE_SLACK_SECS: i64 = 400 * 86_400;

fn version() -> u32 {
    GOVERNOR_STATE_VERSION
}

/// `state.json`. `limit` 0 means "not yet set": the first admission sets it
/// to `governor.max_concurrency`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub version: u32,
    pub limit: u32,
    pub last_evaluated_at: Option<i64>,
    pub next_seq: u64,
    pub slots: Vec<Slot>,
    pub admissions: Vec<Admission>,
    pub fail_open: Option<FailOpenRecord>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: version(),
            limit: 0,
            last_evaluated_at: None,
            next_seq: 1,
            slots: Vec::new(),
            admissions: Vec::new(),
            fail_open: None,
        }
    }
}

/// `queue.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Queue {
    pub version: u32,
    pub entries: Vec<QueueEntry>,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            version: version(),
            entries: Vec::new(),
        }
    }
}

/// `history.json`: the governor's own record of readings, per profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct History {
    pub version: u32,
    pub weekly: BTreeMap<String, Vec<Sample>>,
    pub session: BTreeMap<String, Vec<Sample>>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            version: version(),
            weekly: BTreeMap::new(),
            session: BTreeMap::new(),
        }
    }
}

/// A state file: its fixed name and its checks.
pub trait Persisted: Serialize + DeserializeOwned + Default {
    const FILE: &'static str;
    /// The quarantine name.
    const CORRUPT: &'static str;
    fn version(&self) -> u32;
    fn validate(&self, now: i64) -> Result<(), String>;
}

fn time_ok(t: i64, now: i64) -> bool {
    t >= 0 && t <= now.saturating_add(FUTURE_SLACK_SECS)
}

fn workstream_ok(ws: &str) -> bool {
    parse_workstream(ws).is_ok()
}

pub fn tree_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_TREE_ID
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("invalid {what}"))
    }
}

impl Persisted for State {
    const FILE: &'static str = "state.json";
    const CORRUPT: &'static str = "state.json.corrupt";
    fn version(&self) -> u32 {
        self.version
    }
    fn validate(&self, now: i64) -> Result<(), String> {
        check(self.limit <= GOVERNOR_LIMIT_CAP, "limit")?;
        check(
            self.last_evaluated_at.is_none_or(|t| time_ok(t, now)),
            "last_evaluated_at",
        )?;
        let mut keys = BTreeSet::new();
        for s in &self.slots {
            check(
                workstream_ok(&s.workstream) && tree_id_ok(&s.tree_id),
                "slot",
            )?;
            check(time_ok(s.admitted_at, now), "slot time")?;
            check(keys.insert((s.pid, s.tree_id.as_str())), "duplicate slot")?;
        }
        for a in &self.admissions {
            check(
                workstream_ok(&a.workstream) && time_ok(a.at, now),
                "admission",
            )?;
        }
        if let Some(f) = &self.fail_open {
            check(time_ok(f.at, now) && f.message.len() <= 512, "fail_open")?;
        }
        Ok(())
    }
}

impl Persisted for Queue {
    const FILE: &'static str = "queue.json";
    const CORRUPT: &'static str = "queue.json.corrupt";
    fn version(&self) -> u32 {
        self.version
    }
    fn validate(&self, now: i64) -> Result<(), String> {
        let mut seqs = BTreeSet::new();
        for e in &self.entries {
            check(
                workstream_ok(&e.workstream) && time_ok(e.enqueued_at, now),
                "queue entry",
            )?;
            check(seqs.insert(e.seq), "duplicate seq")?;
        }
        Ok(())
    }
}

impl Persisted for History {
    const FILE: &'static str = "history.json";
    const CORRUPT: &'static str = "history.json.corrupt";
    fn version(&self) -> u32 {
        self.version
    }
    fn validate(&self, now: i64) -> Result<(), String> {
        for rings in [&self.weekly, &self.session] {
            for (name, ring) in rings {
                check(
                    name.len() <= 64 && ring.len() <= GOVERNOR_HISTORY_RING,
                    "history ring",
                )?;
                for s in ring {
                    let pct = s.used_pct.is_finite() && (0.0..=100.0).contains(&s.used_pct);
                    check(pct && time_ok(s.observed_at, now), "sample")?;
                }
            }
        }
        Ok(())
    }
}

/// Moves `next_seq` past every queued seq and clamps `limit` to `1..=max`.
pub fn repair(state: &mut State, queue: &Queue, max: u32) {
    let past = queue
        .entries
        .iter()
        .map(|e| e.seq.saturating_add(1))
        .max()
        .unwrap_or(1);
    state.next_seq = state.next_seq.max(past).max(1);
    state.limit = state.limit.clamp(1, max.clamp(1, GOVERNOR_LIMIT_CAP));
}

/// Appends a sample to a ring, skipping one already recorded at the same
/// time and keeping the newest [`GOVERNOR_HISTORY_RING`].
pub fn push_sample(ring: &mut Vec<Sample>, sample: Sample) {
    if ring.iter().any(|s| s.observed_at == sample.observed_at) {
        return;
    }
    ring.push(sample);
    ring.sort_by_key(|s| s.observed_at);
    if ring.len() > GOVERNOR_HISTORY_RING {
        let excess = ring.len() - GOVERNOR_HISTORY_RING;
        ring.drain(..excess);
    }
}
