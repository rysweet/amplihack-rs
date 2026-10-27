//! The credit-backed overflow policy (issue #1606): a pure function of the
//! settings, the readings with their reset times, the overflow names, the
//! spend blocks and the clock. See `tests_overflow` for the contract and
//! "Rotate overflow" in docs/reference/profile-command.md.
//!
//! - With no overflow profiles the verdict is exactly `decide` (P1).
//! - A plan login has capacity when its probe read `ok` and its session and
//!   week use are below `--session-at` and `--week-at`; margins and the hold
//!   play no part.
//! - Onto credits only when the live plan login is at a threshold (and its
//!   probe did not fail), no plan login has capacity and the earliest plan
//!   reset is more than `overflow_wait` away.
//! - Off credits as soon as a plan login has capacity, ignoring the hold and
//!   both margins. Both rules use the same capacity test, so the rotator
//!   does not flap (P2).
//! - A spend-limit reading on an overflow profile blocks it, once per block.

use super::decide::{Decision, Measured, Probed, Reading, Reason, Windows, best, decide};
use super::ledger::OverflowBlock;
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;

/// What a cycle records about credits in `last_cycle.overflow`.
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

    /// The live login runs on credits after this cycle.
    pub fn on_credits(tag: Option<Self>) -> bool {
        matches!(tag, Some(Self::In | Self::On))
    }
}

pub struct OverflowInput<'a> {
    pub settings: &'a Settings,
    /// The profile live in `~/.claude`; `None` when no profile owns it.
    pub live_name: Option<&'a ProfileName>,
    pub live: &'a Probed,
    pub candidates: &'a [Measured],
    /// The overflow profiles, in config order.
    pub overflow: &'a [ProfileName],
    /// The ledger's spend blocks, by profile name; expired ones are ignored.
    pub blocks: &'a BTreeMap<String, OverflowBlock>,
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
    /// The earliest plan reset, for the `credits:` log lines; `None` when
    /// unknown or when the verdict does not use credits.
    pub next_plan_reset: Option<i64>,
}

/// A plan login with capacity: an `ok` probe below both thresholds.
pub fn plan_usable(s: &Settings, r: &Reading) -> bool {
    match r {
        Reading::Measured { session, week } => {
            *session < f64::from(s.session_at) && *week < f64::from(s.week_at)
        }
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
    let session_hit = session_used.is_some_and(|u| u >= f64::from(s.session_at));
    let week_hit = week_used.is_some_and(|u| u >= f64::from(s.week_at));
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
/// count as far away, so they never shorten the wait.
pub fn earliest_plan_reset<'a>(
    s: &Settings,
    pool: impl IntoIterator<Item = (&'a Reading, &'a Windows)>,
) -> Option<i64> {
    pool.into_iter().filter_map(|(r, w)| regains(s, r, w)).min()
}

/// Whether the earliest plan reset is more than `wait` seconds after `now`.
/// `None` (unknown) is far away.
pub fn worth_overflow(earliest: Option<i64>, now: i64, wait: u64) -> bool {
    earliest.is_none_or(|t| t.saturating_sub(now) > i64::try_from(wait).unwrap_or(i64::MAX))
}

fn active(blocks: &BTreeMap<String, OverflowBlock>, name: &ProfileName, now: i64) -> bool {
    blocks.get(name.as_str()).is_some_and(|b| now < b.until)
}

pub fn decide_overflow(i: &OverflowInput<'_>) -> Verdict {
    let s = i.settings;
    let is_overflow = |n: &ProfileName| i.overflow.contains(n);
    let live_over = i.live_name.is_some_and(is_overflow);

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
    for o in i.overflow {
        if Some(o) == i.live_name || active(i.blocks, o, i.now) {
            continue;
        }
        if i.candidates.iter().any(|c| &c.name == o && c.spend_limit) {
            new_blocks.push(o.clone());
        }
    }
    let blocked = |n: &ProfileName| active(i.blocks, n, i.now) || new_blocks.contains(n);
    // The first overflow profile in config order that may be moved to:
    // a candidate, not live, not blocked, and its probe did not fail.
    let next_overflow = || {
        i.overflow
            .iter()
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
    let verdict = |decision, tag, next_plan_reset| Verdict {
        decision,
        tag,
        new_blocks: new_blocks.clone(),
        next_plan_reset,
    };
    let swap = |to: ProfileName, reason| Decision::Swap { to, reason };

    if !live_over {
        let d = decide(s, &i.live.reading, &plan, i.swapped_at, i.now);
        // Credits only replace today's no-capacity outcome, and never for a
        // login whose state is unknown.
        if i.overflow.is_empty() || d != Decision::Alert || i.live.reading == Reading::Failed {
            return verdict(d, None, None);
        }
        if plan.iter().any(|c| plan_usable(s, &c.reading)) {
            return verdict(d, None, None);
        }
        let pool = std::iter::once((&i.live.reading, &i.live.windows))
            .chain(plan.iter().map(|c| (&c.reading, &c.windows)));
        let earliest = earliest_plan_reset(s, pool);
        if !worth_overflow(earliest, i.now, s.overflow_wait) {
            return verdict(d, None, None);
        }
        return match next_overflow() {
            Some(to) => verdict(swap(to, Reason::OverflowIn), Some(Tag::In), earliest),
            None => verdict(d, None, None),
        };
    }

    // On credits: leave as soon as a plan login has capacity.
    let with_capacity: Vec<Measured> = plan
        .iter()
        .filter(|c| plan_usable(s, &c.reading))
        .cloned()
        .collect();
    if let Some((to, _)) = best(&with_capacity) {
        return verdict(swap(to.clone(), Reason::OverflowOut), Some(Tag::Out), None);
    }
    let earliest = earliest_plan_reset(s, plan.iter().map(|c| (&c.reading, &c.windows)));
    if i.live_name.is_some_and(blocked) {
        // The live overflow cannot spend: the next overflow profile if the
        // wait still holds, else the best plan login, else the ALERT.
        if worth_overflow(earliest, i.now, s.overflow_wait)
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
    // the only targets (`damp` then applies); otherwise stay on credits.
    if i.live.reading == Reading::Failed
        && let d @ Decision::Swap { .. } = decide(s, &i.live.reading, &plan, i.swapped_at, i.now)
    {
        return verdict(d, Some(Tag::Out), None);
    }
    verdict(Decision::Stay, Some(Tag::On), earliest)
}
