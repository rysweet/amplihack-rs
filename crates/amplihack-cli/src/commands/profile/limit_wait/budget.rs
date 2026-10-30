//! The recipe run's wait budget (issue #1761): every usage-limit wait of
//! one `amplihack recipe run` attempt counts against
//! `recipe.usage_limit_wait_budget_secs`. The record is
//! `$HOME/.amplihack/rerun/wait-budget-<run id>`, the run id coming from
//! `AMPLIHACK_RECIPE_RUN_ID`: one ASCII decimal line per finished wait,
//! only ever appended under an exclusive lock, the total being their
//! saturating sum.
//!
//! The run id is checked on its bytes before any path is built. Nothing is
//! opened before the first budget check, nor ever without a valid run id
//! and an absolute HOME. The record is opened without following a symlink
//! and must be a private regular file with one link; a record that fails a
//! check is refused and left exactly as it was. Each lock covers one read
//! or one append, never a wait.
//!
//! Without a usable record the step keeps its own budget for the rest of
//! the step (the per-step fallback): the amount left at its last good read
//! (the whole budget if it had none) less its own waits since, with one of
//! three fixed warnings. Each time a step opens its run's record, other
//! runs' records more than 7 days old are deleted. Tested in
//! `tests_budget.rs`, `tests_budget_refused.rs` and `tests_budget_sweep.rs`.

use super::super::fsx;
use super::scratch::home_rerun_dir;
use amplihack_utils::secure_files::{lock_exclusive_timeout, open_private_nofollow};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const RUN_ID_ENV: &str = "AMPLIHACK_RECIPE_RUN_ID";

const NOT_SET: &str = "warning: the usage-limit wait budget applies to this step only (AMPLIHACK_RECIPE_RUN_ID is not set)";
const NOT_VALID: &str = "warning: the usage-limit wait budget applies to this step only (AMPLIHACK_RECIPE_RUN_ID is not a valid run id)";
const UNUSABLE: &str = "warning: the usage-limit wait budget applies to this step only (the budget record cannot be used)";

const RECORD_PREFIX: &str = "wait-budget-";
/// A longer record is refused.
const MAX_RECORD_BYTES: usize = 64 * 1024;
/// The most digits a `u64` line may have.
const MAX_LINE_DIGITS: usize = 20;
const MAX_RUN_ID_BYTES: usize = 64;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
/// Other runs' records older than this are deleted.
const SWEEP_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// The most directory entries one clean-up looks at.
const SWEEP_MAX_ENTRIES: usize = 1024;

/// The run's record: an absolute HOME and the record's file name.
struct Record {
    home: PathBuf,
    name: String,
}

/// One step's view of the run budget.
pub(crate) struct Budget {
    /// `None` in the per-step fallback.
    record: Option<Record>,
    /// The fixed warning written when the step falls back; taken once.
    warning: Option<&'static str>,
    /// Seconds left at the last good read of the record.
    last_left: Option<u64>,
    /// Seconds this step waited since that read.
    since: u64,
}

impl Budget {
    /// The budget for run id `run_id` with the record under `home`. Only
    /// checks the values: nothing is opened until [`Budget::left`].
    pub(crate) fn new(run_id: Option<&OsStr>, home: Option<&Path>) -> Self {
        let record = match run_id.filter(|id| !id.is_empty()) {
            None => Err(NOT_SET),
            Some(id) => match checked(id) {
                None => Err(NOT_VALID),
                Some(id) => match home.filter(|h| h.is_absolute()) {
                    None => Err(UNUSABLE),
                    Some(home) => Ok(Record {
                        home: home.to_path_buf(),
                        name: format!("{RECORD_PREFIX}{id}"),
                    }),
                },
            },
        };
        let (record, warning) = match record {
            Ok(r) => (Some(r), UNUSABLE),
            Err(w) => (None, w),
        };
        Self {
            record,
            warning: Some(warning),
            last_left: None,
            since: 0,
        }
    }

    /// [`Budget::new`] from `AMPLIHACK_RECIPE_RUN_ID` and `HOME`.
    pub(crate) fn from_env() -> Self {
        let id = std::env::var_os(RUN_ID_ENV);
        let home = std::env::var_os("HOME");
        Self::new(id.as_deref(), home.as_deref().map(Path::new))
    }

    /// The step keeps its own budget.
    pub(crate) fn per_step(&self) -> bool {
        self.record.is_none()
    }

    /// Seconds left of `total`, read from the run's record. Falls back to
    /// the step's own count when the record cannot be used, writing the
    /// step's one warning through `warn` the first time.
    pub(crate) fn left(&mut self, total: u64, warn: &mut dyn FnMut(&str)) -> u64 {
        if let Some(record) = &self.record {
            match read_total(record) {
                Ok(spent) => {
                    let left = total.saturating_sub(spent);
                    self.last_left = Some(left);
                    self.since = 0;
                    return left;
                }
                // The error's text can name a path, so only the fixed
                // warning below reports it.
                Err(_) => self.record = None,
            }
        }
        self.warn_once(warn);
        self.last_left.unwrap_or(total).saturating_sub(self.since)
    }

    /// Counts a finished wait of `secs` and appends it to the run's record.
    /// A failed append falls back, writing the step's one warning.
    pub(crate) fn add(&mut self, secs: u64, warn: &mut dyn FnMut(&str)) {
        self.since = self.since.saturating_add(secs);
        // As in `left`, a failure is reported only by the fixed warning.
        if let Some(record) = &self.record
            && append(record, secs).is_err()
        {
            self.record = None;
            self.warn_once(warn);
        }
    }

    fn warn_once(&mut self, warn: &mut dyn FnMut(&str)) {
        if let Some(w) = self.warning.take() {
            warn(w);
        }
    }
}

/// `id` as text when it is a valid run id.
fn checked(id: &OsStr) -> Option<&str> {
    if valid_run_id(id) { id.to_str() } else { None }
}

/// 1 to 64 bytes, each an ASCII letter, digit or `-`. Checked on the raw
/// bytes, so no lossy copy can turn a bad value into a good path.
pub(crate) fn valid_run_id(id: &OsStr) -> bool {
    let b = id.as_bytes();
    (1..=MAX_RUN_ID_BYTES).contains(&b.len())
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
}

/// The saturating sum of the record's lines. `None` for a record over
/// 64 KiB or any non-empty line that is not 1 to 20 ASCII digits naming a
/// `u64`.
pub(crate) fn parse_total(raw: &[u8]) -> Option<u64> {
    if raw.len() > MAX_RECORD_BYTES {
        return None;
    }
    let mut total = 0u64;
    for line in raw.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        if line.len() > MAX_LINE_DIGITS || !line.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let secs: u64 = std::str::from_utf8(line).ok()?.parse().ok()?;
        total = total.saturating_add(secs);
    }
    Some(total)
}

/// The record's total, under the lock.
fn read_total(record: &Record) -> io::Result<u64> {
    locked(record, |file| {
        let raw = fsx::read_bounded(file, MAX_RECORD_BYTES as u64)?;
        parse_total(&raw).ok_or_else(|| fsx::invalid("the budget record is malformed"))
    })
}

/// Appends `secs` to the record, under the lock.
fn append(record: &Record, secs: u64) -> io::Result<()> {
    locked(record, |mut file| {
        file.write_all(format!("{secs}\n").as_bytes())
    })
}

/// Runs `f` on the record under its exclusive lock, then, with the lock
/// released, cleans up other runs' records.
fn locked<T>(record: &Record, f: impl FnOnce(&File) -> io::Result<T>) -> io::Result<T> {
    let dir = home_rerun_dir(&record.home)?;
    let out = {
        let lock = lock_exclusive_timeout(
            open_private_nofollow(&dir.join(&record.name))?,
            LOCK_TIMEOUT,
        )?;
        f(lock.file())?
    };
    sweep(&dir, &record.name);
    Ok(out)
}

/// Deletes other runs' records in `dir` more than 7 days old: regular files
/// of this user (per `lstat`) named `wait-budget-<valid run id>`, other
/// than `own`. A future modification time keeps a file. At most 1024
/// entries are looked at and subdirectories are not entered. Clean-up is
/// only housekeeping: a file that cannot be read or deleted is left for a
/// later step, so each error skips that entry and is not reported.
fn sweep(dir: &Path, own: &str) {
    let Some(cutoff) = SystemTime::now().checked_sub(SWEEP_AGE) else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.take(SWEEP_MAX_ENTRIES).flatten() {
        let name = entry.file_name();
        if name.as_bytes() == own.as_bytes() {
            continue;
        }
        let Some(id) = name.as_bytes().strip_prefix(RECORD_PREFIX.as_bytes()) else {
            continue;
        };
        if !valid_run_id(OsStr::from_bytes(id)) {
            continue;
        }
        let path = entry.path();
        let old = fs::symlink_metadata(&path).is_ok_and(|m| {
            m.file_type().is_file()
                && m.uid() == fsx::euid()
                && m.modified().is_ok_and(|t| t < cutoff)
        });
        if old {
            // unlink(2) never follows a symlink swapped in since the check.
            let _ = fs::remove_file(&path);
        }
    }
}
