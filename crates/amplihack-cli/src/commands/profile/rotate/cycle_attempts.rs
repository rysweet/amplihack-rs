//! Finite candidate traversal over one cycle's retained measurements (#2242).
use super::cycle::Ctx;
use super::decide::{Decision, Reason};
use super::log::Sink;
use super::observe::{Measurement, Snap, decide_available, snapshot};
use super::swap::{RecoverOutcome, Source};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, exists};
use crate::commands::profile::name::ProfileName;

pub(super) struct Success {
    pub from: ProfileName,
    pub to: ProfileName,
    pub reason: Reason,
    pub check: super::overflow_msg::CheckNote,
}
/// Safe refusals are only returned when no active journal owns any movement.
pub(super) enum Attempts {
    Swapped(Success),
    Refused(ProfileError),
}

pub(super) fn run(
    ctx: &mut Ctx,
    state: &State,
    first: &Snap,
    m: &Measurement,
    out: &mut Sink,
    failed_before: u32,
) -> Result<Attempts, ProfileError> {
    let mut decision = m.decision.clone();
    let mut attempted = Vec::new();
    let mut strategy = super::custody::Strategy::Original;
    let mut last = super::swap_authorize::refused("no remaining eligible candidate");
    // The list is fixed by the measurement. No candidate is probed or attempted twice.
    for _ in 0..=first.candidates.len() {
        let Decision::Swap { to, reason } = decision else {
            break;
        };
        if attempted.contains(&to) {
            break;
        }
        attempted.push(to.clone());
        let t = (ctx.now)();
        out.emit(
            t,
            &format!("candidate {to}: selected ({})", reason.as_str()),
        );
        let source = match &first.live {
            super::identify::LiveId::Known(n) => Source::Profile(n.clone()),
            _ => Source::Adopt,
        };
        let mut checked = None;
        let placement = || -> Result<ProfileName, ProfileError> {
            if exists(&ctx.rs.journal) {
                return Err(ProfileError::RotateInconsistent(
                    "pending journal before candidate attempt".into(),
                ));
            }
            let fresh = snapshot(ctx, state)?;
            let changed = super::revalidate::changed(
                first,
                &fresh,
                &Decision::Swap {
                    to: to.clone(),
                    reason,
                },
                false,
            );
            if changed.iter().any(|c| {
                !matches!(
                    c,
                    super::revalidate::Changed::Target(_)
                        | super::revalidate::Changed::TargetIdentity
                )
            }) {
                return Err(ProfileError::RotateInconsistent(
                    super::revalidate::changed_line(&changed),
                ));
            }
            if !changed.is_empty() {
                return Err(super::swap_authorize::refused(
                    super::revalidate::changed_line(&changed),
                ));
            }
            if let Some((live, probes)) = &m.probed
                && let Some((_, p)) = probes.iter().find(|(n, _)| n == &to)
                && super::fresh::gated_target(&Decision::Swap {
                    to: to.clone(),
                    reason,
                })
                .is_some()
                && super::fresh::stale(
                    super::fresh::read_at(p, m.measured_at),
                    super::fresh::live_time(live, m.measured_at),
                    None,
                    t,
                )
            {
                return Err(super::swap_authorize::refused(
                    "candidate usage is stale; waits for the next cycle",
                ));
            }
            let check = match super::overflow_record::target_check(ctx, first, &to) {
                Ok(check) => check,
                Err(why) => {
                    let mut ledger = ctx.rs.load_ledger()?.unwrap_or_default();
                    super::overflow_record::refuse_key(out, &mut ledger, &to, &why, t);
                    ctx.rs.save_ledger(&ledger)?;
                    return Err(super::swap_authorize::refused(why));
                }
            };
            checked = Some(check);
            let reading = m
                .others
                .iter()
                .find(|(n, _)| n == &to)
                .map_or(super::decide::Reading::Failed, |(_, r)| *r);
            let completion = super::swap_record::Completion::new(
                &reading,
                m.measured_at,
                (first.source_org(), first.org_of(&to)),
                m.tag,
            );
            super::swap::swap_operation_custody(
                &ctx.store,
                &ctx.rs,
                &ctx.home,
                source,
                &to,
                t,
                ctx.identity_reader(),
                Some(first),
                Some(completion),
                ctx.now,
                &mut strategy,
            )
        };
        match placement() {
            Ok(from) => {
                return Ok(Attempts::Swapped(Success {
                    from,
                    to,
                    reason,
                    check: checked.ok_or_else(|| {
                        ProfileError::RotateInconsistent(
                            "committed candidate has no login check".into(),
                        )
                    })?,
                }));
            }
            Err(error) => {
                out.emit(t, &format!("candidate {to}: refused or failed: {error}"));
                // Even an I/O error can follow a rename. Never infer placement
                // from its error class, and never fall back across a journal.
                if exists(&ctx.rs.journal) {
                    match super::recover::recover_at(
                        &ctx.store,
                        &ctx.rs,
                        &ctx.home,
                        ctx.identity_reader(),
                        ctx.now,
                    )? {
                        RecoverOutcome::RolledForward { from, to } => {
                            return Ok(Attempts::Swapped(Success {
                                from,
                                to,
                                reason,
                                check: checked.ok_or_else(|| {
                                    ProfileError::RotateInconsistent(
                                        "recovered candidate has no login check".into(),
                                    )
                                })?,
                            }));
                        }
                        RecoverOutcome::RolledBack { .. } => {}
                        _ => {
                            return Err(ProfileError::RotateInconsistent(
                                "candidate recovery unresolved; further attempts stopped".into(),
                            ));
                        }
                    }
                } else if matches!(error, ProfileError::RotateInconsistent(_)) {
                    return Err(error);
                }
                last = error;
            }
        }
        let Some((live, probes)) = &m.probed else {
            break;
        };
        let counted = std::mem::replace(&mut ctx.failed_live, failed_before);
        let next = decide_available(
            ctx,
            first,
            live.clone(),
            probes.clone(),
            m.measured_at,
            &attempted,
        );
        ctx.failed_live = counted;
        decision = next?.decision;
    }
    out.emit(
        (ctx.now)(),
        "candidates exhausted: rotation will reassess on the next cycle",
    );
    Ok(Attempts::Refused(last))
}
