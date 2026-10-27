//! `<root>/.auto-switch.d/rotate/`: the rotate ledger and journal, the pid
//! file and the log (D7). Ledger and journal are versioned, reject unknown
//! fields, are capped at 64 KiB and are written privately and atomically. A
//! file that fails to parse is reported and never overwritten here.

use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    check_root_private, ensure_private_dir, euid, fsync_dir, read_bounded_nofollow,
    writable_by_others, write_private_atomic,
};
use crate::commands::profile::name::ProfileName;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;
const MAX_STATE: u64 = 64 * 1024;
/// The longest account value accepted (an e-mail address is at most 320).
pub const MAX_ACCOUNT: usize = 320;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LastCycle {
    pub at: i64,
    pub outcome: String,
    /// The credit-backed overflow tag (issue #1606): `in`, `out` or `on`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overflow: Option<String>,
}

/// The tags `LastCycle.overflow` may hold.
pub const OVERFLOW_TAGS: [&str; 3] = ["in", "out", "on"];
/// The only reason an overflow profile is blocked.
pub const SPEND_LIMIT: &str = "spend_limit";
/// The most overflow blocks a ledger may hold.
pub const MAX_BLOCKS: usize = 256;
/// The longest a loaded block may last. The writer uses the next spend
/// reset, at most 31 days away; the slack lets a later change take the
/// reset from Claude's message without invalidating older ledgers.
pub const MAX_BLOCK_SECS: i64 = 62 * 86_400;

/// An overflow profile that reported the organisation's spend limit: not
/// used from `at` until `until` (the 1st of the next month, 00:00 UTC) or
/// until `profile overflow clear`. Unknown fields are ignored, so a later
/// binary can add one without making this ledger unreadable here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverflowBlock {
    pub at: i64,
    pub until: i64,
    pub reason: String,
}

impl OverflowBlock {
    /// A spend-limit block recorded at `at`.
    pub fn spend_limit(at: i64) -> Self {
        Self {
            at,
            until: spend_reset(at),
            reason: SPEND_LIMIT.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Overflow profiles blocked by a spend limit, by profile name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overflow_blocked: BTreeMap<String, OverflowBlock>,
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
            overflow_blocked: BTreeMap::new(),
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

/// The overflow fields of a loaded ledger: a known tag, at most
/// [`MAX_BLOCKS`] blocks, each keyed by a profile name, for a spend limit,
/// recorded at a positive time and ending after it but within
/// [`MAX_BLOCK_SECS`], so no block can be made to last for ever. An end
/// that cannot be computed counts as invalid.
fn valid_overflow(l: &RotateLedger) -> bool {
    let tag_ok = l
        .last_cycle
        .as_ref()
        .and_then(|c| c.overflow.as_deref())
        .is_none_or(|t| OVERFLOW_TAGS.contains(&t));
    tag_ok
        && l.overflow_blocked.len() <= MAX_BLOCKS
        && l.overflow_blocked.iter().all(|(name, b)| {
            ProfileName::parse(name).is_ok()
                && b.reason == SPEND_LIMIT
                && b.at > 0
                && b.at < b.until
                && b.at
                    .checked_add(MAX_BLOCK_SECS)
                    .is_some_and(|max| b.until <= max)
        })
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
        if !valid_overflow(&l) {
            return Err(inconsistent(format!("{WHAT}: invalid overflow field")));
        }
        Ok(Some(l))
    }

    pub fn save_ledger(&self, l: &RotateLedger) -> Result<(), ProfileError> {
        let bytes = serde_json::to_vec_pretty(l).map_err(|e| inconsistent(e.to_string()))?;
        self.write("ledger.json", &bytes)
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

/// When an organisation's monthly spend resets, as assumed for a block
/// recorded at `at`: the 1st of the next calendar month, 00:00 UTC. Claude's
/// message gives no reset time, so this is the documented assumption.
pub fn spend_reset(at: i64) -> i64 {
    use chrono::{Datelike, TimeZone, Utc};
    let Some(t) = chrono::DateTime::from_timestamp(at, 0) else {
        return at;
    };
    let (y, m) = if t.month() == 12 {
        (t.year() + 1, 1)
    } else {
        (t.year(), t.month() + 1)
    };
    Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0)
        .single()
        .map_or(at, |d| d.timestamp())
}

/// The blocks still in force at `now` (`now < until`).
pub fn active_blocks(l: &RotateLedger, now: i64) -> BTreeMap<String, OverflowBlock> {
    l.overflow_blocked
        .iter()
        .filter(|(_, b)| now < b.until)
        .map(|(n, b)| (n.clone(), b.clone()))
        .collect()
}

/// Drops the blocks that ended at or before `now`.
pub fn prune_blocks(l: &mut RotateLedger, now: i64) {
    l.overflow_blocked.retain(|_, b| now < b.until);
}
