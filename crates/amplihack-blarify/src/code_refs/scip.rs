use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use super::reference::{Reference, SymbolRole};
use crate::project::config::ProjectDetector;

// ---------------------------------------------------------------------------
// ScipReferenceResolver
// ---------------------------------------------------------------------------

/// Resolves code references using a SCIP (Source Code Intelligence Protocol)
/// index. Parses the protobuf index and builds lookup tables for symbol
/// occurrences.
///
/// This is a structural port — actual protobuf parsing requires the `scip`
/// crate or generated bindings. We model the lookup tables and resolution
/// logic here.
#[derive(Debug)]
pub struct ScipReferenceResolver {
    pub root_path: String,
    pub scip_index_path: Option<String>,
    pub language: String,
    loaded: bool,
    /// symbol → list of (relative_path, occurrence positions)
    symbol_occurrences: HashMap<String, Vec<OccurrenceRecord>>,
    /// relative_path → list of (symbol, occurrence)
    document_index: HashMap<String, Vec<SymbolOccurrence>>,
}

/// A recorded occurrence of a symbol in the index.
#[derive(Debug, Clone)]
pub struct OccurrenceRecord {
    pub relative_path: String,
    pub range: OccurrenceRange,
    pub role: u32,
    pub symbol: String,
}

/// Range format from SCIP (3 or 4 element).
#[derive(Debug, Clone)]
pub struct OccurrenceRange {
    pub start_line: u32,
    pub start_char: u32,
    pub end_line: u32,
    pub end_char: u32,
}

/// A symbol occurrence within a document.
#[derive(Debug, Clone)]
pub struct SymbolOccurrence {
    pub symbol: String,
    pub range: OccurrenceRange,
    pub role: u32,
}

impl ScipReferenceResolver {
    pub fn new(root_path: &str, scip_index_path: Option<String>) -> Self {
        let language =
            ProjectDetector::get_primary_language(root_path).unwrap_or_else(|| "python".into());

        Self {
            root_path: root_path.to_string(),
            scip_index_path,
            language,
            loaded: false,
            symbol_occurrences: HashMap::new(),
            document_index: HashMap::new(),
        }
    }

    /// Load the SCIP index (if not already loaded).
    pub fn ensure_loaded(&mut self) -> bool {
        if self.loaded {
            return true;
        }
        // In a full implementation, this would parse the protobuf index.
        // For the structural port, we mark as loaded.
        info!(path = ?self.scip_index_path, "SCIP index load requested");
        self.loaded = true;
        true
    }

    /// Check if a SCIP index exists at the expected path.
    pub fn index_exists(&self) -> bool {
        if let Some(ref path) = self.scip_index_path {
            Path::new(path).exists()
        } else {
            let default_path = Path::new(&self.root_path).join("index.scip");
            default_path.exists()
        }
    }

    /// Generate a SCIP index if needed.
    pub fn generate_index_if_needed(&self, project_name: &str) -> Result<bool> {
        if self.index_exists() {
            debug!("SCIP index already exists");
            return Ok(false);
        }

        info!(
            language = %self.language,
            project = %project_name,
            "generating SCIP index"
        );

        match self.language.as_str() {
            "python" => self.generate_python_index(project_name),
            "typescript" | "javascript" => self.generate_typescript_index(),
            lang => {
                warn!(language = %lang, "unsupported language for SCIP index generation");
                Ok(false)
            }
        }
    }

    fn generate_python_index(&self, project_name: &str) -> Result<bool> {
        let output_path = Path::new(&self.root_path).join("index.scip");

        let status = Command::new("scip-python")
            .args([
                "index",
                "--project-name",
                project_name,
                "--output",
                &output_path.to_string_lossy(),
                "--quiet",
            ])
            .current_dir(&self.root_path)
            .status()
            .context("failed to run scip-python")?;

        Ok(status.success())
    }

    fn generate_typescript_index(&self) -> Result<bool> {
        let _output_path = Path::new(&self.root_path).join("index.scip");

        let status = Command::new("scip-typescript")
            .args(["index", "--output", "index.scip"])
            .current_dir(&self.root_path)
            .status()
            .context("failed to run scip-typescript")?;

        Ok(status.success())
    }

    /// Check if an occurrence is a reference (not a definition).
    pub fn is_reference_occurrence(role: u32, language: &str) -> bool {
        // Must not be a definition
        if role & SymbolRole::DEFINITION != 0 {
            return false;
        }

        // For TS/JS, role 0 counts as reference
        if (language == "typescript" || language == "javascript") && role == 0 {
            return true;
        }

        // Must be read, write, or import
        role & (SymbolRole::READ_ACCESS | SymbolRole::WRITE_ACCESS | SymbolRole::IMPORT) != 0
    }

    /// Look up references for a symbol string.
    pub fn get_references_for_symbol(&self, symbol: &str) -> Vec<Reference> {
        self.symbol_occurrences
            .get(symbol)
            .map(|occurrences| {
                occurrences
                    .iter()
                    .filter(|occ| Self::is_reference_occurrence(occ.role, &self.language))
                    .filter_map(|occ| self.occurrence_to_reference(occ))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn occurrence_to_reference(&self, occ: &OccurrenceRecord) -> Option<Reference> {
        let uri = format!("file://{}/{}", self.root_path, occ.relative_path);
        Some(Reference::new(
            super::reference::Range::from_coords(
                occ.range.start_line,
                occ.range.start_char,
                occ.range.end_line,
                occ.range.end_char,
            ),
            uri,
        ))
    }

    /// Get statistics about the loaded index.
    pub fn get_statistics(&self) -> HashMap<String, usize> {
        let mut stats = HashMap::new();
        stats.insert("documents".into(), self.document_index.len());
        stats.insert("symbols".into(), self.symbol_occurrences.len());
        stats.insert(
            "total_occurrences".into(),
            self.symbol_occurrences.values().map(|v| v.len()).sum(),
        );
        stats
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scip_resolver_new() {
        let r = ScipReferenceResolver::new("/repo", None);
        assert_eq!(r.root_path, "/repo");
        assert!(!r.loaded);
    }

    #[test]
    fn scip_resolver_ensure_loaded() {
        let mut r = ScipReferenceResolver::new("/repo", None);
        assert!(r.ensure_loaded());
        assert!(r.loaded);
        // Second call should also return true
        assert!(r.ensure_loaded());
    }

    #[test]
    fn is_reference_occurrence_definition_excluded() {
        assert!(!ScipReferenceResolver::is_reference_occurrence(
            SymbolRole::DEFINITION,
            "python"
        ));
    }

    #[test]
    fn is_reference_occurrence_read_access() {
        assert!(ScipReferenceResolver::is_reference_occurrence(
            SymbolRole::READ_ACCESS,
            "python"
        ));
    }

    #[test]
    fn is_reference_occurrence_ts_zero_role() {
        assert!(ScipReferenceResolver::is_reference_occurrence(
            0,
            "typescript"
        ));
        assert!(!ScipReferenceResolver::is_reference_occurrence(0, "python"));
    }

    #[test]
    fn is_reference_occurrence_import() {
        assert!(ScipReferenceResolver::is_reference_occurrence(
            SymbolRole::IMPORT,
            "python"
        ));
    }

    #[test]
    fn get_references_for_unknown_symbol() {
        let r = ScipReferenceResolver::new("/repo", None);
        let refs = r.get_references_for_symbol("nonexistent.symbol");
        assert!(refs.is_empty());
    }

    #[test]
    fn get_statistics_empty() {
        let r = ScipReferenceResolver::new("/repo", None);
        let stats = r.get_statistics();
        assert_eq!(stats["documents"], 0);
        assert_eq!(stats["symbols"], 0);
    }
}
