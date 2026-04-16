use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::file_explorer::ProjectFilesIterator;

// ---------------------------------------------------------------------------
// FileStats
// ---------------------------------------------------------------------------

/// Stats for a single source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStats {
    pub name: String,
    pub lines_count: usize,
    pub size: u64,
}

// ---------------------------------------------------------------------------
// ProjectFileStats
// ---------------------------------------------------------------------------

/// Collects size and line-count statistics for all files in a project.
#[derive(Debug)]
pub struct ProjectFileStats {
    stats: Vec<FileStats>,
}

impl ProjectFileStats {
    /// Analyze all files reachable by the given iterator.
    pub fn analyze(iterator: ProjectFilesIterator) -> Self {
        let mut stats = Vec::new();

        for folder in iterator {
            for file in &folder.files {
                let path = file.path();
                let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let lines = Self::count_lines(&path);
                stats.push(FileStats {
                    name: path.to_string_lossy().into_owned(),
                    lines_count: lines,
                    size,
                });
            }
        }

        stats.sort_by_key(|s| std::cmp::Reverse(s.size));

        Self { stats }
    }

    /// Get stats for a specific file path.
    pub fn get_file_stats(&self, file_path: &str) -> Option<&FileStats> {
        self.stats.iter().find(|s| s.name == file_path)
    }

    /// Return the top N files by size.
    pub fn top_by_size(&self, limit: usize) -> &[FileStats] {
        let end = limit.min(self.stats.len());
        &self.stats[..end]
    }

    /// Total number of files analyzed.
    pub fn file_count(&self) -> usize {
        self.stats.len()
    }

    /// Total lines across all files.
    pub fn total_lines(&self) -> usize {
        self.stats.iter().map(|s| s.lines_count).sum()
    }

    fn count_lines(path: &Path) -> usize {
        fs::read_to_string(path)
            .map(|content| content.lines().count())
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// NestingStats / CodeComplexityCalculator
// ---------------------------------------------------------------------------

/// Code complexity metrics based on indentation depth.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComplexityStats {
    pub max_indentation: f64,
    pub min_indentation: f64,
    pub average_indentation: f64,
    pub standard_deviation: f64,
}

/// Calculates complexity metrics from source code text.
pub struct CodeComplexityCalculator;

impl CodeComplexityCalculator {
    /// Calculate nesting stats from raw source text.
    pub fn calculate_nesting_stats(code: &str) -> ComplexityStats {
        let depths: Vec<f64> = code
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let indent = line.len() - line.trim_start().len();
                indent as f64
            })
            .collect();

        if depths.is_empty() {
            return ComplexityStats::default();
        }

        let max = depths.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min = depths.iter().cloned().fold(f64::INFINITY, f64::min);
        let sum: f64 = depths.iter().sum();
        let avg = sum / depths.len() as f64;

        let variance = depths.iter().map(|d| (d - avg).powi(2)).sum::<f64>() / depths.len() as f64;
        let sd = variance.sqrt();

        ComplexityStats {
            max_indentation: max,
            min_indentation: min,
            average_indentation: avg,
            standard_deviation: sd,
        }
    }

    /// Count the number of parameters in a function signature string.
    pub fn calculate_parameter_count(signature: &str) -> u32 {
        // Extract content between first ( and last )
        let start = match signature.find('(') {
            Some(i) => i + 1,
            None => return 0,
        };
        let end = match signature.rfind(')') {
            Some(i) => i,
            None => return 0,
        };

        if start >= end {
            return 0;
        }

        let params = &signature[start..end];
        if params.trim().is_empty() {
            return 0;
        }

        // Split by comma, filtering out 'self' for Python methods
        params
            .split(',')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty() && *p != "self" && *p != "cls")
            .count() as u32
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complexity_empty_code() {
        let stats = CodeComplexityCalculator::calculate_nesting_stats("");
        assert_eq!(stats.max_indentation, 0.0);
    }

    #[test]
    fn complexity_flat_code() {
        let code = "line1\nline2\nline3\n";
        let stats = CodeComplexityCalculator::calculate_nesting_stats(code);
        assert_eq!(stats.max_indentation, 0.0);
        assert_eq!(stats.min_indentation, 0.0);
        assert_eq!(stats.average_indentation, 0.0);
    }

    #[test]
    fn complexity_nested_code() {
        let code = "def foo():\n    if True:\n        pass\n";
        let stats = CodeComplexityCalculator::calculate_nesting_stats(code);
        assert_eq!(stats.max_indentation, 8.0);
        assert_eq!(stats.min_indentation, 0.0);
        assert!(stats.average_indentation > 0.0);
    }

    #[test]
    fn complexity_ignores_blank_lines() {
        let code = "a\n\n    b\n\n";
        let stats = CodeComplexityCalculator::calculate_nesting_stats(code);
        assert_eq!(stats.max_indentation, 4.0);
        assert_eq!(stats.min_indentation, 0.0);
    }

    #[test]
    fn parameter_count_empty() {
        assert_eq!(
            CodeComplexityCalculator::calculate_parameter_count("def f()"),
            0
        );
    }

    #[test]
    fn parameter_count_some() {
        assert_eq!(
            CodeComplexityCalculator::calculate_parameter_count("def f(a, b, c)"),
            3
        );
    }

    #[test]
    fn parameter_count_self_excluded() {
        assert_eq!(
            CodeComplexityCalculator::calculate_parameter_count("def f(self, a, b)"),
            2
        );
    }

    #[test]
    fn parameter_count_no_parens() {
        assert_eq!(
            CodeComplexityCalculator::calculate_parameter_count("no parens"),
            0
        );
    }

    #[test]
    fn file_stats_struct() {
        let s = FileStats {
            name: "test.py".into(),
            lines_count: 100,
            size: 2048,
        };
        assert_eq!(s.lines_count, 100);
    }

    #[test]
    fn project_file_stats_empty_iterator() {
        let iter = ProjectFilesIterator::new("/nonexistent/dir/12345", &[], &[], None, 0.8);
        let stats = ProjectFileStats::analyze(iter);
        assert_eq!(stats.file_count(), 0);
        assert_eq!(stats.total_lines(), 0);
    }
}
