//! Helpers shared by the agent-binary integration tests.

/// A temp directory the resolver's walk-up cannot leave.
///
/// The walk-up stops at a `.git` boundary. Without one at the root, it
/// carries on above the system temp directory, and a fresh
/// `.claude/runtime/launcher_context.json` in any ancestor (a session started
/// in a parent of `TMPDIR`, for instance) answers instead of the case under
/// test, so the suite passes or fails with the host it runs on (#1666).
pub fn bounded_tempdir() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}
