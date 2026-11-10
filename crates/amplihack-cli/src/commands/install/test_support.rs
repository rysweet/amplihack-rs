//! Binary staging safeguards compiled only into test builds.
pub mod artifacts;
pub mod footprint;

/// An inert private resolver for tests which do not exercise asset resolution.
/// This compiled seam takes precedence over both sibling and PATH discovery.
pub(super) fn resolver_stub() -> anyhow::Result<std::path::PathBuf> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    static FIXTURE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    if let Some(dir) = FIXTURE.get() {
        return Ok(dir.path().join("amplihack-asset-resolver"));
    }
    let dir = tempfile::Builder::new()
        .prefix("amplihack-private-resolver-")
        .tempdir()?;
    let path = dir.path().join("amplihack-asset-resolver");
    // An accidental invocation must fail rather than pretend to resolve assets.
    std::fs::write(
        &path,
        b"#!/bin/sh\necho 'inert test resolver invoked' >&2\nexit 1\n",
    )?;
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    let _ = FIXTURE.set(dir);
    Ok(FIXTURE
        .get()
        .expect("resolver initialized")
        .path()
        .join("amplihack-asset-resolver"))
}
