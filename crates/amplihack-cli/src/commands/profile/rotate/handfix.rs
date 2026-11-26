//! Hand-change recovery (issues #1693 and #1886). A manual `/login` in
//! `~/.claude` replaces whatever login was there, and the rotate ledger then
//! names a profile `L` whose login is no longer the one in `~/.claude`.
//! Whose login it is, is told by the account the OAuth profile endpoint
//! reports for each credential (`cred_identity`), never by `.claude.json`:
//! running sessions rewrite that file from memory, so for minutes after a
//! swap it names the previous account. When `~/.claude` holds the same
//! account as exactly one other profile `M`'s own login on two cycles at
//! least an interval apart, and outside
//! `SWAP_GRACE`, `M`'s copy is renamed into a fresh
//! `<root>/.rejected-logins/rotate-<M>-<time>/` and `M` is recorded as live,
//! unless that would leave `L` with no login anywhere. Anything else writes
//! a note or guidance. See "Rotate hand-change recovery" in
//! docs/reference/profile-command.md.
//!
//! A login is moved only by `move_login` (rename without replace) and never
//! opened, copied or deleted here; the readings are `usage_direct`'s.

use super::backup::{BackupRefusal, discard_empty, first_free, fresh_backup, rejected_dir};
use super::cycle::Ctx;
use super::guide::{self, Fix, Kind};
use super::handfix_msg as msg;
pub use super::handfix_plan::Read;
use super::handfix_plan::{Confirmed, Input, Plan, ino_ctime, plan, unreadable_since};
use super::ledger::{MAX_GUIDANCE_LINE, RotateLedger, RotateState};
use super::log::{Sink, cut};
use super::lost;
pub use super::precheck::Env;
use super::swap::{self, LOGIN_NOT_MOVABLE};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CREDS, MoveError, State, abort_point, movable_login, move_login,
};
use crate::commands::profile::login_check::REJECTED_DIR;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::path::{Path, PathBuf};

const CHANGED: &str = "~/.claude changed during recovery";

/// What one cycle's hand-change step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing to report: the cycle goes on.
    Clear,
    /// A note (a first reading, or cannot tell): the cycle goes on, and an
    /// open suspicion then stops it as `hand-change`.
    Noted,
    /// The suspected profile was recorded as live: the cycle goes on with it.
    Recovered,
    /// Guidance was written: the cycle ends `alerted`.
    Guided,
}

/// What a confirmed move needs besides the plan.
struct Mover<'r> {
    store: &'r ProfileStore,
    home: &'r Path,
    read: Read<'r>,
    t: i64,
}

/// After the checks: read both logins again and check `M`'s directory,
/// then create the backup directory, then check that neither login file was
/// replaced since the readings, then move `M`'s copy. Each failure is a
/// refusal; nothing else moved. A refusal leaves no new directory: the
/// checks that can refuse run before it is made, and the one after it
/// removes it again (only while it is empty).
fn move_aside(
    c: &Confirmed<'_>,
    mv: &Mover<'_>,
) -> Result<(PathBuf, super::identify::Account), (String, Fix)> {
    let changed = || (CHANGED.to_string(), Fix::Retry);
    // SEC-9: each read ends within IDENTITY_READ_TIMEOUT, so the two
    // re-reads together take at most 10 seconds; with `assess` the cycle
    // holds its lock for at most IDENTITY_BUDGET + 3 reads, 45 seconds.
    // That bound holds in release builds and in debug builds with the
    // AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS knob unset (issue #1973).
    let here = (mv.read)(&mv.home.join(".claude")).map_err(|_| changed())?;
    let there = (mv.read)(&mv.store.root().join(c.m.as_str())).map_err(|_| changed())?;
    if !here.same(&there) {
        return Err(changed());
    }
    if mv.store.verify_profile_dir(&c.m).is_err() {
        return Err(changed());
    }
    let by_hand = |r: BackupRefusal| (r.reason(), Fix::ByHand);
    let rejected = rejected_dir(&mv.store.root().join(REJECTED_DIR)).map_err(by_hand)?;
    let dir = fresh_backup(&rejected, &c.m, mv.t).map_err(by_hand)?;
    abort_point("rotate-handfix-dir");
    (c.env.after_create)();
    let src = mv.store.root().join(c.m.as_str()).join(CREDS);
    // Last, just before the rename: neither file was replaced since the
    // readings this cycle.
    let now = [
        ino_ctime(&mv.home.join(".claude").join(CREDS)),
        ino_ctime(&src),
    ];
    if now != c.seen || now.iter().any(Option::is_none) {
        discard_empty(&dir);
        return Err(changed());
    }
    let to = dir.join(CREDS);
    if let Err(e) = move_login(&src, &to, &swap::show) {
        let reason = match e {
            MoveError::Other(_) if !movable_login(&src) => LOGIN_NOT_MOVABLE.to_string(),
            e => swap::describe(e),
        };
        discard_empty(&dir);
        return Err((reason, Fix::ByHand));
    }
    abort_point("rotate-handfix-moved");
    Ok((to, here))
}

/// Records `m` and its confirmed full identity; clears prior guidance and
/// suspicion. The grace and independent-reading requirements are unchanged.
fn record(l: &mut RotateLedger, m: &ProfileName, account: &super::identify::Account, t: i64) {
    l.live = Some(m.to_string());
    l.account = Some(account.key().to_string());
    l.login_identity = Some(super::login_identity::Identity::new(account));
    l.since = Some(t);
    super::alert::clear_alert(l);
    l.guidance = None;
    l.hand_suspect = None;
}

/// The guidance for a confirmed move that `move_aside` refused.
fn refused_lines(c: &Confirmed<'_>, why: (String, Fix)) -> Vec<String> {
    let kind = Kind::Refused {
        m: &c.m,
        reason: &why.0,
        fix: why.1,
    };
    guide::lines(&c.l, &kind, &c.paths)
}

/// One cycle's hand-change step, under the cycle's lock: recovers when it
/// safely can, else writes a note or guidance; then a line for each profile
/// in `lost_login`. The ledger is written only when it changed, and before
/// any line is written. A note identical to the ledger's `guidance` is not
/// written again (D9). `read` is the identity reader; `min_gap` the
/// rotation interval, at least 1. `env_for` is called only for a confirmed
/// suspicion.
#[allow(clippy::too_many_arguments)]
pub fn step_with<'a>(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    out: &mut Sink,
    env_for: impl FnOnce() -> Env<'a>,
    read: Read<'_>,
    min_gap: i64,
    t: i64,
) -> Result<Step, ProfileError> {
    let mut ledger = rs.load_ledger()?.unwrap_or_default();
    let before = ledger.clone();
    let input = Input {
        store,
        rs,
        home,
        ledger: &before,
        read,
        min_gap,
        t,
    };
    let (plan, unreadable) = plan(&input, env_for);
    ledger.home_unreadable_since =
        unreadable.then(|| unreadable_since(ledger.home_unreadable_since, t));
    lost::prune(&mut ledger, store);
    let (step, lines) = match plan {
        Plan::Nothing { reset } => {
            ledger.guidance = None;
            if reset {
                ledger.hand_suspect = None;
            }
            (Step::Clear, Vec::new())
        }
        Plan::Note { line, suspect } => {
            ledger.hand_suspect = suspect;
            let repeat = before.guidance.as_deref() == Some(std::slice::from_ref(&line));
            ledger.guidance = Some(vec![line.clone()]);
            (Step::Noted, if repeat { Vec::new() } else { vec![line] })
        }
        Plan::Guide { lines, suspect } => {
            ledger.hand_suspect = suspect;
            ledger.guidance = Some(lines.clone());
            (Step::Guided, lines)
        }
        Plan::Move(c) => {
            let mv = Mover {
                store,
                home,
                read,
                t,
            };
            match move_aside(&c, &mv) {
                Ok((to, account)) => {
                    record(&mut ledger, &c.m, &account, t);
                    (Step::Recovered, vec![msg::recovered_moved(&c.m, &to)])
                }
                Err(why) => {
                    let lines = refused_lines(&c, why);
                    ledger.guidance = Some(lines.clone());
                    (Step::Guided, lines)
                }
            }
        }
    };
    if ledger != before {
        rs.save_ledger(&ledger)?;
    }
    for line in &lines {
        out.emit(t, &cut(line, MAX_GUIDANCE_LINE));
    }
    for line in lost::lines(&ledger) {
        out.emit(t, &line);
    }
    Ok(step)
}

/// The rotation interval as the two-reading rule's gap: at least 1 second.
pub fn min_gap(ctx: &Ctx) -> i64 {
    i64::try_from(ctx.settings.interval)
        .unwrap_or(i64::MAX)
        .max(1)
}

/// [`step_with`] on this host: `/proc` and the auto-switch ledger are read
/// only for a confirmed suspicion; the readings are `ctx.identity`'s.
pub fn step(ctx: &Ctx, state: &State, out: &mut Sink, t: i64) -> Result<Step, ProfileError> {
    let env_for = || Env::system(ctx.store.root(), state, ctx.scan);
    let read = |p: &Path| ctx.identity_reader().oauth(p);
    let (store, rs, home) = (&ctx.store, &ctx.rs, &ctx.home);
    step_with(store, rs, home, out, env_for, &read, min_gap(ctx), t)
}

/// Whether `--dry-run` stops after the preview's lines: everything but a
/// cannot-tell note does, as the real cycle would.
pub fn dry_run_stops(lines: &[String]) -> bool {
    let unsure = format!("would note: {}", msg::UNSURE_HEAD);
    !lines.is_empty() && !lines.iter().all(|l| l.starts_with(&unsure))
}

/// The `--dry-run` lines: `would recover: ...`, `would note: ...`, or
/// `would alert: ...` and the `fix` lines; empty when nothing would be
/// written. Takes no lock and writes nothing, so it never starts or
/// confirms a suspicion: it applies the rules to the ledger as it stands.
pub fn preview<'a>(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    env_for: impl FnOnce() -> Env<'a>,
    read: Read<'_>,
    min_gap: i64,
    t: i64,
) -> Result<Vec<String>, ProfileError> {
    let ledger = rs.load_ledger()?.unwrap_or_default();
    let input = Input {
        store,
        rs,
        home,
        ledger: &ledger,
        read,
        min_gap,
        t,
    };
    let lines = match plan(&input, env_for).0 {
        Plan::Nothing { .. } => return Ok(Vec::new()),
        Plan::Note { line, .. } => {
            let text = line.strip_prefix("note: ").unwrap_or(&line);
            return Ok(vec![format!("would note: {text}")]);
        }
        Plan::Guide { lines, .. } => lines,
        Plan::Move(c) => match first_free(&store.root().join(REJECTED_DIR), &c.m, t) {
            Some(dir) => {
                let line = msg::recovered_moved(&c.m, &dir.join(CREDS));
                let text = line.strip_prefix("recovered: ").unwrap_or(&line);
                return Ok(vec![format!("would recover: {text}")]);
            }
            None => refused_lines(&c, (BackupRefusal::NoFreeName.reason(), Fix::ByHand)),
        },
    };
    Ok(lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| match (i, l.strip_prefix("ALERT: ")) {
            (0, Some(text)) => format!("would alert: {text}"),
            _ => l,
        })
        .collect())
}
