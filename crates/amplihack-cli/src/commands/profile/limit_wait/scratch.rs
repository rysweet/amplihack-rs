//! Scratch and stdin of a supervised step (issue #1618): the home scratch
//! directory `$HOME/.amplihack/rerun/`, and what each attempt reads on
//! stdin.
//!
//! Nothing here reads stdin, so a pipe the step inherited and nobody closes
//! cannot hold the supervisor up. The first attempt inherits stdin
//! unchanged. A rerun gets a regular file again from where the step started
//! reading it, a terminal unchanged, and anything else (a pipe, a socket, a
//! closed descriptor) `/dev/null`: what the first attempt read from it is
//! gone. Recipe agent steps get their prompt as `-p` argv, so stdin is not
//! their prompt.

use super::super::fsx;
use std::fs;
use std::io::{self, Seek, SeekFrom};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// `$HOME/.amplihack/rerun`, checked or created: `$HOME/.amplihack` must be
/// a real directory owned by the current user that group and others cannot
/// write (created at `0700` when missing, never `chmod`ed), and `rerun/`
/// must be a private (`0700`) directory owned by the current user.
pub(crate) fn home_rerun_dir(home: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
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

/// What a rerun reads on stdin.
#[derive(Debug)]
pub(crate) enum RerunStdin {
    /// A terminal, or the platform cannot tell: passed through unchanged.
    Inherit,
    /// A regular file: a duplicate of the step's descriptor and the offset
    /// the step started at. Seeking it moves the shared offset back.
    File { file: fs::File, start: u64 },
    /// A pipe, socket or closed descriptor: `/dev/null`.
    Null,
}

impl RerunStdin {
    /// Looks at this process's stdin without reading it.
    pub(crate) fn capture() -> Self {
        let stdin = io::stdin();
        let Ok(owned) = stdin.as_fd().try_clone_to_owned() else {
            return RerunStdin::Null; // closed
        };
        Self::from_file(fs::File::from(owned))
    }

    /// The policy for `file`, a duplicate of the step's stdin.
    pub(crate) fn from_file(mut file: fs::File) -> Self {
        let Ok(meta) = file.metadata() else {
            return RerunStdin::Null;
        };
        if meta.is_file() {
            return match file.stream_position() {
                Ok(start) => RerunStdin::File { file, start },
                Err(_) => RerunStdin::Null,
            };
        }
        use std::os::unix::fs::FileTypeExt;
        if meta.file_type().is_char_device() {
            // A terminal, or /dev/null itself: never waits for input that
            // a held-open pipe would withhold.
            return RerunStdin::Inherit;
        }
        RerunStdin::Null
    }

    /// The stdin for attempt `n` (from 1).
    pub(crate) fn for_attempt(&self, n: u32) -> io::Result<Stdio> {
        if n == 1 {
            return Ok(Stdio::inherit());
        }
        match self {
            RerunStdin::Inherit => Ok(Stdio::inherit()),
            RerunStdin::File { file, start } => {
                let mut f = file.try_clone()?;
                f.seek(SeekFrom::Start(*start))?;
                Ok(Stdio::from(f))
            }
            RerunStdin::Null => Ok(Stdio::null()),
        }
    }
}
