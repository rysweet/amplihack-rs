//! The documents `governor status` prints (JSON schema version 1:
//! docs/reference/capacity-governor.md#json-output).

use super::model::Class;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct StatusView {
    pub version: u32,
    pub generated_at: i64,
    pub enabled: bool,
    pub limit: Option<u32>,
    pub max: u32,
    pub profiles: Vec<ProfileView>,
    pub reserve: ReserveView,
    pub slots: Vec<SlotView>,
    pub queue: Vec<QueueView>,
    pub workstreams: Vec<WorkstreamView>,
    pub fail_open_reason: Option<ReasonView>,
}

#[derive(Debug, Serialize)]
pub struct WindowView {
    pub used_pct: f64,
    pub resets_at: Option<i64>,
    pub burn_rate: Option<f64>,
    pub sustainable_rate: Option<f64>,
    pub forecast_exhaustion: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ProfileView {
    pub name: String,
    pub active: bool,
    pub eligible: bool,
    /// `false` when the reading has a week window and no session window.
    /// That is not "no session limit": the stored reading cannot tell a plan
    /// with none from a session window that was not recorded (#1818).
    pub session_window: bool,
    /// A window fails `usage::plausible` (see `budget::invalid_reading`): the
    /// profile is out of the pool and `session` and `weekly` are `null`. A
    /// percentage outside 0 to 100 is clamped, not invalid.
    pub invalid_reading: bool,
    /// The reading is older than `governor.max_reading_age_secs` (see
    /// `budget::stale_reading`). The profile stays in the pool, as
    /// exhausted, only while a window of the reading is exhausted with its
    /// reset still ahead; otherwise it is out of the pool and `session` and
    /// `weekly` are `null`.
    pub stale_reading: bool,
    /// When the reading was taken (Unix seconds); `null` without a reading.
    pub observed_at: Option<i64>,
    pub session: Option<WindowView>,
    /// `null` for a profile with no weekly reading.
    pub weekly: Option<WindowView>,
    pub burn_rate: Option<f64>,
    pub sustainable_rate: Option<f64>,
    pub forecast_exhaustion: Option<i64>,
}

/// The reserve summary. `best_weekly_remaining` is taken over the profiles
/// step 4 of `policy::decide` reads: eligible, with neither window
/// exhausted. A profile kept for a stale exhausted window is never among
/// them, so a stale reading adds no room. `ordinary_held` and `held_reason`
/// come from `policy::decide` itself (see `status::ordinary_hold`), so they
/// report a hold at steps 2 and 3 as well as at the reserve.
#[derive(Debug, Serialize)]
pub struct ReserveView {
    pub points: u32,
    pub best_weekly_remaining: Option<f64>,
    pub ordinary_held: bool,
    /// The reason code `policy::decide` gives for holding a top-level
    /// ordinary start (`weekly-exhausted`, `session-exhausted` or
    /// `reserve`); `None` when it would be admitted.
    pub held_reason: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct SlotView {
    pub pid: u32,
    pub workstream: String,
    pub class: Class,
    pub admitted_at: i64,
    pub nested_children: u32,
    pub stale: bool,
}

#[derive(Debug, Serialize)]
pub struct QueueView {
    pub seq: u64,
    pub workstream: String,
    pub class: Class,
    pub pid: u32,
    pub enqueued_at: i64,
    pub stale: bool,
}

#[derive(Debug, Serialize)]
pub struct WorkstreamView {
    pub name: String,
    pub admissions: usize,
    pub slots: usize,
    pub waiting: usize,
    pub token_share: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ReasonView {
    pub code: String,
    pub message: String,
}
