//! Credential movement stays relative to retained, verified directory handles.
//! Path checks detect substitution; they never supply rename authority.
use super::swap_authorize::refused;
use crate::commands::profile::{
    ProfileError,
    auto_switch::kit::{CREDS, euid, writable_by_others},
    store::renameat_noreplace,
};
use std::ffi::CString;
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Component, Path, PathBuf};

pub(super) struct Directory {
    file: File,
    path: PathBuf,
    id: (u64, u64),
    private: bool,
}

fn invalid() -> ProfileError {
    refused("credential directory binding changed or is unsafe; journal retained")
}

/// Apply the shared private-group/ACL rule to the retained descriptor, so a
/// substituted path cannot supply the ACL evidence for a different directory.
fn untrusted_writers(file: &File, meta: &std::fs::Metadata) -> bool {
    amplihack_utils::secure_files::dir_rule(meta.mode(), !writable_by_others(meta), || {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: retained descriptor and a constant NUL-terminated name;
            // a null buffer asks only for the attribute's size.
            let size = unsafe {
                libc::fgetxattr(
                    file.as_raw_fd(),
                    c"system.posix_acl_access".as_ptr(),
                    std::ptr::null_mut(),
                    0,
                )
            };
            if size < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = file;
            Err(std::io::ErrorKind::Unsupported.into())
        }
    })
}

pub(super) fn incomplete(error: ProfileError) -> ProfileError {
    ProfileError::RotateInconsistent(format!(
        "credential custody incomplete; journal retained: {error}"
    ))
}

impl Directory {
    /// Existing live directories permit readers, but only trusted writers.
    pub(super) fn open_login(
        path: &Path,
        expected: Option<(u64, u64)>,
    ) -> Result<Self, ProfileError> {
        Self::open_with_policy(path, expected, false)
    }

    /// Stored profiles and reserved custody directories remain owner-only.
    pub(super) fn open(path: &Path, expected: Option<(u64, u64)>) -> Result<Self, ProfileError> {
        Self::open_with_policy(path, expected, true)
    }

    /// Walk from `/` without following symlinks. System ancestors may be owned
    /// by root; only root's sticky temporary directories may be world writable.
    fn open_with_policy(
        path: &Path,
        expected: Option<(u64, u64)>,
        private: bool,
    ) -> Result<Self, ProfileError> {
        if !path.is_absolute() {
            return Err(invalid());
        }
        #[allow(unused_mut)]
        let mut walked = path.to_path_buf();
        // macOS exposes these immutable root-owned aliases. Resolve only that
        // system component, never an application-controlled ancestor.
        #[cfg(target_os = "macos")]
        for alias in ["/var", "/tmp", "/etc"] {
            if let Ok(tail) = path.strip_prefix(alias) {
                walked = Path::new("/private").join(&alias[1..]).join(tail);
                break;
            }
        }
        let mut file = File::open("/")?;
        for component in walked.components() {
            let Component::Normal(name) = component else {
                if component == Component::RootDir {
                    continue;
                }
                return Err(invalid());
            };
            let name = CString::new(name.as_bytes()).map_err(|_| invalid())?;
            // SAFETY: valid directory descriptor and a single terminated name.
            let fd = unsafe {
                libc::openat(
                    file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(invalid());
            }
            // SAFETY: openat returned a fresh owned descriptor.
            file = unsafe { File::from_raw_fd(fd) };
            let m = file.metadata()?;
            let uid = euid();
            if (m.uid() != uid && m.uid() != 0)
                || (untrusted_writers(&file, &m) && !(m.uid() == 0 && m.mode() & 0o1000 != 0))
            {
                return Err(invalid());
            }
        }
        let m = file.metadata()?;
        let id = (m.dev(), m.ino());
        if m.uid() != euid()
            || (private && m.mode() & 0o077 != 0)
            || untrusted_writers(&file, &m)
            || expected.is_some_and(|e| e != id)
        {
            return Err(invalid());
        }
        Ok(Self {
            file,
            path: path.to_owned(),
            id,
            private,
        })
    }

    pub(super) fn id(&self) -> (u64, u64) {
        self.id
    }

    pub(super) fn current(&self) -> Result<(), ProfileError> {
        Self::open_with_policy(&self.path, Some(self.id), self.private).map(|_| ())
    }

    fn credential(&self, expected: u64) -> Result<(), ProfileError> {
        let mut m = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: stat output is initialized only on success; name is constant.
        let rc = unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                c".credentials.json".as_ptr(),
                m.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc != 0 {
            return Err(invalid());
        }
        // SAFETY: fstatat succeeded and initialized the complete stat structure.
        let m = unsafe { m.assume_init() };
        if m.st_mode & libc::S_IFMT != libc::S_IFREG
            || m.st_uid != euid()
            || m.st_mode & 0o077 != 0
            || m.st_nlink != 1
            || m.st_ino != expected
        {
            return Err(invalid());
        }
        Ok(())
    }

    /// Atomic no-replace rename. Synchronization is a separate custody barrier.
    pub(super) fn move_to(&self, dst: &Self, inode: u64) -> Result<(), ProfileError> {
        self.current()?;
        dst.current()?;
        self.credential(inode)?;
        boundary("move");
        renameat_noreplace(
            self.file.as_raw_fd(),
            CREDS.as_ref(),
            dst.file.as_raw_fd(),
            CREDS.as_ref(),
        )?;
        self.current()
            .and_then(|_| dst.current())
            .and_then(|_| dst.credential(inode))
            .map_err(incomplete)
    }

    /// Movement only: a failed barrier must never cause a second exchange.
    pub(super) fn exchange(&self, dst: &Self, own: u64, other: u64) -> Result<(), ProfileError> {
        self.current()?;
        dst.current()?;
        self.credential(own)?;
        dst.credential(other)?;
        boundary("exchange");
        #[cfg(target_os = "linux")]
        {
            // SAFETY: retained directory descriptors and constant file names.
            let rc = unsafe {
                libc::renameat2(
                    self.file.as_raw_fd(),
                    c".credentials.json".as_ptr(),
                    dst.file.as_raw_fd(),
                    c".credentials.json".as_ptr(),
                    libc::RENAME_EXCHANGE,
                )
            };
            if rc != 0 {
                let error = io::Error::last_os_error();
                if matches!(
                    error.raw_os_error(),
                    Some(libc::EXDEV | libc::EINVAL | libc::ENOSYS)
                ) {
                    return Err(refused("atomic exchange is not supported here"));
                }
                return Err(error.into());
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            #[cfg(test)]
            let simulated = crate::commands::profile::auto_switch::kit::test_exchange::active();
            #[cfg(not(test))]
            let simulated = false;
            if !simulated {
                return Err(invalid());
            }
            #[cfg(test)]
            {
                // Portable placement simulation uses the same retained handles.
                let aside = std::ffi::OsStr::new(".test-exchange-aside");
                renameat_noreplace(
                    self.file.as_raw_fd(),
                    CREDS.as_ref(),
                    self.file.as_raw_fd(),
                    aside,
                )?;
                renameat_noreplace(
                    dst.file.as_raw_fd(),
                    CREDS.as_ref(),
                    self.file.as_raw_fd(),
                    CREDS.as_ref(),
                )?;
                renameat_noreplace(
                    self.file.as_raw_fd(),
                    aside,
                    dst.file.as_raw_fd(),
                    CREDS.as_ref(),
                )?;
            }
        }
        self.current()
            .and_then(|_| dst.current())
            .and_then(|_| self.credential(other))
            .and_then(|_| dst.credential(own))
            .map_err(incomplete)
    }
}

/// The one custody durability barrier, including recovery of completed moves.
/// All bindings must remain current before metadata or success can be published.
pub(super) fn durable(dirs: &[&Directory]) -> Result<(), ProfileError> {
    boundary("sync");
    #[cfg(test)]
    if FAIL_SYNC.with(std::cell::Cell::get) {
        return Err(ProfileError::RotateInconsistent(
            "credential directory sync failed; journal retained".into(),
        ));
    }
    for dir in dirs {
        dir.current().map_err(incomplete)?;
        dir.file.sync_all().map_err(|e| {
            ProfileError::RotateInconsistent(format!(
                "credential directory sync failed; journal retained: {e}"
            ))
        })?;
    }
    for dir in dirs {
        dir.current().map_err(incomplete)?;
    }
    Ok(())
}

fn boundary(point: &str) {
    #[cfg(test)]
    HOOK.with(|hook| {
        let hook = hook.borrow_mut().take();
        if let Some(mut hook) = hook {
            hook(point);
            HOOK.with(|h| *h.borrow_mut() = Some(hook));
        }
    });
    let _ = point;
}

#[cfg(test)]
type BoundaryHook = Box<dyn FnMut(&str)>;

#[cfg(test)]
thread_local! {
    pub(super) static FAIL_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(super) static HOOK: std::cell::RefCell<Option<BoundaryHook>> = const { std::cell::RefCell::new(None) };
}
