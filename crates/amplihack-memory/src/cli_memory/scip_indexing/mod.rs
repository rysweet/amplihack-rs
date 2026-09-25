mod commands;
mod helpers;
mod indexer;
mod types;

pub use commands::{
    check_prerequisites, detect_project_languages, run_index_scip, run_native_scip_indexing,
};
pub use types::{LanguageStatus, NativeScipIndexSummary, PrerequisiteResult, ScipIndexResult};

pub(crate) use helpers::{language_for_path, normalize_languages, should_ignore_dir};
pub(crate) use types::LANGUAGE_ORDER;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn write_executable(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn detect_project_languages_discovers_supported_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.path().join("src/app.py"), "print('hi')\n").unwrap();
        fs::write(dir.path().join("src/app.ts"), "export {};\n").unwrap();

        let languages = detect_project_languages(dir.path()).unwrap();

        assert_eq!(languages, vec!["python", "typescript", "rust"]);
    }

    #[test]
    fn run_native_scip_indexing_with_stubbed_python_indexer_creates_artifact() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let project = tempfile::tempdir().unwrap();
        let bin_dir = tempfile::tempdir().unwrap();
        // Pin the artifact cache to a directory this test owns: the indexed
        // artifact now lands there rather than in `<project>/.amplihack`, and
        // without the guard the assertion would read the developer's real
        // `~/.cache/amplihack` (issue #1476).
        let cache = tempfile::tempdir().unwrap();
        let _cache_guard = crate::test_support::ArtifactCacheGuard::set(cache.path());
        fs::write(project.path().join("app.py"), "print('hi')\n").unwrap();

        // The stub must honour `--output`, because that is what the real tool
        // does and what the code now passes (`scip-python index --output <path>`,
        // tier `ScipOutput::Flag`). The previous stub wrote `index.scip` into its
        // working directory and ignored the flag — faithful while the code relied
        // on that convention, misleading once it stopped. A stub that ignores the
        // flag reports "was not created" and looks like a code defect.
        //
        // Builtins only: this test replaces PATH with `bin_dir` alone, so
        // `mkdir` and `dirname` are not resolvable. The parent directory is
        // created by the caller before the indexer runs.
        write_executable(
            &bin_dir.path().join("scip-python"),
            "#!/bin/sh\n\
             out=\"\"\n\
             prev=\"\"\n\
             for a in \"$@\"; do\n\
             \x20 if [ \"$prev\" = \"--output\" ]; then out=\"$a\"; fi\n\
             \x20 prev=\"$a\"\n\
             done\n\
             printf 'stub-scip' > \"$out\"\n",
        );

        let old_path = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", bin_dir.path()) };

        let summary = run_native_scip_indexing(Some(project.path()), &[]).unwrap();

        match old_path {
            Some(path) => unsafe { std::env::set_var("PATH", path) },
            None => unsafe { std::env::remove_var("PATH") },
        }

        assert!(
            summary.success,
            "indexing failed: errors={:?} failed={:?} skipped={:?} artifacts={:?}",
            summary.errors,
            summary.failed_languages,
            summary.skipped_languages,
            summary.artifacts
        );
        assert_eq!(summary.completed_languages, vec!["python"]);

        // The stub writes `index.scip` into whatever directory it is run from.
        // That is exactly the convention #1476 had to stop relying on, so assert
        // both halves: the artifact arrives in the cache, and nothing at all was
        // left inside the checkout.
        let artifact = crate::cli_memory::artifact_root::project_artifact_root(project.path())
            .unwrap()
            .join("indexes")
            .join("python.scip");
        assert!(
            artifact.exists(),
            "expected the artifact in the cache at {}",
            artifact.display()
        );
        assert_eq!(fs::read_to_string(&artifact).unwrap(), "stub-scip");
        assert!(
            !project.path().join(".amplihack").exists(),
            "indexing must not create .amplihack inside the checkout"
        );
        assert!(
            !project.path().join("index.scip").exists(),
            "indexing must not leave index.scip in the project root"
        );
    }
}
