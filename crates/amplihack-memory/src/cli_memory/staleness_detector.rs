use super::project_artifact_paths;
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

const IGNORED_DIRS: &[&str] = &[
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
];

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
    let index_file = resolve_index_artifact(&project_path)?;

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

fn resolve_index_artifact(project_path: &Path) -> Result<std::path::PathBuf> {
    let paths = project_artifact_paths(project_path)?;
    if paths.blarify_json.exists() {
        return Ok(paths.blarify_json);
    }
    if let Some(latest) = latest_scip_artifact(&paths.indexes_dir) {
        return Ok(latest);
    }
    Ok(paths.index_scip)
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

fn scan_source_files(project_path: &Path) -> (usize, Option<SystemTime>) {
    let mut count = 0usize;
    let mut newest_mtime = None;
    scan_dir(project_path, &mut count, &mut newest_mtime);
    (count, newest_mtime)
}

fn scan_dir(path: &Path, count: &mut usize, newest_mtime: &mut Option<SystemTime>) {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            return;
        }
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if entry_path.is_dir() {
            if should_ignore_dir(&file_name) {
                continue;
            }
            scan_dir(&entry_path, count, newest_mtime);
            continue;
        }

        if !entry_path.is_file() || !is_indexable_file(&entry_path) {
            continue;
        }

        *count += 1;
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                continue;
            }
            Err(_) => continue,
        };

        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(_) => continue,
        };

        if newest_mtime.is_none_or(|current| modified > current) {
            *newest_mtime = Some(modified);
        }
    }
}

fn should_ignore_dir(file_name: &str) -> bool {
    IGNORED_DIRS.contains(&file_name) || file_name.ends_with(".egg-info")
}

fn is_indexable_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| INDEXABLE_EXTENSIONS.contains(&ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_memory::artifact_root::project_artifact_root;
    use crate::test_support::{ArtifactCacheGuard, env_lock};
    use std::thread;
    use std::time::Duration;

    /// Isolate the artifact cache and hand back the project plus its artifact
    /// root. These tests write the fixtures the detector then reads, so they must
    /// write them where it looks: since #1476 that is the per-project cache, not
    /// `<project>/.amplihack`. Without the guard they resolved into the
    /// developer's real `~/.cache/amplihack` and the detector answered
    /// "missing", because the fixture was planted somewhere it never reads.
    fn project_with_isolated_cache() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        std::path::PathBuf,
        ArtifactCacheGuard,
    ) {
        let project = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let guard = ArtifactCacheGuard::set(cache.path());
        let root = project_artifact_root(project.path()).unwrap();
        (project, cache, root, guard)
    }

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
        let _lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (project, _cache, artifact_dir, _cache_guard) = project_with_isolated_cache();
        fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();
        thread::sleep(Duration::from_millis(20));
        fs::create_dir_all(&artifact_dir).unwrap();
        fs::write(artifact_dir.join("blarify.json"), "{}\n").unwrap();

        let status = check_index_status(project.path()).unwrap();

        assert!(!status.needs_indexing);
        assert_eq!(status.reason, "up-to-date (index is current)");
        assert!(status.last_indexed.is_some());
    }

    #[test]
    fn check_index_status_reports_stale_when_source_is_newer() {
        let _lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (project, _cache, artifact_dir, _cache_guard) = project_with_isolated_cache();
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
        let _lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (project, _cache, artifact_root, _cache_guard) = project_with_isolated_cache();
        fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();
        thread::sleep(Duration::from_millis(20));
        let artifact_dir = artifact_root.join("indexes");
        fs::create_dir_all(&artifact_dir).unwrap();
        fs::write(artifact_dir.join("rust.scip"), "scip-bytes").unwrap();

        let status = check_index_status(project.path()).unwrap();

        assert!(!status.needs_indexing);
        assert_eq!(status.reason, "up-to-date (index is current)");
        assert!(status.last_indexed.is_some());
    }
}
