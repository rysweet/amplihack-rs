//! The login fingerprint (issue #1618): which account the retry would use.
//! The wait ends early when it changes, because a rotation or auto-switch
//! moved a login with capacity into place.
//!
//! The watched directory is the one the retry runs with: the inherited
//! profile's, else `$CLAUDE_CONFIG_DIR`, else `$HOME/.claude`. Three parts
//! are compared: the ledger account (auto-switch ledger for a profile,
//! rotate ledger's `live` for `$HOME/.claude`), the account identity in
//! `.claude.json`, and whether `.credentials.json` exists. The credentials
//! file is never opened, so an OAuth token refresh is not a change.
//!
//! Every read error counts as absent and is never reported. A fingerprint
//! is only compared: it has no `Debug`, `Display` or `Clone`.

use super::super::auto_switch::{self, CREDS};
use super::super::fsx;
use super::super::name::ProfileName;
use super::super::store::ProfileStore;
use amplihack_utils::rotator_status::read_cycle;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(PartialEq, Eq)]
pub(crate) struct Fingerprint {
    ledger: Option<String>,
    identity: Option<Identity>,
    creds: bool,
}

#[derive(PartialEq, Eq)]
enum Identity {
    Uuid(String),
    Email(String),
}

enum Kind {
    /// `<root>/<name>`, with the auto-switch ledger under `root`.
    Profile { root: PathBuf, name: String },
    /// `$CLAUDE_CONFIG_DIR`, with no ledger.
    ConfigDir,
    /// `$HOME/.claude`, with the rotator ledger under `root` naming the
    /// account live there. `json` holds the `.claude.json` Claude reads:
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
            Kind::Home { json, root } => (
                root.as_deref()
                    .and_then(|r| read_cycle(r, fsx::now()))
                    .and_then(|c| c.live),
                json.as_path(),
            ),
        };
        Some(Fingerprint {
            ledger,
            identity: identity(json_dir),
            creds: fsx::exists(&dir.join(CREDS)),
        })
    }
}

/// `oauthAccount.accountUuid` of `dir/.claude.json`, else its
/// `emailAddress`, from a private regular file only.
fn identity(dir: &Path) -> Option<Identity> {
    let (v, meta) = auto_switch::read_claude_json(dir)?;
    if meta.uid() != fsx::euid() || fsx::writable_by_others(&meta) {
        return None;
    }
    let block = v.get("oauthAccount")?;
    let field = |k: &str| {
        block
            .get(k)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    field("accountUuid")
        .map(Identity::Uuid)
        .or_else(|| field("emailAddress").map(Identity::Email))
}
