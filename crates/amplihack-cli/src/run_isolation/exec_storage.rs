//! Stable execution storage `<work root>/exec/<identity>/` (issues #2096,
//! #2493): who holds it, and when it may be reclaimed.
//!
//! An attempt claims the path by creating it exclusively (mode 0700) and
//! writing `claim.json`, which names the attempt's run folder. The path is
//! reclaimed, by the next attempt with the same identity or by
//! `amplihack cleanup`, only when all of these hold:
//! - it is a real directory, never a symlink, and it is still the same
//!   directory (device and inode) when it is moved aside to be removed;
//! - the run its claim names has ended. A directory with no claim is
//!   reclaimed only when it is empty: a claim stopped before its record was
//!   written;
//! - no worktree registered in its private `repository.git` lies outside it
//!   and still exists. In default-workflow, step 04 creates such worktrees,
//!   and removing their repository would break them.
//!
//! Claims and reclaims of one `exec/` folder hold an exclusive lock on it, so
//! a reclaim never moves aside a path that another attempt has just claimed.
//! This file stands alone: the caller supplies the "has this run ended" check.

use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub(crate) const CLAIM: &str = "claim.json";
const TOMBSTONE_PREFIX: &str = ".reclaim-";
const RECORD_MAX_BYTES: u64 = 64 * 1024;

/// Whether the run in a run folder has ended. An error means "cannot tell",
/// which never permits a removal.
pub(crate) type OwnerEnded = fn(&Path) -> Result<bool>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Nothing is at the path.
    Absent,
    /// The path may be removed, for this reason.
    Reclaimable(String),
    /// The path must stay, for this reason.
    Kept(String),
}

/// The result of [`claim`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    /// This attempt now holds the path.
    Claimed(PathBuf),
    /// An earlier attempt still holds it, for this reason.
    Held(String),
}

/// Whether a name in `exec/` is a folder an interrupted reclaim moved aside.
pub(crate) fn is_tombstone(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(TOMBSTONE_PREFIX))
}

/// What [`reclaim`] would do with `stable`, without changing anything.
pub(crate) fn inspect(stable: &Path, owner_ended: OwnerEnded) -> Result<Verdict> {
    Ok(inspect_identity(stable, owner_ended)?.0)
}

/// Claims `stable` for the run in `owner_run_dir`. When the path exists it
/// is reclaimed once, if [`inspect`] allows; otherwise `Held` says why. An
/// error while deciding is also `Held`, with the error as the reason: this
/// attempt then uses a fresh path, and the path stays for `amplihack cleanup`
/// to report.
pub(crate) fn claim(stable: &Path, owner_run_dir: &Path, owner_ended: OwnerEnded) -> Result<Claim> {
    let parent = private_parent(stable)?;
    let tombstone = {
        let _lock = lock(&parent)?;
        if create(stable, owner_run_dir)? {
            return Ok(Claim::Claimed(stable.to_path_buf()));
        }
        let tombstone = match move_aside(stable, owner_ended) {
            Ok((Verdict::Kept(reason), _)) => return Ok(Claim::Held(reason)),
            Ok((_, tombstone)) => tombstone,
            Err(error) => return Ok(Claim::Held(format!("{error:#}"))),
        };
        if !create(stable, owner_run_dir)? {
            return Ok(Claim::Held("another attempt claimed it first".to_string()));
        }
        tombstone
    };
    if let Some((tombstone, identity)) = tombstone {
        remove_tombstone(&tombstone, Some(identity))?;
    }
    Ok(Claim::Claimed(stable.to_path_buf()))
}

/// Removes `stable` when the rules in the module comment allow it, checking
/// them again first. `Reclaimable` means it was removed; `Kept` says why not.
pub(crate) fn reclaim(stable: &Path, owner_ended: OwnerEnded) -> Result<Verdict> {
    let parent = stable
        .parent()
        .context("execution storage path has no parent")?;
    let (verdict, tombstone) = {
        let _lock = lock(parent)?;
        move_aside(stable, owner_ended)?
    };
    if let Some((tombstone, identity)) = tombstone {
        remove_tombstone(&tombstone, Some(identity))?;
    }
    Ok(verdict)
}

/// Removes a folder an interrupted reclaim moved aside: only a real
/// directory, and, when `identity` is given, only that directory.
pub(crate) fn remove_tombstone(tombstone: &Path, identity: Option<(u64, u64)>) -> Result<()> {
    let metadata = fs::symlink_metadata(tombstone)
        .with_context(|| format!("cannot inspect {}", tombstone.display()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || identity.is_some_and(|identity| identity != (metadata.dev(), metadata.ino()))
    {
        bail!(
            "{} changed while it was being reclaimed; it is left in place",
            tombstone.display()
        );
    }
    // `remove_dir_all` removes a symlink inside rather than following it.
    fs::remove_dir_all(tombstone).with_context(|| format!("cannot remove {}", tombstone.display()))
}

/// The verdict for `stable` and, when it is reclaimable, its identity.
fn inspect_identity(
    stable: &Path,
    owner_ended: OwnerEnded,
) -> Result<(Verdict, Option<(u64, u64)>)> {
    let metadata = match fs::symlink_metadata(stable) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok((Verdict::Absent, None)),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", stable.display()));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok((
            Verdict::Kept("it is not a real directory, so it is left untouched".to_string()),
            None,
        ));
    }
    let identity = Some((metadata.dev(), metadata.ino()));
    let bytes = match read_record(&stable.join(CLAIM)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let empty = fs::read_dir(stable)
                .with_context(|| format!("cannot list {}", stable.display()))?
                .next()
                .is_none();
            return Ok(if empty {
                (
                    Verdict::Reclaimable(
                        "an empty claim whose record was never written".to_string(),
                    ),
                    identity,
                )
            } else {
                (
                    Verdict::Kept(
                        "it holds files but no claim record, so its owner is unknown".to_string(),
                    ),
                    None,
                )
            });
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot read {}", stable.join(CLAIM).display()));
        }
    };
    let record: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("cannot parse {}", stable.join(CLAIM).display()))?;
    let run_dir = match (
        record["schema_version"].as_u64(),
        record["run_dir"].as_str(),
    ) {
        (Some(1), Some(dir)) if Path::new(dir).is_absolute() => PathBuf::from(dir),
        _ => bail!(
            "{} is not a valid claim record",
            stable.join(CLAIM).display()
        ),
    };
    let ended = owner_ended(&run_dir)
        .with_context(|| format!("cannot tell whether run {} has ended", run_dir.display()))?;
    if !ended {
        return Ok((
            Verdict::Kept(format!(
                "its owner run {} is still running",
                run_dir.display()
            )),
            None,
        ));
    }
    if let Some(worktree) = live_dependant(stable)? {
        return Ok((
            Verdict::Kept(format!(
                "durable worktree {} still uses its repository",
                worktree.display()
            )),
            None,
        ));
    }
    Ok((
        Verdict::Reclaimable(format!(
            "its owner run {} has ended and no durable worktree uses it",
            run_dir.display()
        )),
        identity,
    ))
}

/// Under the lock: inspects `stable` and, when it is reclaimable, renames it
/// to a tombstone beside it. Returns the tombstone and its identity.
#[allow(clippy::type_complexity)]
fn move_aside(
    stable: &Path,
    owner_ended: OwnerEnded,
) -> Result<(Verdict, Option<(PathBuf, (u64, u64))>)> {
    let (verdict, identity) = inspect_identity(stable, owner_ended)?;
    let (Verdict::Reclaimable(_), Some(identity)) = (&verdict, identity) else {
        return Ok((verdict, None));
    };
    let tombstone = stable.with_file_name(format!("{TOMBSTONE_PREFIX}{}", uuid::Uuid::new_v4()));
    fs::rename(stable, &tombstone)
        .with_context(|| format!("cannot move {} aside to reclaim it", stable.display()))?;
    Ok((verdict, Some((tombstone, identity))))
}

/// The first worktree registered in `<stable>/repository.git` that lies
/// outside `stable` and still exists. Git records each one's `.git` file in
/// `worktrees/<name>/gitdir`, as an absolute path or one relative to that
/// folder; an entry without it is one Git itself would prune.
fn live_dependant(stable: &Path) -> Result<Option<PathBuf>> {
    let admin = stable.join("repository.git").join("worktrees");
    let entries = match fs::read_dir(&admin) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot list {}", admin.display()));
        }
    };
    let own = stable
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", stable.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot list {}", admin.display()))?;
        let record = entry.path().join("gitdir");
        let bytes = match read_record(&record) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", record.display()));
            }
        };
        let text = std::str::from_utf8(&bytes)
            .with_context(|| format!("{} is not UTF-8", record.display()))?
            .trim_end_matches(['\n', '\r']);
        let gitfile = entry.path().join(text);
        match fs::symlink_metadata(&gitfile) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot inspect {}", gitfile.display()));
            }
        }
        let resolved = gitfile
            .canonicalize()
            .with_context(|| format!("cannot resolve {}", gitfile.display()))?;
        if !resolved.starts_with(&own) {
            return Ok(Some(
                resolved.parent().map(Path::to_path_buf).unwrap_or(resolved),
            ));
        }
    }
    Ok(None)
}

/// Creates `stable` exclusively and writes its claim record. `false` when
/// it already exists.
fn create(stable: &Path, owner_run_dir: &Path) -> Result<bool> {
    match fs::DirBuilder::new().mode(0o700).create(stable) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot create execution storage {}", stable.display()));
        }
    }
    let record = serde_json::json!({"schema_version": 1, "run_dir": owner_run_dir});
    let path = stable.join(CLAIM);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    file.write_all(&serde_json::to_vec(&record)?)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("cannot write {}", path.display()))?;
    fs::File::open(stable)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("cannot sync {}", stable.display()))?;
    Ok(true)
}

/// Creates the parent of `stable` (mode 0700) and requires it to be a
/// private real directory.
fn private_parent(stable: &Path) -> Result<PathBuf> {
    let parent = stable
        .parent()
        .context("execution storage path has no parent")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .with_context(|| format!("cannot create {}", parent.display()))?;
    let metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("cannot inspect {}", parent.display()))?;
    if !metadata.is_dir() || metadata.mode() & 0o077 != 0 {
        bail!(
            "execution storage parent {} must be a private directory (mode 0700)",
            parent.display()
        );
    }
    Ok(parent.to_path_buf())
}

/// An exclusive lock on the `exec/` folder, released when dropped.
fn lock(parent: &Path) -> Result<fs::File> {
    let dir = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(parent)
        .with_context(|| format!("cannot open {}", parent.display()))?;
    loop {
        // SAFETY: `dir` is an open descriptor for the duration of the call.
        if unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(dir);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error).with_context(|| format!("cannot lock {}", parent.display()));
        }
    }
}

/// Reads a small regular file without following a symlink.
fn read_record(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    let mut bytes = Vec::new();
    file.take(RECORD_MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > RECORD_MAX_BYTES {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("{} is larger than {RECORD_MAX_BYTES} bytes", path.display()),
        ));
    }
    Ok(bytes)
}
