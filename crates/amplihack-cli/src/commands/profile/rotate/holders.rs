//! Who holds the account in `~/.claude` (issue #1693). Split from
//! `identify`, which re-exports it; accounts are compared only through
//! [`Account`], and login files are checked by `lstat` only.

use super::identify::Account;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{CREDS, exists};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::path::Path;

/// Who holds the account in `~/.claude` (issue #1693). Lists are sorted by
/// name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holders {
    /// No profile has the account.
    None,
    /// Exactly one profile has it; `own_login`: something is at its
    /// `.credentials.json` (by `lstat`).
    One {
        name: ProfileName,
        own_login: bool,
    },
    Many(Vec<ProfileName>),
    /// Profiles with a login whose account cannot be read: any of them could
    /// hold the account, so no match can be trusted.
    Unreadable(Vec<ProfileName>),
    /// `~/.claude.json` has no usable account.
    HomeUnreadable,
    /// `~/.claude/.credentials.json` is missing or not a regular file.
    HomeNoLogin,
}

/// Every profile that passes the directory check, with or without its own
/// login, compared by [`Account::same`] with the account in `~/.claude`.
/// Reads `.claude.json` files and metadata only; a login-less profile with
/// no account is skipped.
pub fn home_holders(store: &ProfileStore, home: &Path) -> Result<Holders, ProfileError> {
    let creds = home.join(".claude").join(CREDS);
    if !std::fs::symlink_metadata(&creds).is_ok_and(|m| m.is_file()) {
        return Ok(Holders::HomeNoLogin);
    }
    let Some(account) = Account::of_dir(home) else {
        return Ok(Holders::HomeUnreadable);
    };
    let mut names: Vec<ProfileName> = store
        .list()?
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect();
    names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let (mut matched, mut unreadable) = (Vec::new(), Vec::new());
    for name in names {
        let Ok(dir) = store.verify_profile_dir(&name) else {
            continue;
        };
        let own = exists(&dir.join(CREDS));
        match Account::of_dir(&dir) {
            Some(a) if a.same(&account) => matched.push((name, own)),
            Some(_) => {}
            None if own => unreadable.push(name),
            None => {}
        }
    }
    if !unreadable.is_empty() {
        return Ok(Holders::Unreadable(unreadable));
    }
    Ok(match matched.len() {
        0 => Holders::None,
        1 => {
            let (name, own_login) = matched.remove(0);
            Holders::One { name, own_login }
        }
        _ => Holders::Many(matched.into_iter().map(|(n, _)| n).collect()),
    })
}

/// The ledger key (uuid, else e-mail) of the account in `~/.claude.json`.
pub fn home_key(home: &Path) -> Option<String> {
    Account::of_dir(home).map(|a| a.key().to_string())
}

/// Whether `~/.claude.json`, read again, still holds the account `key`.
pub fn home_still_matches(home: &Path, key: &str) -> bool {
    Account::of_dir(home).is_some_and(|a| a.matches_key(key))
}
