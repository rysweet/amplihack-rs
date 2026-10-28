//! Hand-change recovery (issue #1693). A manual `/login` in `~/.claude`
//! replaces whatever login was there, and the rotate ledger then names a
//! profile `L` whose account is no longer in `~/.claude.json`. When exactly
//! one profile `M` holds that account, `M` is recorded as live; if `M` also
//! has its own login, that copy is first renamed into a fresh
//! `<root>/.rejected-logins/rotate-<M>-<time>/`. Anything else gives exact
//! guidance, written every cycle until resolved. See "Rotate hand-change
//! recovery" in docs/reference/profile-command.md.
//!
//! Only `.claude.json` files and metadata are read; a login is moved only by
//! `move_login` (rename without replace) and never opened, copied or deleted.

use super::backup::{BackupRefusal, first_free, fresh_backup, rejected_dir, stamp};
use super::cycle::Ctx;
use super::guide::{self, Fix, Kind, Paths};
use super::identify::{self, Holders, LiveId};
use super::ledger::{MAX_GUIDANCE_LINE, RotateLedger, RotateState};
use super::log::{Sink, cut, msg};
use super::lost;
pub use super::precheck::Env;
use super::precheck::{move_check, record_check};
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
    /// No hand change (or no login in `~/.claude`): the cycle goes on.
    Clear,
    /// The holder was recorded as live: the cycle goes on with it.
    Recovered,
    /// Guidance was written: the cycle ends `alerted`.
    Guided,
}

/// The hand change: the profile the ledger names, whether it has no login
/// file, and the paths the guidance may name.
struct Case {
    l: ProfileName,
    l_lost: bool,
    paths: Paths,
}

enum Plan<'a> {
    /// Case A: record `m`; nothing moves.
    Record {
        case: Case,
        m: ProfileName,
        key: String,
    },
    /// Case B: move `m`'s own copy aside, then record `m`.
    Move {
        case: Case,
        m: ProfileName,
        key: String,
        env: Env<'a>,
    },
    /// The guidance, and L when its `fix 1` already logs it in: L then
    /// gets no lost-login line this cycle.
    Guide(Vec<String>, Option<ProfileName>),
}

impl Case {
    fn skip(&self) -> Option<ProfileName> {
        self.l_lost.then(|| self.l.clone())
    }
}

/// The guidance for a refused recovery.
fn refused_lines(case: &Case, m: &ProfileName, own_login: bool, why: (String, Fix)) -> Vec<String> {
    let kind = Kind::Refused {
        m,
        own_login,
        reason: &why.0,
        fix: why.1,
    };
    guide::lines(&case.l, case.l_lost, &kind, &case.paths)
}

fn refused<'a>(case: &Case, m: &ProfileName, own_login: bool, why: (String, Fix)) -> Plan<'a> {
    Plan::Guide(refused_lines(case, m, own_login, why), case.skip())
}

/// Decides without writing anything; `None` without a hand change. Checks
/// for one once, and calls `env_for` only after one.
fn plan<'a>(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    env_for: impl FnOnce() -> Env<'a>,
    t: i64,
) -> Result<Option<Plan<'a>>, ProfileError> {
    let LiveId::HandChanged(l) = identify::live(store, rs, home)?.0 else {
        return Ok(None);
    };
    let env = env_for();
    let key = identify::home_key(home);
    let holders = identify::home_holders(store, home)?;
    let case = Case {
        l_lost: lost::lost(store, l.as_str()),
        l,
        paths: Paths {
            home: home.to_path_buf(),
            root: store.root().to_path_buf(),
            ledger: rs.ledger.clone(),
            manual: store
                .root()
                .join(REJECTED_DIR)
                .join(format!("manual-{}", stamp(t))),
        },
    };
    let guide = |kind: Kind<'_>| {
        let lines = guide::lines(&case.l, case.l_lost, &kind, &case.paths);
        Ok(Some(Plan::Guide(lines, case.skip())))
    };
    let (m, own_login, key) = match (holders, key) {
        (Holders::One { name, own_login }, Some(key)) => (name, own_login, key),
        (Holders::One { .. } | Holders::HomeUnreadable, _) => return guide(Kind::HomeUnreadable),
        (Holders::HomeNoLogin, _) => return guide(Kind::HomeNoLogin),
        (Holders::None, _) => return guide(Kind::None),
        (Holders::Many(v), _) => return guide(Kind::Many(&v)),
        (Holders::Unreadable(v), _) => return guide(Kind::Unreadable(&v)),
    };
    if let Some(why) = record_check(store, rs, home) {
        return Ok(Some(refused(&case, &m, own_login, why)));
    }
    if !own_login {
        return Ok(Some(Plan::Record { case, m, key }));
    }
    if let Some(why) = move_check(store, home, &m, &env) {
        return Ok(Some(refused(&case, &m, own_login, why)));
    }
    Ok(Some(Plan::Move { case, m, key, env }))
}

/// Case B after the checks: create the backup, check `~/.claude.json`
/// again, move `m`'s copy. Each failure is a refusal; nothing else moved.
fn move_aside(
    store: &ProfileStore,
    home: &Path,
    m: &ProfileName,
    key: &str,
    env: &Env<'_>,
    t: i64,
) -> Result<PathBuf, (String, Fix)> {
    let by_hand = |r: BackupRefusal| (r.reason(), Fix::ByHand);
    let rejected = rejected_dir(&store.root().join(REJECTED_DIR)).map_err(by_hand)?;
    let dir = fresh_backup(&rejected, m, t).map_err(by_hand)?;
    abort_point("rotate-handfix-dir");
    (env.after_create)();
    if !identify::home_still_matches(home, key) {
        return Err((CHANGED.into(), Fix::Retry));
    }
    let (src, to) = (store.root().join(m.as_str()).join(CREDS), dir.join(CREDS));
    if let Err(e) = move_login(&src, &to, &swap::show) {
        let reason = match e {
            MoveError::Other(_) if !movable_login(&src) => LOGIN_NOT_MOVABLE.to_string(),
            e => swap::describe(e),
        };
        return Err((reason, Fix::ByHand));
    }
    abort_point("rotate-handfix-moved");
    Ok(to)
}

/// Records `m` as live in place of `case.l`; `done` is the recovery line.
/// When `case.l` is lost but `lost_login` is full, a one-off line says so.
fn record(
    l: &mut RotateLedger,
    case: &Case,
    m: &ProfileName,
    key: &str,
    t: i64,
    done: String,
) -> (Step, Vec<String>) {
    l.live = Some(m.to_string());
    l.account = Some(key.to_string());
    l.since = Some(t);
    l.alert = None;
    l.guidance = None;
    let mut lines = vec![done];
    if case.l != *m && case.l_lost {
        lines.extend(lost::add(l, &case.l));
    }
    (Step::Recovered, lines)
}

/// One cycle's hand-change step, under the cycle's lock: recovers when it
/// safely can, else writes guidance; then writes a line for each profile in
/// `lost_login`. The ledger is written only when it changed, and before any
/// line is written. `env_for` is called only after a hand change.
pub fn step_with<'a>(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    out: &mut Sink,
    env_for: impl FnOnce() -> Env<'a>,
    t: i64,
) -> Result<Step, ProfileError> {
    let plan = plan(store, rs, home, env_for, t)?;
    let mut ledger = rs.load_ledger()?.unwrap_or_default();
    let before = ledger.clone();
    lost::prune(&mut ledger, store);
    let mut skip = None;
    let (step, lines) = match plan {
        None => {
            ledger.guidance = None;
            (Step::Clear, Vec::new())
        }
        Some(Plan::Guide(lines, s)) => {
            skip = s;
            (Step::Guided, lines)
        }
        Some(Plan::Record { case, m, key }) => record(
            &mut ledger,
            &case,
            &m,
            &key,
            t,
            msg::recovered_record(&m, &case.l),
        ),
        Some(Plan::Move { case, m, key, env }) => {
            match move_aside(store, home, &m, &key, &env, t) {
                Ok(to) => {
                    let line = msg::recovered_moved(&m, &to);
                    record(&mut ledger, &case, &m, &key, t, line)
                }
                Err(why) => {
                    skip = case.skip();
                    (Step::Guided, refused_lines(&case, &m, true, why))
                }
            }
        }
    };
    if step == Step::Guided {
        ledger.guidance = Some(lines.clone());
    }
    if ledger != before {
        rs.save_ledger(&ledger)?;
    }
    for line in &lines {
        out.emit(t, &cut(line, MAX_GUIDANCE_LINE));
    }
    for line in lost::lines(&ledger, skip.as_ref()) {
        out.emit(t, &line);
    }
    Ok(step)
}

/// [`step_with`] on this host: `/proc` and the auto-switch ledger are read
/// only after a hand change.
pub fn step(ctx: &Ctx, state: &State, out: &mut Sink, t: i64) -> Result<Step, ProfileError> {
    let env_for = || Env::system(ctx.store.root(), state, ctx.scan);
    step_with(&ctx.store, &ctx.rs, &ctx.home, out, env_for, t)
}

/// The `--dry-run` lines: `would recover: ...`, or `would alert: ...` and
/// the `fix` lines; empty without a hand change. Takes no lock and writes
/// nothing. `env_for` is called only after a hand change.
pub fn preview<'a>(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    env_for: impl FnOnce() -> Env<'a>,
    t: i64,
) -> Result<Vec<String>, ProfileError> {
    let would = |line: String| {
        let text = line.strip_prefix("recovered: ").unwrap_or(&line);
        vec![format!("would recover: {text}")]
    };
    let Some(plan) = plan(store, rs, home, env_for, t)? else {
        return Ok(Vec::new());
    };
    let lines = match plan {
        Plan::Guide(lines, _) => lines,
        Plan::Record { case, m, .. } => return Ok(would(msg::recovered_record(&m, &case.l))),
        Plan::Move { case, m, .. } => match first_free(&store.root().join(REJECTED_DIR), &m, t) {
            Some(dir) => return Ok(would(msg::recovered_moved(&m, &dir.join(CREDS)))),
            None => {
                let why = (BackupRefusal::NoFreeName.reason(), Fix::ByHand);
                refused_lines(&case, &m, true, why)
            }
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
