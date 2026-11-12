//! Short-lived credential identity bound to non-secret file metadata.
//! A refresh invalidates the binding; only a fresh reading can authorize it.

use super::cred_identity::{ReadFn, identity_budget};
use super::identify::{Account, LoginKind, login_kind};
use crate::commands::profile::auto_switch::kit::{CREDS, dir_writable_by_others, euid};
use crate::commands::profile::usage_direct::reason;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
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
    /// Issue #1947: the load-time rule for a stored block, kept with the other
    /// readers of these keys in [`Account::block_is_valid`].
    pub fn valid(&self) -> bool {
        Account::block_is_valid(&self.0)
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

/// Issue #1962: why a login's identity was not read, besides the
/// `usage_direct::reason` of a request that was made. Fixed text, so no server
/// answer can reach a log. See "Identity read causes" in
/// docs/reference/profile-command.md.
pub const DIR_NOT_PRIVATE: &str = "directory not private";
pub const LOGIN_UNTRUSTED: &str = "login file not trusted";
pub const LOGIN_CHANGED: &str = "login changed during read";
pub const BUDGET_SPENT: &str = "identity read budget used up";

/// The refusal for a failed read: a cause gets the fixed start, and a fixed
/// refusal that already says what failed stays as it is.
pub fn unavailable_line(cause: &str) -> String {
    if cause.starts_with("trusted login identity") {
        cause.to_string()
    } else {
        format!("trusted login identity unavailable: {cause}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stamp {
    /// Device, inode, owner, group and mode: group write is accepted only
    /// for the directory's group, so a new group is a new set of writers.
    directory: (u64, u64, u32, u32, u32),
    file: FileStamp,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

fn stamp(dir: &Path) -> Result<Stamp, &'static str> {
    let d = fs::symlink_metadata(dir).map_err(|_| LOGIN_UNTRUSTED)?;
    if !d.is_dir() || d.uid() != euid() || dir_writable_by_others(dir, &d) {
        return Err(DIR_NOT_PRIVATE);
    }
    let f = fs::symlink_metadata(dir.join(CREDS)).map_err(|_| LOGIN_UNTRUSTED)?;
    if !f.is_file()
        || f.uid() != euid()
        || f.mode() & 0o022 != 0
        || f.nlink() != 1
        || d.dev() != f.dev()
    {
        return Err(LOGIN_UNTRUSTED);
    }
    Ok(Stamp {
        directory: (d.dev(), d.ino(), d.uid(), d.gid(), d.mode()),
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

impl Stamp {
    pub(super) fn current(&self, dir: &Path) -> bool {
        stamp(dir).is_ok_and(|s| s == *self)
    }
    /// Rename changes ctime. Every other credential attribute must agree.
    pub(super) fn moved(&self, dir: &Path) -> bool {
        stamp(dir).is_ok_and(|s| {
            let mut expected = self.file.clone();
            expected.changed = s.file.changed;
            expected == s.file
        })
    }
}

/// Shared identity-work budget; unrelated usage measurement is excluded.
/// Issue #2218: it also holds this cycle's local-only folders, whose
/// identity [`Reader::bound`] takes from local files, never a request.
pub struct Budget {
    started: Cell<Instant>,
    local_only: RefCell<Vec<PathBuf>>,
    home: RefCell<Option<PathBuf>>,
    suppressed: RefCell<Vec<PathBuf>>,
}
impl Budget {
    /// Start a new identity-work budget, with no local-only folder.
    pub fn new() -> Self {
        Self {
            started: Cell::new(Instant::now()),
            local_only: RefCell::new(Vec::new()),
            home: RefCell::new(None),
            suppressed: RefCell::new(Vec::new()),
        }
    }
    /// Begin the next cycle's identity work.
    pub fn reset(&self) {
        self.started.set(Instant::now());
        self.suppressed.borrow_mut().clear();
    }
    pub(super) fn set_suppressed(&self, paths: Vec<PathBuf>) {
        *self.suppressed.borrow_mut() = paths;
    }
    pub(super) fn suppressed(&self, dir: &Path) -> bool {
        self.suppressed.borrow().iter().any(|p| same_dir(p, dir))
    }
    /// Whether another bounded identity read may start.
    pub fn available(&self) -> bool {
        self.started.get().elapsed() < identity_budget()
    }
    /// Exclude time spent measuring usage from the identity-work deadline.
    pub fn exclude_measurement(&self, elapsed: Duration) {
        // The measured interval starts after this budget, so shifting its
        // origin cannot exceed the current monotonic time. On an impossible
        // overflow retain the expired budget rather than grant more time.
        if let Some(origin) = self.started.get().checked_add(elapsed) {
            self.started.set(origin);
        }
    }
    /// Issue #2218: replaces the local-only folders. `home` is `$HOME`, so
    /// that `~/.claude`'s account is read from `~/.claude.json`.
    ///
    /// Paths are kept as given and canonicalised, so a caller passing the
    /// resolved path of a folder under a symlinked ancestor still matches.
    pub fn set_local_only(&self, home: &Path, dirs: Vec<PathBuf>) {
        *self.home.borrow_mut() = Some(home.to_path_buf());
        *self.local_only.borrow_mut() = dirs.iter().flat_map(|d| both_forms(d)).collect();
    }
    /// Issue #2218: adds `dir` to the local-only folders.
    pub fn add_local_only(&self, dir: &Path) {
        self.local_only.borrow_mut().extend(both_forms(dir));
    }
    fn is_local_only(&self, dir: &Path) -> bool {
        let forms = both_forms(dir);
        self.local_only.borrow().iter().any(|d| forms.contains(d))
    }
    /// The account of a local-only folder, from its `.claude.json`, or
    /// `~/.claude.json` for `~/.claude`. No request is made.
    fn local_account(&self, dir: &Path) -> Result<Account, &'static str> {
        let home = self.home.borrow();
        let source = match home.as_deref() {
            Some(h) if same_dir(dir, &h.join(".claude")) => h,
            _ => dir,
        };
        super::identify::excluded_offline::local_account(source).ok_or(UNAVAILABLE)
    }
}

/// `dir` as given and, when it resolves, canonicalised. A path that does
/// not resolve names no folder holding a login, so its raw form is enough.
fn both_forms(dir: &Path) -> Vec<PathBuf> {
    let mut forms = vec![dir.to_path_buf()];
    if let Ok(c) = fs::canonicalize(dir)
        && c != dir
    {
        forms.push(c);
    }
    forms
}

/// Whether `a` and `b` name the same folder, by raw or canonical path.
fn same_dir(a: &Path, b: &Path) -> bool {
    let b = both_forms(b);
    both_forms(a).iter().any(|x| b.contains(x))
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
    pub(super) fn directory(&self) -> &Path {
        &self.dir
    }
    pub(super) fn stamp(&self) -> Stamp {
        self.stamp.clone()
    }
    /// The account returned for this login; absent only for platform logins.
    pub fn account(&self) -> Option<&Account> {
        self.account.as_ref()
    }
    /// Whether the observed private directory and login are unchanged.
    pub fn current(&self) -> bool {
        stamp(&self.dir).is_ok_and(|now| now == self.stamp)
    }
    /// Rebinds the exact login after our journalled rename while its
    /// refresh lock is held. Rename changes ctime, never file contents,
    /// inode, size, mtime, owner, mode or link count.
    pub fn moved_to(&self, dir: &Path) -> Result<Self, &'static str> {
        let after = stamp(dir)?;
        let mut expected = self.stamp.file.clone();
        expected.changed = after.file.changed;
        if expected != after.file {
            return Err(LOGIN_CHANGED);
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
    /// Parking requires server evidence even when ordinary excluded reads are local.
    pub(super) fn server_bound(&self, dir: &Path) -> Result<Bound, &'static str> {
        if self.budget.is_local_only(dir) {
            return Err(UNAVAILABLE);
        }
        self.bound(dir, false)
    }
    /// Reads only an unchanged private regular login; platform evidence must
    /// be explicitly permitted by the caller's existing kind policy. The
    /// error is the cause, one of "Identity read causes".
    pub fn bound(&self, dir: &Path, platform: bool) -> Result<Bound, &'static str> {
        if self.budget.suppressed(dir) {
            return Err("needs-login");
        }
        if !self.budget.available() {
            return Err(BUDGET_SPENT);
        }
        let before = stamp(dir)?;
        let account = match login_kind(&dir.join(CREDS)) {
            LoginKind::OAuth if self.budget.is_local_only(dir) => {
                Some(self.budget.local_account(dir)?)
            }
            LoginKind::OAuth => Some((self.read)(dir)?),
            LoginKind::NoOAuth if platform => None,
            LoginKind::NoOAuth => return Err(reason::NO_TOKEN),
            LoginKind::Unreadable | LoginKind::Missing => return Err(LOGIN_UNTRUSTED),
        };
        if !stamp(dir).is_ok_and(|now| now == before) {
            return Err(LOGIN_CHANGED);
        }
        Ok(Bound {
            dir: dir.to_path_buf(),
            stamp: before,
            account,
        })
    }
    /// Read an OAuth identity without accepting a platform login.
    pub fn oauth(&self, dir: &Path) -> Result<Account, &'static str> {
        self.bound(dir, false)?.account.ok_or(reason::NO_TOKEN)
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

    /// Issue #2218: the folders whose identity this cycle takes from local
    /// files only; replaces the previous set.
    pub(super) fn set_excluded_dirs(&self, dirs: Vec<PathBuf>) {
        self.identity_budget.set_local_only(&self.home, dirs);
    }

    /// Issue #2218: whether `~/.claude` holds an excluded login this cycle,
    /// so its identity is local evidence only and hand-change recovery,
    /// which would move logins on that evidence, does not run.
    pub(super) fn home_local_only(&self) -> bool {
        self.identity_budget
            .is_local_only(&self.home.join(".claude"))
    }
}

#[cfg(test)]
#[path = "tests_bound_identity.rs"]
mod tests;
