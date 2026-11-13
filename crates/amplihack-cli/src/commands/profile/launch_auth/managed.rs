//! Local managed-policy sources. Never execute helpers or alter administrator
//! policy to make a fallback usable. Native and remote formats we cannot bind
//! are refused rather than treated as absent.

use super::REFUSED;
use std::path::Path;

pub(super) fn absent(path: &Path) -> Result<(), &'static str> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(
            "fallback authentication cannot be bound to native or cached managed policy: use an explicit profile selection or ask your administrator",
        ),
    }
}

pub(super) fn validate(isolated_root: Option<&Path>) -> Result<(), &'static str> {
    #[cfg(target_os = "linux")]
    unix::refuse_wsl(isolated_root)?;
    // The existing debug-only home guard redirects ALL machine-policy reads
    // to fixture-owned sources; no additional public bypass is introduced.
    if let Some(root) = isolated_root {
        #[cfg(unix)]
        {
            unix::files(&root.join("managed"))?;
            return unix::native_policy(&root.join("managed-preferences"));
        }
        #[cfg(not(unix))]
        return Err(REFUSED);
    }
    #[cfg(target_os = "linux")]
    return unix::files(Path::new("/etc/claude-code"));
    #[cfg(target_os = "macos")]
    {
        unix::files(Path::new("/Library/Application Support/ClaudeCode"))?;
        unix::native_policy(Path::new("/Library/Managed Preferences"))
    }
    // Windows registry policy is not covered by Unix file inspection.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    Err(REFUSED)
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;

    const MAX_ENTRIES: usize = 128;
    const MAX_BYTES: u64 = 1024 * 1024;

    #[cfg(target_os = "linux")]
    pub(super) fn refuse_wsl(isolated_root: Option<&Path>) -> Result<(), &'static str> {
        let path = isolated_root
            .map(|root| root.join("kernel-release"))
            .unwrap_or_else(|| "/proc/sys/kernel/osrelease".into());
        let release =
            super::super::super::fsx::read_bounded_nofollow(&path, 4096).map_err(|_| REFUSED)?;
        let release = match release {
            Some((bytes, _)) => String::from_utf8(bytes)
                .map_err(|_| REFUSED)?
                .to_ascii_lowercase(),
            None if isolated_root.is_some() => String::new(),
            None => return Err(REFUSED),
        };
        if release.contains("microsoft")
            || release.contains("wsl")
            || (isolated_root.is_none() && std::fs::symlink_metadata("/run/WSL").is_ok())
        {
            return Err(
                "fallback authentication cannot be bound to inherited Windows managed policy on WSL: use an explicit profile selection",
            );
        }
        Ok(())
    }

    pub(super) fn files(root: &Path) -> Result<(), &'static str> {
        super::super::file(&root.join("managed-settings.json"))?;
        let path = CString::new(root.join("managed-settings.d").as_os_str().as_bytes())
            .map_err(|_| REFUSED)?;
        // SAFETY: path is terminated; flags open only a directory, without
        // following a symlink. The resulting DIR owns the descriptor.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(REFUSED),
            };
        }
        // SAFETY: fd is valid; ownership transfers only on success.
        let handle = unsafe { libc::fdopendir(fd) };
        if handle.is_null() {
            // SAFETY: fdopendir failed, so fd is still ours.
            unsafe { libc::close(fd) };
            return Err(REFUSED);
        }
        struct Directory(*mut libc::DIR);
        impl Drop for Directory {
            fn drop(&mut self) {
                // SAFETY: this wrapper owns the live DIR and its descriptor.
                unsafe { libc::closedir(self.0) };
            }
        }
        let directory = Directory(handle);
        let mut entries = 0;
        let mut total = 0;
        loop {
            errno::set_errno(errno::Errno(0));
            // SAFETY: directory remains live throughout enumeration.
            let entry = unsafe { libc::readdir(directory.0) };
            if entry.is_null() {
                return if errno::errno().0 == 0 {
                    Ok(())
                } else {
                    Err(REFUSED)
                };
            }
            // SAFETY: readdir's name is terminated and valid until next call.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            let bytes = name.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            entries += 1;
            if entries > MAX_ENTRIES {
                return Err(REFUSED);
            }
            // Claude ignores hidden and non-JSON entries. Checking every
            // applicable file is conservative regardless of merge order.
            if bytes.starts_with(b".") || !bytes.ends_with(b".json") {
                continue;
            }
            // SAFETY: fd belongs to the live DIR; name is a single entry.
            let input = unsafe {
                libc::openat(
                    fd,
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if input < 0 {
                return Err(REFUSED);
            }
            // SAFETY: openat returned a descriptor now owned by File.
            let input = unsafe { File::from_raw_fd(input) };
            if !input.metadata().map_err(|_| REFUSED)?.is_file() {
                return Err(REFUSED);
            }
            let bytes =
                super::super::super::fsx::read_bounded(input, MAX_BYTES).map_err(|_| REFUSED)?;
            total += bytes.len();
            if bytes.len() as u64 > MAX_BYTES || total > 8 * MAX_BYTES as usize {
                return Err(REFUSED);
            }
            super::super::document(&bytes)?;
        }
    }

    /// Claude reads device and per-user plists for this domain. Refuse any
    /// such deployment, including other users', without guessing an account
    /// from HOME/USER or parsing native policy as JSON.
    pub(super) fn native_policy(root: &Path) -> Result<(), &'static str> {
        const DOMAIN: &str = "com.anthropic.claudecode.plist";
        absent(&root.join(DOMAIN))?;
        match std::fs::symlink_metadata(root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Ok(meta) if meta.is_dir() => (),
            _ => return Err(REFUSED),
        }
        for (index, entry) in std::fs::read_dir(root).map_err(|_| REFUSED)?.enumerate() {
            if index >= MAX_ENTRIES {
                return Err(REFUSED);
            }
            let entry = entry.map_err(|_| REFUSED)?;
            let kind = entry.file_type().map_err(|_| REFUSED)?;
            if kind.is_symlink() {
                return Err(REFUSED);
            }
            if kind.is_dir() {
                absent(&entry.path().join(DOMAIN))?;
            }
        }
        Ok(())
    }
}
