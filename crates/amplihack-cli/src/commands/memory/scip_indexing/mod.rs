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
        fs::write(project.path().join("app.py"), "print('hi')\n").unwrap();

        write_executable(
            &bin_dir.path().join("scip-python"),
            "#!/bin/sh\nprintf 'stub-scip' > index.scip\n",
        );

        let old_path = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", bin_dir.path()) };

        let summary = run_native_scip_indexing(Some(project.path()), &[]).unwrap();

        match old_path {
            Some(path) => unsafe { std::env::set_var("PATH", path) },
            None => unsafe { std::env::remove_var("PATH") },
        }

        assert!(summary.success);
        assert_eq!(summary.completed_languages, vec!["python"]);
        let artifact = project.path().join(".amplihack/indexes/python.scip");
        assert!(artifact.exists());
        assert_eq!(fs::read_to_string(artifact).unwrap(), "stub-scip");
    }
}
