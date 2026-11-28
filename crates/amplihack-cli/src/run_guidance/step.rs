//! Guidance for an agent step that is already running (issue #3435).
//!
//! Every agent launch inside a recipe run (`AMPLIHACK_RUN_GUIDANCE_DIRS`
//! set) gets a fresh step id, which the launcher exports to the agent as
//! [`STEP_ID_ENV`]. The step's hooks (PostToolUse and UserPromptSubmit, in
//! Claude Code and in Codex) run `amplihack recipe guidance deliver`, which
//! calls [`deliver`]: it gives the step the entries it has not been given
//! yet, as one block, and records them in `guidance_injected` with the step
//! id. An entry is therefore given to a step once, whether at launch or by a
//! hook; a later step, with its own id, receives every entry at launch, as
//! before.
//!
//! The delivered set of a step is read back from `guidance.log`
//! ([`super::log_state::LogState::delivered_by_step`]), under an exclusive
//! `flock` on each folder's log, taken in one fixed order, so hooks that run
//! at the same moment (parallel tool calls) cannot both deliver an entry.
//! The lock blocks until it is free; there is no timeout, poll or retry.
//!
//! A launch form with no such hook writes `guidance_step_unhooked` at
//! launch ([`record_step_start`]). `recipe guide` reads it after an append
//! and names each such step that is still running ([`step_alive`]), so the
//! sender knows at once that the step will not see the entry.
//!
//! The launch's judgement that a step has a hook is not proof that the hook
//! fires: the hooks binary may be missing, hooks may be off in a layer the
//! launch does not read, or the harness may not fire that event. So a
//! hooked launch writes `guidance_step_hooked`, and the first hook run that
//! reaches `amplihack recipe guidance deliver` writes
//! `guidance_step_hook_seen` ([`record_hook_seen`]). `recipe guide` names
//! each hooked step that is still running and has no such record as not yet
//! confirmed, so a hook that never fires is reported from evidence, not
//! assumed to work. There is no timer.

use super::env::{DirCheck, listed_dirs};
use super::fs_safety::{DirState, dir_state};
use super::log_state::{StepStart, read_log_state};
use super::{
    Entry, GUIDANCE_DIRS_ENV, GUIDANCE_LOG, GuidanceForLaunch, GuideError, StepLabel, open_log,
    read_entries, valid_step_run_id, warn,
};
use crate::commands::recipe::run_id::validate_run_id;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The variable that names the running agent step to its hooks.
pub(crate) const STEP_ID_ENV: &str = "AMPLIHACK_GUIDANCE_STEP_ID";
/// The longest step id accepted.
const MAX_STEP_ID: usize = 200;
/// The longest run id kept as the prefix of a step id.
const MAX_PREFIX: usize = 160;

/// Whether `id` can be a step id: `[A-Za-z0-9._-]`, not starting with `.`,
/// at most [`MAX_STEP_ID`] bytes.
pub(crate) fn valid_step_id(id: &str) -> bool {
    id.len() <= MAX_STEP_ID && validate_run_id(id)
}

/// A new step id: the run id (or `step` when it is missing or invalid)
/// joined to a random suffix.
pub(crate) fn new_step_id(run_id: &str) -> String {
    let prefix = if validate_run_id(run_id) && run_id.len() <= MAX_PREFIX {
        run_id
    } else {
        "step"
    };
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

/// The step id an agent launch exports: a new one inside a recipe run (the
/// guidance folder list is set and not empty), `None` otherwise. A value
/// inherited from a parent step is never reused.
pub(crate) fn step_id_for_launch() -> Option<String> {
    std::env::var_os(GUIDANCE_DIRS_ENV).filter(|v| !v.is_empty())?;
    Some(new_step_id(
        &std::env::var("AMPLIHACK_RECIPE_RUN_ID").unwrap_or_default(),
    ))
}

/// The entries of `entries` that `delivered` does not list for their folder,
/// in order. An entry with copies in several folders counts as given when
/// any copy was.
pub(crate) fn undelivered(
    entries: &[Entry],
    delivered: &HashMap<PathBuf, HashSet<String>>,
) -> Vec<Entry> {
    let given = |dir: &Path, name: &str| delivered.get(dir).is_some_and(|set| set.contains(name));
    entries
        .iter()
        .filter(|e| !(given(&e.dir, &e.name) || e.also_in.iter().any(|(d, n)| given(d, n))))
        .cloned()
        .collect()
}

/// `guidance.log` beside a guidance folder, opened for appending and held
/// under an exclusive `flock` until dropped.
pub(crate) struct LockedLog {
    file: std::fs::File,
    path: PathBuf,
}

impl LockedLog {
    /// Opens (creating at 0600) and locks the log beside `guidance_dir`.
    /// Blocks until the lock is free.
    pub(crate) fn open(guidance_dir: &Path) -> Result<Self, GuideError> {
        let parent = guidance_dir.parent().ok_or_else(|| {
            GuideError::NotFound(format!("{} has no parent folder", guidance_dir.display()))
        })?;
        let path = parent.join(GUIDANCE_LOG);
        let file = open_log(&path)?;
        fs4::fs_std::FileExt::lock_exclusive(&file)
            .map_err(|e| GuideError::Io(format!("cannot lock {}: {e}", path.display())))?;
        Ok(Self { file, path })
    }

    /// Appends one JSON line.
    pub(crate) fn append(&mut self, record: &serde_json::Value) -> Result<(), GuideError> {
        let mut line = record.to_string();
        line.push('\n');
        self.file
            .write_all(line.as_bytes())
            .map_err(|e| GuideError::Io(format!("cannot write {}: {e}", self.path.display())))
    }
}

impl Drop for LockedLog {
    fn drop(&mut self) {
        let _ = fs4::fs_std::FileExt::unlock(&self.file);
    }
}

/// Gives step `step_id` the entries of `dirs` it has not been given yet:
/// returns their block, after recording them in `guidance_injected` with
/// the step id in each folder they came from. `Ok(None)` when there is
/// nothing new.
///
/// # Errors
///
/// [`GuideError::Usage`] for an invalid step id; an error when a log cannot
/// be locked, read or written. Nothing is returned as delivered then.
pub(crate) fn deliver(
    step_id: &str,
    step_run_id: &str,
    dirs: &[PathBuf],
) -> Result<Option<String>, GuideError> {
    if !valid_step_id(step_id) {
        return Err(GuideError::Usage(format!(
            "{STEP_ID_ENV} is not a valid step id"
        )));
    }
    let mut order: Vec<&PathBuf> = dirs.iter().collect();
    order.sort();
    order.dedup();
    // One fixed order for every caller, so two callers never wait on each
    // other's locks.
    let mut locks: Vec<(PathBuf, LockedLog)> = Vec::with_capacity(order.len());
    for dir in order {
        locks.push((dir.clone(), LockedLog::open(dir)?));
    }
    let entries = read_entries(dirs);
    if entries.is_empty() {
        return Ok(None);
    }
    let mut delivered: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    for (dir, _) in &locks {
        let mut state = read_log_state(dir);
        if let Some(reason) = state.log_error {
            return Err(GuideError::Unsafe(format!(
                "cannot tell what this step was given: {reason}"
            )));
        }
        delivered.insert(
            dir.clone(),
            state.delivered_by_step.remove(step_id).unwrap_or_default(),
        );
    }
    let new = undelivered(&entries, &delivered);
    if new.is_empty() {
        return Ok(None);
    }
    let guidance = GuidanceForLaunch::from_entries(new);
    let ts = super::now_text();
    for (dir, lock) in &mut locks {
        if let Some(record) = guidance.injection_record(dir, &ts, step_run_id, Some(step_id)) {
            lock.append(&record)?;
        }
    }
    Ok(guidance.block().map(str::to_string))
}

/// The `guidance.log` paths beside every listed guidance folder whose
/// parent exists and passes the trust rule, the folder itself not being
/// untrusted. The folder need not exist yet.
fn listed_logs() -> Vec<PathBuf> {
    let Some(value) = std::env::var_os(GUIDANCE_DIRS_ENV).filter(|v| !v.is_empty()) else {
        return Vec::new();
    };
    let mut logs = Vec::new();
    for (dir, check) in listed_dirs(&value) {
        if matches!(check, DirCheck::Untrusted(_)) {
            continue;
        }
        let Some(parent) = dir.parent() else { continue };
        if matches!(dir_state(parent), Ok(DirState::Trusted)) {
            logs.push(dir);
        }
    }
    logs
}

/// Records, in the log beside every listed folder ([`listed_logs`]), that
/// step `step_id` has started: `guidance_step_hooked` when the launch judged
/// that it has a hook that gives a running step new guidance,
/// `guidance_step_unhooked` when not. The record names this process and its
/// start time, which is how [`step_alive`] tells that the step is still
/// running. A failed write is a warning.
pub(crate) fn record_step_start(step_id: &str, label: &StepLabel, hooked: bool) {
    let pid = std::process::id();
    let event = if hooked {
        "guidance_step_hooked"
    } else {
        "guidance_step_unhooked"
    };
    let mut record = serde_json::json!({
        "event": event,
        "ts": super::now_text(),
        "step_id": step_id,
        "step_run_id": valid_step_run_id(&label.step_run_id),
        "tool": label.tool,
        "launch_form": label.launch_form,
        "pid": pid,
    });
    if let Some(agent) = label.agent.as_deref() {
        record["agent"] = agent.into();
    }
    if let Some(start) = process_start(pid) {
        record["start"] = start.into();
    }
    for dir in listed_logs() {
        let Some(parent) = dir.parent() else { continue };
        if let Err(e) = super::log_event(&parent.join(GUIDANCE_LOG), &record) {
            let what = if hooked {
                "this step's start was not logged, so a hook that never fires cannot be reported"
            } else {
                "this step cannot take guidance while it runs, and that was not logged"
            };
            warn(&format!("amplihack: warning: {what}: {e}"));
        }
    }
}

/// Records, once per step, that a hook of step `step_id` has run and
/// reached `amplihack`: a `guidance_step_hook_seen` record in the log
/// beside every listed folder ([`listed_logs`]) that does not hold one for
/// it yet, written under the log's lock. It confirms the launch's judgement
/// that the step has a hook.
///
/// # Errors
///
/// [`GuideError::Usage`] for an invalid step id; an error when a log cannot
/// be locked, read or written.
pub(crate) fn record_hook_seen(step_id: &str, step_run_id: &str) -> Result<(), GuideError> {
    if !valid_step_id(step_id) {
        return Err(GuideError::Usage(format!(
            "{STEP_ID_ENV} is not a valid step id"
        )));
    }
    let mut dirs = listed_logs();
    dirs.sort();
    for dir in dirs {
        let mut lock = LockedLog::open(&dir)?;
        let state = read_log_state(&dir);
        if let Some(reason) = state.log_error {
            return Err(GuideError::Unsafe(format!(
                "cannot record that this step's hook ran: {reason}"
            )));
        }
        if state.hook_seen.contains(step_id) {
            continue;
        }
        lock.append(&serde_json::json!({
            "event": "guidance_step_hook_seen",
            "ts": super::now_text(),
            "step_id": step_id,
            "step_run_id": valid_step_run_id(step_run_id),
        }))?;
    }
    Ok(())
}

/// The start time of process `pid`, in clock ticks since boot, from
/// `/proc/<pid>/stat`. `None` where the system does not give it.
pub(crate) fn process_start(pid: u32) -> Option<u64> {
    if cfg!(target_os = "linux") {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_start_time(&stat)
    } else {
        None
    }
}

/// Field 22 of a `/proc/<pid>/stat` line. The command name (field 2) may
/// hold spaces and parentheses, so fields are counted after its last `)`.
pub(crate) fn parse_start_time(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    // The first field after the name is field 3.
    rest.split_whitespace().nth(22 - 3)?.parse().ok()
}

/// Whether a process recorded with `recorded` start time is the one now
/// running under its pid. `current` is the start time read now (`None`
/// when the process is gone or the system gives none) and `exists` whether
/// a process with that pid exists at all.
pub(crate) fn judge_alive(recorded: Option<u64>, current: Option<u64>, exists: bool) -> bool {
    match (recorded, current) {
        (Some(recorded), Some(current)) => recorded == current,
        // The start time was readable when the step began, and is not now:
        // the process is gone.
        (Some(_), None) => false,
        (None, _) => exists,
    }
}

/// Whether a process with this pid exists (`kill(pid, 0)`).
fn pid_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 sends nothing; it only checks the pid.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Whether the step a step start record names is still running.
pub(crate) fn step_alive(step: &StepStart) -> bool {
    judge_alive(step.start, process_start(step.pid), pid_exists(step.pid))
}
