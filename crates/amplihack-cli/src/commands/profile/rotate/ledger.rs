//! `<root>/.auto-switch.d/rotate/`: the rotate ledger and journal, the pid
//! file and the log (D7). Ledger and journal are versioned, capped at 64 KiB
//! and written privately and atomically. The journal rejects unknown fields;
//! the ledger ignores them (#1659), so a field added by a newer amplihack
//! never stops an older rotator. A file that fails to parse is reported and
//! never overwritten here.

use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    check_root_private, ensure_private_dir, euid, fsync_dir, read_bounded_nofollow,
    writable_by_others, write_private_atomic,
};
use crate::commands::profile::name::ProfileName;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;
const MAX_STATE: u64 = 64 * 1024;
/// The longest account value accepted (an e-mail address is at most 320).
pub const MAX_ACCOUNT: usize = 320;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastCycle {
    /// When the cycle was recorded, at its end.
    pub at: i64,
    /// When the cycle started measuring usage (issue #1659): a waiting
    /// limit-resume hook trusts only a measurement taken after its limit.
    /// Absent for a cycle that measured nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<i64>,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotateLedger {
    pub version: u32,
    /// The profile whose login is in `~/.claude`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<String>,
    /// That login's account (`accountUuid`, else `emailAddress`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// When `live` last changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swapped_at: Option<i64>,
    /// The profile swapped out by the last swap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    /// The ALERT de-duplication key, `<kind>:<live or ->`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<LastCycle>,
}

impl Default for RotateLedger {
    fn default() -> Self {
        Self {
            version: VERSION,
            live: None,
            account: None,
            since: None,
            swapped_at: None,
            previous: None,
            alert: None,
            last_cycle: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    J0,
    S1,
    S2,
    S3,
    S4,
    Inconsistent,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::J0 => "j0",
            Self::S1 => "s1",
            Self::S2 => "s2",
            Self::S3 => "s3",
            Self::S4 => "s4",
            Self::Inconsistent => "inconsistent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotateJournal {
    pub version: u32,
    pub from: String,
    pub to: String,
    /// `from` was created by adopting the login in `~/.claude`.
    pub adopt: bool,
    pub phase: Phase,
    /// `$HOME` when the swap started. Only ever compared, never a path.
    pub home: String,
    /// Inode of the outgoing login (`~/.claude/.credentials.json`).
    pub live_ino: u64,
    /// Inode of the incoming login (`<root>/<to>/.credentials.json`).
    pub target_ino: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_lock_ino: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_lock_ino: Option<u64>,
    pub started_at: i64,
    pub pid: u32,
}

/// Paths of the rotate state.
#[derive(Debug, Clone)]
pub struct RotateState {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub ledger: PathBuf,
    pub journal: PathBuf,
    pub pid: PathBuf,
    pub log: PathBuf,
    pub log1: PathBuf,
    /// The background process's own stdout and stderr.
    pub err: PathBuf,
}

fn inconsistent(msg: impl Into<String>) -> ProfileError {
    ProfileError::RotateInconsistent(msg.into())
}

/// An account value amplihack may store: non-empty, at most 320 bytes, no
/// control characters.
pub fn safe_account(v: &str) -> bool {
    !v.is_empty() && v.len() <= MAX_ACCOUNT && !v.chars().any(char::is_control)
}

fn valid_name(v: Option<&str>) -> bool {
    v.is_none_or(|n| ProfileName::parse(n).is_ok())
}

impl RotateState {
    /// `root` is canonicalised when it exists.
    pub fn new(root: &Path) -> Self {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let dir = root.join(".auto-switch.d").join("rotate");
        Self {
            ledger: dir.join("ledger.json"),
            journal: dir.join("journal.json"),
            pid: dir.join("rotate.pid"),
            log: dir.join("rotate.log"),
            log1: dir.join("rotate.log.1"),
            err: dir.join("rotate.err"),
            dir,
            root,
        }
    }

    /// Checks the root and creates the state directories, private.
    pub fn ensure(&self) -> Result<(), ProfileError> {
        let refuse =
            |what: &str, e: std::io::Error| ProfileError::RotateRefused(format!("{what} {e}"));
        check_root_private(&self.root).map_err(|e| refuse("profile root", e))?;
        let parent = self.dir.parent().unwrap_or(&self.root);
        ensure_private_dir(parent).map_err(|e| refuse(".auto-switch.d", e))?;
        ensure_private_dir(&self.dir).map_err(|e| refuse(".auto-switch.d/rotate", e))
    }

    /// A state file: `None` when missing; an error for a symlink, a file
    /// over 64 KiB, or one that is someone else's or writable by others.
    fn read(&self, path: &Path, what: &str) -> Result<Option<Vec<u8>>, ProfileError> {
        match read_bounded_nofollow(path, MAX_STATE) {
            Ok(None) => Ok(None),
            Ok(Some((raw, m))) if m.uid() == euid() && !writable_by_others(&m) => Ok(Some(raw)),
            Ok(Some(_)) => Err(inconsistent(format!("{what}: not a private file"))),
            Err(e) => Err(inconsistent(format!("{what}: {e}"))),
        }
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), ProfileError> {
        self.ensure()?;
        write_private_atomic(&self.dir, name, bytes).map_err(ProfileError::Io)
    }

    pub fn load_ledger(&self) -> Result<Option<RotateLedger>, ProfileError> {
        const WHAT: &str = "rotate ledger";
        let Some(raw) = self.read(&self.ledger, WHAT)? else {
            return Ok(None);
        };
        let l: RotateLedger = serde_json::from_slice(&raw)
            .map_err(|e| inconsistent(format!("{WHAT}: {}", position(&e))))?;
        if l.version != VERSION {
            return Err(inconsistent(format!("{WHAT}: unknown version")));
        }
        if !valid_name(l.live.as_deref()) || !valid_name(l.previous.as_deref()) {
            return Err(inconsistent(format!("{WHAT}: invalid profile name")));
        }
        if l.account.as_deref().is_some_and(|a| !safe_account(a))
            || l.alert.as_deref().is_some_and(|a| !safe_account(a))
        {
            return Err(inconsistent(format!("{WHAT}: invalid account or alert")));
        }
        Ok(Some(l))
    }

    /// Writes the ledger. While a rotator from another amplihack binary
    /// runs ([`Self::foreign_rotator`]), it is written as that binary reads
    /// it ([`for_older_readers`]).
    pub fn save_ledger(&self, l: &RotateLedger) -> Result<(), ProfileError> {
        let older = self.foreign_rotator().is_some();
        let bytes = serde_json::to_vec_pretty(&for_older_readers(l, older))
            .map_err(|e| inconsistent(e.to_string()))?;
        self.write("ledger.json", &bytes)
    }

    /// The pid of a running `profile rotate watch` whose executable is not
    /// this process's, for example one started before an upgrade.
    pub fn foreign_rotator(&self) -> Option<u32> {
        let exe = std::env::current_exe().ok();
        amplihack_utils::rotator_status::foreign_rotator(
            &self.pid,
            Path::new("/proc"),
            exe.as_deref(),
        )
    }

    pub fn load_journal(&self) -> Result<Option<RotateJournal>, ProfileError> {
        const WHAT: &str = "rotate journal";
        let Some(raw) = self.read(&self.journal, WHAT)? else {
            return Ok(None);
        };
        let j: RotateJournal = serde_json::from_slice(&raw)
            .map_err(|e| inconsistent(format!("{WHAT}: {}", position(&e))))?;
        if j.version != VERSION {
            return Err(inconsistent(format!("{WHAT}: unknown version")));
        }
        if ProfileName::parse(&j.from).is_err() || ProfileName::parse(&j.to).is_err() {
            return Err(inconsistent(format!("{WHAT}: invalid profile name")));
        }
        Ok(Some(j))
    }

    pub fn save_journal(&self, j: &RotateJournal) -> Result<(), ProfileError> {
        let bytes = serde_json::to_vec_pretty(j).map_err(|e| inconsistent(e.to_string()))?;
        self.write("journal.json", &bytes)
    }

    pub fn delete_journal(&self) -> Result<(), ProfileError> {
        match std::fs::remove_file(&self.journal) {
            Ok(()) => fsync_dir(&self.dir).map_err(ProfileError::Io),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// The ledger as written for its readers. With `older` (a rotator from
/// another binary runs), fields added after the ledger first shipped are
/// left out, because amplihack before #1659 rejects unknown ledger fields:
/// today `last_cycle.measured_at`, without which the limit-resume hook
/// falls back to its 60-second margin.
pub fn for_older_readers(l: &RotateLedger, older: bool) -> RotateLedger {
    let mut out = l.clone();
    if older && let Some(c) = &mut out.last_cycle {
        c.measured_at = None;
    }
    out
}

/// The one line printed when a rotator from another amplihack binary runs.
pub fn foreign_rotator_note(pid: u32) -> String {
    format!(
        "amplihack: the Claude login rotator (pid {pid}) runs another amplihack binary, so the \
         rotate ledger is written in the format it reads; restart it to finish the upgrade: \
         amplihack profile rotate stop && amplihack profile rotate start"
    )
}

/// Line and column only: a serde message can quote input.
fn position(e: &serde_json::Error) -> String {
    format!("invalid JSON at line {} column {}", e.line(), e.column())
}

pub fn alert_key(kind: &str, live: Option<&ProfileName>) -> String {
    format!("{kind}:{}", live.map_or("-", ProfileName::as_str))
}

/// Whether an ALERT with `key` is to be written: only when the stored key
/// differs. Stores `key`.
pub fn should_alert(l: &mut RotateLedger, key: &str) -> bool {
    if l.alert.as_deref() == Some(key) {
        return false;
    }
    l.alert = Some(key.to_string());
    true
}

pub fn clear_alert(l: &mut RotateLedger) {
    l.alert = None;
}
