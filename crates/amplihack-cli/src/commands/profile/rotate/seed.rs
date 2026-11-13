//! Seeds ownership from one fresh reading (#1962). A record is a trust anchor;
//! uncertainty refuses the seed. No credential moves or refreshes occur.
//! Identity is read through [`Reader`]; see "Seeding a record" in
//! docs/reference/profile-command.md.
//!
//! [`evaluate`] shares policy across [`in_cycle`], [`preview`] and [`explicit`].
//! [`commit`] rechecks bound logins and claims under the lock; account values stay private.

use super::cycle::{Ctx, LOCK_WAIT, prototype_guard};
use super::handfix_msg::join;
use super::identify::{Account, LiveId, LoginKind, login_kind};
use super::identity_claims::{self, Claim, Claims};
use super::ledger::{RotateLedger, RotateState};
use super::log::{Sink, open_or_warn};
use super::login_identity::{Bound, Reader, default_read};
use super::observe::{Snap, live_name};
use super::swap_authorize::ledger_owns;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{CREDS, State, now};
use crate::commands::profile::expected::{self, Seeded};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::path::Path;

/// The ALERT kind of the same-account stop; written on every cycle.
pub const SAME_ACCOUNT: &str = "same-account";

const LEDGER_UNREADABLE: &str = "the rotate ledger is unreadable";
const JOURNAL: &str = "a swap journal is pending or unreadable";
const SUSPECT: &str = "a hand-change suspicion is open";
const NO_UUID_OR_ORG: &str = "the reading has no account uuid or organization";
const UNPRINTABLE: &str = "the account cannot be printed safely";
const NO_IDENTITY: &str = "the ledger has no login identity";
const NO_MATCH: &str = "the ledger does not match the reading";
const OLDER_NO_MATCH: &str = "the ledger's older key does not match the reading";
const OLDER_EMAIL: &str = "the ledger's older key is an e-mail, which cannot prove ownership";
const HOME_HOLDS: &str =
    "~/.claude holds this account, so the login may be live under another name";
const CHANGED: &str = "the login or a claim changed after it was read";

/// Which path is asking, and so which ownership rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    /// A rotate cycle, for the live profile: full ledger ownership.
    Cycle,
    /// The operator named the profile.
    Explicit,
}

/// What a seed needs from its caller.
struct Scope<'a> {
    store: &'a ProfileStore,
    rs: &'a RotateState,
    state: &'a State,
    home: &'a Path,
    reader: Reader<'a>,
}

/// What was read, kept until the write so that it can be checked again.
struct Evidence {
    bound: Bound,
    account: Account,
    claims: Claims,
    /// A non-live profile's reading of `~/.claude`, or nothing when that login
    /// is definitely missing or this is the live profile.
    home: Option<Bound>,
    /// `~/.claude` held no login when it was compared.
    home_absent: bool,
}

enum Verdict {
    /// A usable record exists, and stays.
    Existing,
    Ready(Box<Evidence>),
    /// The seed stops for this reason.
    Stop(String),
    /// These other profiles claim the account, sorted by name.
    Claimed(Vec<ProfileName>),
}

fn stop(reason: &str) -> Verdict {
    Verdict::Stop(reason.to_string())
}

/// Every rule, in the documented order. Writes nothing.
fn evaluate(s: &Scope<'_>, name: &ProfileName, rule: Rule) -> Verdict {
    let Ok(ledger) = s.rs.load_ledger() else {
        return stop(LEDGER_UNREADABLE);
    };
    // S8: loaded directly, so a journal that cannot be read counts as present.
    if !matches!(s.rs.load_journal(), Ok(None)) || !matches!(s.state.load_journal(), Ok(None)) {
        return stop(JOURNAL);
    }
    if ledger.as_ref().is_some_and(|l| l.hand_suspect.is_some()) {
        return stop(SUSPECT);
    }
    match expected::lookup(s.state, name) {
        Ok(Some(_)) => return Verdict::Existing,
        Ok(None) => {}
        Err(()) => {
            return Verdict::Stop(format!(
                "profile {name} has an unusable record; repair it first"
            ));
        }
    }
    // S3: the ledger's live profile is always `~/.claude`, whatever its folder
    // holds. A non-live profile's folder is judged by the login directory rule.
    let live = ledger
        .as_ref()
        .filter(|l| l.live.as_deref() == Some(name.as_str()));
    let dir = match live {
        Some(_) => s.home.join(".claude"),
        None => s.store.root().join(name.as_str()),
    };
    // A cycle cannot seed from an older ledger whatever the reading says, so it
    // sends no identity request that would only repeat on every cycle.
    if rule == Rule::Cycle && live.is_some_and(|l| l.login_identity.is_none()) {
        return stop(NO_IDENTITY);
    }
    let bound = match s.reader.bound(&dir, false) {
        Ok(bound) => bound,
        Err(cause) => return Verdict::Stop(format!("identity read failed: {cause}")),
    };
    let Some(account) = bound.account().filter(|a| a.has_uuid_and_org()).cloned() else {
        return stop(NO_UUID_OR_ORG);
    };
    if rule == Rule::Cycle && account.email().is_none() {
        return stop(UNPRINTABLE);
    }
    if let Some(why) = live.and_then(|l| ownership(rule, l, name, &account)) {
        return stop(why);
    }
    let claims = match identity_claims::scan_for_seed(s.store, s.state, name, &account, s.reader) {
        Ok(claims) => claims,
        Err(blocked) => return Verdict::Stop(blocked.reason()),
    };
    let (home, home_absent) = if live.is_some() {
        (None, false)
    } else {
        match compare_home(s, &account) {
            Ok(seen) => seen,
            Err(reason) => return Verdict::Stop(reason),
        }
    };
    if matches!(claims.claim, Claim::Claimed) {
        return Verdict::Claimed(claims.claimers);
    }
    Verdict::Ready(Box::new(Evidence {
        bound,
        account,
        claims,
        home,
        home_absent,
    }))
}

/// Why the ledger does not let this reading be the live profile's, if so.
fn ownership(
    rule: Rule,
    l: &RotateLedger,
    name: &ProfileName,
    account: &Account,
) -> Option<&'static str> {
    if ledger_owns(l, name, account) {
        return None;
    }
    if l.login_identity.is_some() {
        return Some(NO_MATCH);
    }
    if rule == Rule::Cycle {
        return Some(NO_IDENTITY);
    }
    // The operator named this profile, so an older key may back the reading,
    // but only the uuid itself: an e-mail is shared across organizations.
    match l.account.as_deref() {
        Some(key) if key.contains('@') => Some(OLDER_EMAIL),
        Some(key) if key == account.key() => None,
        _ => Some(OLDER_NO_MATCH),
    }
}

/// Reads `~/.claude` for a non-live profile. The login may be live under another
/// name if it holds this account, and a failed read cannot rule that out. A
/// definitely missing login or an API-key login holds no account to compare.
fn compare_home(s: &Scope<'_>, account: &Account) -> Result<(Option<Bound>, bool), String> {
    let dir = s.home.join(".claude");
    if login_file_absent(&dir) {
        return Ok((None, true));
    }
    let platform = login_kind(&dir.join(CREDS)) == LoginKind::NoOAuth;
    let bound = s
        .reader
        .bound(&dir, platform)
        .map_err(|cause| format!("~/.claude identity read failed: {cause}"))?;
    if bound.account().is_some_and(|a| a.same(account)) {
        return Err(HOME_HOLDS.to_string());
    }
    Ok((Some(bound), false))
}

/// Only a definite `NotFound` is absence.
fn login_file_absent(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(CREDS))
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

/// Writes the record, under the lock the caller holds, only if everything
/// [`evaluate`] read still holds. A preview cannot justify a write.
fn commit(s: &Scope<'_>, name: &ProfileName, e: &Evidence) -> Result<Seeded, String> {
    let home_same = e.home.as_ref().is_none_or(Bound::current)
        && (!e.home_absent || login_file_absent(&s.home.join(".claude")));
    if !e.bound.current() || !home_same || !e.claims.current_ignoring(s.store, s.state, None) {
        return Err(CHANGED.to_string());
    }
    expected::seed(s.state, name, &e.account).map_err(|err| err.to_string())
}

/// `ALERT: cannot tell ...` without its start, with the live profile among them.
fn same_account_text(live: &ProfileName, claimers: &[ProfileName]) -> String {
    let mut all = claimers.to_vec();
    all.push(live.clone());
    all.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    all.dedup();
    format!(
        "cannot tell profiles {} apart: both claim the account in ~/.claude; no record written, nothing moved",
        join(&all)
    )
}

fn note_text(live: &ProfileName, reason: &str) -> String {
    format!(
        "no record for live profile {live}: {reason}; nothing moved; run: amplihack profile seed-record {live}"
    )
}

/// The ledger's live profile, when a cycle may try to seed it: it has an OAuth
/// login in `~/.claude` and no record. An unusable record or an open hand-change
/// suspicion is already an ALERT, and a live API-key profile has no account to
/// record.
fn live_without_record(ctx: &Ctx, state: &State, snap: &Snap) -> Option<ProfileName> {
    if snap.home_no_oauth || matches!(snap.live, LiveId::HandChanged(_)) {
        return None;
    }
    let name = live_name(&snap.live)?;
    // An unreadable ledger was already stopped by prelude::ledger_stop.
    let ledger = ctx.rs.load_ledger().ok()??;
    let named = ledger.live.as_deref() == Some(name.as_str());
    (named && matches!(expected::lookup(state, name), Ok(None))).then(|| name.clone())
}

fn scope_of<'a>(ctx: &'a Ctx, state: &'a State) -> Scope<'a> {
    Scope {
        store: &ctx.store,
        rs: &ctx.rs,
        state,
        home: &ctx.home,
        reader: ctx.identity_reader(),
    }
}

/// A rotate cycle's seed, under the cycle's lock. `Some(line)` is the same-account
/// ALERT that stops the cycle. Anything else goes on: a note says what stopped the
/// seed, once per process, and the swap's own authorization then refuses.
pub fn in_cycle(ctx: &mut Ctx, state: &State, snap: &Snap, out: &mut Sink) -> Option<String> {
    let name = live_without_record(ctx, state, snap)?;
    let scope = scope_of(ctx, state);
    let reason = match evaluate(&scope, &name, Rule::Cycle) {
        Verdict::Existing => return None,
        Verdict::Claimed(claimers) => {
            return Some(format!("ALERT: {}", same_account_text(&name, &claimers)));
        }
        Verdict::Stop(reason) => reason,
        Verdict::Ready(e) => match commit(&scope, &name, &e) {
            Ok(Seeded::Written) => {
                out.emit(
                    now(),
                    &format!("seeded: recorded account for live profile {name}"),
                );
                ctx.last_seed_note = None;
                return None;
            }
            Ok(Seeded::Existing) => return None,
            Err(reason) => reason,
        },
    };
    let line = format!("note: {}", note_text(&name, &reason));
    if ctx.last_seed_note.as_ref() != Some(&line) {
        out.emit(now(), &line);
        ctx.last_seed_note = Some(line);
    }
    None
}

/// What [`in_cycle`] would do, for `--dry-run`, with the same rules and no write.
/// `true` when the cycle would stop here.
pub fn preview(ctx: &Ctx, state: &State, snap: &Snap, out: &mut Sink) -> bool {
    let Some(name) = live_without_record(ctx, state, snap) else {
        return false;
    };
    let line = match evaluate(&scope_of(ctx, state), &name, Rule::Cycle) {
        Verdict::Existing => return false,
        Verdict::Claimed(claimers) => {
            out.emit(
                0,
                &format!("would alert: {}", same_account_text(&name, &claimers)),
            );
            return true;
        }
        Verdict::Stop(reason) => format!("would note: {}", note_text(&name, &reason)),
        Verdict::Ready(_) => {
            format!("would seed: record for live profile {name} from a fresh reading")
        }
    };
    out.emit(0, &line);
    false
}

/// What `profile seed-record` found.
pub enum Explicit {
    /// A usable record exists, and stays.
    Existing,
    /// No record, and nothing was written.
    Preview(Account),
    /// The record was written.
    Recorded(Account),
    /// The fixed reason, for `seed-record <name>: refused: <reason>`.
    Refused(String),
}

/// `profile seed-record <name> [--write]`. With `write`, the rotate lock is taken
/// first and every check runs under it, and a busy lock is an error. `name` was
/// parsed, and its folder resolved, before this.
pub fn explicit(
    store: &ProfileStore,
    home: &Path,
    name: &ProfileName,
    write: bool,
) -> Result<Explicit, ProfileError> {
    let rs = RotateState::new(store.root());
    let state = State::new(store.root())?;
    if !matches!(rs.load_journal(), Ok(None)) || !matches!(state.load_journal(), Ok(None)) {
        return Ok(Explicit::Refused(JOURNAL.to_string()));
    }
    // Issue #2218: an excluded profile's identity comes from local files.
    let budget = super::identify::excluded_offline::gated_budget(store, &rs, home)?;
    let read = default_read();
    let scope = Scope {
        store,
        rs: &rs,
        state: &state,
        home,
        reader: Reader {
            read: &read,
            budget: &budget,
        },
    };
    let _lock = if write {
        prototype_guard(store.root())?;
        let Some(lock) = state.try_lock(LOCK_WAIT)? else {
            return Err(ProfileError::RotateDaemon("lock busy".into()));
        };
        Some(lock)
    } else {
        None
    };
    let e = match evaluate(&scope, name, Rule::Explicit) {
        Verdict::Existing => return Ok(Explicit::Existing),
        Verdict::Stop(reason) => return Ok(Explicit::Refused(reason)),
        Verdict::Claimed(claimers) => return Ok(Explicit::Refused(claimed(&claimers))),
        Verdict::Ready(e) => e,
    };
    if !write {
        return Ok(Explicit::Preview(e.account));
    }
    // S6: what cannot be shown to the operator cannot be confirmed by them.
    if e.account.email().is_none() {
        return Ok(Explicit::Refused(UNPRINTABLE.to_string()));
    }
    match commit(&scope, name, &e) {
        Ok(Seeded::Existing) => Ok(Explicit::Existing),
        Err(reason) => Ok(Explicit::Refused(reason)),
        Ok(Seeded::Written) => {
            // The record exists now; a failed audit line is only a warning.
            match rs.ensure() {
                Ok(()) => Sink::new(open_or_warn(&rs.log, &rs.log1), false, true).emit(
                    now(),
                    &format!("seed-record: recorded account for profile {name}"),
                ),
                Err(err) => eprintln!(
                    "seed-record {name}: recorded, but the audit log could not be written: {err}"
                ),
            }
            Ok(Explicit::Recorded(e.account))
        }
    }
}

fn claimed(claimers: &[ProfileName]) -> String {
    format!("another profile claims this account: {}", join(claimers))
}
