//! Cross-platform, persistent files for coordinating Codex filesystem changes.
use super::*;
use std::fs::OpenOptions;

pub(super) fn open_package_lock(root: &Path) -> Result<fs::File> {
    let parent = root.parent().context("Codex staging root has no parent")?;
    let mut name = root
        .file_name()
        .context("Codex staging root has no name")?
        .to_os_string();
    name.push(".install.lock");
    fs::create_dir_all(parent)?;
    path_scope::check()?;
    // Keep coordination outside the package so rejecting foreign ownership
    // never changes its contents, and removing the package cannot unlink a lock.
    open_lock_file(&parent.join(name))
}

pub(super) fn open_lock_file(path: &Path) -> Result<fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "refusing nonregular Codex lock file {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.mode() & libc::S_IFMT == libc::S_IFREG,
            "refusing nonregular Codex lock file {}",
            path.display()
        );
        ensure!(
            metadata.nlink() == 1,
            "refusing hardlinked Codex lock file {}",
            path.display()
        );
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        ensure!(
            !metadata.file_type().is_symlink(),
            "refusing symlink Codex lock file {}",
            path.display()
        );
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_file_is_a_regular_persistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codex.lock");
        let lock = open_lock_file(&path).unwrap();
        lock.lock_exclusive().unwrap();
        drop(lock);

        assert!(fs::symlink_metadata(path).unwrap().is_file());
    }

    #[cfg(unix)]
    #[test]
    fn lock_file_rejects_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"untouched").unwrap();
        let link = dir.path().join("codex.lock");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(open_lock_file(&link).is_err());
        assert_eq!(fs::read(target).unwrap(), b"untouched");
    }

    #[cfg(unix)]
    #[test]
    fn hardlinked_lock_preserves_foreign_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("foreign");
        fs::write(&target, b"untouched").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let lock = dir.path().join("codex.lock");
        fs::hard_link(&target, &lock).unwrap();

        assert!(open_lock_file(&lock).is_err());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(fs::read(target).unwrap(), b"untouched");
    }
}
