use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::debug;

// ---------------------------------------------------------------------------
// ProjectConfig
// ---------------------------------------------------------------------------

/// Configuration for a blarify project, persisted to `~/.blarify/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub repo_id: String,
    pub entity_id: String,
    pub neo4j_uri: String,
    pub created_at: String,
    pub updated_at: String,
}

impl ProjectConfig {
    pub fn config_dir() -> std::path::PathBuf {
        dirs_or_home().join(".blarify")
    }

    pub fn projects_file() -> std::path::PathBuf {
        Self::config_dir().join("projects.json")
    }
}

fn dirs_or_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// ProjectDetector
// ---------------------------------------------------------------------------

/// Heuristic detection of project language based on config files and file extensions.
pub struct ProjectDetector;

const PYTHON_CONFIG_FILES: &[&str] = &[
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
    "Pipfile",
    "poetry.lock",
    "uv.lock",
];

const PYTHON_DIRECTORIES: &[&str] = &["__pycache__", ".venv", "venv", "env", ".pytest_cache"];

const PYTHON_FILE_EXTENSIONS: &[&str] = &[".py", ".pyx", ".pyi"];

const TYPESCRIPT_CONFIG_FILES: &[&str] = &[
    "tsconfig.json",
    "package.json",
    "yarn.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
];

const TYPESCRIPT_DIRECTORIES: &[&str] = &["node_modules", ".next", "dist", "build", ".nuxt"];

const TYPESCRIPT_FILE_EXTENSIONS: &[&str] = &[".ts", ".tsx", ".js", ".jsx"];

/// Non-source directories to skip during heuristic scanning.
const SCAN_SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "__pycache__",
    ".venv",
    "venv",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    "env",
];

/// Code file extensions for counting (beyond Python/TS).
const CODE_EXTENSIONS: &[&str] = &[
    ".py", ".pyx", ".pyi", ".ts", ".tsx", ".js", ".jsx", ".rs", ".go", ".java", ".c", ".cpp", ".h",
    ".hpp", ".cs", ".rb", ".php", ".swift", ".kt", ".scala",
];

impl ProjectDetector {
    /// Check if the given root is a Python project.
    pub fn is_python_project(root_path: &str) -> bool {
        let root = Path::new(root_path);

        // Check config files
        for f in PYTHON_CONFIG_FILES {
            if root.join(f).exists() {
                return true;
            }
        }

        // Check directories
        for d in PYTHON_DIRECTORIES {
            if root.join(d).is_dir() {
                return true;
            }
        }

        // Heuristic: count files at depth ≤ 3
        Self::language_ratio(root, PYTHON_FILE_EXTENSIONS) > 0.5
    }

    /// Check if the given root is a TypeScript/JavaScript project.
    pub fn is_typescript_project(root_path: &str) -> bool {
        let root = Path::new(root_path);

        // Check config files
        for f in TYPESCRIPT_CONFIG_FILES {
            if *f == "package.json" {
                if let Some(true) = Self::package_json_has_ts_indicators(root) {
                    return true;
                }
            } else if root.join(f).exists() {
                return true;
            }
        }

        // Check directories
        for d in TYPESCRIPT_DIRECTORIES {
            if root.join(d).is_dir() {
                return true;
            }
        }

        Self::language_ratio(root, TYPESCRIPT_FILE_EXTENSIONS) > 0.5
    }

    /// Determine the primary language of the project.
    pub fn get_primary_language(root_path: &str) -> Option<String> {
        if Self::is_python_project(root_path) {
            Some("python".into())
        } else if Self::is_typescript_project(root_path) {
            Some("typescript".into())
        } else {
            None
        }
    }

    /// Compute the ratio of target-language files to total code files.
    fn language_ratio(root: &Path, target_extensions: &[&str]) -> f64 {
        let target_set: HashSet<&str> = target_extensions.iter().copied().collect();
        let code_set: HashSet<&str> = CODE_EXTENSIONS.iter().copied().collect();

        let mut target_count = 0u64;
        let mut total_code = 0u64;

        Self::walk_depth(root, 0, 3, &mut |path| {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let dotted = format!(".{ext}");
                if code_set.contains(dotted.as_str()) {
                    total_code += 1;
                    if target_set.contains(dotted.as_str()) {
                        target_count += 1;
                    }
                }
            }
        });

        if total_code == 0 {
            0.0
        } else {
            target_count as f64 / total_code as f64
        }
    }

    fn walk_depth(dir: &Path, depth: u32, max_depth: u32, visitor: &mut impl FnMut(&Path)) {
        if depth > max_depth || !dir.is_dir() {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !SCAN_SKIP_DIRS.contains(&name.as_str()) {
                    Self::walk_depth(&path, depth + 1, max_depth, visitor);
                }
            } else {
                visitor(&path);
            }
        }
    }

    fn package_json_has_ts_indicators(root: &Path) -> Option<bool> {
        let pkg_path = root.join("package.json");
        if !pkg_path.exists() {
            return None;
        }
        let content = fs::read_to_string(&pkg_path).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;

        let ts_indicators = [
            "typescript",
            "@types/node",
            "ts-node",
            "tsc",
            "next",
            "react",
            "vue",
            "angular",
        ];

        for key in ["dependencies", "devDependencies"] {
            if let Some(deps) = parsed.get(key).and_then(|v| v.as_object()) {
                for indicator in &ts_indicators {
                    if deps.contains_key(*indicator) {
                        debug!(indicator = %indicator, "found TS indicator in package.json");
                        return Some(true);
                    }
                }
            }
        }

        Some(false)
    }
}

// ---------------------------------------------------------------------------
// PathCalculator
// ---------------------------------------------------------------------------

/// Utilities for manipulating file paths and URIs.
pub struct PathCalculator;

impl PathCalculator {
    /// Strip `file://` prefix from a URI.
    pub fn uri_to_path(uri: &str) -> &str {
        uri.strip_prefix("file://").unwrap_or(uri)
    }

    /// Extract the last directory component: `/path/to/dir` → `/dir/`.
    pub fn extract_last_directory(path: &str) -> String {
        let p = Path::new(path);
        let base = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        format!("/{base}/")
    }

    /// Compute a relative path prefixed with the root's last directory.
    pub fn compute_relative_path_with_prefix(pure_path: &str, root_path: &str) -> String {
        let rel = pure_path.strip_prefix(root_path).unwrap_or(pure_path);
        let prefix = Self::extract_last_directory(root_path);
        let trimmed_prefix = prefix.trim_end_matches('/');
        format!("{trimmed_prefix}{rel}")
    }

    /// Get the parent folder path from a file path.
    pub fn get_parent_folder_path(file_path: &str) -> String {
        let p = Path::new(file_path);
        p.parent()
            .map(|pp| pp.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Compute a relative path from a root URI.
    pub fn get_relative_path_from_uri(root_uri: &str, uri: &str) -> String {
        let root = Self::uri_to_path(root_uri);
        let path = Self::uri_to_path(uri);
        path.strip_prefix(root).unwrap_or(path).to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_calculator_uri_to_path() {
        assert_eq!(PathCalculator::uri_to_path("file:///repo/src"), "/repo/src");
        assert_eq!(PathCalculator::uri_to_path("/plain/path"), "/plain/path");
    }

    #[test]
    fn path_calculator_extract_last_directory() {
        assert_eq!(
            PathCalculator::extract_last_directory("/home/user/project"),
            "/project/"
        );
    }

    #[test]
    fn path_calculator_relative_path_with_prefix() {
        let result = PathCalculator::compute_relative_path_with_prefix(
            "/home/user/project/src/main.py",
            "/home/user/project",
        );
        assert_eq!(result, "/project/src/main.py");
    }

    #[test]
    fn path_calculator_parent_folder() {
        assert_eq!(
            PathCalculator::get_parent_folder_path("/repo/src/main.py"),
            "/repo/src"
        );
    }

    #[test]
    fn path_calculator_relative_from_uri() {
        let result =
            PathCalculator::get_relative_path_from_uri("file:///repo", "file:///repo/src/lib.rs");
        assert_eq!(result, "/src/lib.rs");
    }

    #[test]
    fn project_config_dir() {
        let dir = ProjectConfig::config_dir();
        assert!(dir.to_string_lossy().contains(".blarify"));
    }

    #[test]
    fn project_detector_nonexistent_is_not_python() {
        assert!(!ProjectDetector::is_python_project("/nonexistent/path/xyz"));
    }

    #[test]
    fn project_detector_nonexistent_is_not_typescript() {
        assert!(!ProjectDetector::is_typescript_project(
            "/nonexistent/path/xyz"
        ));
    }

    #[test]
    fn project_detector_primary_language_none() {
        assert_eq!(
            ProjectDetector::get_primary_language("/nonexistent/path/xyz"),
            None
        );
    }
}
