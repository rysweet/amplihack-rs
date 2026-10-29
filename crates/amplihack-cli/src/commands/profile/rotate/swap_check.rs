//! The checks a swap runs before anything is written (D9), split from
//! `swap.rs`: `$HOME` and `~/.claude` ownership, the incoming profile's
//! account block and, issue #1700, the refusal of a login without OAuth for
//! a subscription profile.

use super::identify::{LoginKind, login_kind};
use super::swap::{DOT_CLAUDE_NOT_PRIVATE, HOME_NOT_OWNED};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{euid, read_claude_json, writable_by_others};
use crate::commands::profile::name::ProfileName;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

fn refused(msg: impl Into<String>) -> ProfileError {
    ProfileError::RotateRefused(msg.into())
}

/// `$HOME` and `~/.claude` may be changed by this user only.
pub(super) fn check_home(home: &Path) -> Result<PathBuf, ProfileError> {
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
pub(super) fn account_block(dir: &Path) -> Option<serde_json::Value> {
    read_claude_json(dir)?
        .0
        .get("oauthAccount")
        .filter(|b| b.is_object())
        .cloned()
}

/// Issue #1700: the end of the refusal for a login without OAuth that would
/// leave `~/.claude` on the API key for a subscription profile.
const NO_OAUTH: &str = "has no oauth login and is not an API-key profile";

/// The refusal for `to`'s login file, which is not an OAuth login while
/// `to` is not an API-key profile.
pub(super) fn no_oauth(to: &ProfileName) -> ProfileError {
    refused(format!("{to} {NO_OAUTH}"))
}

/// Whether `e` is that refusal: the cycle writes its ALERT once per
/// profile, keyed `no_oauth:<to>`.
pub fn is_no_oauth(e: &ProfileError) -> bool {
    matches!(e, ProfileError::RotateRefused(m) if m.ends_with(NO_OAUTH))
}

/// Whether the login at `t` may make `to` live in `~/.claude`: an OAuth
/// login, or any login of an API-key profile.
pub(super) fn may_go_live(t: &Path, platform: bool) -> bool {
    platform || login_kind(t) == LoginKind::OAuth
}
