//! Identity-bound external launcher paths; resolution never creates directories.
use crate::workspace_validation::Workspace;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LauncherState {
    pub directory: PathBuf,
    pub shared_directory: PathBuf,
    pub checkout: PathBuf,
    pub common: PathBuf,
}

/// Resolve existing ancestors, including symlinks, before checking containment.
pub fn physical(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("external runtime path must be absolute and contain no parent traversal");
    }
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    if path.is_symlink() {
        bail!("dangling runtime symlink: {}", path.display());
    }
    let parent = path
        .parent()
        .context("runtime path has no existing ancestor")?;
    Ok(physical(parent)?.join(path.file_name().context("missing runtime component")?))
}

fn outside(workspace: &Workspace, path: &Path) -> Result<()> {
    let path = physical(path)?;
    if path.starts_with(workspace.common()) {
        bail!("launcher runtime resolves inside Git metadata");
    }
    let listing = crate::workspace_validation::bytes(
        workspace.root(),
        &["worktree", "list", "--porcelain", "-z"],
    )?;
    for entry in listing.split(|b| *b == 0) {
        if let Some(name) = entry.strip_prefix(b"worktree ") {
            let name =
                std::str::from_utf8(name).context("unsupported registered checkout filename")?;
            if path.starts_with(physical(Path::new(name))?) {
                bail!("launcher runtime resolves inside a registered checkout");
            }
        }
    }
    Ok(())
}

/// Validate another launcher-owned storage location without creating it.
/// Ancillary state must obey the same registered-checkout boundary.
pub fn validate_external(root: &Path, path: &Path) -> Result<PathBuf> {
    let workspace = Workspace::open(root)?;
    let path = physical(path)?;
    outside(&workspace, &path)?;
    Ok(path)
}

pub fn resolve(root: &Path) -> Result<LauncherState> {
    let workspace = Workspace::open(root)?;
    let base =
        if let Some(value) = std::env::var_os("AMPLIHACK_RUNTIME_DIR").filter(|v| !v.is_empty()) {
            PathBuf::from(value)
        } else {
            amplihack_types::paths::amplihack_state_root()
                .context("no external amplihack state root")?
                .join("runtime/launchers")
        };
    let base = physical(&base)?;
    let key = |path: &Path| format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()));
    let clone = base.join(key(workspace.common()));
    let directory = clone.join(key(workspace.root()));
    let shared_directory = clone.join("shared");
    // Computed descendants need checking too, before any mkdir follows them.
    for path in [&base, &clone, &directory, &shared_directory] {
        outside(&workspace, path)?;
        if physical(path)? != *path {
            bail!("substituted external runtime descendant");
        }
    }
    Ok(LauncherState {
        directory,
        shared_directory,
        checkout: workspace.root().to_owned(),
        common: workspace.common().to_owned(),
    })
}

/// Walk from a directory handle so a substituted ancestor cannot redirect mkdir.
#[cfg(unix)]
fn private_directory(path: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    let mut current: OwnedFd = fs::File::open("/")?.into();
    let components: Vec<_> = path
        .components()
        .filter_map(|c| {
            if let Component::Normal(c) = c {
                Some(c)
            } else {
                None
            }
        })
        .collect();
    for (position, component) in components.iter().enumerate() {
        let name = CString::new(component.as_bytes())?;
        // SAFETY: names are terminated, descriptors are owned, and no link is followed.
        let open = || unsafe {
            libc::openat(
                current.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        let mut fd = open();
        if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            // SAFETY: as above. A concurrent creator is validated by openat below.
            let result = unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o700) };
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error())
                    .context("cannot create private runtime directory");
            }
            fd = open();
        }
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("unsafe external runtime directory");
        }
        // SAFETY: successful openat returns a new descriptor owned here.
        current = unsafe { OwnedFd::from_raw_fd(fd) };
        // Only tighten the clone and leaf directories; never chmod shared ancestors.
        if position + 2 >= components.len() {
            let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: fstat initialises metadata for the held directory descriptor.
            if unsafe { libc::fstat(current.as_raw_fd(), metadata.as_mut_ptr()) } < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("cannot inspect runtime owner");
            }
            // SAFETY: fstat succeeded, and geteuid has no preconditions.
            if unsafe { metadata.assume_init().st_uid != libc::geteuid() } {
                bail!("external runtime directory belongs to another user");
            }
            // SAFETY: descriptor is a validated directory, still held open.
            if unsafe { libc::fchmod(current.as_raw_fd(), 0o700) } < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("cannot protect runtime directory");
            }
        }
    }
    Ok(())
}

pub fn prepare(root: &Path) -> Result<LauncherState> {
    let state = resolve(root)?;
    for directory in [&state.directory, &state.shared_directory] {
        #[cfg(unix)]
        private_directory(directory)?;
        #[cfg(not(unix))]
        fs::create_dir_all(directory)?;
    }
    let checked = resolve(root)?;
    if checked.directory != state.directory || checked.shared_directory != state.shared_directory {
        bail!("external launcher state changed while preparing it");
    }
    Ok(state)
}
