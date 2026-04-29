//! Per-binary install version stamp.
//!
//! Records the amplihack binary version that last successfully staged
//! framework assets into `~/.amplihack`. Read on startup by the
//! [`crate::self_heal`] module to detect drift between the running binary
//! and the staged assets, and re-stage automatically when they disagree.
//!
//! The stamp file lives at `~/.amplihack/.installed-version` and contains
//! a single line — the value of [`crate::VERSION`] — with no trailing
//! newline. Writes are atomic via a sibling tempfile + `rename` (the same
//! pattern used by `write_layout_marker` in this crate).

use anyhow::{Context, Result};
use std::fs;
use std::io;
use std::path::PathBuf;

use super::paths::home_dir;

/// Filename of the stamp inside `~/.amplihack/`.
const STAMP_FILE: &str = ".installed-version";

/// Sibling tempfile used for atomic writes.
const STAMP_TMP: &str = ".installed-version.tmp";

/// Resolve the absolute path of the install version stamp file.
///
/// Returns `Err` if `HOME` is unset (matches the failure mode of every other
/// path helper in this crate; we do not silently fall back to the current
/// directory).
pub(crate) fn installed_version_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack").join(STAMP_FILE))
}

/// Atomically write the stamp file with `version` as its sole contents.
///
/// Creates `~/.amplihack/` (with parents) if missing. Writes to a sibling
/// `.installed-version.tmp` first and then renames into place, so a crashed
/// or interrupted write never produces a torn read.
pub(crate) fn write_installed_version(version: &str) -> Result<()> {
    let final_path = installed_version_path()?;
    let parent = final_path.parent().with_context(|| {
        format!(
            "version stamp path has no parent directory: {}",
            final_path.display()
        )
    })?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let tmp_path = parent.join(STAMP_TMP);
    fs::write(&tmp_path, version.as_bytes())
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &final_path).with_context(|| {
        format!(
            "failed to rename {} to {}",
            tmp_path.display(),
            final_path.display()
        )
    })?;
    Ok(())
}

/// Read the stamp file, returning `Ok(None)` when it does not exist.
///
/// Trims surrounding whitespace from the contents (defensive — manual edits
/// or pre-existing CRLF endings should not cause false mismatches). Returns
/// `Err` for any IO error other than `NotFound` so corruption fails loud.
pub(crate) fn read_installed_version() -> Result<Option<String>> {
    let path = installed_version_path()?;
    match fs::read_to_string(&path) {
        Ok(contents) => Ok(Some(contents.trim().to_string())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use tempfile::TempDir;

    /// Save/restore guard for `HOME`. Drops always restore, even on panic.
    /// Uses the crate-wide [`crate::self_heal::test_support::ENV_LOCK`] so
    /// that `version_stamp` and `self_heal` tests cannot race each other —
    /// both mutate the process-wide `HOME` env var.
    struct HomeGuard {
        prior: Option<OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn set(home: &std::path::Path) -> Self {
            let lock = crate::test_support::env_lock()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let prior = std::env::var_os("HOME");
            // SAFETY: edition 2024 requires unsafe for env mutation;
            // serialized via the crate-wide env_lock above.
            unsafe { std::env::set_var("HOME", home) };
            HomeGuard { prior, _lock: lock }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: serialized via ENV_LOCK held in self._lock.
            unsafe {
                match self.prior.take() {
                    Some(prev) => std::env::set_var("HOME", prev),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    #[test]
    fn write_then_read_round_trip() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        write_installed_version("0.8.111").expect("write");
        let read = read_installed_version().expect("read");
        assert_eq!(read.as_deref(), Some("0.8.111"));
    }

    #[test]
    fn read_returns_none_when_file_missing() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        let read = read_installed_version().expect("read");
        assert_eq!(read, None);
    }

    #[test]
    fn write_overwrites_existing_stamp() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        write_installed_version("0.8.110").expect("first write");
        write_installed_version("0.8.111").expect("second write");
        let read = read_installed_version().expect("read");
        assert_eq!(read.as_deref(), Some("0.8.111"));
    }

    #[test]
    fn write_creates_amplihack_dir_if_missing() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());
        // Note: ~/.amplihack does not exist yet inside the tempdir.
        assert!(!tmp.path().join(".amplihack").exists());

        write_installed_version("0.9.0").expect("write");
        assert!(tmp.path().join(".amplihack").exists());
        assert!(tmp.path().join(".amplihack").join(STAMP_FILE).exists());
    }

    #[test]
    fn write_contents_have_no_trailing_newline() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        write_installed_version("0.8.111").expect("write");
        let raw = fs::read(installed_version_path().unwrap()).unwrap();
        assert_eq!(raw, b"0.8.111");
    }

    #[test]
    fn read_trims_whitespace_for_compat() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        let path = installed_version_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "0.8.111\n").unwrap();

        let read = read_installed_version().expect("read");
        assert_eq!(read.as_deref(), Some("0.8.111"));
    }

    #[test]
    fn installed_version_path_under_home() {
        let tmp = TempDir::new().expect("tempdir");
        let _g = HomeGuard::set(tmp.path());

        let path = installed_version_path().expect("path");
        assert_eq!(path, tmp.path().join(".amplihack").join(STAMP_FILE));
    }
}
