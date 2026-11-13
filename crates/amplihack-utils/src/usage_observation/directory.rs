//! Descriptor-relative I/O: no managed directory or file is followed through
//! a symlink, and a replacement of a pathname cannot redirect an open store.
use crate::secure_files::{check_private_file, euid, invalid};
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

pub(in crate::usage_observation) struct Directory(pub File);
fn cstr(s: &[u8]) -> io::Result<CString> {
    CString::new(s).map_err(|_| invalid("NUL in managed pathname"))
}
fn owned_fd(fd: libc::c_int) -> io::Result<File> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    }
    // SAFETY: successful open/openat transferred ownership of a new fd.
    else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
impl Directory {
    pub fn open(path: &Path) -> io::Result<Self> {
        // Match the configured profile-root access boundary. Managed state
        // below it is still owner-only; a readable root is not writable state.
        Self::open_checked(path, false)
    }
    pub fn open_refresh(path: &Path) -> io::Result<Self> {
        Self::open_checked(path, false)
    }
    fn open_checked(path: &Path, private: bool) -> io::Result<Self> {
        // Resolve system aliases such as macOS /tmp in the parent only;
        // the managed root itself must still be opened with O_NOFOLLOW.
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let parent = full
            .parent()
            .ok_or_else(|| invalid("managed root needs a parent"))?
            .canonicalize()?;
        let path = parent.join(
            full.file_name()
                .ok_or_else(|| invalid("managed root needs a name"))?,
        );
        let path = path.as_path();
        let start = if path.is_absolute() { "/" } else { "." };
        let start = cstr(start.as_bytes())?;
        // SAFETY: valid NUL-terminated path and flags, result is owned.
        let mut dir = Self(owned_fd(unsafe {
            libc::open(
                start.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        })?);
        for part in path.components() {
            match part {
                Component::RootDir | Component::CurDir => (),
                Component::Normal(name) => dir = dir.child(name.as_bytes(), false)?,
                _ => return Err(invalid("parent traversal in managed pathname")),
            }
        }
        if private {
            dir.private()?;
        } else {
            let m = dir.0.metadata()?;
            if !m.is_dir() || m.uid() != euid() || m.mode() & 0o022 != 0 {
                return Err(invalid(
                    "managed root must be owned and not writable by others",
                ));
            }
        }
        Ok(dir)
    }
    fn private(&self) -> io::Result<()> {
        let m = self.0.metadata()?;
        if !m.is_dir() || m.uid() != euid() || m.mode() & 0o077 != 0 {
            return Err(invalid(
                "managed directory must be owned and private (0700)",
            ));
        }
        Ok(())
    }
    fn child(&self, name: &[u8], create: bool) -> io::Result<Self> {
        let name = cstr(name)?;
        if create {
            // SAFETY: live directory fd, validated component and mode.
            if unsafe { libc::mkdirat(self.0.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::AlreadyExists {
                    return Err(e);
                }
            }
        }
        // SAFETY: live directory fd and valid path; O_NOFOLLOW forbids aliases.
        let dir = Self(owned_fd(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?);
        if create {
            dir.private()?;
        }
        Ok(dir)
    }
    pub fn managed(&self, name: &str) -> io::Result<Self> {
        if name.is_empty() || name.contains('/') {
            return Err(invalid("invalid managed component"));
        }
        self.child(name.as_bytes(), true)
    }
    pub fn file(&self, name: &str, create: bool) -> io::Result<File> {
        let name = cstr(name.as_bytes())?;
        let flags = if create {
            libc::O_RDWR | libc::O_CREAT
        } else {
            libc::O_RDONLY
        };
        // SAFETY: live fd, valid name, new fd owned by returned File.
        let file = owned_fd(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                0o600,
            )
        })?;
        check_private_file(&file.metadata()?)?;
        Ok(file)
    }
    pub fn read(&self, name: &str, bound: usize) -> io::Result<Option<Vec<u8>>> {
        let f = match self.file(name, false) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        f.take(bound as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > bound {
            return Err(invalid("managed record exceeds size bound"));
        }
        Ok(Some(bytes))
    }
    pub fn replace(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        match self.file(name, false) {
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        // Random exclusive same-directory temporary names also survive process
        // crashes without being mistaken for complete records on recovery.
        let mut random = [0u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let temp = cstr(format!(".pending-{:032x}", u128::from_ne_bytes(random)).as_bytes())?;
        let target = cstr(name.as_bytes())?;
        // SAFETY: directory and C strings are live, returned fd is owned.
        let mut f = owned_fd(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                temp.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        })?;
        let result = (|| {
            f.write_all(bytes)?;
            f.sync_all()?;
            // SAFETY: both names are relative to the same verified directory.
            if unsafe {
                libc::renameat(
                    self.0.as_raw_fd(),
                    temp.as_ptr(),
                    self.0.as_raw_fd(),
                    target.as_ptr(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            self.0.sync_all()
        })();
        if result.is_err() {
            // SAFETY: only our exclusive temporary file can have this name.
            if unsafe { libc::unlinkat(self.0.as_raw_fd(), temp.as_ptr(), 0) } != 0 {
                let cleanup = io::Error::last_os_error();
                if cleanup.kind() != io::ErrorKind::NotFound {
                    tracing::warn!("observation temporary-file cleanup failed: {cleanup}");
                }
            }
        }
        result
    }
}
