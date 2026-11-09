//! Issue #2018: the record a headless recipe step leaves where a supervisor
//! can find it.
//!
//! A launcher started by a recipe step (see `crate::step_lifetime`) writes
//! one record per agent it spawns, at
//! `<HOME>/.amplihack/runtime/recipe-steps/<recipe_run_id>/<launcher_pid>.json`,
//! and removes it when the step ends. `amplihack recipe steps` lists the
//! records and classifies each one as live, orphan or stale.
//!
//! - `mod.rs`: the format-1 schema, the location and the run-id rule.
//! - `write.rs`: [`StepRecordGuard`], which writes a record and removes it on
//!   drop.
//! - `probe.rs`: [`Probe`], [`SystemProbe`] and [`classify`].
//! - `list.rs`: [`list`], which reads every record under the root.
//! - `collect.rs`: [`collect_run`] and [`collect_abandoned`], which remove
//!   records classified `stale`, for `recipe run` (see that module).
//!
//! Fixed invariant: nothing here sends a signal on the strength of a
//! record's contents. The writer removes only the path it wrote itself, the
//! reader only reads, and the collector removes only a record that, read a
//! second time just before the removal, is unchanged and still names no
//! running process by pid and start time. Any future code that signals a
//! process named by a record needs its own fresh check of the process's
//! owner, start time and ancestry, and its own review. See
//! `docs/reference/recipe-step-records.md`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod collect;
mod list;
mod probe;
mod write;

pub use collect::{
    collect_abandoned, collect_abandoned_as, collect_abandoned_in_home, collect_after_attempt,
    collect_run, collect_run_as,
};
pub use list::{Listed, list, list_as};
pub use probe::{Probe, StepState, SystemProbe, classify, proc_id_of};
pub use write::StepRecordGuard;

#[cfg(test)]
mod tests_classify;
#[cfg(test)]
mod tests_collect;
#[cfg(test)]
mod tests_list;
#[cfg(test)]
mod tests_list_dirs;
#[cfg(test)]
mod tests_schema;
#[cfg(test)]
mod tests_support;
#[cfg(test)]
mod tests_write;

/// The record layout this code writes and the only one it reads.
pub const FORMAT: u32 = 1;

/// The largest record written or read, in bytes.
pub const MAX_RECORD_BYTES: usize = 64 * 1024;

/// The longest run id accepted, in bytes.
const MAX_RUN_ID_BYTES: usize = 128;

/// A process, told apart from a later process given the same pid by its
/// start time (field 22 of `/proc/<pid>/stat`, `None` off Linux or when it
/// could not be read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcId {
    pub pid: i32,
    pub start_ticks: Option<u64>,
}

/// One headless step, format 1. Every field is always serialised: `None` is
/// written as `null` and never left out. The field set is fixed so that no
/// argument, prompt, token or other environment variable can be recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRecord {
    pub format: u32,
    /// `AMPLIHACK_RECIPE_RUN_ID`, equal to the run-id directory name.
    pub run_id: String,
    /// `AMPLIHACK_RUN_ID`, or `None` when unset or empty.
    pub workspace_run_id: Option<String>,
    /// `AMPLIHACK_RUN_DIR`, or `None` when unset or empty.
    pub run_dir: Option<String>,
    /// The agent tool the launcher started.
    pub tool: String,
    /// The launcher's working directory when the record was written.
    pub cwd: String,
    /// RFC 3339 UTC, to the second, with a `Z` suffix.
    pub created_at: String,
    pub launcher: ProcId,
    pub agent: ProcId,
    /// The launcher's parent, or `None` when `getppid()` returned 0 (a parent
    /// outside the launcher's PID namespace).
    pub parent: Option<ProcId>,
}

impl StepRecord {
    /// A record for this launch, with the run ids, run directory, working
    /// directory and time taken from this process. The run id is not
    /// validated here: [`StepRecordGuard::try_write`] refuses an invalid one
    /// before building any path.
    pub fn from_env(
        tool: &str,
        launcher: ProcId,
        agent: ProcId,
        parent: Option<ProcId>,
    ) -> StepRecord {
        let run_id = std::env::var_os(crate::commands::governor::RECIPE_RUN_ID_ENV)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let cwd = std::env::current_dir()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default();
        StepRecord {
            format: FORMAT,
            run_id,
            workspace_run_id: non_empty_env("AMPLIHACK_RUN_ID"),
            run_dir: non_empty_env("AMPLIHACK_RUN_DIR"),
            tool: tool.to_string(),
            cwd,
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            launcher,
            agent,
            parent,
        }
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string_lossy().into_owned())
}

/// `<home>/.amplihack/runtime/recipe-steps`. `AMPLIHACK_HOME`,
/// `AMPLIHACK_RUNTIME_ROOT` and `AMPLIHACK_WORKFLOW_RUNTIME_DIR` do not move
/// it: the last two name per-run directories that are removed when the run
/// ends, and a record must outlive its run to be useful.
pub fn root(home: &Path) -> PathBuf {
    home.join(".amplihack").join("runtime").join("recipe-steps")
}

/// `^[A-Za-z0-9_-]{1,128}$` on bytes, so no run id can name `.`, `..`, a
/// separator or a character a terminal would act on.
pub fn valid_run_id(run_id: &str) -> bool {
    (1..=MAX_RUN_ID_BYTES).contains(&run_id.len())
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `<root>/<run_id>/<launcher_pid>.json`. The caller validates `run_id`.
pub fn record_path(root: &Path, run_id: &str, launcher_pid: i32) -> PathBuf {
    root.join(run_id).join(format!("{launcher_pid}.json"))
}

/// The launcher pid named by a record file name, which must be
/// `^[1-9][0-9]{0,9}\.json$` and fit in an `i32`.
fn pid_from_record_name(name: &str) -> Option<i32> {
    let digits = name.strip_suffix(".json")?;
    let well_formed = (1..=10).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !digits.starts_with('0');
    if !well_formed {
        return None;
    }
    digits.parse::<i32>().ok()
}

/// This process's effective uid.
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}
