//! The record step of a measuring cycle, after the decision and any swap,
//! under the state lock and before the ledger is saved: the capacity
//! history, `no_room`, the swap-back damping and the stored failed-probe
//! count (#1880). Kept apart from `cycle.rs` to hold that file within its
//! line budget.

use super::cycle::Ctx;
use super::decide::Reading;
use super::failed_store;
use super::ledger::RotateLedger;
use super::log::Sink;
use super::observe::{Probes, Snap, live_name};
use super::overflow::Tag;
use super::overflow_record::refresh_capacity;
use super::plan_room::no_room;
use super::swap_back;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;

/// What the cycle measured and did, for [`finish`].
pub(super) struct Done<'a> {
    pub first: &'a Snap,
    /// The live reading the decision used (a plan spend limit is exhausted).
    pub live: &'a Reading,
    pub others: &'a [(ProfileName, Reading)],
    pub probed: Option<&'a Probes>,
    /// `[to, from]` of a completed swap.
    pub swapped: Option<&'a [ProfileName; 2]>,
    /// Consecutive failed live probes, this cycle's included.
    pub failed: u32,
    pub tag: Option<Tag>,
    pub measured_at: i64,
    pub t: i64,
}

/// Records `d` in `l`, which the caller then saves, and logs the
/// swap-back lines. The failed-probe count is written only here
/// (`failed_store::record`): cleared by a completed swap, else stored for
/// the live profile.
pub(super) fn finish(ctx: &Ctx, out: &mut Sink, l: &mut RotateLedger, d: Done) {
    let live_n = live_name(&d.first.live);
    refresh_capacity(
        ctx,
        live_n,
        d.live,
        d.others,
        d.t,
        d.tag,
        d.first.shown_platform(),
    );
    no_room::record(l, ctx, d.first, (d.live, d.others), d.probed, d.measured_at);
    let left = d.swapped.map(|[_, from]| (from, d.live));
    let s = &ctx.settings;
    for line in swap_back::update(l, s, live_n, d.probed, left, d.measured_at, d.t) {
        out.emit(now(), &line);
    }
    failed_store::record(l, &d.first.live, d.failed, d.swapped.is_some());
}
