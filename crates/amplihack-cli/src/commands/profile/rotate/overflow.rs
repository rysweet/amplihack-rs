//! The rotator's overflow policy (issue #1606): a pure function of the
//! settings, the readings with their reset times, the overflow entries, the
//! spend blocks and the clock. See `tests_overflow` for the contract and
//! "Rotate overflow" in docs/reference/profile-command.md.
//!
//! - With no overflow profiles the verdict is exactly `decide` (P1).
//! - A plan login has capacity when its probe read `ok` and its session and
//!   week use pass [`Gate::plan_capacity`] (below `--session-at` and
//!   `--week-at`); margins and the hold play no part. Auto-switch applies the
//!   same test with the same thresholds, read from the running rotator, for
//!   moving onto and off overflow whatever its own threshold
//!   (`candidate::may_return_to`; `tests_gate_record` checks thresholds 70,
//!   80 and 90 against 85/90).
//! - Onto overflow only when the live plan login is at a threshold (and its
//!   probe did not fail), no plan candidate's probe is unknown (it timed out
//!   or could not run, issue #1698), no plan profile is skipped as
//!   `directory refused`, and the shared [`Gate`] allows it: no plan login
//!   has capacity and the earliest plan reset is more than `overflow_wait`
//!   away. An unknown reset allows it only with `--overflow-unknown-reset 1`.
//!   This replaces the ALERT, and also a threshold swap onto a plan login
//!   that is itself at a threshold; that swap still happens when the gate
//!   does not allow overflow. Auto-switch decides in the same order.
//! - The overflow profile chosen is the first usable one by tier, then by
//!   file order within a tier.
//! - Off overflow as soon as a plan login has capacity, ignoring the hold
//!   (`--hold`, the minimum time after a rotator swap) and both margins.
//!   Both rules use the same capacity test, so the rotator does not flap
//!   (P2).
//! - A `profile switch` pin (`rotate::pin`, #1691) is a different thing from
//!   the hold, and it wins over every rule here: the cycle stops on the pin
//!   before measuring, so this function never runs while one is in force.
//!   A held cycle still probes a pinned live overflow profile for the spend
//!   limit (`held::watch_pinned_spend`), and `profile switch` refuses one
//!   with an active block. See `tests_pin_overflow` and "Pin and overflow"
//!   in docs/reference/profile-command.md.
//! - A spend-limit reading on an overflow profile blocks it, once per block.
//! - Issue #1700: the API-key profile is the entry at
//!   [`PLATFORM_TIER`], after every credit tier, and the API-key block
//!   covers it. Off the API key, every cycle and ignoring the hold: a plan
//!   login with capacity, else a usable credit profile; blocked, the best
//!   usable plan login, else an ALERT that stays (no swap, no loop).

use super::decide::{Decision, Measured, Probed, Reading, Reason, Windows, best, decide};
use super::ledger::OverflowBlock;
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{Gate, OverflowEntry, PLATFORM_TIER};
use std::collections::BTreeMap;

/// What a cycle records about overflow in `last_cycle.overflow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// Moved onto an overflow profile.
    In,
    /// Moved off an overflow profile.
    Out,
    /// Stayed on an overflow profile.
    On,
}

impl Tag {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
            Self::On => "on",
        }
    }

    /// The live login runs on an overflow profile after this cycle.
    pub fn on_overflow(tag: Option<Self>) -> bool {
        matches!(tag, Some(Self::In | Self::On))
    }
}

pub struct OverflowInput<'a> {
    pub settings: &'a Settings,
    /// The profile live in `~/.claude`; `None` when no profile owns it.
    pub live_name: Option<&'a ProfileName>,
    pub live: &'a Probed,
    pub candidates: &'a [Measured],
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
    /// [`PLATFORM_TIER`] while `now < until`.
    pub platform_block: Option<&'a OverflowBlock>,
    pub swapped_at: Option<i64>,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub decision: Decision,
    pub tag: Option<Tag>,
    /// Overflow profiles that reported a spend limit and have no active
    /// block yet: each is blocked, with one ALERT, by the cycle.
    pub new_blocks: Vec<ProfileName>,
    /// The earliest plan reset, for the `overflow:` log lines; `None` when
    /// unknown or when the verdict does not use overflow.
    pub next_plan_reset: Option<i64>,
}

/// A plan login with capacity: an `ok` probe that passes
/// [`Gate::plan_capacity`], the test auto-switch also applies.
pub fn plan_usable(s: &Settings, r: &Reading) -> bool {
    match r {
        Reading::Measured { session, week } => gate(s).plan_capacity(*session, *week),
        Reading::Exhausted | Reading::Failed => false,
    }
}

/// When a plan login without capacity has it again: the reset of the window
/// at its threshold, or the later of the two when both are. `None` when
/// unknown, including every failed probe.
pub fn regains(s: &Settings, r: &Reading, w: &Windows) -> Option<i64> {
    let (session_used, week_used) = match r {
        Reading::Failed => return None,
        Reading::Measured { session, week } => (Some(*session), Some(*week)),
        Reading::Exhausted => (w.session.map(|x| x.used), w.week.map(|x| x.used)),
    };
    let g = gate(s);
    let session_hit = session_used.is_some_and(|u| g.at_threshold(u, 0.0).0);
    let week_hit = week_used.is_some_and(|u| g.at_threshold(0.0, u).1);
    let session_reset = w.session.and_then(|x| x.resets_at);
    let week_reset = w.week.and_then(|x| x.resets_at);
    match (session_hit, week_hit) {
        (true, false) => session_reset,
        (false, true) => week_reset,
        (true, true) => Some(session_reset?.max(week_reset?)),
        (false, false) => None,
    }
}

/// The earliest known time any of `pool` regains capacity. Unknown resets
/// are left out, so they never shorten the wait; `None` when no reset in
/// `pool` is known, which [`Gate::far`] treats as far only by opt-in.
pub fn earliest_plan_reset<'a>(
    s: &Settings,
    pool: impl IntoIterator<Item = (&'a Reading, &'a Windows)>,
) -> Option<i64> {
    pool.into_iter().filter_map(|(r, w)| regains(s, r, w)).min()
}

/// The rotator's [`Gate`]: `--overflow-wait`, `--overflow-unknown-reset`,
/// `--session-at` and `--week-at`. The running rotator records it for
/// auto-switch (`gate_record`).
pub fn gate(s: &Settings) -> Gate {
    Gate {
        wait: s.overflow_wait,
        unknown_reset: s.overflow_unknown_reset,
        session_at: s.session_at,
        week_at: s.week_at,
    }
}

fn active(blocks: &BTreeMap<String, OverflowBlock>, name: &ProfileName, now: i64) -> bool {
    blocks.get(name.as_str()).is_some_and(|b| now < b.until)
}

pub fn decide_overflow(i: &OverflowInput<'_>) -> Verdict {
    let s = i.settings;
    let gate = gate(s);
    // Tried by tier; a stable sort keeps the given order within a tier.
    let mut order: Vec<&OverflowEntry> = i.overflow.iter().collect();
    order.sort_by_key(|e| e.tier);
    let platform = order
        .iter()
        .find(|e| e.tier == PLATFORM_TIER)
        .map(|e| &e.name);
    let order: Vec<&ProfileName> = order.into_iter().map(|e| &e.name).collect();
    let is_overflow = |n: &ProfileName| order.contains(&n);
    let live_over = i.live_name.is_some_and(is_overflow);
    let platform_blocked = i.platform_block.is_some_and(|b| i.now < b.until);
    let live_platform = platform.is_some() && i.live_name == platform;

    // Spend-limit evidence on any overflow profile, live or candidate,
    // blocks it once.
    let mut new_blocks = Vec::new();
    if let Some(live) = i.live_name
        && live_over
        && i.live.spend_limit
        && !active(i.blocks, live, i.now)
    {
        new_blocks.push(live.clone());
    }
    for &o in &order {
        if Some(o) == i.live_name || active(i.blocks, o, i.now) {
            continue;
        }
        if i.candidates.iter().any(|c| &c.name == o && c.spend_limit) {
            new_blocks.push(o.clone());
        }
    }
    let blocked = |n: &ProfileName| {
        active(i.blocks, n, i.now)
            || new_blocks.contains(n)
            || (platform_blocked && Some(n) == platform)
    };
    // The first overflow profile by tier, then given order, that may be
    // moved to: a candidate, not live, not blocked, its probe did not fail.
    // `credit_only` leaves the API-key profile out.
    let next_in = |credit_only: bool| {
        order
            .iter()
            .copied()
            .filter(|o| !(credit_only && Some(*o) == platform))
            .filter(|o| Some(*o) != i.live_name && !blocked(o))
            .find_map(|o| {
                i.candidates
                    .iter()
                    .find(|c| &c.name == o && c.reading != Reading::Failed && !c.spend_limit)
            })
            .map(|c| c.name.clone())
    };
    let next_overflow = || next_in(false);
    let plan: Vec<Measured> = i
        .candidates
        .iter()
        .filter(|c| !is_overflow(&c.name))
        .cloned()
        .collect();
    let verdict = |decision, tag, next_plan_reset| Verdict {
        decision,
        tag,
        new_blocks: new_blocks.clone(),
        next_plan_reset,
    };
    let swap = |to: ProfileName, reason| Decision::Swap { to, reason };

    if !live_over {
        let d = decide(s, &i.live.reading, &plan, i.swapped_at, i.now);
        // Overflow only replaces a move off a live login at a threshold (an
        // ALERT, or a threshold swap onto a plan login that is itself
        // without capacity), and never for a login whose state is unknown.
        // A threshold swap onto a plan login with capacity stands. This is
        // auto-switch's order too: plan capacity, then overflow, then a plan
        // login without capacity.
        let moving_without_capacity = match &d {
            Decision::Alert => true,
            Decision::Swap {
                to,
                reason: Reason::Threshold,
            } => !plan
                .iter()
                .any(|c| &c.name == to && plan_usable(s, &c.reading)),
            _ => false,
        };
        // A failed live probe holds the move onto credits, and so does an
        // unknown plan probe (a timeout, a `claude` that could not be
        // started, read or waited for, or a directory refused just before
        // the spawn) or a plan profile skipped as `directory refused`. That
        // login might have capacity. A plan login that read `not_logged_in`,
        // `unparsed`, `spend_limit`, or `ok` with no week figure, does not.
        if i.overflow.is_empty()
            || !moving_without_capacity
            || i.live.reading == Reading::Failed
            || plan.iter().any(|c| c.unknown)
            || i.refused.iter().any(|n| !is_overflow(n))
        {
            return verdict(d, None, None);
        }
        let plan_has_capacity = plan.iter().any(|c| plan_usable(s, &c.reading));
        let pool = std::iter::once((&i.live.reading, &i.live.windows))
            .chain(plan.iter().map(|c| (&c.reading, &c.windows)));
        let earliest = earliest_plan_reset(s, pool);
        if !gate.allows(plan_has_capacity, earliest, i.now) {
            return verdict(d, None, None);
        }
        return match next_overflow() {
            Some(to) => verdict(swap(to, Reason::OverflowIn), Some(Tag::In), earliest),
            None => verdict(d, None, None),
        };
    }

    // On overflow: leave as soon as a plan login has capacity.
    let with_capacity: Vec<Measured> = plan
        .iter()
        .filter(|c| plan_usable(s, &c.reading))
        .cloned()
        .collect();
    if let Some((to, _)) = best(&with_capacity) {
        return verdict(swap(to.clone(), Reason::OverflowOut), Some(Tag::Out), None);
    }
    let earliest = earliest_plan_reset(s, plan.iter().map(|c| (&c.reading, &c.windows)));
    if live_platform {
        // On the API key: credits cost less, so a usable credit profile is
        // a way off, at once. Blocked, the best usable plan login; with
        // nowhere to go, an ALERT that stays: no swap is attempted.
        if let Some(to) = next_in(true) {
            return verdict(swap(to, Reason::OverflowOut), Some(Tag::Out), None);
        }
        if platform_blocked {
            if let Some((to, _)) = best(&plan) {
                return verdict(swap(to.clone(), Reason::OverflowOut), Some(Tag::Out), None);
            }
            return verdict(Decision::Alert, Some(Tag::On), None);
        }
        return verdict(Decision::Stay, Some(Tag::On), earliest);
    }
    if i.live_name.is_some_and(blocked) {
        // The live overflow cannot spend: the next overflow profile if the
        // wait still holds, else the best plan login, else the ALERT.
        if gate.far(earliest, i.now)
            && let Some(to) = next_overflow()
        {
            return verdict(swap(to, Reason::OverflowIn), Some(Tag::In), earliest);
        }
        if let Some((to, _)) = best(&plan) {
            return verdict(swap(to.clone(), Reason::OverflowOut), Some(Tag::Out), None);
        }
        return verdict(Decision::Alert, None, None);
    }
    // A failed live probe: the failed-probe rules with plan candidates as
    // the only targets (`damp` then applies); otherwise stay on overflow.
    if i.live.reading == Reading::Failed
        && let d @ Decision::Swap { .. } = decide(s, &i.live.reading, &plan, i.swapped_at, i.now)
    {
        return verdict(d, Some(Tag::Out), None);
    }
    verdict(Decision::Stay, Some(Tag::On), earliest)
}
