//! What a cycle sees: who is live and who may receive the login, the
//! measurements and the decision, and the `--dry-run` preview built from
//! them without a lock or a write.

use super::cycle::{Ctx, CycleOutcome};
use super::damp::{damp, next_failed};
use super::decide::{Decision, Measured, Probed, Reading};
use super::handfix;
use super::identify::{self, Account, CandidateInput, LiveId, LoginKind, Skip};
use super::ledger::{Phase, spend_reset};
use super::log::{Sink, msg, timestamp};
use super::measure::{self, Live};
use super::overflow::{OverflowInput, Tag, decide_overflow};
use super::overflow_record::alert_line;
use super::pin::{self, Gate as PinGate, PinState};
use super::swap;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::CREDS;
use crate::commands::profile::auto_switch::kit::{Liveness, State, effective, exists, now};
use crate::commands::profile::kind;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{OverflowEntry, PLATFORM_TIER, tiers_detail};
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
    /// The overflow entries with their tiers, in file order. Part of the
    /// snapshot, so a mark or tier changed during measurement skips the
    /// cycle. The API-key profile (issue #1700) is the entry at
    /// [`PLATFORM_TIER`].
    pub overflow: Vec<OverflowEntry>,
    pub pin: PinState,
    /// API-key profiles listed in `overflow=`, which are ignored there.
    pub ignored: Vec<ProfileName>,
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
    let config = effective(root).config;
    let (overflow, ignored) = tiers_detail(&ctx.store, Some(&ctx.home));
    let input = CandidateInput {
        live: live_name.as_ref(),
        exclude: config.exclude,
        liveness,
        auto_switch: state.load_ledger()?,
    };
    let (mut candidates, skips) = identify::candidates(&ctx.store, &input)?;
    // Only the first API-key profile is ever a target: the others share its
    // credential and are never probed or treated as plan logins.
    let platforms = kind::Ctx::load(&ctx.store, Some(&ctx.home)).platform_profiles(&ctx.store);
    candidates.retain(|c| !platforms.contains(c) || platforms.first() == Some(c));
    let home_no_oauth =
        identify::login_kind(&ctx.home.join(".claude").join(CREDS)) == LoginKind::NoOAuth;
    Ok(Snap {
        account: Account::of_dir(&ctx.home).map(|a| a.key().to_string()),
        candidate_accounts: super::revalidate::accounts(&ctx.store, &candidates),
        live,
        proto,
        candidates,
        skips,
        sees_sessions,
        overflow,
        pin: pin::read(&ctx.rs),
        ignored,
        home_no_oauth,
    })
}

pub fn live_name(id: &LiveId) -> Option<&ProfileName> {
    match id {
        LiveId::Known(n) | LiveId::HandChanged(n) | LiveId::HomeUnreadable(n) => Some(n),
        _ => None,
    }
}

/// The ALERT for a live login that cannot be used, if any.
pub fn live_alert(snap: &Snap) -> Option<(&'static str, String)> {
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

pub fn reading_of(others: &[(ProfileName, Reading)], name: &ProfileName) -> Reading {
    others
        .iter()
        .find(|(n, _)| n == name)
        .map_or(Reading::Failed, |(_, r)| *r)
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
    /// When measuring started, epoch seconds.
    pub measured_at: i64,
    /// The probes decided on, for step 6 to re-decide a swap whose target
    /// left the candidates (#1721); `None` from a stub, which disables that.
    /// `Probed` must never hold a token or an account identifier.
    pub probed: Option<Probes>,
}

pub fn measure_and_decide(
    ctx: &Ctx,
    binary: &crate::binary_finder::BinaryInfo,
    snap: &Snap,
) -> Result<Measurement, ProfileError> {
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
    let (probe, mut probes) = measure::measure(&ctx.store, binary, live, &to_probe, measured_at);
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
    // A ledger that cannot be read ends the cycle here, before any move onto
    // credits: its spend blocks are unknown. Only a missing ledger means no
    // blocks.
    let ledger = ctx.rs.load_ledger()?;
    let swapped_at = ledger.as_ref().and_then(|l| l.swapped_at);
    let platform_block = ledger.as_ref().and_then(|l| l.platform_block.clone());
    let blocks = ledger.map(|l| l.overflow_blocked).unwrap_or_default();
    let live_name = match &snap.live {
        LiveId::Known(n) => Some(n),
        _ => None,
    };
    let t = now();
    let verdict = decide_overflow(&OverflowInput {
        settings: &ctx.settings,
        live_name,
        live: &probe,
        candidates: &measured,
        overflow: &snap.overflow,
        blocks: &blocks,
        platform_block: platform_block.as_ref(),
        swapped_at,
        now: t,
    });
    // A spend limit on a live overflow profile is handled by its block, so
    // it does not count towards the failed live probes.
    let live_over = live_name.is_some_and(|n| snap.overflow.iter().any(|e| &e.name == n));
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
    // A damped move off credits is a stay on them.
    let tag = match (&decision, verdict.tag) {
        (Decision::Stay, Some(Tag::Out)) => Some(Tag::On),
        (_, tag) => tag,
    };
    let others = probes.iter().map(|(n, p)| (n.clone(), p.reading)).collect();
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
        measured_at,
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
    if let Some((_, line)) = live_alert(&snap) {
        let text = line.strip_prefix("ALERT: ").unwrap_or(&line);
        out.emit(0, &format!("would alert: {text}"));
        return Ok(CycleOutcome::Alerted);
    }
    match pin::gate(&snap.pin) {
        PinGate::Open => {}
        PinGate::Ignored(line) => out.emit(0, &line),
        // `skip: held by switch to NAME; run ...` -> `would stay (held by
        // switch to NAME)`.
        PinGate::Held(line) => {
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
        new_blocks,
        tag,
        ..
    } = measure_and_decide(ctx, &binary, &snap)?;
    let name = live_name(&snap.live);
    out.emit(0, &msg::cycle(name, &live, &others, snap.platform()));
    let t = now();
    for n in &new_blocks {
        out.emit(
            0,
            &format!(
                "would block {n} (spend limit) until {}",
                timestamp(spend_reset(t))
            ),
        );
    }
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
            let reason = || platform_block_reason(ctx);
            let (_, line) = alert_line(&snap, tag, &live, &others, reason);
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
