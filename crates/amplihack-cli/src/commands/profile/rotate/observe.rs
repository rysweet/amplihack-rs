//! What a cycle sees: who is live and who may receive the login, the
//! measurements and the decision.

use super::cycle::Ctx;
use super::damp::{damp, is_failed_probe_swap, keep_for_home_key, next_failed};
use super::decide::{Decision, Measured, Probed, Reading};
use super::identify::{self, Account, CandidateInput, LiveId, LoginKind, Skip};
use super::key_check;
use super::log::msg;
use super::measure::{self, Live};
use super::overflow::{OverflowInput, Tag, decide_restricted};
use super::pin::{self, PinState};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::kind;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{OverflowEntry, PLATFORM_TIER};

/// Who is live, who may receive the login, and the rotation pin: compared
/// again under the lock before a swap, so a `profile switch` during the
/// measurement stops it.
#[derive(Debug, Clone, PartialEq)]
pub struct Snap {
    pub live: LiveId,
    pub proto: Option<&'static str>,
    pub account: Option<String>,
    /// Full credential identity, compared with Account::same under the lock.
    pub trusted_live: Option<Account>,
    /// A fixed, redacted authorization refusal, naming only the profile.
    pub identity_refusal: Option<String>,
    /// Why `trusted_live` is empty: one of "Identity read causes".
    pub identity_cause: Option<&'static str>,
    /// Why each candidate skipped as `no account` had no trusted reading.
    pub candidate_refusals: Vec<(ProfileName, &'static str)>,
    pub candidates: Vec<ProfileName>,
    pub skips: Vec<(ProfileName, Skip)>,
    pub sees_sessions: bool,
    /// The overflow entries with their tiers, in file order. Part of the
    /// snapshot, so a mark or tier changed during measurement skips the
    /// cycle. The API-key profile (issue #1700) is the entry at
    /// [`PLATFORM_TIER`].
    pub overflow: Vec<OverflowEntry>,
    pub pin: PinState,
    /// Marked API-key profiles that are not used: another comes first by
    /// name.
    pub ignored: Vec<ProfileName>,
    /// API-key profiles that `overflow=` does not mark (issue #1804): never
    /// candidates, never probed, never plan capacity or plan room.
    pub unmarked: Vec<ProfileName>,
    /// The marked API-key profile, when its kind is detected and no
    /// `kind set` confirmed it: its mark may have been made for a credit
    /// account (issue #1804).
    pub detected_mark: Option<ProfileName>,
    /// `~/.claude/.credentials.json` is a readable login without OAuth (API key).
    pub home_no_oauth: bool,
    /// Each candidate's account; step 6 compares the target's (#1721).
    pub candidate_accounts: Vec<(ProfileName, Option<Account>)>,
}

impl Snap {
    /// The API-key profile the tier uses, if any.
    pub fn platform(&self) -> Option<&ProfileName> {
        self.overflow
            .iter()
            .find(|e| e.tier == PLATFORM_TIER)
            .map(|e| &e.name)
    }

    /// The live login is an API-key profile's: `identify::live` names a
    /// profile for a login without OAuth only when it is one.
    pub fn platform_live(&self) -> bool {
        self.home_no_oauth && matches!(&self.live, LiveId::Known(_))
    }

    /// The live API-key profile when it is not the marked one, for example
    /// an unmarked one left live by a release with the implicit tier
    /// (issue #1804). The rules that move work off the API key apply to it;
    /// it is never a target and never counts as a mark.
    pub fn adopted_platform(&self) -> Option<&ProfileName> {
        match &self.live {
            LiveId::Known(n) if self.platform_live() && self.platform() != Some(n) => Some(n),
            _ => None,
        }
    }

    /// The API-key profile shown as `per-token` and left out of the
    /// no-capacity file: the marked one, or an adopted live one.
    pub fn shown_platform(&self) -> Option<&ProfileName> {
        self.adopted_platform().or(self.platform())
    }

    /// Busy or identity-unreadable plan logins remain unknown capacity.
    /// Excluded, login-less, non-OAuth and overflow profiles are not plan room.
    pub fn unmeasured_plan(&self) -> Vec<ProfileName> {
        self.skips
            .iter()
            .filter(|(_, s)| {
                matches!(
                    s,
                    Skip::SessionRunning
                        | Skip::Leased
                        | Skip::Hosting
                        | Skip::Parked
                        | Skip::NoAccount
                )
            })
            .map(|(n, _)| n)
            .filter(|n| {
                !self.overflow.iter().any(|e| &e.name == *n)
                    && !self.unmarked.contains(n)
                    && !self.ignored.contains(n)
            })
            .cloned()
            .collect()
    }

    /// Plan profiles skipped as `directory refused` (issue #1698). Unmarked
    /// and unused API-key profiles among them were never plan logins, so
    /// they hold nothing back (issue #1804).
    pub fn refused_plan(&self) -> Vec<ProfileName> {
        identify::refused_dirs(&self.skips)
            .into_iter()
            .filter(|n| !self.unmarked.contains(n) && !self.ignored.contains(n))
            .collect()
    }

    /// The overflow entries the decision uses: [`Self::overflow`], with an
    /// adopted live API-key profile at [`PLATFORM_TIER`] in place of the
    /// marked one. Live, it is never moved to; the entry only lets the exit
    /// rules apply. A marked API-key profile is then not a target either,
    /// since the two share one credential.
    pub fn decision_overflow(&self) -> Vec<OverflowEntry> {
        let Some(adopted) = self.adopted_platform() else {
            return self.overflow.clone();
        };
        self.overflow
            .iter()
            .filter(|e| e.tier != PLATFORM_TIER)
            .cloned()
            .chain(std::iter::once(OverflowEntry::platform(adopted.clone())))
            .collect()
    }
}

#[path = "observe_snapshot.rs"]
mod take;
pub use take::snapshot;

pub fn live_name(id: &LiveId) -> Option<&ProfileName> {
    match id {
        LiveId::Known(n) | LiveId::HandChanged(n) | LiveId::HomeUnreadable(n) => Some(n),
        _ => None,
    }
}

/// The ALERT for a live login that cannot be used, if any.
pub fn live_alert(snap: &Snap) -> Option<(&'static str, String)> {
    if let Some(reason) = &snap.identity_refusal {
        let kind = if reason.starts_with(super::swap_expect::ACCOUNT_MISMATCH) {
            "account_mismatch"
        } else {
            "trusted-identity"
        };
        return Some((kind, format!("ALERT: swap refused: {reason}")));
    }
    if !snap.home_no_oauth && snap.trusted_live.is_none() && !matches!(snap.live, LiveId::NoLogin) {
        let text = snap.identity_cause.map_or_else(
            || super::login_identity::UNAVAILABLE.to_string(),
            super::login_identity::unavailable_line,
        );
        return Some(("trusted-identity", format!("ALERT: {text}")));
    }
    live_alert_for(&snap.live, snap.home_no_oauth)
}

/// [`live_alert`] for a live login `live`; `home_no_oauth` is
/// [`Snap::home_no_oauth`].
pub fn live_alert_for(live: &LiveId, home_no_oauth: bool) -> Option<(&'static str, String)> {
    match live {
        LiveId::NoLogin => Some(("no-login", msg::NO_LOGIN.to_string())),
        LiveId::HandChanged(n) => Some(("hand-change", msg::hand_change(n))),
        LiveId::HomeUnreadable(n) => Some(("home-unreadable", msg::home_unreadable(n))),
        LiveId::Ambiguous if home_no_oauth => {
            Some(("ambiguous", msg::AMBIGUOUS_PLATFORM.to_string()))
        }
        LiveId::Ambiguous => Some(("ambiguous", msg::AMBIGUOUS.to_string())),
        LiveId::Known(_) | LiveId::Adopt => None,
    }
}

/// The live profile's probe and each candidate's.
pub type Probes = (Probed, Vec<(ProfileName, Probed)>);

/// One cycle's measurements and its decision, after the overflow policy
/// and failed-probe damping.
pub struct Measurement {
    pub live: Reading,
    pub others: Vec<(ProfileName, Reading)>,
    pub decision: Decision,
    /// The log line for a damped stay.
    pub note: Option<String>,
    /// Consecutive failed live probes, including this cycle's.
    pub failed: u32,
    /// What the cycle records about overflow.
    pub tag: Option<Tag>,
    /// Overflow profiles to block for a spend limit.
    pub new_blocks: Vec<ProfileName>,
    /// The earliest plan reset, for the `overflow:` log lines.
    pub next_plan_reset: Option<i64>,
    /// Plan logins whose state is unknown this cycle.
    pub unknown_plan: Vec<ProfileName>,
    /// The unknown logins, when only they kept work off the API key.
    pub key_held: Vec<ProfileName>,
    /// When measuring started, epoch seconds.
    pub measured_at: i64,
    /// The probes decided on, for step 6 to re-decide a swap whose target
    /// left the candidates (#1721); `None` from a stub, which disables that.
    /// `Probed` must never hold a token or an account identifier.
    pub probed: Option<Probes>,
    /// Issue #1870: the freshness gate's `reread:` and `damp:` lines.
    pub fresh_lines: Vec<String>,
}

pub fn measure_and_decide(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let measured_at = now();
    let platform = snap.platform();
    let live = match &snap.live {
        _ if snap.platform_live() => Live::Platform,
        LiveId::Known(n) => Live::Profile(n),
        _ => Live::Unowned,
    };
    // The API-key profile is never probed: it gets its synthetic reading,
    // and it is in the overflow order, so never in the plan pool.
    let to_probe: Vec<ProfileName> = snap
        .candidates
        .iter()
        .filter(|c| Some(*c) != platform)
        .cloned()
        .collect();
    // Keep the existing usage reader's refresh opportunity for otherwise
    // eligible OAuth logins. Their readings cannot enter this decision:
    // a later cycle must first bind identity to the refreshed credentials.
    let unbound: Vec<ProfileName> = snap
        .skips
        .iter()
        .filter(|(_, reason)| matches!(reason, Skip::NoAccount))
        .map(|(name, _)| name.clone())
        .collect();
    let (probe, mut probes) = measure::measure_ordered(ctx, live, &to_probe, &unbound, measured_at);
    if let Some(p) = platform.filter(|p| snap.candidates.contains(p)) {
        probes.push((p.clone(), Probed::platform()));
    }
    decide_measured(ctx, snap, probe, probes, measured_at)
}

/// Step 5 of a cycle: the overflow-aware decision on this cycle's probes,
/// the live profile's and each candidate's, measured from `measured_at`.
/// Tests that stub probing call this, so they run the real decision.
pub(super) fn decide_measured(
    ctx: &Ctx,
    snap: &Snap,
    probe: Probed,
    probes: Vec<(ProfileName, Probed)>,
    measured_at: i64,
) -> Result<Measurement, ProfileError> {
    let measured: Vec<Measured> = probes
        .iter()
        .map(|(name, p)| Measured::new(name.clone(), p))
        .collect();
    // Restrict the pool before headroom ranking. Affinity never makes an
    // ineligible login eligible and unknown identities never match.
    use amplihack_utils::organisation_affinity::{Affinity, Candidate, Purpose, allowed_indices};
    let source_org = snap.trusted_live.as_ref().and_then(Account::org);
    let orgs: Vec<_> = measured
        .iter()
        .map(|c| {
            snap.candidate_accounts
                .iter()
                .find(|(n, _)| *n == c.name)
                .and_then(|(_, a)| a.as_ref())
                .and_then(Account::org)
        })
        .collect();
    let ledger = ctx.rs.load_ledger()?;
    let t = now();
    let blocked = |n: &ProfileName| {
        ledger.as_ref().is_some_and(|l| {
            l.overflow_blocked
                .get(n.as_str())
                .is_some_and(|b| t < b.until)
                || (snap.platform() == Some(n)
                    && l.platform_block.as_ref().is_some_and(|b| t < b.until))
        })
    };
    let on_overflow =
        live_name(&snap.live).is_some_and(|n| snap.overflow.iter().any(|e| &e.name == n));
    let necessary = if on_overflow {
        probe.spend_limit
            || live_name(&snap.live).is_some_and(blocked)
            || probe.reading == Reading::Failed
    } else {
        super::decide::needs_swap(&ctx.settings, &probe.reading)
    };
    let mut candidates: Vec<_> = measured
        .iter()
        .zip(&orgs)
        .map(|(c, org)| Candidate {
            org: *org,
            eligible: !c.spend_limit && !blocked(&c.name),
            capacity: false, // Filled by the authoritative selector below.
        })
        .collect();
    // A ledger that cannot be read ends the cycle here, before any move onto
    // credits: its spend blocks are unknown. Only a missing ledger means no
    // blocks.
    let swapped_at = ledger.as_ref().and_then(|l| l.swapped_at);
    let platform_block = ledger.as_ref().and_then(|l| l.platform_block.clone());
    let blocks = ledger.map(|l| l.overflow_blocked).unwrap_or_default();
    let live_name = match &snap.live {
        LiveId::Known(n) => Some(n),
        _ => None,
    };
    let overflow = snap.decision_overflow();
    let unmeasured_plan = snap.unmeasured_plan();
    let refused = snap.refused_plan();
    let input = OverflowInput {
        settings: &ctx.settings,
        live_name,
        live: &probe,
        candidates: &measured,
        unmeasured_plan: &unmeasured_plan,
        refused: &refused,
        overflow: &overflow,
        blocks: &blocks,
        platform_block: platform_block.as_ref(),
        swapped_at,
        now: t,
    };
    // Overflow must pass the existing gate before it can count as a
    // same-organisation alternative. In particular, plan room excludes it.
    let unrestricted = decide_restricted(&input, None);
    let paid_allowed = matches!(&unrestricted.decision, Decision::Swap { to, .. } if overflow.iter().any(|e| &e.name == to));
    for (candidate, measured) in candidates.iter_mut().zip(&measured) {
        if !paid_allowed && overflow.iter().any(|e| e.name == measured.name) {
            candidate.eligible = false;
        }
    }
    // Ask the authoritative selector whether each target can provide relief.
    // Disable plan-room fallback here; its Same-only pool is separate below.
    for (candidate, measured) in candidates.iter_mut().zip(&measured) {
        let permitted = [measured.name.clone()];
        candidate.capacity = matches!(
            decide_restricted(&input, Some((&permitted, &[]))).decision,
            Decision::Swap { ref to, .. } if to == &measured.name
        );
    }
    let purpose = if necessary {
        Purpose::CapacityRelief
    } else {
        Purpose::Upgrade
    };
    let allowed: Vec<_> = allowed_indices(source_org, purpose, necessary, &candidates)
        .into_iter()
        .map(|i| measured[i].name.clone())
        .collect();
    // The downstream plan-room selector checks actual room.
    let room_candidates: Vec<_> = candidates
        .iter()
        .map(|c| Candidate {
            org: c.org,
            eligible: c.eligible,
            capacity: true,
        })
        .collect();
    let same: Vec<_> = allowed_indices(source_org, Purpose::PlanRoom, false, &room_candidates)
        .into_iter()
        .map(|i| measured[i].name.clone())
        .collect();
    let mut verdict = decide_restricted(&input, Some((&allowed, &same)));
    // Fallbacks use their final purpose: a threshold cycle can end in a
    // preference-only plan-room move, which still requires known affinity.
    if let Decision::Swap {
        to,
        reason: super::decide::Reason::PlanRoom | super::decide::Reason::Upgrade,
    } = &verdict.decision
    {
        let target_org = snap
            .candidate_accounts
            .iter()
            .find(|(n, _)| n == to)
            .and_then(|(_, a)| a.as_ref())
            .and_then(Account::org);
        if Affinity::between(source_org, target_org) != Affinity::Same {
            verdict.decision = Decision::Stay;
        }
    }
    // A spend limit on a live overflow profile is handled by its block, so
    // it does not count towards the failed live probes.
    let live_over = live_name.is_some_and(|n| overflow.iter().any(|e| &e.name == n));
    let failed = if live_over && probe.spend_limit {
        ctx.failed_live
    } else {
        next_failed(ctx.failed_live, &probe.reading)
    };
    let (decision, note) = damp(
        verdict.decision,
        &probe.reading,
        failed,
        swapped_at,
        t,
        &ctx.settings,
    );
    // Only here is the file read: a swap on this ground is rare. The strict
    // reader, not `key_check::home_key`: a key Claude may not read must not
    // keep a dead login live, and the swap it would have stopped is harmless.
    let home_key = (is_failed_probe_swap(&decision) && !snap.platform_live())
        .then(|| key_check::reachable(&ctx.home).ok())
        .flatten();
    let (decision, note) = keep_for_home_key(decision, note, home_key);
    // A damped move off credits is a stay on them.
    let tag = match (&decision, verdict.tag) {
        (Decision::Stay, Some(Tag::Out)) => Some(Tag::On),
        (_, tag) => tag,
    };
    let others = probes.iter().map(|(n, p)| (n.clone(), p.reading)).collect();
    // `probed` is always `Some` here: `Room::of` treats `None` (test stubs
    // only) as clean.
    Ok(Measurement {
        live: probe.reading,
        others,
        probed: Some((probe, probes)),
        decision,
        note,
        failed,
        tag,
        new_blocks: verdict.new_blocks,
        next_plan_reset: verdict.next_plan_reset,
        unknown_plan: verdict.unknown_plan,
        key_held: verdict.key_held,
        measured_at,
        fresh_lines: Vec::new(),
    })
}

/// The reason of the API-key block in the ledger, for the ALERT that stays
/// on a blocked API key.
pub fn platform_block_reason(ctx: &Ctx) -> String {
    ctx.rs
        .load_ledger()
        .ok()
        .flatten()
        .and_then(|l| l.platform_block)
        .map_or_else(|| "blocked".to_string(), |b| b.reason)
}
