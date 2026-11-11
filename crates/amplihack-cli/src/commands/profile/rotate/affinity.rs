//! Organisation affinity and session warnings for the rotator (#2136).
//!
//! Selection: [`decide`] restricts the overflow-aware decision with the
//! shared pure policy (`amplihack_utils::organisation_affinity::pools`),
//! called once per decision. Only a confirmed crossing is restricted; an
//! unknown organisation is not one.
//!
//! Warnings: advisory. The swap is the job and the warning tells live
//! sessions about it, so no failure here stops or undoes a swap; each one
//! is reported on stderr by `swap_warning`.
use super::super::cycle::Ctx;
use super::super::decide::{Decision, Measured, Reason};
use super::super::identify::Account;
use super::super::ledger::RotateJournal;
use super::super::overflow::{OverflowInput, Verdict, decide_restricted};
use super::super::swap::{self, Source};
use super::Snap;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::swap_warning;
use amplihack_utils::organisation_affinity::{Candidate, pools, preference_permitted, valid_id};
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
/// affinity never makes an ineligible login eligible. `necessary` is
/// whether the live login needs capacity relief.
pub(in super::super) fn decide(
    snap: &Snap,
    input: &OverflowInput<'_>,
    measured: &[Measured],
    eligible: &[bool],
    necessary: bool,
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
            let capacity = eligible
                && matches!(
                    decide_restricted(input, Some((&permitted, &[]))).decision,
                    Decision::Swap { ref to, .. } if to == &m.name
                );
            Candidate {
                org: snap.org_of(&m.name),
                eligible,
                capacity,
            }
        })
        .collect();
    let pools = pools(snap.source_org(), necessary, &candidates);
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

/// A warning intent written before a swap.
pub(in super::super) struct Prepared {
    pub transaction: String,
    outgoing: ProfileName,
}

/// Write the warning intent for a swap onto `to`, or report why not and
/// return `None`: the swap then runs without a session warning.
pub(in super::super) fn prepare_warning(
    ctx: &Ctx,
    first: &Snap,
    source: &Source,
    to: &ProfileName,
    reason: Reason,
    t: i64,
) -> Option<Prepared> {
    let outgoing = match source {
        Source::Profile(n) => Some(n.clone()),
        Source::Adopt => swap::adoption_name(&ctx.store).ok(),
    };
    let live = swap::ino_at(&ctx.home.join(".claude/.credentials.json"));
    let target = swap::ino_at(&ctx.store.root().join(to.as_str()).join(".credentials.json"));
    let (Some(outgoing), Some(live), Some(target)) = (outgoing, live, target) else {
        eprintln!(
            "rotator: session warning intent skipped; the swap is unaffected: login identity unavailable"
        );
        return None;
    };
    let tx = swap_warning::rotator_key(std::process::id(), t, live, target);
    let transaction = swap_warning::prepare(
        ctx.store.root(),
        &tx,
        "rotator",
        reason.as_str(),
        "home",
        outgoing.as_str(),
        to.as_str(),
        first.source_org(),
        first.org_of(to),
    )?;
    Some(Prepared {
        transaction,
        outgoing,
    })
}

/// Drop the intent of a swap that did not happen and left no journal.
pub(in super::super) fn abort_prepared(ctx: &Ctx, prepared: Option<&Prepared>, to: &ProfileName) {
    if let Some(p) = prepared {
        let (from, to) = (p.outgoing.as_str(), to.as_str());
        swap_warning::abort(
            ctx.store.root(),
            &p.transaction,
            "rotator",
            "home",
            from,
            to,
        );
    }
}

/// Drop the intent a rolled-back journal links.
pub(in super::super) fn abort_warning(store: &ProfileStore, j: &RotateJournal) {
    if let Some(tx) = &j.warning_transaction {
        swap_warning::abort(store.root(), tx, "rotator", "home", &j.from, &j.to);
    }
}

/// Whether a journal's warning reference, if any, is well formed.
pub(in super::super) fn valid_transaction(tx: Option<&str>) -> bool {
    tx.is_none_or(valid_id)
}

/// `rotate status`: pending session warnings and the last swap's outcome.
pub(in super::super) fn print_status(root: &Path) {
    if root.join(".swap-warning-history").exists() {
        match amplihack_utils::swap_warning_history::WarningHistory::open(root)
            .and_then(|h| h.unconsumed())
        {
            Ok(events) => {
                for event in events {
                    if let Some(warning) = event.affinity.warning() {
                        println!("pending swap warning #{}: {warning}", event.id);
                    }
                }
            }
            Err(e) => eprintln!("warning: swap warning history unreadable: {e}"),
        }
    }
    if let Some(record) = amplihack_utils::rotator_status::swap_record::read_swap(root, now())
        && let Some(warning) = record.affinity.and_then(|a| a.warning())
    {
        println!("last swap: {warning}");
    }
}
