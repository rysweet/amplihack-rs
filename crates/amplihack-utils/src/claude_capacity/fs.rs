//! Bounded, fail-closed small-file reads for the capacity reader and the
//! dedupe records. Off Unix every read is refused, so nothing is ever shown.

pub(super) use imp::*;

#[cfg(unix)]
mod imp {
    use crate::claude_capacity::{MAX_SMALL_FILE, Read};
    use std::fs;
    use std::io::{self, Read as _};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;

    pub(in crate::claude_capacity) fn euid() -> u32 {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() }
    }

    /// Opens `path` without following a final symlink or blocking on a FIFO,
    /// and checks on the open descriptor that it is a regular file, owned by
    /// the current user when `owned`.
    fn open_regular(path: &Path, owned: bool) -> Result<Option<(fs::File, fs::Metadata)>, ()> {
        let mut opts = fs::OpenOptions::new();
        opts.read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
        let file = match opts.open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                tracing::debug!(op = "claude_capacity open", kind = ?e.kind());
                return Err(());
            }
        };
        let meta = file.metadata().map_err(|_| ())?;
        if !meta.is_file() || (owned && meta.uid() != euid()) {
            return Err(());
        }
        Ok(Some((file, meta)))
    }

    /// At most `limit` bytes of an owned regular file; `None` when it is
    /// missing, refused, or larger than `limit`.
    pub(in crate::claude_capacity) fn read_owned(path: &Path, limit: u64) -> Option<Vec<u8>> {
        let (file, _) = open_regular(path, true).ok()??;
        let mut buf = Vec::new();
        file.take(limit + 1).read_to_end(&mut buf).ok()?;
        (buf.len() as u64 <= limit).then_some(buf)
    }

    /// The saved-root or toggle file. `toggle` files are refused unless they
    /// are the current user's; saved-root files are refused when group or
    /// others can write them (the rule `root_config::read_limited` applies).
    pub(in crate::claude_capacity) fn read_small(path: &Path, toggle: bool) -> Read {
        let (file, meta) = match open_regular(path, toggle) {
            Ok(Some(v)) => v,
            Ok(None) => return Read::Missing,
            Err(()) => return Read::Refused,
        };
        if !toggle && meta.mode() & 0o022 != 0 {
            return Read::Refused;
        }
        let mut buf = Vec::new();
        match file.take(MAX_SMALL_FILE + 1).read_to_end(&mut buf) {
            Ok(_) => Read::Bytes(buf),
            Err(_) => Read::Refused,
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use crate::claude_capacity::Read;
    use std::path::Path;

    pub(in crate::claude_capacity) fn read_owned(_: &Path, _: u64) -> Option<Vec<u8>> {
        None
    }

    pub(in crate::claude_capacity) fn read_small(_: &Path, _: bool) -> Read {
        Read::Refused
    }
}
