//! The input of the overflow policy (`overflow::decide_overflow`), kept
//! apart from `overflow.rs` to hold that file within its line budget.

use super::super::decide::{Measured, Probed};
use super::super::ledger::OverflowBlock;
use super::super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::OverflowEntry;
use std::collections::BTreeMap;

pub struct OverflowInput<'a> {
    pub settings: &'a Settings,
    /// The profile live in `~/.claude`; `None` when no profile owns it.
    pub live_name: Option<&'a ProfileName>,
    pub live: &'a Probed,
    pub candidates: &'a [Measured],
    /// Plan logins left unmeasured this cycle because they are busy: they
    /// may have room, so they hold back the API key.
    pub unmeasured_plan: &'a [ProfileName],
    /// Profiles skipped as `directory refused` (issue #1698). Their logins
    /// cannot be checked, so a plan profile among them holds a move onto
    /// overflow, as an unknown probe does, until its directory is fixed.
    /// Overflow profiles among them are ignored.
    pub refused: &'a [ProfileName],
    /// The overflow entries, in any order: they are tried by tier, then in
    /// the order given within a tier.
    pub overflow: &'a [OverflowEntry],
    /// The ledger's spend blocks, by profile name; expired ones are ignored.
    pub blocks: &'a BTreeMap<String, OverflowBlock>,
    /// The API-key block (issue #1700); it covers the entry at
    /// [`PLATFORM_TIER`](crate::commands::profile::overflow::PLATFORM_TIER) while `now < until`.
    pub platform_block: Option<&'a OverflowBlock>,
    pub swapped_at: Option<i64>,
    pub now: i64,
    /// Consecutive failed live probes, this cycle's included (#1880): a
    /// failed live login reaches the plan-room swap only at
    /// `FAILED_PROBE_LIMIT` (`live_limit::blocks_room`).
    pub failed: u32,
}
