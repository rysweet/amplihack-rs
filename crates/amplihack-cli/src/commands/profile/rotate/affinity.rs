//! Organisation affinity and session warnings for the rotator (#2136).
//!
//! Selection: [`decide`] restricts the overflow-aware decision with the
//! shared pure policy (`amplihack_utils::organisation_affinity::pools`),
//! called once per decision. Only a confirmed crossing is restricted; an
//! unknown organisation is not one. The issue #2211 outlook, a projected or
//! substituted live figure, is decided here too ([`with_outlook`]), with
//! the cycle's one `Need`: it never makes a confirmed crossing, nor makes
//! one sooner.
//!
//! Warnings: the swap record (`swap.json`) carries `from_org`, `to_org` and
//! `affinity`; `limit-resume` reads that record and adds the warning to the
//! resume it sends. The rotate log and `rotate status` report the same
//! outcome. Nothing here can stop or undo a swap.
use super::super::decide::{self as rule, Decision, Measured, Probed, Reading, Reason};
use super::super::fresh::live::{self, Outlook};
use super::super::identify::Account;
use super::super::overflow::{OverflowInput, Verdict, decide_restricted};
use super::super::settings::Settings;
use super::Snap;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use amplihack_utils::organisation_affinity::{
    Affinity, Candidate, Need, pools, preference_permitted,
};
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

/// Issue #2211: `verdict`, the decision on the reading as measured in
/// `input`, with the live login's outlook `o` applied (`live::with_outlook`).
/// The outlook is decided with `need`, the cycle's own, worked out by
/// [`Need::of`] on the reading as measured: by [`outlook`] for a measured
/// reading, and by [`outlook_failed`] for a failed one.
pub(in super::super) fn with_outlook(
    snap: &Snap,
    input: &OverflowInput<'_>,
    verdict: Verdict,
    o: &Outlook,
    measured: &[Measured],
    eligible: &[bool],
    need: Need,
) -> Verdict {
    let (s, real) = (input.settings, &input.live.reading);
    let on_outlook = if *real == Reading::Failed {
        outlook_failed(snap, s, &verdict.decision, &o.reading(), measured)
    } else {
        let figures = Probed {
            reading: o.reading(),
            ..input.live.clone()
        };
        let input = OverflowInput {
            live: &figures,
            ..*input
        };
        outlook(snap, &input, measured, eligible, need)
    };
    live::with_outlook(verdict, on_outlook, o.basis, s, real)
}

/// Issue #2211: the decision on the outlook of a live plan login whose
/// reading was measured, for `live::with_outlook`: a threshold swap, the
/// ALERT, or a stay. `input.live` holds the outlook's figures (a projected
/// or substituted session use), and `need` is the cycle's own, worked out by
/// [`Need::of`] on the reading as measured, not on the outlook.
///
/// The outlook matters only when that reading is below both thresholds
/// (`with_outlook` keeps the measured verdict otherwise). The live login
/// then has capacity, so `need` is [`Need::Upgrade`] and the outlook's
/// pools never hold a confirmed crossing: a forecast does not make the
/// relief necessary. A same-organisation login with capacity on the outlook
/// is taken first, then an unresolved one.
///
/// - A threshold swap under affinity: that swap.
/// - None, but one without affinity: only a confirmed crossing has
///   capacity. A stay, not the no-capacity ALERT: there is capacity, in
///   another organisation, and the reading as measured crosses to it once
///   it reaches a threshold ([`Need::Relief`]).
/// - None either way: the ALERT. No login has capacity on the outlook,
///   whatever the plan-room or overflow rules would make of it, since those
///   moves never run on an outlook.
fn outlook(
    snap: &Snap,
    input: &OverflowInput<'_>,
    measured: &[Measured],
    eligible: &[bool],
    need: Need,
) -> Decision {
    let threshold = |d: &Decision| {
        matches!(
            d,
            Decision::Swap {
                reason: Reason::Threshold,
                ..
            }
        )
    };
    let d = decide(snap, input, measured, eligible, need).decision;
    if threshold(&d) {
        d
    } else if threshold(&decide_restricted(input, None).decision) {
        Decision::Stay
    } else {
        Decision::Alert
    }
}

/// Issue #2211: the decision on the outlook of a live plan login whose
/// probe failed, for `live::with_outlook`. `real` is the cycle's verdict on
/// the failed reading: the failed-probe rules chose its target, under the
/// cycle's `need` ([`Need::Relief`], since a failed probe needs a swap), and
/// the outlook can only make that swap sooner.
///
/// So the outlook is the decision rule on the substituted `figures` with
/// `real`'s own target as the only candidate: a threshold swap to it when
/// it has `--margin` more headroom than the substituted figure leaves the
/// live login, and otherwise a stay. Another candidate's headroom never
/// counts. A target in another organisation, a confirmed crossing, gets a
/// stay: a substituted figure never makes a crossing sooner, so that swap
/// waits for failed-probe damping, as without the outlook.
fn outlook_failed(
    snap: &Snap,
    s: &Settings,
    real: &Decision,
    figures: &Reading,
    measured: &[Measured],
) -> Decision {
    let Decision::Swap { to, .. } = real else {
        return Decision::Stay;
    };
    if Affinity::between(snap.source_org(), snap.org_of(to)) == Affinity::Crossing {
        return Decision::Stay;
    }
    let target: Vec<Measured> = measured.iter().filter(|c| &c.name == to).cloned().collect();
    // The substituted figure is at `--session-at` or above, so the rule's
    // threshold row decides: it reads neither the hold nor the clock.
    match rule::decide(s, figures, &target, None, 0) {
        d @ Decision::Swap {
            reason: Reason::Threshold,
            ..
        } => d,
        _ => Decision::Stay,
    }
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
