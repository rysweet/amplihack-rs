//! Issue #2910, rule 3: before a swap, one fresh reading of the chosen
//! target confirms that it has session and week room now. If it does not,
//! the next candidate is tried; if none has room, the rotator says so and
//! waits for the next event.
//!
//! On 21 November 2026 at 18:00Z the rotator on calypso swapped to nnorth,
//! which was spent within minutes, and about nine runs went into waits
//! bound to 22:22Z until the 18:10:15Z swap released them. The stored
//! reading was right when it was taken and wrong by the time it was used.
//!
//! This extends the freshness gate (`fresh::settle`) rather than adding a
//! probe path of its own: the reading is taken by the same re-read hook,
//! out of the same per-cycle budget, and the gate's own re-decision picks
//! the next candidate.
//!
//! Where no read is allowed at all, under the lock (`Reread::Never`) or with
//! no re-read hook, which only tests build, the swap is not refused for want
//! of a confirmation: refusing every swap whenever a read cannot be made
//! would turn a safety check into an outage. Under the lock the target has
//! normally been confirmed already by the cycle's own gate; a target the
//! revalidation chose in its place is swapped to on the reading the cycle
//! had, after the `revalidate:` line.
//!
//! Every such departure from rule 3 is written. The gate itself logs
//! `msg::CONFIRM_LOCKED` before it breaks under the lock, and [`read`] logs
//! `msg::CONFIRM_SKIPPED` when the cycle's re-read or time budget is spent,
//! so a swap made on a stored reading can be told from one confirmed, from
//! the log alone. The one silent case is a context with no re-read hook,
//! which only tests build: `Ctx::with_rereads` sets it on the one context
//! the daemon runs.

use super::super::cycle::Ctx;
use super::super::decide::Probed;
use super::super::log::msg;
use super::super::overflow::plan_usable;
use crate::commands::profile::name::ProfileName;
use std::time::{Duration, Instant};

/// The confirming read of `to` before a swap (issue #2910): one reading,
/// taken now, out of the same per-cycle budget as the freshness gate's
/// re-reads.
///
/// `None` means no reading was taken and the swap goes ahead on the stored
/// one. Where the budget is spent it writes a line, so the weaker guarantee
/// is visible; with no re-read hook, which only tests build, it writes
/// nothing. A reading that shows the login has no room after all becomes
/// [`Probed::no_room`], and the gate's own re-decision then picks the next
/// candidate; a read that fails is unknown, as a stale re-read's is.
pub(super) fn read(
    ctx: &Ctx,
    to: &ProfileName,
    old: &Probed,
    reads: &mut usize,
    budget: Duration,
    started: Instant,
    lines: &mut Vec<String>,
) -> Option<Probed> {
    let left = budget.saturating_sub(started.elapsed());
    let reread = ctx.reread?;
    if *reads >= super::MAX_REREADS_PER_CYCLE || left.is_zero() {
        lines.push(msg::CONFIRM_SKIPPED.to_string());
        return None;
    }
    *reads += 1;
    let p = super::fresh_probe(ctx, to, old, reread(ctx, to, left), false, true, lines);
    if p == Probed::failed() {
        return Some(p);
    }
    if !plan_usable(&ctx.settings, &p.reading) {
        lines.push(msg::no_room(to));
        return Some(Probed::no_room());
    }
    Some(p)
}
