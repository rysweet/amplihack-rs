//! The wait-state record (issue #2967): the small file a supervisor keeps
//! for the length of a usage-limit wait, which the stderr pump of
//! `amplihack recipe run` reads to rewrite the runner's heartbeat
//! (`recipe::run::heartbeat`).
//!
//! `amplihack recipe run` makes a private directory and names it in
//! [`DIR_ENV`]. A supervisor that starts a wait writes `<dir>/<its pid>` and
//! removes it when the wait ends, on return and on unwind alike ([`Guard`]).
//! The record is `{"v":1,"pid":N,"until":T,"expires":E,"steps":[..]}`:
//! integers only, in this and any later version. It holds no path, login
//! name, account identifier or credential.
//!
//! `steps` is the chain of supervisors this one runs beneath, carried down
//! in [`STEPS_ENV`], so the heartbeat of an enclosing step can state the
//! earliest wait beneath it. The chain holds supervisor process ids, not
//! step ids.
//!
//! The directory is a place other processes of the same user can write, so
//! the reader trusts little. A directory is used only when it is an absolute
//! path to a real directory owned by the effective user with mode exactly
//! `0700`. Each record is opened without following links and without
//! blocking, its type and owner are checked on the open descriptor, at most
//! [`MAX_RECORD`] bytes are read, only file names that are a process id in
//! plain decimal are considered, and a scan looks at no more than
//! [`MAX_ENTRIES`] entries and never enters a subdirectory. `expires` is the
//! controlling limit on how long a record is believed; whether its process
//! is alive only narrows that. Every failure reads as "not waiting", which
//! leaves the heartbeat as the runner wrote it.
//!
//! This module is built on Unix only, as the rest of the wait is. On other
//! hosts nothing is written or read and the heartbeat is unchanged.
//!
//! Tested in `tests_wait_state.rs`.

use super::super::fsx;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The directory the records are kept in, set by `amplihack recipe run`.
pub(crate) const DIR_ENV: &str = "AMPLIHACK_USAGE_LIMIT_WAIT_DIR";
/// The supervisors a step runs beneath: their process ids in decimal,
/// separated by commas, outermost first.
pub(crate) const STEPS_ENV: &str = "AMPLIHACK_USAGE_LIMIT_WAIT_STEPS";

/// The directory's name inside a run's own folder.
const DIR_NAME: &str = "usage-limit-wait";
/// The record's version.
const VERSION: u64 = 1;
/// A record is never larger than this, written or read.
const MAX_RECORD: usize = 1024;
/// Directory entries a scan looks at.
const MAX_ENTRIES: usize = 64;
/// Supervisors a chain holds; a deeper chain keeps the innermost ones.
const MAX_CHAIN: usize = 32;
/// How long past its deadline a record may still be believed. The
/// supervisor removes its record when the wait ends, so this only bounds
/// what a supervisor that died without unwinding leaves behind.
const EXPIRY_SLACK: i64 = 120;

#[derive(Serialize, Deserialize)]
struct Record {
    v: u64,
    pid: u32,
    /// The wait's deadline, as the wait's `start` line states it.
    until: i64,
    /// When a reader stops believing the record, in Unix seconds.
    expires: i64,
    steps: Vec<u32>,
}

/// Whether `pid` can be a process id: not zero, and within `pid_t`.
fn valid_pid(pid: u32) -> bool {
    pid != 0 && pid <= i32::MAX as u32
}

/// The process id `name` writes in plain decimal: digits only, no sign,
/// space or leading zero.
fn pid_of(name: &str) -> Option<u32> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let pid: u32 = name.parse().ok()?;
    (valid_pid(pid) && pid.to_string() == name).then_some(pid)
}

/// Whether `dir` may hold records: an absolute path to a directory, not a
/// symlink to one, owned by the effective user, with mode exactly `0700`.
/// Any other value is treated as unset by every caller.
pub(crate) fn valid_dir(dir: &Path) -> bool {
    if !dir.is_absolute() {
        return false;
    }
    fs::symlink_metadata(dir).is_ok_and(|meta| {
        meta.file_type().is_dir() && meta.uid() == fsx::euid() && meta.mode() & 0o7777 == 0o700
    })
}

/// The record directory of one `amplihack recipe run`, removed when the run
/// ends if this run created it.
pub(crate) struct RunDir {
    path: PathBuf,
    created: bool,
}

impl RunDir {
    /// The directory for a run whose own folder is `run_dir`.
    ///
    /// A [`valid_dir`] inherited in [`DIR_ENV`] is an enclosing run's: it is
    /// reused, so a nested run's waits are seen by the outer run's pump, and
    /// it is neither removed nor re-moded here. Otherwise
    /// `<run_dir>/usage-limit-wait` is created with mode `0700` and checked.
    /// `None` when there is no run folder or the directory cannot be made
    /// private; the run goes on and its heartbeat stays as the runner
    /// writes it.
    pub(crate) fn for_run(run_dir: Option<&Path>) -> Option<Self> {
        let inherited = std::env::var_os(DIR_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| valid_dir(p));
        if let Some(path) = inherited {
            return Some(Self {
                path,
                created: false,
            });
        }
        let path = std::path::absolute(run_dir?.join(DIR_NAME)).ok()?;
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            // The mode asked for is narrowed by the umask and never widened.
            // The directory is this run's own, so it is set outright.
            Ok(()) => {
                if let Err(e) = fs::set_permissions(&path, fs::Permissions::from_mode(0o700)) {
                    tracing::warn!("usage-limit wait directory not made private: {}", e.kind());
                }
            }
            // One left in this run's own folder by an earlier attempt is
            // used as it stands; `valid_dir` below decides, and its mode is
            // not changed.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                tracing::warn!("usage-limit wait directory not created: {}", e.kind());
                return None;
            }
        }
        if !valid_dir(&path) {
            tracing::warn!("usage-limit wait directory is not private, so it is not used");
            return None;
        }
        Some(Self {
            path,
            created: true,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RunDir {
    /// Removes the directory this run created and the files directly in it.
    /// Nothing is entered: a subdirectory, which no supervisor makes, stops
    /// the removal and is left for the run folder's own cleanup.
    fn drop(&mut self) {
        if !self.created {
            return;
        }
        if let Ok(entries) = fs::read_dir(&self.path) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
        if let Err(e) = fs::remove_dir(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!("usage-limit wait directory not removed: {}", e.kind());
        }
    }
}

/// The supervisors in a [`STEPS_ENV`] value, read strictly: process ids in
/// plain decimal separated by single commas. `None` for anything else.
fn parse_chain(value: &str) -> Option<Vec<u32>> {
    value.split(',').map(pid_of).collect()
}

/// The [`STEPS_ENV`] value for a child of supervisor `pid`: `inherited`
/// with `pid` appended. A malformed inherited value is replaced, not
/// repaired, and a chain longer than [`MAX_CHAIN`] keeps its innermost end.
pub(crate) fn chain_with(inherited: Option<&str>, pid: u32) -> String {
    let mut chain = inherited.and_then(parse_chain).unwrap_or_default();
    chain.push(pid);
    let keep = chain.len().saturating_sub(MAX_CHAIN);
    chain[keep..]
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Removes the record when dropped: on return, on error and on unwind.
pub(crate) struct Guard {
    path: PathBuf,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            // The record expires on its own; a reader stops believing it.
            tracing::debug!("usage-limit wait record not removed: {}", e.kind());
        }
    }
}

/// Writes supervisor `pid`'s record in `dir` and returns the guard that
/// removes it. `None`, with nothing written, when `dir` is not a
/// [`valid_dir`] or the record cannot be written; the wait itself is never
/// held up by this, and its heartbeat then stays as the runner wrote it.
///
/// The record is written to a temporary file made with `create_new` and
/// mode `0600`, then renamed into place, so a reader never sees part of one.
pub(crate) fn begin(
    dir: &Path,
    pid: u32,
    until: i64,
    expires: i64,
    steps: &[u32],
) -> Option<Guard> {
    if !valid_dir(dir) || !valid_pid(pid) {
        return None;
    }
    let body = body(pid, until, expires, steps)?;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{pid}.{}.tmp",
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let path = dir.join(pid.to_string());
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&tmp)
        .and_then(|mut f| f.write_all(&body))
        .and_then(|()| fs::rename(&tmp, &path));
    match written {
        Ok(()) => Some(Guard { path }),
        Err(e) => {
            tracing::debug!("usage-limit wait record not written: {}", e.kind());
            let _ = fs::remove_file(&tmp);
            None
        }
    }
}

/// The record's bytes, within [`MAX_RECORD`]: a chain too long to fit loses
/// its outermost supervisors first.
fn body(pid: u32, until: i64, expires: i64, steps: &[u32]) -> Option<Vec<u8>> {
    let mut steps: Vec<u32> = steps.iter().copied().filter(|p| valid_pid(*p)).collect();
    loop {
        let record = Record {
            v: VERSION,
            pid,
            until,
            expires,
            steps: steps.clone(),
        };
        let bytes = serde_json::to_vec(&record).ok()?;
        if bytes.len() <= MAX_RECORD {
            return Some(bytes);
        }
        if steps.is_empty() {
            return None;
        }
        steps.remove(0);
    }
}

/// The record for the wait this process is about to make, in the directory
/// [`DIR_ENV`] names: `until` is the wait's deadline and `wait_now` the
/// wait's clock when it was planned. `None` when no directory is named or
/// it is not valid, which is every step outside `amplihack recipe run`.
///
/// `expires` is on the real clock, because the reader has no other: the
/// distance to the deadline from now, plus [`EXPIRY_SLACK`].
pub(crate) fn begin_wait(until: i64, wait_now: i64) -> Option<Guard> {
    let dir = PathBuf::from(std::env::var_os(DIR_ENV).filter(|v| !v.is_empty())?);
    let steps = std::env::var(STEPS_ENV)
        .ok()
        .and_then(|v| parse_chain(&v))
        .unwrap_or_default();
    let expires = fsx::now()
        .saturating_add(until.saturating_sub(wait_now).max(0))
        .saturating_add(EXPIRY_SLACK);
    begin(&dir, std::process::id(), until, expires, &steps)
}

/// Until when supervisor `pid` is waiting at `now`: the earliest `until`
/// among the live records that are `pid`'s own or name `pid` in their
/// chain, so an enclosing step's heartbeat states the earliest wait beneath
/// it. `None` when there is none, and for a directory that is not a
/// [`valid_dir`].
pub(crate) fn waiting(dir: &Path, pid: u32, now: i64) -> Option<i64> {
    if !valid_pid(pid) || !valid_dir(dir) {
        return None;
    }
    let own = read(dir, pid, now).map(|r| r.until);
    let beneath = beneath(dir, pid, now);
    [own, beneath].into_iter().flatten().min()
}

/// The earliest `until` among the live records of other supervisors that
/// name `outer` in their chain. Looks at no more than [`MAX_ENTRIES`]
/// entries and enters no subdirectory.
fn beneath(dir: &Path, outer: u32, now: i64) -> Option<i64> {
    fs::read_dir(dir)
        .ok()?
        .take(MAX_ENTRIES)
        .filter_map(|entry| pid_of(entry.ok()?.file_name().to_str()?))
        .filter(|pid| *pid != outer)
        .filter_map(|pid| read(dir, pid, now))
        .filter(|record| record.steps.contains(&outer))
        .map(|record| record.until)
        .min()
}

/// Supervisor `pid`'s record, when it is live at `now`: version 1, naming
/// the pid its file name does, not past `expires`, with a deadline still
/// ahead and a process that is alive.
fn read(dir: &Path, pid: u32, now: i64) -> Option<Record> {
    // No link is followed, and a FIFO put in a record's place cannot block
    // the open or the read.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(dir.join(pid.to_string()))
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.file_type().is_file() || meta.uid() != fsx::euid() {
        return None;
    }
    let mut bytes = Vec::with_capacity(MAX_RECORD + 1);
    file.take(MAX_RECORD as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_RECORD {
        return None;
    }
    let record: Record = serde_json::from_slice(&bytes).ok()?;
    let live = record.v == VERSION
        && record.pid == pid
        && record.steps.iter().all(|p| valid_pid(*p))
        && now < record.expires
        && record.until > now
        && alive(pid);
    live.then_some(record)
}

/// Whether process `pid` exists and may be signalled by this user. A
/// process of another user reads as gone, which is the safe direction: a
/// record must be this user's own, and the cost is an unrewritten heartbeat.
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: kill(2) with signal 0 sends nothing; it only checks `pid`,
    // which the callers have kept positive.
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}
