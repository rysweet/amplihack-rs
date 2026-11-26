//! The project's own source files, shared by the staleness scan and the
//! indexer (issue #2170).
//!
//! Both ask git for the files it tracks plus the untracked files it does not
//! ignore, so anything in `.gitignore` is skipped. Every path is then checked
//! against [`EXCLUDED_DIRS`], which holds the caches and build folders that
//! are skipped in any case, including nested task `worktrees/`, `target/` and
//! the project's own `.amplihack/` artifacts. When git cannot answer (no git
//! binary, or the folder is not the top of a repository), the folder is walked
//! directly with the same exclusions, without following symlinked folders.

use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

/// Folder names that never hold the project's own sources.
pub const EXCLUDED_DIRS: &[&str] = &[
    ".git",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    "node_modules",
    ".mypy_cache",
    ".tox",
    "dist",
    "build",
    ".eggs",
    "worktrees",
    "target",
    ".amplihack",
];

/// True when a folder with this name is skipped.
pub fn is_excluded_dir_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    EXCLUDED_DIRS.contains(&name.as_ref()) || name.ends_with(".egg-info")
}

/// True when any folder on a project-relative path is skipped.
pub fn is_excluded(relative_path: &Path) -> bool {
    relative_path.parent().is_some_and(|parent| {
        parent.components().any(|component| match component {
            Component::Normal(name) => is_excluded_dir_name(name),
            _ => false,
        })
    })
}

/// Every file of the project at `root`, as absolute paths.
///
/// Uses git's view of the project when `root` is the top of a repository and
/// falls back to a directory walk otherwise. It never fails because of git.
pub fn project_source_files(root: &Path) -> Vec<PathBuf> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    match git_listed_files(&root) {
        Some(relative) => relative
            .into_iter()
            .filter(|path| !is_excluded(path))
            .map(|path| root.join(path))
            .collect(),
        None => {
            let mut files = Vec::new();
            walk(&root, &mut files);
            files
        }
    }
}

/// Paths relative to `root` from `git ls-files`, or `None` when git cannot
/// answer for `root` itself.
///
/// `GIT_CEILING_DIRECTORIES` stops git from using a repository above `root`,
/// whose ignore rules could hide the whole folder. `amplihack_git` removes the
/// variables a git hook sets, so that they cannot point git at another
/// repository.
fn git_listed_files(root: &Path) -> Option<Vec<PathBuf>> {
    let mut cmd = amplihack_git::command_in(root);
    cmd.arg("ls-files")
        .arg("-z")
        .arg("--cached")
        .arg("--others")
        .arg("--exclude-standard")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if let Some(parent) = root.parent() {
        cmd.env("GIT_CEILING_DIRECTORIES", parent);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let mut paths: Vec<PathBuf> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| PathBuf::from(String::from_utf8_lossy(entry).into_owned()))
        .collect();
    // `--cached` and `--others` can both list a path during a merge.
    paths.sort();
    paths.dedup();
    Some(paths)
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if !is_excluded_dir_name(&entry.file_name()) {
                walk(&entry.path(), files);
            }
        } else if file_type.is_file() {
            files.push(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) {
        let status = amplihack_git::command_in(root)
            .args(args)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn write(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "x\n").unwrap();
    }

    fn relative_names(root: &Path, files: Vec<PathBuf>) -> Vec<String> {
        let root = root.canonicalize().unwrap();
        let mut names: Vec<String> = files
            .into_iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn is_excluded_checks_every_folder_on_the_path() {
        assert!(is_excluded(Path::new("worktrees/task-1/src/lib.rs")));
        assert!(is_excluded(Path::new("crates/x/target/debug/build.rs")));
        assert!(is_excluded(Path::new(".amplihack/blarify.json")));
        assert!(is_excluded(Path::new("pkg.egg-info/x.py")));
        assert!(!is_excluded(Path::new("src/target.rs")));
        assert!(!is_excluded(Path::new("src/lib.rs")));
    }

    #[test]
    fn git_project_lists_tracked_and_untracked_but_not_ignored_files() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        git(root, &["init", "-q"]);
        fs::write(root.join(".gitignore"), "generated/\n").unwrap();
        write(root, "src/main.rs");
        git(root, &["add", "src/main.rs"]);
        write(root, "src/new.rs");
        write(root, "generated/x.rs");
        write(root, "worktrees/task-1/src/lib.rs");
        write(root, "target/debug/x.rs");
        write(root, ".amplihack/indexes/rust.scip");

        let names = relative_names(root, project_source_files(root));

        assert_eq!(names, vec![".gitignore", "src/main.rs", "src/new.rs"]);
    }

    #[test]
    fn folder_outside_git_is_walked_with_the_same_exclusions() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        write(root, "src/main.rs");
        write(root, "worktrees/task-1/src/lib.rs");
        write(root, "target/debug/x.rs");
        write(root, "node_modules/pkg/index.js");

        let names = relative_names(root, project_source_files(root));

        assert_eq!(names, vec!["src/main.rs"]);
    }

    #[cfg(unix)]
    #[test]
    fn walk_does_not_follow_symlinked_folders() {
        let project = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let root = project.path();
        write(root, "src/main.rs");
        write(elsewhere.path(), "lib.rs");
        std::os::unix::fs::symlink(elsewhere.path(), root.join("linked")).unwrap();

        let names = relative_names(root, project_source_files(root));

        assert_eq!(names, vec!["src/main.rs"]);
    }
}
