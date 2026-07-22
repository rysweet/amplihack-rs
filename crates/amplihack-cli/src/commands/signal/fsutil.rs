//! Shared filesystem helper for `amplihack signal` (#921).
//!
//! The onboarding config and the fleet rollout state are both secrets-adjacent,
//! so both are written with `0600`. Keeping the single writer here means the
//! permission-enforcement logic has one source of truth rather than being
//! duplicated (and drifting) across `run` and `distribute`.

use std::path::{Path, PathBuf};

/// Write bytes to `path` **atomically** with `0600` permissions on Unix.
///
/// The bytes are written to a sibling temp file (created `0600`, fsynced), then
/// `rename(2)`d onto the final path. Because `rename` is atomic on a single
/// filesystem, a crash or interruption mid-write can never leave the target in a
/// truncated/partial state — a reader either sees the old contents or the fully
/// written new contents. This matters for the resumable rollout state
/// (`signal-distribute-state.json`) and the onboarding config, both of which are
/// re-read on the next run: a half-written file would otherwise fail to parse
/// and block every resume. The mode is enforced at create time (umask-independent)
/// and re-set before the rename in case the temp file pre-existed with looser bits.
pub(super) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let file_name = path.file_name().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name")
        })?;
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
        // Temp file lives in the SAME directory as the target so the rename stays
        // on one filesystem (a cross-device rename is not atomic and would EXDEV).
        let mut tmp_name = file_name.to_os_string();
        tmp_name.push(format!(".tmp.{}", std::process::id()));
        let tmp: PathBuf = match dir {
            Some(d) => d.join(&tmp_name),
            None => PathBuf::from(&tmp_name),
        };

        let write_result = (|| -> std::io::Result<()> {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
            std::fs::rename(&tmp, path)
        })();

        if write_result.is_err() {
            // Never leave a stray temp file behind on failure (best-effort).
            let _ = std::fs::remove_file(&tmp);
        }
        write_result
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}
