use super::project_artifact_paths;
use super::source_files::project_source_files;
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

const INDEXABLE_EXTENSIONS: &[&str] = &[
    "py", "ts", "tsx", "js", "jsx", "go", "rs", "cs", "c", "cpp", "h", "hpp",
];

#[derive(Debug, Clone)]
pub struct IndexStatus {
    pub needs_indexing: bool,
    pub reason: String,
    pub estimated_files: usize,
    pub last_indexed: Option<SystemTime>,
}

pub fn check_index_status(project_path: &Path) -> Result<IndexStatus> {
    let project_path = project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf());
    let (estimated_files, newest_source_mtime) = scan_source_files(&project_path);
    let index_file = resolve_index_artifact(&project_path);

    let index_metadata = match fs::metadata(&index_file) {
        Ok(metadata) => Some(metadata),
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            None
        }
        Err(err) => return Err(err.into()),
    };

    if let Some(metadata) = index_metadata {
        let last_indexed = metadata.modified().ok();
        if newest_source_mtime
            .is_some_and(|mtime| metadata.modified().is_ok_and(|indexed| mtime > indexed))
        {
            return Ok(IndexStatus {
                needs_indexing: true,
                reason: "stale (source files modified after index)".to_string(),
                estimated_files,
                last_indexed,
            });
        }

        return Ok(IndexStatus {
            needs_indexing: false,
            reason: if estimated_files == 0 {
                "no files to index (empty project)".to_string()
            } else {
                "up-to-date (index is current)".to_string()
            },
            estimated_files,
            last_indexed,
        });
    }

    if estimated_files == 0 {
        return Ok(IndexStatus {
            needs_indexing: false,
            reason: "no files to index (empty project)".to_string(),
            estimated_files: 0,
            last_indexed: None,
        });
    }

    Ok(IndexStatus {
        needs_indexing: true,
        reason: format!("missing (no {} found)", index_file.display()),
        estimated_files,
        last_indexed: None,
    })
}

fn resolve_index_artifact(project_path: &Path) -> std::path::PathBuf {
    let paths = project_artifact_paths(project_path);
    if paths.blarify_json.exists() {
        return paths.blarify_json;
    }
    if let Some(latest) = latest_scip_artifact(&paths.indexes_dir) {
        return latest;
    }
    paths.index_scip
}

fn latest_scip_artifact(indexes_dir: &Path) -> Option<std::path::PathBuf> {
    let mut newest: Option<(SystemTime, std::path::PathBuf)> = None;
    let entries = fs::read_dir(indexes_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("scip") {
            continue;
        }
        let modified = entry.metadata().ok()?.modified().ok()?;
        if newest
            .as_ref()
            .is_none_or(|(current_modified, _)| modified > *current_modified)
        {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, path)| path)
}

/// Counts the project's indexable sources and finds the newest mtime.
///
/// Uses the same file list as the indexer, so git-ignored paths, nested
/// `worktrees/`, `target/` and `.amplihack/` never mark the index stale
/// (issue #2170). Files whose metadata cannot be read, such as tracked files
/// deleted from the working tree, are skipped.
fn scan_source_files(project_path: &Path) -> (usize, Option<SystemTime>) {
    let mut count = 0usize;
    let mut newest_mtime: Option<SystemTime> = None;
    for path in project_source_files(project_path) {
        if !is_indexable_file(&path) {
            continue;
        }
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        count += 1;
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if newest_mtime.is_none_or(|current| modified > current) {
            newest_mtime = Some(modified);
        }
    }
    (count, newest_mtime)
}

fn is_indexable_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| INDEXABLE_EXTENSIONS.contains(&ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn check_index_status_reports_missing_when_sources_exist() {
        let project = tempfile::tempdir().unwrap();
        fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();

        let status = check_index_status(project.path()).unwrap();

        assert!(status.needs_indexing);
        assert_eq!(status.estimated_files, 1);
        assert!(status.reason.contains("missing"));
        assert!(status.last_indexed.is_none());
    }

    #[test]
    fn check_index_status_reports_up_to_date_for_current_blarify_json() {
        let project = tempfile::tempdir().unwrap();
        fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();
        thread::sleep(Duration::from_millis(20));
        let artifact_dir = project.path().join(".amplihack");
        fs::create_dir_all(&artifact_dir).unwrap();
        fs::write(artifact_dir.join("blarify.json"), "{}\n").unwrap();

        let status = check_index_status(project.path()).unwrap();

        assert!(!status.needs_indexing);
        assert_eq!(status.reason, "up-to-date (index is current)");
        assert!(status.last_indexed.is_some());
    }

    #[test]
    fn check_index_status_reports_stale_when_source_is_newer() {
        let project = tempfile::tempdir().unwrap();
        let artifact_dir = project.path().join(".amplihack");
        fs::create_dir_all(&artifact_dir).unwrap();
        fs::write(artifact_dir.join("blarify.json"), "{}\n").unwrap();
        thread::sleep(Duration::from_millis(20));
        fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();

        let status = check_index_status(project.path()).unwrap();

        assert!(status.needs_indexing);
        assert_eq!(status.reason, "stale (source files modified after index)");
        assert_eq!(status.estimated_files, 1);
        assert!(status.last_indexed.is_some());
    }

    #[test]
    fn check_index_status_ignores_cached_directories() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("node_modules/pkg")).unwrap();
        fs::create_dir_all(project.path().join("src")).unwrap();
        fs::write(
            project.path().join("node_modules/pkg/index.ts"),
            "export {};\n",
        )
        .unwrap();
        fs::write(project.path().join("src/main.ts"), "export {};\n").unwrap();

        let status = check_index_status(project.path()).unwrap();

        assert_eq!(status.estimated_files, 1);
    }

    #[test]
    fn check_index_status_uses_generated_scip_artifacts_directory() {
        let project = tempfile::tempdir().unwrap();
        fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();
        thread::sleep(Duration::from_millis(20));
        let artifact_dir = project.path().join(".amplihack").join("indexes");
        fs::create_dir_all(&artifact_dir).unwrap();
        fs::write(artifact_dir.join("rust.scip"), "scip-bytes").unwrap();

        let status = check_index_status(project.path()).unwrap();

        assert!(!status.needs_indexing);
        assert_eq!(status.reason, "up-to-date (index is current)");
        assert!(status.last_indexed.is_some());
    }

    // Issue #2170: only the project's own sources decide staleness.

    use std::process::Stdio;

    fn git(root: &Path, args: &[&str]) {
        let status = amplihack_git::command_in(root)
            .args(args)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn write_with_mtime(root: &Path, relative: &str, mtime: SystemTime) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "fn main() {}\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    /// A project with a tracked source and an index written after it.
    /// Returns the time the index was written.
    fn indexed_project(root: &Path, git_ignore: Option<&str>) -> SystemTime {
        let now = SystemTime::now();
        let indexed_at = now - Duration::from_secs(60);
        if let Some(rules) = git_ignore {
            git(root, &["init", "-q"]);
            fs::write(root.join(".gitignore"), rules).unwrap();
        }
        write_with_mtime(root, "src/main.rs", now - Duration::from_secs(120));
        if git_ignore.is_some() {
            git(root, &["add", "src/main.rs", ".gitignore"]);
        }
        write_with_mtime(root, ".amplihack/blarify.json", indexed_at);
        indexed_at
    }

    #[test]
    fn ignored_worktree_newer_than_index_is_not_stale() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let indexed_at = indexed_project(root, Some("worktrees/\n"));
        write_with_mtime(
            root,
            "worktrees/task-1/src/lib.rs",
            indexed_at + Duration::from_secs(30),
        );

        let status = check_index_status(root).unwrap();

        assert!(!status.needs_indexing, "{}", status.reason);
        assert_eq!(status.estimated_files, 1);
    }

    #[test]
    fn target_and_amplihack_folders_never_mark_the_index_stale() {
        for git_ignore in [Some("\n"), None] {
            let project = tempfile::tempdir().unwrap();
            let root = project.path();
            let indexed_at = indexed_project(root, git_ignore);
            let later = indexed_at + Duration::from_secs(30);
            write_with_mtime(root, "target/debug/build/out.rs", later);
            write_with_mtime(root, ".amplihack/scratch/x.rs", later);
            write_with_mtime(root, "worktrees/task-2/src/lib.rs", later);

            let status = check_index_status(root).unwrap();

            assert!(
                !status.needs_indexing,
                "git: {}, reason: {}",
                git_ignore.is_some(),
                status.reason
            );
        }
    }

    #[test]
    fn git_ignored_path_newer_than_index_is_not_stale() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let indexed_at = indexed_project(root, Some("generated/\n"));
        write_with_mtime(root, "generated/x.rs", indexed_at + Duration::from_secs(30));

        let status = check_index_status(root).unwrap();

        assert!(!status.needs_indexing, "{}", status.reason);
    }

    #[test]
    fn tracked_source_newer_than_index_is_stale() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let indexed_at = indexed_project(root, Some("worktrees/\n"));
        write_with_mtime(root, "src/main.rs", indexed_at + Duration::from_secs(30));

        let status = check_index_status(root).unwrap();

        assert!(status.needs_indexing);
        assert_eq!(status.reason, "stale (source files modified after index)");
    }

    #[test]
    fn untracked_source_that_git_does_not_ignore_is_stale() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let indexed_at = indexed_project(root, Some("worktrees/\n"));
        write_with_mtime(root, "src/new.rs", indexed_at + Duration::from_secs(30));

        let status = check_index_status(root).unwrap();

        assert!(status.needs_indexing);
        assert_eq!(status.estimated_files, 2);
    }

    // Issue #3249: the scan counts every file the indexer gives a language.

    #[test]
    fn every_cpp_extension_the_indexer_knows_counts_as_a_source() {
        for name in ["main.cc", "main.cxx"] {
            let project = tempfile::tempdir().unwrap();
            fs::write(project.path().join(name), "int main() { return 0; }\n").unwrap();

            let status = check_index_status(project.path()).unwrap();

            assert!(status.needs_indexing, "{name}: {}", status.reason);
            assert_eq!(status.estimated_files, 1, "{name}");
        }
    }
}
