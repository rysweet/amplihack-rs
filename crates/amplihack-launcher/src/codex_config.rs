//! Preserving, destination-adjacent transactions for supported Codex TOML.
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use toml_edit::DocumentMut;

fn read_regular(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.is_file() || meta.file_type().is_symlink() {
                bail!(
                    "refusing nonregular or symlink Codex config {}",
                    path.display()
                );
            }
            Ok(Some(fs::read_to_string(path).context(
                "unreadable Codex config; preserve it and repair manually",
            )?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Ordinary launch validates existing preferences without adding approvals.
/// Historical bootstrap alone fills an absent approval_policy with never.
pub fn configure(directory: &Path, bootstrap: bool) -> Result<()> {
    if directory.exists() && fs::symlink_metadata(directory)?.file_type().is_symlink() {
        bail!("refusing symlink Codex config directory");
    }
    if !directory.exists() && !bootstrap {
        return Ok(());
    }
    fs::create_dir_all(directory)?;
    let path = directory.join("config.toml");
    // A persistent sidecar lock survives atomic replacement and works on Windows,
    // where opening a directory as a regular file is not supported.
    let lock_path = directory.join(".amplihack-config.lock");
    let mut lock_options = OpenOptions::new();
    lock_options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        lock_options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let lock = lock_options
        .open(&lock_path)
        .context("failed to open Codex configuration lock file")?;
    let lock_metadata = lock.metadata()?;
    if !lock_metadata.is_file() {
        bail!("refusing nonregular Codex configuration lock file");
    }
    #[cfg(windows)]
    if lock_metadata.file_type().is_symlink() {
        bail!("refusing symlink Codex configuration lock file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if lock_metadata.nlink() != 1 {
            bail!("refusing hardlinked Codex configuration lock file");
        }
        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    lock.lock_exclusive()
        .context("failed to lock Codex configuration directory")?;
    let original = read_regular(&path)?;
    let mut doc = original
        .as_deref()
        .unwrap_or("")
        .parse::<DocumentMut>()
        .context("malformed Codex config.toml; repair manually before retrying")?;
    if let Some(policy) = doc.get("approval_policy") {
        if policy.as_str().is_none()
            && policy.get("reject").is_none()
            && policy.get("granular").is_none()
        {
            bail!(
                "refusing to overwrite approval_policy: not a string or granular reject table; existing config preserved"
            );
        }
    } else if bootstrap {
        doc["approval_policy"] = toml_edit::value("never");
        let mut staged = tempfile::NamedTempFile::new_in(directory)?;
        if original.is_some() {
            staged
                .as_file()
                .set_permissions(fs::metadata(&path)?.permissions())?;
        }
        staged.write_all(doc.to_string().as_bytes())?;
        staged.as_file().sync_all()?;
        if read_regular(&path)? != original {
            bail!("Codex configuration changed concurrently; retry without overwriting");
        }
        staged
            .persist(&path)
            .context("failed to atomically replace Codex configuration")?;
        lock.sync_all()?;
    }
    // Leave legacy config.yaml untouched. It is outside this TOML transaction,
    // and even known historical contents must survive a failed installation.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_uses_a_persistent_regular_sidecar_lock() {
        let directory = tempfile::tempdir().unwrap();
        configure(directory.path(), true).unwrap();

        let lock = fs::symlink_metadata(directory.path().join(".amplihack-config.lock")).unwrap();
        assert!(lock.is_file());
        let config = fs::read_to_string(directory.path().join("config.toml")).unwrap();
        assert!(config.contains("approval_policy = \"never\""));
    }

    #[cfg(unix)]
    #[test]
    fn hardlinked_config_lock_preserves_foreign_file() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("foreign");
        fs::write(&target, b"untouched").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        fs::hard_link(&target, directory.path().join(".amplihack-config.lock")).unwrap();

        assert!(configure(directory.path(), true).is_err());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(fs::read(target).unwrap(), b"untouched");
        assert!(!directory.path().join("config.toml").exists());
    }
}
