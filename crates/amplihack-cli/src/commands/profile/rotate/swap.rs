//! The journalled swap (D9, D12) and its recovery (D10). `L` is
//! `~/.claude/.credentials.json`, `T` is `<root>/<to>/.credentials.json` and
//! `S` is `<root>/<from>/.credentials.json`. Logins are only checked with
//! `lstat`, exchanged, and renamed without replacing: each login is in
//! exactly one place at every step and `~/.claude` always holds one.

use super::identify::Account;
use super::ledger::{Phase, RotateJournal, RotateState};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, CREDS, MoveError, abort_point, check_root_private, euid, exchange_logins, exists,
    movable_login, move_login, read_claude_json, release_refresh_lock_at, take_refresh_lock_at,
    writable_by_others, write_atomic_keep_mode, write_private_atomic,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const ADOPT_NAME: &str = "default-original";

// The reasons the how-to quotes word for word; `tests_docs_sync` holds the
// docs to them.
const HOME_NOT_OWNED: &str = "HOME is not a directory owned by you";
const DOT_CLAUDE_NOT_PRIVATE: &str = "~/.claude is not a private directory owned by you";
const LOGIN_NOT_MOVABLE: &str = "a login file is not a private regular file with one link";
const DIFFERENT_DEVICES: &str = "~/.claude and the profile root are on different devices";
const NO_EXCHANGE: &str = "atomic exchange is not supported here";
const ALREADY_LOGGED_IN: &str = "already has a login";
const LOGINS_MISPLACED: &str = "the login files are not where the journal expects";

#[cfg(test)]
pub const DOCUMENTED: [&str; 7] = [
    HOME_NOT_OWNED,
    DOT_CLAUDE_NOT_PRIVATE,
    LOGIN_NOT_MOVABLE,
    DIFFERENT_DEVICES,
    NO_EXCHANGE,
    ALREADY_LOGGED_IN,
    LOGINS_MISPLACED,
];

/// Where the outgoing login goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Profile(ProfileName),
    /// A new `default-original[-N]` profile (D12).
    Adopt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverOutcome {
    None,
    RolledBack { from: ProfileName, to: ProfileName },
    RolledForward { from: ProfileName, to: ProfileName },
    Inconsistent(String),
}

fn refused(msg: impl Into<String>) -> ProfileError {
    ProfileError::RotateRefused(msg.into())
}

fn describe(e: MoveError) -> String {
    match e {
        MoveError::Unsupported => NO_EXCHANGE.to_string(),
        MoveError::Exists => "the exchange found an unexpected file".to_string(),
        MoveError::Other(m) => m,
    }
}

fn ino_at(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path).ok().map(|m| m.ino())
}

fn show(p: &Path) -> String {
    let parent = p.parent().and_then(Path::file_name).unwrap_or_default();
    format!("{}/{}", parent.to_string_lossy(), CREDS)
}

/// The first free `default-original`, `default-original-2`, ... name.
pub fn adoption_name(store: &ProfileStore) -> Result<ProfileName, ProfileError> {
    for n in 1..=1000 {
        let raw = if n == 1 {
            ADOPT_NAME.to_string()
        } else {
            format!("{ADOPT_NAME}-{n}")
        };
        if !exists(&store.root().join(&raw)) {
            return ProfileName::parse(&raw);
        }
    }
    Err(refused("no free default-original name"))
}

/// `$HOME` and `~/.claude` may be changed by this user only.
fn check_home(home: &Path) -> Result<PathBuf, ProfileError> {
    if !home.is_absolute() {
        return Err(refused("HOME is not an absolute path"));
    }
    let m = fs::symlink_metadata(home).map_err(|_| refused("HOME cannot be read"))?;
    if m.file_type().is_symlink() || !m.is_dir() || m.uid() != euid() {
        return Err(refused(HOME_NOT_OWNED));
    }
    let dot = home.join(".claude");
    let m = fs::symlink_metadata(&dot).map_err(|_| refused("~/.claude cannot be read"))?;
    if m.file_type().is_symlink() || !m.is_dir() || m.uid() != euid() || writable_by_others(&m) {
        return Err(refused(DOT_CLAUDE_NOT_PRIVATE));
    }
    Ok(dot)
}

/// The `oauthAccount` block of `dir/.claude.json`, if it is an object.
fn account_block(dir: &Path) -> Option<serde_json::Value> {
    read_claude_json(dir)?
        .0
        .get("oauthAccount")
        .filter(|b| b.is_object())
        .cloned()
}

/// Runs the swap; returns the profile the outgoing login went to. Every
/// check runs before anything is written; a failure there is
/// `RotateRefused` and nothing changes.
pub fn swap(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: Source,
    to: &ProfileName,
    now: i64,
) -> Result<ProfileName, ProfileError> {
    check_root_private(store.root()).map_err(|e| refused(format!("profile root {e}")))?;
    let dot = check_home(home)?;
    if exists(&rs.journal) {
        return Err(refused("a swap journal is pending; run recovery first"));
    }
    let to_dir = store
        .verify_profile_dir(to)
        .map_err(|_| refused(format!("profile {to} failed the directory check")))?;
    let (l, t) = (dot.join(CREDS), to_dir.join(CREDS));
    if !movable_login(&l) || !movable_login(&t) {
        return Err(refused(LOGIN_NOT_MOVABLE));
    }
    if account_block(&to_dir).is_none() {
        return Err(refused(format!(
            "profile {to} has no account in .claude.json"
        )));
    }
    let dev = |p: &Path| fs::metadata(p).map(|m| m.dev()).ok();
    if dev(&dot).is_none() || dev(&dot) != dev(&to_dir) {
        return Err(refused(DIFFERENT_DEVICES));
    }
    if !cfg!(target_os = "linux") {
        return Err(refused(NO_EXCHANGE));
    }
    if let Source::Profile(name) = &from {
        let dir = store
            .verify_profile_dir(name)
            .map_err(|_| refused(format!("profile {name} failed the directory check")))?;
        if name == to || exists(&dir.join(CREDS)) {
            return Err(refused(format!("profile {name} {ALREADY_LOGGED_IN}")));
        }
    }
    let home_str = home.to_str().ok_or_else(|| refused("HOME is not UTF-8"))?;
    let (from, adopt) = match from {
        Source::Profile(name) => (name, false),
        Source::Adopt => (adopt(store, home)?, true),
    };
    let mut j = RotateJournal {
        version: 1,
        from: from.to_string(),
        to: to.to_string(),
        adopt,
        phase: Phase::J0,
        home: home_str.to_string(),
        live_ino: ino_at(&l).unwrap_or_default(),
        target_ino: ino_at(&t).unwrap_or_default(),
        home_lock_ino: None,
        to_lock_ino: None,
        started_at: now,
        pid: std::process::id(),
    };
    rs.save_journal(&j)?;
    // Each lock's inode is recorded as soon as it is taken.
    if let Some(ino) = take_refresh_lock_at(&dot) {
        j.home_lock_ino = Some(ino);
        rs.save_journal(&j)?;
    }
    if let Some(ino) = take_refresh_lock_at(&to_dir) {
        j.to_lock_ino = Some(ino);
        rs.save_journal(&j)?;
    }
    abort_point("rotate-J0");

    if let Err(e) = exchange_logins(&t, &l) {
        release_locks(store, home, &j);
        rs.delete_journal()?;
        return Err(refused(describe(e)));
    }
    step(rs, &mut j, Phase::S1)?;
    let s = store.root().join(from.as_str()).join(CREDS);
    move_login(&t, &s, &show).map_err(|e| {
        ProfileError::RotateInconsistent(format!(
            "the outgoing login could not be moved from {to} to {from}: {}",
            describe(e)
        ))
    })?;
    release_locks(store, home, &j);
    step(rs, &mut j, Phase::S2)?;
    finish(store, rs, home, &j)?;
    Ok(from)
}

/// Rewrites the journal at `phase`, then stops there in a test run.
fn step(rs: &RotateState, j: &mut RotateJournal, phase: Phase) -> Result<(), ProfileError> {
    j.phase = phase;
    rs.save_journal(j)?;
    abort_point(&format!("rotate-{}", phase.as_str().to_uppercase()));
    Ok(())
}

/// Creates the adoption profile and writes the `oauthAccount` block of
/// `~/.claude.json` into it, and nothing else. `add` copies no login.
fn adopt(store: &ProfileStore, home: &Path) -> Result<ProfileName, ProfileError> {
    let name = adoption_name(store)?;
    let dir = store.add(&name)?;
    if exists(&dir.join(CREDS)) {
        return Err(refused(format!("new profile {name} {ALREADY_LOGGED_IN}")));
    }
    if let Some(block) = account_block(home) {
        let body = serde_json::json!({ "oauthAccount": block });
        let bytes = serde_json::to_vec_pretty(&body).map_err(|e| refused(e.to_string()))?;
        write_private_atomic(&dir, CLAUDE_JSON, &bytes)?;
    }
    Ok(name)
}

fn release_locks(store: &ProfileStore, home: &Path, j: &RotateJournal) {
    if let Some(ino) = j.home_lock_ino {
        release_refresh_lock_at(&home.join(".claude"), ino);
    }
    if let Some(ino) = j.to_lock_ino {
        release_refresh_lock_at(&store.root().join(&j.to), ino);
    }
}

/// `s3` and `s4`, then the journal is deleted. Safe to repeat.
fn finish(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    j: &RotateJournal,
) -> Result<(), ProfileError> {
    let mut j = j.clone();
    let to_dir = store.root().join(&j.to);
    let block = account_block(&to_dir);
    patch_home_account(home, block.as_ref())?;
    step(rs, &mut j, Phase::S3)?;
    let mut l = rs.load_ledger()?.unwrap_or_default();
    l.live = Some(j.to.clone());
    l.previous = Some(j.from.clone());
    l.account = block
        .map(|b| serde_json::json!({ "oauthAccount": b }))
        .and_then(|v| Account::from_value(&v))
        .map(|a| a.key().to_string());
    l.since = Some(j.started_at);
    l.swapped_at = Some(j.started_at);
    l.alert = None;
    rs.save_ledger(&l)?;
    step(rs, &mut j, Phase::S4)?;
    rs.delete_journal()
}

/// `s3`: replaces only `oauthAccount` in `$HOME/.claude.json`, keeping
/// every other key and the file's mode. A missing file is created.
fn patch_home_account(home: &Path, block: Option<&serde_json::Value>) -> Result<(), ProfileError> {
    let Some(block) = block else {
        return Ok(());
    };
    let mut v = match read_claude_json(home) {
        Some((v, _)) => v,
        None if !exists(&home.join(CLAUDE_JSON)) => serde_json::json!({}),
        None => {
            eprintln!("warning: ~/.claude.json is not a JSON object; its account was not updated");
            return Ok(());
        }
    };
    if v.get("oauthAccount") == Some(block) {
        return Ok(());
    }
    v["oauthAccount"] = block.clone();
    let bytes = serde_json::to_vec_pretty(&v).map_err(|e| refused(e.to_string()))?;
    write_atomic_keep_mode(home, CLAUDE_JSON, &bytes).map_err(ProfileError::Io)
}

/// Marks the journal `inconsistent`: no swap until a person resolves it.
fn freeze(
    rs: &RotateState,
    mut j: RotateJournal,
    reason: &str,
) -> Result<RecoverOutcome, ProfileError> {
    j.phase = Phase::Inconsistent;
    rs.save_journal(&j)?;
    Ok(RecoverOutcome::Inconsistent(reason.to_string()))
}

/// Settles an interrupted swap by where the recorded inodes are; the
/// phase never decides. Only renames; never deletes or opens a login.
pub fn recover(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Result<RecoverOutcome, ProfileError> {
    let Some(j) = rs.load_journal()? else {
        return Ok(RecoverOutcome::None);
    };
    if j.phase == Phase::Inconsistent {
        return Ok(RecoverOutcome::Inconsistent(
            "an earlier swap stopped half way".into(),
        ));
    }
    if home.to_str() != Some(j.home.as_str()) {
        return freeze(rs, j, "the journal is for another home directory");
    }
    // Names were checked when the journal was loaded.
    let (from, to) = (ProfileName::parse(&j.from)?, ProfileName::parse(&j.to)?);
    let (Ok(from_dir), Ok(to_dir)) = (
        store.verify_profile_dir(&from),
        store.verify_profile_dir(&to),
    ) else {
        return freeze(rs, j, "a profile directory failed the directory check");
    };
    let l = home.join(".claude").join(CREDS);
    if !fs::symlink_metadata(&l).is_ok_and(|m| m.is_file()) {
        return freeze(rs, j, "no login in ~/.claude");
    }
    let (t, s) = (to_dir.join(CREDS), from_dir.join(CREDS));
    let (at_t, at_s) = (ino_at(&t), ino_at(&s));
    if at_t == Some(j.target_ino) && at_s.is_none() {
        release_locks(store, home, &j);
        rs.delete_journal()?;
        return Ok(RecoverOutcome::RolledBack { from, to });
    }
    if at_t == Some(j.live_ino) && at_s.is_none() {
        if move_login(&t, &s, &show).is_err() {
            return freeze(
                rs,
                j,
                "the outgoing login could not be moved to its profile",
            );
        }
    } else if !(at_s == Some(j.live_ino) && at_t.is_none()) {
        return freeze(rs, j, LOGINS_MISPLACED);
    }
    release_locks(store, home, &j);
    finish(store, rs, home, &j)?;
    Ok(RecoverOutcome::RolledForward { from, to })
}
