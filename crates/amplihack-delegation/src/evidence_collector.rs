//! Evidence collection from working directories.
//!
//! Scans a directory tree for artifacts generated during AI assistant execution
//! and classifies them by [`EvidenceType`]. Supports filtering by type, path
//! pattern, and custom exclusion globs.
//!
//! Ported from the Python `evidence_collector.py`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

use crate::error::{DelegationError, Result};
use crate::models::{EvidenceItem, EvidenceType};

// ---------------------------------------------------------------------------
// Evidence pattern map
// ---------------------------------------------------------------------------

/// File-extension / glob patterns used to classify evidence by type.
///
/// Mirrors the Python `EVIDENCE_PATTERNS` dictionary.
pub fn evidence_patterns() -> HashMap<EvidenceType, Vec<&'static str>> {
    let mut m = HashMap::new();
    m.insert(
        EvidenceType::CodeFile,
        vec![
            "*.py", "*.js", "*.ts", "*.java", "*.go", "*.rs", "*.cpp", "*.c", "*.h", "*.rb",
            "*.php", "*.swift", "*.kt",
        ],
    );
    m.insert(
        EvidenceType::TestFile,
        vec![
            "test_*.py",
            "*_test.py",
            "test_*.js",
            "*_test.js",
            "*.test.js",
            "*.test.ts",
            "*_test.go",
            "*Test.java",
            "*_spec.rb",
        ],
    );
    m.insert(
        EvidenceType::Documentation,
        vec!["README.md", "README.txt", "*.md", "GUIDE.md", "TUTORIAL.md"],
    );
    m.insert(
        EvidenceType::ArchitectureDoc,
        vec!["ARCHITECTURE.md", "DESIGN.md", "ADR-*.md"],
    );
    m.insert(
        EvidenceType::ApiSpec,
        vec![
            "openapi.yaml",
            "openapi.json",
            "swagger.yaml",
            "swagger.json",
            "api.yaml",
            "*.openapi.yaml",
        ],
    );
    m.insert(
        EvidenceType::TestResults,
        vec![
            "test-results.xml",
            "test-results.json",
            "coverage.xml",
            "pytest.xml",
        ],
    );
    m.insert(EvidenceType::ExecutionLog, vec!["*.log"]);
    m.insert(
        EvidenceType::ValidationReport,
        vec!["validation-report.md", "qa-report.md", "test-report.md"],
    );
    m.insert(
        EvidenceType::Diagram,
        vec!["*.mmd", "*.mermaid", "*.puml", "*.dot"],
    );
    m.insert(
        EvidenceType::Configuration,
        vec![
            "*.yaml", "*.yml", "*.json", "*.toml", "*.ini", "*.cfg", ".env",
        ],
    );
    m
}

/// Map file extensions to programming language names.
pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "py" => Some("python"),
        "js" => Some("javascript"),
        "ts" => Some("typescript"),
        "java" => Some("java"),
        "go" => Some("go"),
        "rs" => Some("rust"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "cpp" => Some("c++"),
        "c" | "h" => Some("c"),
        "swift" => Some("swift"),
        "kt" => Some("kotlin"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Default exclusion patterns
// ---------------------------------------------------------------------------

/// Default glob patterns for directories / files to skip.
const DEFAULT_EXCLUDES: &[&str] = &[
    "**/__pycache__/**",
    "**/*.pyc",
    "**/.git/**",
    "**/node_modules/**",
    "**/target/**",
];

/// Build a [`GlobSet`] from a slice of glob pattern strings.
fn build_glob_set(patterns: &[&str]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p).map_err(|e| DelegationError::Validation(e.to_string()))?;
        builder.add(g);
    }
    builder
        .build()
        .map_err(|e| DelegationError::Validation(e.to_string()))
}

// ---------------------------------------------------------------------------
// EvidenceCollector
// ---------------------------------------------------------------------------

/// Collects and organises evidence artifacts from a working directory.
///
/// # Example (conceptual)
/// ```rust,ignore
/// let collector = EvidenceCollector::new("/my/project", None);
/// let evidence = collector.collect(None, None, None)?;
/// ```
#[derive(Debug)]
pub struct EvidenceCollector {
    working_directory: PathBuf,
    evidence_priorities: Vec<EvidenceType>,
    collected: Vec<EvidenceItem>,
}

impl EvidenceCollector {
    /// Create a new collector rooted at `working_directory`.
    ///
    /// `evidence_priorities` optionally reorders which evidence types are
    /// collected first; it does *not* filter out unlisted types.
    pub fn new(
        working_directory: impl Into<PathBuf>,
        evidence_priorities: Option<Vec<EvidenceType>>,
    ) -> Self {
        Self {
            working_directory: working_directory.into(),
            evidence_priorities: evidence_priorities.unwrap_or_default(),
            collected: Vec::new(),
        }
    }

    /// Collect evidence, optionally filtering by type and exclusion patterns.
    ///
    /// * `execution_log` – if provided, injected as an [`EvidenceType::ExecutionLog`] item.
    /// * `evidence_types` – restrict collection to these types (default: all).
    /// * `exclude_patterns` – extra glob patterns to exclude on top of defaults.
    pub fn collect(
        &mut self,
        execution_log: Option<&str>,
        evidence_types: Option<&[EvidenceType]>,
        exclude_patterns: Option<&[&str]>,
    ) -> Result<&[EvidenceItem]> {
        let mut evidence: Vec<EvidenceItem> = Vec::new();
        let patterns = evidence_patterns();

        let mut excludes: Vec<&str> = DEFAULT_EXCLUDES.to_vec();
        if let Some(extra) = exclude_patterns {
            excludes.extend_from_slice(extra);
        }
        let exclude_set = build_glob_set(&excludes)?;

        // Decide which types to collect (priority-ordered if set).
        let all_types: Vec<EvidenceType> = if let Some(requested) = evidence_types {
            requested.to_vec()
        } else if !self.evidence_priorities.is_empty() {
            let mut ordered = self.evidence_priorities.clone();
            for t in patterns.keys() {
                if !ordered.contains(t) {
                    ordered.push(t.clone());
                }
            }
            ordered
        } else {
            patterns.keys().cloned().collect()
        };

        for ev_type in &all_types {
            // Special handling for execution log injection.
            if *ev_type == EvidenceType::ExecutionLog {
                if let Some(log) = execution_log {
                    let excerpt = truncate(log, 200);
                    evidence.push(EvidenceItem {
                        evidence_type: EvidenceType::ExecutionLog,
                        path: "<execution_log>".into(),
                        content: log.to_string(),
                        excerpt,
                        size_bytes: log.len() as u64,
                        timestamp: Utc::now(),
                        metadata: HashMap::new(),
                    });
                }
                continue;
            }

            let globs = match patterns.get(ev_type) {
                Some(g) => g,
                None => continue,
            };

            let match_set = build_glob_set(globs)?;

            for entry in WalkDir::new(&self.working_directory)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let rel = match entry.path().strip_prefix(&self.working_directory) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let rel_str = rel.to_string_lossy();

                // Check exclusions.
                if exclude_set.is_match(rel_str.as_ref()) {
                    continue;
                }

                // Match file name against type patterns.
                let file_name = match entry.path().file_name() {
                    Some(n) => n.to_string_lossy(),
                    None => continue,
                };
                if !match_set.is_match(file_name.as_ref()) {
                    continue;
                }

                // Avoid duplicates (same path already collected).
                if evidence.iter().any(|e| e.path == rel_str.as_ref()) {
                    continue;
                }

                match self.create_item(entry.path(), ev_type, rel) {
                    Ok(item) => evidence.push(item),
                    Err(_) => continue, // skip unreadable files
                }
            }
        }

        self.collected = evidence;
        Ok(&self.collected)
    }

    /// Return previously collected evidence items.
    pub fn collected(&self) -> &[EvidenceItem] {
        &self.collected
    }

    /// Filter collected evidence by type.
    pub fn get_by_type(&self, evidence_type: &EvidenceType) -> Vec<&EvidenceItem> {
        self.collected
            .iter()
            .filter(|e| &e.evidence_type == evidence_type)
            .collect()
    }

    /// Filter collected evidence by a glob on the `path` field.
    pub fn get_by_path_pattern(&self, pattern: &str) -> Result<Vec<&EvidenceItem>> {
        let g = Glob::new(pattern)
            .map_err(|e| DelegationError::Validation(e.to_string()))?
            .compile_matcher();
        Ok(self
            .collected
            .iter()
            .filter(|e| g.is_match(&e.path))
            .collect())
    }

    // -- private helpers ----------------------------------------------------

    fn create_item(
        &self,
        abs_path: &Path,
        evidence_type: &EvidenceType,
        rel_path: &Path,
    ) -> Result<EvidenceItem> {
        let content = std::fs::read_to_string(abs_path)?;
        let excerpt = truncate(&content, 200);
        let size_bytes = content.len() as u64;
        let mut metadata = HashMap::new();

        if let Some(ext) = abs_path.extension().and_then(|e| e.to_str())
            && let Some(lang) = language_for_extension(ext)
        {
            metadata.insert("language".into(), lang.into());
        }
        let line_count = content.matches('\n').count() + 1;
        metadata.insert("line_count".into(), line_count.to_string());

        Ok(EvidenceItem {
            evidence_type: evidence_type.clone(),
            path: rel_path.to_string_lossy().into_owned(),
            content,
            excerpt,
            size_bytes,
            timestamp: Utc::now(),
            metadata,
        })
    }
}

/// Truncate `s` to at most `max` characters.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s[..max].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_patterns_has_all_types() {
        let pats = evidence_patterns();
        assert!(pats.contains_key(&EvidenceType::CodeFile));
        assert!(pats.contains_key(&EvidenceType::TestFile));
        assert!(pats.contains_key(&EvidenceType::Documentation));
        assert!(pats.contains_key(&EvidenceType::Configuration));
        assert!(pats.len() >= 10);
    }

    #[test]
    fn language_for_known_extensions() {
        assert_eq!(language_for_extension("rs"), Some("rust"));
        assert_eq!(language_for_extension("py"), Some("python"));
        assert_eq!(language_for_extension("ts"), Some("typescript"));
        assert_eq!(language_for_extension("unknown"), None);
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 200), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(300);
        let t = truncate(&long, 200);
        assert_eq!(t.len(), 200);
    }

    #[test]
    fn build_glob_set_valid() {
        let gs = build_glob_set(&["*.rs", "*.py"]);
        assert!(gs.is_ok());
    }

    #[test]
    fn collector_new_defaults() {
        let c = EvidenceCollector::new("/some/path", None);
        assert!(c.evidence_priorities.is_empty());
        assert!(c.collected.is_empty());
    }
}
