//! Regular-file snapshots and atomic writes.
use super::*;
pub(super) fn regular_json(path: &Path) -> Result<Option<Value>> {
    match fs::symlink_metadata(path) {
        Ok(m) => {
            ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "refusing nonregular Codex JSON file"
            );
            Ok(Some(serde_json::from_slice(&fs::read(path)?).context(
                "malformed Codex JSON; repair manually before retrying",
            )?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub(super) fn atomic_json(path: &Path, value: &Value, original: Option<Value>) -> Result<()> {
    let parent = path.parent().context("JSON destination has no parent")?;
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    if original.is_some() {
        staged
            .as_file()
            .set_permissions(fs::metadata(path)?.permissions())?;
    }
    staged.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
    staged.write_all(b"\n")?;
    staged.as_file().sync_all()?;
    ensure!(
        regular_json(path)? == original,
        "Codex JSON changed concurrently; retry"
    );
    staged
        .persist(path)
        .context("failed atomic Codex JSON write")?;
    Ok(())
}

/// Snapshot absence or exact regular-file bytes before live mutations.
pub(super) fn snapshot(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "refusing nonregular Codex snapshot"
            );
            Ok(Some(fs::read(path)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
pub(super) fn restore_bytes(path: &Path, value: &Value) -> Result<()> {
    let current = snapshot(path)?;
    if value.is_null() {
        if current.is_some() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }
    let bytes: Vec<u8> = value
        .as_array()
        .context("invalid recovery snapshot")?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .context("invalid recovery byte")
        })
        .collect::<Result<_>>()?;
    let parent = path.parent().context("snapshot parent missing")?;
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    if current.is_some() {
        staged
            .as_file()
            .set_permissions(fs::metadata(path)?.permissions())?;
    }
    staged.write_all(&bytes)?;
    staged.as_file().sync_all()?;
    ensure!(
        snapshot(path)? == current,
        "Codex snapshot changed concurrently"
    );
    staged.persist(path).context("snapshot recovery failed")?;
    Ok(())
}
