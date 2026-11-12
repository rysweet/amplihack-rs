//! How a preflight refusal reaches the final event of the `recipe run` that
//! started it. The PR worktree decision is made once, by the loop recipe's
//! own preflight step calling `amplihack orch helper autodrive-preflight`
//! after that step has made the state directory private and applied its
//! merged and phase-done skips (autodrive_state.sh). The runner makes no
//! decision of its own: it only reads the refusal the helper handed back.
//! Child stdout and persisted state files never assert a refusal.
use super::autodrive_preflight::RefusalOutcome;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

pub(crate) fn known(outcome: &str) -> bool {
    serde_json::from_value::<RefusalOutcome>(Value::String(outcome.into())).is_ok()
}

/// File in the run's private runtime directory through which the
/// `autodrive-preflight` helper hands a refusal to the `recipe run` that
/// started it, whether the loop ran directly or as a sub-recipe of
/// `auto-drive-to-merge` inside one recipe-runner child. Without it the
/// final event would read `failure`.
pub(crate) const HANDOFF_FILE: &str = "autodrive-preflight-refusal.json";
const RUNTIME_DIR_ENV: &str = "AMPLIHACK_WORKFLOW_RUNTIME_DIR";
const RUN_ID_ENV: &str = "AMPLIHACK_RECIPE_RUN_ID";

/// Helper side: write `envelope`, stamped with this process's recipe run id,
/// to the run's runtime directory. Does nothing outside a `recipe run`.
pub(crate) fn write_handoff(envelope: &Value) -> Result<()> {
    let (Some(dir), Some(run_id)) = (
        std::env::var_os(RUNTIME_DIR_ENV),
        std::env::var(RUN_ID_ENV).ok(),
    ) else {
        return Ok(());
    };
    let dir = std::path::PathBuf::from(dir);
    if dir.as_os_str().is_empty() || run_id.is_empty() {
        return Ok(());
    }
    let mut stamped = envelope.clone();
    stamped["run_id"] = Value::String(run_id);
    // NamedTempFile is created 0600 and renamed into place atomically.
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
    serde_json::to_writer(&mut tmp, &stamped)?;
    std::io::Write::write_all(&mut tmp, b"\n")?;
    tmp.as_file().sync_all()?;
    tmp.persist(dir.join(HANDOFF_FILE))?;
    Ok(())
}

/// Runner side: the refusal a helper handed to this run, if any. Accepted
/// only from a regular file (not a symlink) owned by this user with mode
/// 0600 in the run's own runtime directory, stamped with this attempt's run
/// id, carrying a known refusal outcome. Anything else is ignored, and the
/// run ends as it otherwise would.
pub(crate) fn read_handoff(runtime_dir: &Path, run_id: &str) -> Option<(RefusalOutcome, Value)> {
    use std::io::Read;
    let path = runtime_dir.join(HANDOFF_FILE);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o777 != 0o600 {
            return None;
        }
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 1024 * 1024 {
        return None;
    }
    let mut envelope: Value = serde_json::from_slice(&bytes).ok()?;
    if envelope["run_id"].as_str() != Some(run_id) || envelope["terminal_refusal"] != true {
        return None;
    }
    let outcome = serde_json::from_value::<RefusalOutcome>(envelope["outcome"].clone()).ok()?;
    if let Some(map) = envelope.as_object_mut() {
        map.remove("run_id");
    }
    Some((outcome, envelope))
}
