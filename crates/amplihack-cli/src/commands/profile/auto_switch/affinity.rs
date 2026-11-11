//! Organisation affinity for auto-switch selection (#2136): the one place
//! the threshold swap ([`super::swap`]) and the nested rerun
//! ([`super::rerun`]) restrict their candidates. The rotator applies the
//! same pure policy (`amplihack_utils::organisation_affinity::pools`) in
//! `rotate/observe.rs`, on its trusted identity reads.
use super::candidate::{self, Pool};
use super::overflow_inputs::OverflowInputs;
use super::state::{Ledger, State};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::organisation;
use amplihack_utils::organisation_affinity::{Candidate, pools};

/// The pools one auto-switch decision may select from, and the
/// organisations it read to work them out.
pub struct Restriction {
    /// Targets the selector may rank.
    pub allowed: Vec<ProfileName>,
    /// Targets the plan-room fallback may use.
    pub plan_room: Vec<ProfileName>,
    /// The organisation of the login hosted in the source directory.
    pub source_org: Option<String>,
    orgs: Vec<(ProfileName, Option<String>)>,
}

impl Restriction {
    /// `target`'s organisation, as read for this decision.
    pub fn org_of(&self, target: &ProfileName) -> Option<&str> {
        self.orgs
            .iter()
            .find(|(p, _)| p == target)
            .and_then(|(_, o)| o.as_deref())
    }
}

/// Work out both pools once for one decision. `eligible` is the caller's
/// own target rule; affinity only ever narrows it. Overflow targets are
/// eligible only when the unrestricted selection would already move onto
/// overflow, so affinity cannot turn a plan move into paid spend.
#[allow(clippy::too_many_arguments)]
pub fn restrict(
    state: &State,
    ledger: &Ledger,
    pool: &Pool<'_>,
    overflow: &OverflowInputs,
    source_host: &str,
    current: &str,
    necessary: bool,
    eligible: impl Fn(&ProfileName) -> bool,
) -> Restriction {
    let ov = overflow.pool();
    let paid_allowed = candidate::select_with_overflow(pool, &ov, current, &eligible)
        .is_some_and(|p| overflow.order.contains(p));
    let affinity_eligible =
        |p: &ProfileName| eligible(p) && (paid_allowed || !overflow.order.contains(p));
    let target = |p: &ProfileName| {
        p.as_str() != current
            && affinity_eligible(p)
            && !pool.exclude.contains(p)
            && candidate::overflow_usable(p.as_str(), &overflow.blocked)
    };
    let source_org = organisation::resolve(state, ledger, source_host);
    let orgs: Vec<(ProfileName, Option<String>)> = pool
        .profiles
        .iter()
        .filter(|p| target(p))
        .map(|p| (p.clone(), organisation::resolve_login(state, p.as_str())))
        .collect();
    // Capacity: whether the selector, offered only this target, picks it.
    // Asked only of eligible targets.
    let candidates: Vec<Candidate<'_>> = orgs
        .iter()
        .map(|(p, org)| Candidate {
            org: org.as_deref(),
            eligible: true,
            capacity: candidate::select_restricted(
                pool,
                &ov,
                current,
                |q| q == p && affinity_eligible(q),
                |_| false,
            )
            .is_some(),
        })
        .collect();
    let pools = pools(source_org.as_deref(), necessary, &candidates);
    let names = |indices: &[usize]| -> Vec<ProfileName> {
        indices.iter().map(|&i| orgs[i].0.clone()).collect()
    };
    Restriction {
        allowed: names(&pools.allowed),
        plan_room: names(&pools.plan_room),
        source_org,
        orgs,
    }
}
