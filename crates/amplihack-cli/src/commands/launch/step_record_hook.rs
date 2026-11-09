//! Issue #2018: the step record a headless launcher writes after spawning
//! its agent.
//!
//! A record failure never stops the step (D6): the launcher prints one
//! sanitised line on stderr and carries on without a record.

use crate::commands::install::paths::sanitize_text;
use crate::step_lifetime::Binding;
use anyhow::{Result, anyhow};

#[cfg(unix)]
pub(super) type Guard = crate::step_record::StepRecordGuard;

/// Off Unix there are no step records; nothing is ever held.
#[cfg(not(unix))]
pub(super) type Guard = ();

/// Writes the record for the agent `agent_pid` that this launcher has just
/// spawned. `None` for an interactive launch or when the write failed.
pub(super) fn write_step_record(binding: &Binding, tool: &str, agent_pid: u32) -> Option<Guard> {
    if !binding.headless {
        return None;
    }
    match try_write(binding, tool, agent_pid) {
        Ok(guard) => Some(guard),
        Err(error) => {
            // The whole chain is sanitised: it may embed a path.
            eprintln!(
                "{}",
                sanitize_text(&format!(
                    "amplihack: not writing a step record: {error:#}; continuing"
                ))
            );
            None
        }
    }
}

#[cfg(unix)]
fn try_write(binding: &Binding, tool: &str, agent_pid: u32) -> Result<Guard> {
    use crate::step_record::{ProcId, StepRecord, StepRecordGuard, proc_id_of, root};

    let home = crate::commands::recipe::home_dir()
        .ok()
        .filter(|home| home.is_absolute())
        .ok_or_else(|| anyhow!("HOME is not set"))?;
    let agent_pid = i32::try_from(agent_pid).map_err(|_| anyhow!("invalid pid"))?;
    let parent = (binding.parent_pid != 0).then_some(ProcId {
        pid: binding.parent_pid,
        start_ticks: binding.parent_start_ticks,
    });
    let record = StepRecord::from_env(
        tool,
        proc_id_of(std::process::id() as i32),
        proc_id_of(agent_pid),
        parent,
    );
    let guard = StepRecordGuard::try_write(&root(&home), &record)?;
    tracing::debug!(path = %guard.path().display(), "wrote step record");
    Ok(guard)
}

#[cfg(not(unix))]
fn try_write(_binding: &Binding, _tool: &str, _agent_pid: u32) -> Result<Guard> {
    Err(anyhow!("step records are written on Unix only"))
}
