//! Pinned external runtime directories. All I/O is relative to the held handle.
use anyhow::{Context, Result, bail};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RuntimeDirectory(Arc<File>);

impl RuntimeDirectory {
    /// Open an existing absolute directory without following any symlink.
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            bail!("runtime directory requires an absolute path without traversal");
        }
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            use std::os::unix::ffi::OsStrExt;
            let mut directory = File::open("/")?;
            for component in path.components() {
                if let Component::Normal(name) = component {
                    let name = std::ffi::CString::new(name.as_bytes())?;
                    // SAFETY: the held descriptor and NUL-terminated name are valid.
                    let fd = unsafe {
                        libc::openat(
                            directory.as_raw_fd(),
                            name.as_ptr(),
                            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                        )
                    };
                    if fd < 0 {
                        return Err(std::io::Error::last_os_error())
                            .context("unsafe runtime directory traversal");
                    }
                    // SAFETY: openat returned a new owned descriptor.
                    directory = unsafe { File::from_raw_fd(fd) };
                }
            }
            Ok(Self(Arc::new(directory)))
        }
        #[cfg(not(unix))]
        bail!("descriptor-relative runtime storage is unsupported on this platform")
    }

    #[cfg(unix)]
    fn name(name: &str) -> Result<std::ffi::CString> {
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            bail!("runtime filename must be a single component");
        }
        Ok(std::ffi::CString::new(name)?)
    }

    #[cfg(unix)]
    fn open_file(&self, name: &str, flags: i32) -> Result<File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::fs::PermissionsExt;
        let name = Self::name(name)?;
        // Open existing files first. On APFS, concurrent non-exclusive
        // O_CREAT opens can report ENOENT while another thread creates the
        // lock. Exclusive creation makes that race explicit as EEXIST.
        let open = |options| unsafe {
            // SAFETY: the descriptor is held and the filename is NUL-terminated.
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                options | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600,
            )
        };
        let mut fd;
        if flags & libc::O_CREAT != 0 && flags & libc::O_EXCL == 0 {
            fd = open(flags & !libc::O_CREAT);
            if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                fd = open(flags | libc::O_EXCL);
                if fd < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists
                {
                    fd = open(flags & !libc::O_CREAT);
                }
            }
        } else {
            fd = open(flags);
        }
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("cannot open runtime file");
        }
        // SAFETY: openat returned a new owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            bail!("runtime file must be regular and singly linked");
        }
        if flags & (libc::O_WRONLY | libc::O_RDWR) != 0 {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(file)
    }

    pub fn append(&self, name: &str) -> Result<File> {
        #[cfg(unix)]
        {
            self.open_file(name, libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT)
        }
        #[cfg(not(unix))]
        bail!("descriptor-relative runtime storage unsupported")
    }

    pub fn lock_file(&self, name: &str) -> Result<File> {
        #[cfg(unix)]
        {
            self.open_file(name, libc::O_RDWR | libc::O_CREAT)
        }
        #[cfg(not(unix))]
        bail!("descriptor-relative runtime storage unsupported")
    }

    pub fn reader(&self, name: &str) -> Result<File> {
        #[cfg(unix)]
        {
            self.open_file(name, libc::O_RDONLY)
        }
        #[cfg(not(unix))]
        bail!("descriptor-relative runtime storage unsupported")
    }

    pub fn read(&self, name: &str) -> Result<String> {
        let mut text = String::new();
        self.reader(name)?.read_to_string(&mut text)?;
        Ok(text)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name = Self::name(name)?;
            // SAFETY: the name is a checked sibling of the held directory.
            if unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) } < 0 {
                return Err(std::io::Error::last_os_error()).context("cannot remove runtime file");
            }
            Ok(())
        }
        #[cfg(not(unix))]
        bail!("descriptor-relative runtime storage unsupported")
    }

    /// Create a private temporary file and atomically replace a sibling file.
    pub fn replace(&self, name: &str, content: &[u8]) -> Result<()> {
        self.replace_with(name, |file| file.write_all(content))
    }

    /// Replace a sibling file only when writing and syncing the new file succeed.
    pub fn replace_with(
        &self,
        name: &str,
        write: impl FnOnce(&mut File) -> std::io::Result<()>,
    ) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let destination = Self::name(name)?;
            let temporary = format!(
                ".runtime-{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let mut file =
                self.open_file(&temporary, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
            let source = Self::name(&temporary)?;
            let result = (|| -> Result<()> {
                write(&mut file)?;
                file.sync_all()?;
                // SAFETY: both names are checked siblings of the held directory.
                if unsafe {
                    libc::renameat(
                        self.0.as_raw_fd(),
                        source.as_ptr(),
                        self.0.as_raw_fd(),
                        destination.as_ptr(),
                    )
                } < 0
                {
                    return Err(std::io::Error::last_os_error())
                        .context("cannot replace runtime file");
                }
                Ok(())
            })();
            if result.is_err() {
                // SAFETY: cleanup is relative to the same pinned directory.
                unsafe {
                    libc::unlinkat(self.0.as_raw_fd(), source.as_ptr(), 0);
                }
            }
            result
        }
        #[cfg(not(unix))]
        bail!("descriptor-relative runtime storage unsupported")
    }
}

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
