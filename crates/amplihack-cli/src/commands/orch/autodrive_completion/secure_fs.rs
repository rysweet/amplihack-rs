//! Descriptor-relative, no-follow access. Private state is integrity protection
//! against other users, not a sandbox against arbitrary same-user execution.
use anyhow::Result;
#[cfg(unix)]
use anyhow::{Context, ensure};
#[cfg(unix)]
use std::io::{Read, Write};
use std::path::{Component, Path};

/// Where the completion checker may repair entries (#2546).
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    /// No repair anywhere: entries with group or other bits are refused.
    Strict,
    /// Repair only at or below this state root, compared part by part.
    StateTree(&'a Path),
}

impl<'a> Scope<'a> {
    /// The scope for a drive's state folder: `StateTree` when the folder is
    /// absolute, has no `..` part, already equals its canonical form
    /// and lies directly under the auto-drive state root (`in_state_root`);
    /// otherwise `Strict`, including for a folder that does not exist.
    /// Canonicalisation only decides eligibility: `StateTree` keeps the raw
    /// path, and every access still goes through the no-follow walk.
    pub fn of(state_dir: &'a Path) -> Self {
        if state_dir.is_absolute()
            && !state_dir
                .components()
                .any(|c| matches!(c, Component::ParentDir))
            && state_dir.canonicalize().is_ok_and(|canonical| {
                canonical == state_dir
                    && crate::commands::orch::autodrive::private_mode::in_state_root(&canonical)
            })
        {
            Scope::StateTree(state_dir)
        } else {
            Scope::Strict
        }
    }

    /// Whether `at` (an absolute path with no `..` part) may be repaired.
    #[cfg(unix)]
    fn repair(self, at: &Path) -> crate::commands::orch::autodrive::private_mode::Repair {
        use crate::commands::orch::autodrive::private_mode::Repair;
        match self {
            Scope::StateTree(root) if at.starts_with(root) => Repair::Allowed,
            _ => Repair::Refuse,
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use crate::commands::orch::autodrive::private_mode::{Kind, Repair, make_private};
    use std::ffi::CString;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};

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

    fn private(file: &File, path: &Path, kind: Kind, repair: Repair) -> Result<()> {
        // Same rule as the evidence store (#2546).
        make_private(file, path, kind, repair)
            .context("private state ownership, permissions or file type are unsafe")
    }

    /// Opens the parent folder of `path` without following links.
    ///
    /// The folders checked are the last parent, the folder directly above the
    /// first `.completion-*` part, and every folder below that part. Each is
    /// repaired only when `scope` allows it (at or below the state root);
    /// folders above the root are checked strictly. Returns the folder and
    /// how the target entry may be repaired: `Repair::Allowed` where `scope`
    /// allows it, and `Repair::PrivateFolder` otherwise. The returned folder
    /// has always passed `make_private(.., Kind::Dir, ..)` above, and callers
    /// open the target as its single file name with `openat(O_NOFOLLOW)` on
    /// that descriptor, which is what `PrivateFolder` requires (#2546).
    fn parent(scope: Scope<'_>, path: &Path) -> Result<(File, Repair)> {
        ensure!(path.is_absolute(), "private state path must be absolute");
        let mut dir = File::open("/")?;
        let mut at = std::path::PathBuf::from("/");
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
                    if !in_private_tree && name.to_string_lossy().starts_with(".completion-") {
                        private(&dir, &at, Kind::Dir, scope.repair(&at))?;
                        in_private_tree = true;
                    }
                    dir = open_at(&dir, name, libc::O_RDONLY | libc::O_DIRECTORY)?;
                    at.push(name);
                    if in_private_tree {
                        private(&dir, &at, Kind::Dir, scope.repair(&at))?;
                    }
                }
                _ => anyhow::bail!("unsafe private state directory chain"),
            }
        }
        if !in_private_tree {
            private(&dir, &at, Kind::Dir, scope.repair(&at))?;
        }
        let repair = match scope.repair(&at) {
            Repair::Allowed => Repair::Allowed,
            Repair::PrivateFolder | Repair::Refuse => Repair::PrivateFolder,
        };
        Ok((dir, repair))
    }

    /// Checks the state folder itself, repairing it only when it lies directly
    /// under the auto-drive state root (`Scope::of`).
    pub fn check_dir(root: &Path) -> Result<()> {
        parent(Scope::of(root), &root.join(".directory-check"))?;
        Ok(())
    }

    pub fn output(scope: Scope<'_>, path: &Path) -> Result<File> {
        let (dir, _) = parent(scope, path)?;
        open_at(
            &dir,
            path.file_name().context("missing filename")?,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )
    }

    pub fn read(scope: Scope<'_>, path: &Path, limit: usize) -> Result<Vec<u8>> {
        read_file(scope, path, limit, None)
    }

    /// Like [`read`] with `Scope::Strict`, but the file itself is never
    /// repaired either: step recovery refuses any unsafe record (#2539).
    pub fn read_strict(path: &Path, limit: usize) -> Result<Vec<u8>> {
        read_file(Scope::Strict, path, limit, Some(Repair::Refuse))
    }

    /// Like [`read_strict`], but a file that does not exist reads as `None`.
    pub fn read_optional(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                check_dir(path.parent().context("missing parent")?)?;
                Ok(None)
            }
            _ => read_strict(path, limit).map(Some),
        }
    }

    /// Appends `bytes` with one write, creating the file at 0600 if needed.
    /// The folder and the file must be private; neither is repaired (#2539).
    pub fn append(path: &Path, bytes: &[u8]) -> Result<()> {
        let (dir, _) = parent(Scope::Strict, path)?;
        let mut file = open_at(
            &dir,
            path.file_name().context("missing filename")?,
            libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT,
        )?;
        private(&file, path, Kind::File, Repair::Refuse)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }

    fn read_file(
        scope: Scope<'_>,
        path: &Path,
        limit: usize,
        file_repair: Option<Repair>,
    ) -> Result<Vec<u8>> {
        let (dir, repair) = parent(scope, path)?;
        let repair = file_repair.unwrap_or(repair);
        let file = open_at(
            &dir,
            path.file_name().context("missing filename")?,
            libc::O_RDONLY | libc::O_NONBLOCK,
        )?;
        private(&file, path, Kind::File, repair)?;
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

    pub fn create(scope: Scope<'_>, path: &Path, bytes: &[u8]) -> Result<()> {
        let (dir, _) = parent(scope, path)?;
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
pub use platform::{append, check_dir, create, output, read, read_optional, read_strict};
#[cfg(not(unix))]
pub fn read(_: Scope<'_>, _: &Path, _: usize) -> Result<Vec<u8>> {
    anyhow::bail!("completion dispatch requires Unix private-state support")
}
#[cfg(not(unix))]
pub fn create(_: Scope<'_>, _: &Path, _: &[u8]) -> Result<()> {
    anyhow::bail!("completion dispatch requires Unix private-state support")
}

#[cfg(not(unix))]
pub fn check_dir(_: &Path) -> Result<()> {
    anyhow::bail!("completion dispatch requires Unix private-state support")
}
#[cfg(not(unix))]
pub fn output(_: Scope<'_>, _: &Path) -> Result<std::fs::File> {
    anyhow::bail!("completion dispatch requires Unix private-state support")
}
#[cfg(not(unix))]
pub fn read_strict(_: &Path, _: usize) -> Result<Vec<u8>> {
    anyhow::bail!("step recovery requires Unix private-state support")
}
#[cfg(not(unix))]
pub fn read_optional(_: &Path, _: usize) -> Result<Option<Vec<u8>>> {
    anyhow::bail!("step recovery requires Unix private-state support")
}
#[cfg(not(unix))]
pub fn append(_: &Path, _: &[u8]) -> Result<()> {
    anyhow::bail!("step recovery requires Unix private-state support")
}
