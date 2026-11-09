//! Private directory creation for hook state under the amplihack state root.
//!
//! Hook state (pre-compact transcripts, original prompts, tool metrics) is
//! sensitive, so every directory a hook creates under
//! `<state root>/runtime/sessions/<key>` or `<state root>/runtime/projects/<hex16>`
//! is created with mode `0700` on Unix (#1963, HARD-1). Directories that
//! already exist keep their mode.

use std::io;
use std::path::Path;

/// Create `dir` and any missing ancestors, each with mode `0700` on Unix.
///
/// Existing directories are left as they are. On other platforms this is
/// `std::fs::create_dir_all`.
pub(crate) fn create_private_dir_all(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn creates_every_missing_level_private() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a").join("b").join("c");
        create_private_dir_all(&dir).unwrap();
        assert_eq!(mode(&tmp.path().join("a")), 0o700);
        assert_eq!(mode(&tmp.path().join("a/b")), 0o700);
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn existing_directory_keeps_its_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir_all(&existing.join("child")).unwrap();
        assert_eq!(mode(&existing), 0o755);
        assert_eq!(mode(&existing.join("child")), 0o700);
    }

    #[test]
    fn regular_file_in_the_way_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(create_private_dir_all(&file.join("child")).is_err());
    }
}
