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
//! Every read error counts as absent and is never reported, with one
//! exception: a `.claude.json` that is there but does not parse, which is
//! what a read made while another process is half-way through writing it
//! looks like. That makes the whole fingerprint unreadable ([`Watch::probe`]
//! returns `None`), and the wait treats it as no reading at all rather than
//! as a login that changed (`wait`). A fingerprint is only compared: it has
//! no `Debug`, `Display` or `Clone`.
//!
//! [`Watch::files`] lists every file the wait's checks read, which is what
//! the wait blocks on (`wake`).

use super::super::auto_switch;
use super::super::auto_switch::kit::{
    CLAUDE_JSON, CREDS, HOME_USAGE, MAX_CLAUDE_JSON, STATE_DIR, USAGE,
};
use super::super::fsx;
use super::super::name::ProfileName;
use super::super::rotate::identify::Account;
use super::super::store::ProfileStore;
use amplihack_utils::rotator_status::swap_record::{SWAP_FILE, rotate_dir};
use amplihack_utils::rotator_status::{CycleStatus, Ledger, read_cycle, read_ledger, swap_record};
use amplihack_utils::secure_files::read_bounded_nofollow;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The ledger's file name, in the auto-switch state directory and in the
/// rotate directory alike.
const LEDGER: &str = "ledger.json";

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

/// Which file a [`Watch::login`] key was read from.
///
/// Derived from which reader succeeded and from nothing either file holds,
/// so no field of a ledger or a swap record can influence it.
///
/// It exists because the fallback is silent. `login` prefers the rotate
/// ledger's `live` and falls back to the swap record's `live_login`, and the
/// two name different things: the ledger names the login that is live now,
/// the record names the login the *last* swap put in place. A ledger that is
/// missing, unreadable or stale between two readings therefore moves the
/// answer to the other file, and on a host that has swapped before the two
/// readings carry different names with no swap between them at all. Comparing
/// them would report a change that did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// The rotate ledger's `live` profile.
    Cycle,
    /// The swap record's `live_login`, read when the ledger names none.
    Swap,
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

    /// The step uses `$HOME/.claude`, the one directory the rotator swaps
    /// logins into.
    pub(crate) fn is_home(&self) -> bool {
        matches!(self.kind, Kind::Home { .. })
    }

    /// The profile root this watch was built with (issue #2890), where the
    /// rotate ledger lives; `None` for a `$CLAUDE_CONFIG_DIR` step and for
    /// a `$HOME/.claude` step with no store.
    pub(crate) fn store_root(&self) -> Option<&Path> {
        match &self.kind {
            Kind::Profile { root, .. } => Some(root),
            Kind::Home { root, .. } => root.as_deref(),
            Kind::ConfigDir => None,
        }
    }

    /// The inherited profile this watch names, parsed (issue #2890); `None`
    /// for a `$HOME/.claude` or `$CLAUDE_CONFIG_DIR` step, and for a name
    /// the rules refuse.
    pub(crate) fn inherited_profile(&self) -> Option<ProfileName> {
        match &self.kind {
            Kind::Profile { name, .. } => ProfileName::parse(name).ok(),
            Kind::Home { .. } | Kind::ConfigDir => None,
        }
    }

    /// The fingerprint now; `None` when no directory is watched, and when
    /// `.claude.json` is there but does not parse, which is a read made
    /// part-way through a write rather than a login.
    pub(crate) fn probe(&self) -> Option<Fingerprint> {
        let dir = self.dir()?;
        let (ledger, json_dir) = match &self.kind {
            Kind::Profile { root, name } => (auto_switch::rerun::ledger_account(root, name), dir),
            Kind::ConfigDir => (None, dir),
            Kind::Home { json, .. } => (self.cycle().and_then(|c| c.live), json.as_path()),
        };
        Some(Fingerprint {
            ledger,
            identity: identity(json_dir).ok()?,
            creds: fsx::exists(&dir.join(CREDS)),
        })
    }

    /// Every file the wait's checks read for this watch, as `(directory,
    /// name)`: the fingerprint's, the login key's, the rotator's and the
    /// auto-switch readings'. The wait blocks on writes to these and reads
    /// nothing on a timer (issue #2911).
    pub(crate) fn files(&self) -> Vec<(PathBuf, &'static str)> {
        let mut out = Vec::new();
        if let Some(dir) = self.dir() {
            out.push((dir.to_path_buf(), CREDS));
        }
        let root = match &self.kind {
            Kind::Profile { root, .. } => {
                if let Some(dir) = self.dir() {
                    out.push((dir.to_path_buf(), CLAUDE_JSON));
                }
                Some(root.as_path())
            }
            Kind::ConfigDir => {
                if let Some(dir) = self.dir() {
                    out.push((dir.to_path_buf(), CLAUDE_JSON));
                }
                None
            }
            Kind::Home { json, root } => {
                out.push((json.clone(), CLAUDE_JSON));
                if let Some(root) = root {
                    let rotate = rotate_dir(root);
                    out.push((rotate.clone(), LEDGER));
                    out.push((rotate, SWAP_FILE));
                }
                root.as_deref()
            }
        };
        if let Some(root) = root {
            let state = root.join(STATE_DIR);
            for name in [LEDGER, USAGE, HOME_USAGE] {
                out.push((state.clone(), name));
            }
        }
        out
    }

    /// The **login key**: the name of the login a `$HOME/.claude` step would
    /// run on now, and which file said so (issue #2731). It is the rotate
    /// ledger's `live` profile, else the swap record's live login, both
    /// written by the rotator under its own lock and both held to
    /// [`valid_live_name`] by their own readers. `None` for any other kind
    /// of step, for a step with no profile root, and whenever neither file
    /// can be read.
    ///
    /// The [`Source`] rides with the name because the two readers answer
    /// slightly different questions, so a reading from one is not comparable
    /// with a reading from the other: see that type. Callers comparing two
    /// readings must compare the pair, never the two names alone.
    ///
    /// This is not the fingerprint. The fingerprint says when a wait ends and
    /// has no readable form; the key says which waits a step may still make
    /// and is the `account=` field of the recipe-log lines. It is never taken
    /// from `.claude.json`, whose account may hold an e-mail address or a
    /// UUID, and it is always in the rotator's own name form, so it needs no
    /// quoting in a log line.
    ///
    /// [`valid_live_name`]: amplihack_utils::rotator_status::valid_live_name
    pub(crate) fn login(&self) -> Option<(Source, String)> {
        let Kind::Home { root, .. } = &self.kind else {
            return None;
        };
        let root = root.as_deref()?;
        let now = fsx::now();
        if let Some(live) = read_cycle(root, now).and_then(|c| c.live) {
            return Some((Source::Cycle, live));
        }
        swap_record::read_swap(root, now).map(|r| (Source::Swap, r.live_login))
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
        self.cycle().is_some_and(|c| c.fresh_room(start))
    }

    /// The rotate ledger under this watch's root, as the checked reader
    /// sees it (issue #2857).
    ///
    /// [`Ledger::Missing`] and [`Ledger::Unreadable`] are different answers
    /// and the room gate treats them differently: nothing to consult, as
    /// against a record that cannot be trusted. A watch the rotator never
    /// touches, and a `$HOME/.claude` watch with no profile root, have
    /// nothing to consult either, so both read as missing.
    pub(crate) fn rotator_state(&self) -> Ledger {
        match &self.kind {
            Kind::Home {
                root: Some(root), ..
            } => read_ledger(root, fsx::now()),
            _ => Ledger::Missing,
        }
    }
}

/// A `.claude.json` that is there but does not parse.
struct HalfWritten;

/// The account in `dir/.claude.json`, from a private regular file only.
///
/// `Ok(None)` when there is no account to read: no file, a file that is not
/// a private regular file, or one with no account in it. `Err` only when the
/// file is there and is not JSON, which is what a read made during a write
/// looks like; the write that completes it wakes the wait again.
fn identity(dir: &Path) -> Result<Option<Account>, HalfWritten> {
    let Ok(Some((raw, meta))) = read_bounded_nofollow(&dir.join(CLAUDE_JSON), MAX_CLAUDE_JSON)
    else {
        return Ok(None);
    };
    if meta.uid() != fsx::euid() || fsx::writable_by_others(&meta) {
        return Ok(None);
    }
    let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| HalfWritten)?;
    Ok(v.is_object().then(|| Account::from_value(&v)).flatten())
}
