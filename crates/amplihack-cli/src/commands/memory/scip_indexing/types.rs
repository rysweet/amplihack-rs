use crate::commands::memory::code_graph::CodeGraphImportCounts;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(crate) const LANGUAGE_ORDER: &[&str] = &[
    "python",
    "typescript",
    "javascript",
    "go",
    "rust",
    "csharp",
    "cpp",
];

pub(super) const IGNORED_DIRS: &[&str] = &[
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageStatus {
    pub language: String,
    pub available: bool,
    pub error_message: Option<String>,
    pub missing_tools: Vec<String>,
    pub install_instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrerequisiteResult {
    pub can_proceed: bool,
    pub available_languages: Vec<String>,
    pub unavailable_languages: Vec<String>,
    pub partial_success: bool,
    pub language_statuses: BTreeMap<String, LanguageStatus>,
}

impl PrerequisiteResult {
    pub fn generate_report(&self) -> String {
        let mut lines = vec!["Prerequisite Check Report".to_string(), "=".repeat(40)];
        if !self.available_languages.is_empty() {
            lines.push(format!(
                "\nAvailable Languages ({}):",
                self.available_languages.len()
            ));
            for lang in &self.available_languages {
                lines.push(format!("  ✓ {lang}"));
            }
        }
        if !self.unavailable_languages.is_empty() {
            lines.push(format!(
                "\nUnavailable Languages ({}):",
                self.unavailable_languages.len()
            ));
            for lang in &self.unavailable_languages {
                if let Some(status) = self.language_statuses.get(lang) {
                    if let Some(error) = &status.error_message {
                        lines.push(format!("  ✗ {lang}: {error}"));
                        if let Some(instructions) = &status.install_instructions {
                            lines.push(format!("      Install: {instructions}"));
                        }
                    } else {
                        lines.push(format!("  ✗ {lang}"));
                    }
                }
            }
        }
        lines.push(format!("\nCan Proceed: {}", self.can_proceed));
        if self.partial_success {
            lines.push("Note: Partial success - some languages unavailable".to_string());
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScipIndexResult {
    pub language: String,
    pub success: bool,
    pub artifact_path: Option<PathBuf>,
    pub index_size_bytes: u64,
    pub duration_seconds: f64,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativeScipIndexSummary {
    pub success: bool,
    pub completed_languages: Vec<String>,
    pub failed_languages: Vec<String>,
    pub skipped_languages: Vec<String>,
    pub artifacts: Vec<PathBuf>,
    pub errors: Vec<String>,
    pub partial_success: bool,
    pub import_counts: CodeGraphImportCounts,
}
