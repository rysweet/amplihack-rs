mod commands;
mod helpers;
mod indexer;
mod types;

pub use commands::{
    check_prerequisites, detect_project_languages, run_index_scip, run_native_scip_indexing,
};
pub use types::{LanguageStatus, NativeScipIndexSummary, PrerequisiteResult, ScipIndexResult};

pub(crate) use crate::cli_memory::source_files::language_for_path;
pub(crate) use helpers::normalize_languages;
pub(crate) use types::LANGUAGE_ORDER;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

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
    fn detect_project_languages_skips_worktrees_and_git_ignored_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let status = amplihack_git::command_in(root)
                .args(args)
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        fs::write(root.join(".gitignore"), "scratch/\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::create_dir_all(root.join("worktrees/task-1")).unwrap();
        fs::write(root.join("worktrees/task-1/app.py"), "print(1)\n").unwrap();
        fs::create_dir_all(root.join("scratch")).unwrap();
        fs::write(root.join("scratch/tool.py"), "print(2)\n").unwrap();

        let languages = detect_project_languages(root).unwrap();

        assert_eq!(languages, vec!["rust"]);
    }

    #[test]
    fn run_native_scip_indexing_with_stubbed_python_indexer_creates_artifact() {
        const PROJECT_ENV: &str = "AMPLIHACK_TEST_SCIP_PROJECT";
        if let Some(project) = std::env::var_os(PROJECT_ENV) {
            let project = PathBuf::from(project);
            let summary = run_native_scip_indexing(Some(&project), &[]).unwrap();
            assert!(summary.success);
            assert_eq!(summary.completed_languages, vec!["python"]);
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let bin_dir = tempfile::tempdir().unwrap();
        fs::write(project.path().join("app.py"), "print('hi')\n").unwrap();

        write_executable(
            &bin_dir.path().join("scip-python"),
            "#!/bin/sh\nprintf 'stub-scip' > index.scip\n",
        );

        // Isolate PATH and HOME without changing how other tests find Git.
        let probe = concat!(
            module_path!(),
            "::run_native_scip_indexing_with_stubbed_python_indexer_creates_artifact"
        );
        let (_, probe) = probe.split_once("::").unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", probe])
            .env(PROJECT_ENV, project.path())
            .env("HOME", project.path())
            .env("PATH", bin_dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated indexing probe failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let artifact = project.path().join(".amplihack/indexes/python.scip");
        assert_eq!(fs::read_to_string(artifact).unwrap(), "stub-scip");
    }
}
