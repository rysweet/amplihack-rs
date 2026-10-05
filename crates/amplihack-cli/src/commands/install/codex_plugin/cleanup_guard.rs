//! Cheap change detection between full cleanup preflights on Unix.
//! Content is validated before and after capture. Inode/change-time stamps
//! reject replacement, in-place edits and type changes before each unlink.
use super::*;

#[cfg(unix)]
#[derive(PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    mode: u32,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[cfg(unix)]
fn stamp(path: &Path) -> Result<Option<Stamp>> {
    use std::os::unix::fs::MetadataExt;
    let m = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some(Stamp {
        device: m.dev(),
        inode: m.ino(),
        mode: m.mode(),
        length: m.len(),
        modified: (m.mtime(), m.mtime_nsec()),
        changed: (m.ctime(), m.ctime_nsec()),
    }))
}

pub(super) struct Guard {
    #[cfg(unix)]
    paths: Vec<(PathBuf, Option<Stamp>)>,
}

impl Guard {
    pub(super) fn capture(pending: &Value, root: &Path, home: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            fn collect(path: &Path, paths: &mut Vec<(PathBuf, Option<Stamp>)>) -> Result<()> {
                let m = fs::symlink_metadata(path)?;
                paths.push((path.into(), stamp(path)?));
                if m.is_dir() && !m.file_type().is_symlink() {
                    for entry in fs::read_dir(path)? {
                        collect(&entry?.path(), paths)?;
                    }
                }
                Ok(())
            }
            let mut paths = Vec::new();
            collect(&root.join("market/plugin"), &mut paths)?;
            for path in [
                root.to_path_buf(),
                root.join("market"),
                root.join("market/.agents"),
                root.join("market/.agents/plugins"),
                home.to_path_buf(),
                root.join("pending.json"),
                root.join("ownership.json"),
                root.join("market/.agents/plugins/marketplace.json"),
                home.join("config.toml"),
                home.join("hooks.json"),
            ] {
                paths.push((path.clone(), stamp(&path)?));
            }
            let guard = Self { paths };
            // Never authorize a stamp captured after a foreign edit without
            // checking its content against the transaction's exact proof.
            recovery::preflight(pending, root, home)?;
            guard.check(pending, root, home)?;
            Ok(guard)
        }
        #[cfg(not(unix))]
        {
            recovery::preflight(pending, root, home)?;
            Ok(Self {})
        }
    }

    pub(super) fn check(&self, pending: &Value, root: &Path, home: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            let _ = (pending, root, home);
            for (path, expected) in &self.paths {
                ensure!(
                    stamp(path)? == *expected,
                    "foreign Codex cleanup resource changed concurrently; record retained"
                );
            }
            Ok(())
        }
        // Keep exact preflights on platforms without Unix inode/change-time
        // semantics; no weaker metadata-only portability assumption.
        #[cfg(not(unix))]
        recovery::preflight(pending, root, home)
    }
}
