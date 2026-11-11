//! Organisation affinity and session warnings for the rotator (#2136).
//!
//! Selection: [`decide`] restricts the overflow-aware decision with the
//! shared pure policy (`amplihack_utils::organisation_affinity::pools`),
//! called once per decision. Only a confirmed crossing is restricted; an
//! unknown organisation is not one.
//!
//! Warnings: the swap record (`swap.json`) carries `from_org`, `to_org` and
//! `affinity`; `limit-resume` reads that record and adds the warning to the
//! resume it sends. The rotate log and `rotate status` report the same
//! outcome. Nothing here can stop or undo a swap.
use super::super::decide::{Decision, Measured, Reason};
use super::super::identify::Account;
use super::super::overflow::{OverflowInput, Verdict, decide_restricted};
use super::Snap;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use amplihack_utils::organisation_affinity::{Candidate, Need, pools, preference_permitted};
use std::path::Path;

impl Snap {
    /// The live login's organisation, from its trusted identity read. The
    /// swap guard compares that read with the profile's expected record
    /// (`expected::lookup`) before any move, so where a record exists it
    /// agrees with what auto-switch reads; see `organisation.rs`.
    pub fn source_org(&self) -> Option<&str> {
        self.trusted_live.as_ref().and_then(Account::org)
    }

    /// `name`'s organisation, from the same trusted reads.
    pub fn org_of(&self, name: &ProfileName) -> Option<&str> {
        self.candidate_accounts
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, a)| a.as_ref())
            .and_then(Account::org)
    }
}

/// The overflow-aware decision, restricted by organisation affinity before
/// headroom ranking. `eligible` is each measured candidate's own rule;
/// affinity never makes an ineligible login eligible. `need` says why the
/// session may move ([`Need::of`]).
pub(in super::super) fn decide(
    snap: &Snap,
    input: &OverflowInput<'_>,
    measured: &[Measured],
    eligible: &[bool],
    need: Need,
) -> Verdict {
    let overflow = input.overflow;
    let is_overflow = |n: &ProfileName| overflow.iter().any(|e| &e.name == n);
    // Overflow must pass the existing gate before it can count as an
    // alternative. In particular, plan room excludes it.
    let unrestricted = decide_restricted(input, None);
    let paid_allowed = matches!(&unrestricted.decision,
        Decision::Swap { to, .. } if is_overflow(to));

    let candidates: Vec<Candidate<'_>> = measured
        .iter()
        .zip(eligible)
        .map(|(m, &ok)| {
            let eligible = ok && (paid_allowed || !is_overflow(&m.name));
            // Whether the authoritative selector, offered only this target,
            // picks it. Plan-room fallback has its own pool.
            let permitted = [m.name.clone()];
            let picks = |d: Decision| matches!(d, Decision::Swap { ref to, .. } if to == &m.name);
            let capacity =
                eligible && picks(decide_restricted(input, Some((&permitted, &[]))).decision);
            // Whether the plan-room fallback, offered only this target,
            // picks it: room without strict capacity counts here.
            let room =
                eligible && picks(decide_restricted(input, Some((&[], &permitted))).decision);
            Candidate {
                org: snap.org_of(&m.name),
                eligible,
                capacity,
                room,
            }
        })
        .collect();
    let pools = pools(snap.source_org(), need, &candidates);
    let names = |indices: &[usize]| -> Vec<ProfileName> {
        indices.iter().map(|&i| measured[i].name.clone()).collect()
    };
    let (allowed, plan_room) = (names(&pools.allowed), names(&pools.plan_room));
    let mut verdict = decide_restricted(input, Some((&allowed, &plan_room)));
    // Fallbacks use their final purpose: a threshold cycle can end in a
    // preference-only move, which never makes a confirmed crossing.
    if let Decision::Swap {
        to,
        reason: Reason::PlanRoom | Reason::Upgrade,
    } = &verdict.decision
        && !preference_permitted(snap.source_org(), snap.org_of(to))
    {
        verdict.decision = Decision::Stay;
    }
    // Where affinity changed nothing, the verdict is the unrestricted one,
    // whole: its notes (such as the plan logins that kept work off the API
    // key) describe the full candidate set, not the restricted pools.
    if verdict.decision == unrestricted.decision {
        return unrestricted;
    }
    verdict
}

/// `rotate status`: the last swap's organisation outcome, from the swap
/// record. Nothing else records it; the rotate log keeps the history.
pub(in super::super) fn print_status(root: &Path) {
    if let Some(record) = amplihack_utils::rotator_status::swap_record::read_swap(root, now())
        && let Some(warning) = record.affinity.and_then(|a| a.warning())
    {
        println!("last swap: {warning}");
    }
}
