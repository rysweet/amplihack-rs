//! Shared filesystem helper for `amplihack signal` (#921).
//!
//! The onboarding config and the fleet rollout state are both secrets-adjacent,
//! so both are written with `0600`. Keeping the single writer here means the
//! permission-enforcement logic has one source of truth rather than being
//! duplicated (and drifting) across `run` and `distribute`.

use std::path::Path;

/// Write bytes to `path` with `0600` permissions on Unix (mode enforced at
/// create time so it is umask-independent, then re-set in case the file
/// pre-existed with looser permissions).
pub(super) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}
