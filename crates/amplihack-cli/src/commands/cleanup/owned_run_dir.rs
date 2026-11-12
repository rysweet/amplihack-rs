//! Exclusive creation is the only source of deletion authority. Records outside
//! and inside the leaf bind its identity to an independently retained root.
//! Linux removal uses no-follow descriptors, statx mount IDs and no-replace
//! claims. Other platforms preserve candidates rather than weaken this contract.
#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{Result, bail, ensure};
#[cfg(target_os = "linux")]
use serde::{Deserialize, Serialize};
use std::fs;
#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::Arc;

#[cfg(target_os = "linux")]
const META: &str = ".amplihack-ownership";
const MARKER: &str = ".amplihack-owner.json";
#[cfg(target_os = "linux")]
const LIMIT: u64 = 16384;

#[derive(Debug)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) enum CleanupOutcome {
    Removed,
    AlreadyAbsent,
    Unsupported(String),
    Preserved(String),
    Failed(String),
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    dev: u64,
    ino: u64,
    mount: u64,
    birth_sec: i64,
    birth_nsec: u32,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    path: PathBuf,
    identity: Identity,
}

#[cfg(target_os = "linux")]
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

/// Placement authority selected before creating any disposable entry.
#[derive(Debug, Clone)]
pub(crate) struct RunRoot {
    pub(crate) path: PathBuf,
    #[cfg(target_os = "linux")]
    fd: Arc<File>,
    #[cfg(target_os = "linux")]
    identity: Identity,
}

impl RunRoot {
    pub(crate) fn select(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path)?;
        #[cfg(target_os = "linux")]
        {
            let fd = Arc::new(sys::open_dir(&path)?);
            let identity = sys::identity(&fd)?;
            Ok(Self { path, fd, identity })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(Self { path })
        }
    }
    pub(crate) fn metadata(&self) -> Result<fs::Metadata> {
        #[cfg(target_os = "linux")]
        {
            Ok(self.fd.metadata()?)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(fs::metadata(&self.path)?)
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedRunDir {
    path: PathBuf,
    #[cfg(target_os = "linux")]
    root: Arc<File>,
    #[cfg(target_os = "linux")]
    metadata: Arc<File>,
    // Retaining the descriptor also prevents inode reuse during this lifecycle.
    #[cfg(target_os = "linux")]
    candidate: Arc<File>,
    #[cfg(target_os = "linux")]
    record: Record,
}

impl OwnedRunDir {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Verified current location, including a recoverable interrupted claim.
    pub(crate) fn current_path(&self) -> Result<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            self.validate()?;
            Ok(self.record.root.path.join(self.location()?))
        }
        #[cfg(not(target_os = "linux"))]
        {
            bail!("current location unavailable: Linux identity support required")
        }
    }

    pub(crate) fn root_path(&self) -> Option<&Path> {
        #[cfg(target_os = "linux")]
        {
            Some(&self.record.root.path)
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn create(root: &Path, leaf: &str, run_id: &str) -> Result<Self> {
        Self::create_selected(&RunRoot::select(root)?, leaf, run_id)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn create_selected(selected: &RunRoot, leaf: &str, run_id: &str) -> Result<Self> {
        valid_leaf(leaf)?;
        ensure!(!run_id.is_empty(), "empty run identifier");
        let root_path = selected.path.clone();
        let root = selected.fd.clone();
        let root_id = selected.identity.clone();
        ensure!(
            fs::canonicalize(&root_path)? == root_path,
            "selected parent resolution changed"
        );
        ensure!(
            sys::identity(&sys::open_dir(&root_path)?)? == root_id,
            "selected parent replaced before creation"
        );
        let protected = observations()?;
        let metadata = Arc::new(metadata_dir(&root, true)?);
        let _lock = Lock::acquire(&metadata)?;
        // mkdirat is exclusive: never mark a pre-existing leaf, even if empty.
        let path = root_path.join(leaf);
        if let Err(error) = sys::mkdir(&root, leaf) {
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists)
            {
                bail!("per-run folder {path:?} already exists");
            }
            return Err(error)
                .with_context(|| format!("per-run folder {path:?} could not be created"));
        }
        let initialize = || -> Result<Self> {
            ensure!(
                sys::identity(&sys::open_dir(&root_path)?)? == root_id,
                "selected parent replaced during creation"
            );
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
            #[cfg(test)]
            test_event("after_internal_record", &path)?;
            write_record(&metadata, &record_name(leaf), &bytes)?;
            root.sync_all()?;
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
    #[cfg(target_os = "linux")]
    pub(crate) fn load(root: &Path, path: &Path, run_id: &str) -> Result<Self> {
        let root_path = fs::canonicalize(root)?;
        let root = Arc::new(sys::open_dir(&root_path)?);
        let leaf = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("invalid run leaf")?;
        valid_leaf(leaf)?;
        ensure!(
            path.parent().map(fs::canonicalize).transpose()? == Some(root_path.clone()),
            "wrong run parent"
        );
        let metadata = Arc::new(metadata_dir(&root, false)?);
        let record = read_record(&metadata, &record_name(leaf))?;
        ensure!(
            record.schema_version == 1 && uuid::Uuid::parse_str(&record.nonce).is_ok(),
            "unsupported or malformed ownership record"
        );
        valid_leaf(&record.leaf)?;
        // An interrupted claim has a durable association with this creation's
        // nonce. Never scan for, or adopt, arbitrary claim-shaped directories.
        let location = match sys::open_child_dir(&root, leaf) {
            Ok(candidate) => candidate,
            Err(error) if is_absent(&error) => sys::open_child_dir(&root, &claim_name(&record))?,
            Err(error) => return Err(error),
        };
        let candidate = Arc::new(location);
        ensure!(
            record.root.path == root_path,
            "record disagrees with configured root"
        );
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

    #[cfg(target_os = "linux")]
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
        self.location()?;
        ensure!(
            self.path.file_name().and_then(|s| s.to_str()) == Some(&r.leaf),
            "run leaf mismatch"
        );
        ensure!(
            self.path.parent().map(fs::canonicalize).transpose()? == Some(r.root.path.clone()),
            "wrong run parent"
        );
        ensure!(
            fs::canonicalize(&r.root.path)? == r.root.path,
            "run root resolution changed"
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

    /// Resolve only the original entry or its nonce-bound interrupted claim.
    /// A replacement original always prevents recovery, even if a claim exists.
    #[cfg(target_os = "linux")]
    fn location(&self) -> Result<String> {
        match sys::open_child_dir(&self.root, &self.record.leaf) {
            Ok(fd) => {
                ensure!(
                    sys::identity(&fd)? == self.record.candidate,
                    "run candidate replaced"
                );
                ensure!(
                    !sys::exists(&self.root, &claim_name(&self.record))?,
                    "ambiguous run claim"
                );
                Ok(self.record.leaf.clone())
            }
            Err(error) if is_absent(&error) => {
                let claim = claim_name(&self.record);
                let fd = sys::open_child_dir(&self.root, &claim)?;
                ensure!(
                    sys::identity(&fd)? == self.record.candidate,
                    "interrupted claim replaced"
                );
                Ok(claim)
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(target_os = "linux")]
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
            ensure!(
                !observation.path.starts_with(&self.path),
                "protected root within candidate"
            );
            let claim_path = r.root.path.join(claim_name(r));
            ensure!(
                !observation.path.starts_with(claim_path),
                "protected root within claim"
            );
        }
        ensure!(
            r.candidate.dev != r.root.identity.dev || r.candidate.ino != r.root.identity.ino,
            "candidate is configured root"
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn validate_claim(&self, claim: &str) -> Result<()> {
        ensure!(
            read_record(&self.candidate, MARKER)? == self.record,
            "internal evidence changed during traversal"
        );
        self.validate_claim_location(claim)
    }

    #[cfg(target_os = "linux")]
    fn validate_claim_location(&self, claim: &str) -> Result<()> {
        ensure!(
            sys::identity(&sys::open_dir(&self.record.root.path)?)? == self.record.root.identity,
            "root replaced during traversal"
        );
        ensure!(
            fs::canonicalize(&self.record.root.path)? == self.record.root.path,
            "root resolution changed during traversal"
        );
        ensure!(
            !sys::exists(&self.root, &self.record.leaf)?,
            "original leaf occupied during claim"
        );
        ensure!(
            sys::identity(&sys::open_child_dir(&self.root, claim)?)? == self.record.candidate,
            "claim replaced during traversal"
        );
        ensure!(
            sys::identity(&metadata_dir(&self.root, false)?)? == sys::identity(&self.metadata)?,
            "metadata replaced during traversal"
        );
        ensure!(
            read_record(&self.metadata, &record_name(&self.record.leaf))? == self.record,
            "external evidence changed during traversal"
        );
        self.protected()
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn remove(&self) -> CleanupOutcome {
        let _lock = match Lock::acquire(&self.metadata) {
            Ok(lock) => lock,
            Err(e) => return preservation(e.context("ownership lock unavailable")),
        };
        // Only a retained, now-unlinked identity can establish an already
        // completed operation. A missing marker on a live directory cannot.
        let absent = || -> Result<bool> {
            ensure!(
                sys::identity(&sys::open_dir(&self.record.root.path)?)?
                    == self.record.root.identity,
                "run root replaced"
            );
            ensure!(
                fs::canonicalize(&self.record.root.path)? == self.record.root.path,
                "run root resolution changed"
            );
            use std::os::unix::fs::MetadataExt;
            Ok(self.candidate.metadata()?.nlink() == 0
                && !sys::exists(&self.root, &self.record.leaf)?
                && !sys::exists(&self.root, &claim_name(&self.record))?
                && !sys::exists(&self.metadata, &record_name(&self.record.leaf))?)
        };
        match absent() {
            Ok(true) => return CleanupOutcome::AlreadyAbsent,
            Err(e) => return preservation(e),
            Ok(false) => {}
        }
        if let Err(e) = self.validate().and_then(|_| {
            sys::walk(
                &self.candidate,
                self.record.candidate.mount,
                false,
                false,
                &|| self.validate(),
            )
        }) {
            return preservation(e);
        }
        let claim = claim_name(&self.record);
        let location = match self.location() {
            Ok(location) => location,
            Err(e) => return preservation(e),
        };
        if location != claim {
            if let Err(e) = sys::rename(&self.root, &self.record.leaf, &claim) {
                return preservation(e.context("cannot claim run directory"));
            }
            if let Err(e) = self.root.sync_all() {
                return self.restore_claim(&claim, e.into(), false);
            }
        }
        let claimed = || -> Result<File> {
            #[cfg(test)]
            test_event("after_claim", &self.record.root.path.join(&claim))?;
            self.validate_claim(&claim)?;
            let fd = sys::open_child_dir(&self.root, &claim)?;
            ensure!(
                sys::identity(&fd)? == self.record.candidate,
                "claimed candidate was replaced"
            );
            sys::walk(&fd, self.record.candidate.mount, false, false, &|| {
                self.validate_claim(&claim)
            })?;
            Ok(fd)
        };
        let fd = match claimed() {
            Ok(fd) => fd,
            Err(e) => return self.restore_claim(&claim, e, false),
        };
        let removal = || -> Result<()> {
            sys::walk(&fd, self.record.candidate.mount, true, true, &|| {
                self.validate_claim(&claim)
            })?;
            self.validate_claim(&claim)?;
            sys::unlink(&fd, MARKER, false)?;
            // Past this point a crash preserves the empty remnant: missing
            // internal evidence is never reconstructed by a later loader.
            self.validate_claim_location(&claim)?;
            sys::unlink(&self.root, &claim, true)?;
            self.root.sync_all()?;
            sys::unlink(&self.metadata, &record_name(&self.record.leaf), false)?;
            self.metadata.sync_all()?;
            Ok(())
        };
        match removal() {
            Ok(()) => CleanupOutcome::Removed,
            Err(e) => self.restore_claim(&claim, e, true),
        }
    }

    #[cfg(target_os = "linux")]
    fn restore_claim(&self, claim: &str, error: anyhow::Error, started: bool) -> CleanupOutcome {
        // Recovery cannot overwrite a replacement. It must also never move a
        // replacement claim: recheck the claimed inode before the rename.
        let restore = (|| -> Result<()> {
            ensure!(
                sys::identity(&sys::open_child_dir(&self.root, claim)?)? == self.record.candidate,
                "cannot restore replaced claim"
            );
            sys::rename(&self.root, claim, &self.record.leaf)?;
            self.root.sync_all()?;
            Ok(())
        })();
        let message = match restore {
            Ok(()) => format!("{error:#}; preserved at {:?}", self.path),
            Err(restore) => format!(
                "{error:#}; retained claim {:?}; restore refused: {restore:#}",
                self.record.root.path.join(claim)
            ),
        };
        if unsupported(&error) {
            CleanupOutcome::Unsupported(message)
        } else if started {
            CleanupOutcome::Failed(message)
        } else {
            CleanupOutcome::Preserved(message)
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
        !leaf.starts_with('.') && !leaf.contains(['/', '\\', '\0']) && leaf.len() <= 128,
        "reserved or oversized run leaf"
    );
    Ok(())
}
#[cfg(target_os = "linux")]
fn claim_name(record: &Record) -> String {
    format!(".amplihack-claim-{}", record.nonce)
}
#[cfg(target_os = "linux")]
fn is_absent(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}
#[cfg(target_os = "linux")]
fn unsupported(error: &anyhow::Error) -> bool {
    error.downcast_ref::<std::io::Error>().is_some_and(|e| {
        e.kind() == std::io::ErrorKind::Unsupported
            || matches!(e.raw_os_error(), Some(libc::ENOSYS | libc::EOPNOTSUPP))
    })
}
#[cfg(target_os = "linux")]
fn preservation(error: anyhow::Error) -> CleanupOutcome {
    if unsupported(&error) {
        CleanupOutcome::Unsupported(format!(
            "required filesystem guarantee unavailable: {error:#}"
        ))
    } else {
        CleanupOutcome::Preserved(format!("{error:#}"))
    }
}
#[cfg(target_os = "linux")]
fn record_name(leaf: &str) -> String {
    format!("{leaf}.json")
}
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
fn metadata_dir(root: &File, create: bool) -> Result<File> {
    if create {
        sys::mkdir_if_missing(root, META)?;
    }
    let fd = sys::open_child_dir(root, META)?;
    sys::private_dir(&fd)?;
    Ok(fd)
}
#[cfg(target_os = "linux")]
fn write_record(dir: &File, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = sys::create_file(dir, name)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    dir.sync_all()?;
    Ok(())
}
#[cfg(target_os = "linux")]
fn read_record(dir: &File, name: &str) -> Result<Record> {
    let file = sys::open_file(dir, name)?;
    sys::private_file(&file)?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= LIMIT, "ownership record too large");
    serde_json::from_slice(&bytes).context("invalid ownership record")
}
#[cfg(target_os = "linux")]
struct Lock(File);
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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
    pub(super) fn exists(dir: &File, leaf: &str) -> Result<bool> {
        match stat(dir.as_raw_fd(), &name(leaf)?, 0) {
            Ok(_) => Ok(true),
            Err(error) if is_absent(&error) => Ok(false),
            Err(error) => Err(error),
        }
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
            std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "mount/birth identity unavailable"
            )
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
        #[cfg(test)]
        test_event("identity", Path::new(""))?;
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
        #[cfg(test)]
        test_event("rename", Path::new(from))?;
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
    pub(super) fn walk(
        fd: &File,
        mount: u64,
        remove: bool,
        keep_marker: bool,
        validate: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        validate()?;
        ensure!(identity(fd)?.mount == mount, "directory mount boundary");
        for leaf in entries(fd)? {
            if keep_marker && leaf.as_bytes() == MARKER.as_bytes() {
                continue;
            }
            let before = stat(fd.as_raw_fd(), &leaf, 0)?;
            ensure!(before.stx_mnt_id == mount, "entry mount boundary");
            let directory = before.stx_mode as u32 & libc::S_IFMT == libc::S_IFDIR;
            if directory {
                let child = open_at(fd.as_raw_fd(), &leaf, libc::O_RDONLY | libc::O_DIRECTORY)?;
                ensure!(
                    identity(&child)? == id(&before),
                    "directory replaced during traversal"
                );
                walk(&child, mount, remove, false, &|| {
                    validate()?;
                    ensure!(
                        id(&stat(fd.as_raw_fd(), &leaf, 0)?) == id(&before),
                        "ancestor replaced during traversal"
                    );
                    Ok(())
                })?;
            }
            if remove {
                #[cfg(test)]
                test_event(
                    "before_unlink",
                    &fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))?,
                )?;
                validate()?;
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
impl OwnedRunDir {
    /// Keep execution available where cleanup cannot prove mount identities.
    /// This handle deliberately carries no removal authority.
    pub(crate) fn create(root: &Path, leaf: &str, run_id: &str) -> Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        valid_leaf(leaf)?;
        ensure!(!run_id.is_empty(), "empty run identifier");
        let path = fs::canonicalize(root)?.join(leaf);
        if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&path) {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                bail!("per-run folder {path:?} already exists");
            }
            bail!("per-run folder {path:?} could not be created: {error}");
        }
        let mut record = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path.join(MARKER))?;
        record.write_all(
            br#"{"schema_version":0,"cleanup":"preserved: mount identity unavailable"}"#,
        )?;
        tracing::warn!(
            ?path,
            "run directory will be preserved: mount identity unavailable"
        );
        Ok(Self { path })
    }
    pub(crate) fn create_selected(root: &RunRoot, leaf: &str, run_id: &str) -> Result<Self> {
        Self::create(&root.path, leaf, run_id)
    }
    pub(crate) fn load(_root: &Path, _path: &Path, _run_id: &str) -> Result<Self> {
        bail!("owned cleanup unavailable: Linux mount identity support required")
    }
    pub(crate) fn remove(&self) -> CleanupOutcome {
        CleanupOutcome::Unsupported(
            "Linux descriptor and mount identity support unavailable".into(),
        )
    }
}

#[cfg(all(test, target_os = "linux"))]
type TestHook = Box<dyn FnMut(&str, &Path) -> Result<()>>;
#[cfg(all(test, target_os = "linux"))]
thread_local! {
    static TEST_HOOK: std::cell::RefCell<Option<TestHook>> = const { std::cell::RefCell::new(None) };
}
#[cfg(all(test, target_os = "linux"))]
fn test_event(event: &str, path: &Path) -> Result<()> {
    TEST_HOOK.with(|hook| match hook.borrow_mut().as_mut() {
        Some(hook) => hook(event, path),
        None => Ok(()),
    })
}
#[cfg(all(test, target_os = "linux"))]
#[path = "tests_owned_faults.rs"]
mod tests_faults;
