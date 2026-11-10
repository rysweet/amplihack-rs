//! Registered executable identities and copy-free staging.
use anyhow::{Context, Result, bail};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

pub trait LinkOps {
    fn hard_link(&self, src: &Path, dst: &Path) -> io::Result<()>;
    fn symlink(&self, src: &Path, dst: &Path) -> io::Result<()>;
}
pub struct FilesystemLinks;
impl LinkOps for FilesystemLinks {
    fn hard_link(&self, src: &Path, dst: &Path) -> io::Result<()> {
        fs::hard_link(src, dst)
    }
    fn symlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(src, dst)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(src, dst)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (src, dst);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "artifact symlinks unsupported",
            ))
        }
    }
}
#[derive(Default)]
pub struct ArtifactRegistry {
    sources: Vec<(PathBuf, Identity)>,
}

#[derive(PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
    size: u64,
    modified: std::time::SystemTime,
}

fn identity(m: &fs::Metadata) -> io::Result<Identity> {
    Ok(Identity {
        #[cfg(unix)]
        device: m.dev(),
        #[cfg(unix)]
        inode: m.ino(),
        #[cfg(unix)]
        mode: m.permissions().mode(),
        size: m.len(),
        modified: m.modified()?,
    })
}
/// Resolve and validate a source before either copying or linking it.
pub fn validate_executable(source: &Path) -> Result<PathBuf> {
    let path = source
        .canonicalize()
        .with_context(|| format!("resolve artifact {}", source.display()))?;
    let m = fs::metadata(&path)?;
    #[cfg(unix)]
    let executable = m.permissions().mode() & 0o111 != 0;
    #[cfg(not(unix))]
    let executable = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
    if !m.is_file() || !executable {
        bail!("artifact is not an executable file: {}", path.display());
    }
    Ok(path)
}

impl ArtifactRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, source: &Path) -> Result<()> {
        let path = validate_executable(source)?;
        let m = fs::metadata(&path)?;
        self.sources.push((path, identity(&m)?));
        Ok(())
    }
    pub(super) fn matches(&self, target: &Path, forbidden: Option<&Path>) -> Result<bool> {
        let target_meta = fs::metadata(target)?;
        for (path, registered) in &self.sources {
            if forbidden.is_some_and(|root| path.starts_with(root)) {
                continue;
            }
            let m = fs::metadata(path)
                .with_context(|| format!("registered artifact disappeared: {}", path.display()))?;
            if identity(&m)? != *registered {
                bail!("registered artifact changed: {}", path.display());
            }
            #[cfg(unix)]
            let same =
                (target_meta.dev(), target_meta.ino()) == (registered.device, registered.inode);
            // Without filesystem identities only the canonical registered path
            // proves provenance; unrelated equal-sized files never suffice.
            #[cfg(not(unix))]
            let same = target.canonicalize()? == *path;
            if same {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn link_with(&self, src: &Path, dst: &Path, root: &Path, ops: &impl LinkOps) -> Result<()> {
        let src = src.canonicalize()?;
        if !self.matches(&src, None)? {
            bail!("unregistered artifact: {}", src.display());
        }
        if dst.components().any(|c| matches!(c, Component::ParentDir)) {
            bail!("destination contains parent traversal: {}", dst.display());
        }
        let root = root.canonicalize()?;
        let parent = dst
            .parent()
            .context("destination has no parent")?
            .canonicalize()?;
        if !parent.starts_with(&root) {
            bail!("destination escapes staging root: {}", dst.display());
        }
        // A new entry only: never truncate or chmod through a pre-existing link.
        if dst.symlink_metadata().is_ok() {
            bail!("staging destination already exists: {}", dst.display());
        }
        #[cfg(unix)]
        let hard_result = ops.hard_link(&src, dst);
        #[cfg(not(unix))]
        let hard_result: io::Result<()> = Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "hardlink identity verification unavailable",
        ));
        if let Err(hard_error) = hard_result {
            ops.symlink(&src, dst).with_context(|| {
                format!(
                    "hardlink failed ({hard_error}); symlink failed for {}",
                    dst.display()
                )
            })?;
        }
        if !self.matches(dst, None)? {
            bail!("staged artifact identity mismatch: {}", dst.display());
        }
        Ok(())
    }
}
