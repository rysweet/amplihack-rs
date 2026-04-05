//! Secure file and directory creation with restrictive permissions.
//!
//! Provides helpers that create files and directories with owner-only
//! access (`0o700` for dirs, `0o600` for files) on Unix systems.

use std::fs;
use std::io;
use std::path::Path;

/// Create a directory (and parents) with owner-only permissions.
///
/// On Unix the directory mode is set to `0o700`. On non-Unix platforms
/// the directory is created with default permissions.
pub fn ensure_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o700);
        fs::set_permissions(path, perms)?;
    }

    Ok(())
}

/// Open a file for appending with owner-only permissions.
///
/// On Unix the file is created with mode `0o600` using `O_APPEND | O_CREAT | O_WRONLY`.
/// On non-Unix platforms the file is created with default permissions.
pub fn open_private_append(path: &Path) -> io::Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
    }

    #[cfg(not(unix))]
    {
        fs::OpenOptions::new().create(true).append(true).open(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn ensure_private_directory_creates_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("secure").join("nested");

        ensure_private_directory(&target).unwrap();
        assert!(target.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_directory_sets_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("private_dir");

        ensure_private_directory(&target).unwrap();

        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "directory should be 0o700, got {mode:o}");
    }

    #[test]
    fn ensure_private_directory_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("idem");

        ensure_private_directory(&target).unwrap();
        ensure_private_directory(&target).unwrap();
        assert!(target.is_dir());
    }

    #[test]
    fn open_private_append_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("data").join("log.txt");

        let mut f = open_private_append(&target).unwrap();
        writeln!(f, "line 1").unwrap();
        drop(f);

        assert!(target.exists());
        let content = fs::read_to_string(&target).unwrap();
        assert!(content.contains("line 1"));
    }

    #[test]
    fn open_private_append_actually_appends() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("append.log");

        let mut f = open_private_append(&target).unwrap();
        writeln!(f, "first").unwrap();
        drop(f);

        let mut f = open_private_append(&target).unwrap();
        writeln!(f, "second").unwrap();
        drop(f);

        let content = fs::read_to_string(&target).unwrap();
        assert!(content.contains("first"));
        assert!(content.contains("second"));
    }

    #[cfg(unix)]
    #[test]
    fn open_private_append_sets_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("secure.log");

        let f = open_private_append(&target).unwrap();
        drop(f);

        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "file should be 0o600, got {mode:o}");
    }
}
