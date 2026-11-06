//! Short-lived credential identity bound to non-secret file metadata.
//! A refresh invalidates the binding; only a fresh reading can authorize it.

use super::cred_identity::{ReadFn, identity_budget};
use super::identify::{Account, LoginKind, login_kind};
use crate::commands::profile::auto_switch::kit::{CREDS, euid};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Sanitized full account provenance, stored only in the existing state files.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Identity(serde_json::Value);
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Identity(<redacted>)")
    }
}
impl Identity {
    /// Capture an account returned by the trusted reader.
    pub fn new(account: &Account) -> Self {
        Self(account.block())
    }
    /// Compare all available account and organization fields; invalid state refuses.
    pub fn names(&self, account: &Account) -> bool {
        Account::from_block(&self.0).is_some_and(|prior| prior.same(account))
    }
}

/// The full identities authorized before an exchange; no token is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwapIdentity {
    /// The authorized outgoing OAuth account, or an explicit platform login.
    pub outgoing: Option<Identity>,
    /// The authorized incoming OAuth account, or an explicit platform login.
    pub incoming: Option<Identity>,
}
impl SwapIdentity {
    /// Retain validated evidence for interrupted recovery.
    pub fn new(outgoing: Option<&Account>, incoming: Option<&Account>) -> Self {
        Self {
            outgoing: outgoing.map(Identity::new),
            incoming: incoming.map(Identity::new),
        }
    }
    /// Fresh evidence must identify exactly the accounts authorized before movement.
    pub fn names(&self, outgoing: Option<&Account>, incoming: Option<&Account>) -> bool {
        let same = |prior: Option<&Identity>, now: Option<&Account>| match (prior, now) {
            (Some(prior), Some(now)) => prior.names(now),
            (None, None) => true,
            _ => false,
        };
        same(self.outgoing.as_ref(), outgoing) && same(self.incoming.as_ref(), incoming)
    }
}

/// Fixed, redacted refusal; no file content or server response is included.
pub const UNAVAILABLE: &str =
    "trusted login identity unavailable or changed; retry after checking profile verify";

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    directory: (u64, u64, u32, u32),
    file: FileStamp,
}
#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    dev: u64,
    ino: u64,
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    uid: u32,
    mode: u32,
    links: u64,
}

fn stamp(dir: &Path) -> Option<Stamp> {
    let d = fs::symlink_metadata(dir).ok()?;
    let f = fs::symlink_metadata(dir.join(CREDS)).ok()?;
    if !d.is_dir()
        || d.uid() != euid()
        || d.mode() & 0o022 != 0
        || !f.is_file()
        || f.uid() != euid()
        || f.mode() & 0o022 != 0
        || f.nlink() != 1
        || d.dev() != f.dev()
    {
        return None;
    }
    Some(Stamp {
        directory: (d.dev(), d.ino(), d.uid(), d.mode()),
        file: FileStamp {
            dev: f.dev(),
            ino: f.ino(),
            len: f.len(),
            modified: (f.mtime(), f.mtime_nsec()),
            changed: (f.ctime(), f.ctime_nsec()),
            uid: f.uid(),
            mode: f.mode(),
            links: f.nlink(),
        },
    })
}

/// Shared identity-work budget; unrelated usage measurement is excluded.
pub struct Budget(Cell<Instant>);
impl Budget {
    /// Start a new identity-work budget.
    pub fn new() -> Self {
        Self(Cell::new(Instant::now()))
    }
    /// Begin the next cycle's identity work.
    pub fn reset(&self) {
        self.0.set(Instant::now());
    }
    /// Whether another bounded identity read may start.
    pub fn available(&self) -> bool {
        self.0.get().elapsed() < identity_budget()
    }
    /// Exclude time spent measuring usage from the identity-work deadline.
    pub fn exclude_measurement(&self, elapsed: Duration) {
        // The measured interval starts after this budget, so shifting its
        // origin cannot exceed the current monotonic time. On an impossible
        // overflow retain the expired budget rather than grant more time.
        if let Some(origin) = self.0.get().checked_add(elapsed) {
            self.0.set(origin);
        }
    }
}

/// The bounded reader and the budget shared by all of its callers.
#[derive(Clone, Copy)]
pub struct Reader<'a> {
    /// Credential-bound account reader, or an explicit synthetic test reader.
    pub read: &'a dyn Fn(&Path) -> Result<Account, &'static str>,
    /// Identity-work deadline shared by this cycle's readers.
    pub budget: &'a Budget,
}

/// Evidence kept only in memory. Debug never exposes account or metadata.
#[derive(Clone)]
pub struct Bound {
    dir: PathBuf,
    stamp: Stamp,
    account: Option<Account>,
}
impl std::fmt::Debug for Bound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Bound(..)")
    }
}
impl Bound {
    /// The account returned for this login; absent only for platform logins.
    pub fn account(&self) -> Option<&Account> {
        self.account.as_ref()
    }
    /// Whether the observed private directory and login are unchanged.
    pub fn current(&self) -> bool {
        stamp(&self.dir).as_ref() == Some(&self.stamp)
    }
    /// Rebinds the exact login after our journalled rename while its
    /// refresh lock is held. Rename changes ctime, never file contents,
    /// inode, size, mtime, owner, mode or link count.
    pub fn moved_to(&self, dir: &Path) -> Result<Self, &'static str> {
        let after = stamp(dir).ok_or(UNAVAILABLE)?;
        let mut expected = self.stamp.file.clone();
        expected.changed = after.file.changed;
        if expected != after.file {
            return Err(UNAVAILABLE);
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            stamp: after,
            account: self.account.clone(),
        })
    }
    /// Full account continuity, allowing a same-account credential refresh.
    pub fn same(&self, other: &Self) -> bool {
        match (&self.account, &other.account) {
            (Some(a), Some(b)) => a.same(b),
            (None, None) => true,
            _ => false,
        }
    }
}
impl Reader<'_> {
    /// Reads only an unchanged private regular login; platform evidence must
    /// be explicitly permitted by the caller's existing kind policy.
    pub fn bound(&self, dir: &Path, platform: bool) -> Result<Bound, &'static str> {
        if !self.budget.available() {
            return Err(UNAVAILABLE);
        }
        let before = stamp(dir).ok_or(UNAVAILABLE)?;
        let account = match login_kind(&dir.join(CREDS)) {
            LoginKind::OAuth => Some((self.read)(dir).map_err(|_| UNAVAILABLE)?),
            LoginKind::NoOAuth if platform => None,
            _ => return Err(UNAVAILABLE),
        };
        if stamp(dir).as_ref() != Some(&before) {
            return Err(UNAVAILABLE);
        }
        Ok(Bound {
            dir: dir.to_path_buf(),
            stamp: before,
            account,
        })
    }
    /// Read an OAuth identity without accepting a platform login.
    pub fn oauth(&self, dir: &Path) -> Result<Account, &'static str> {
        self.bound(dir, false)?.account.ok_or(UNAVAILABLE)
    }
}

/// Default bounded reader for standalone rotation entry points.
pub fn default_read() -> ReadFn {
    super::cred_identity::DEFAULT_READ
}

impl super::cycle::Ctx {
    pub(super) fn identity_reader(&self) -> super::login_identity::Reader<'_> {
        super::login_identity::Reader {
            read: &self.identity,
            budget: &self.identity_budget,
        }
    }
}

#[cfg(test)]
#[path = "tests_bound_identity.rs"]
mod tests;
