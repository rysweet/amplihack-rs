//! The login fingerprint (issue #1618): which account the retry would use.
//! The wait ends early when it changes, because a rotation or auto-switch
//! moved a login with capacity into place. For `$HOME/.claude` the wait also
//! ends when the rotator records a fresh cycle with capacity
//! ([`Watch::rotator_capacity`]), the signal the limit-resume hook uses.
//!
//! The watched directory is the one the retry runs with: the inherited
//! profile's, else `$CLAUDE_CONFIG_DIR`, else `$HOME/.claude`. Three parts
//! are compared: the ledger account (auto-switch ledger for a profile,
//! rotate ledger's `live` for `$HOME/.claude`), the `.claude.json` account
//! (`rotate::identify::Account`), and whether `.credentials.json` exists.
//! The credentials file is never opened, so an OAuth token refresh is not a
//! change.
//!
//! Every read error counts as absent and is never reported. A fingerprint
//! is only compared: it has no `Debug`, `Display` or `Clone`.
//!
//! Issue #1761: [`Watch::rotator_running`] says whether a rotator serves the
//! watched directory, and [`Watch::login`] names the profile a retry
//! resumes on. Both only read files; neither signals a process.

use super::super::auto_switch::{self, CREDS};
use super::super::fsx;
use super::super::name::ProfileName;
use super::super::rotate::identify::Account;
use super::super::store::ProfileStore;
use amplihack_utils::rotator_status::{CycleStatus, read_cycle, rotator_down};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(PartialEq, Eq)]
pub(crate) struct Fingerprint {
    ledger: Option<String>,
    identity: Option<Account>,
    creds: bool,
}

enum Kind {
    /// `<root>/<name>`, with the auto-switch ledger under `root`.
    Profile { root: PathBuf, name: String },
    /// `$CLAUDE_CONFIG_DIR`, with no ledger.
    ConfigDir,
    /// `$HOME/.claude`, with the rotate ledger under `root` naming the
    /// profile live there. `json` holds the `.claude.json` Claude reads:
    /// `$HOME` by default, `$CLAUDE_CONFIG_DIR` when that is set to it.
    Home {
        json: PathBuf,
        root: Option<PathBuf>,
    },
}

pub(crate) struct Watch {
    dir: Option<PathBuf>,
    kind: Kind,
}

impl Watch {
    pub(crate) fn new(
        store: Option<&ProfileStore>,
        inherited: Option<&ProfileName>,
        config_dir: Option<&Path>,
        home: Option<&Path>,
    ) -> Self {
        let root = store.map(|s| s.root().to_path_buf());
        if let (Some(root), Some(name)) = (&root, inherited) {
            return Self {
                dir: Some(root.join(name.as_str())),
                kind: Kind::Profile {
                    root: root.clone(),
                    name: name.as_str().to_string(),
                },
            };
        }
        let home_dir = home.map(|h| h.join(".claude"));
        match home {
            Some(home) if config_dir.is_none() || config_dir == home_dir.as_deref() => Self {
                dir: home_dir,
                kind: Kind::Home {
                    json: config_dir.unwrap_or(home).to_path_buf(),
                    root,
                },
            },
            _ => Self {
                dir: config_dir.map(Path::to_path_buf),
                kind: Kind::ConfigDir,
            },
        }
    }

    /// The configuration directory the retry will use.
    pub(crate) fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The fingerprint now; `None` only when no directory is watched.
    pub(crate) fn probe(&self) -> Option<Fingerprint> {
        let dir = self.dir()?;
        let (ledger, json_dir) = match &self.kind {
            Kind::Profile { root, name } => (auto_switch::rerun::ledger_account(root, name), dir),
            Kind::ConfigDir => (None, dir),
            Kind::Home { json, .. } => (self.cycle().and_then(|c| c.live), json.as_path()),
        };
        Some(Fingerprint {
            ledger,
            identity: identity(json_dir),
            creds: fsx::exists(&dir.join(CREDS)),
        })
    }

    /// The rotate ledger's last cycle, for a `$HOME/.claude` watch only:
    /// the rotator swaps logins into that directory and no other.
    fn cycle(&self) -> Option<CycleStatus> {
        match &self.kind {
            Kind::Home {
                root: Some(root), ..
            } => read_cycle(root, fsx::now()),
            _ => None,
        }
    }

    /// The rotator recorded a cycle with capacity whose usage probe began
    /// after `start` (its `measured_at`, or, from a rotator older than
    /// #1659, a cycle recorded more than a minute after `start`), so the
    /// login now live in `$HOME/.claude` has capacity even if it is the one
    /// that hit the limit (its limit has since reset). The rule is
    /// [`CycleStatus::fresh_capacity`], shared with the limit-resume hook.
    pub(crate) fn rotator_capacity(&self, start: i64) -> bool {
        self.cycle().is_some_and(|c| c.fresh_capacity(start))
    }

    /// A rotator runs for the watched directory: only `$HOME/.claude` with a
    /// known profile root, whose pid file names a running rotator of this
    /// user and whose rotate ledger is readable or absent
    /// ([`rotator_down`]). Not a security boundary: the same user can fake
    /// one, and the wait's caps still hold.
    pub(crate) fn rotator_running(&self) -> bool {
        match &self.kind {
            Kind::Home {
                root: Some(root), ..
            } => !rotator_down(root, fsx::now()),
            _ => false,
        }
    }

    /// The profile a retry resumes on: an inherited profile's own name, the
    /// rotate ledger's live profile for `$HOME/.claude`, else `None`.
    pub(crate) fn login(&self) -> Option<String> {
        match &self.kind {
            Kind::Profile { name, .. } => Some(name.clone()),
            Kind::Home { .. } => self.cycle().and_then(|c| c.live),
            Kind::ConfigDir => None,
        }
    }
}

/// The account in `dir/.claude.json`, from a private regular file only.
fn identity(dir: &Path) -> Option<Account> {
    let (v, meta) = auto_switch::read_claude_json(dir)?;
    if meta.uid() != fsx::euid() || fsx::writable_by_others(&meta) {
        return None;
    }
    Account::from_value(&v)
}
