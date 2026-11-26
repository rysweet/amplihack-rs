//! `--dry-run`: the preview of a cycle, built from the same measurements and
//! decision without a lock or a write. Issue #1962: read-only on every read
//! path, so it never refreshes a token and never starts `claude`.

use super::cycle::event_read::measure_and_decide;
use super::cycle::{Ctx, CycleOutcome};
use super::decide::{Decision, target_msg};
use super::fresh::{self, Reread};
use super::handfix;
use super::identify::excluded_offline;
use super::ledger::{Phase, spend_reset};
use super::log::{Sink, msg, timestamp};
use super::observe::{Measurement, live_alert, live_name, platform_block_reason, snapshot};
use super::overflow_record::target_check;
use super::pin::{self, Gate as PinGate};
use super::prelude::ledger_stop;
use super::seed;
use super::swap;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, exists, now};
use crate::commands::profile::usage_direct;

/// The outcome `--dry-run` reports for `decision`, matching the line it
/// printed. It is not always what a live cycle would record: a live stay
/// without a fresh reading records `stayed-unread`, and a live advisory ALERT
/// records `stayed`, while this returns `Stayed` and `Alerted`. Nothing is
/// recorded and the exit code does not depend on it.
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
    ctx.identity_budget.reset();
    // Issue #2218: as a real cycle does, before any identity read.
    let dirs = excluded_offline::local_only_dirs(&ctx.store, &ctx.rs, &ctx.home)?;
    ctx.set_excluded_dirs(dirs);
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
    let now_ms = chrono::Utc::now().timestamp_millis();
    if usage_direct::expired(&ctx.home.join(".claude"), now_ms) {
        out.emit(0, "would refresh: expired login in ~/.claude");
    }
    // Issue #1962: as a real cycle stops, after what its refresh would do.
    if let Err(e) = ctx.rs.load_ledger() {
        out.emit(0, &format!("would alert: {}", ledger_stop(ctx, &e)));
        return Ok(CycleOutcome::Alerted);
    }
    // Issues #1693, #1886: what hand-change recovery would do, from the same
    // identity reads; it stops here unless it could only not tell.
    let env_for = || handfix::Env::system(ctx.store.root(), &state, ctx.scan);
    let read = |p: &std::path::Path| ctx.identity_reader().oauth(p);
    let (store, rs, home) = (&ctx.store, &ctx.rs, &ctx.home);
    let gap = handfix::min_gap(ctx);
    // Issue #2218: as a real cycle, no recovery on local evidence alone.
    let lines = if ctx.home_local_only() {
        Vec::new()
    } else {
        handfix::preview(store, rs, home, env_for, &read, gap, now())?
    };
    for l in &lines {
        out.emit(0, l);
    }
    if handfix::dry_run_stops(&lines) {
        return Ok(CycleOutcome::Alerted);
    }
    let snap = snapshot(ctx, &state)?;
    if !ctx.home_local_only() && seed::preview(ctx, &state, &snap, &mut out) {
        return Ok(CycleOutcome::Alerted);
    }
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
    // Issue #2218: where a real cycle pauses on an unreadable exclude list.
    if let Some(alert) = excluded_offline::exclude_paused(&ctx.store, |w| out.emit(0, w)) {
        out.emit(0, &format!("would alert: {alert}"));
        return Ok(CycleOutcome::Alerted);
    }
    // Tests stub the measurement as for a real cycle.
    ctx.live_outlook = None;
    let m = match ctx.measure {
        Some(stub) => stub(ctx, &snap)?,
        None => measure_and_decide(ctx, &snap)?,
    };
    // Issue #1870: the same re-reads as a real cycle; no rotate state is written.
    let m = fresh::live::step(ctx, &snap, m)?;
    // Issue #2889: as in a real cycle, the line reports the live figures the
    // preview decided on, after the live step and before the target gate.
    let name = live_name(&snap.live);
    let (live_shown, others_shown) = super::log::shown(&m, snap.live_excluded, (ctx.now)());
    out.emit(
        0,
        &msg::cycle(name, &live_shown, &others_shown, snap.shown_platform()),
    );
    let m = fresh::gate(ctx, &snap, m, Reread::Allowed)?;
    for line in &m.fresh_lines {
        out.emit(0, line);
    }
    let (live_shown, others_shown) = super::log::shown(&m, snap.live_excluded, (ctx.now)());
    let Measurement {
        live,
        decision,
        note,
        new_blocks,
        tag,
        key_held,
        seen,
        ..
    } = m;
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
    // A stay note is printed as `would stay (<the note without its prefix>)`,
    // whether it came from damping or from the issue #2910 ALERT branch.
    let would_stay = |t: &str| format!("would stay ({})", t.strip_prefix("stay: ").unwrap_or(t));
    match decision {
        Decision::Stay => match note {
            Some(n) => out.emit(0, &would_stay(&n)),
            None => out.emit(0, "would stay"),
        },
        Decision::Alert => {
            let reason = || platform_block_reason(ctx);
            let s = &ctx.settings;
            let line = target_msg::alert_line(&snap, tag, &live, &key_held, &seen, s, reason);
            let text = &line.text;
            // Issue #2910 (D10): a stay note is a stay, not an ALERT.
            let shown = if line.stay {
                would_stay(text)
            } else {
                format!(
                    "would alert: {}",
                    text.strip_prefix("ALERT: ").unwrap_or(text)
                )
            };
            out.emit(0, &shown);
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
            let shown = super::log::shown_of(&others_shown, &to);
            let line = target_msg::swap_line(&seen, &from, &to, &live_shown, shown, reason);
            out.emit(0, &format!("would {line}"));
        }
    }
    Ok(outcome)
}
