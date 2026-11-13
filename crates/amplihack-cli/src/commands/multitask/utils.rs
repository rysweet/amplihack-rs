//! Utility functions for the parallel workstream orchestrator.

use anyhow::{Result, bail};
use std::fs;
use std::io::Write;
use std::path::Path;
use tracing::debug;

/// Remove `dir`, but only after confirming it really sits under `base_dir`.
///
/// SR2 (issue #2553): the sink is irreversible and the directory name now
/// carries a string derived in part from model-produced text. Both paths are
/// canonicalised first so a symbolic link cannot carry the deletion out of the
/// base directory. Returns whether the removal was attempted.
pub(super) fn remove_dir_within(base_dir: &Path, dir: &Path) -> bool {
    if !fs::symlink_metadata(dir).is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
    {
        return false;
    }
    let (Ok(base), Ok(target)) = (fs::canonicalize(base_dir), fs::canonicalize(dir)) else {
        tracing::warn!(
            base = %base_dir.display(),
            dir = %dir.display(),
            "refusing to remove a workstream work directory that cannot be canonicalised"
        );
        return false;
    };
    // Every runtime artefact directory is an immediate child named from the
    // validated key. Canonical containment alone would accept a substituted
    // symlink to a different workstream under the same root.
    if dir.file_name().is_none_or(|name| target != base.join(name)) {
        tracing::warn!(
            base = %base.display(),
            dir = %target.display(),
            "refusing to remove a work directory outside the workstream base directory"
        );
        return false;
    }
    if let Err(e) = fs::remove_dir_all(&target) {
        tracing::warn!(dir = %target.display(), error = %e, "failed to remove work directory");
        return false;
    }
    true
}

/// Default base directory for workstream artifacts.
pub(super) fn default_base_dir() -> String {
    format!(
        "{}/amplihack-workstreams",
        std::env::temp_dir().to_string_lossy()
    )
}

/// Commit a complete private replacement, then durably record the rename.
/// NamedTempFile removes failed replacements and cannot collide with leftovers
/// from interrupted writers. The destination remains intact until commit.
pub(super) fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    atomic_write_from(path, |file| file.write_all(data))
}

fn atomic_write_from(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    write(temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// Set a file as executable (chmod +x). No-op on Windows.
#[cfg(unix)]
pub(super) fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Calculate total size of a directory tree.
pub(super) fn dir_size_bytes(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if path.is_dir() {
                total += dir_size_bytes(&path);
            }
        }
    }
    total
}

/// Simple pseudo-random u32 using time-based seed (no external crate needed).
pub(super) fn rand_u32() -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_nanos() & 0xFFFF_FFFF) as u32
}

/// Check available disk space at the given path.
pub(super) fn check_disk_space(base_dir: &Path, min_free_gb: f64) -> Result<()> {
    #[cfg(unix)]
    {
        // Use statvfs to check available space
        let path_cstr = std::ffi::CString::new(base_dir.to_string_lossy().as_bytes())?;
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path_cstr.as_ptr(), &mut stat) == 0 {
                // macOS `statvfs` uses `u32` fields; Linux uses `u64`. Cast
                // both so the multiplication type-checks on every target.
                #[allow(clippy::unnecessary_cast)]
                let free_bytes = (stat.f_bavail as u64) * (stat.f_frsize as u64);
                let free_gb = free_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                if free_gb < min_free_gb {
                    bail!(
                        "Insufficient disk space: {free_gb:.1}GB free, need {min_free_gb}GB minimum"
                    );
                }
                debug!("Disk space check: {free_gb:.1}GB available");
            }
        }
    }
    #[cfg(not(unix))]
    {
        // Windows: skip the precondition check. multitask is not yet
        // wired into Windows release flows; if/when it is, port via
        // GetDiskFreeSpaceExW. Silently passing is safer than failing.
        let _ = (base_dir, min_free_gb);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_replacements_preserve_committed_data_and_remove_temporary_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        atomic_write(&path, b"committed").unwrap();
        let failure = atomic_write_from(&path, |file| {
            file.write_all(b"incomplete")?;
            Err(std::io::Error::other("injected write failure"))
        })
        .unwrap_err();
        assert!(failure.to_string().contains("injected write failure"));
        assert_eq!(fs::read(&path).unwrap(), b"committed");
        let directory_target = temp.path().join("directory");
        fs::create_dir(&directory_target).unwrap();
        assert!(atomic_write(&directory_target, b"cannot replace directory").is_err());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
        assert_eq!(fs::read(&path).unwrap(), b"committed");
        atomic_write(&path, b"recovered").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"recovered");
    }

    #[test]
    fn test_atomic_write() {
        let dir = std::env::temp_dir().join("amplihack-test-atomic-write");
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("test.json");
        atomic_write(&file, b"hello world").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello world");
        let _ = fs::remove_dir_all(&dir);
    }
}
