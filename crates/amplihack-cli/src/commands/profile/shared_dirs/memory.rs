//! Claude's per-project memory index (issue #1634). Claude reads only
//! `projects/<proj>/memory/MEMORY.md`, which lists the memory files beside
//! it. When a profile's index differs from the shared one, the shared copy
//! still wins and the profile's copy is still saved under a conflict name,
//! but the profile's entries the shared index lacks are then appended to it,
//! so Claude can still find the memory files the merge moved in. Appending
//! is the only change ever made to an existing file in the shared store.

use super::super::store::fdwalk;
use super::merge::{Ctx, MergeError, fail, merge_file, stat_at};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
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
        let line = rewrite_links(line, renamed);
        if !have.contains(line.as_str()) && !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

/// Each `](target)` whose target was renamed points at the new name; every
/// link is rewritten at most once.
fn rewrite_links(line: &str, renamed: &[(String, String)]) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("](") {
        let (head, tail) = rest.split_at(i + 2);
        out.push_str(head);
        let Some(end) = tail.find(')') else {
            rest = tail;
            break;
        };
        let target = &tail[..end];
        match renamed.iter().find(|(from, _)| from == target) {
            Some((_, to)) => out.push_str(to),
            None => out.push_str(target),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Appends `entries` to the shared `dir/MEMORY.md` with `O_APPEND`, so a
/// concurrent writer's bytes are never replaced. Fails if the file was
/// replaced by a rename meanwhile, since the entries then went nowhere.
fn append_entries(dir: &OwnedFd, ours: &str, entries: &[String]) -> io::Result<()> {
    let flags = libc::O_WRONLY | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let mut file = File::from(fdwalk::openat(
        dir.as_raw_fd(),
        OsStr::new(INDEX),
        flags,
        0,
    )?);
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::other(
            "the shared MEMORY.md is not a regular file",
        ));
    }
    let mut text = String::new();
    if !ours.is_empty() && !ours.ends_with('\n') {
        text.push('\n');
    }
    for entry in entries {
        text.push_str(entry);
        text.push('\n');
    }
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    let now = stat_at(dir.as_raw_fd(), OsStr::new(INDEX))?;
    if now.st_ino as u64 != meta.ino() || now.st_dev as u64 != meta.dev() {
        return Err(io::Error::other(
            "the shared MEMORY.md was replaced meanwhile",
        ));
    }
    Ok(())
}

/// `MEMORY.md` merges like any file, then, if this profile's copy lost to a
/// different shared copy, the entries the shared copy lacks are appended.
pub(super) fn merge_index(
    src: &OwnedFd,
    dst: &OwnedFd,
    name: &OsStr,
    path: &Path,
    dst_abs: &Path,
    cx: &mut Ctx,
) -> Result<(), MergeError> {
    let theirs = read_index(src, name)?;
    let conflicts_before = cx.out.conflicts.len();
    match stat_at(src.as_raw_fd(), name) {
        Ok(st) if st.st_mode & libc::S_IFMT == libc::S_IFREG => {
            merge_file(src, dst, name, path, dst_abs, cx)?
        }
        Ok(_) => cx.out.skipped += 1,
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
        Err(e) => return Err(fail("open")(e)),
    }
    // The profile's copy is saved under a conflict name before anything is
    // appended, so its bytes are kept whatever happens next.
    if cx.out.conflicts.len() == conflicts_before {
        return Ok(());
    }
    // From here on a failure is reported, not fatal: the profile's copy is
    // already safe, and the rest of the merge goes on.
    let ours = match read_index(dst, name) {
        Ok(ours) => ours,
        Err(MergeError::Io { err, .. }) => {
            cx.out
                .indexes
                .push((path.to_path_buf(), Err(err.to_string())));
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let (Some(theirs), Some(ours)) = (theirs, ours) else {
        let why = "an index is over 1 MiB or not UTF-8".to_string();
        cx.out.indexes.push((path.to_path_buf(), Err(why)));
        return Ok(());
    };
    let dir = path.parent().unwrap_or(Path::new(""));
    let renamed: Vec<(String, String)> = cx
        .out
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
    let added = if entries.is_empty() {
        Ok(0)
    } else {
        append_entries(dst, &ours, &entries)
            .map(|()| entries.len())
            .map_err(|e| e.to_string())
    };
    cx.out.indexes.push((path.to_path_buf(), added));
    Ok(())
}
