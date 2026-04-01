//! Type definitions for the code-graph subsystem.

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub(super) const BLARIFY_JSON_MAX_BYTES: u64 = 500 * 1024 * 1024;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(super) struct BlarifyOutput {
    #[serde(default)]
    pub(super) files: Vec<BlarifyFile>,
    #[serde(default)]
    pub(super) classes: Vec<BlarifyClass>,
    #[serde(default)]
    pub(super) functions: Vec<BlarifyFunction>,
    #[serde(default)]
    pub(super) imports: Vec<BlarifyImport>,
    #[serde(default)]
    pub(super) relationships: Vec<BlarifyRelationship>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct BlarifyFile {
    #[serde(default)]
    pub(super) path: String,
    #[serde(default)]
    pub(super) language: String,
    #[serde(default)]
    pub(super) lines_of_code: i64,
    #[serde(default)]
    pub(super) last_modified: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct BlarifyClass {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) file_path: String,
    #[serde(default)]
    pub(super) line_number: i64,
    #[serde(default)]
    pub(super) docstring: String,
    #[serde(default)]
    pub(super) is_abstract: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct BlarifyFunction {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) file_path: String,
    #[serde(default)]
    pub(super) line_number: i64,
    #[serde(default)]
    pub(super) docstring: String,
    #[serde(default)]
    pub(super) parameters: Vec<String>,
    #[serde(default)]
    pub(super) return_type: String,
    #[serde(default)]
    pub(super) is_async: bool,
    #[serde(default)]
    pub(super) complexity: i64,
    #[serde(default)]
    pub(super) class_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct BlarifyImport {
    #[serde(default)]
    pub(super) source_file: String,
    #[serde(default)]
    pub(super) target_file: String,
    #[serde(default)]
    pub(super) symbol: String,
    #[serde(default)]
    pub(super) alias: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct BlarifyRelationship {
    #[serde(default, rename = "type")]
    pub(super) relationship_type: String,
    #[serde(default)]
    pub(super) source_id: String,
    #[serde(default)]
    pub(super) target_id: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphImportCounts {
    pub files: usize,
    pub classes: usize,
    pub functions: usize,
    pub imports: usize,
    pub relationships: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphSummary {
    pub files: i64,
    pub classes: i64,
    pub functions: i64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphStats {
    pub files: i64,
    pub classes: i64,
    pub functions: i64,
    pub memory_file_links: i64,
    pub memory_function_links: i64,
}

impl From<CodeGraphStats> for CodeGraphSummary {
    fn from(value: CodeGraphStats) -> Self {
        Self {
            files: value.files,
            classes: value.classes,
            functions: value.functions,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphContextPayload {
    pub memory_id: String,
    pub files: Vec<CodeGraphContextFile>,
    pub functions: Vec<CodeGraphContextFunction>,
    pub classes: Vec<CodeGraphContextClass>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphContextFile {
    #[serde(rename = "type")]
    pub kind: String,
    pub path: String,
    pub language: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphContextFunction {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub signature: String,
    pub docstring: String,
    pub complexity: i64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphContextClass {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub fully_qualified_name: String,
    pub docstring: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphNamedEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphSearchEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CodeGraphEdgeEntry {
    pub caller: String,
    pub callee: String,
}

pub(crate) trait CodeGraphReaderBackend {
    fn stats(&self) -> Result<CodeGraphStats>;
    fn context_payload(&self, memory_id: &str) -> Result<CodeGraphContextPayload>;
    fn files(&self, pattern: Option<&str>, limit: u32) -> Result<Vec<String>>;
    fn functions(&self, file: Option<&str>, limit: u32) -> Result<Vec<CodeGraphNamedEntry>>;
    fn classes(&self, file: Option<&str>, limit: u32) -> Result<Vec<CodeGraphNamedEntry>>;
    fn search(&self, name: &str, limit: u32) -> Result<Vec<CodeGraphSearchEntry>>;
    fn callers(&self, name: &str, limit: u32) -> Result<Vec<CodeGraphEdgeEntry>>;
    fn callees(&self, name: &str, limit: u32) -> Result<Vec<CodeGraphEdgeEntry>>;
}

pub(super) trait CodeGraphWriterBackend {
    fn import_blarify_output(&self, payload: &BlarifyOutput) -> Result<CodeGraphImportCounts>;
}
