//! Moving logins: the in-session swap, the return when a session ends, crash
//! recovery, and the lease checks. A login file is only ever renamed with
//! `RENAME_NOREPLACE`, never opened, so each account's login is in exactly
//! one place: its home slot or the one directory where it is live.

use super::candidate::{self, Pool};
use super::config::Config;
use super::fsx;
use super::liveness::{self, Liveness};
use super::mv::{self, MoveError, abort_point};
use super::overflow_inputs::OverflowInputs;
use super::state::{
    CLAUDE_JSON, CREDS, HostEntry, Journal, Ledger, Lock, MAX_CLAUDE_JSON, Op, PHASE_INCONSISTENT,
    State,
};
use super::usage::{self, Attribution, RateLimits};
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::identify::Account;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const RERUN_MAX_AGE_SECS: u64 = 24 * 3600;

fn invariant(msg: impl Into<String>) -> ProfileError {
    ProfileError::AutoSwitchInvariant(msg.into())
}

fn move_login(state: &State, src: &Path, dst: &Path) -> Result<(), MoveError> {
    mv::move_login(src, dst, &|p| state.rel(p))
}

/// Home slot of `account` while `dir`'s live login changes: the owner of
/// `dir` always has its home slot in `parked/`.
fn slot(state: &State, ledger: &Ledger, dir: &str, account: &str) -> PathBuf {
    if account == dir {
        state.parked_dir(dir).join(CREDS)
    } else {
        state.home_slot(ledger, account)
    }
}

/// Moves `j.from`'s login out of `j.dir` and `j.to`'s in, journaled.
fn transfer(state: &State, ledger: &mut Ledger, mut j: Journal) -> Result<(), ProfileError> {
    let live = state.live_creds(&j.dir);
    let from_slot = slot(state, ledger, &j.dir, &j.from);
    let to_slot = slot(state, ledger, &j.dir, &j.to);
    state.ensure_sub(&state.dir.join("parked"))?;
    state.ensure_sub(&state.parked_dir(&j.dir))?;
    j.phase = "J0".into();
    state.save_journal(&j)?;
    abort_point("J0");

    if j.from_present {
        if let Err(e) = move_login(state, &live, &from_slot) {
            return Err(fail(state, &mut j, e, &[]));
        }
        abort_point("S1");
    }
    if let Err(e) = move_login(state, &to_slot, &live) {
        let undo: &[(&Path, &Path)] = if j.from_present {
            &[(&from_slot, &live)]
        } else {
            &[]
        };
        return Err(fail(state, &mut j, e, undo));
    }
    if let Err(e) = fsx::touch_nofollow(&live) {
        eprintln!(
            "warning: auto-switch could not update the time of {}: {e}",
            state.rel(&live)
        );
    }
    abort_point("S2");
    finish(state, ledger, &j)?;
    abort_point("S3");
    state.delete_journal()
}

/// A failed move. `EEXIST` freezes everything with an `inconsistent` journal;
/// other errors undo the moves already done and drop the journal.
/// Any error from bookkeeping after a failed move is added to the message,
/// so the user is never told something happened that did not.
fn fail(state: &State, j: &mut Journal, e: MoveError, undo: &[(&Path, &Path)]) -> ProfileError {
    let freeze = |j: &mut Journal, mut msg: String| {
        j.phase = PHASE_INCONSISTENT.into();
        if let Err(e) = state.save_journal(j) {
            msg.push_str(&format!("; could not save the journal: {e}"));
        }
        invariant(msg)
    };
    if matches!(e, MoveError::Exists) {
        let msg = format!(
            "a login file already exists where profile {:?}'s login was being moved",
            j.to
        );
        return freeze(j, msg);
    }
    for (src, dst) in undo {
        if move_login(state, src, dst).is_err() {
            let msg = "a login could not be moved back after a failed swap".to_string();
            return freeze(j, msg);
        }
    }
    let mut msg = match e {
        MoveError::Unsupported => match mark_disabled(state, &j.dir) {
            Ok(()) => format!(
                "logins cannot be renamed on this filesystem; swaps disabled for profile {:?}",
                j.dir
            ),
            Err(err) => format!(
                "logins cannot be renamed on this filesystem; could not disable swaps for profile {:?}: {err}",
                j.dir
            ),
        },
        MoveError::Other(msg) => msg,
        MoveError::Exists => unreachable!("handled above"),
    };
    if let Err(err) = state.delete_journal() {
        msg.push_str(&format!("; could not delete the journal: {err}"));
    }
    invariant(msg)
}

fn mark_disabled(state: &State, dir: &str) -> Result<(), ProfileError> {
    let d = state.dir.join("disabled");
    state.ensure_sub(&d)?;
    fsx::write_private_atomic(&d, dir, b"")?;
    Ok(())
}

pub fn swaps_disabled(state: &State, dir: &str) -> bool {
    fsx::exists(&state.dir.join("disabled").join(dir))
}

/// Step 3: the ledger and the account metadata Claude shows.
fn finish(state: &State, ledger: &mut Ledger, j: &Journal) -> Result<(), ProfileError> {
    if j.to == j.dir {
        ledger.hosts.remove(&j.dir);
    } else {
        ledger.hosts.insert(
            j.dir.clone(),
            HostEntry {
                live: j.to.clone(),
                swapped_at: j.started_at,
                pre_swap_fingerprint: j.fingerprint.clone(),
            },
        );
    }
    state.save_ledger(ledger)?;
    patch_oauth_account(state, &j.dir, &j.to);
    Ok(())
}

// ---------------------------------------------------------------------------
// oauthAccount
// ---------------------------------------------------------------------------

pub fn read_claude_json(dir: &Path) -> Option<(serde_json::Value, fs::Metadata)> {
    let (raw, meta) =
        fsx::read_bounded_nofollow(&dir.join(CLAUDE_JSON), MAX_CLAUDE_JSON).ok()??;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    v.is_object().then_some((v, meta))
}

/// Saves `name`'s `oauthAccount` block while its directory holds its own
/// login. Only that key is copied; it holds no tokens.
pub fn snapshot_account(state: &State, name: &str) {
    let Some((v, _)) = read_claude_json(&state.profile_dir(name)) else {
        return;
    };
    let Some(block) = v.get("oauthAccount").filter(|b| b.is_object()) else {
        return;
    };
    if let Ok(Some(saved)) = state.read_sub_json("accounts", name)
        && &saved == block
    {
        return;
    }
    if let Err(e) = state.write_sub_json("accounts", name, block) {
        eprintln!("warning: auto-switch could not save account details for profile {name:?}: {e}");
    }
}

/// The account that owns `name`'s login, for the duplicate-login checks:
/// the saved block while `name`'s directory hosts another account, else the
/// directory's own `.claude.json`. Read-only.
pub fn account(state: &State, name: &str, hosting: bool) -> Option<Account> {
    if hosting {
        Account::from_block(&state.read_sub_json("accounts", name).ok()??)
    } else {
        Account::of_dir(&state.profile_dir(name))
    }
}

/// Sets `oauthAccount` in `dir`'s `.claude.json` to `account`'s saved block,
/// keeping every other key. Retries once if the file changes meanwhile.
fn patch_oauth_account(state: &State, dir: &str, account: &str) {
    let block = match state.read_sub_json("accounts", account) {
        Ok(Some(b)) => b,
        Ok(None) => {
            eprintln!(
                "warning: auto-switch has no saved account details for profile {account:?}; \
                 /status may name the previous account"
            );
            return;
        }
        Err(e) => {
            eprintln!("warning: auto-switch ignored damaged account details: {e}");
            return;
        }
    };
    let path = state.profile_dir(dir);
    for _ in 0..2 {
        let Some((mut v, before)) = read_claude_json(&path) else {
            return;
        };
        if v.get("oauthAccount") == Some(&block) {
            return;
        }
        v["oauthAccount"] = block.clone();
        let Ok(bytes) = serde_json::to_vec_pretty(&v) else {
            return;
        };
        let unchanged = fs::symlink_metadata(path.join(CLAUDE_JSON))
            .is_ok_and(|now| same_version(&before, &now));
        if !unchanged {
            continue;
        }
        if let Err(e) = fsx::write_atomic_keep_mode(&path, CLAUDE_JSON, &bytes) {
            eprintln!("warning: auto-switch could not update {dir}/{CLAUDE_JSON}: {e}");
        }
        return;
    }
    eprintln!("warning: {dir}/{CLAUDE_JSON} kept changing; account details not updated");
}

/// Whether `now` is the same version of a file as `before`: same inode,
/// size and full mtime (seconds and nanoseconds).
fn same_version(before: &fs::Metadata, now: &fs::Metadata) -> bool {
    now.ino() == before.ino()
        && now.mtime() == before.mtime()
        && now.mtime_nsec() == before.mtime_nsec()
        && now.len() == before.len()
}

// ---------------------------------------------------------------------------
// Recovery and return
// ---------------------------------------------------------------------------

/// Finishes or undoes an interrupted swap, returns every hosted directory
/// that is no longer live, and removes stale rerun buffers. Call with the
/// global lock held. Returns the `/proc` scan when it took one: it is
/// skipped while no directory hosts a borrowed login, since that scan costs
/// a read of every same-user process's environment.
pub fn recover(state: &State) -> Result<Option<Liveness>, ProfileError> {
    recover_reporting(state).map(|(live, _)| live)
}

/// `recover`, plus one line for each thing it did, for `repair`.
pub fn recover_reporting(state: &State) -> Result<(Option<Liveness>, Vec<String>), ProfileError> {
    let mut done = Vec::new();
    let mut ledger = state.load_ledger()?;
    if let Some(j) = state.load_journal()? {
        let forward = resolve_journal(state, &mut ledger, &j)?;
        done.push(format!(
            "{} an interrupted swap in {}",
            if forward { "finished" } else { "rolled back" },
            j.dir
        ));
    }
    if ledger.hosts.is_empty() {
        cleanup_rerun(state);
        return Ok((None, done));
    }
    let live = liveness::scan_system(&state.root);
    let hosted: Vec<(String, HostEntry)> = ledger
        .hosts
        .iter()
        .map(|(d, h)| (d.clone(), h.clone()))
        .collect();
    for (dir, host) in hosted {
        let Ok(name) = ProfileName::parse(&dir) else {
            continue;
        };
        if live.is_live(&name) {
            continue;
        }
        return_dir(state, &mut ledger, &dir, &host)?;
        done.push(format!(
            "returned the login of {} hosted in {dir} (session ended)",
            host.live
        ));
    }
    cleanup_rerun(state);
    Ok((Some(live), done))
}

/// Whether a swap journal is waiting for recovery. Looks only at whether
/// the file exists, so `status` can report it without acting on it.
pub fn pending_journal(state: &State) -> bool {
    fsx::exists(&state.dir.join("journal.json"))
}

/// Settles an interrupted swap; `true` when it was rolled forward.
fn resolve_journal(state: &State, ledger: &mut Ledger, j: &Journal) -> Result<bool, ProfileError> {
    if j.phase == PHASE_INCONSISTENT {
        return Err(invariant(format!(
            "an earlier move of profile {:?}'s login into {:?} stopped half way",
            j.to, j.dir
        )));
    }
    let live = state.live_creds(&j.dir);
    let from_slot = slot(state, ledger, &j.dir, &j.from);
    let to_slot = slot(state, ledger, &j.dir, &j.to);
    let (live_there, from_there, to_there) = (
        fsx::exists(&live),
        fsx::exists(&from_slot),
        fsx::exists(&to_slot),
    );
    let ambiguous = || {
        invariant(format!(
            "after an interrupted move into {:?}, the login files are not where expected",
            j.dir
        ))
    };
    let forward = !to_there;
    if to_there {
        // The incoming login never reached the live directory: roll back.
        match (live_there, j.from_present && from_there) {
            (true, true) => return Err(ambiguous()),
            (true, false) => {}
            (false, true) => {
                move_login(state, &from_slot, &live).map_err(|_| ambiguous())?;
            }
            (false, false) if j.from_present => return Err(ambiguous()),
            (false, false) => {}
        }
    } else if live_there {
        // It did: roll forward.
        finish(state, ledger, j)?;
    } else {
        return Err(ambiguous());
    }
    release_refresh_lock(state, &j.dir, j.refresh_lock_ino);
    state.delete_journal()?;
    Ok(forward)
}

/// Returns directory `dir`, hosting `host.live`, to its owner.
fn return_dir(
    state: &State,
    ledger: &mut Ledger,
    dir: &str,
    host: &HostEntry,
) -> Result<(), ProfileError> {
    let parked = state.parked_dir(dir).join(CREDS);
    if !fsx::exists(&parked) {
        // The owner's login is live in another directory; it goes home from
        // there. Only the borrowed login moves now.
        let live = state.live_creds(dir);
        if fsx::exists(&live) {
            let home = state.home_slot(ledger, &host.live);
            move_login(state, &live, &home).map_err(|_| {
                invariant(format!("the login hosted in {dir:?} could not be returned"))
            })?;
        }
        ledger.hosts.remove(dir);
        state.save_ledger(ledger)?;
        patch_oauth_account(state, dir, dir);
        return Ok(());
    }
    let j = Journal {
        version: 1,
        op: Op::Return,
        dir: dir.to_string(),
        from: host.live.clone(),
        from_present: fsx::exists(&state.live_creds(dir)),
        to: dir.to_string(),
        phase: String::new(),
        started_at: fsx::now(),
        pid: std::process::id(),
        fingerprint: None,
        refresh_lock_ino: None,
    };
    transfer(state, ledger, j)?;
    let _ = fs::remove_dir(state.parked_dir(dir));
    Ok(())
}

fn cleanup_rerun(state: &State) {
    sweep_rerun(&state.dir.join("rerun"));
}

/// Removes regular `<pid>-<n>.out` files in `dir` (no recursion) whose
/// supervisor is gone or that are over 24 h old. Other names are left.
pub(crate) fn sweep_rerun(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let file = e.file_type().is_ok_and(|t| t.is_file());
        let Some(pid) = rerun_owner(&e.file_name()).filter(|_| file) else {
            continue;
        };
        let old = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() > RERUN_MAX_AGE_SECS);
        if old || !pid_running(pid) {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// The supervisor pid in a rerun buffer name `<pid>-<n>.out`.
pub fn rerun_owner(name: &std::ffi::OsStr) -> Option<u32> {
    let name = name.to_str()?.strip_suffix(".out")?;
    let (pid, n) = name.split_once('-')?;
    n.parse::<u32>().ok()?;
    pid.parse().ok().filter(|&p| p > 0)
}

fn pid_running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

// ---------------------------------------------------------------------------
// Claude's refresh lock
// ---------------------------------------------------------------------------

/// Takes Claude's refresh lock in profile `dir`; `None` when it is held.
fn take_refresh_lock(state: &State, dir: &str) -> Option<u64> {
    // Auto-switch does not swap without the lock; why it was not taken
    // does not change that.
    mv::take_refresh_lock_at(&state.profile_dir(dir)).ok()
}

/// Removes the refresh lock only if it is still the one amplihack created.
fn release_refresh_lock(state: &State, dir: &str, ino: Option<u64>) {
    if let Some(ino) = ino {
        mv::release_refresh_lock_at(&state.profile_dir(dir), ino);
    }
}

// ---------------------------------------------------------------------------
// In-session swap
// ---------------------------------------------------------------------------

/// No swap without a usable `/proc`, or in a directory whose swaps were
/// disabled after a failed rename.
fn swap_allowed(state: &State, live: &Liveness, owner: &ProfileName) -> bool {
    live.supported() && !swaps_disabled(state, owner.as_str())
}

/// The swap check after a status-line update for a session in `owner`'s
/// directory. Call with the global lock held and the switch on.
pub fn swap_check(
    state: &State,
    profiles: &[ProfileName],
    owner: &ProfileName,
    rl: &RateLimits,
    cfg: &Config,
) -> Result<(), ProfileError> {
    let now = fsx::now();
    // Most updates need no move: decide that cheaply, before recovery and
    // the `/proc` scan. The status-line wrapper ran recovery moments ago in
    // this same update. On a plan profile, the reading alone decides. On an
    // overflow profile its own reading does not count (issue #1606: a credit
    // account at its plan limit is still usable); only a spend block or a
    // plan profile to move back to is worth the full check.
    let exhausted =
        candidate::is_exhausted(Some(&usage::reading_from(rl, now)), cfg.threshold, now);
    let worth_checking = overflow_precheck(state, owner, cfg, now).unwrap_or(exhausted);
    if !worth_checking {
        return Ok(());
    }
    let live = match recover(state)? {
        Some(live) => live,
        None => liveness::scan_system(&state.root),
    };
    if !swap_allowed(state, &live, owner) {
        return Ok(());
    }
    let mut ledger = state.load_ledger()?;
    let current = match usage::attribute(owner.as_str(), ledger.hosts.get(owner.as_str()), rl, now)
    {
        Attribution::Record(account) => account,
        Attribution::Ignore(_) => return Ok(()),
    };
    let live_path = state.live_creds(owner.as_str());
    if !fsx::movable_login(&live_path) {
        return Ok(()); // no file to move (Keychain, logged out, or linked)
    }
    let readings = state.load_usage().accounts;
    let overflow = OverflowInputs::load(&state.root, cfg, now);
    let leaving_overflow = overflow.order.iter().any(|o| o.as_str() == current);
    if !exhausted && !leaving_overflow {
        return Ok(()); // the account changed since the first check
    }
    let pool = Pool {
        profiles,
        readings: &readings,
        threshold: cfg.threshold,
        exclude: &cfg.exclude,
        now,
    };
    // A candidate's home slot must hold its login; when that slot is its own
    // directory, no running process may be using the directory. An API-key
    // profile is never swapped into a running session: its `.credentials.json`
    // is no login on its own, so the session would be logged out (#1804).
    let eligible = |c: &ProfileName| {
        let n = c.as_str();
        !overflow.not_plan.contains(c)
            && ledger.leased_to(n).is_none()
            && fsx::movable_login(&slot(state, &ledger, owner.as_str(), n))
            && (ledger.hosts.contains_key(n) || n == owner.as_str() || !live.is_live(c))
    };
    let Some(target) =
        candidate::select_with_overflow(&pool, &overflow.pool(), &current, eligible).cloned()
    else {
        return Ok(());
    };
    if !ledger.hosts.contains_key(target.as_str()) {
        snapshot_account(state, target.as_str());
    }
    if current == owner.as_str() {
        snapshot_account(state, owner.as_str());
    }
    let Some(ino) = take_refresh_lock(state, owner.as_str()) else {
        return Ok(());
    };
    let j = Journal {
        version: 1,
        op: Op::Swap,
        dir: owner.to_string(),
        from: current.clone(),
        from_present: true,
        to: target.to_string(),
        phase: String::new(),
        started_at: now,
        pid: std::process::id(),
        fingerprint: Some(usage::fingerprint(rl)),
        refresh_lock_ino: Some(ino),
    };
    let result = transfer(state, &mut ledger, j);
    release_refresh_lock(state, owner.as_str(), Some(ino));
    result?;
    let why = if !leaving_overflow {
        "reached the auto-switch threshold"
    } else if !candidate::overflow_usable(&current, &overflow.blocked) {
        "is an overflow profile that reached its spend limit"
    } else {
        "is an overflow profile and a plan profile has capacity again"
    };
    eprintln!(
        "amplihack: profile {current:?} {why}; the session in profile {:?} now uses the login of profile {:?}",
        owner.as_str(),
        target.as_str()
    );
    Ok(())
}

/// The status line's cheap pre-check for a session whose account in
/// `owner`'s directory is an overflow profile: `Some(true)` when the full
/// check may move it, `Some(false)` when it cannot, `None` when the account
/// is not an overflow profile (the reading alone decides then). It may move
/// when the profile has a spend block, or when some plan profile may be
/// moved back to ([`candidate::way_off`]): the tests the selection applies
/// ([`candidate::overflow_usable`] and steps 1 and 2 of
/// `select_with_overflow`), on the same inputs ([`OverflowInputs`]), so the
/// check does not pass on readings the selection then refuses, nor stop one
/// it would act on. The overflow profile's own reading plays no part.
/// Reads the ledger, `usage.json`, the spend blocks and the gate; no
/// recovery, no `/proc` scan.
fn overflow_precheck(state: &State, owner: &ProfileName, cfg: &Config, now: i64) -> Option<bool> {
    // Nothing marked means no overflow account: skip reading every login.
    if cfg.overflow_order().is_empty() {
        return None;
    }
    let ledger = state.load_ledger().ok()?;
    let account = ProfileName::parse(ledger.account_in(owner.as_str())).ok()?;
    // Issue #1700: the API-key profile is in the order too.
    let overflow = OverflowInputs::load(&state.root, cfg, now);
    if !overflow.order.contains(&account) {
        return None;
    }
    let readings = state.load_usage().accounts;
    Some(overflow_may_move(&account, cfg, &overflow, &readings, now))
}

/// The decision of [`overflow_precheck`] for a session on overflow profile
/// `account`, from what it read. Pure.
pub(super) fn overflow_may_move(
    account: &ProfileName,
    cfg: &Config,
    overflow: &OverflowInputs,
    readings: &std::collections::BTreeMap<String, usage::Reading>,
    now: i64,
) -> bool {
    if !candidate::overflow_usable(account.as_str(), &overflow.blocked) {
        return true;
    }
    // Issue #1700: from the API key, a usable credit profile is a way off.
    if overflow.platform.as_ref() == Some(account)
        && overflow.order.iter().any(|o| {
            Some(o) != overflow.platform.as_ref()
                && !cfg.exclude.contains(o)
                && candidate::overflow_usable(o.as_str(), &overflow.blocked)
        })
    {
        return true;
    }
    readings
        .iter()
        .filter_map(|(name, r)| ProfileName::parse(name).ok().map(|n| (n, r)))
        .filter(|(n, _)| overflow.pool().is_plan(n) && !cfg.exclude.contains(n))
        .any(|(_, r)| candidate::way_off(Some(r), overflow.gate.as_ref(), cfg.threshold, now))
}

// ---------------------------------------------------------------------------
// Leases
// ---------------------------------------------------------------------------

pub use super::Action;

/// Refuses an action on `name` while its login is away from home. Runs
/// recovery first. A no-op (one metadata check) without a state directory.
pub fn guard(state: &State, name: &ProfileName, action: Action) -> Result<(), ProfileError> {
    guard_held(state, name, action).map(drop)
}

/// [`guard`], returning the state lock still held (`None` without a state
/// directory), so a launch can keep swaps out until its process exists.
pub fn guard_held(
    state: &State,
    name: &ProfileName,
    action: Action,
) -> Result<Option<Lock>, ProfileError> {
    if !state.exists() {
        return Ok(None);
    }
    let lock = state.lock()?;
    recover(state)?;
    let ledger = state.load_ledger()?;
    check_lease(state, &ledger, name, action)?;
    Ok(Some(lock))
}

pub fn check_lease(
    state: &State,
    ledger: &Ledger,
    name: &ProfileName,
    action: Action,
) -> Result<(), ProfileError> {
    let n = name.as_str();
    if let Some(host) = ledger.leased_to(n) {
        return Err(ProfileError::Leased {
            profile: n.to_string(),
            host: host.to_string(),
        });
    }
    if let Some(h) = ledger.hosts.get(n) {
        let joined = matches!(action, Action::Launch { join: true });
        if !joined {
            return Err(ProfileError::HostsForeignAccount {
                profile: n.to_string(),
                live: h.live.clone(),
            });
        }
    }
    if action == Action::Remove && fsx::exists(&state.parked_dir(n).join(CREDS)) {
        return Err(invariant(format!("profile {n:?} has a parked login")));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests_swap.rs"]
mod tests;
