//! Private, bounded evidence snapshots. Historical completion markers grant no authority.
use super::private_mode::{Kind, Repair, make_private, repairable_root};
use super::{authorization::InvocationAuthorization, records::*, validation::*};
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub struct EvidenceStore {
    root: PathBuf,
    directory: File,
    /// Whether entries in this store may be repaired (#2546), decided once
    /// by `repairable_root`: the root was created by this call's own mkdir,
    /// or its canonical path lies directly under the auto-drive state root.
    /// The folder's name never matters.
    repair: Repair,
}
pub struct AcceptedSnapshot {
    path: PathBuf,
    pub record: AcceptedRecord,
}
impl AcceptedSnapshot {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Makes the store folder `root` at 0700 if it is missing, and returns
/// whether this call's own `mkdir` created it, with its lstat. The parents are
/// made recursively, but only the final, non-recursive `mkdir` can report
/// `created_now`: a folder that already exists, or that appears between the
/// lstat and the `mkdir` (EEXIST), is reported as existing (#2546).
pub(crate) fn open_or_create_root(root: &Path) -> Result<(bool, fs::Metadata)> {
    let existing = |m: fs::Metadata| {
        ensure!(
            !m.file_type().is_symlink() && m.is_dir(),
            "state root is a symlink or non-directory"
        );
        Ok((false, m))
    };
    let builder = |recursive: bool| {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(recursive);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
    };
    match fs::symlink_metadata(root) {
        Ok(m) => existing(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = root.parent() {
                builder(true)
                    .create(parent)
                    .with_context(|| format!("create private state {}", parent.display()))?;
            }
            match builder(false).create(root) {
                Ok(()) => Ok((true, fs::symlink_metadata(root)?)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    existing(fs::symlink_metadata(root)?)
                }
                Err(e) => {
                    Err(e).with_context(|| format!("create private state {}", root.display()))
                }
            }
        }
        Err(e) => Err(e.into()),
    }
}

impl EvidenceStore {
    pub fn create(root: &Path) -> Result<Self> {
        let root = if root.is_absolute() {
            root.to_owned()
        } else {
            std::env::current_dir()?.join(root)
        };
        ensure!(
            !root.components().any(|c| matches!(c, Component::ParentDir)),
            "state path contains traversal"
        );
        // Resolve parent aliases uniformly (for example system temporary directories).
        // The designated state entry itself is never allowed to be a symlink.
        let (created_now, before) = open_or_create_root(&root)?;
        let root = root.canonicalize()?;
        let directory = open_directory(&root)?;
        // The folder opened must be the folder checked above: a root swapped
        // for a symlink in between is refused before any repair.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let opened = directory.metadata()?;
            ensure!(
                (opened.dev(), opened.ino()) == (before.dev(), before.ino()),
                "state root changed during open: {}",
                root.display()
            );
        }
        #[cfg(not(unix))]
        let _ = before;
        let repair = if repairable_root(&root, created_now) {
            Repair::Allowed
        } else {
            Repair::Refuse
        };
        make_private(&directory, &root, Kind::Dir, repair)?;
        Ok(Self {
            root,
            directory,
            repair,
        })
    }
    pub fn accept_candidate(
        &self,
        name: &str,
        auth: &InvocationAuthorization,
        expected: &Binding,
    ) -> Result<AcceptedSnapshot> {
        let bytes = self.read_private(name)?;
        let record: CandidateRecord =
            serde_json::from_slice(&bytes).context("invalid candidate evidence JSON")?;
        let record = validate_candidate(auth, expected, &record)?;
        self.snapshot_references(record.record())?;
        let path = self.snapshot(&bytes)?;
        Ok(AcceptedSnapshot { path, record })
    }
    /// Snapshot every referenced artifact from the same opened bytes that were hashed.
    pub fn snapshot_references(&self, record: &CandidateRecord) -> Result<Vec<PathBuf>> {
        use sha2::{Digest, Sha256};
        let mut paths = Vec::new();
        let mut seen = std::collections::BTreeMap::new();
        let mut total = 0usize;
        for e in record.criteria.iter().flat_map(|c| &c.evidence) {
            if let Some(reference) = &e.reference {
                if let Some(digest) = seen.insert(&reference.file, &reference.sha256) {
                    ensure!(
                        digest == &reference.sha256,
                        "contradictory artifact digests"
                    );
                    continue;
                }
                let bytes = self
                    .read_private(&reference.file)
                    .with_context(|| format!("unreadable evidence artifact {}", reference.file))?;
                ensure!(
                    format!("{:x}", Sha256::digest(&bytes)) == reference.sha256,
                    "missing, stale, or altered evidence artifact {}",
                    reference.file
                );
                total = total
                    .checked_add(bytes.len())
                    .context("artifact size overflow")?;
                ensure!(
                    total <= MAX_RECORD_BYTES * 16,
                    "artifact evidence exceeds aggregate collection limit"
                );
                paths.push(self.snapshot(&bytes)?);
            }
        }
        Ok(paths)
    }
    pub fn read_private(&self, name: &str) -> Result<Vec<u8>> {
        let file = self.open(name, false)?;
        make_private(
            &file,
            &self.root.join(name),
            Kind::SingleLinkFile,
            self.repair,
        )?;
        ensure!(
            file.metadata()?.len() <= MAX_RECORD_BYTES as u64,
            "evidence exceeds size limit"
        );
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_RECORD_BYTES,
            "evidence exceeds size limit"
        );
        Ok(bytes)
    }
    pub fn write_new(&self, name: &str, bytes: &[u8]) -> Result<()> {
        ensure!(bytes.len() <= MAX_RECORD_BYTES, "record exceeds size limit");
        let mut file = self.open(name, true)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    pub fn snapshot(&self, bytes: &[u8]) -> Result<PathBuf> {
        ensure!(
            bytes.len() <= MAX_RECORD_BYTES,
            "snapshot exceeds size limit"
        );
        let id = uuid::Uuid::new_v4();
        let staging = format!(".pending-{id}");
        let name = format!("accepted-{id}.json");
        let mut file = self.open(&staging, true)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.publish(&staging, &name)?;
        Ok(self.root.join(name))
    }
    fn open(&self, name: &str, create: bool) -> Result<File> {
        ensure!(
            !name.is_empty()
                && !name.chars().any(char::is_control)
                && Path::new(name).components().count() == 1
                && matches!(
                    Path::new(name).components().next(),
                    Some(Component::Normal(_))
                ),
            "evidence must be a direct child of private state"
        );
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = std::ffi::CString::new(name)?;
            let flags = libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | if create {
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL
                } else {
                    libc::O_RDONLY
                };
            // SAFETY: owned directory descriptor and NUL-terminated basename; returned fd is owned once.
            let fd =
                unsafe { libc::openat(self.directory.as_raw_fd(), name.as_ptr(), flags, 0o600) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("open private evidence without following links");
            }
            Ok(unsafe { File::from_raw_fd(fd) })
        }
        #[cfg(not(unix))]
        {
            let _ = create;
            anyhow::bail!("private auto-drive evidence requires Unix file ownership support")
        }
    }
    fn publish(&self, staging: &str, name: &str) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let from = std::ffi::CString::new(staging)?;
            let to = std::ffi::CString::new(name)?;
            let fd = self.directory.as_raw_fd();
            // linkat publishes complete bytes atomically and never replaces an existing snapshot.
            if unsafe { libc::linkat(fd, from.as_ptr(), fd, to.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error()).context("publish evidence snapshot");
            }
            if unsafe { libc::unlinkat(fd, from.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("remove snapshot staging entry");
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (staging, name);
            anyhow::bail!("private auto-drive evidence requires Unix file ownership support")
        }
    }
}
fn open_directory(path: &Path) -> Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    Ok(options.open(path)?)
}
