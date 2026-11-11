//! Filesystem guarantees for recovery evidence, isolated from classification.
use super::*;
#[cfg(unix)]
use std::io::Read;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
    sha256: String,
}
pub(super) struct Capture {
    pub identity: Identity,
    pub bytes: Vec<u8>,
}
pub(super) fn capture(path: &Path) -> Result<Option<Capture>> {
    path_scope::check()?;
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink(),
        "refusing nonregular Codex recovery evidence {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        ensure!(
            meta.uid() == unsafe { libc::geteuid() } && meta.nlink() == 1,
            "refusing foreign or hardlinked recovery evidence {}",
            path.display()
        );
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let opened = file.metadata()?;
        ensure!(
            (opened.dev(), opened.ino()) == (meta.dev(), meta.ino()),
            "Codex recovery evidence replaced; record retained: {}",
            path.display()
        );
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let after = fs::symlink_metadata(path)?;
        ensure!(
            (
                after.dev(),
                after.ino(),
                after.len(),
                after.mtime(),
                after.mtime_nsec()
            ) == (
                opened.dev(),
                opened.ino(),
                opened.len(),
                opened.mtime(),
                opened.mtime_nsec()
            ),
            "Codex recovery evidence changed; record retained: {}",
            path.display()
        );
        Ok(Some(Capture {
            identity: Identity {
                device: opened.dev(),
                inode: opened.ino(),
                sha256: completion_proof::hash(&bytes),
            },
            bytes,
        }))
    }
    #[cfg(not(unix))]
    bail!(
        "safe Codex recovery archival is unavailable on this platform: {}",
        path.display()
    )
}
pub(super) fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    path_scope::check()?;
    let parent = path.parent().context("recovery evidence parent missing")?;
    #[cfg(test)]
    storage::testing::check_write()?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    path_scope::check()?;
    file.persist_noclobber(path)
        .context("cannot exclusively publish recovery archive intent")?;
    sync_path(path)?;
    sync_path(parent)
}
/// A single no-replace rename, never an overwriting rename or copy/unlink.
pub(super) fn rename(source: &Path, destination: &Path) -> Result<()> {
    path_scope::check()?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let from = CString::new(source.as_os_str().as_bytes())?;
        let to = CString::new(destination.as_os_str().as_bytes())?;
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = destination;
        bail!(
            "atomic no-replace Codex archival is unavailable: {}",
            source.display()
        )
    }
}
