//! Where the config files are: the user file under `$HOME`, the project file
//! under the git root, and the legacy JSON file.

use std::path::{Path, PathBuf};

/// The user file for `home` (`$HOME`, never `AMPLIHACK_HOME`).
pub(crate) fn user_file(home: &Path) -> PathBuf {
    home.join(".amplihack").join("config.toml")
}

/// The legacy JSON file for `home`.
pub(crate) fn legacy_file(home: &Path) -> PathBuf {
    home.join(".amplihack").join("config")
}

/// Where a migrated legacy file is moved to.
pub(crate) fn migrated_file(home: &Path) -> PathBuf {
    home.join(".amplihack").join("config.json.migrated")
}

/// The project file inside `dir`.
fn project_file_in(dir: &Path) -> PathBuf {
    dir.join(".amplihack").join("config.toml")
}

/// The nearest directory, from `cwd` upward, that holds a git repository:
/// a `.git` file (a worktree or submodule) or a `.git` directory with a
/// `HEAD`. An empty `.git` directory is not a repository to git, and a stray
/// one (seen at `/tmp/.git` on a shared host) must not make every temp
/// directory a project.
pub(crate) fn git_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| is_repository(dir))
        .map(Path::to_path_buf)
}

fn is_repository(dir: &Path) -> bool {
    let dot_git = dir.join(".git");
    match std::fs::metadata(&dot_git) {
        Ok(meta) if meta.is_file() => true,
        Ok(meta) if meta.is_dir() => dot_git.join("HEAD").is_file(),
        _ => false,
    }
}

/// The project-file situation for `cwd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Project {
    /// `cwd` is not inside a git repository: there is no project layer.
    NotInRepo,
    /// The first `.amplihack/config.toml` from `cwd` up to the git root, or
    /// `<git root>/.amplihack/config.toml` when none exists yet (`found` is
    /// then `false`).
    File { path: PathBuf, found: bool },
}

impl Project {
    /// The file a `--project` write targets.
    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Project::NotInRepo => None,
            Project::File { path, .. } => Some(path),
        }
    }
}

/// Find the project file for `cwd`. The search never goes above the git root.
pub(crate) fn project(cwd: &Path) -> Project {
    let Some(root) = git_root(cwd) else {
        return Project::NotInRepo;
    };
    for dir in cwd.ancestors() {
        let candidate = project_file_in(dir);
        if std::fs::symlink_metadata(&candidate).is_ok() {
            return Project::File {
                path: candidate,
                found: true,
            };
        }
        if dir == root {
            break;
        }
    }
    Project::File {
        path: project_file_in(&root),
        found: false,
    }
}

/// `true` when `a` and `b` are the same file (after resolving symlinks), or
/// the same path when either does not exist.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn project_search_stops_at_the_nearest_git_root() {
        let tmp = tempfile::tempdir().unwrap();
        let outer = tmp.path();
        fs::create_dir_all(outer.join(".git")).unwrap();
        fs::write(outer.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(outer.join(".amplihack")).unwrap();
        fs::write(project_file_in(outer), "").unwrap();
        let inner = outer.join("sub");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join(".git"), "gitdir: x\n").unwrap();
        assert_eq!(
            project(&inner),
            Project::File {
                path: project_file_in(&inner),
                found: false
            }
        );
        let deep = outer.join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            project(&deep),
            Project::File {
                path: project_file_in(outer),
                found: true
            }
        );
    }

    #[test]
    fn an_empty_git_directory_is_not_a_repository() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        assert!(!is_repository(tmp.path()));
        fs::write(tmp.path().join(".git").join("HEAD"), "ref: x\n").unwrap();
        assert!(is_repository(tmp.path()));
    }

    #[test]
    fn no_git_means_no_project() {
        let tmp = tempfile::tempdir().unwrap();
        // The temp dir itself may be inside a repository on odd hosts; only
        // assert when it is not.
        if git_root(tmp.path()).is_none() {
            assert_eq!(project(tmp.path()), Project::NotInRepo);
        }
    }
}
