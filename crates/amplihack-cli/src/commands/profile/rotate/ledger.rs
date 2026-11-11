//! `<root>/.auto-switch.d/rotate/`: the rotate ledger and journal, the pid
//! file and the log (D7), versioned, capped at 64 KiB and written privately
//! and atomically. The journal rejects unknown fields; the ledger ignores
//! them (#1659). A file that fails to parse is reported, never overwritten.

use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    check_root_private, ensure_private_dir, euid, fsync_dir, now, parse_position as position,
    read_bounded_nofollow, writable_by_others, write_private_atomic,
};
use crate::commands::profile::name::ProfileName;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::alert::valid_alerts;
pub use super::hand_fields::{HandSuspect, MAX_GUIDANCE_LINE, MAX_GUIDANCE_LINES, unprintable};
use super::hand_fields::{valid_guidance, valid_hand_suspect, valid_lost_login, valid_name};
pub use super::older_readers::for_older_readers;
use super::plan_room::no_room::{self, NoRoom};
#[cfg(test)]
pub use super::spend_blocks::KEY_REASONS;
use super::spend_blocks::valid_overflow;
pub use super::spend_blocks::{
    KEY_EXHAUSTED, KEY_RATE_LIMITED, KEY_REVOKED, LIVE_KIND_PLATFORM, active_blocks, prune_blocks,
    spend_reset,
};
const VERSION: u32 = 1;
const MAX_STATE: u64 = 64 * 1024;
/// The longest account value accepted (an e-mail address is at most 320).
pub const MAX_ACCOUNT: usize = 320;

/// The `last_cycle.outcome` written by `profile switch NAME` (issue #1691):
/// the person vouches for NAME's login, so the limit-resume hook counts it as
/// capacity measured at the switch. A held cycle never replaces it.
pub const SWITCHED: &str = "switched";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastCycle {
    /// When the cycle was recorded, at its end.
    pub at: i64,
    /// When the usage the cycle relied on was read (issue #1659): a waiting
    /// limit-resume hook trusts only a measurement taken after its limit.
    /// A `stayed` cycle records when the live reading was taken: a passive
    /// reading's `observed_at`, but never later than when the cycle started
    /// measuring (see `capacity::recorded_at`). Every other rotate outcome
    /// records when the cycle started measuring. [`SWITCHED`] records the
    /// time of the switch. Absent for a cycle that measured nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<i64>,
    pub outcome: String,
    /// The credit-backed overflow tag (issue #1606): `in`, `out` or `on`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overflow: Option<String>,
}

/// Whether `last` was written after a cycle began measuring at
/// `measured_at`, so another writer used the ledger while that cycle held no
/// lock. A switch record always counts, whatever the clock says.
pub fn newer_than(last: Option<&LastCycle>, measured_at: i64) -> bool {
    last.is_some_and(|c| c.outcome == SWITCHED || c.at >= measured_at)
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
    /// An API-key block for `reason`, from `at` until `until`.
    pub fn key(reason: &str, at: i64, until: i64) -> Self {
        Self {
            at,
            until,
            reason: reason.to_string(),
        }
    }

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
pub struct RotateLedger {
    pub version: u32,
    /// The profile whose login is in `~/.claude`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<String>,
    /// That login's account (`accountUuid`, else `emailAddress`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Full credential-bound identity; a legacy key alone cannot prove ownership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_identity: Option<super::login_identity::Identity>,
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
    /// Issue #1907: a no-capacity ALERT's key while the live login still
    /// read below 100%, so it does not block a resume; never with `alert`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisory_alert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<LastCycle>,
    /// Overflow profiles blocked by a spend limit, by profile name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overflow_blocked: BTreeMap<String, OverflowBlock>,
    /// The current hand-change note, or ALERT and `fix` lines (#1693, #1886).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<Vec<String>>,
    /// The open hand-change suspicion (issue #1886); no reading, no account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hand_suspect: Option<HandSuspect>,
    /// When `~/.claude.json` was first found with no readable account under a
    /// ledger-named live profile; after [`HOME_UNREADABLE_GRACE`] the
    /// `home-unreadable` ALERT gets `fix` lines (#1702 review).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_unreadable_since: Option<i64>,
    /// Profiles recorded as live that lost their login to a manual `/login`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lost_login: Vec<String>,
    /// `"platform"` while `live` is an API-key profile (issue #1700);
    /// absent for a subscription login. Readers that cannot detect kinds,
    /// such as the limit-resume hook, use it; the rotator repairs it on
    /// every cycle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_kind: Option<String>,
    /// The API-key block (issue #1700): it covers every API-key profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_block: Option<OverflowBlock>,
    /// The logins the last measuring cycle found with no room (issue
    /// #1804), for auto-switch, which cannot read the login in `~/.claude`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_room: Option<NoRoom>,
    /// Logins left while full, by name, at the swap's epoch seconds (#1870).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub swapped_away: BTreeMap<String, i64>,
}

impl Default for RotateLedger {
    fn default() -> Self {
        Self {
            version: VERSION,
            live: None,
            account: None,
            login_identity: None,
            since: None,
            swapped_at: None,
            previous: None,
            alert: None,
            advisory_alert: None,
            last_cycle: None,
            overflow_blocked: BTreeMap::new(),
            guidance: None,
            hand_suspect: None,
            home_unreadable_since: None,
            lost_login: Vec::new(),
            live_kind: None,
            platform_block: None,
            no_room: None,
            swapped_away: BTreeMap::new(),
        }
    }
}

#[path = "swap_journal.rs"]
mod journal;
pub use journal::{Phase, RotateJournal};

/// Paths of the rotate state.
#[derive(Debug, Clone)]
pub struct RotateState {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub ledger: PathBuf,
    pub journal: PathBuf,
    /// The rotation pin written by `profile switch NAME` (issue #1691); read
    /// and written only by `pin.rs`.
    pub pin: PathBuf,
    pub pid: PathBuf,
    pub log: PathBuf,
    pub log1: PathBuf,
    /// The background process's own stdout and stderr.
    pub err: PathBuf,
}

/// A state file amplihack may act on: this user's, and not writable by the
/// group or others.
pub(super) fn trusted(m: &std::fs::Metadata) -> bool {
    m.uid() == euid() && !writable_by_others(m)
}

fn inconsistent(msg: impl Into<String>) -> ProfileError {
    ProfileError::RotateInconsistent(msg.into())
}

/// Issue #1947: the reasons [`RotateState::load_ledger`] gives for a ledger
/// it parsed but whose fields it refuses. Each names a field in fixed text,
/// never a stored value, so the unreadable-ledger stop and `status` show it.
const FIELD_REFUSALS: [&str; 7] = [
    "unknown version",
    "invalid profile name",
    "invalid account or alert",
    "invalid overflow field",
    "invalid no_room",
    "invalid hand-change field",
    "invalid login_identity",
];

/// The load error for a refused field; `reason` is one of [`FIELD_REFUSALS`].
fn refuse(reason: &'static str) -> ProfileError {
    debug_assert!(FIELD_REFUSALS.contains(&reason), "{reason}");
    inconsistent(format!("rotate ledger: {reason}"))
}

/// The fixed reason a failed ledger load gave when it refused a field. `None`
/// for every other failure, such as a read or parse error, whose text may
/// hold a path or a position and is not shown.
pub fn refused_field(e: &ProfileError) -> Option<&'static str> {
    let ProfileError::RotateInconsistent(m) = e else {
        return None;
    };
    let reason = m.strip_prefix("rotate ledger: ")?;
    FIELD_REFUSALS.iter().copied().find(|r| *r == reason)
}

/// An account value amplihack may store: non-empty, at most 320 bytes, no
/// control or bidirectional-override characters ([`unprintable`]).
pub fn safe_account(v: &str) -> bool {
    !v.is_empty() && v.len() <= MAX_ACCOUNT && !v.chars().any(unprintable)
}

impl RotateState {
    /// `root` is canonicalised when it exists.
    pub fn new(root: &Path) -> Self {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let dir = root.join(".auto-switch.d").join("rotate");
        Self {
            ledger: dir.join("ledger.json"),
            journal: dir.join("journal.json"),
            pin: dir.join("pin"),
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
    pub(super) fn read(&self, path: &Path, what: &str) -> Result<Option<Vec<u8>>, ProfileError> {
        match read_bounded_nofollow(path, MAX_STATE) {
            Ok(None) => Ok(None),
            Ok(Some((raw, m))) if trusted(&m) => Ok(Some(raw)),
            Ok(Some(_)) => Err(inconsistent(format!("{what}: not a private file"))),
            Err(e) => Err(inconsistent(format!("{what}: {e}"))),
        }
    }

    pub(super) fn write(&self, name: &str, bytes: &[u8]) -> Result<(), ProfileError> {
        self.ensure()?;
        write_private_atomic(&self.dir, name, bytes).map_err(ProfileError::Io)
    }

    pub fn load_ledger(&self) -> Result<Option<RotateLedger>, ProfileError> {
        const WHAT: &str = "rotate ledger";
        let Some(raw) = self.read(&self.ledger, WHAT)? else {
            return Ok(None);
        };
        let mut l: RotateLedger = serde_json::from_slice(&raw)
            .map_err(|e| inconsistent(format!("{WHAT}: {}", position(&e))))?;
        if l.version != VERSION {
            return Err(refuse("unknown version"));
        }
        if !valid_name(l.live.as_deref()) || !valid_name(l.previous.as_deref()) {
            return Err(refuse("invalid profile name"));
        }
        if l.account.as_deref().is_some_and(|a| !safe_account(a)) || !valid_alerts(&l) {
            return Err(refuse("invalid account or alert"));
        }
        if !valid_overflow(&l) {
            return Err(refuse("invalid overflow field"));
        }
        if !no_room::valid(l.no_room.as_ref()) {
            return Err(refuse("invalid no_room"));
        }
        if !valid_guidance(l.guidance.as_deref())
            || !valid_lost_login(&l.lost_login)
            || !valid_hand_suspect(l.hand_suspect.as_ref())
        {
            return Err(refuse("invalid hand-change field"));
        }
        if l.login_identity.as_ref().is_some_and(|i| !i.valid()) {
            return Err(refuse("invalid login_identity"));
        }
        l.swapped_away = super::swap_back::valid(std::mem::take(&mut l.swapped_away), now());
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
        self.foreign_rotator_at(
            &super::daemon::proc_root(),
            std::env::current_exe().ok().as_deref(),
        )
    }

    /// [`Self::foreign_rotator`] against another `/proc` and executable.
    pub fn foreign_rotator_at(&self, proc_root: &Path, exe: Option<&Path>) -> Option<u32> {
        amplihack_utils::rotator_status::foreign_rotator(&self.pid, proc_root, exe)
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
        if !super::observe::affinity::valid_transaction(j.warning_transaction.as_deref()) {
            return Err(inconsistent(format!("{WHAT}: invalid warning transaction")));
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
