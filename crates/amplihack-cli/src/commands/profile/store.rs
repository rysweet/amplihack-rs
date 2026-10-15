//! The profile root directory and the profiles inside it.

use super::name::ProfileName;
use super::{ProfileError, pointer};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";
const SHARED_ASSET_DIRS: &[&str] = &["agents", "commands", "output-styles", "rules", "skills"];
const MAX_COPY_DEPTH: usize = 16;
const MAX_COPY_ENTRIES: usize = 5_000;
const MAX_COPY_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
const SHARED_ASSET_FILES: &[&str] = &["CLAUDE.md"];
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

    pub fn from_root_str(raw: &str) -> Result<Self, ProfileError> {
        let path = PathBuf::from(raw);
        if raw.is_empty() || !path.is_absolute() {
            return Err(ProfileError::InvalidRoot(raw.to_string()));
        }
        Ok(Self::new(path))
    }

    pub fn from_env() -> Result<Self, ProfileError> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        let root = if let Some(raw) = std::env::var_os(ROOT_ENV) {
            let raw = raw
                .into_string()
                .map_err(|_| ProfileError::InvalidRoot("non-UTF-8 path".into()))?;
            Self::from_root_str(&raw)?.root
        } else {
            home.as_ref()
                .ok_or_else(|| ProfileError::InvalidRoot("HOME is not set".into()))?
                .join(".amplihack")
                .join("claude-profiles")
        };
        let store = Self::new(root);
        Ok(match home {
            Some(home) => store.with_shared_assets(home.join(".claude")),
            None => store,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
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
                private_dir_builder().create(&self.root)?;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn add(&self, name: &ProfileName) -> Result<PathBuf, ProfileError> {
        self.ensure_root()?;
        for existing in self.list()? {
            if ProfileName::parse(&existing).is_ok_and(|e| e.eq_fold(name)) {
                return Err(ProfileError::AlreadyExists(name.to_string()));
            }
        }
        let dir = self.root.join(name.as_str());
        match private_dir_builder().create(&dir) {
            Ok(()) => {
                if let Some(source) = &self.shared_assets
                    && let Err(error) = seed_shared_assets(source, &dir)
                {
                    // A failed seed must not leave a profile that appears ready.
                    if let Err(cleanup) = fs::remove_dir_all(&dir) {
                        eprintln!(
                            "warning: profile rollback failed to remove {}: {cleanup}",
                            dir.display()
                        );
                    }
                    return Err(error);
                }
                Ok(dir)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(ProfileError::AlreadyExists(name.to_string()))
            }
            Err(e) => Err(e.into()),
        }
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
    /// directory to be owned by the current user and not group/world writable.
    #[cfg(unix)]
    pub fn verify_profile_dir(&self, name: &ProfileName) -> Result<PathBuf, ProfileError> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let canonical = self.resolve(name)?;
        let meta = fs::symlink_metadata(&canonical)?;
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != uid {
            return Err(ProfileError::SymlinkRefused(name.to_string()));
        }
        if meta.permissions().mode() & 0o022 != 0 {
            return Err(ProfileError::SymlinkRefused(format!(
                "{name} (group/world-writable)"
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
        let was_default = matches!(pointer::read(&self.root), Ok(Some(ref d)) if d == name);
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

fn private_dir_builder() -> fs::DirBuilder {
    let mut b = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b
}

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
    source: &Path,
    destination: &Path,
    depth: usize,
    entries: &mut usize,
) -> Result<(), ProfileError> {
    let Some(src) = fdwalk::open_dir(libc::AT_FDCWD, source)? else {
        return Ok(());
    };
    private_dir_builder().create(destination)?;
    let dst = fdwalk::open_dir(libc::AT_FDCWD, destination)?
        .ok_or_else(|| ProfileError::SymlinkRefused(destination.display().to_string()))?;
    fdwalk::copy_dir(&src, &dst, source, depth, entries)
}

#[cfg(all(unix, test))]
pub(super) mod test_hook {
    use std::cell::RefCell;
    use std::path::Path;

    type Hook = Box<dyn Fn(&Path)>;
    thread_local! {
        static BEFORE_OPEN: RefCell<Option<Hook>> = const { RefCell::new(None) };
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
}

#[cfg(unix)]
mod fdwalk {
    use super::{MAX_COPY_DEPTH, MAX_COPY_ENTRIES, MAX_COPY_FILE_BYTES, ProfileError};
    use std::ffi::{CString, OsStr, OsString};
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;

    fn cstr(name: &OsStr) -> std::io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| std::io::ErrorKind::InvalidInput.into())
    }

    fn openat(
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

    /// readdir signals errors only through errno, so it must be cleared first.
    fn clear_errno() {
        // SAFETY: errno is thread-local and always writable.
        unsafe {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                *libc::__errno_location() = 0;
            }
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
            {
                *libc::__error() = 0;
            }
        }
    }

    fn list(dir: &OwnedFd) -> std::io::Result<Vec<OsString>> {
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
            clear_errno();
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
        let out_flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
        let mut output = File::from(openat(dst.as_raw_fd(), name, out_flags, 0o600)?);
        let copied = std::io::copy(&mut input.take(MAX_COPY_FILE_BYTES + 1), &mut output)?;
        if copied > MAX_COPY_FILE_BYTES {
            return Err(ProfileError::SharedAssetLimit(format!(
                "file larger than {MAX_COPY_FILE_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

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
    std::io::copy(&mut input, &mut output)?;
    Ok(())
}

/// Opens without following a symlink swapped in after the metadata check, and
/// without blocking on a FIFO swapped in the same way.
fn open_source_nofollow(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

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
    let value: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|error| ProfileError::InvalidSharedSettings(error.to_string()))?;
    let serde_json::Value::Object(settings) = value else {
        return Err(ProfileError::InvalidSharedSettings(
            "expected a JSON object".into(),
        ));
    };
    let safe: serde_json::Map<String, serde_json::Value> = settings
        .into_iter()
        .filter(|(key, _)| SHARED_SETTINGS_KEYS.contains(&key.as_str()))
        .collect();
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
