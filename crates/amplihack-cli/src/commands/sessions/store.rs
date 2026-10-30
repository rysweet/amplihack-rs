//! The archive destination (DEST): its privacy checks, its lock and its
//! manifest. Every file here is opened without following a final symlink
//! and created with mode `0600`; every directory is created `0700`.

use amplihack_utils::secure_files::{euid, fsync_dir};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

pub const MANIFEST: &str = "MANIFEST.jsonl";
pub const LOCK: &str = ".archive.lock";
pub const MAX_MANIFEST: u64 = 64 * 1024 * 1024;

static TMP_NAME: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\.[^/]+\.tar\.gz\.tmp-[0-9]+$").expect("static regex compiles")
});

/// One manifest line. Unknown fields are ignored when read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub time: String,
    pub session_id: String,
    pub project: String,
    pub bytes: u64,
    /// Relative to DEST: `<project>/<file>.tar.gz`.
    pub archive: String,
    pub parts: u64,
}

#[derive(Debug)]
pub enum LockError {
    Held(PathBuf),
    Setup(PathBuf, io::ErrorKind),
}

#[derive(Debug)]
pub enum LookupError {
    TooLarge,
    Io(io::ErrorKind),
}

/// The exclusive lock; released when dropped.
#[derive(Debug)]
pub struct Lock {
    _file: fs::File,
}

#[derive(Debug, Clone)]
pub struct Dest {
    pub root: PathBuf,
}

fn open_opts(write: bool, append: bool, create: bool) -> fs::OpenOptions {
    let mut o = fs::OpenOptions::new();
    o.read(!append)
        .write(write && !append)
        .append(append)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    o
}

/// A regular file owned by us, checked on the open descriptor.
fn own_regular(f: &fs::File) -> io::Result<()> {
    let m = f.metadata()?;
    if !m.is_file() || m.uid() != euid() {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(())
}

impl Dest {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn manifest(&self) -> PathBuf {
        self.root.join(MANIFEST)
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK)
    }

    /// Creates DEST (and missing parents) with mode `0700`.
    pub fn ensure(&self) -> io::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)
    }

    /// A directory owned by the effective user, with no group or other
    /// write bit. A symlinked DEST is checked at its target.
    pub fn check_private(&self) -> io::Result<bool> {
        let m = fs::metadata(&self.root)?;
        Ok(m.is_dir() && m.uid() == euid() && m.mode() & 0o022 == 0)
    }

    pub fn lock(&self) -> Result<Lock, LockError> {
        let path = self.lock_path();
        let setup = |e: io::Error| LockError::Setup(path.clone(), e.kind());
        let file = open_opts(true, false, true).open(&path).map_err(setup)?;
        own_regular(&file).map_err(setup)?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(true) => Ok(Lock { _file: file }),
            Ok(false) => Err(LockError::Held(path)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(LockError::Held(path)),
            Err(e) => Err(setup(e)),
        }
    }

    /// `<DEST>/<project>`: created `0700`, or an existing real directory
    /// owned by us. Anything else is `InvalidData`.
    pub fn project_dir(&self, project: &str) -> io::Result<PathBuf> {
        let dir = self.root.join(project);
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {
                fsync_dir(&self.root)?;
                return Ok(dir);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let m = fs::symlink_metadata(&dir)?;
        if m.file_type().is_symlink() || !m.is_dir() || m.uid() != euid() {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        Ok(dir)
    }

    /// Removes crash leftovers `<DEST>/<project>/.<id>.tar.gz.tmp-<digits>`:
    /// regular files only, one level down, in real directories only. Call
    /// with the lock held. Returns what could not be removed.
    pub fn clean_tmp(&self) -> Vec<(PathBuf, io::ErrorKind)> {
        let mut failed = Vec::new();
        let Ok(projects) = fs::read_dir(&self.root) else {
            return failed;
        };
        for project in projects.flatten() {
            if !project.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Ok(files) = fs::read_dir(project.path()) else {
                continue;
            };
            for f in files.flatten() {
                let matches = f.file_name().to_str().is_some_and(|n| TMP_NAME.is_match(n));
                if matches
                    && f.file_type().is_ok_and(|t| t.is_file())
                    && let Err(e) = fs::remove_file(f.path())
                {
                    failed.push((f.path(), e.kind()));
                }
            }
        }
        failed
    }

    /// Appends one line with `O_APPEND` in a single write, then syncs it.
    pub fn append(&self, entry: &Entry) -> io::Result<()> {
        let mut line = serde_json::to_vec(entry).map_err(|_| io::ErrorKind::InvalidData)?;
        line.push(b'\n');
        let mut f = open_opts(true, true, true).open(self.manifest())?;
        own_regular(&f)?;
        f.write_all(&line)?;
        f.sync_all()
    }

    /// The last line for `id`. Unparseable lines are skipped; a missing
    /// manifest is `None`; one over 64 MiB is refused unread.
    pub fn lookup(&self, id: &str) -> Result<Option<Entry>, LookupError> {
        let f = match open_opts(false, false, false).open(self.manifest()) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(LookupError::Io(e.kind())),
        };
        own_regular(&f).map_err(|e| LookupError::Io(e.kind()))?;
        let len = f.metadata().map_err(|e| LookupError::Io(e.kind()))?.len();
        if len > MAX_MANIFEST {
            return Err(LookupError::TooLarge);
        }
        let mut raw = Vec::new();
        f.take(MAX_MANIFEST + 1)
            .read_to_end(&mut raw)
            .map_err(|e| LookupError::Io(e.kind()))?;
        if raw.len() as u64 > MAX_MANIFEST {
            return Err(LookupError::TooLarge);
        }
        Ok(raw
            .split(|&b| b == b'\n')
            .filter_map(|l| serde_json::from_slice::<Entry>(l).ok())
            .rfind(|e| e.session_id == id))
    }
}

/// Whether DEST equals, lies inside, or contains the projects root,
/// comparing the canonical form of each path's deepest existing ancestor
/// with the rest appended. Creates nothing.
pub fn contains_or_overlaps(dest: &Path, projects_root: &Path) -> bool {
    let d = resolved(dest);
    let r = resolved(projects_root);
    d.starts_with(&r) || r.starts_with(&d)
}

fn resolved(p: &Path) -> PathBuf {
    let mut base = p.to_path_buf();
    let mut tail = Vec::new();
    let canonical = loop {
        if let Ok(c) = base.canonicalize() {
            break c;
        }
        match (base.file_name().map(|n| n.to_os_string()), base.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                base = parent.to_path_buf();
            }
            // `..` as the last component, or the root: no name to keep.
            _ => match base.parent() {
                Some(parent) => {
                    tail.push("..".into());
                    base = parent.to_path_buf();
                }
                None => break base.clone(),
            },
        }
    };
    let mut out = canonical;
    for part in tail.iter().rev() {
        match Path::new(part).components().next() {
            Some(Component::ParentDir) => {
                out.pop();
            }
            Some(Component::Normal(n)) => out.push(n),
            _ => {}
        }
    }
    out
}
