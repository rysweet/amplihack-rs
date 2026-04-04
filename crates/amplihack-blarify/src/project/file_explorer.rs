use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::debug;

// ---------------------------------------------------------------------------
// File
// ---------------------------------------------------------------------------

/// A single source file discovered during project traversal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    pub name: String,
    pub root_path: String,
    pub level: u32,
}

impl File {
    pub fn new(name: impl Into<String>, root_path: impl Into<String>, level: u32) -> Self {
        Self {
            name: name.into(),
            root_path: root_path.into(),
            level,
        }
    }

    /// Full path on disk.
    pub fn path(&self) -> PathBuf {
        Path::new(&self.root_path).join(&self.name)
    }

    /// File extension including the dot, e.g. `.py`.
    pub fn extension(&self) -> String {
        Path::new(&self.name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default()
    }

    /// URI representation: `file://<full_path>`.
    pub fn uri_path(&self) -> String {
        format!("file://{}", self.path().display())
    }
}

impl std::fmt::Display for File {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path().display())
    }
}

// ---------------------------------------------------------------------------
// Folder
// ---------------------------------------------------------------------------

/// A directory discovered during project traversal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub name: String,
    pub path: String,
    pub files: Vec<File>,
    pub folders: Vec<Folder>,
    pub level: u32,
}

impl Folder {
    pub fn new(
        name: impl Into<String>,
        path: impl Into<String>,
        files: Vec<File>,
        folders: Vec<Folder>,
        level: u32,
    ) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            files,
            folders,
            level,
        }
    }

    /// URI representation: `file://<path>`.
    pub fn uri_path(&self) -> String {
        format!("file://{}", self.path)
    }
}

impl std::fmt::Display for Folder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", self.path)?;
        for file in &self.files {
            writeln!(f, "  {file}")?;
        }
        for folder in &self.folders {
            writeln!(f, "  {folder}")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ProjectFilesIterator
// ---------------------------------------------------------------------------

/// Walks a project directory tree, yielding [`Folder`] objects while
/// respecting skip lists and file-size limits.
pub struct ProjectFilesIterator {
    root_path: String,
    extensions_to_skip: Vec<String>,
    names_to_skip: Vec<String>,
    max_file_size_bytes: u64,
}

/// Default names that are always skipped during traversal.
const DEFAULT_SKIP_NAMES: &[&str] = &[
    "node_modules",
    ".git",
    "__pycache__",
    ".venv",
    "venv",
    ".idea",
    ".vs",
    ".vscode",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    "vendor",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    "env",
    ".env",
    "coverage",
    ".coverage",
    ".eggs",
    "*.egg-info",
];

impl ProjectFilesIterator {
    pub fn new(
        root_path: &str,
        extensions_to_skip: &[String],
        names_to_skip: &[String],
        blarignore_path: Option<&str>,
        max_file_size_mb: f64,
    ) -> Self {
        let mut all_names: Vec<String> = DEFAULT_SKIP_NAMES
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        all_names.extend(names_to_skip.iter().cloned());

        // Load .blarignore if available
        if let Some(ignore_path) = blarignore_path {
            let p = Path::new(ignore_path).join(".blarignore");
            if p.exists()
                && let Ok(content) = fs::read_to_string(&p)
            {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        all_names.push(trimmed.to_string());
                    }
                }
            }
        }

        Self {
            root_path: root_path.to_string(),
            extensions_to_skip: extensions_to_skip.to_vec(),
            names_to_skip: all_names,
            max_file_size_bytes: (max_file_size_mb * 1024.0 * 1024.0) as u64,
        }
    }

    /// Compute directory depth relative to root.
    fn path_level(&self, path: &Path) -> u32 {
        let root = Path::new(&self.root_path);
        path.strip_prefix(root)
            .map(|rel| rel.components().count() as u32)
            .unwrap_or(0)
    }

    fn should_skip_dir(&self, name: &str) -> bool {
        self.names_to_skip.iter().any(|skip| {
            if skip.contains('*') {
                // Simple glob: *.egg-info → ends_with
                let suffix = skip.trim_start_matches('*');
                name.ends_with(suffix)
            } else {
                name == skip
            }
        })
    }

    fn should_skip_file(&self, path: &Path, name: &str) -> bool {
        // Check name-based skip
        if self.names_to_skip.contains(&name.to_string()) {
            return true;
        }

        // Check extension-based skip
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let dotted = format!(".{ext}");
            if self.extensions_to_skip.contains(&dotted) {
                return true;
            }
        }

        // Check file size
        if let Ok(meta) = fs::metadata(path)
            && meta.len() > self.max_file_size_bytes
        {
            return true;
        }

        false
    }
}

impl IntoIterator for ProjectFilesIterator {
    type Item = Folder;
    type IntoIter = ProjectFilesIter;

    fn into_iter(self) -> Self::IntoIter {
        let root = PathBuf::from(&self.root_path);
        let mut stack = Vec::new();
        if root.is_dir() {
            stack.push(root);
        }
        ProjectFilesIter {
            stack,
            iterator: self,
        }
    }
}

/// Implements iteration by DFS-walking the filesystem.
pub struct ProjectFilesIter {
    stack: Vec<PathBuf>,
    iterator: ProjectFilesIterator,
}

impl Iterator for ProjectFilesIter {
    type Item = Folder;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(dir) = self.stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let level = self.iterator.path_level(&dir);
            let dir_name = dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();

            let mut files = Vec::new();
            let mut child_dirs = Vec::new();

            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();

                if path.is_dir() {
                    if !self.iterator.should_skip_dir(&name) {
                        child_dirs.push(path.clone());
                        self.stack.push(path);
                    }
                } else if path.is_file() && !self.iterator.should_skip_file(&path, &name) {
                    files.push(File::new(
                        name,
                        dir.to_string_lossy().to_string(),
                        level + 1,
                    ));
                }
            }

            debug!(
                path = %dir.display(),
                files = files.len(),
                subdirs = child_dirs.len(),
                "visiting directory"
            );

            let child_folders: Vec<Folder> = child_dirs
                .iter()
                .map(|d| {
                    let cname = d
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    Folder::new(
                        cname,
                        d.to_string_lossy().to_string(),
                        vec![],
                        vec![],
                        level + 1,
                    )
                })
                .collect();

            return Some(Folder::new(
                dir_name,
                dir.to_string_lossy().to_string(),
                files,
                child_folders,
                level,
            ));
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_extension() {
        let f = File::new("main.py", "/repo", 0);
        assert_eq!(f.extension(), ".py");
    }

    #[test]
    fn file_extension_none() {
        let f = File::new("Makefile", "/repo", 0);
        assert_eq!(f.extension(), "");
    }

    #[test]
    fn file_uri_path() {
        let f = File::new("src/main.rs", "/repo", 1);
        assert_eq!(f.uri_path(), "file:///repo/src/main.rs");
    }

    #[test]
    fn file_display() {
        let f = File::new("lib.rs", "/repo/src", 2);
        assert_eq!(f.to_string(), "/repo/src/lib.rs");
    }

    #[test]
    fn folder_uri_path() {
        let f = Folder::new("src", "/repo/src", vec![], vec![], 1);
        assert_eq!(f.uri_path(), "file:///repo/src");
    }

    #[test]
    fn iterator_skips_default_names() {
        let iter = ProjectFilesIterator::new("/repo", &[], &[], None, 0.8);
        assert!(iter.should_skip_dir("node_modules"));
        assert!(iter.should_skip_dir(".git"));
        assert!(iter.should_skip_dir("__pycache__"));
        assert!(!iter.should_skip_dir("src"));
    }

    #[test]
    fn iterator_skips_glob_patterns() {
        let iter = ProjectFilesIterator::new("/repo", &[], &[], None, 0.8);
        assert!(iter.should_skip_dir("mypackage.egg-info"));
    }

    #[test]
    fn iterator_skips_extensions() {
        let iter =
            ProjectFilesIterator::new("/repo", &[".pyc".into(), ".o".into()], &[], None, 0.8);
        assert!(iter.should_skip_file(Path::new("/repo/file.pyc"), "file.pyc"));
        assert!(!iter.should_skip_file(Path::new("/repo/file.py"), "file.py"));
    }

    #[test]
    fn iterator_on_nonexistent_dir_produces_nothing() {
        let iter = ProjectFilesIterator::new("/nonexistent/dir/12345", &[], &[], None, 0.8);
        let folders: Vec<Folder> = iter.into_iter().collect();
        assert!(folders.is_empty());
    }

    #[test]
    fn folder_display() {
        let folder = Folder::new(
            "src",
            "/repo/src",
            vec![File::new("main.rs", "/repo/src", 1)],
            vec![],
            0,
        );
        let s = folder.to_string();
        assert!(s.contains("/repo/src"));
        assert!(s.contains("main.rs"));
    }
}
