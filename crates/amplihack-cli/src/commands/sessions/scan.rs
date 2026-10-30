//! Finds Claude Code sessions under the projects root from metadata only.
//! Every walk uses `symlink_metadata`, so no symlink is followed; a session
//! with a symlink or special file anywhere in it is skipped whole.

use super::fmt::kind_word;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One file or directory of a session, as walked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// Relative to the project directory: `<id>.jsonl`, `<id>`, `<id>/…`.
    pub rel: PathBuf,
    pub is_dir: bool,
    /// File size; 0 for a directory.
    pub len: u64,
    pub mtime: SystemTime,
    pub dev: u64,
    pub ino: u64,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub project: String,
    pub id: String,
    pub jsonl: PathBuf,
    pub dir: Option<PathBuf>,
    pub bytes: u64,
    pub parts: u64,
    pub newest_mtime: SystemTime,
    /// Every part, `.jsonl` first, then the folder depth first in name order.
    pub fingerprint: Vec<Part>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    Symlink,
    Special,
    BadName,
    Unreadable(io::ErrorKind),
}

impl Skip {
    pub fn reason(&self) -> String {
        match self {
            Self::Symlink => "symlink".into(),
            Self::Special => "special file".into(),
            Self::BadName => "bad name".into(),
            Self::Unreadable(k) => format!("unreadable ({})", kind_word(*k)),
        }
    }
}

/// `(project, id, reason)` for a session that was left alone.
pub type Skipped = (String, String, Skip);

/// Non-empty, no leading `.`, no `/`, NUL or other control character.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('.') && !s.chars().any(|c| c == '/' || c.is_control())
}

pub fn projects_root(home: &Path) -> PathBuf {
    home.join(".claude/projects")
}

/// Every session under `root`. A missing root has none; a root that cannot
/// be listed is an error. Symlinked projects are passed over silently.
pub fn scan(root: &Path) -> io::Result<(Vec<Session>, Vec<Skipped>)> {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
        Err(e) => return Err(e),
    };
    let mut projects = Vec::new();
    for entry in entries {
        let entry = entry?;
        // DirEntry::file_type does not follow symlinks.
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Ok(name) = entry.file_name().into_string()
            && valid_name(&name)
        {
            projects.push(name);
        }
    }
    projects.sort();
    let (mut sessions, mut skipped) = (Vec::new(), Vec::new());
    for project in projects {
        let pdir = root.join(&project);
        let entries = match fs::read_dir(&pdir) {
            Ok(e) => e,
            Err(e) => {
                skipped.push((project, String::new(), Skip::Unreadable(e.kind())));
                continue;
            }
        };
        let mut found = Vec::new();
        for entry in entries {
            let entry = entry?;
            let raw = entry.file_name();
            let lossy = raw.to_string_lossy();
            let Some(stem) = lossy.strip_suffix(".jsonl") else {
                continue;
            };
            let ft = entry.file_type()?;
            if ft.is_dir() {
                continue;
            }
            let skip = if ft.is_symlink() {
                Some(Skip::Symlink)
            } else if !ft.is_file() {
                Some(Skip::Special)
            } else if raw.to_str().is_none() || !valid_name(stem) {
                Some(Skip::BadName)
            } else {
                None
            };
            found.push((stem.to_string(), skip));
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, skip) in found {
            match skip.map_or_else(|| describe(&pdir, &project, &id), Err) {
                Ok(s) => sessions.push(s),
                Err(k) => skipped.push((project.clone(), id, k)),
            }
        }
    }
    Ok((sessions, skipped))
}

/// Walks one session: `<pdir>/<id>.jsonl` and, if present, `<pdir>/<id>/`.
/// Used by the scan and again just before the originals are removed.
pub fn describe(pdir: &Path, project: &str, id: &str) -> Result<Session, Skip> {
    let unreadable = |e: io::Error| Skip::Unreadable(e.kind());
    let jsonl = pdir.join(format!("{id}.jsonl"));
    let meta = fs::symlink_metadata(&jsonl).map_err(unreadable)?;
    let mut parts = vec![part(PathBuf::from(format!("{id}.jsonl")), &meta)?];
    let dir_path = pdir.join(id);
    let dir = match fs::symlink_metadata(&dir_path) {
        Ok(m) if m.file_type().is_symlink() => return Err(Skip::Symlink),
        Ok(m) if m.is_dir() => {
            walk(&dir_path, PathBuf::from(id), &m, &mut parts)?;
            Some(dir_path)
        }
        // A file named `<id>` is not part of the session.
        Ok(_) => None,
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(unreadable(e)),
    };
    let bytes = parts.iter().fold(0u64, |a, p| a.saturating_add(p.len));
    let newest_mtime = parts
        .iter()
        .map(|p| p.mtime)
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Ok(Session {
        project: project.to_string(),
        id: id.to_string(),
        jsonl,
        dir,
        bytes,
        parts: parts.len() as u64,
        newest_mtime,
        fingerprint: parts,
    })
}

fn part(rel: PathBuf, m: &fs::Metadata) -> Result<Part, Skip> {
    let ft = m.file_type();
    if ft.is_symlink() {
        return Err(Skip::Symlink);
    }
    if !ft.is_file() && !ft.is_dir() {
        return Err(Skip::Special);
    }
    Ok(Part {
        rel,
        is_dir: ft.is_dir(),
        len: if ft.is_file() { m.len() } else { 0 },
        mtime: m.modified().map_err(|e| Skip::Unreadable(e.kind()))?,
        dev: m.dev(),
        ino: m.ino(),
    })
}

/// Adds `dir` and everything below it, depth first in name order.
fn walk(dir: &Path, rel: PathBuf, meta: &fs::Metadata, out: &mut Vec<Part>) -> Result<(), Skip> {
    let unreadable = |e: io::Error| Skip::Unreadable(e.kind());
    out.push(part(rel.clone(), meta)?);
    let mut names = fs::read_dir(dir)
        .map_err(unreadable)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<io::Result<Vec<_>>>()
        .map_err(unreadable)?;
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let m = fs::symlink_metadata(&path).map_err(unreadable)?;
        if m.is_dir() {
            walk(&path, rel.join(&name), &m, out)?;
        } else {
            out.push(part(rel.join(&name), &m)?);
        }
    }
    Ok(())
}
