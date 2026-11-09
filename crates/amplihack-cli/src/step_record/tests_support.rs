//! Issue #2018: helpers shared by the `step_record` unit tests.
//!
//! `step_record/mod.rs` declares, under `#[cfg(test)]`:
//! `mod tests_support; mod tests_write; mod tests_classify; mod tests_list;`
//!
//! The contract the tests fix, reached through `crate::step_record`:
//!
//! - `FORMAT: u32 = 1` and `MAX_RECORD_BYTES: usize = 64 * 1024`.
//! - `ProcId { pid: i32, start_ticks: Option<u64> }` and `StepRecord` with
//!   the format-1 fields (`parent: Option<ProcId>`), both
//!   `Serialize + Deserialize + Debug + Clone + PartialEq`. `Option` fields
//!   serialise as `null` and are never skipped.
//! - `root(home) -> PathBuf`, `<home>/.amplihack/runtime/recipe-steps`.
//! - `valid_run_id(&str) -> bool`, `^[A-Za-z0-9_-]{1,128}$` on bytes.
//! - `record_path(root, run_id, launcher_pid: i32) -> PathBuf`.
//! - `StepRecordGuard::try_write(root, &record) -> anyhow::Result<Self>`,
//!   which is `try_write_as(root, &record, geteuid())`. The uid parameter is
//!   the only seam: it lets a test stand in for "owned by another user"
//!   without root. The error text (`{:#}`) starts with one of the fixed
//!   reasons in the reference page's Diagnostics table. `path()` names the
//!   record; `Drop` removes it, then the run-id directory if it is empty.
//! - `trait Probe { fn exists(&self, pid: i32) -> bool; fn start_ticks(&self,
//!   pid: i32) -> Option<u64>; }`, `SystemProbe`, and
//!   `classify(&record, &probe) -> StepState` (`Live`, `Orphan`, `Stale`,
//!   with `as_str()` giving the lowercase name).
//! - `list(root) -> Vec<Listed>`, which is `list_as(root, geteuid())`, with
//!   `Listed::Record { path, record }` or `Listed::Unreadable { path, reason }`.
//!
//! Every test works inside its own `tempfile` directory and never reads or
//! writes the real home directory.

#![cfg(unix)]

use crate::step_record::{FORMAT, Listed, ProcId, StepRecord};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(super) const RUN: &str = "5b60657b-76ef-4f49-8a22-8b89ed75f43e";

pub(super) fn proc_id(pid: i32, start_ticks: Option<u64>) -> ProcId {
    ProcId { pid, start_ticks }
}

pub(super) fn record(run_id: &str, launcher: i32) -> StepRecord {
    StepRecord {
        format: FORMAT,
        run_id: run_id.to_string(),
        workspace_run_id: None,
        run_dir: Some("/tmp/runs/ws-1".to_string()),
        tool: "claude".to_string(),
        cwd: "/tmp/project".to_string(),
        created_at: "2026-11-07T10:15:03Z".to_string(),
        launcher: proc_id(launcher, Some(1000)),
        agent: proc_id(launcher + 1, Some(1001)),
        parent: Some(proc_id(launcher - 1, Some(999))),
    }
}

pub(super) fn record_json(run_id: &str, launcher: i32) -> serde_json::Value {
    serde_json::to_value(record(run_id, launcher)).unwrap()
}

/// Writes `body` as `<root>/<run>/<name>`, creating the run directory.
pub(super) fn plant(root: &Path, run: &str, name: &str, body: &[u8]) -> PathBuf {
    let dir = root.join(run);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    path
}

pub(super) fn plant_json(root: &Path, run: &str, name: &str, value: &serde_json::Value) -> PathBuf {
    plant(root, run, name, &serde_json::to_vec(value).unwrap())
}

pub(super) fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

pub(super) fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// This process's effective uid.
pub(super) fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// A uid that is not this process's, for the ownership checks.
pub(super) fn other_uid() -> u32 {
    euid().wrapping_add(1)
}

/// Root ignores directory permissions, so the "cannot read" cases cannot be
/// produced; those tests return early when this is true.
pub(super) fn running_as_root() -> bool {
    euid() == 0
}

/// Names in `dir`, sorted, or none when it does not exist.
pub(super) fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

pub(super) fn records(listed: &[Listed]) -> Vec<(&Path, &StepRecord)> {
    listed
        .iter()
        .filter_map(|entry| match entry {
            Listed::Record { path, record } => Some((path.as_path(), record)),
            Listed::Unreadable { .. } => None,
        })
        .collect()
}

pub(super) fn unreadable(listed: &[Listed]) -> Vec<(&Path, &str)> {
    listed
        .iter()
        .filter_map(|entry| match entry {
            Listed::Unreadable { path, reason } => Some((path.as_path(), reason.as_str())),
            Listed::Record { .. } => None,
        })
        .collect()
}

/// The one entry `list` gave, which must be unreadable, as `(path, reason)`.
pub(super) fn only_unreadable(listed: &[Listed]) -> (PathBuf, String) {
    assert_eq!(listed.len(), 1, "exactly one entry expected: {listed:?}");
    match &listed[0] {
        Listed::Unreadable { path, reason } => (path.clone(), reason.clone()),
        other => panic!("expected an unreadable entry, got {other:?}"),
    }
}
