//! `amplihack sessions restore <id>`: extracts one archived session back
//! into `~/.claude/projects/<project>/`. The archive is opened once without
//! following symlinks and read twice: the first pass checks every member
//! and writes nothing; the second writes, never replacing anything.

use super::fmt::{esc, kind_word, thousands};
use super::scan::{projects_root, valid_name};
use super::store::{Dest, Entry, LockError, LookupError};
use flate2::read::GzDecoder;
use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

/// Room for tar headers, padding and long-name records beyond the data.
const PER_MEMBER_OVERHEAD: u64 = 8 * 1024;
const SLACK: u64 = 1024 * 1024;

#[derive(Debug)]
pub struct RestoreReport {
    pub restored: u64,
    pub conflicts: u64,
    pub target: PathBuf,
}

impl RestoreReport {
    pub fn summary(&self) -> String {
        let mut line = format!(
            "restored {} files to {}",
            thousands(self.restored),
            esc(&self.target)
        );
        match self.conflicts {
            0 => {}
            1 => line.push_str("; 1 conflict kept"),
            n => line.push_str(&format!("; {} conflicts kept", thousands(n))),
        }
        line
    }
}

#[derive(Debug)]
pub enum RestoreError {
    BadId(String),
    Setup(PathBuf, io::ErrorKind),
    NotPrivate(PathBuf),
    LockHeld(PathBuf),
    NotFound(String, PathBuf),
    ManifestTooLarge(PathBuf),
    BadLine(String),
    ArchiveMissing(PathBuf),
    Symlink(PathBuf),
    Unreadable(PathBuf),
    BadMember(PathBuf, PathBuf),
    Mismatch(PathBuf),
    TooMuchData(PathBuf),
    Write(PathBuf, io::ErrorKind),
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadId(id) => write!(f, "refused session {}: bad session id", esc(id)),
            Self::Setup(p, k) => write!(f, "cannot prepare {} ({})", esc(p), kind_word(*k)),
            Self::NotPrivate(p) => write!(
                f,
                "destination {} is not private (must be your own directory, not writable by group or others)",
                esc(p)
            ),
            Self::LockHeld(p) => write!(
                f,
                "another sessions archive run is in progress (lock: {})",
                esc(p)
            ),
            Self::NotFound(id, m) => write!(f, "session {} not found in {}", esc(id), esc(m)),
            Self::ManifestTooLarge(m) => write!(
                f,
                "manifest {} is larger than 64 MiB; nothing restored",
                esc(m)
            ),
            Self::BadLine(id) => write!(f, "refused session {}: bad manifest line", esc(id)),
            Self::ArchiveMissing(p) => write!(f, "archive {} not found", esc(p)),
            Self::Symlink(p) => write!(f, "refused {}: symlink", esc(p)),
            Self::Unreadable(p) => write!(f, "refused {}: not a readable archive", esc(p)),
            Self::BadMember(a, m) => write!(f, "refused {}: bad member {}", esc(a), esc(m)),
            Self::Mismatch(a) => write!(f, "refused {}: member count or size mismatch", esc(a)),
            Self::TooMuchData(a) => {
                write!(f, "refused {}: more data than the manifest records", esc(a))
            }
            Self::Write(p, k) => write!(f, "cannot write {} ({})", esc(p), kind_word(*k)),
        }
    }
}

/// Restores session `id` from `dest` into `<home>/.claude/projects`.
/// Conflicts are reported through `warn` and counted in the report.
pub fn run(
    id: &str,
    home: &Path,
    dest: &Path,
    warn: &mut dyn FnMut(&str),
) -> Result<RestoreReport, RestoreError> {
    if !valid_name(id) {
        return Err(RestoreError::BadId(id.to_string()));
    }
    let dest = Dest::new(dest.to_path_buf());
    match dest.check_private() {
        Ok(true) => {}
        Ok(false) => return Err(RestoreError::NotPrivate(dest.root.clone())),
        Err(e) => return Err(RestoreError::Setup(dest.root.clone(), e.kind())),
    }
    let _lock = dest.lock().map_err(|e| match e {
        LockError::Held(p) => RestoreError::LockHeld(p),
        LockError::Setup(p, k) => RestoreError::Setup(p, k),
    })?;
    let entry = match dest.lookup(id) {
        Ok(Some(e)) => e,
        Ok(None) => return Err(RestoreError::NotFound(id.into(), dest.manifest())),
        Err(LookupError::TooLarge) => return Err(RestoreError::ManifestTooLarge(dest.manifest())),
        Err(LookupError::Io(k)) => return Err(RestoreError::Setup(dest.manifest(), k)),
    };
    if !line_ok(&entry, id) {
        return Err(RestoreError::BadLine(id.to_string()));
    }
    let archive = dest.root.join(&entry.archive);
    let mut file = open_archive(&archive)?;
    let target = projects_root(home).join(&entry.project);
    let limit = entry
        .bytes
        .saturating_add(
            entry
                .parts
                .saturating_add(2)
                .saturating_mul(PER_MEMBER_OVERHEAD),
        )
        .saturating_add(SLACK);
    check_members(&file, &archive, id, &entry, limit)?;
    match fs::symlink_metadata(&target) {
        Ok(m) if m.file_type().is_symlink() => return Err(RestoreError::Symlink(target)),
        _ => {}
    }
    file.rewind()
        .map_err(|_| RestoreError::Unreadable(archive.clone()))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&target)
        .map_err(|e| RestoreError::Write(target.clone(), e.kind()))?;
    let mut x = Extract {
        archive: &archive,
        id,
        target: &target,
        budget: entry.bytes,
        restored: 0,
        conflicts: 0,
        cut: Vec::new(),
        warn,
    };
    x.run(&file, limit)?;
    Ok(RestoreReport {
        restored: x.restored,
        conflicts: x.conflicts,
        target,
    })
}

/// `project` is one valid name; `archive` is `<project>/<name>.tar.gz`.
fn line_ok(e: &Entry, id: &str) -> bool {
    let parts: Vec<Component<'_>> = Path::new(&e.archive).components().collect();
    fn normal<'a>(c: &Component<'a>) -> Option<&'a str> {
        match c {
            Component::Normal(n) => n.to_str(),
            _ => None,
        }
    }
    e.session_id == id
        && valid_name(&e.project)
        && parts.len() == 2
        && normal(&parts[0]) == Some(e.project.as_str())
        && normal(&parts[1]).is_some_and(|n| valid_name(n) && n.ends_with(".tar.gz"))
}

fn open_archive(path: &Path) -> Result<fs::File, RestoreError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => RestoreError::ArchiveMissing(path.to_path_buf()),
            _ if e.raw_os_error() == Some(libc::ELOOP) => RestoreError::Symlink(path.to_path_buf()),
            k => RestoreError::Setup(path.to_path_buf(), k),
        })?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(RestoreError::Unreadable(path.to_path_buf()));
    }
    Ok(file)
}

/// The member's path if it is `<id>.jsonl` (a file) or `<id>` / `<id>/…`
/// with only normal components, and it is a regular file or directory.
fn member_ok(path: &Path, kind: tar::EntryType, id: &str) -> bool {
    if !(kind.is_file() || kind.is_dir()) {
        return false;
    }
    let mut comps = path.components();
    let Some(Component::Normal(first)) = comps.next() else {
        return false;
    };
    if !comps.all(|c| matches!(c, Component::Normal(_))) {
        return false;
    }
    let single = path.components().count() == 1;
    let first = first.to_str();
    let jsonl = format!("{id}.jsonl");
    if single {
        (kind.is_file() && first == Some(jsonl.as_str())) || (kind.is_dir() && first == Some(id))
    } else {
        first == Some(id)
    }
}

fn entries(file: &fs::File, limit: u64) -> tar::Archive<GzDecoder<io::Take<&fs::File>>> {
    tar::Archive::new(GzDecoder::new(file.take(limit)))
}

/// Pass 1: every member checked, nothing written.
fn check_members(
    file: &fs::File,
    archive: &Path,
    id: &str,
    entry: &Entry,
    limit: u64,
) -> Result<(), RestoreError> {
    let unreadable = || RestoreError::Unreadable(archive.to_path_buf());
    let mut a = entries(file, limit);
    let (mut count, mut bytes) = (0u64, 0u64);
    for m in a.entries().map_err(|_| unreadable())? {
        let m = m.map_err(|_| unreadable())?;
        let path = m.path().map_err(|_| unreadable())?.into_owned();
        if !member_ok(&path, m.header().entry_type(), id) {
            return Err(RestoreError::BadMember(archive.to_path_buf(), path));
        }
        count += 1;
        bytes = bytes.saturating_add(m.size());
        // Stop before reading more than the manifest allows.
        if count > entry.parts || bytes > entry.bytes {
            return Err(RestoreError::Mismatch(archive.to_path_buf()));
        }
    }
    if count != entry.parts || bytes != entry.bytes {
        return Err(RestoreError::Mismatch(archive.to_path_buf()));
    }
    // Drain to EOF so the decoder checks the gzip trailer.
    io::copy(&mut a.into_inner(), &mut io::sink()).map_err(|_| unreadable())?;
    Ok(())
}

/// Pass 2 state.
struct Extract<'a> {
    archive: &'a Path,
    id: &'a str,
    target: &'a Path,
    /// Bytes the manifest allows that are not yet written.
    budget: u64,
    restored: u64,
    conflicts: u64,
    /// Relative paths whose subtree is not restored (a conflict was there).
    cut: Vec<PathBuf>,
    warn: &'a mut dyn FnMut(&str),
}

impl Extract<'_> {
    fn run(&mut self, file: &fs::File, limit: u64) -> Result<(), RestoreError> {
        let archive = self.archive;
        let unreadable = || RestoreError::Unreadable(archive.to_path_buf());
        let mut a = entries(file, limit);
        for m in a.entries().map_err(|_| unreadable())? {
            let mut m = m.map_err(|_| unreadable())?;
            let rel = m.path().map_err(|_| unreadable())?.into_owned();
            let kind = m.header().entry_type();
            if !member_ok(&rel, kind, self.id) {
                return Err(RestoreError::BadMember(archive.to_path_buf(), rel));
            }
            if self.cut.iter().any(|c| rel.starts_with(c)) {
                continue;
            }
            let dirs = if kind.is_dir() {
                Some(rel.as_path())
            } else {
                rel.parent()
            };
            if let Some(d) = dirs.filter(|d| !d.as_os_str().is_empty())
                && !self.make_dirs(d)?
            {
                continue;
            }
            if kind.is_file() {
                self.write_file(&rel, &mut m)?;
            }
        }
        Ok(())
    }

    fn conflict(&mut self, rel: &Path) {
        self.conflicts += 1;
        let line = format!(
            "conflict: {} already exists; kept",
            esc(self.target.join(rel))
        );
        (self.warn)(&line);
        self.cut.push(rel.to_path_buf());
    }

    /// Creates each component of `rel` (0700), accepting only real
    /// directories. False when a conflict stops the subtree.
    fn make_dirs(&mut self, rel: &Path) -> Result<bool, RestoreError> {
        let mut sofar = PathBuf::new();
        for c in rel.components() {
            sofar.push(c);
            let path = self.target.join(&sofar);
            match fs::symlink_metadata(&path) {
                Ok(m) if m.is_dir() => continue,
                Ok(_) => {
                    self.conflict(&sofar);
                    return Ok(false);
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(RestoreError::Write(path, e.kind())),
            }
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .map_err(|e| RestoreError::Write(path.clone(), e.kind()))?;
        }
        Ok(true)
    }

    fn write_file(&mut self, rel: &Path, data: &mut impl Read) -> Result<(), RestoreError> {
        let path = self.target.join(rel);
        let mut out = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                self.conflict(rel);
                return Ok(());
            }
            Err(e) => return Err(RestoreError::Write(path, e.kind())),
        };
        let mut limited = data.take(self.budget.saturating_add(1));
        let written = io::copy(&mut limited, &mut out)
            .map_err(|e| RestoreError::Write(path.clone(), e.kind()))?;
        if written > self.budget {
            return Err(RestoreError::TooMuchData(self.archive.to_path_buf()));
        }
        self.budget -= written;
        out.flush()
            .map_err(|e| RestoreError::Write(path.clone(), e.kind()))?;
        self.restored += 1;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests_restore.rs"]
mod tests;
