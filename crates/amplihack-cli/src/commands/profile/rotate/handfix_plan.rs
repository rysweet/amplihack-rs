//! What one cycle's hand-change step would do (issues #1693 and #1886),
//! decided without writing anything, so the real step and `--dry-run` share
//! it. Order: the stuck `~/.claude.json` check, the gates of "When recovery
//! reads", the credential readings ([`assess`]), the two-reading rule, then
//! the checks, the only-login rule and the move checks. Only a confirmed
//! "Suspect M" that passes every check plans a move. `.claude.json` never
//! causes a move: an unreadable one (the #1702 check) can only postpone the
//! step, and its contents are not read here. See "Rotate hand-change
//! recovery" in docs/reference/profile-command.md.

use super::account::home_readable;
use super::backup::stamp;
use super::cred_identity::{Finding, IDENTITY_BUDGET, Why, assess};
use super::guide::{self, Kind, Paths};
use super::hand_fields::{HOME_UNREADABLE_GRACE, HandSuspect, SWAP_GRACE};
use super::handfix_msg as msg;
use super::identify::{self, Account, LoginKind, login_kind};
use super::ledger::{RotateLedger, RotateState};
use super::precheck::{Env, move_check, record_check};
use crate::commands::profile::auto_switch::kit::{CREDS, exists};
use crate::commands::profile::login_check::REJECTED_DIR;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// The reader the step uses: `<home>/.claude` or `<root>/<name>` in, the
/// account the server reports or a `usage_direct::reason` out.
pub type Read<'r> = &'r dyn Fn(&Path) -> Result<Account, &'static str>;

/// What the plan reads: the loaded ledger, the reader, the rotation
/// interval (`min_gap`, at least 1) and the cycle's time.
pub struct Input<'r> {
    pub store: &'r ProfileStore,
    pub rs: &'r RotateState,
    pub home: &'r Path,
    pub ledger: &'r RotateLedger,
    pub read: Read<'r>,
    pub min_gap: i64,
    pub t: i64,
}

/// The device, inode and change time (seconds, nanoseconds) of a login
/// file, by `lstat`. The change time catches a file replaced by one that
/// reuses the freed inode number.
pub type Seen = Option<(u64, u64, i64, i64)>;

/// A confirmed suspicion that passed every check before the move.
pub struct Confirmed<'a> {
    pub l: ProfileName,
    pub m: ProfileName,
    pub env: Env<'a>,
    /// `~/.claude/.credentials.json` and `<root>/<m>/.credentials.json` as
    /// first seen this cycle; the move goes ahead only if both still are.
    pub seen: [Seen; 2],
    pub paths: Paths,
}

/// The plan. Each arm says what `hand_suspect` becomes.
pub enum Plan<'a> {
    /// No line. `reset`: the finding was read and is Clear, so `guidance`
    /// and `hand_suspect` are cleared; otherwise a gate returned before any
    /// read and only `guidance` is cleared.
    Nothing { reset: bool },
    /// One note line, written once until it changes.
    Note {
        line: String,
        suspect: Option<HandSuspect>,
    },
    /// ALERT and `fix` lines, written every cycle; the suspicion is kept.
    Guide {
        lines: Vec<String>,
        suspect: Option<HandSuspect>,
    },
    /// Move `m`'s own copy aside, then record `m` as live.
    Move(Box<Confirmed<'a>>),
}

/// The plan, and whether `~/.claude.json` had no readable account under a
/// ledger-named live profile this cycle (the `home_unreadable_since` spell).
/// Calls `env_for` only for a confirmed suspicion.
pub fn plan<'a>(input: &Input<'_>, env_for: impl FnOnce() -> Env<'a>) -> (Plan<'a>, bool) {
    let Input {
        store,
        rs,
        home,
        ledger,
        t,
        ..
    } = *input;
    let Some(l) = ledger
        .live
        .as_deref()
        .and_then(|s| ProfileName::parse(s).ok())
    else {
        return (Plan::Nothing { reset: false }, false);
    };
    if !oauth_login(home) {
        return (Plan::Nothing { reset: false }, false);
    }
    // #1702: a `~/.claude.json` with no account stops the swap, not the
    // recovery decision; one that stays so gets fix lines.
    if !home_readable(home) {
        let since = unreadable_since(ledger.home_unreadable_since, t);
        if t.saturating_sub(since) < HOME_UNREADABLE_GRACE {
            return (Plan::Nothing { reset: false }, true);
        }
        let kind = Kind::HomeStuck {
            problem: identify::home_problem(home),
        };
        let lines = guide::lines(&l, &kind, &paths(store, rs, home, t));
        let suspect = ledger.hand_suspect.clone();
        return (Plan::Guide { lines, suspect }, true);
    }
    if exists(&rs.journal) {
        return (Plan::Nothing { reset: false }, false);
    }
    if let Some(at) = ledger.swapped_at {
        if at.saturating_sub(t) > SWAP_GRACE {
            let line = msg::note_unsure(&l, &Why::ClockBack);
            return (
                Plan::Note {
                    line,
                    suspect: None,
                },
                false,
            );
        }
        if t.saturating_sub(at) < SWAP_GRACE {
            return (Plan::Nothing { reset: false }, false);
        }
    }
    let seen_home = ino_ctime(&home.join(".claude").join(CREDS));
    let m = match assess(store, home, &l, input.read, IDENTITY_BUDGET) {
        Finding::Clear => return (Plan::Nothing { reset: true }, false),
        Finding::Unsure(why) => {
            let line = msg::note_unsure(&l, &why);
            let suspect = None;
            return (Plan::Note { line, suspect }, false);
        }
        Finding::Suspect(m) => m,
    };
    (confirm(input, l, m, seen_home, env_for), false)
}

/// The two-reading rule for "Suspect `m`", then the checks.
fn confirm<'a>(
    input: &Input<'_>,
    l: ProfileName,
    m: ProfileName,
    seen_home: Seen,
    env_for: impl FnOnce() -> Env<'a>,
) -> Plan<'a> {
    let Input {
        store,
        rs,
        home,
        ledger,
        t,
        min_gap,
        ..
    } = *input;
    let after_swap = ledger.swapped_at.unwrap_or(i64::MIN);
    let prior = ledger
        .hand_suspect
        .clone()
        .filter(|s| s.profile == m.as_str() && (after_swap..=t).contains(&s.first_seen));
    let first_seen = prior.as_ref().map_or(t, |s| s.first_seen);
    let suspect = Some(HandSuspect {
        profile: m.to_string(),
        first_seen,
    });
    if prior.is_none() || t.saturating_sub(first_seen) < min_gap.max(1) {
        let line = msg::note_suspect(&m, &l);
        return Plan::Note { line, suspect };
    }
    let paths = paths(store, rs, home, t);
    let refused = |why: (String, guide::Fix), suspect: Option<HandSuspect>| {
        let kind = Kind::Refused {
            m: &m,
            reason: &why.0,
            fix: why.1,
        };
        let lines = guide::lines(&l, &kind, &paths);
        Plan::Guide { lines, suspect }
    };
    if let Some(why) = record_check(store, rs, home) {
        return refused(why, suspect);
    }
    // D7: recording `m` leaves `l` only what is in its own directory.
    if !own_login(store, &l) {
        let lines = guide::lines(&l, &Kind::OnlyLogin { m: &m }, &paths);
        return Plan::Guide { lines, suspect };
    }
    let env = env_for();
    if let Some(why) = move_check(store, home, &m, &env) {
        return refused(why, suspect);
    }
    let seen = [
        seen_home,
        ino_ctime(&store.root().join(m.as_str()).join(CREDS)),
    ];
    Plan::Move(Box::new(Confirmed {
        l,
        m,
        env,
        seen,
        paths,
    }))
}

/// Whether `<home>/.claude/.credentials.json` is a regular file holding an
/// OAuth login (by its top-level keys only).
fn oauth_login(home: &Path) -> bool {
    let creds = home.join(".claude").join(CREDS);
    std::fs::symlink_metadata(&creds).is_ok_and(|m| m.is_file())
        && login_kind(&creds) == LoginKind::OAuth
}

/// SEC-8: `<root>/<l>/.credentials.json` is a login by [`login_kind`]: a
/// regular file of the current user that others cannot write, holding a
/// JSON object (an OAuth or an API-key login). A symlink, another owner,
/// or an empty, unparsable or unreadable file is no login.
fn own_login(store: &ProfileStore, l: &ProfileName) -> bool {
    let path = store.root().join(l.as_str()).join(CREDS);
    matches!(login_kind(&path), LoginKind::OAuth | LoginKind::NoOAuth)
}

/// The [`Seen`] of the file at `p`, by `lstat`.
pub fn ino_ctime(p: &Path) -> Seen {
    std::fs::symlink_metadata(p)
        .ok()
        .map(|m| (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()))
}

/// When `~/.claude.json` has been unreadable since, counting this cycle at
/// `t`: the ledger's time, or `t` when it has none or it lies in the future.
pub fn unreadable_since(stored: Option<i64>, t: i64) -> i64 {
    stored.filter(|s| *s <= t).unwrap_or(t)
}

/// The paths the guidance may name.
fn paths(store: &ProfileStore, rs: &RotateState, home: &Path, t: i64) -> Paths {
    Paths {
        home: home.to_path_buf(),
        root: store.root().to_path_buf(),
        ledger: rs.ledger.clone(),
        manual: store
            .root()
            .join(REJECTED_DIR)
            .join(format!("manual-{}", stamp(t))),
    }
}
