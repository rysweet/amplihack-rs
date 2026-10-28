//! `<session>.meta.json`: what a detached run records about who started it
//! (issue #1710), plus the private-file readers every workstreams module uses.
//!
//! The meta file is written by `recipe run --detach` beside the run's log. It
//! is display data only: `workstreams status` uses it to list the runs a
//! session started, and nothing ever decides access, stops, kills or merges
//! from it.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The only schema this version writes and reads.
pub(crate) const SCHEMA: u32 = 1;
/// A meta file larger than this is not read.
const MAX_META_BYTES: u64 = 16 * 1024;
const MAX_ID_LEN: usize = 128;
const MAX_BRANCH_BYTES: usize = 255;
const MAX_CWD_BYTES: usize = 4096;

/// One detached run's parentage and identity. Display only (see the module
/// doc): never an authority for stopping, killing, merging or reading.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RunMeta {
    pub schema: u32,
    pub session: String,
    /// `$AMPLIHACK_SESSION_ID` of the launcher, when set and safe.
    pub parent_session_id: Option<String>,
    /// `$AMPLIHACK_DETACHED_SESSION` of the launcher: the detached run that
    /// started this one.
    pub parent_run: Option<String>,
    pub recipe: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub issue_number: Option<u64>,
    pub pr_number: Option<u64>,
}

/// `id` if it is 1 to 128 characters of `[A-Za-z0-9._:-]`, else `None`.
pub(crate) fn sanitize_id(id: &str) -> Option<String> {
    let ok = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'));
    ok.then(|| id.to_string())
}

/// A branch name of at most 255 bytes with no control character.
pub(crate) fn sanitize_branch(branch: &str) -> Option<String> {
    let ok = !branch.is_empty()
        && branch.len() <= MAX_BRANCH_BYTES
        && !branch.chars().any(char::is_control);
    ok.then(|| branch.to_string())
}

/// Write `meta` to `path` with mode 0600: a new temporary file beside it
/// (never an existing file or a planted symlink), synced, then renamed over
/// `path`. A symlink at `path` is replaced, never followed.
pub(crate) fn write_atomic(path: &Path, meta: &RunMeta) -> Result<()> {
    let mut meta = meta.clone();
    meta.cwd = meta.cwd.filter(|c| c.as_os_str().len() <= MAX_CWD_BYTES);
    let body = serde_json::to_vec(&meta).context("cannot serialise the run meta")?;
    let name = path
        .file_name()
        .context("the meta path has no file name")?
        .to_string_lossy()
        .into_owned();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| -> Result<()> {
        let mut file = crate::commands::recipe::detach::create_private(&tmp)?;
        file.write_all(&body)?;
        file.sync_all()?;
        fs::rename(&tmp, path).with_context(|| format!("cannot rename into {}", path.display()))
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// The meta file at `path`, when it is a private regular file owned by this
/// user, at most 16 KiB, and schema 1. Anything else reads as no meta.
pub(crate) fn read(path: &Path) -> Option<RunMeta> {
    let body = read_private(path, MAX_META_BYTES)?;
    let meta: RunMeta = serde_json::from_slice(&body).ok()?;
    (meta.schema == SCHEMA).then_some(meta)
}

/// Open `path` without following a final symlink and without blocking on a
/// FIFO, and check it is a regular file owned by this user with no group or
/// world write bit.
pub(crate) fn open_private(path: &Path) -> Option<(File, fs::Metadata)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    let private = meta.is_file() && meta.uid() == current_uid() && meta.mode() & 0o022 == 0;
    private.then_some((file, meta))
}

/// The contents of a private file (see [`open_private`]) of at most `cap`
/// bytes; `None` for anything else, including a larger file.
pub(crate) fn read_private(path: &Path, cap: u64) -> Option<Vec<u8>> {
    let (file, meta) = open_private(path)?;
    if meta.len() > cap {
        return None;
    }
    let mut body = Vec::new();
    file.take(cap + 1).read_to_end(&mut body).ok()?;
    (body.len() as u64 <= cap).then_some(body)
}

/// How a directory a reader is about to trust looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirCheck {
    Missing,
    /// A symlink, not a directory, not owned by this user, or writable by
    /// group or others.
    Unsafe,
    Private,
}

/// Check `dir` without following a symlink and without changing it.
pub(crate) fn check_dir(dir: &Path) -> DirCheck {
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DirCheck::Missing,
        Err(_) => DirCheck::Unsafe,
        Ok(m) if m.is_dir() && m.uid() == current_uid() && m.mode() & 0o022 == 0 => {
            DirCheck::Private
        }
        Ok(_) => DirCheck::Unsafe,
    }
}

pub(crate) fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}
