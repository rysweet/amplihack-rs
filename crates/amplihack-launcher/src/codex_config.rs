//! Preserving, destination-adjacent transactions for supported Codex TOML.
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::{fs, io::Write, path::Path};
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
    // A directory lock survives atomic replacement of the configuration inode.
    let lock = fs::File::open(directory)?;
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
    // Only the exact historical launcher output is owned. Ambiguous files stay.
    let legacy = directory.join("config.yaml");
    if read_regular(&legacy)?.as_deref() == Some("approval_mode: auto\n") {
        fs::remove_file(&legacy).context("failed to retire owned legacy Codex YAML")?;
    }
    Ok(())
}
