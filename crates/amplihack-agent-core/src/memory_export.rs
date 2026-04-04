//! Memory export/import utilities for knowledge transfer between agents.
//!
//! Port of Python `memory_export.py`. Provides:
//! - [`export_memory`] — export an agent's memory to JSON
//! - [`import_memory`] — import memory from JSON into an agent
//! - [`ExportFormat`] — supported formats (JSON, Raw)
//!
//! The Python version also supports raw Kuzu DB copies; the Rust port
//! focuses on the JSON format since the in-memory backend has no raw DB
//! to copy.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AgentError, Result};
use crate::hierarchical_memory_local::HierarchicalMemoryLocal;

// ── ExportFormat ─────────────────────────────────────────────────────────

/// Supported export/import formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// JSON — portable, human-readable, supports merge.
    Json,
    /// Raw — direct storage copy (not supported for in-memory backend).
    Raw,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json => write!(f, "json"),
            Self::Raw => write!(f, "raw"),
        }
    }
}

// ── ExportMetadata ───────────────────────────────────────────────────────

/// Metadata returned by export/import operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportMetadata {
    pub agent_name: String,
    pub format: String,
    pub path: String,
    #[serde(default)]
    pub file_size_bytes: u64,
    #[serde(default)]
    pub statistics: HashMap<String, serde_json::Value>,
}

// ── Export ────────────────────────────────────────────────────────────────

/// Export an agent's in-memory memory to a JSON file.
///
/// # Errors
///
/// Returns an error if `agent_name` is empty, the format is unsupported
/// for in-memory backends, or writing the file fails.
pub fn export_memory(
    memory: &HierarchicalMemoryLocal,
    output_path: &Path,
    fmt: ExportFormat,
) -> Result<ExportMetadata> {
    if memory.agent_name().trim().is_empty() {
        return Err(AgentError::ConfigError("agent_name cannot be empty".into()));
    }
    if fmt == ExportFormat::Raw {
        return Err(AgentError::MemoryError(
            "Raw format not supported for in-memory backend. Use JSON.".into(),
        ));
    }

    let export_data = memory.export_to_json();

    if let Some(parent) = output_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }

    let json_str = serde_json::to_string_pretty(&export_data)?;
    std::fs::write(output_path, &json_str)?;

    let file_size = std::fs::metadata(output_path)?.len();
    let statistics = export_data
        .get("statistics")
        .and_then(|v| serde_json::from_value::<HashMap<String, serde_json::Value>>(v.clone()).ok())
        .unwrap_or_default();

    Ok(ExportMetadata {
        agent_name: memory.agent_name().to_string(),
        format: fmt.to_string(),
        path: output_path.to_string_lossy().into_owned(),
        file_size_bytes: file_size,
        statistics,
    })
}

// ── Import ────────────────────────────────────────────────────────────────

/// Import memory from a JSON file into an agent's in-memory store.
///
/// # Errors
///
/// Returns an error if the format is unsupported, the file doesn't exist,
/// or JSON parsing fails.
pub fn import_memory(
    memory: &mut HierarchicalMemoryLocal,
    input_path: &Path,
    fmt: ExportFormat,
    merge: bool,
) -> Result<ExportMetadata> {
    if fmt == ExportFormat::Raw {
        return Err(AgentError::MemoryError(
            "Raw format not supported for in-memory backend. Use JSON.".into(),
        ));
    }
    if !input_path.exists() {
        return Err(AgentError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Input path does not exist: {}", input_path.display()),
        )));
    }

    let json_str = std::fs::read_to_string(input_path)?;
    let data: serde_json::Value = serde_json::from_str(&json_str)?;

    let import_stats = memory.import_from_json(&data, merge);

    let source_agent = data
        .get("agent_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let mut statistics: HashMap<String, serde_json::Value> = import_stats;
    statistics.insert("source_agent".into(), serde_json::json!(source_agent));
    statistics.insert("merge".into(), serde_json::json!(merge));

    Ok(ExportMetadata {
        agent_name: memory.agent_name().to_string(),
        format: fmt.to_string(),
        path: input_path.to_string_lossy().into_owned(),
        file_size_bytes: std::fs::metadata(input_path).map(|m| m.len()).unwrap_or(0),
        statistics,
    })
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hierarchical_memory_types::{MemoryCategory, StoreKnowledgeParams};

    fn sk(mem: &mut HierarchicalMemoryLocal, content: &str, concept: &str, confidence: f64) {
        mem.store_knowledge(StoreKnowledgeParams {
            content,
            concept,
            confidence,
            category: MemoryCategory::Semantic,
            source_id: "",
            tags: &[],
            temporal_metadata: None,
        });
    }

    fn make_memory() -> HierarchicalMemoryLocal {
        let mut mem = HierarchicalMemoryLocal::new("export-test");
        sk(&mut mem, "Cells are alive", "Biology", 0.9);
        sk(&mut mem, "Quantum tunneling", "Physics", 0.8);
        mem
    }

    #[test]
    fn export_format_display() {
        assert_eq!(ExportFormat::Json.to_string(), "json");
        assert_eq!(ExportFormat::Raw.to_string(), "raw");
    }

    #[test]
    fn export_format_serde() {
        let json = serde_json::to_string(&ExportFormat::Json).unwrap();
        assert_eq!(json, "\"json\"");
        let parsed: ExportFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ExportFormat::Json);
    }

    #[test]
    fn export_json_creates_file() {
        let mem = make_memory();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.json");

        let meta = export_memory(&mem, &path, ExportFormat::Json).unwrap();
        assert_eq!(meta.agent_name, "export-test");
        assert_eq!(meta.format, "json");
        assert!(path.exists());
        assert!(meta.file_size_bytes > 0);
    }

    #[test]
    fn export_raw_not_supported() {
        let mem = make_memory();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.raw");
        assert!(export_memory(&mem, &path, ExportFormat::Raw).is_err());
    }

    #[test]
    fn import_roundtrip() {
        let mem = make_memory();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.json");

        export_memory(&mem, &path, ExportFormat::Json).unwrap();

        let mut mem2 = HierarchicalMemoryLocal::new("import-test");
        let meta = import_memory(&mut mem2, &path, ExportFormat::Json, false).unwrap();

        assert_eq!(meta.agent_name, "import-test");
        assert_eq!(
            meta.statistics["source_agent"],
            serde_json::json!("export-test")
        );
        assert_eq!(meta.statistics["imported_nodes"], serde_json::json!(2));
    }

    #[test]
    fn import_merge_mode() {
        let mem = make_memory();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.json");
        export_memory(&mem, &path, ExportFormat::Json).unwrap();

        let mut mem2 = HierarchicalMemoryLocal::new("import-test");
        sk(&mut mem2, "existing fact", "Existing", 0.7);

        import_memory(&mut mem2, &path, ExportFormat::Json, true).unwrap();
        assert_eq!(mem2.get_all_knowledge(50).len(), 3);
    }

    #[test]
    fn import_replace_mode() {
        let mem = make_memory();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.json");
        export_memory(&mem, &path, ExportFormat::Json).unwrap();

        let mut mem2 = HierarchicalMemoryLocal::new("import-test");
        sk(&mut mem2, "will be replaced", "Gone", 0.7);

        import_memory(&mut mem2, &path, ExportFormat::Json, false).unwrap();
        assert_eq!(mem2.get_all_knowledge(50).len(), 2);
    }

    #[test]
    fn import_file_not_found() {
        let mut mem = HierarchicalMemoryLocal::new("test");
        let result = import_memory(
            &mut mem,
            Path::new("/nonexistent/file.json"),
            ExportFormat::Json,
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn import_raw_not_supported() {
        let mut mem = HierarchicalMemoryLocal::new("test");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dummy.raw");
        std::fs::write(&path, "{}").unwrap();
        assert!(import_memory(&mut mem, &path, ExportFormat::Raw, false).is_err());
    }

    #[test]
    fn metadata_serde_roundtrip() {
        let meta = ExportMetadata {
            agent_name: "test".into(),
            format: "json".into(),
            path: "/some/path.json".into(),
            file_size_bytes: 1234,
            statistics: HashMap::new(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: ExportMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent_name, "test");
        assert_eq!(parsed.file_size_bytes, 1234);
    }
}
