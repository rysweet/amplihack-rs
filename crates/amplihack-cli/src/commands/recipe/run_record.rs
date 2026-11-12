//! Issue #2175: the process record that lets `recipe stop <run_id>` find a
//! run without reading any other process's `environ` or scanning `/proc`.
//!
//! Each attempt of `recipe run` writes
//! `<work root>/runs/<run folder>/procs/<run_id>.json`, where `<run_id>` is
//! the attempt's correlation id (the `run_id` of its
//! `amplihack.recipe.log_pointer` lines). It names two processes, each with
//! its start time in clock ticks (field 22 of `/proc/<pid>/stat`):
//! `recipe run` itself, and the runner it spawned, which leads its own
//! process group (it calls `setsid`). The record is written right after the
//! runner is spawned and before the `early` log pointer is printed, so a run
//! id seen in that line always has its record, and the [`Guard`] removes it
//! when the attempt ends.
//!
//! [`find`] lists `<work root>/runs/` and opens `procs/<run_id>.json` in each
//! real run folder. A record is accepted only if the opened file belongs to
//! this user and nobody else can write to it: start times are public, so
//! that, not the start-time match, is what makes a record trustworthy. It
//! never reads `/proc`; `recipe stop` checks the start times itself before
//! every signal.
//!
//! Records hold pids, the group, start times, the run id and the version:
//! no argv, working directory or environment. Folder 0700, file 0600.
//! Linux only, like `recipe stop <run_id>`.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::commands::cleanup::run_dir::{
    pid_start_ticks, read_capped_nofollow_with_metadata, write_atomic_0600,
};
use crate::commands::cleanup::work_root::create_private_dir_all;

/// The record format this binary writes and accepts.
pub(crate) const VERSION: u32 = 1;
/// The largest record [`find`] reads, in bytes.
pub(crate) const MAX_RECORD: u64 = 4096;
/// The longest run id accepted, in bytes.
pub(crate) const MAX_RUN_ID: usize = 128;

/// The folder of a run folder that holds its attempts' records.
const PROCS: &str = "procs";

/// One attempt's processes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub version: u32,
    pub run_id: String,
    pub recipe_run: Proc,
    pub runner: Runner,
}

/// The `recipe run` process of the attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Proc {
    pub pid: i32,
    pub start_ticks: u64,
}

/// The runner, which leads its own process group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Runner {
    pub pid: i32,
    pub pgid: i32,
    pub start_ticks: u64,
}

/// Removes the record when dropped.
#[derive(Debug)]
pub(crate) struct Guard {
    path: PathBuf,
}

impl Drop for Guard {
    fn drop(&mut self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "amplihack: warning: cannot remove recipe run record {}: {error}",
                self.path.display()
            ),
        }
    }
}

/// A run id that can name a record: 1 to [`MAX_RUN_ID`] bytes, each an ASCII
/// letter or digit, `.`, `_` or `-`, not starting with `.`. Nothing that
/// could name another path.
pub(crate) fn validate_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= MAX_RUN_ID
        && !run_id.starts_with('.')
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn file_name(run_id: &str) -> String {
    format!("{run_id}.json")
}

/// A real folder, not a symlink to one.
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_dir())
}

/// Writes the record of attempt `run_id` in `run_dir`: this process and the
/// runner `runner_pid`, which leads its own group. Start times are read from
/// `/proc` here, never supplied by the caller. Dropping the returned guard
/// removes the record.
pub(crate) fn write(run_dir: &Path, run_id: &str, runner_pid: u32) -> Result<Guard> {
    if !validate_run_id(run_id) {
        bail!("invalid recipe run id {run_id:?}");
    }
    let me = std::process::id();
    let recipe_run = Proc {
        pid: i32::try_from(me).with_context(|| format!("pid {me} is out of range"))?,
        start_ticks: pid_start_ticks(me)
            .with_context(|| format!("cannot read the start time of process {me}"))?,
    };
    let pid =
        i32::try_from(runner_pid).with_context(|| format!("pid {runner_pid} is out of range"))?;
    let runner = Runner {
        pid,
        pgid: pid,
        start_ticks: pid_start_ticks(runner_pid)
            .with_context(|| format!("cannot read the start time of process {runner_pid}"))?,
    };
    let record = Record {
        version: VERSION,
        run_id: run_id.to_string(),
        recipe_run,
        runner,
    };
    let bytes = serde_json::to_vec(&record).context("cannot encode the recipe run record")?;
    let procs = run_dir.join(PROCS);
    match fs::symlink_metadata(&procs) {
        Ok(meta) if !meta.file_type().is_dir() => {
            bail!("refusing {}: it is not a real folder", procs.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("cannot check {}", procs.display()));
        }
    }
    create_private_dir_all(&procs)?;
    let path = procs.join(file_name(run_id));
    write_atomic_0600(&path, &bytes)?;
    Ok(Guard { path })
}

/// The record of `run_id` under `work_root`, or `None` when there is none
/// that can be trusted: an invalid id, no `runs/` folder, no file, a run
/// folder, `procs` folder or record that is a symlink, a file over
/// [`MAX_RECORD`] bytes, owned by another user or writable by group or
/// others, a body that does not parse, another version or run id, a pid
/// `<= 1` or a runner group that is not its pid. The first acceptable record
/// wins. Changes nothing on disk and reads nothing under `/proc`.
pub(crate) fn find(work_root: &Path, run_id: &str) -> Option<Record> {
    if !validate_run_id(run_id) {
        return None;
    }
    let runs = work_root.join("runs");
    if !is_real_dir(&runs) {
        return None;
    }
    let name = file_name(run_id);
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    fs::read_dir(&runs)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path().join(PROCS))
        .filter(|procs| is_real_dir(procs))
        .find_map(|procs| read_record(&procs.join(&name), run_id, euid))
}

fn read_record(path: &Path, run_id: &str, euid: u32) -> Option<Record> {
    let (bytes, meta) = read_capped_nofollow_with_metadata(path, MAX_RECORD).ok()?;
    if !meta.file_type().is_file() || meta.uid() != euid || meta.mode() & 0o022 != 0 {
        return None;
    }
    let record: Record = serde_json::from_slice(&bytes).ok()?;
    let usable = record.version == VERSION
        && record.run_id == run_id
        && record.recipe_run.pid > 1
        && record.runner.pid > 1
        && record.runner.pgid == record.runner.pid;
    usable.then_some(record)
}

#[cfg(test)]
#[path = "run_record_tests.rs"]
mod tests;
