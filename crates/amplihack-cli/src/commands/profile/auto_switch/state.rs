//! `<root>/.auto-switch.d/`: the global lock and the state files. Every file
//! is versioned, rejects unknown fields, is capped at 64 KiB and is written
//! privately and atomically. A damaged ledger or journal is never overwritten.

use super::fsx;
use super::usage::Reading;
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const DIR: &str = ".auto-switch.d";
pub const CREDS: &str = ".credentials.json";
pub const CLAUDE_JSON: &str = ".claude.json";
const MAX_STATE: u64 = 64 * 1024;
/// The marker that keeps auto-switch's held-key ALERT to one line per
/// held set (`per_token::record_key_held`).
const KEY_HELD: &str = "key-held";
/// `.claude.json` keeps project history and can grow large.
pub const MAX_CLAUDE_JSON: u64 = 32 * 1024 * 1024;
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostEntry {
    pub live: String,
    pub swapped_at: i64,
    pub pre_swap_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub version: u32,
    pub hosts: BTreeMap<String, HostEntry>,
}

impl Ledger {
    /// The directory account `name` is live in, other than its own.
    pub fn leased_to(&self, name: &str) -> Option<&str> {
        self.hosts
            .iter()
            .find(|(dir, h)| h.live == name && dir.as_str() != name)
            .map(|(dir, _)| dir.as_str())
    }

    /// The account whose login is in directory `dir`.
    pub fn account_in<'a>(&'a self, dir: &'a str) -> &'a str {
        self.hosts.get(dir).map_or(dir, |h| h.live.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Swap,
    Return,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub version: u32,
    pub op: Op,
    /// The directory whose live login changes.
    pub dir: String,
    /// The outgoing account.
    pub from: String,
    /// Whether the outgoing login was present when the journal was written.
    pub from_present: bool,
    /// The incoming account.
    pub to: String,
    pub phase: String,
    pub started_at: i64,
    pub pid: u32,
    pub fingerprint: Option<String>,
    /// Inode of the refresh lock amplihack created in `dir`, if any.
    pub refresh_lock_ino: Option<u64>,
}

pub const PHASE_INCONSISTENT: &str = "inconsistent";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageFile {
    pub version: u32,
    pub accounts: BTreeMap<String, Reading>,
}

/// Paths under one profile root. `root` is canonical.
#[derive(Debug, Clone)]
pub struct State {
    pub root: PathBuf,
    pub dir: PathBuf,
}

pub struct Lock {
    _file: fs::File,
}

fn invariant(msg: impl Into<String>) -> ProfileError {
    ProfileError::AutoSwitchInvariant(msg.into())
}

impl State {
    pub fn new(root: &Path) -> Result<Self, ProfileError> {
        let root = root.canonicalize()?;
        let dir = root.join(DIR);
        Ok(Self { root, dir })
    }

    pub fn exists(&self) -> bool {
        fsx::exists(&self.dir)
    }

    pub fn profile_dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn live_creds(&self, dir: &str) -> PathBuf {
        self.root.join(dir).join(CREDS)
    }

    pub fn parked_dir(&self, name: &str) -> PathBuf {
        self.dir.join("parked").join(name)
    }

    /// The home slot of account `name` given the ledger: its own directory,
    /// unless that directory is hosting another account.
    pub fn home_slot(&self, ledger: &Ledger, name: &str) -> PathBuf {
        if ledger.hosts.contains_key(name) {
            self.parked_dir(name).join(CREDS)
        } else {
            self.live_creds(name)
        }
    }

    /// Checks the root and creates the state directory if needed.
    pub fn ensure(&self) -> Result<(), ProfileError> {
        fsx::check_root_private(&self.root).map_err(|e| invariant(format!("profile root {e}")))?;
        self.ensure_sub(&self.dir)
    }

    pub fn ensure_sub(&self, path: &Path) -> Result<(), ProfileError> {
        fsx::ensure_private_dir(path).map_err(|e| invariant(format!("{}: {e}", self.rel(path))))
    }

    /// A path relative to the root, for messages.
    pub fn rel(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    fn open_lock(&self) -> Result<fs::File, ProfileError> {
        self.ensure()?;
        let mut opts = fs::OpenOptions::new();
        opts.read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = opts.open(self.dir.join("lock"))?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.uid() != fsx::euid() {
            return Err(invariant("lock is not a private regular file"));
        }
        Ok(file)
    }

    pub fn lock(&self) -> Result<Lock, ProfileError> {
        let file = self.open_lock()?;
        file.lock_exclusive()?;
        Ok(Lock { _file: file })
    }

    /// Tries for at most `wait`; `None` when another process holds the lock.
    pub fn try_lock(&self, wait: Duration) -> Result<Option<Lock>, ProfileError> {
        let file = self.open_lock()?;
        let start = Instant::now();
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(true) => return Ok(Some(Lock { _file: file })),
                Ok(false) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
            if start.elapsed() >= wait {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn read_state(&self, name: &str) -> Result<Option<Vec<u8>>, ProfileError> {
        let path = self.dir.join(name);
        match fsx::read_bounded_nofollow(&path, MAX_STATE) {
            Ok(Some((raw, meta))) if meta.uid() == fsx::euid() && meta.mode() & 0o077 == 0 => {
                Ok(Some(raw))
            }
            Ok(Some(_)) => Err(invariant(format!("{DIR}/{name}: not a private file"))),
            Ok(None) => Ok(None),
            Err(e) => Err(invariant(format!("{DIR}/{name}: {e}"))),
        }
    }

    fn write_state(&self, name: &str, bytes: &[u8]) -> Result<(), ProfileError> {
        self.ensure()?;
        fsx::write_private_atomic(&self.dir, name, bytes)
            .map_err(|e| invariant(format!("{DIR}/{name}: {e}")))
    }

    pub fn load_ledger(&self) -> Result<Ledger, ProfileError> {
        let Some(raw) = self.read_state("ledger.json")? else {
            return Ok(Ledger {
                version: VERSION,
                hosts: BTreeMap::new(),
            });
        };
        let ledger: Ledger = serde_json::from_slice(&raw)
            .map_err(|e| invariant(format!("{DIR}/ledger.json: {}", parse_position(&e))))?;
        if ledger.version != VERSION {
            return Err(invariant(format!("{DIR}/ledger.json: unknown version")));
        }
        for (dir, h) in &ledger.hosts {
            if ProfileName::parse(dir).is_err() || ProfileName::parse(&h.live).is_err() {
                return Err(invariant(format!(
                    "{DIR}/ledger.json: invalid profile name"
                )));
            }
        }
        Ok(ledger)
    }

    pub fn save_ledger(&self, ledger: &Ledger) -> Result<(), ProfileError> {
        let bytes = serde_json::to_vec_pretty(ledger).map_err(|e| invariant(e.to_string()))?;
        self.write_state("ledger.json", &bytes)
    }

    pub fn load_journal(&self) -> Result<Option<Journal>, ProfileError> {
        let Some(raw) = self.read_state("journal.json")? else {
            return Ok(None);
        };
        let j: Journal = serde_json::from_slice(&raw)
            .map_err(|e| invariant(format!("{DIR}/journal.json: {}", parse_position(&e))))?;
        if j.version != VERSION
            || ProfileName::parse(&j.dir).is_err()
            || ProfileName::parse(&j.from).is_err()
            || ProfileName::parse(&j.to).is_err()
        {
            return Err(invariant(format!(
                "{DIR}/journal.json: unknown version or invalid name"
            )));
        }
        Ok(Some(j))
    }

    pub fn save_journal(&self, j: &Journal) -> Result<(), ProfileError> {
        let bytes = serde_json::to_vec_pretty(j).map_err(|e| invariant(e.to_string()))?;
        self.write_state("journal.json", &bytes)
    }

    pub fn delete_journal(&self) -> Result<(), ProfileError> {
        match fs::remove_file(self.dir.join("journal.json")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        fsx::fsync_dir(&self.dir)?;
        Ok(())
    }

    /// The plan logins named by the last `ALERT: API key not used` that
    /// auto-switch wrote to the rotate log, one per line; `None` when there
    /// is none, or the marker cannot be read, so the ALERT is written again.
    pub fn load_key_held(&self) -> Option<String> {
        let raw = self.read_state(KEY_HELD).ok()??;
        String::from_utf8(raw).ok()
    }

    pub fn save_key_held(&self, held: &str) -> Result<(), ProfileError> {
        self.write_state(KEY_HELD, held.as_bytes())
    }

    /// Forgets the last held set, so the next one is written. Missing is fine.
    pub fn clear_key_held(&self) -> Result<(), ProfileError> {
        match fs::remove_file(self.dir.join(KEY_HELD)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Readings. A missing file is empty; a damaged one (bad JSON, unknown
    /// version, or a key that is not a valid profile name) is an error that
    /// names the file and, for bad JSON, only the line and column.
    pub fn try_load_usage(&self) -> Result<UsageFile, ProfileError> {
        let Some(raw) = self.read_state("usage.json")? else {
            return Ok(UsageFile {
                version: VERSION,
                accounts: BTreeMap::new(),
            });
        };
        let u: UsageFile = serde_json::from_slice(&raw)
            .map_err(|e| invariant(format!("{DIR}/usage.json: {}", parse_position(&e))))?;
        if u.version != VERSION {
            return Err(invariant(format!("{DIR}/usage.json: unknown version")));
        }
        if u.accounts.keys().any(|k| ProfileName::parse(k).is_err()) {
            return Err(invariant(format!("{DIR}/usage.json: invalid profile name")));
        }
        Ok(u)
    }

    /// Readings; a damaged file counts as empty and is overwritten later.
    /// Silent, for the status line, which must not print anything of its own.
    pub fn load_usage(&self) -> UsageFile {
        self.try_load_usage().unwrap_or_else(|_| UsageFile {
            version: VERSION,
            accounts: BTreeMap::new(),
        })
    }

    /// As [`State::load_usage`], but tells the user once when the file is
    /// damaged. For the CLI and the rerun supervisor.
    pub fn load_usage_or_warn(&self) -> UsageFile {
        self.try_load_usage().unwrap_or_else(|e| {
            eprintln!("auto-switch: warning: {e}; usage readings treated as empty");
            UsageFile {
                version: VERSION,
                accounts: BTreeMap::new(),
            }
        })
    }

    pub fn save_usage(&self, usage: &UsageFile) -> Result<(), ProfileError> {
        let mut usage = usage.clone();
        usage.version = VERSION;
        usage
            .accounts
            .retain(|name, _| ProfileName::parse(name).is_ok());
        let bytes = serde_json::to_vec_pretty(&usage).map_err(|e| invariant(e.to_string()))?;
        self.write_state("usage.json", &bytes)
    }

    /// A small JSON file under a state subdirectory (`accounts/`,
    /// `statusline/`). Damaged files are reported, never overwritten here.
    pub fn read_sub_json(
        &self,
        sub: &str,
        name: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = self.dir.join(sub).join(format!("{name}.json"));
        let shown = format!("{DIR}/{sub}/{name}.json");
        match fsx::read_bounded_nofollow(&path, MAX_STATE) {
            Ok(None) => Ok(None),
            Ok(Some((raw, meta))) => {
                if meta.uid() != fsx::euid() || meta.mode() & 0o777 != 0o600 {
                    return Err(format!("{shown}: not a private (0600) file"));
                }
                serde_json::from_slice(&raw)
                    .map(Some)
                    .map_err(|e| format!("{shown}: {}", parse_position(&e)))
            }
            Err(e) => Err(format!("{shown}: {e}")),
        }
    }

    pub fn write_sub_json(
        &self,
        sub: &str,
        name: &str,
        v: &serde_json::Value,
    ) -> Result<(), ProfileError> {
        self.ensure()?;
        let dir = self.dir.join(sub);
        self.ensure_sub(&dir)?;
        let bytes = serde_json::to_vec_pretty(v).map_err(|e| invariant(e.to_string()))?;
        fsx::write_private_atomic(&dir, &format!("{name}.json"), &bytes)
            .map_err(|e| invariant(format!("{DIR}/{sub}/{name}.json: {e}")))
    }
}

/// Line and column only: a serde message can quote input.
pub fn parse_position(e: &serde_json::Error) -> String {
    format!("invalid JSON at line {} column {}", e.line(), e.column())
}
