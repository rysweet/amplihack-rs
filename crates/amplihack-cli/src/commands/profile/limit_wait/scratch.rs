//! Scratch files of a supervised step (issue #1618): the home scratch
//! directory `$HOME/.amplihack/rerun/` and the copy of the step's stdin.
//!
//! The stdin copy lets every attempt read the same prompt from the start.
//! Its device and inode are recorded, and each attempt reopens it read-only
//! and no-follow and is refused anything else, so a swapped file is never
//! fed to an attempt. No descriptor is shared between attempts. Dropping the
//! copy deletes the file, on every exit path of the supervisor.

use super::super::fsx;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// `$HOME/.amplihack/rerun`, checked or created. `$HOME/.amplihack` must be
/// a real directory owned by the current user that others cannot write; it
/// is created at `0700` when missing and never `chmod`ed. `rerun/` must be a
/// private (`0700`) directory owned by the current user.
pub(crate) fn home_rerun_dir(home: &Path) -> io::Result<PathBuf> {
    let dot = home.join(".amplihack");
    match fs::symlink_metadata(&dot) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => fsx::ensure_private_dir(&dot)?,
        Err(e) => return Err(e),
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(fsx::invalid("~/.amplihack is not a real directory"));
        }
        Ok(m) if m.uid() != fsx::euid() => {
            return Err(fsx::invalid("~/.amplihack is owned by another user"));
        }
        Ok(m) if fsx::writable_by_others(&m) => {
            return Err(fsx::invalid("~/.amplihack is writable by other users"));
        }
        Ok(_) => {}
    }
    let rerun = dot.join("rerun");
    fsx::ensure_private_dir(&rerun)?;
    Ok(rerun)
}

/// Why the stdin copy could not be made.
#[derive(Debug)]
pub(crate) enum CopyError {
    /// Nothing was read: the launch can still run unsupervised.
    BeforeRead(io::Error),
    /// Input was consumed: an unsupervised run would get a truncated prompt.
    AfterRead(io::Error),
}

impl std::fmt::Display for CopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyError::BeforeRead(e) | CopyError::AfterRead(e) => e.fmt(f),
        }
    }
}

/// The step's stdin, copied to `rerun/<pid>-0.in`, or inherited when it is
/// a terminal or closed.
pub(crate) struct StdinCopy {
    file: Option<Copied>,
}

struct Copied {
    path: PathBuf,
    dev: u64,
    ino: u64,
}

impl StdinCopy {
    /// Copies this process's stdin into `dir`.
    pub(crate) fn take(dir: &Path) -> Result<Self, CopyError> {
        // SAFETY: isatty and fcntl(F_GETFD) only inspect descriptor 0.
        let (tty, open) = unsafe { (libc::isatty(0) == 1, libc::fcntl(0, libc::F_GETFD) != -1) };
        StdinCopy::take_from(dir, io::stdin().lock(), tty || !open)
    }

    /// Copies `reader` into `dir`, or keeps nothing when `inherit`.
    pub(crate) fn take_from<R: Read>(
        dir: &Path,
        mut reader: R,
        inherit: bool,
    ) -> Result<Self, CopyError> {
        if inherit {
            return Ok(Self { file: None });
        }
        let path = dir.join(format!("{}-0.in", std::process::id()));
        let mut opts = fs::OpenOptions::new();
        opts.write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let mut file = opts.open(&path).map_err(CopyError::BeforeRead)?;
        let made = (|| {
            // `mode` on open is filtered by the umask; set it exactly.
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            let meta = file.metadata()?;
            Ok::<_, io::Error>((meta.dev(), meta.ino()))
        })();
        let (dev, ino) = match made {
            Ok(id) => id,
            Err(e) => {
                let _ = fs::remove_file(&path);
                return Err(CopyError::BeforeRead(e));
            }
        };
        // From here on the copy is owned, so any failure removes the file.
        let copy = Self {
            file: Some(Copied { path, dev, ino }),
        };
        // No fsync: attempts read the copy through the page cache, and the
        // file is deleted when the supervisor ends, so durability buys nothing.
        io::copy(&mut reader, &mut file).map_err(CopyError::AfterRead)?;
        Ok(copy)
    }

    /// The copy's path; `None` when stdin is inherited.
    pub(crate) fn path(&self) -> Option<&Path> {
        self.file.as_ref().map(|c| c.path.as_path())
    }

    /// A fresh read-only handle on the copy for one attempt, refused unless
    /// it is the same regular file that was written.
    pub(crate) fn stdio(&self) -> io::Result<Stdio> {
        let Some(c) = &self.file else {
            return Ok(Stdio::inherit());
        };
        let mut opts = fs::OpenOptions::new();
        opts.read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let f = opts.open(&c.path)?;
        let m = f.metadata()?;
        if !m.is_file() || m.dev() != c.dev || m.ino() != c.ino {
            return Err(fsx::invalid("the stdin copy was replaced"));
        }
        Ok(Stdio::from(f))
    }
}

impl Drop for StdinCopy {
    fn drop(&mut self) {
        if let Some(c) = &self.file {
            let _ = fs::remove_file(&c.path);
        }
    }
}
