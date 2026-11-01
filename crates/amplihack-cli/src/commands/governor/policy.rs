//! The decision function: pure, with no I/O and no environment. The steps
//! are in docs/reference/capacity-governor.md#model:
//!
//! 1. No eligible reading: admit (`no-readings`).
//! 2. Every eligible profile weekly-exhausted: wait for the earliest weekly
//!    reset, or deny when it falls after the deadline (`weekly-exhausted`).
//!    This is the only step that denies.
//! 3. No profile with session room: wait for the earliest session reset
//!    (`session-exhausted`).
//! 4. Ordinary work and no profile above the reserve: wait (`reserve`).
//! 5. Top-level slots at the limit: wait (`concurrency`).
//! 6. An earlier request in the fair queue: wait (`queue`).
//! 7. Admit (`room`); a nested request is admitted after step 4 (`nested`).
//!
//! Rate feedback runs first, at most once per evaluation interval, and sets
//! the limit step 5 uses. Profiles are sorted before use, so the order of
//! `Inputs::budgets` never changes the decision.

use super::budget::{Headroom, headroom, usable_points};
use super::model::{Budget, Decision, DecisionKind, Inputs, Reason, ReasonCode};
use super::queue::{head, key_for, order_key};
use super::rate::next_limit;

fn decision(
    kind: DecisionKind,
    code: ReasonCode,
    message: String,
    limit: u32,
    evaluated: bool,
) -> Decision {
    Decision {
        kind,
        reason: Reason { code, message },
        limit_after: limit,
        evaluated,
        retry_after: None,
        announce: false,
    }
}

/// A Unix time as `YYYY-MM-DD HH:MM UTC`.
fn utc(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(
        || format!("{t} (unix time)"),
        |d| d.format("%Y-%m-%d %H:%M UTC").to_string(),
    )
}

fn earliest(resets: impl Iterator<Item = Option<i64>>) -> Option<i64> {
    resets.flatten().min()
}

pub fn decide(i: &Inputs, now: i64) -> Decision {
    let s = &i.settings;
    let max = s.max_concurrency.max(1);
    let interval = i64::try_from(s.eval_interval_secs).unwrap_or(i64::MAX);
    let evaluated = i
        .last_evaluated_at
        .is_none_or(|t| now.saturating_sub(t) >= interval);
    let limit = if evaluated {
        next_limit(i.limit, i.signal, s.step, max)
    } else {
        i.limit.clamp(1, max)
    };
    let made = |kind, code, message: String| decision(kind, code, message, limit, evaluated);

    // Active profile first, then by name: a total order.
    let mut pool: Vec<&Budget> = i.budgets.iter().filter(|b| b.eligible).collect();
    pool.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.name.cmp(&b.name)));

    // Step 1.
    if pool.is_empty() {
        return made(
            DecisionKind::Admit,
            ReasonCode::NoReadings,
            "no usage reading for any eligible profile; admitted without a budget check".into(),
        );
    }

    // Step 2.
    let open: Vec<&Budget> = pool
        .iter()
        .copied()
        .filter(|b| headroom(b, 0.0) != Headroom::WeeklyExhausted)
        .collect();
    if open.is_empty() {
        let reset = earliest(
            pool.iter()
                .map(|b| b.weekly.resets_at)
                .map(|r| r.filter(|r| *r > now)),
        );
        let at = reset.map_or_else(|| "an unknown time".to_string(), utc);
        let message = format!(
            "every eligible profile has used its weekly limit; the next weekly reset is at {at}"
        );
        let fits = reset.is_some_and(|r| r <= i.request.deadline) && i.request.deadline > now;
        let kind = if fits {
            DecisionKind::Wait
        } else {
            DecisionKind::Deny
        };
        let mut d = made(kind, ReasonCode::WeeklyExhausted, message);
        d.retry_after = reset;
        return d;
    }

    // Step 3.
    let with_session: Vec<&Budget> = open
        .iter()
        .copied()
        .filter(|b| headroom(b, 0.0) != Headroom::SessionExhausted)
        .collect();
    if with_session.is_empty() {
        let reset = earliest(open.iter().map(|b| b.session.and_then(|w| w.resets_at)));
        let at = reset.map_or_else(|| "an unknown time".to_string(), utc);
        let message = format!(
            "every eligible profile has used its session limit; the next session reset is at {at}"
        );
        let mut d = made(DecisionKind::Wait, ReasonCode::SessionExhausted, message);
        d.retry_after = reset;
        return d;
    }

    // Step 4.
    let class = i.request.class;
    let reserve = if class.protected() {
        0.0
    } else {
        f64::from(s.reserve_points)
    };
    let room: Vec<&Budget> = with_session
        .iter()
        .copied()
        .filter(|b| headroom(b, reserve) == Headroom::Room)
        .collect();
    let Some(chosen) = room.iter().copied().find(|b| b.active).or_else(|| {
        room.iter().copied().max_by(|a, b| {
            usable_points(a)
                .total_cmp(&usable_points(b))
                .then_with(|| b.name.cmp(&a.name))
        })
    }) else {
        let best = with_session
            .iter()
            .map(|b| usable_points(b))
            .fold(0.0, f64::max);
        let message = format!(
            "ordinary work is held: the best usable remainder is {best:.1} points, at or below the {} point reserve",
            s.reserve_points
        );
        return made(DecisionKind::Wait, ReasonCode::Reserve, message);
    };

    if i.request.nested {
        return made(
            DecisionKind::Admit,
            ReasonCode::Nested,
            "nested launch admitted under its parent's slot".into(),
        );
    }

    // Step 5.
    let held = u32::try_from(i.slots.len()).unwrap_or(u32::MAX);
    if held >= limit {
        let message = format!("{held} of {limit} slots are in use");
        return made(DecisionKind::Wait, ReasonCode::Concurrency, message);
    }

    // Step 6.
    if let Some(first) = head(&i.queue, &i.admissions) {
        let ahead = match i.request.seq {
            Some(seq) => seq != first,
            None => {
                let mine = key_for(class, &i.request.workstream, u64::MAX, &i.admissions);
                let theirs = i
                    .queue
                    .iter()
                    .find(|e| e.seq == first)
                    .map(|e| order_key(e, &i.admissions));
                theirs.is_some_and(|t| t < mine)
            }
        };
        if ahead {
            return made(
                DecisionKind::Wait,
                ReasonCode::Queue,
                "an earlier request is first in the fair queue".into(),
            );
        }
    }

    // Step 7.
    let mut d = if chosen.active {
        made(
            DecisionKind::Admit,
            ReasonCode::Room,
            format!("profile {} has room", chosen.name),
        )
    } else {
        made(
            DecisionKind::Admit,
            ReasonCode::Room,
            format!(
                "profile {} has room; the rotator moves work there from the active profile",
                chosen.name
            ),
        )
    };
    d.announce = !chosen.active;
    d
}
