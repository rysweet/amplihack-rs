//! What a cycle sees: who is live and who may receive the login, the
//! measurements and the decision, and the `--dry-run` preview built from
//! them without a lock or a write.

use super::cycle::{Ctx, CycleOutcome};
use super::decide::{Decision, Measured, Reading, decide};
use super::identify::{self, Account, CandidateInput, LiveId, Skip};
use super::log::{Sink, msg};
use super::measure::{self, Live};
use super::swap;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    Liveness, State, effective, exists, now, scan_system,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::run::resolve_claude_binary;

/// Who is live and who may receive the login: compared again under the
/// lock before a swap.
#[derive(Debug, Clone, PartialEq)]
pub struct Snap {
    pub live: LiveId,
    pub proto: Option<&'static str>,
    pub account: Option<String>,
    pub candidates: Vec<ProfileName>,
    pub skips: Vec<(ProfileName, Skip)>,
    pub sees_sessions: bool,
}

pub fn snapshot(ctx: &Ctx, state: &State) -> Result<Snap, ProfileError> {
    let root = ctx.store.root();
    let (live, proto) = identify::live(&ctx.store, &ctx.rs, &ctx.home)?;
    let liveness = scan_system(root);
    let sees_sessions = matches!(liveness, Liveness::Known(_));
    let live_name = match &live {
        LiveId::Known(n) => Some(n.clone()),
        _ => None,
    };
    let input = CandidateInput {
        live: live_name.as_ref(),
        exclude: effective(root).config.exclude,
        liveness,
        auto_switch: state.load_ledger()?,
    };
    let (candidates, skips) = identify::candidates(&ctx.store, &input)?;
    Ok(Snap {
        account: Account::of_dir(&ctx.home).map(|a| a.key().to_string()),
        live,
        proto,
        candidates,
        skips,
        sees_sessions,
    })
}

pub fn live_name(id: &LiveId) -> Option<&ProfileName> {
    match id {
        LiveId::Known(n) | LiveId::HandChanged(n) => Some(n),
        _ => None,
    }
}

/// The ALERT for a live login that cannot be used, if any.
pub fn live_alert(id: &LiveId) -> Option<(&'static str, String)> {
    match id {
        LiveId::NoLogin => Some(("no-login", msg::NO_LOGIN.to_string())),
        LiveId::HandChanged(n) => Some(("hand-change", msg::hand_change(n))),
        LiveId::Ambiguous => Some(("ambiguous", msg::AMBIGUOUS.to_string())),
        LiveId::Known(_) | LiveId::Adopt => None,
    }
}

pub fn reading_of(others: &[(ProfileName, Reading)], name: &ProfileName) -> Reading {
    others
        .iter()
        .find(|(n, _)| n == name)
        .map_or(Reading::Failed, |(_, r)| *r)
}

pub type Measurement = (Reading, Vec<(ProfileName, Reading)>, Decision);

pub fn measure_and_decide(
    ctx: &Ctx,
    binary: &crate::binary_finder::BinaryInfo,
    snap: &Snap,
) -> Result<Measurement, ProfileError> {
    let t = now();
    let live = match &snap.live {
        LiveId::Known(n) => Live::Profile(n),
        _ => Live::Unowned,
    };
    let (reading, others) = measure::measure(&ctx.store, binary, live, &snap.candidates, t);
    let measured: Vec<Measured> = others
        .iter()
        .map(|(name, reading)| Measured {
            name: name.clone(),
            reading: *reading,
        })
        .collect();
    let swapped_at = ctx.rs.load_ledger()?.and_then(|l| l.swapped_at);
    let decision = decide(&ctx.settings, &reading, &measured, swapped_at, now());
    Ok((reading, others, decision))
}

/// The outcome a live cycle would record for `decision`. `--dry-run` returns
/// it so the preview reports what it printed; nothing is recorded and the
/// exit code does not depend on it.
pub(super) fn would(decision: &Decision) -> CycleOutcome {
    match decision {
        Decision::Stay => CycleOutcome::Stayed,
        Decision::Swap { .. } => CycleOutcome::Swapped,
        Decision::Alert => CycleOutcome::Alerted,
    }
}

/// `--dry-run`: no lock, no prototype check, no recovery, no writes. Prints
/// `would ...` lines and the `cycle` line, and returns the outcome a live
/// cycle would have recorded.
pub fn dry_run(ctx: &mut Ctx) -> Result<CycleOutcome, ProfileError> {
    let mut out = Sink::new(None, true, false);
    if exists(&ctx.rs.journal) {
        match ctx.rs.load_journal() {
            Ok(Some(j)) => out.emit(
                0,
                &format!(
                    "would recover an interrupted swap (phase {})",
                    j.phase.as_str()
                ),
            ),
            Ok(None) => {}
            Err(_) => out.emit(0, "would alert: the rotate journal is damaged"),
        }
    }
    let binary = resolve_claude_binary()
        .map_err(|e| ProfileError::Io(std::io::Error::other(e.to_string())))?;
    let state = State::new(ctx.store.root())?;
    let snap = snapshot(ctx, &state)?;
    if let Some((_, line)) = live_alert(&snap.live) {
        let text = line.strip_prefix("ALERT: ").unwrap_or(&line);
        out.emit(0, &format!("would alert: {text}"));
        return Ok(CycleOutcome::Alerted);
    }
    let (live, others, decision) = measure_and_decide(ctx, &binary, &snap)?;
    let name = live_name(&snap.live);
    out.emit(0, &msg::cycle(name, &live, &others));
    let outcome = would(&decision);
    match decision {
        Decision::Stay => out.emit(0, "would stay"),
        Decision::Alert => {
            let line = msg::alert_no_capacity_for(name.map_or("-", ProfileName::as_str), &live);
            out.emit(
                0,
                &format!(
                    "would alert: {}",
                    line.strip_prefix("ALERT: ").unwrap_or(&line)
                ),
            );
        }
        Decision::Swap { to, reason } => {
            let from = match name {
                Some(n) => n.clone(),
                None => {
                    let n = swap::adoption_name(&ctx.store)?;
                    out.emit(0, &format!("would adopt ~/.claude as {n}"));
                    n
                }
            };
            let target = reading_of(&others, &to);
            out.emit(
                0,
                &format!("would {}", msg::swap(&from, &to, &live, &target, reason)),
            );
        }
    }
    Ok(outcome)
}
