use super::*;
use flate2::read::GzDecoder;
use std::ffi::OsStr;
use tar::Archive;

pub(super) fn binary_filename(name: &'static str) -> &'static str {
    if cfg!(windows) {
        match name {
            "amplihack" => "amplihack.exe",
            "amplihack-hooks" => "amplihack-hooks.exe",
            "amplihack-asset-resolver" => "amplihack-asset-resolver.exe",
            _ => name,
        }
    } else {
        name
    }
}

pub(crate) fn extract_archive(archive_bytes: &[u8], destination: &Path) -> Result<()> {
    let decoder = GzDecoder::new(std::io::Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);
    archive
        .unpack(destination)
        .with_context(|| format!("failed to unpack archive into {}", destination.display()))?;
    Ok(())
}

/// Finds `binary_name` in the extracted archive, or `Ok(None)` when no entry
/// of that name exists.
///
/// Only absence is `Ok(None)`. An entry of that name that is a symlink is an
/// error, so a link out of the archive is never installed, and any I/O error
/// while walking the tree is returned. Symlinked directories are not
/// followed.
pub(super) fn find_optional_binary(root: &Path, binary_name: &str) -> Result<Option<PathBuf>> {
    fn search(root: &Path, binary_name: &str, depth: usize) -> Result<Option<PathBuf>> {
        if depth > 3 {
            return Ok(None);
        }

        let entries = fs::read_dir(root).with_context(|| {
            format!(
                "failed to read extracted archive directory {}",
                root.display()
            )
        })?;
        for entry in entries {
            let entry = entry.with_context(|| {
                format!(
                    "failed to read entry in extracted archive directory {}",
                    root.display()
                )
            })?;
            let path = entry.path();
            let file_type = entry.file_type().with_context(|| {
                format!(
                    "failed to inspect extracted archive entry {}",
                    path.display()
                )
            })?;
            let name_matches = path.file_name() == Some(OsStr::new(binary_name));
            if file_type.is_symlink() {
                if name_matches {
                    bail!(
                        "archive entry {} is a symlink; refusing to install it",
                        path.display()
                    );
                }
                continue;
            }
            if file_type.is_file() && name_matches {
                return Ok(Some(path));
            }
            if file_type.is_dir()
                && let Some(found) = search(&path, binary_name, depth + 1)?
            {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    search(root, binary_name, 0)
}

/// Finds a binary the archive must contain.
pub(super) fn find_binary(root: &Path, binary_name: &str) -> Result<PathBuf> {
    find_optional_binary(root, binary_name)?
        .ok_or_else(|| anyhow!("binary '{binary_name}' not found in downloaded archive"))
}
