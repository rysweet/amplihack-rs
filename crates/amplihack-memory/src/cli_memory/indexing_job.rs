use super::{ensure_parent_dir, project_artifact_paths};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub fn background_index_job_path(project_path: &Path) -> Result<PathBuf> {
    Ok(project_artifact_paths(project_path)?.indexing_pid)
}

/// Whether a background indexer is writing for this project.
///
/// Both the cache location and the pre-#1476 in-repo location are read. An
/// unmigrated project has no `<artifact_root>/indexing.pid` — the artifact root
/// did not exist when the job started — so checking only the new path would
/// fail open on the single upgrade every existing user performs and start a
/// second indexer over the same tree.
pub fn background_index_job_active(project_path: &Path) -> Result<bool> {
    let current = background_index_job_path(project_path)?;
    let legacy = project_path.join(".amplihack").join("indexing.pid");
    Ok(pid_file_active(&current) || pid_file_active(&legacy))
}

fn pid_file_active(pid_path: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(pid_path) else {
        return false;
    };
    let Ok(pid) = raw.trim().parse::<u32>() else {
        let _ = fs::remove_file(pid_path);
        return false;
    };
    if is_process_alive(pid) {
        return true;
    }
    let _ = fs::remove_file(pid_path);
    false
}

pub fn record_background_index_pid(project_path: &Path, pid: u32) -> Result<()> {
    let pid_path = background_index_job_path(project_path)?;
    ensure_parent_dir(&pid_path)?;
    fs::write(pid_path, format!("{pid}\n"))?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn is_process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
pub(crate) fn is_process_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{HomeGuard, env_lock};
    use tempfile::TempDir;

    /// The PID file now lives in the per-project cache, so these tests must
    /// point `HOME`/`XDG_CACHE_HOME` at a tempdir or they would write into the
    /// developer's real cache.
    struct Fixture {
        project: TempDir,
        _home_dir: TempDir,
        _cache: TempDir,
        _home: HomeGuard,
        previous_xdg: Option<std::ffi::OsString>,
        previous_artifact: Option<std::ffi::OsString>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // SAFETY: serialised by env_lock().
            unsafe {
                match self.previous_xdg.take() {
                    Some(value) => std::env::set_var("XDG_CACHE_HOME", value),
                    None => std::env::remove_var("XDG_CACHE_HOME"),
                }
                match self.previous_artifact.take() {
                    Some(value) => std::env::set_var("AMPLIHACK_ARTIFACT_DIR", value),
                    None => std::env::remove_var("AMPLIHACK_ARTIFACT_DIR"),
                }
            }
        }
    }

    fn fixture() -> Fixture {
        let home_dir = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let previous_xdg = std::env::var_os("XDG_CACHE_HOME");
        let previous_artifact = std::env::var_os("AMPLIHACK_ARTIFACT_DIR");
        // SAFETY: serialised by env_lock().
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", cache.path());
            std::env::remove_var("AMPLIHACK_ARTIFACT_DIR");
        }
        Fixture {
            _home: HomeGuard::set(home_dir.path()),
            project: TempDir::new().unwrap(),
            _home_dir: home_dir,
            _cache: cache,
            previous_xdg,
            previous_artifact,
        }
    }

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn background_index_job_active_returns_false_when_missing() {
        let _guard = lock();
        let fx = fixture();
        assert!(!background_index_job_active(fx.project.path()).unwrap());
    }

    #[test]
    fn background_index_job_active_accepts_live_pid() {
        let _guard = lock();
        let fx = fixture();
        record_background_index_pid(fx.project.path(), std::process::id()).unwrap();
        assert!(background_index_job_active(fx.project.path()).unwrap());
    }

    #[test]
    fn background_index_job_active_cleans_stale_pid() {
        let _guard = lock();
        let fx = fixture();
        record_background_index_pid(fx.project.path(), u32::MAX).unwrap();
        assert!(!background_index_job_active(fx.project.path()).unwrap());
        assert!(!background_index_job_path(fx.project.path()).unwrap().exists());
    }

    /// The upgrade window: a job recorded by a pre-#1476 build left its PID in
    /// the checkout, and migrating out from under it would corrupt the graph.
    #[test]
    fn a_legacy_in_repo_pid_file_still_counts_as_active() {
        let _guard = lock();
        let fx = fixture();
        let legacy = fx.project.path().join(".amplihack").join("indexing.pid");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, format!("{}\n", std::process::id())).unwrap();

        assert!(background_index_job_active(fx.project.path()).unwrap());
    }
}
