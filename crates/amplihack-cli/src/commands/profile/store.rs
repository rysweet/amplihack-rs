//! The profile root directory and the profiles inside it.

use super::name::ProfileName;
use super::{ProfileError, pointer};
use fs4::fs_std::FileExt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub use amplihack_utils::rotator_status::ROOT_ENV;
const SHARED_ASSET_DIRS: &[&str] = &["agents", "commands", "output-styles", "rules", "skills"];
const MAX_COPY_DEPTH: usize = 16;
const MAX_COPY_ENTRIES: usize = 5_000;
const MAX_COPY_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
const SHARED_ASSET_FILES: &[&str] = &["CLAUDE.md"];

fn copy_limited<R: Read, W: Write>(input: R, output: &mut W) -> Result<(), ProfileError> {
    let copied = std::io::copy(&mut input.take(MAX_COPY_FILE_BYTES + 1), output)?;
    if copied > MAX_COPY_FILE_BYTES {
        return Err(ProfileError::SharedAssetLimit(format!(
            "file larger than {MAX_COPY_FILE_BYTES} bytes"
        )));
    }
    Ok(())
}
pub(super) const SHARED_SETTINGS_KEYS: &[&str] = &[
    "alwaysThinkingEnabled",
    "attribution",
    "cleanupPeriodDays",
    "hooks",
    "includeCoAuthoredBy",
    "language",
    "model",
    "outputStyle",
    "permissions",
    "respectGitignore",
    "spinnerVerbs",
    "statusLine",
];

#[derive(Debug, Clone)]
pub struct ProfileStore {
    root: PathBuf,
    shared_assets: Option<PathBuf>,
}

impl ProfileStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            shared_assets: None,
        }
    }

    /// Use a fixture or user Claude config as the source of allowlisted assets.
    pub fn with_shared_assets(mut self, source: PathBuf) -> Self {
        self.shared_assets = Some(source);
        self
    }

    #[cfg(test)]
    pub fn from_root_str(raw: &str) -> Result<Self, ProfileError> {
        let path = PathBuf::from(raw);
        if raw.is_empty() || !path.is_absolute() {
            return Err(ProfileError::InvalidRoot(raw.to_string()));
        }
        Ok(Self::new(path))
    }

    /// The store for this process's environment; see `root_config`.
    pub fn from_env() -> Result<Self, ProfileError> {
        super::root_config::store_from_env().map(|(store, _)| store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The Claude config dir assets are seeded from, also the canonical root
    /// of the shared session store (issue #1634). `None` without a HOME.
    pub(super) fn shared_assets(&self) -> Option<&Path> {
        self.shared_assets.as_deref()
    }

    /// Non-creating check: a missing root is fine, a symlinked one is not.
    pub fn check_root_not_symlink(&self) -> Result<(), ProfileError> {
        check_root_not_symlink(&self.root)
    }

    fn ensure_root(&self) -> Result<(), ProfileError> {
        match fs::symlink_metadata(&self.root) {
            Ok(m) if m.file_type().is_symlink() => Err(ProfileError::SymlinkRefused(
                self.root.display().to_string(),
            )),
            Ok(m) if m.is_dir() => Ok(()),
            Ok(_) => Err(ProfileError::InvalidRoot("not a directory".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = self.root.parent() {
                    fs::create_dir_all(parent)?;
                }
                match private_dir_builder().create(&self.root) {
                    Ok(()) => Ok(()),
                    // Another process may have created the root after the first check.
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        match fs::symlink_metadata(&self.root) {
                            Ok(metadata)
                                if metadata.is_dir() && !metadata.file_type().is_symlink() =>
                            {
                                Ok(())
                            }
                            _ => Err(ProfileError::InvalidRoot(
                                "profile root changed during creation".into(),
                            )),
                        }
                    }
                    Err(error) => Err(error.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn add(&self, name: &ProfileName) -> Result<PathBuf, ProfileError> {
        self.check_root_not_symlink()?;
        if let Some(source) = &self.shared_assets {
            reject_root_inside_source(&self.root, source)?;
        }
        self.ensure_root()?;
        let _lock = add_lock(&self.root)?;
        for existing in self.list()? {
            if ProfileName::parse(&existing).is_ok_and(|e| e.eq_fold(name)) {
                return Err(ProfileError::AlreadyExists(name.to_string()));
            }
        }
        let dir = self.root.join(name.as_str());
        match fs::symlink_metadata(&dir) {
            Ok(_) => return Err(ProfileError::AlreadyExists(name.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let staging = create_staging_dir(&self.root, name)?;
        let result = if let Some(source) = &self.shared_assets {
            seed_shared_assets(source, &staging)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            return Err(discard_staging(&staging, error));
        }
        if let Err(error) = rename_noreplace(&staging, &dir) {
            let error = if error.kind() == std::io::ErrorKind::AlreadyExists {
                ProfileError::AlreadyExists(name.to_string())
            } else {
                ProfileError::Io(error)
            };
            return Err(discard_staging(&staging, error));
        }
        // Issue #1634: link the session dirs to the shared store. Never fails.
        if let Some(source) = &self.shared_assets {
            super::shared_dirs::after_add(&dir, name, source);
        }
        Ok(dir)
    }

    pub fn list(&self) -> Result<Vec<String>, ProfileError> {
        self.check_root_not_symlink()?;
        let entries = match fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            // DirEntry::file_type does not follow symlinks.
            if entry.file_type()?.is_dir() && ProfileName::parse(&name).is_ok() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    /// Verifies the profile is a real directory directly inside the root and
    /// returns its canonical path.
    pub fn resolve(&self, name: &ProfileName) -> Result<PathBuf, ProfileError> {
        self.check_root_not_symlink()?;
        let path = self.root.join(name.as_str());
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(ProfileError::SymlinkRefused(name.to_string()));
            }
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(ProfileError::NotFound(name.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ProfileError::NotFound(name.to_string()));
            }
            Err(e) => return Err(e.into()),
        }
        let canonical = path.canonicalize()?;
        let root = self.root.canonicalize()?;
        if canonical.parent() != Some(root.as_path()) {
            return Err(ProfileError::SymlinkRefused(name.to_string()));
        }
        Ok(canonical)
    }

    /// Spawn-time check: re-resolves the profile and additionally requires the
    /// directory to be owned by the current user with no group/other mode bits.
    #[cfg(unix)]
    pub fn verify_profile_dir(&self, name: &ProfileName) -> Result<PathBuf, ProfileError> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let canonical = self.resolve(name)?;
        let meta = fs::symlink_metadata(&canonical)?;
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != uid {
            return Err(ProfileError::UnsafePermissions(name.to_string()));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(ProfileError::UnsafePermissions(format!(
                "{name} (group/other mode bits)"
            )));
        }
        Ok(canonical)
    }

    #[cfg(not(unix))]
    pub fn verify_profile_dir(&self, name: &ProfileName) -> Result<PathBuf, ProfileError> {
        self.resolve(name)
    }

    pub fn remove(&self, name: &ProfileName, yes: bool) -> Result<(), ProfileError> {
        if !yes {
            return Err(ProfileError::ConfirmationRequired);
        }
        let dir = self.resolve(name)?;
        // Validate the pointer before deleting anything. Treating a corrupt
        // pointer as "not the default" could delete its target and leave stale
        // state behind while reporting success.
        let was_default = pointer::read(&self.root)?.as_ref() == Some(name);
        fs::remove_dir_all(dir)?;
        if was_default {
            pointer::clear(&self.root)?;
        }
        Ok(())
    }
}

pub(super) fn check_root_not_symlink(root: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(root) {
        Ok(m) if m.file_type().is_symlink() => {
            Err(ProfileError::SymlinkRefused(root.display().to_string()))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub(super) fn private_dir_builder() -> fs::DirBuilder {
    let mut b = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b
}

fn add_lock(root: &Path) -> Result<fs::File, ProfileError> {
    let path = root.join(".add.lock");
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(ProfileError::InvalidRoot(
            "profile add lock is not a regular file".into(),
        ));
    }
    file.lock_exclusive()?;
    Ok(file)
}

fn create_staging_dir(root: &Path, name: &ProfileName) -> Result<PathBuf, ProfileError> {
    static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            ".seed-{}-{}-{id}",
            name.as_str(),
            std::process::id()
        ));
        match private_dir_builder().create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ProfileError::InvalidRoot(
        "unable to allocate a private staging directory".into(),
    ))
}

fn discard_staging(path: &Path, seed_error: ProfileError) -> ProfileError {
    match fs::remove_dir_all(path) {
        Ok(()) => seed_error,
        Err(cleanup_error) => ProfileError::SeedCleanupFailed {
            path: path.display().to_string(),
            seed_error: seed_error.to_string(),
            cleanup_error: cleanup_error.to_string(),
        },
    }
}

pub(super) fn reject_root_inside_source(root: &Path, source: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ProfileError::SymlinkRefused(source.display().to_string()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let source = canonicalize_with_missing(source)?;
    let root = canonicalize_with_missing(root)?;
    if root.starts_with(&source) {
        return Err(ProfileError::SharedSourceContainsRoot(
            root.display().to_string(),
        ));
    }
    Ok(())
}

fn canonicalize_with_missing(path: &Path) -> Result<PathBuf, ProfileError> {
    let mut existing = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(&existing) {
            Ok(_) => {
                let mut resolved = existing.canonicalize()?;
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else {
                    return Err(ProfileError::InvalidRoot(
                        "cannot resolve profile path".into(),
                    ));
                };
                missing.push(name.to_os_string());
                let Some(parent) = existing.parent() else {
                    return Err(ProfileError::InvalidRoot(
                        "cannot resolve profile path".into(),
                    ));
                };
                existing = parent.to_path_buf();
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(super) fn rename_noreplace(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: both paths are valid NUL-terminated strings; RENAME_NOREPLACE
        // prevents replacing any concurrently-created destination.
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: both paths are valid NUL-terminated strings; RENAME_EXCL
        // prevents replacing any concurrently-created destination.
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        // Windows MoveFile semantics fail when the destination already exists.
        fs::rename(source, destination)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (source, destination);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic no-replace directory rename is unsupported on this platform",
        ))
    }
}

/// Swaps two existing names atomically (`renameat2(RENAME_EXCHANGE)`).
/// `Ok(false)` when this system or filesystem cannot, with nothing moved.
#[cfg(unix)]
pub(super) fn exchange(a: &Path, b: &Path) -> std::io::Result<bool> {
    #[cfg(test)]
    if test_hook::no_exchange() {
        return Ok(false);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let cstr = |p: &Path| {
            std::ffi::CString::new(p.as_os_str().as_bytes())
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
        };
        let (a, b) = (cstr(a)?, cstr(b)?);
        // SAFETY: both paths are valid NUL-terminated strings; RENAME_EXCHANGE
        // swaps two existing names and creates or removes nothing.
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                a.as_ptr(),
                libc::AT_FDCWD,
                b.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if rc == 0 {
            return Ok(true);
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINVAL | libc::ENOSYS) => Ok(false),
            _ => Err(e),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (a, b);
        Ok(false)
    }
}

/// [`rename_noreplace`] relative to two directory descriptors, so neither
/// parent path is resolved again.
#[cfg(unix)]
pub(super) fn renameat_noreplace(
    from_dir: std::os::fd::RawFd,
    from: &std::ffi::OsStr,
    to_dir: std::os::fd::RawFd,
    to: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let cstr = |s: &std::ffi::OsStr| {
        std::ffi::CString::new(s.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    };
    let (from, to) = (cstr(from)?, cstr(to)?);
    #[cfg(target_os = "linux")]
    // SAFETY: valid descriptors and NUL-terminated names; RENAME_NOREPLACE
    // never replaces an existing destination.
    let rc = unsafe {
        libc::renameat2(
            from_dir,
            from.as_ptr(),
            to_dir,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    // SAFETY: valid descriptors and NUL-terminated names; RENAME_EXCL never
    // replaces an existing destination.
    let rc = unsafe {
        libc::renameatx_np(
            from_dir,
            from.as_ptr(),
            to_dir,
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let rc: libc::c_int = {
        let _ = (from_dir, &from, to_dir, &to);
        errno::set_errno(errno::Errno(libc::ENOSYS));
        -1
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Unix: the source root is opened once with no-follow and every later open is
/// relative to that descriptor, so a root swapped for a symlink afterwards is
/// never followed. Only the final component of `source` is no-follow; symlinked
/// ancestors are resolved once, at this open.
#[cfg(unix)]
fn seed_shared_assets(source: &Path, destination: &Path) -> Result<(), ProfileError> {
    let Some(root) = fdwalk::open_root(source)? else {
        return Ok(());
    };
    #[cfg(test)]
    test_hook::fire_after_root_anchor(source);

    let mut entries = 0usize;
    for name in SHARED_ASSET_DIRS {
        copy_allowlisted_tree(&root, name, &destination.join(name), 0, &mut entries)?;
    }
    let dst = fdwalk::open_dir(libc::AT_FDCWD, destination)?
        .ok_or_else(|| ProfileError::SymlinkRefused(destination.display().to_string()))?;
    for name in SHARED_ASSET_FILES {
        fdwalk::copy_root_file(&root, &dst, name)?;
    }
    if let Some(raw) = fdwalk::read_root_file(&root, "settings.json", MAX_SETTINGS_BYTES)? {
        write_filtered_settings(&raw, &destination.join("settings.json"))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn seed_shared_assets(source: &Path, destination: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(source) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(ProfileError::SymlinkRefused(source.display().to_string()));
        }
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(ProfileError::InvalidSharedSource(
                source.display().to_string(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }

    let mut entries = 0usize;
    for name in SHARED_ASSET_DIRS {
        copy_allowlisted_tree(&source.join(name), &destination.join(name), 0, &mut entries)?;
    }
    for name in SHARED_ASSET_FILES {
        copy_regular_file(&source.join(name), &destination.join(name))?;
    }
    copy_filtered_settings(
        &source.join("settings.json"),
        &destination.join("settings.json"),
    )
}

/// Copies only regular files and directories. Symlinks and special files are
/// skipped so shared assets cannot pull credentials or arbitrary paths in.
#[cfg(not(unix))]
fn copy_allowlisted_tree(
    source: &Path,
    destination: &Path,
    depth: usize,
    entries: &mut usize,
) -> Result<(), ProfileError> {
    if depth > MAX_COPY_DEPTH {
        return Err(ProfileError::SharedAssetLimit(format!(
            "directory nesting deeper than {MAX_COPY_DEPTH}"
        )));
    }
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    private_dir_builder().create(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        *entries += 1;
        if *entries > MAX_COPY_ENTRIES {
            return Err(ProfileError::SharedAssetLimit(format!(
                "more than {MAX_COPY_ENTRIES} entries"
            )));
        }
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_allowlisted_tree(&source_path, &destination_path, depth + 1, entries)?;
        } else if kind.is_file() {
            copy_regular_file(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

/// Unix variant: every directory is opened with `O_NOFOLLOW|O_DIRECTORY` and
/// entries are read, classified and opened relative to that descriptor, so a
/// path component swapped for a symlink mid-copy is never followed.
#[cfg(unix)]
fn copy_allowlisted_tree(
    root: &std::os::fd::OwnedFd,
    name: &str,
    destination: &Path,
    depth: usize,
    entries: &mut usize,
) -> Result<(), ProfileError> {
    use std::os::fd::AsRawFd;
    let Some(src) = fdwalk::open_dir(root.as_raw_fd(), Path::new(name))? else {
        return Ok(());
    };
    private_dir_builder().create(destination)?;
    let dst = fdwalk::open_dir(libc::AT_FDCWD, destination)?
        .ok_or_else(|| ProfileError::SymlinkRefused(destination.display().to_string()))?;
    fdwalk::copy_dir(&src, &dst, Path::new(name), depth, entries)
}

#[cfg(all(unix, test))]
pub(super) mod test_hook {
    use std::cell::RefCell;
    use std::path::Path;

    type Hook = Box<dyn Fn(&Path)>;
    thread_local! {
        static BEFORE_OPEN: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    thread_local! {
        static AFTER_ROOT_ANCHOR: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub fn set_after_root_anchor(hook: Option<Hook>) {
        AFTER_ROOT_ANCHOR.with(|h| *h.borrow_mut() = hook);
    }

    pub(super) fn fire_after_root_anchor(path: &Path) {
        AFTER_ROOT_ANCHOR.with(|h| {
            if let Some(hook) = h.borrow().as_ref() {
                hook(path);
            }
        });
    }

    pub fn set_before_open(hook: Option<Hook>) {
        BEFORE_OPEN.with(|h| *h.borrow_mut() = hook);
    }

    pub(super) fn fire(path: &Path) {
        BEFORE_OPEN.with(|h| {
            if let Some(hook) = h.borrow().as_ref() {
                hook(path);
            }
        });
    }

    thread_local! {
        static NO_EXCHANGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Makes [`super::exchange`] report "unsupported" on this thread.
    pub fn set_no_exchange(on: bool) {
        NO_EXCHANGE.with(|c| c.set(on));
    }

    pub(super) fn no_exchange() -> bool {
        NO_EXCHANGE.with(std::cell::Cell::get)
    }
}

#[cfg(unix)]
pub(super) mod fdwalk {
    use super::{MAX_COPY_DEPTH, MAX_COPY_ENTRIES, ProfileError};
    use std::ffi::{CString, OsStr, OsString};
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;

    pub(in crate::commands::profile) fn cstr(name: &OsStr) -> std::io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| std::io::ErrorKind::InvalidInput.into())
    }

    pub(in crate::commands::profile) fn openat(
        dir: RawFd,
        name: &OsStr,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> std::io::Result<OwnedFd> {
        let c = cstr(name)?;
        // SAFETY: `c` is a valid NUL-terminated string; the result is checked.
        let fd = unsafe {
            libc::openat(
                dir,
                c.as_ptr(),
                flags | libc::O_CLOEXEC,
                mode as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a freshly opened descriptor that we own.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn skippable(e: &std::io::Error) -> bool {
        matches!(
            e.raw_os_error(),
            Some(libc::ELOOP | libc::ENOTDIR | libc::ENOENT)
        )
    }

    /// The only absolute-path source open. `Ok(None)` when the root is absent.
    pub(super) fn open_root(path: &Path) -> Result<Option<OwnedFd>, ProfileError> {
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY;
        match openat(libc::AT_FDCWD, path.as_os_str(), flags, 0) {
            Ok(fd) => Ok(Some(fd)),
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => Ok(None),
            Err(e) if matches!(e.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) => {
                // Error classification only; no data is read through this path.
                match std::fs::symlink_metadata(path) {
                    Ok(m) if m.file_type().is_symlink() => {
                        Err(ProfileError::SymlinkRefused(path.display().to_string()))
                    }
                    Ok(_) => Err(ProfileError::InvalidSharedSource(
                        path.display().to_string(),
                    )),
                    Err(m) if m.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(m) => Err(m.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Opens a root-relative regular file; `None` if absent, a symlink or special.
    fn open_regular(root: &OwnedFd, name: &str) -> Result<Option<File>, ProfileError> {
        debug_assert!(!name.is_empty() && !name.contains('/') && name != "..");
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        let input = match openat(root.as_raw_fd(), OsStr::new(name), flags, 0) {
            Ok(fd) => File::from(fd),
            Err(e) if skippable(&e) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(input.metadata()?.file_type().is_file().then_some(input))
    }

    pub(super) fn copy_root_file(
        root: &OwnedFd,
        dst: &OwnedFd,
        name: &str,
    ) -> Result<(), ProfileError> {
        match open_regular(root, name)? {
            Some(input) => write_file(input, dst, OsStr::new(name)),
            None => Ok(()),
        }
    }

    pub(super) fn read_root_file(
        root: &OwnedFd,
        name: &str,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, ProfileError> {
        let Some(input) = open_regular(root, name)? else {
            return Ok(None);
        };
        let mut raw = Vec::new();
        input.take(limit + 1).read_to_end(&mut raw)?;
        if raw.len() as u64 > limit {
            return Err(ProfileError::SharedAssetLimit(format!(
                "{name} larger than {limit} bytes"
            )));
        }
        Ok(Some(raw))
    }

    /// `Ok(None)` when the path is absent, not a directory, or a symlink.
    pub(super) fn open_dir(dir: RawFd, path: &Path) -> Result<Option<OwnedFd>, ProfileError> {
        match openat(
            dir,
            path.as_os_str(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            0,
        ) {
            Ok(fd) => Ok(Some(fd)),
            Err(e) if skippable(&e) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn file_type(dir: RawFd, name: &OsStr) -> std::io::Result<libc::mode_t> {
        let c = cstr(name)?;
        let mut st = std::mem::MaybeUninit::<libc::stat>::zeroed();
        // SAFETY: valid path and out-pointer; initialised only on success.
        let rc =
            unsafe { libc::fstatat(dir, c.as_ptr(), st.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: fstatat succeeded.
        Ok(unsafe { st.assume_init() }.st_mode & libc::S_IFMT)
    }

    pub(in crate::commands::profile) fn list(dir: &OwnedFd) -> std::io::Result<Vec<OsString>> {
        // SAFETY: dup returns a new descriptor or -1.
        let dup = unsafe { libc::dup(dir.as_raw_fd()) };
        if dup < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `dup` is valid; on success the DIR* owns it.
        let handle = unsafe { libc::fdopendir(dup) };
        if handle.is_null() {
            let e = std::io::Error::last_os_error();
            // SAFETY: fdopendir failed, so `dup` is still ours to close.
            unsafe { libc::close(dup) };
            return Err(e);
        }
        let mut names = Vec::new();
        let result = loop {
            // `readdir` only reports EOF versus error through errno. Use the
            // portable Unix accessor instead of maintaining OS-specific errno
            // symbols here.
            errno::set_errno(errno::Errno(0));
            // SAFETY: `handle` is a live DIR*.
            let ent = unsafe { libc::readdir(handle) };
            if ent.is_null() {
                let e = std::io::Error::last_os_error();
                break if e.raw_os_error() == Some(0) {
                    Ok(())
                } else {
                    Err(e)
                };
            }
            // SAFETY: d_name is a NUL-terminated string inside a live dirent.
            let name = unsafe { std::ffi::CStr::from_ptr((*ent).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                names.push(OsString::from_vec(name.to_vec()));
            }
        };
        // SAFETY: closes the DIR* and its dup'd descriptor exactly once.
        unsafe { libc::closedir(handle) };
        result.map(|()| names)
    }

    pub(super) fn copy_dir(
        src: &OwnedFd,
        dst: &OwnedFd,
        shown: &Path,
        depth: usize,
        entries: &mut usize,
    ) -> Result<(), ProfileError> {
        if depth > MAX_COPY_DEPTH {
            return Err(ProfileError::SharedAssetLimit(format!(
                "directory nesting deeper than {MAX_COPY_DEPTH}"
            )));
        }
        for name in list(src)? {
            *entries += 1;
            if *entries > MAX_COPY_ENTRIES {
                return Err(ProfileError::SharedAssetLimit(format!(
                    "more than {MAX_COPY_ENTRIES} entries"
                )));
            }
            let path = shown.join(&name);
            let kind = match file_type(src.as_raw_fd(), &name) {
                Ok(k) => k,
                Err(e) if skippable(&e) => continue,
                Err(e) => return Err(e.into()),
            };
            if kind != libc::S_IFDIR && kind != libc::S_IFREG {
                continue;
            }
            #[cfg(test)]
            super::test_hook::fire(&path);
            if kind == libc::S_IFDIR {
                copy_subdir(src, dst, &name, &path, depth, entries)?;
            } else {
                copy_file(src, dst, &name)?;
            }
        }
        Ok(())
    }

    fn copy_subdir(
        src: &OwnedFd,
        dst: &OwnedFd,
        name: &OsStr,
        path: &Path,
        depth: usize,
        entries: &mut usize,
    ) -> Result<(), ProfileError> {
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY;
        let child = match openat(src.as_raw_fd(), name, flags, 0) {
            Ok(fd) => fd,
            Err(e) if skippable(&e) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let c = cstr(name)?;
        // SAFETY: valid directory descriptor and path.
        if unsafe { libc::mkdirat(dst.as_raw_fd(), c.as_ptr(), 0o700) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let out = openat(dst.as_raw_fd(), name, flags, 0)?;
        copy_dir(&child, &out, path, depth + 1, entries)
    }

    fn copy_file(src: &OwnedFd, dst: &OwnedFd, name: &OsStr) -> Result<(), ProfileError> {
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        let input = match openat(src.as_raw_fd(), name, flags, 0) {
            Ok(fd) => File::from(fd),
            Err(e) if skippable(&e) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        // The opened descriptor, not the earlier path stat, decides the type.
        if !input.metadata()?.file_type().is_file() {
            return Ok(());
        }
        write_file(input, dst, name)
    }

    fn write_file(input: File, dst: &OwnedFd, name: &OsStr) -> Result<(), ProfileError> {
        let out_flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
        let mut output = File::from(openat(dst.as_raw_fd(), name, out_flags, 0o600)?);
        super::copy_limited(input, &mut output)
    }

    #[cfg(test)]
    mod tests {
        use super::{list, open_dir};

        #[test]
        fn empty_directory_eof_ignores_preexisting_errno() {
            let dir = tempfile::tempdir().unwrap();
            let fd = open_dir(libc::AT_FDCWD, dir.path()).unwrap().unwrap();
            errno::set_errno(errno::Errno(libc::EACCES));

            assert!(list(&fd).unwrap().is_empty());
        }
    }
}

#[cfg(not(unix))]
fn copy_regular_file(source: &Path, destination: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let mut input = open_source_nofollow(source)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(destination)?;
    copy_limited(input, &mut output)
}

/// Best-effort on non-Unix: no descriptor-relative open is available.
#[cfg(not(unix))]
fn open_source_nofollow(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        // Open the reparse point itself so a final-component swap cannot
        // redirect the path-based copy to an unrelated file.
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "shared asset changed to a non-regular file or reparse point",
            ));
        }
        return Ok(file);
    }
    #[cfg(not(windows))]
    options.open(path)
}

#[cfg(not(unix))]
fn copy_filtered_settings(source: &Path, destination: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let mut raw = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(open_source_nofollow(source)?, MAX_SETTINGS_BYTES + 1),
        &mut raw,
    )?;
    if raw.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(ProfileError::SharedAssetLimit(
            "settings.json larger than 1 MiB".into(),
        ));
    }
    write_filtered_settings(&raw, destination)
}

fn write_filtered_settings(raw: &[u8], destination: &Path) -> Result<(), ProfileError> {
    let value: serde_json::Value = serde_json::from_slice(raw)
        .map_err(|error| ProfileError::InvalidSharedSettings(error.to_string()))?;
    let serde_json::Value::Object(settings) = value else {
        return Err(ProfileError::InvalidSharedSettings(
            "expected a JSON object".into(),
        ));
    };
    let mut safe: serde_json::Map<String, serde_json::Value> = settings
        .into_iter()
        .filter(|(key, _)| SHARED_SETTINGS_KEYS.contains(&key.as_str()))
        .collect();
    // Issue #1699: never seed a missing or temp-dir hooks path.
    let target = destination.parent().unwrap_or(destination);
    for path in super::hook_path_guard::strip_unsafe_hooks(&mut safe, target) {
        eprintln!(
            "⚠️  New profile: left out amplihack hook {path:?} (missing, unsafe or temporary)"
        );
    }
    if safe.is_empty() {
        return Ok(());
    }
    let contents = serde_json::to_vec_pretty(&safe)
        .map_err(|error| ProfileError::InvalidSharedSettings(error.to_string()))?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(destination)?.write_all(&contents)?;
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::open_source_nofollow;
    use std::fs;

    #[test]
    fn final_file_symlink_is_rejected_by_opened_handle_check() {
        let dir = tempfile::tempdir().unwrap();
        let protected = dir.path().join("protected.txt");
        let link = dir.path().join("shared.md");
        fs::write(&protected, "must not be copied").unwrap();
        std::os::windows::fs::symlink_file(&protected, &link).unwrap();

        let error = open_source_nofollow(&link).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
