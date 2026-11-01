//! `--dry-run`: the preview of a cycle, built from the same measurements and
//! decision without a lock or a write.

use super::cycle::{Ctx, CycleOutcome};
use super::decide::Decision;
use super::handfix;
use super::ledger::{Phase, spend_reset};
use super::log::{Sink, msg, timestamp};
use super::observe::{
    Measurement, live_alert, live_name, measure_and_decide, platform_block_reason, reading_of,
    snapshot,
};
use super::overflow_record::{alert_line, target_check};
use super::pin::{self, Gate as PinGate};
use super::swap;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, exists, now};
use crate::commands::profile::run::resolve_claude_binary;

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
        key_held,
        ..
    } = measure_and_decide(ctx, &binary, &snap)?;
    let name = live_name(&snap.live);
    out.emit(0, &msg::cycle(name, &live, &others, snap.shown_platform()));
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
            let (_, line) = alert_line(&snap, tag, &live, &others, &key_held, reason);
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
            // Issue #1804: the login check only reads files, so the preview
            // runs it too.
            if let Err(why) = target_check(ctx, &snap, &to) {
                out.emit(0, &msg::would_refuse(&to, &why));
                return Ok(CycleOutcome::Alerted);
            }
            let target = reading_of(&others, &to);
            out.emit(
                0,
                &format!("would {}", msg::swap(&from, &to, &live, &target, reason)),
            );
        }
    }
    Ok(outcome)
}
