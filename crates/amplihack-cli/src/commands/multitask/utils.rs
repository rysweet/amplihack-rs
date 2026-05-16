//! Utility functions for the parallel workstream orchestrator.

use anyhow::{Result, bail};
use std::fs;
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use tracing::debug;

/// Default base directory for workstream artifacts.
pub(super) fn default_base_dir() -> String {
    format!(
        "{}/amplihack-workstreams",
        std::env::temp_dir().to_string_lossy()
    )
}

/// Tail subprocess output to a log file and prefixed stdout.
pub(super) fn tail_output(
    stdout: impl std::io::Read,
    log_file: &Path,
    issue_id: i64,
    max_log_bytes: u64,
) {
    let reader = BufReader::new(stdout);
    let mut log_bytes_written: u64 = 0;

    // Open log file with 0o600 permissions on Unix, default permissions on Windows.
    let log_fd = {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        opts.mode(0o600);
        opts.open(log_file)
    };

    let mut log_writer = log_fd.ok();

    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line_bytes = line.len() as u64 + 1; // +1 for newline

        if log_bytes_written < max_log_bytes && log_bytes_written + line_bytes <= max_log_bytes {
            if let Some(ref mut w) = log_writer {
                let _ = writeln!(w, "{line}");
                let _ = w.flush();
            }
            log_bytes_written += line_bytes;
        }

        // Prefix output to stdout
        println!("[ws:{issue_id}] {line}");
    }
}

/// Atomically write data to a file (write to tmp, then rename).
pub(super) fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("tmp");
    let mut file = {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        opts.mode(0o600);
        opts.open(&tmp_path)?
    };
    file.write_all(data)?;
    file.flush()?;
    fs::rename(&tmp_path, path)?;
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
    fn test_atomic_write() {
        let dir = std::env::temp_dir().join("amplihack-test-atomic-write");
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("test.json");
        atomic_write(&file, b"hello world").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello world");
        let _ = fs::remove_dir_all(&dir);
    }
}
