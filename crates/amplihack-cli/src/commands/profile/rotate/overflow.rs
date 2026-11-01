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
//!   probe did not fail), no plan login, live included, has room
//!   ([`plan_room`]), no plan candidate's probe is unknown (it timed out or
//!   could not run, issue #1698), no plan profile is skipped as
//!   `directory refused`, and the shared [`Gate`] allows it: the earliest
//!   plan reset is more than `overflow_wait` away. An unknown reset allows
//!   it only with `--overflow-unknown-reset 1`. Auto-switch decides in the
//!   same order.
//! - The overflow profile chosen is the first usable one by tier, then by
//!   file order within a tier.
//! - Off overflow as soon as a plan login has capacity, then as soon as one
//!   has room, ignoring the hold (`--hold`, the minimum time after a rotator
//!   swap) and both margins (P2).
//! - A `profile switch` pin (`rotate::pin`, #1691) is a different thing from
//!   the hold, and it wins over every rule here: the cycle stops on the pin
//!   before measuring, so this function never runs while one is in force.
//!   A held cycle still probes a pinned live overflow profile for the spend
//!   limit (`held::watch_pinned_spend`), and `profile switch` refuses one
//!   with an active block. See `tests_pin_overflow` and "Pin and overflow"
//!   in docs/reference/profile-command.md.
//! - A spend-limit reading on an overflow profile blocks it, once per block.
//! - Issue #1700: the API-key profile is the entry at [`PLATFORM_TIER`],
//!   after every credit tier, and the API-key block covers it. Off the API
//!   key with no plan room, every cycle and ignoring the hold: a usable
//!   credit profile; blocked, the best usable plan login, else an ALERT that
//!   stays (no swap, no loop).
//! - Issue #1804: the API-key entry exists only when `overflow=` marks it,
//!   and every per-token profile waits for plan logins. Off a plan login
//!   the order is `decide`, the plan-room swap, then the marked per-token
//!   profiles by tier, then the ALERT. A threshold swap onto a plan login
//!   without capacity goes where [`plan_room::choose`] ranks first, not to
//!   the most headroom. When the live login ranks first it is an ALERT, so
//!   two logins with the same week are not swapped back and forth.
//! - "No plan login has room" covers measured logins only. A plan login that
//!   was skipped as busy ([`crate::commands::profile::rotate::observe::Snap::unmeasured_plan`])
//!   or failed its probe is unknown, not empty: while one is unknown the
//!   API-key tier is not used and [`Verdict::key_held`] names them for the
//!   ALERT. Credit tiers do not wait for a busy login or a probe that read
//!   a state such as `not_logged_in`: they have a spend limit and a marked
//!   account behind them, and the `per-token` line gives the count. Credit
//!   tiers do wait, off a plan login, for a probe that is
//!   [`Measured::unknown`] and for a plan profile skipped as
//!   `directory refused` (issue #1698): that login might have capacity. An
//!   excluded login is the operator's choice and counts as neither.
//! - A key in the `$HOME` files can make the live reading `Failed`
//!   (`key_check` says which), and then the per-token tier is not tried off
//!   a plan login. Nothing is lost: Claude is already using that key, a
//!   swap would not change the billing, and the cycle logs the ALERT.
//!   Without a home key the move fails its login check. `decide`'s swap on
//!   `Failed` is also dropped, after this function, by
//!   `damp::keep_for_home_key`. A normal live reading follows every rule.

use super::decide::{Decision, Measured, Probed, Reading, Reason, Windows, best, decide};
use super::ledger::OverflowBlock;
use super::plan_room;
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
    /// Plan logins of unknown state: unmeasured, probe failed or refused.
    pub unknown_plan: Vec<ProfileName>,
    /// [`Self::unknown_plan`] when they alone kept work off the API key, for
    /// the ALERT; otherwise empty.
    pub key_held: Vec<ProfileName>,
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
    let plan: Vec<Measured> = i
        .candidates
        .iter()
        .filter(|c| !is_overflow(&c.name))
        .cloned()
        .collect();
    let unknown_plan: Vec<ProfileName> = i
        .unmeasured_plan
        .iter()
        .cloned()
        .chain(
            plan.iter()
                .filter(|c| c.reading == Reading::Failed)
                .map(|c| c.name.clone()),
        )
        .chain(i.refused.iter().filter(|n| !is_overflow(n)).cloned())
        .collect();
    let verdict = |decision, tag, next_plan_reset| Verdict {
        decision,
        tag,
        new_blocks: new_blocks.clone(),
        next_plan_reset,
        unknown_plan: unknown_plan.clone(),
        key_held: Vec::new(),
    };
    // The next per-token profile; the API key only with every plan login known.
    let next_paid = || next_in(!unknown_plan.is_empty());
    // No per-token move: `d`, naming the unknown logins if only they stopped it.
    let no_move = |d, earliest: Option<i64>| {
        let held = gate.far(earliest, i.now) && next_in(true).is_none() && next_in(false).is_some();
        Verdict {
            key_held: if held {
                unknown_plan.clone()
            } else {
                Vec::new()
            },
            ..verdict(d, None, None)
        }
    };
    let swap = |to: ProfileName, reason| Decision::Swap { to, reason };

    if !live_over {
        let d = decide(s, &i.live.reading, &plan, i.swapped_at, i.now);
        // Everything below replaces only a move off a live login at a
        // threshold (an ALERT, or a threshold swap onto a plan login that is
        // itself without capacity), and never for a login whose state is
        // unknown. A threshold swap onto a plan login with capacity stands.
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
        if !moving_without_capacity || i.live.reading == Reading::Failed {
            return verdict(d, None, None);
        }
        // The plan login with the most room costs nothing.
        let live_measured = i.live_name.map(|n| Measured::new(n.clone(), i.live));
        let choice = plan_room::choose(s, live_measured.as_ref(), &plan);
        if let Some(to) = choice {
            let same = matches!(&d, Decision::Swap { to: t, .. } if *t == to);
            let d = if same { d } else { swap(to, Reason::PlanRoom) };
            return verdict(d, None, None);
        }
        // Per-token profiles only when no measured plan login, live
        // included, has room. The API key also needs every one measured.
        let room = std::iter::once(&i.live.reading)
            .chain(plan.iter().map(|c| &c.reading))
            .any(plan_room::has_room);
        if room {
            return verdict(Decision::Alert, None, None);
        }
        // An unknown plan probe (timed out, or `claude` could not run) or a
        // plan profile skipped as `directory refused` might have capacity,
        // so it holds every per-token move (issue #1698). A login that read
        // `not_logged_in`, `unparsed`, `spend_limit`, or `ok` with no week
        // figure does not.
        let held = plan.iter().any(|c| c.unknown) || i.refused.iter().any(|n| !is_overflow(n));
        let pool = std::iter::once((&i.live.reading, &i.live.windows))
            .chain(plan.iter().map(|c| (&c.reading, &c.windows)));
        let earliest = earliest_plan_reset(s, pool);
        if !held
            && gate.far(earliest, i.now)
            && let Some(to) = next_paid()
        {
            return verdict(swap(to, Reason::OverflowIn), Some(Tag::In), earliest);
        }
        return no_move(d, earliest);
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
    // A plan login below 100% costs nothing (issue #1804).
    if let Some(to) = plan_room::choose(s, None, &plan) {
        return verdict(swap(to, Reason::OverflowOut), Some(Tag::Out), None);
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
        // The live overflow cannot spend, and no measured plan login has room: the
        // next per-token profile by tier if the wait still holds, else the
        // best plan login, else the ALERT.
        if gate.far(earliest, i.now)
            && let Some(to) = next_paid()
        {
            return verdict(swap(to, Reason::OverflowIn), Some(Tag::In), earliest);
        }
        if let Some((to, _)) = best(&plan) {
            return verdict(swap(to.clone(), Reason::OverflowOut), Some(Tag::Out), None);
        }
        return no_move(Decision::Alert, earliest);
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
