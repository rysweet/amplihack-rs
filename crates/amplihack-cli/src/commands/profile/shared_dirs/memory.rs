//! Claude's per-project memory index (issue #1634). Claude reads only
//! `projects/<proj>/memory/MEMORY.md`, which lists the memory files beside
//! it. When a profile's index differs from the shared one, the shared copy
//! still wins and the profile's copy is still saved under a conflict name,
//! but the profile's entries the shared index lacks are then appended to it,
//! so Claude can still find the memory files the merge moved in. Appending
//! is the only change ever made to an existing file in the shared store.

use super::super::name::ProfileName;
use super::super::store::fdwalk;
use super::merge::{MergeError, MergeOutcome, fail, merge_file, stat_at};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Component, Path};

pub(super) const INDEX: &str = "MEMORY.md";
/// A larger index is not merged; its conflict copy is still saved.
const MAX_INDEX_BYTES: u64 = 1 << 20;

/// `rel` is `<proj>/memory`, relative to the shared `projects/`.
pub(super) fn is_memory_dir(rel: &Path) -> bool {
    let parts: Vec<_> = rel.components().collect();
    matches!(parts.as_slice(), [Component::Normal(_), Component::Normal(m)] if *m == "memory")
}

/// The bytes of a regular, not-too-large `dir/name`; `None` otherwise.
pub(super) fn read_index(dir: &OwnedFd, name: &OsStr) -> Result<Option<String>, MergeError> {
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let mut file = match fdwalk::openat(dir.as_raw_fd(), name, flags, 0) {
        Ok(fd) => File::from(fd),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ELOOP | libc::ENOENT)) => {
            return Ok(None);
        }
        Err(e) => return Err(fail("open")(e)),
    };
    let meta = file.metadata().map_err(fail("open"))?;
    if !meta.is_file() || meta.len() > MAX_INDEX_BYTES {
        return Ok(None);
    }
    let mut text = String::new();
    match file.read_to_string(&mut text) {
        Ok(_) => Ok(Some(text)),
        // Not UTF-8: not an index this code can merge.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Ok(None),
        Err(e) => Err(fail("read")(e)),
    }
}

/// The lines of `theirs` that `ours` lacks, headings and blank lines left
/// out. A link to a file saved under a conflict name (`renamed`, file name
/// to saved name) points at the saved name.
pub(super) fn missing_entries(
    ours: &str,
    theirs: &str,
    renamed: &[(String, String)],
) -> Vec<String> {
    let have: std::collections::HashSet<&str> = ours.lines().map(str::trim_end).collect();
    let mut out: Vec<String> = Vec::new();
    for line in theirs.lines().map(str::trim_end) {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let mut line = line.to_string();
        for (from, to) in renamed {
            line = line.replace(&format!("]({from})"), &format!("]({to})"));
        }
        if !have.contains(line.as_str()) && !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

/// Appends `entries` to the shared `dir/MEMORY.md` with `O_APPEND`, so a
/// concurrent writer's bytes are never replaced.
pub(super) fn append_entries(
    dir: &OwnedFd,
    ours: &str,
    entries: &[String],
) -> Result<(), MergeError> {
    let flags = libc::O_WRONLY | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let fd = fdwalk::openat(dir.as_raw_fd(), OsStr::new(INDEX), flags, 0).map_err(fail("open"))?;
    let mut file = File::from(fd);
    if !file.metadata().map_err(fail("open"))?.is_file() {
        return Err(fail("open")(std::io::Error::other(
            "the shared MEMORY.md is not a regular file",
        )));
    }
    let mut text = String::new();
    if !ours.is_empty() && !ours.ends_with('\n') {
        text.push('\n');
    }
    for entry in entries {
        text.push_str(entry);
        text.push('\n');
    }
    file.write_all(text.as_bytes()).map_err(fail("copy"))?;
    file.sync_all().map_err(fail("copy"))
}

/// `MEMORY.md` merges like any file, then, if this profile's copy lost to a
/// different shared copy, the entries the shared copy lacks are appended.
pub(super) fn merge_index(
    src: &OwnedFd,
    dst: &OwnedFd,
    name: &OsStr,
    path: &Path,
    dst_abs: &Path,
    profile: &ProfileName,
    out: &mut MergeOutcome,
) -> Result<(), MergeError> {
    let theirs = read_index(src, name)?;
    let conflicts_before = out.conflicts.len();
    match stat_at(src.as_raw_fd(), name) {
        Ok(st) if st.st_mode & libc::S_IFMT == libc::S_IFREG => {
            merge_file(src, dst, name, path, dst_abs, profile, out)?
        }
        Ok(_) => out.skipped += 1,
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
        Err(e) => return Err(fail("open")(e)),
    }
    // The profile's copy is saved under a conflict name before anything is
    // appended, so its bytes are kept whatever happens next.
    if out.conflicts.len() == conflicts_before {
        return Ok(());
    }
    let (Some(theirs), Some(ours)) = (theirs, read_index(dst, name)?) else {
        return Ok(());
    };
    let dir = path.parent().unwrap_or(Path::new(""));
    let renamed: Vec<(String, String)> = out
        .conflicts
        .iter()
        .filter(|(p, _)| p.parent() == Some(dir))
        .filter_map(|(p, saved)| {
            Some((
                p.file_name()?.to_str()?.to_string(),
                saved.file_name()?.to_str()?.to_string(),
            ))
        })
        .collect();
    let entries = missing_entries(&ours, &theirs, &renamed);
    if !entries.is_empty() {
        append_entries(dst, &ours, &entries)?;
    }
    out.indexes.push((path.to_path_buf(), entries.len()));
    Ok(())
}
