//! The login a step moves to before it waits, and the logins whose resets
//! bound a wait (issue #2967).
//!
//! On 21 November 2026 a step waited two hours for its own login's reset
//! while `profile usage` reported four other logins usable. The candidate
//! rule alone decided the move, and its threshold (90% by default) is lower
//! than the "usable now" bound (95% by default): a login between the two was
//! refused, though it could run the step.
//!
//! [`limit_candidate`] therefore applies two rules, in order, before any
//! wait of any kind:
//!
//! 1. the candidate rule, unchanged (`rerun::candidate`);
//! 2. when that yields nothing, the usable rule: the `profile usage` test
//!    ([`candidate::is_usable`]) over the plan logins the step may move to,
//!    ranked as the candidate rule ranks.
//!
//! Both run under one eligibility set-up (`rerun::with_choice`): not
//! excluded, not leased, not hosting another account, holding a login of its
//! own and not in the caller's skip list. Rule 2 adds nothing to who may be
//! chosen; it only reads the bound differently. It never takes an overflow
//! or API-key profile, so it never spends money, and it never takes a login
//! with no reading.
//!
//! [`cross_login_reset`] answers the question that follows when both rules
//! find nothing: when does each of those logins become usable again?
//!
//! Nothing here reads a credential, makes a request or writes a file. A
//! pick carries a local profile name and nothing else about the login.

use super::candidate;
use super::rerun::{self, Choice, KeyAlert, Pick, Reads};
use crate::commands::profile::capacity_weight;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::organisation;
use amplihack_utils::organisation_affinity::{Candidate, Need, pools};

/// The "usable now" session bound: the value `profile usage` uses, read once
/// per choice. A garbled value falls back to the default, as it does there;
/// the warning is `profile usage`'s to print, not a waiting step's.
fn session_at() -> u8 {
    capacity_weight::usable_threshold(None, std::env::var_os(capacity_weight::ENV).as_deref()).0
}

/// The login a step whose own login has just reported a usage limit moves
/// to: rule 1, then rule 2. `current` is the directory the failed attempt
/// ran in and is never chosen, whatever its stored reading says; neither is
/// any name in `skip`. A pick by rule 2 has [`Pick::usable_only`] set.
pub(super) fn limit_candidate(
    reads: Reads<'_>,
    current: &str,
    skip: &[String],
    alert: KeyAlert,
) -> Option<Pick> {
    let session_at = session_at();
    rerun::with_choice(reads, current, skip, |choice| {
        rerun::candidate_in(choice, alert).or_else(|| usable_candidate(choice, session_at))
    })
}

/// Whether a plan login the step may move to: in the store, not the login
/// that failed, not excluded, neither overflow nor an API key, and passing
/// the shared eligibility (not skipped, leased or hosting, and ready).
fn is_target(choice: &Choice<'_, '_>, p: &ProfileName) -> bool {
    let ov = choice.overflow.pool();
    !ov.kinds_unknown
        && p.as_str() != choice.current
        && p.as_str() != choice.source_host
        && !choice.pool.exclude.contains(p)
        && ov.is_plan(p)
        && (choice.eligible)(p)
}

/// Rule 2: the usable plan login with the most quota left, within
/// organisation affinity.
fn usable_candidate(choice: &Choice<'_, '_>, session_at: u8) -> Option<Pick> {
    let usable = |p: &ProfileName| {
        choice
            .pool
            .readings
            .get(p.as_str())
            .is_some_and(|r| candidate::is_usable(r, session_at, choice.pool.now))
    };
    let targets: Vec<&ProfileName> = choice
        .pool
        .profiles
        .iter()
        .filter(|p| is_target(choice, p))
        .collect();
    let allowed = within_affinity(choice, &targets, usable);
    let picked = candidate::select_usable(choice.pool, choice.current, session_at, |p| {
        allowed.contains(&p)
    })?;
    Some(Pick {
        profile: picked.clone(),
        per_token: None,
        usable_only: true,
    })
}

/// The `targets` organisation affinity permits (issue #2136), by the policy
/// rule 1 applies: the failed login needs capacity relief, so a move across
/// organisations is permitted only when no login of its own organisation
/// has `capacity`.
fn within_affinity<'p>(
    choice: &Choice<'_, '_>,
    targets: &[&'p ProfileName],
    capacity: impl Fn(&ProfileName) -> bool,
) -> Vec<&'p ProfileName> {
    let Reads { state, ledger, .. } = choice.reads;
    let source = organisation::resolve(state, ledger, choice.source_host);
    let orgs: Vec<Option<String>> = targets
        .iter()
        .map(|p| organisation::resolve_login(state, p.as_str()))
        .collect();
    let candidates: Vec<Candidate<'_>> = targets
        .iter()
        .zip(&orgs)
        .map(|(p, org)| {
            let has = capacity(p);
            Candidate {
                org: org.as_deref(),
                eligible: true,
                capacity: has,
                room: has,
            }
        })
        .collect();
    pools(source.as_deref(), Need::Relief, &candidates)
        .allowed
        .iter()
        .map(|&i| targets[i])
        .collect()
}

/// When each login the step could still move to becomes usable again, on
/// the wait's clock: for every plan login that passes the same eligibility
/// as [`limit_candidate`], is not `current`, is not in `tried`, has a
/// reading and is not usable, the instant [`candidate::regains_usable`]
/// gives. The readings are stamped on the real clock and the wait may run on
/// its own, so each instant is moved over by its distance from now,
/// saturating.
///
/// Only instants strictly after `wait_now` are returned. A usable login, a
/// login with no reading or no reset ahead, and a login barred by another
/// rule contribute nothing; the wait's own poll picks such a login up when
/// its bar lifts. Whether an instant moves a wait's deadline, and the hold
/// to the weekly window, are the plan's (`limit_wait::plan::earliest`), not
/// this function's.
///
/// Organisation affinity removes nothing here. It narrows a choice among
/// logins that have capacity at the same moment, and a login that is the
/// first to reset has, at that moment, no rival of the failed login's own
/// organisation with capacity, which is the case in which the policy permits
/// a move across organisations.
pub(super) fn cross_login_reset(
    reads: Reads<'_>,
    current: &str,
    tried: &[String],
    wait_now: i64,
) -> Vec<(ProfileName, i64)> {
    let session_at = session_at();
    rerun::with_choice(reads, current, tried, |choice| {
        let real_now = choice.pool.now;
        choice
            .pool
            .profiles
            .iter()
            .filter(|p| is_target(choice, p))
            .filter_map(|p| {
                let reading = choice.pool.readings.get(p.as_str())?;
                let regains = candidate::regains_usable(reading, session_at, real_now)?;
                let at = wait_now.saturating_add(regains.saturating_sub(real_now));
                (at > wait_now).then(|| (p.clone(), at))
            })
            .collect()
    })
}

/// Whether the step may be run on `login` by the checks that do not depend
/// on a reading: it is a stored plan login, not excluded, not leased or
/// hosting another account, and holds a login of its own. Asked for the
/// retry after an earliest-reset wait, which needs no fresh reading because
/// the reset is what the wait was for, but must not land on a login that was
/// excluded, leased or removed while the step waited.
pub(super) fn may_run_on(reads: Reads<'_>, current: &str, login: &ProfileName) -> bool {
    rerun::with_choice(reads, current, &[], |choice| {
        choice.pool.profiles.contains(login) && is_target(choice, login)
    })
}
