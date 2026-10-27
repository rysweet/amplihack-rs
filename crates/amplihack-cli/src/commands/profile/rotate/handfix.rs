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

use super::backup::{first_free, fresh_backup, rejected_dir, stamp};
use super::cycle::Ctx;
use super::guide::{self, Fix, Kind, Paths};
use super::identify::{self, Holders, LiveId};
use super::ledger::{MAX_GUIDANCE_LINE, MAX_LOST_LOGIN, RotateLedger, RotateState};
use super::log::{Sink, cut, msg};
use super::swap::{self, DIFFERENT_DEVICES, LOGIN_NOT_MOVABLE};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CREDS, Ledger, Liveness, MoveError, REFRESH_LOCK, State, abort_point, check_root_private,
    exists, movable_login, move_login, scan_system,
};
use crate::commands::profile::login_check::REJECTED_DIR;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const STOPPED: &str = "an earlier swap stopped half way";
const LOCKED: &str = "Claude holds its refresh lock";
const UNSEEN: &str = "cannot see running sessions";
const NO_NAME: &str = "no free backup name";
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

/// What recovery may not find out by itself: running sessions, the
/// auto-switch ledger (`None`: unreadable), devices, and a hook run after
/// the backup directory is created.
pub struct Env<'a> {
    pub liveness: Liveness,
    pub auto_switch: Option<Ledger>,
    pub dev: &'a dyn Fn(&Path) -> Option<u64>,
    pub after_create: &'a dyn Fn(),
}

fn real_dev(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.dev())
}

fn nothing() {}

impl Env<'static> {
    /// This host's sessions, the auto-switch ledger and real devices.
    pub fn system(root: &Path, state: &State) -> Self {
        Self {
            liveness: scan_system(root),
            auto_switch: state.load_ledger().ok(),
            dev: &real_dev,
            after_create: &nothing,
        }
    }
}

/// The hand change: the profile the ledger names, whether it has no login
/// file, and the paths the guidance may name.
struct Case {
    l: ProfileName,
    l_lost: bool,
    paths: Paths,
}

enum Plan {
    Clear,
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
    },
    Guide(Vec<String>),
}

/// The profile directory exists and has nothing at `.credentials.json`.
fn lost(store: &ProfileStore, name: &str) -> bool {
    let dir = store.root().join(name);
    exists(&dir) && !exists(&dir.join(CREDS))
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

fn refused(case: &Case, m: &ProfileName, own_login: bool, why: (String, Fix)) -> Plan {
    Plan::Guide(refused_lines(case, m, own_login, why))
}

/// Decides without writing anything.
fn plan(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    env: &Env<'_>,
    t: i64,
) -> Result<Plan, ProfileError> {
    let LiveId::HandChanged(l) = identify::live(store, rs, home)?.0 else {
        return Ok(Plan::Clear);
    };
    let key = identify::home_key(home);
    let holders = identify::home_holders(store, home)?;
    let case = Case {
        l_lost: lost(store, l.as_str()),
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
    let guide =
        |kind: Kind<'_>| Plan::Guide(guide::lines(&case.l, case.l_lost, &kind, &case.paths));
    let (m, own_login, key) = match (holders, key) {
        (Holders::One { name, own_login }, Some(key)) => (name, own_login, key),
        (Holders::One { .. } | Holders::HomeUnreadable, _) => {
            return Ok(guide(Kind::HomeUnreadable));
        }
        (Holders::HomeNoLogin, _) => return Ok(guide(Kind::HomeNoLogin)),
        (Holders::None, _) => return Ok(guide(Kind::None)),
        (Holders::Many(v), _) => return Ok(guide(Kind::Many(&v))),
        (Holders::Unreadable(v), _) => return Ok(guide(Kind::Unreadable(&v))),
    };
    if let Some(why) = record_check(store, rs, home) {
        return Ok(refused(&case, &m, own_login, why));
    }
    if !own_login {
        return Ok(Plan::Record { case, m, key });
    }
    if let Some(why) = move_check(store, home, &m, env) {
        return Ok(refused(&case, &m, own_login, why));
    }
    Ok(Plan::Move { case, m, key })
}

/// The checks before recording: a private root, no pending journal, and
/// the swap's home checks.
fn record_check(store: &ProfileStore, rs: &RotateState, home: &Path) -> Option<(String, Fix)> {
    if let Err(e) = check_root_private(store.root()) {
        return Some((format!("profile root {e}"), Fix::Root));
    }
    if exists(&rs.journal) {
        return Some((STOPPED.into(), Fix::Recovery));
    }
    match swap::check_home(home) {
        Ok(_) => None,
        Err(e) => {
            let reason = match e {
                ProfileError::RotateRefused(r) => r,
                e => e.to_string(),
            };
            let fix = if reason.starts_with("HOME") {
                Fix::Home
            } else {
                Fix::DotClaude
            };
            Some((reason, fix))
        }
    }
}

/// The extra checks before `m`'s own copy is moved. Fails closed: sessions
/// that cannot be seen, an unreadable auto-switch ledger or an unknown
/// device each refuse.
fn move_check(
    store: &ProfileStore,
    home: &Path,
    m: &ProfileName,
    env: &Env<'_>,
) -> Option<(String, Fix)> {
    if env.liveness.is_live(m) {
        return Some(match env.liveness {
            Liveness::Known(_) => (format!("a claude session is running on {m}"), Fix::Close),
            Liveness::Everything | Liveness::Unsupported => (UNSEEN.into(), Fix::CloseThenMove),
        });
    }
    let Ok(dir) = store.verify_profile_dir(m) else {
        return Some((
            format!("profile {m} failed the directory check"),
            Fix::ByHand,
        ));
    };
    let dot = home.join(".claude");
    if exists(&dot.join(REFRESH_LOCK)) || exists(&dir.join(REFRESH_LOCK)) {
        return Some((LOCKED.into(), Fix::Wait));
    }
    let lent = env
        .auto_switch
        .as_ref()
        .is_none_or(|a| a.leased_to(m.as_str()).is_some() || a.hosts.contains_key(m.as_str()));
    if lent {
        return Some((
            format!("{m}'s login is lent out or it hosts another login"),
            Fix::Lease,
        ));
    }
    if !movable_login(&dir.join(CREDS)) {
        return Some((LOGIN_NOT_MOVABLE.into(), Fix::ByHand));
    }
    let devs = [dot.as_path(), dir.as_path(), store.root()].map(|p| (env.dev)(p));
    if devs[0].is_none() || devs.iter().any(|d| *d != devs[0]) {
        return Some((DIFFERENT_DEVICES.into(), Fix::ByHand));
    }
    None
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
    let rejected = store.root().join(REJECTED_DIR);
    if let Err(e) = rejected_dir(&rejected) {
        eprintln!("warning: {}: {e}", rejected.display());
        return Err((NO_NAME.into(), Fix::ByHand));
    }
    let dir = fresh_backup(&rejected, m, t).ok_or((NO_NAME.into(), Fix::ByHand))?;
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

/// Records `m` as live in place of `case.l`.
fn record(l: &mut RotateLedger, case: &Case, m: &ProfileName, key: &str, t: i64) {
    l.live = Some(m.to_string());
    l.account = Some(key.to_string());
    l.since = Some(t);
    l.alert = None;
    l.guidance = None;
    let name = case.l.to_string();
    if case.l != *m
        && case.l_lost
        && !l.lost_login.contains(&name)
        && l.lost_login.len() < MAX_LOST_LOGIN
    {
        l.lost_login.push(name);
    }
}

/// One cycle's hand-change step, under the cycle's lock: recovers when it
/// safely can, else writes guidance; then writes a line for each profile in
/// `lost_login`. The ledger is written only when it changed, and before any
/// line is written.
pub fn step_with(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    out: &mut Sink,
    env: &Env<'_>,
    t: i64,
) -> Result<Step, ProfileError> {
    let plan = plan(store, rs, home, env, t)?;
    let mut ledger = rs.load_ledger()?.unwrap_or_default();
    let before = ledger.clone();
    // A profile that has a login again, or is gone, leaves `lost_login`.
    ledger.lost_login.retain(|n| lost(store, n));
    let (step, lines) = match plan {
        Plan::Clear => {
            ledger.guidance = None;
            (Step::Clear, Vec::new())
        }
        Plan::Guide(lines) => (Step::Guided, lines),
        Plan::Record { case, m, key } => {
            record(&mut ledger, &case, &m, &key, t);
            (Step::Recovered, vec![msg::recovered_record(&m, &case.l)])
        }
        Plan::Move { case, m, key } => match move_aside(store, home, &m, &key, env, t) {
            Ok(to) => {
                record(&mut ledger, &case, &m, &key, t);
                (Step::Recovered, vec![msg::recovered_moved(&m, &to)])
            }
            Err(why) => (Step::Guided, refused_lines(&case, &m, true, why)),
        },
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
    for name in ledger
        .lost_login
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
    {
        out.emit(t, &msg::lost_login(&name));
    }
    Ok(step)
}

/// [`step_with`] on this host.
pub fn step(ctx: &Ctx, state: &State, out: &mut Sink, t: i64) -> Result<Step, ProfileError> {
    let env = Env::system(ctx.store.root(), state);
    step_with(&ctx.store, &ctx.rs, &ctx.home, out, &env, t)
}

/// The `--dry-run` lines: `would recover: ...`, or `would alert: ...` and
/// the `fix` lines; empty without a hand change. Takes no lock and writes
/// nothing.
pub fn preview(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    env: &Env<'_>,
    t: i64,
) -> Result<Vec<String>, ProfileError> {
    let would = |line: String| {
        let text = line.strip_prefix("recovered: ").unwrap_or(&line);
        vec![format!("would recover: {text}")]
    };
    let lines = match plan(store, rs, home, env, t)? {
        Plan::Clear => return Ok(Vec::new()),
        Plan::Guide(lines) => lines,
        Plan::Record { case, m, .. } => return Ok(would(msg::recovered_record(&m, &case.l))),
        Plan::Move { case, m, .. } => match first_free(&store.root().join(REJECTED_DIR), &m, t) {
            Some(dir) => return Ok(would(msg::recovered_moved(&m, &dir.join(CREDS)))),
            None => refused_lines(&case, &m, true, (NO_NAME.into(), Fix::ByHand)),
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
