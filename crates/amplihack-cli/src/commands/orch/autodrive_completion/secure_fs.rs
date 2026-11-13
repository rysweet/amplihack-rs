//! Descriptor-relative, no-follow access. Private state is integrity protection
//! against other users, not a sandbox against arbitrary same-user execution.
use anyhow::Result;
#[cfg(unix)]
use anyhow::{Context, ensure};
#[cfg(unix)]
use std::io::{Read, Write};
use std::path::Path;

#[cfg(unix)]
mod platform {
    use super::*;
    use std::ffi::CString;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    fn open_at(dir: &File, name: &std::ffi::OsStr, flags: i32) -> Result<File> {
        use std::os::unix::ffi::OsStrExt;
        let name = CString::new(name.as_bytes())?;
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        ensure!(
            fd >= 0,
            "private state open refused: {}",
            std::io::Error::last_os_error()
        );
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn private(file: &File, directory: bool) -> Result<()> {
        let m = file.metadata()?;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
            "private state ownership or permissions are unsafe"
        );
        ensure!(
            if directory { m.is_dir() } else { m.is_file() },
            "private state has unsafe file type"
        );
        Ok(())
    }

    fn parent(path: &Path) -> Result<File> {
        ensure!(path.is_absolute(), "private state path must be absolute");
        let mut dir = File::open("/")?;
        let components: Vec<_> = path
            .parent()
            .context("missing parent")?
            .components()
            .collect();
        let mut in_private_tree = false;
        for c in components {
            match c {
                Component::RootDir => (),
                Component::Normal(name) => {
                    if name.to_string_lossy().starts_with(".completion-") {
                        private(&dir, true)?;
                        in_private_tree = true;
                    }
                    dir = open_at(&dir, name, libc::O_RDONLY | libc::O_DIRECTORY)?;
                    if in_private_tree {
                        private(&dir, true)?;
                    }
                }
                _ => anyhow::bail!("unsafe private state directory chain"),
            }
        }
        private(&dir, true)?;
        Ok(dir)
    }

    pub fn check_dir(path: &Path) -> Result<()> {
        parent(&path.join(".directory-check"))?;
        Ok(())
    }

    pub fn output(path: &Path) -> Result<File> {
        let dir = parent(path)?;
        open_at(
            &dir,
            path.file_name().context("missing filename")?,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )
    }

    pub fn read(path: &Path, limit: usize) -> Result<Vec<u8>> {
        let dir = parent(path)?;
        let file = open_at(
            &dir,
            path.file_name().context("missing filename")?,
            libc::O_RDONLY | libc::O_NONBLOCK,
        )?;
        private(&file, false)?;
        ensure!(
            file.metadata()?.len() <= limit as u64,
            "private state file exceeds {limit} bytes"
        );
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= limit,
            "private state file exceeds {limit} bytes"
        );
        Ok(bytes)
    }

    /// Appends `bytes` with one write, creating the file at 0600 if needed.
    /// The file must be private and a regular file; it is never repaired.
    pub fn append(path: &Path, bytes: &[u8]) -> Result<()> {
        let dir = parent(path)?;
        let mut file = open_at(
            &dir,
            path.file_name().context("missing filename")?,
            libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT,
        )?;
        private(&file, false)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }

    /// Like [`read`], but a file that does not exist reads as `None`.
    pub fn read_optional(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                check_dir(path.parent().context("missing parent")?)?;
                Ok(None)
            }
            _ => read(path, limit).map(Some),
        }
    }

    pub fn create(path: &Path, bytes: &[u8]) -> Result<()> {
        let dir = parent(path)?;
        // Create an unnamed-to-the-agent temporary file, then publish with a
        // hard link: linkat is atomic and never replaces an existing target.
        let name = format!(".decision-{}", uuid::Uuid::new_v4());
        let mut file = open_at(
            &dir,
            name.as_ref(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            let from = CString::new(name.clone())?;
            use std::os::unix::ffi::OsStrExt;
            let to = CString::new(path.file_name().context("missing filename")?.as_bytes())?;
            ensure!(
                unsafe {
                    libc::linkat(
                        dir.as_raw_fd(),
                        from.as_ptr(),
                        dir.as_raw_fd(),
                        to.as_ptr(),
                        0,
                    )
                } == 0,
                "decision atomic create refused: {}",
                std::io::Error::last_os_error()
            );
            dir.sync_all()?;
            Ok(())
        })();
        let name = CString::new(name)?;
        ensure!(
            unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } == 0,
            "could not remove private temporary file: {}",
            std::io::Error::last_os_error()
        );
        result
    }
}

#[cfg(unix)]
pub use platform::{append, check_dir, create, output, read, read_optional};
#[cfg(not(unix))]
pub fn read(_: &Path, _: usize) -> Result<Vec<u8>> {
    anyhow::bail!("completion dispatch requires Unix private-state support")
}
#[cfg(not(unix))]
pub fn create(_: &Path, _: &[u8]) -> Result<()> {
    anyhow::bail!("completion dispatch requires Unix private-state support")
}

#[cfg(not(unix))]
pub fn append(_: &Path, _: &[u8]) -> Result<()> {
    anyhow::bail!("step recovery requires Unix private-state support")
}
#[cfg(not(unix))]
pub fn read_optional(_: &Path, _: usize) -> Result<Option<Vec<u8>>> {
    anyhow::bail!("step recovery requires Unix private-state support")
}
#[cfg(not(unix))]
pub fn check_dir(_: &Path) -> Result<()> {
    anyhow::bail!("completion dispatch requires Unix private-state support")
}
#[cfg(not(unix))]
pub fn output(_: &Path) -> Result<std::fs::File> {
    anyhow::bail!("completion dispatch requires Unix private-state support")
}
