//! The one organisation pool of the rotator's target choice (issue #2910,
//! D12 as narrowed by S1), and the organisation check the swap step makes
//! once authorisation has read both logins (S4).
//!
//! The pool replaces the rotator's two pools (capacity and plan room): one
//! set of candidates, one rule. With a known source organisation it is the
//! first of these groups that holds a usable login:
//!
//! 1. logins in the live login's organisation;
//! 2. logins whose organisation is not known (an unknown organisation is not
//!    a crossing, and the swap carries the existing "unresolved" warning);
//! 3. logins in another organisation, a confirmed crossing, only when
//!    `may_cross` is true: the live login is an overflow login being left, or
//!    it has no room now. An upgrade, a live login that still has room and a
//!    live login whose probe failed never cross.
//!
//! With no known source organisation the pool is every usable login, as
//! before. `capacity` on a [`Candidate`] means "qualifies and is not held";
//! `room` is not read. The shared `amplihack_utils::organisation_affinity` is
//! unchanged, because auto-switch uses it too. Pure functions: no file, no
//! clock, no network.

use amplihack_utils::organisation_affinity::{Affinity, Candidate};

/// The indices of the candidates the rotator may rank, in their given order.
pub fn pool(source: Option<&str>, may_cross: bool, candidates: &[Candidate<'_>]) -> Vec<usize> {
    let usable: Vec<(usize, Affinity)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.eligible && c.capacity)
        .map(|(i, c)| (i, Affinity::between(source, c.org)))
        .collect();
    let group = |kind: Affinity| -> Vec<usize> {
        usable
            .iter()
            .filter(|(_, a)| *a == kind)
            .map(|(i, _)| *i)
            .collect()
    };
    // `Affinity::between` with no trusted source is `Unresolved` for every
    // candidate, so an unknown source gives the whole usable list.
    let same = group(Affinity::Same);
    if !same.is_empty() {
        return same;
    }
    let unresolved = group(Affinity::Unresolved);
    if !unresolved.is_empty() || !may_cross {
        return unresolved;
    }
    group(Affinity::Crossing)
}

/// What the swap step does with the two organisations authorisation read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgCheck {
    /// The same organisation, or one of the two is unknown: go ahead, as
    /// before (an unknown organisation carries the "unresolved" warning).
    Same,
    /// A confirmed crossing the decision allowed: go ahead with the crossing
    /// warning.
    Cross,
    /// A confirmed crossing the decision did not allow: refuse the swap.
    Refuse,
}

/// S4: the check after authorisation, on the live (outgoing) login's
/// organisation and the incoming login's, from the identity reads the
/// authorisation already made. `may_cross` is the decision's own.
pub fn org_check(live_org: Option<&str>, incoming_org: Option<&str>, may_cross: bool) -> OrgCheck {
    match Affinity::between(live_org, incoming_org) {
        Affinity::Same | Affinity::Unresolved => OrgCheck::Same,
        Affinity::Crossing if may_cross => OrgCheck::Cross,
        Affinity::Crossing => OrgCheck::Refuse,
    }
}

#[cfg(test)]
#[path = "tests_issue_2910_pool.rs"]
mod tests_issue_2910_pool;
