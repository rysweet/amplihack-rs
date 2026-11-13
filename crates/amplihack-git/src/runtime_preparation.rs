//! Serialised, atomic registration in the clone's common info/exclude.
use crate::workspace_validation::Workspace;
use amplihack_types::paths::owned_runtime::EXCLUSIONS;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

pub fn prepare(root: &Path) -> Result<Workspace> {
    let workspace = Workspace::open(root)?;
    register(&workspace)?;
    Ok(workspace)
}

pub fn register(workspace: &Workspace) -> Result<()> {
    let info = workspace.common().join("info");
    if info.is_symlink() {
        bail!("common Git info directory must not be a symlink");
    }
    fs::create_dir_all(&info).context("cannot prepare common Git info directory")?;
    let lock_path = info.join("amplihack-exclude.lock");
    if info.is_symlink() || lock_path.is_symlink() {
        bail!("exclusion lock must not be a symlink");
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    let lock = options.open(&lock_path)?;
    lock.lock_exclusive()
        .context("cannot lock runtime exclusions")?;
    let path = info.join("exclude");
    if path.is_symlink() {
        bail!("common Git info/exclude must not be a symlink");
    }
    let (mut body, permissions) = match fs::read(&path) {
        Ok(body) => (body, Some(fs::metadata(&path)?.permissions())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
        Err(e) => {
            return Err(e).context("cannot read common Git info/exclude before runtime write");
        }
    };
    let original = body.clone();
    for pattern in EXCLUSIONS {
        let lines: Vec<_> = body.split(|b| *b == b'\n').collect();
        let equivalent = lines
            .iter()
            .rposition(|line| {
                let line = *line;
                line == pattern.as_bytes()
                    || (pattern.ends_with('/') && line == format!("{pattern}**").as_bytes())
            })
            .is_some_and(|position| {
                !lines[position + 1..]
                    .iter()
                    .any(|line| line.starts_with(b"!"))
            });
        if !equivalent {
            if !body.is_empty() && !body.ends_with(b"\n") {
                body.push(b'\n');
            }
            body.extend_from_slice(pattern.as_bytes());
            body.push(b'\n');
        }
    }
    if body != original {
        let mut tmp = tempfile::NamedTempFile::new_in(&info)?;
        if let Some(permissions) = permissions {
            tmp.as_file().set_permissions(permissions)?;
        }
        tmp.write_all(&body)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&path)
            .context("cannot atomically register runtime exclusions")?;
    }
    FileExt::unlock(&lock)?;
    Ok(())
}
