//! What a cycle sees: who is live and who may receive the login, the
//! measurements and the decision, and the `--dry-run` preview built from
//! them without a lock or a write.

use super::cycle::{Ctx, CycleOutcome};
use super::damp::{damp, next_failed};
use super::decide::{Decision, Measured, Reading, decide};
use super::handfix;
use super::identify::{self, Account, CandidateInput, LiveId, Skip};
use super::ledger::Phase;
use super::log::{Sink, msg};
use super::measure::{self, Live};
use super::pin::{self, Gate, PinState};
use super::swap;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    Liveness, Rotated, State, effective, exists, now,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::run::resolve_claude_binary;

/// Who is live, who may receive the login, and the rotation pin: compared
/// again under the lock before a swap, so a `profile switch` during the
/// measurement stops it.
#[derive(Debug, Clone, PartialEq)]
pub struct Snap {
    pub live: LiveId,
    pub proto: Option<&'static str>,
    pub account: Option<String>,
    pub candidates: Vec<ProfileName>,
    pub skips: Vec<(ProfileName, Skip)>,
    pub sees_sessions: bool,
    pub pin: PinState,
}

pub fn snapshot(ctx: &Ctx, state: &State) -> Result<Snap, ProfileError> {
    let root = ctx.store.root();
    let (live, proto) = identify::live(&ctx.store, &ctx.rs, &ctx.home)?;
    let liveness = (ctx.scan)(root);
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
        pin: pin::read(&ctx.rs),
    })
}

pub fn live_name(id: &LiveId) -> Option<&ProfileName> {
    match id {
        LiveId::Known(n) | LiveId::HandChanged(n) | LiveId::HomeUnreadable(n) => Some(n),
        _ => None,
    }
}

/// The ALERT for a live login that cannot be used, if any.
pub fn live_alert(id: &LiveId) -> Option<(&'static str, String)> {
    match id {
        LiveId::NoLogin => Some(("no-login", msg::NO_LOGIN.to_string())),
        LiveId::HandChanged(n) => Some(("hand-change", msg::hand_change(n))),
        LiveId::HomeUnreadable(n) => Some(("home-unreadable", msg::home_unreadable(n))),
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

/// One cycle's measurements and its decision, after failed-probe damping.
pub struct Measurement {
    pub live: Reading,
    pub others: Vec<(ProfileName, Reading)>,
    pub decision: Decision,
    /// The log line for a damped stay.
    pub note: Option<String>,
    /// Consecutive failed live probes, including this cycle's.
    pub failed: u32,
    /// When measuring started, epoch seconds.
    pub measured_at: i64,
}

pub fn measure_and_decide(
    ctx: &Ctx,
    binary: &crate::binary_finder::BinaryInfo,
    snap: &Snap,
) -> Result<Measurement, ProfileError> {
    let measured_at = now();
    let live = match &snap.live {
        LiveId::Known(n) => Live::Profile(n),
        _ => Live::Unowned,
    };
    let (reading, others) =
        measure::measure(&ctx.store, binary, live, &snap.candidates, measured_at);
    let measured: Vec<Measured> = others
        .iter()
        .map(|(name, reading)| Measured {
            name: name.clone(),
            reading: *reading,
        })
        .collect();
    let swapped_at = ctx.rs.load_ledger()?.and_then(|l| l.swapped_at);
    let t = now();
    let raw = decide(&ctx.settings, &reading, &measured, swapped_at, t);
    let failed = next_failed(ctx.failed_live, &reading);
    let (decision, note) = damp(raw, &reading, failed, swapped_at, t, &ctx.settings);
    Ok(Measurement {
        live: reading,
        others,
        decision,
        note,
        failed,
        measured_at,
    })
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
            // A real run refuses to go on from an inconsistent journal.
            Ok(Some(j)) if j.phase == Phase::Inconsistent => {
                out.emit(0, "would alert: rotate recovery is inconsistent");
                return Ok(CycleOutcome::Alerted);
            }
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
    let state = State::new(ctx.store.root())?;
    // Issue #1693: what hand-change recovery would do; it stops here.
    let env_for = || handfix::Env::system(ctx.store.root(), &state, ctx.scan);
    let lines = handfix::preview(&ctx.store, &ctx.rs, &ctx.home, env_for, now())?;
    if !lines.is_empty() {
        for l in &lines {
            out.emit(0, l);
        }
        return Ok(CycleOutcome::Alerted);
    }
    let snap = snapshot(ctx, &state)?;
    if let Some((_, line)) = live_alert(&snap.live) {
        let text = line.strip_prefix("ALERT: ").unwrap_or(&line);
        out.emit(0, &format!("would alert: {text}"));
        return Ok(CycleOutcome::Alerted);
    }
    match pin::gate(&snap.pin) {
        Gate::Open => {}
        Gate::Ignored(line) => out.emit(0, &line),
        // `skip: held by switch to NAME; run ...` -> `would stay (held by
        // switch to NAME)`.
        Gate::Held(line) => {
            let text = line.strip_prefix("skip: ").unwrap_or(&line);
            let why = text.split(';').next().unwrap_or(text);
            out.emit(0, &format!("would stay ({why})"));
            return Ok(CycleOutcome::Held);
        }
    }
    let binary = resolve_claude_binary()
        .map_err(|e| ProfileError::Io(std::io::Error::other(e.to_string())))?;
    let Measurement {
        live,
        others,
        decision,
        note,
        ..
    } = measure_and_decide(ctx, &binary, &snap)?;
    let name = live_name(&snap.live);
    out.emit(0, &msg::cycle(name, &live, &others));
    let outcome = would(&decision);
    match decision {
        Decision::Stay => match note {
            Some(n) => out.emit(
                0,
                &format!("would stay ({})", n.strip_prefix("stay: ").unwrap_or(&n)),
            ),
            None => out.emit(0, "would stay"),
        },
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

/// The ALERT key kind for a refused swap: its reason, without control
/// characters and short enough that the key passes the ledger's 320-byte
/// check with any profile name.
pub(super) fn refusal_kind(e: &ProfileError) -> String {
    let reason: String = e.to_string().chars().filter(|c| !c.is_control()).collect();
    let mut kind = String::from("refused ");
    for c in reason.chars() {
        if kind.len() + c.len_utf8() > 200 {
            break;
        }
        kind.push(c);
    }
    kind
}

/// This cycle's readings for the no-capacity file: the live profile's and
/// every other candidate's, as measured.
pub(super) fn rotated(
    live: Option<&ProfileName>,
    reading: &Reading,
    others: &[(ProfileName, Reading)],
) -> Vec<(ProfileName, Rotated)> {
    let of = |r: &Reading| match *r {
        Reading::Measured { session, week } => Rotated::Measured { session, week },
        Reading::Exhausted => Rotated::Exhausted,
        Reading::Failed => Rotated::Failed,
    };
    live.map(|n| (n.clone(), of(reading)))
        .into_iter()
        .chain(others.iter().map(|(n, r)| (n.clone(), of(r))))
        .collect()
}
