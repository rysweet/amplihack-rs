//! `amplihack profile verify [--fix]` (issue #1886, owner decision point 3):
//! for `~/.claude` and every profile folder, the account Anthropic's OAuth
//! profile endpoint reports for the folder's own token, set against the
//! account recorded for it (#1845's expected account). Run once after the
//! #1886 fix to find and undo names crossed by the 3 November misfires.
//!
//! Reads are `cred_identity`'s: no refresh, no process, no `.claude.json`,
//! no reset times. With `--fix`, under the rotate lock and never during a
//! swap, a profile folder whose login is another profile's recorded account
//! has that login moved (renamed, never copied or replaced) into the folder
//! recorded for it, but only when that folder holds no login at all. Before
//! the first move, a private `<root>/.rejected-logins/verify-<time>/moves`
//! file lists every move and the command that undoes it; the backup is that
//! list, not a copy of a login, because a copy of a login is a second holder
//! of its refresh token. `~/.claude` is reported, never moved: a swap or
//! `profile switch` changes it. See "profile verify" in
//! docs/reference/profile-command.md.

use super::ProfileError;
use super::auto_switch::kit::{CREDS, MoveError, State, exists, move_login};
use super::expected::Record;
use super::login_check::{REJECTED_DIR, create_fresh_dir};
use super::name::ProfileName;
use super::rotate::backup::{rejected_dir, stamp};
use super::rotate::cred_identity::record_of;
use super::rotate::identify::{Account, LoginKind, login_kind};
use super::rotate::ledger::RotateState;
use super::store::ProfileStore;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long `--fix` waits for the rotate lock.
pub const LOCK_WAIT: Duration = Duration::from_secs(2);

/// Reads the account of the login in a directory; `cred_identity::read_direct`
/// outside tests.
pub type Read<'r> = &'r dyn Fn(&Path) -> Result<Account, &'static str>;

/// Where a login was looked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// `~/.claude`, and the profile the rotate ledger names as live.
    Home {
        live: Option<ProfileName>,
    },
    Profile(ProfileName),
}

/// What a folder holds.
#[derive(Debug)]
pub enum Held {
    NoLogin,
    /// A login file that is not an OAuth login, or that cannot be used.
    NotOAuth,
    /// An OAuth login the server did not answer for; a `usage_direct::reason`.
    Unread(&'static str),
    Account(Account),
}

/// One line of the report.
#[derive(Debug)]
pub struct Row {
    pub place: Place,
    pub held: Held,
    /// The record of the profile itself, or of the live profile for `~/.claude`.
    pub record: Option<Record>,
}

/// A planned `--fix` move: `from`'s own login into `to`'s empty folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub from: ProfileName,
    pub to: ProfileName,
}

/// What `--fix` decided: the moves, and one line per mismatch it refused.
#[derive(Debug, Default)]
pub struct FixPlan {
    pub moves: Vec<Move>,
    pub refused: Vec<String>,
}

fn sorted_names(store: &ProfileStore) -> Result<Vec<ProfileName>, ProfileError> {
    let mut names: Vec<ProfileName> = store
        .list()?
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect();
    names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    Ok(names)
}

fn held_in(dir: &Path, read: Read<'_>) -> Held {
    match login_kind(&dir.join(CREDS)) {
        LoginKind::Missing => Held::NoLogin,
        LoginKind::NoOAuth | LoginKind::Unreadable => Held::NotOAuth,
        LoginKind::OAuth => match read(dir) {
            Ok(a) => Held::Account(a),
            Err(why) => Held::Unread(why),
        },
    }
}

/// Reads `~/.claude` and then every profile folder in name order. A folder
/// that fails the spawn-time directory check is reported as unread.
pub fn survey(store: &ProfileStore, home: &Path, read: Read<'_>) -> Result<Vec<Row>, ProfileError> {
    let live = RotateState::new(store.root())
        .load_ledger()
        .ok()
        .flatten()
        .and_then(|l| l.live)
        .and_then(|s| ProfileName::parse(&s).ok());
    let mut rows = vec![Row {
        record: live.as_ref().and_then(|l| record_of(store, l)),
        held: held_in(&home.join(".claude"), read),
        place: Place::Home { live },
    }];
    for name in sorted_names(store)? {
        let held = match store.verify_profile_dir(&name) {
            Ok(dir) => held_in(&dir, read),
            Err(_) => Held::Unread("folder refused"),
        };
        rows.push(Row {
            record: record_of(store, &name),
            held,
            place: Place::Profile(name),
        });
    }
    Ok(rows)
}

/// The profiles whose record names `a`, in name order.
fn recorded_for<'r>(rows: &'r [Row], a: &Account) -> Vec<&'r ProfileName> {
    rows.iter()
        .filter_map(|r| match &r.place {
            Place::Profile(n) if r.record.as_ref().is_some_and(|rec| rec.names(a)) => Some(n),
            _ => None,
        })
        .collect()
}

fn names(v: &[&ProfileName]) -> String {
    v.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ")
}

/// Whether the row holds an account its record does not name.
pub fn mismatched(row: &Row) -> bool {
    matches!((&row.held, &row.record), (Held::Account(a), Some(rec)) if !rec.names(a))
}

/// One line per row: `<place> -> <account>: <status>`.
pub fn render(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|r| line(rows, r)).collect()
}

fn line(rows: &[Row], r: &Row) -> String {
    let place = match &r.place {
        Place::Home { live: Some(l) } => format!("~/.claude (live {l})"),
        Place::Home { live: None } => "~/.claude".to_string(),
        Place::Profile(n) => n.to_string(),
    };
    let a = match &r.held {
        Held::NoLogin => return format!("{place} -> no login"),
        Held::NotOAuth => return format!("{place} -> not an OAuth login"),
        Held::Unread(why) => return format!("{place} -> not read ({why})"),
        Held::Account(a) => a,
    };
    let owners = recorded_for(rows, a);
    let elsewhere = match owners.as_slice() {
        [] => "recorded for no profile".to_string(),
        v => format!("recorded for {}", names(v)),
    };
    let status = match (&r.record, &r.place) {
        (Some(rec), _) if rec.names(a) => "ok".to_string(),
        (Some(_), _) => format!("MISMATCH, {elsewhere}"),
        (None, Place::Home { live: None }) => format!("no live profile, {elsewhere}"),
        (None, _) => format!("no record, {elsewhere}"),
    };
    format!("{place} -> {}: {status}", a.shown())
}

/// The `--fix` plan. Only a profile folder whose login is the recorded
/// account of exactly one other profile `T` is moved, and only when `T` is
/// not live and its folder holds no login file at all and no other move
/// goes there. Every other mismatch is a refusal line.
pub fn plan_fix(rows: &[Row]) -> FixPlan {
    let mut plan = FixPlan::default();
    let live = rows.iter().find_map(|r| match &r.place {
        Place::Home { live } => live.clone(),
        Place::Profile(_) => None,
    });
    for r in rows.iter().filter(|r| mismatched(r)) {
        let (Place::Profile(from), Held::Account(a)) = (&r.place, &r.held) else {
            plan.refused
                .push("~/.claude: not moved by --fix; use amplihack profile switch".into());
            continue;
        };
        let refuse = |why: String| format!("{from}: not moved: {why}");
        let to = match recorded_for(rows, a).as_slice() {
            [] => {
                plan.refused
                    .push(refuse("its account is recorded for no profile".into()));
                continue;
            }
            [one] => (*one).clone(),
            many => {
                plan.refused.push(refuse(format!(
                    "its account is recorded for several profiles ({})",
                    names(many)
                )));
                continue;
            }
        };
        let target = rows.iter().find(|t| t.place == Place::Profile(to.clone()));
        let why = if live.as_ref() == Some(&to) {
            Some(format!("{to} is live; its login is in ~/.claude"))
        } else {
            match target.map(|t| &t.held) {
                Some(Held::NoLogin) => None,
                Some(Held::Account(b)) if b.same(a) => {
                    Some(format!("{to} already holds that account"))
                }
                Some(Held::Account(_)) => Some(format!("{to} holds a different account")),
                _ => Some(format!("{to} holds a login that could not be read")),
            }
        };
        if let Some(why) = why {
            plan.refused.push(refuse(why));
        } else if plan.moves.iter().any(|m| m.to == to) {
            plan.refused
                .push(refuse(format!("another login is moving to {to}")));
        } else {
            plan.moves.push(Move {
                from: from.clone(),
                to,
            });
        }
    }
    plan
}

/// `p` in single quotes for a POSIX shell.
fn quoted(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

/// Writes the backup list of `moves` into a fresh private directory under
/// `<root>/.rejected-logins` and returns its path.
fn write_backup(store: &ProfileStore, moves: &[Move], t: i64) -> Result<PathBuf, ProfileError> {
    let fail = |e: String| ProfileError::Verify(format!("backup not written: {e}"));
    let rejected = rejected_dir(&store.root().join(REJECTED_DIR)).map_err(|r| fail(r.reason()))?;
    let dir = (1..=99)
        .map(|k| match k {
            1 => rejected.join(format!("verify-{}", stamp(t))),
            k => rejected.join(format!("verify-{}-{k}", stamp(t))),
        })
        .find(|d| create_fresh_dir(d).is_ok())
        .ok_or_else(|| fail("no free backup name".into()))?;
    let mut text =
        String::from("# profile verify --fix: each move, then the command that undoes it\n");
    for m in moves {
        let (src, dst) = (
            store.root().join(m.from.as_str()),
            store.root().join(m.to.as_str()),
        );
        text.push_str(&format!(
            "# {} -> {}\nmv -n {} {}\n",
            m.from,
            m.to,
            quoted(&dst.join(CREDS)),
            quoted(&src.join(CREDS))
        ));
    }
    let list = dir.join("moves");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&list)
        .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
        .map_err(|e| fail(e.to_string()))?;
    Ok(list)
}

/// Carries out `plan` under the lock the caller holds: writes the backup
/// list, then for each move reads `from` again and moves its login only if
/// it is still the account surveyed and `to` still has no login file. One
/// line per move done or skipped.
pub fn apply(
    store: &ProfileStore,
    rows: &[Row],
    plan: &FixPlan,
    read: Read<'_>,
    t: i64,
) -> Result<(usize, Vec<String>), ProfileError> {
    if plan.moves.is_empty() {
        return Ok((0, Vec::new()));
    }
    let list = write_backup(store, &plan.moves, t)?;
    let mut out = vec![format!(
        "backup: the moves and how to undo them are in {}",
        list.display()
    )];
    let mut moved = 0;
    for m in &plan.moves {
        let surveyed = rows.iter().find_map(|r| match (&r.place, &r.held) {
            (Place::Profile(n), Held::Account(a)) if *n == m.from => Some(a),
            _ => None,
        });
        let (from_dir, to_dir) = match (
            store.verify_profile_dir(&m.from),
            store.verify_profile_dir(&m.to),
        ) {
            (Ok(f), Ok(t)) => (f, t),
            _ => {
                out.push(format!("{}: not moved: a folder changed", m.from));
                continue;
            }
        };
        let unchanged = matches!((read(&from_dir), surveyed), (Ok(a), Some(b)) if a.same(b));
        if !unchanged {
            out.push(format!(
                "{}: not moved: its login changed since it was read",
                m.from
            ));
            continue;
        }
        if exists(&to_dir.join(CREDS)) {
            out.push(format!("{}: not moved: {} now holds a login", m.from, m.to));
            continue;
        }
        match move_login(&from_dir.join(CREDS), &to_dir.join(CREDS), &|p| {
            p.display().to_string()
        }) {
            Ok(()) => {
                moved += 1;
                out.push(format!("moved {}'s login to {}", m.from, m.to));
            }
            Err(MoveError::Exists) => {
                out.push(format!("{}: not moved: {} now holds a login", m.from, m.to))
            }
            Err(MoveError::Unsupported) => out.push(format!(
                "{}: not moved: the filesystem cannot rename the login into {}",
                m.from, m.to
            )),
            Err(MoveError::Other(why)) => out.push(format!("{}: not moved: {why}", m.from)),
        }
    }
    Ok((moved, out))
}

/// What the command found, for its exit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub mismatches: usize,
    pub moved: usize,
}

/// The command: prints the report and, with `fix`, the moves. Holds the
/// rotate lock for the whole run with `fix`, and refuses during a swap.
pub fn run_with(
    store: &ProfileStore,
    home: &Path,
    fix: bool,
    read: Read<'_>,
    t: i64,
    out: &mut dyn FnMut(&str),
) -> Result<Summary, ProfileError> {
    let state = State::new(store.root())?;
    let _lock = if fix {
        let Some(lock) = state.try_lock(LOCK_WAIT)? else {
            return Err(ProfileError::Verify(
                "the rotate lock is busy; try again".into(),
            ));
        };
        if exists(&RotateState::new(store.root()).journal) {
            return Err(ProfileError::Verify(
                "a swap is in progress; try again after the next rotate cycle".into(),
            ));
        }
        Some(lock)
    } else {
        None
    };
    let rows = survey(store, home, read)?;
    for l in render(&rows) {
        out(&l);
    }
    let mismatches = rows.iter().filter(|r| mismatched(r)).count();
    if !fix {
        return Ok(Summary {
            mismatches,
            moved: 0,
        });
    }
    let plan = plan_fix(&rows);
    let (moved, lines) = apply(store, &rows, &plan, read, t)?;
    for l in lines.iter().chain(&plan.refused) {
        out(l);
    }
    Ok(Summary { mismatches, moved })
}

/// [`run_with`] with the real home, the server reader and the clock,
/// printing to stdout.
pub fn run(store: &ProfileStore, fix: bool) -> Result<Summary, ProfileError> {
    let home = super::usage_cmd::home_dir()
        .ok_or_else(|| ProfileError::Verify("the home directory is not known".into()))?;
    let read = super::rotate::cred_identity::read_direct;
    let t = super::auto_switch::kit::now();
    run_with(store, &home, fix, &read, t, &mut |l| println!("{l}"))
}
