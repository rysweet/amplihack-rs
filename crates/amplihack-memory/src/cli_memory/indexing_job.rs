use super::{ensure_parent_dir, project_artifact_paths};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub fn background_index_job_path(project_path: &Path) -> PathBuf {
    project_artifact_paths(project_path).indexing_pid
}

pub fn background_index_job_active(project_path: &Path) -> Result<bool> {
    let pid_path = background_index_job_path(project_path);
    let Ok(raw) = fs::read_to_string(&pid_path) else {
        return Ok(false);
    };
    let Ok(pid) = raw.trim().parse::<u32>() else {
        let _ = fs::remove_file(&pid_path);
        return Ok(false);
    };
    if is_process_alive(pid) {
        return Ok(true);
    }
    let _ = fs::remove_file(&pid_path);
    Ok(false)
}

pub fn record_background_index_pid(project_path: &Path, pid: u32) -> Result<()> {
    let pid_path = background_index_job_path(project_path);
    ensure_parent_dir(&pid_path)?;
    fs::write(pid_path, format!("{pid}\n"))?;
    Ok(())
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn background_index_job_active_returns_false_when_missing() {
        let dir = TempDir::new().unwrap();
        assert!(!background_index_job_active(dir.path()).unwrap());
    }

    #[test]
    fn background_index_job_active_accepts_live_pid() {
        let dir = TempDir::new().unwrap();
        record_background_index_pid(dir.path(), std::process::id()).unwrap();
        assert!(background_index_job_active(dir.path()).unwrap());
    }

    #[test]
    fn background_index_job_active_cleans_stale_pid() {
        let dir = TempDir::new().unwrap();
        record_background_index_pid(dir.path(), u32::MAX).unwrap();
        assert!(!background_index_job_active(dir.path()).unwrap());
        assert!(!background_index_job_path(dir.path()).exists());
    }
}
