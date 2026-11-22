//! What a cycle writes down once its decision is made and any swap has
//! happened: the shared capacity file, the `no_room` entry, the swap-back
//! journal, the ledger's own `last_cycle` and the swap record the
//! limit-resume hook waits for. `cycle.rs` owns the steps that produce it;
//! see "Rotate cycle" in docs/reference/profile-command.md.

use super::super::capacity;
use super::super::decide::Reading;
use super::super::ledger::RotateLedger;
use super::super::log::Sink;
use super::super::observe::{Probes, Snap, live_name};
use super::super::overflow::Tag;
use super::super::overflow_record::{self, refresh_capacity};
use super::super::plan_room::{self, no_room};
use super::super::swap_back;
use super::super::swap_record;
use super::{Ctx, CycleOutcome, save};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;

/// Everything a finished cycle measured, decided and did, in the one place
/// where all of it is in hand.
pub(super) struct Done<'a> {
    /// The cycle's verdict, as it is written to `last_cycle.outcome`.
    pub outcome: CycleOutcome,
    /// The snapshot the cycle decided from, taken under the first lock.
    pub first: &'a Snap,
    /// The live login's reading, and the other logins' by name.
    pub live: &'a Reading,
    pub others: &'a [(ProfileName, Reading)],
    /// The probes the readings came from; `None` from a stubbed measure.
    pub probed: Option<&'a Probes>,
    /// When measuring started, epoch seconds.
    pub measured_at: i64,
    /// When the cycle's decision was taken, epoch seconds.
    pub t: i64,
    /// The cycle's overflow tag, after any refusal cleared it.
    pub tag: Option<Tag>,
    /// `[to, from]` when the cycle swapped, in that order.
    pub swapped: Option<&'a [ProfileName; 2]>,
    /// When the swap finished; `t` when none did (issue #2085).
    pub swapped_at: i64,
}

impl Done<'_> {
    /// The login that is live after this cycle has room, as the rotator
    /// itself reads it (issue #2857).
    ///
    /// This is the one point where the login the cycle leaves in place and
    /// every reading it took are both in hand. The bit is the rotator's own
    /// answer to the question a waiting step asks (may a retry use this
    /// login), decided by [`plan_room::has_room_under`]: both windows below
    /// 100% and session use below `--session-at`. Since issue #2910 the
    /// target rule (`target::fit`) may move a live login with no room onto a
    /// login at or above `--session-at`, so after such a move the bit is
    /// `false` and a waiting step is not woken until a cycle reads the new
    /// login below `--session-at`. `None` for a cycle that measured nothing
    /// about that login, which every reader takes as no room.
    fn live_room(&self, session_at: u8) -> Option<bool> {
        let to = self.swapped.map(|[to, _]| to);
        let after = overflow_record::live_reading(to, self.live, self.others);
        after.map(|r| plan_room::has_room_under(r, session_at))
    }
}

/// Writes the cycle's record, under the lock the caller still holds.
pub(super) fn record(
    ctx: &mut Ctx,
    out: &mut Sink,
    l: &mut RotateLedger,
    d: &Done<'_>,
) -> Result<(), ProfileError> {
    // Issue #2218: nothing measured is recorded under an excluded live
    // login's name; it was never probed.
    let recorded_live = live_name(&d.first.live).filter(|_| !d.first.live_excluded);
    let unreadable = refresh_capacity(
        ctx,
        recorded_live,
        d.live,
        d.others,
        d.t,
        d.tag,
        d.first.shown_platform(),
    );
    // Issue #2885: said once, not left as a silent skip.
    ctx.note_keychain(out, d.t, unreadable);
    no_room::record(l, ctx, d.first, (d.live, d.others), d.probed, d.measured_at);
    let left = d
        .swapped
        .filter(|_| !d.first.live_excluded)
        .map(|[_, from]| (from, d.live));
    let (s, m, t) = (&ctx.settings, d.measured_at, d.t);
    for line in swap_back::update(l, s, recorded_live, d.probed, left, m, t) {
        out.emit(now(), &line);
    }
    let at = capacity::recorded_at(d.outcome, d.probed, d.measured_at);
    let live_room = d.live_room(ctx.settings.session_at);
    save(ctx, l, d.outcome, now(), Some(at), d.tag, live_room)?;
    // Issue #2085: the record the limit-resume hook waits for, under the lock.
    if let Some([to, from]) = d.swapped {
        swap_record::after_swap(
            &ctx.rs,
            out,
            from,
            to,
            d.others,
            d.measured_at,
            d.swapped_at,
            (d.first.source_org(), d.first.org_of(to)),
        );
    }
    Ok(())
}
