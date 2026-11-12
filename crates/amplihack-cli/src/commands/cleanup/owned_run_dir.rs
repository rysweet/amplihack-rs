//! Exclusive creation is the only source of deletion authority. Records outside
//! and inside the leaf bind its identity to an independently retained root.
//! Linux removal uses no-follow descriptors, statx mount IDs and no-replace
//! claims. Other platforms preserve candidates rather than weaken this contract.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

const META: &str = ".amplihack-ownership";
const MARKER: &str = ".amplihack-owner.json";
const LIMIT: u64 = 16384;

#[derive(Debug)]
pub(crate) enum CleanupOutcome {
    Removed,
    Preserved(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    dev: u64,
    ino: u64,
    mount: u64,
    birth_sec: i64,
    birth_nsec: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    path: PathBuf,
    identity: Identity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u32,
    run_id: String,
    nonce: String,
    leaf: String,
    root: Observation,
    candidate: Identity,
    protected: Vec<Observation>,
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedRunDir {
    path: PathBuf,
    root: Arc<File>,
    metadata: Arc<File>,
    // Retaining the descriptor also prevents inode reuse during this lifecycle.
    candidate: Arc<File>,
    record: Record,
}

impl OwnedRunDir {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn create(root: &Path, leaf: &str, run_id: &str) -> Result<Self> {
        valid_leaf(leaf)?;
        ensure!(!run_id.is_empty(), "empty run identifier");
        let root_path = fs::canonicalize(root).context("resolve selected run root")?;
        let root = Arc::new(sys::open_dir(&root_path)?);
        let root_id = sys::identity(&root)?;
        let protected = observations()?;
        let metadata = Arc::new(metadata_dir(&root, true)?);
        let _lock = Lock::acquire(&metadata)?;
        // mkdirat is exclusive: never mark a pre-existing leaf, even if empty.
        sys::mkdir(&root, leaf)?;
        let path = root_path.join(leaf);
        let initialize = || -> Result<Self> {
            let candidate = Arc::new(sys::open_child_dir(&root, leaf)?);
            sys::private_dir(&candidate)?;
            let candidate_id = sys::identity(&candidate)?;
            ensure!(
                candidate_id.mount == root_id.mount,
                "run leaf crosses a mount boundary"
            );
            let record = Record {
                schema_version: 1,
                run_id: run_id.to_owned(),
                nonce: uuid::Uuid::new_v4().to_string(),
                leaf: leaf.to_owned(),
                root: Observation {
                    path: root_path.clone(),
                    identity: root_id.clone(),
                },
                candidate: candidate_id,
                protected: protected.clone(),
            };
            let bytes = serde_json::to_vec(&record)?;
            write_record(&candidate, MARKER, &bytes)?;
            write_record(&metadata, &record_name(leaf), &bytes)?;
            let owned = Self {
                path: path.clone(),
                root: root.clone(),
                metadata: metadata.clone(),
                candidate,
                record,
            };
            owned.validate()?;
            Ok(owned)
        };
        initialize()
            .with_context(|| format!("ownership initialization incomplete; preserving {path:?}"))
    }

    /// Stale discovery can recover authority only from two matching records.
    /// The caller supplies the root independently of the candidate/record.
    pub(crate) fn load(root: &Path, path: &Path, run_id: &str) -> Result<Self> {
        let root_path = fs::canonicalize(root)?;
        let root = Arc::new(sys::open_dir(&root_path)?);
        let leaf = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("invalid run leaf")?;
        valid_leaf(leaf)?;
        ensure!(
            path.parent().map(fs::canonicalize).transpose()? == Some(root_path),
            "wrong run parent"
        );
        ensure!(
            !fs::symlink_metadata(path)?.file_type().is_symlink(),
            "symlink run target"
        );
        let metadata = Arc::new(metadata_dir(&root, false)?);
        let candidate = Arc::new(sys::open_child_dir(&root, leaf)?);
        let record = read_record(&metadata, &record_name(leaf))?;
        ensure!(
            record.run_id == run_id && record.leaf == leaf,
            "ownership run identifier mismatch"
        );
        let owned = Self {
            path: path.to_owned(),
            root,
            metadata,
            candidate,
            record,
        };
        owned.validate()?;
        Ok(owned)
    }

    fn validate(&self) -> Result<()> {
        let r = &self.record;
        ensure!(
            r.schema_version == 1 && uuid::Uuid::parse_str(&r.nonce).is_ok(),
            "unsupported or malformed ownership record"
        );
        ensure!(
            sys::identity(&self.root)? == r.root.identity,
            "run root identity changed"
        );
        ensure!(
            sys::identity(&sys::open_dir(&r.root.path)?)? == r.root.identity,
            "run root replaced"
        );
        ensure!(
            sys::identity(&self.candidate)? == r.candidate,
            "run candidate identity changed"
        );
        ensure!(
            sys::identity(&sys::open_child_dir(&self.root, &r.leaf)?)? == r.candidate,
            "run candidate replaced"
        );
        ensure!(
            self.path.file_name().and_then(|s| s.to_str()) == Some(&r.leaf),
            "run leaf mismatch"
        );
        ensure!(
            self.path.parent().map(fs::canonicalize).transpose()? == Some(r.root.path.clone()),
            "wrong run parent"
        );
        ensure!(
            !fs::symlink_metadata(&self.path)?.file_type().is_symlink(),
            "symlink run target"
        );
        ensure!(
            r.candidate.mount == r.root.identity.mount,
            "mounted run candidate"
        );
        sys::private_dir(&self.candidate)?;
        sys::private_dir(&self.metadata)?;
        ensure!(
            sys::identity(&metadata_dir(&self.root, false)?)? == sys::identity(&self.metadata)?,
            "ownership metadata replaced"
        );
        ensure!(
            read_record(&self.candidate, MARKER)? == *r,
            "internal ownership evidence inconsistent"
        );
        ensure!(
            read_record(&self.metadata, &record_name(&r.leaf))? == *r,
            "external ownership evidence inconsistent"
        );
        self.protected()?;
        Ok(())
    }

    fn protected(&self) -> Result<()> {
        let r = &self.record;
        for observation in r.protected.iter().chain(observations()?.iter()) {
            let now = sys::identity(&sys::open_dir(&observation.path)?)?;
            ensure!(now == observation.identity, "protected root replaced");
            // dev/inode comparison also catches bind-mount aliases.
            ensure!(
                !(now.dev == r.candidate.dev && now.ino == r.candidate.ino),
                "protected root identity"
            );
            ensure!(self.path != observation.path, "protected root path");
        }
        ensure!(
            r.candidate.dev != r.root.identity.dev || r.candidate.ino != r.root.identity.ino,
            "candidate is configured root"
        );
        Ok(())
    }

    pub(crate) fn check(&self) -> Result<()> {
        self.validate()
    }

    pub(crate) fn remove(&self) -> CleanupOutcome {
        let _lock = match Lock::acquire(&self.metadata) {
            Ok(lock) => lock,
            Err(e) => {
                return CleanupOutcome::Preserved(format!("ownership lock unavailable: {e:#}"));
            }
        };
        if let Err(e) = self
            .validate()
            .and_then(|_| sys::walk(&self.candidate, self.record.candidate.mount, false))
        {
            return CleanupOutcome::Preserved(format!("{e:#}"));
        }
        let claim = format!(".amplihack-claim-{}", uuid::Uuid::new_v4());
        if let Err(e) = sys::rename(&self.root, &self.record.leaf, &claim) {
            return CleanupOutcome::Preserved(format!("cannot claim run directory: {e:#}"));
        }
        let claimed = || -> Result<File> {
            let fd = sys::open_child_dir(&self.root, &claim)?;
            ensure!(
                sys::identity(&fd)? == self.record.candidate,
                "claimed candidate was replaced"
            );
            ensure!(
                sys::identity(&sys::open_dir(&self.record.root.path)?)?
                    == self.record.root.identity,
                "claimed root was replaced"
            );
            self.protected()?;
            ensure!(
                read_record(&fd, MARKER)? == self.record,
                "claimed ownership changed"
            );
            ensure!(
                read_record(&self.metadata, &record_name(&self.record.leaf))? == self.record,
                "claimed external evidence changed"
            );
            sys::walk(&fd, self.record.candidate.mount, false)?;
            Ok(fd)
        };
        let fd = match claimed() {
            Ok(fd) => fd,
            Err(e) => {
                let restore = sys::rename(&self.root, &claim, &self.record.leaf);
                return CleanupOutcome::Preserved(format!(
                    "{e:#}; claim {claim:?} restore: {restore:?}"
                ));
            }
        };
        let removal = || -> Result<()> {
            sys::walk(&fd, self.record.candidate.mount, true)?;
            ensure!(
                sys::identity(&sys::open_child_dir(&self.root, &claim)?)? == self.record.candidate,
                "claim replaced during removal"
            );
            sys::unlink(&self.root, &claim, true)?;
            sys::unlink(&self.metadata, &record_name(&self.record.leaf), false)?;
            Ok(())
        };
        match removal() {
            Ok(()) => CleanupOutcome::Removed,
            Err(e) => {
                let restore = sys::rename(&self.root, &claim, &self.record.leaf);
                CleanupOutcome::Failed(format!(
                    "{e:#}; remaining claim {claim:?} restore: {restore:?}"
                ))
            }
        }
    }
}

fn valid_leaf(leaf: &str) -> Result<()> {
    let mut components = Path::new(leaf).components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none(),
        "run leaf must be one normal component"
    );
    ensure!(
        !leaf.starts_with('.') && !leaf.contains('/') && leaf.len() <= 128,
        "reserved or oversized run leaf"
    );
    Ok(())
}
fn record_name(leaf: &str) -> String {
    format!("{leaf}.json")
}
fn observations() -> Result<Vec<Observation>> {
    let mut paths = vec![PathBuf::from("/tmp"), std::env::temp_dir()];
    for key in ["TMPDIR", "HOME"] {
        if let Some(value) = std::env::var_os(key).filter(|s| !s.is_empty()) {
            paths.push(value.into());
        }
    }
    paths
        .into_iter()
        .map(|p| {
            let path = fs::canonicalize(&p)
                .with_context(|| format!("cannot resolve protected root {p:?}"))?;
            let identity = sys::identity(&sys::open_dir(&path)?)?;
            Ok(Observation { path, identity })
        })
        .collect()
}
fn metadata_dir(root: &File, create: bool) -> Result<File> {
    if create {
        sys::mkdir_if_missing(root, META)?;
    }
    let fd = sys::open_child_dir(root, META)?;
    sys::private_dir(&fd)?;
    Ok(fd)
}
fn write_record(dir: &File, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = sys::create_file(dir, name)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    dir.sync_all()?;
    Ok(())
}
fn read_record(dir: &File, name: &str) -> Result<Record> {
    let file = sys::open_file(dir, name)?;
    sys::private_file(&file)?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= LIMIT, "ownership record too large");
    Ok(serde_json::from_slice(&bytes).context("invalid ownership record")?)
}
struct Lock(File);
impl Lock {
    fn acquire(dir: &File) -> Result<Self> {
        let file = match sys::create_file(dir, "lock") {
            Ok(file) => file,
            Err(_) => sys::open_file(dir, "lock")?,
        };
        sys::private_file(&file)?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(Self(file))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        if let Err(e) = fs2::FileExt::unlock(&self.0) {
            tracing::warn!(%e, "ownership lock release failed");
        }
    }
}

#[cfg(target_os = "linux")]
mod sys {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    fn name(value: &str) -> Result<CString> {
        Ok(CString::new(value)?)
    }
    fn cvt(rc: i32) -> Result<i32> {
        if rc < 0 {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(rc)
        }
    }
    fn open_at(dir: i32, name: &CStr, flags: i32) -> Result<File> {
        // SAFETY: valid C string and flags; returned descriptor uniquely owned.
        let fd = cvt(unsafe {
            libc::openat(
                dir,
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                0o600,
            )
        })?;
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub(super) fn open_dir(path: &Path) -> Result<File> {
        open_at(
            libc::AT_FDCWD,
            &CString::new(path.as_os_str().as_bytes())?,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )
    }
    pub(super) fn open_child_dir(dir: &File, leaf: &str) -> Result<File> {
        open_at(
            dir.as_raw_fd(),
            &name(leaf)?,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )
    }
    pub(super) fn open_file(dir: &File, leaf: &str) -> Result<File> {
        open_at(dir.as_raw_fd(), &name(leaf)?, libc::O_RDONLY)
    }
    pub(super) fn create_file(dir: &File, leaf: &str) -> Result<File> {
        open_at(
            dir.as_raw_fd(),
            &name(leaf)?,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )
    }
    pub(super) fn mkdir(dir: &File, leaf: &str) -> Result<()> {
        cvt(unsafe { libc::mkdirat(dir.as_raw_fd(), name(leaf)?.as_ptr(), 0o700) })?;
        Ok(())
    }
    pub(super) fn mkdir_if_missing(dir: &File, leaf: &str) -> Result<()> {
        match mkdir(dir, leaf) {
            Ok(()) => Ok(()),
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists) =>
            {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
    fn stat(dir: i32, leaf: &CStr, flags: i32) -> Result<libc::statx> {
        let mut result = std::mem::MaybeUninit::<libc::statx>::zeroed();
        cvt(unsafe {
            libc::statx(
                dir,
                leaf.as_ptr(),
                flags | libc::AT_SYMLINK_NOFOLLOW,
                libc::STATX_BASIC_STATS | libc::STATX_BTIME | libc::STATX_MNT_ID,
                result.as_mut_ptr(),
            )
        })?;
        let result = unsafe { result.assume_init() };
        ensure!(
            result.stx_mask & (libc::STATX_MNT_ID | libc::STATX_BTIME | libc::STATX_INO)
                == (libc::STATX_MNT_ID | libc::STATX_BTIME | libc::STATX_INO),
            "mount/birth identity unavailable"
        );
        Ok(result)
    }
    fn id(s: &libc::statx) -> Identity {
        Identity {
            dev: ((s.stx_dev_major as u64) << 32) | s.stx_dev_minor as u64,
            ino: s.stx_ino,
            mount: s.stx_mnt_id,
            birth_sec: s.stx_btime.tv_sec,
            birth_nsec: s.stx_btime.tv_nsec,
        }
    }
    pub(super) fn identity(fd: &File) -> Result<Identity> {
        Ok(id(&stat(fd.as_raw_fd(), c"", libc::AT_EMPTY_PATH)?))
    }
    pub(super) fn private_dir(fd: &File) -> Result<()> {
        let m = fd.metadata()?;
        ensure!(
            m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
            "ownership directory is not private"
        );
        Ok(())
    }
    pub(super) fn private_file(fd: &File) -> Result<()> {
        let m = fd.metadata()?;
        ensure!(
            m.is_file()
                && m.nlink() == 1
                && m.uid() == unsafe { libc::geteuid() }
                && m.mode() & 0o777 == 0o600,
            "ownership record is not a private regular file with one link"
        );
        Ok(())
    }
    pub(super) fn rename(root: &File, from: &str, to: &str) -> Result<()> {
        cvt(unsafe {
            libc::renameat2(
                root.as_raw_fd(),
                name(from)?.as_ptr(),
                root.as_raw_fd(),
                name(to)?.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        })?;
        Ok(())
    }
    pub(super) fn unlink(root: &File, leaf: &str, directory: bool) -> Result<()> {
        cvt(unsafe {
            libc::unlinkat(
                root.as_raw_fd(),
                name(leaf)?.as_ptr(),
                if directory { libc::AT_REMOVEDIR } else { 0 },
            )
        })?;
        Ok(())
    }
    fn entries(fd: &File) -> Result<Vec<CString>> {
        // Open a new file description: dup would share a consumed directory offset.
        let scan = open_at(fd.as_raw_fd(), c".", libc::O_RDONLY | libc::O_DIRECTORY)?;
        use std::os::fd::IntoRawFd;
        let raw = scan.into_raw_fd();
        let ptr = unsafe { libc::fdopendir(raw) };
        if ptr.is_null() {
            unsafe { libc::close(raw) };
            bail!(
                "cannot open directory stream: {}",
                std::io::Error::last_os_error()
            );
        }
        struct Stream(*mut libc::DIR);
        impl Drop for Stream {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let stream = Stream(ptr);
        let mut out = Vec::new();
        loop {
            errno::set_errno(errno::Errno(0));
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                ensure!(
                    errno::errno().0 == 0,
                    "directory read failed: {}",
                    std::io::Error::last_os_error()
                );
                break;
            }
            let leaf = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if leaf != c"." && leaf != c".." {
                out.push(leaf.to_owned());
            }
        }
        Ok(out)
    }
    pub(super) fn walk(fd: &File, mount: u64, remove: bool) -> Result<()> {
        ensure!(identity(fd)?.mount == mount, "directory mount boundary");
        for leaf in entries(fd)? {
            let before = stat(fd.as_raw_fd(), &leaf, 0)?;
            ensure!(before.stx_mnt_id == mount, "entry mount boundary");
            let directory = before.stx_mode as u32 & libc::S_IFMT == libc::S_IFDIR;
            if directory {
                let child = open_at(fd.as_raw_fd(), &leaf, libc::O_RDONLY | libc::O_DIRECTORY)?;
                ensure!(
                    identity(&child)? == id(&before),
                    "directory replaced during traversal"
                );
                walk(&child, mount, remove)?;
            }
            if remove {
                ensure!(
                    id(&stat(fd.as_raw_fd(), &leaf, 0)?) == id(&before),
                    "entry replaced before unlink"
                );
                cvt(unsafe {
                    libc::unlinkat(
                        fd.as_raw_fd(),
                        leaf.as_ptr(),
                        if directory { libc::AT_REMOVEDIR } else { 0 },
                    )
                })?;
            }
        }
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
mod sys {
    use super::*;
    macro_rules! unsupported {
        ($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty) => {
            pub(super) fn $name($($arg: $ty),*) -> Result<$ret> {
                $(let _ = $arg;)*
                bail!("owned run cleanup requires Linux descriptor and mount identity support")
            }
        }
    }
    unsupported!(open_dir(path: &Path) -> File);
    unsupported!(open_child_dir(dir: &File, leaf: &str) -> File);
    unsupported!(open_file(dir: &File, leaf: &str) -> File);
    unsupported!(create_file(dir: &File, leaf: &str) -> File);
    unsupported!(mkdir(dir: &File, leaf: &str) -> ());
    unsupported!(mkdir_if_missing(dir: &File, leaf: &str) -> ());
    unsupported!(identity(fd: &File) -> Identity);
    unsupported!(private_dir(fd: &File) -> ());
    unsupported!(private_file(fd: &File) -> ());
    unsupported!(rename(root: &File, from: &str, to: &str) -> ());
    unsupported!(unlink(root: &File, leaf: &str, directory: bool) -> ());
    unsupported!(walk(fd: &File, mount: u64, remove: bool) -> ());
}
