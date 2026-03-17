//! Native blarify JSON → graph-db code-graph import.

pub(crate) mod backend;

use anyhow::{Context, Result, bail};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Component;
use std::path::{Path, PathBuf};
#[cfg(test)]
use time::OffsetDateTime;

const BLARIFY_JSON_MAX_BYTES: u64 = 500 * 1024 * 1024;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct BlarifyOutput {
    #[serde(default)]
    files: Vec<BlarifyFile>,
    #[serde(default)]
    classes: Vec<BlarifyClass>,
    #[serde(default)]
    functions: Vec<BlarifyFunction>,
    #[serde(default)]
    imports: Vec<BlarifyImport>,
    #[serde(default)]
    relationships: Vec<BlarifyRelationship>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BlarifyFile {
    #[serde(default)]
    path: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    lines_of_code: i64,
    #[serde(default)]
    last_modified: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BlarifyClass {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    file_path: String,
    #[serde(default)]
    line_number: i64,
    #[serde(default)]
    docstring: String,
    #[serde(default)]
    is_abstract: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BlarifyFunction {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    file_path: String,
    #[serde(default)]
    line_number: i64,
    #[serde(default)]
    docstring: String,
    #[serde(default)]
    parameters: Vec<String>,
    #[serde(default)]
    return_type: String,
    #[serde(default)]
    is_async: bool,
    #[serde(default)]
    complexity: i64,
    #[serde(default)]
    class_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BlarifyImport {
    #[serde(default)]
    source_file: String,
    #[serde(default)]
    target_file: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    alias: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BlarifyRelationship {
    #[serde(default, rename = "type")]
    relationship_type: String,
    #[serde(default)]
    source_id: String,
    #[serde(default)]
    target_id: String,
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

pub(crate) fn open_code_graph_reader(
    path_override: Option<&Path>,
) -> Result<Box<dyn CodeGraphReaderBackend>> {
    self::backend::open_code_graph_reader(path_override)
}

trait CodeGraphWriterBackend {
    fn import_blarify_output(&self, payload: &BlarifyOutput) -> Result<CodeGraphImportCounts>;
}

fn open_code_graph_writer(path_override: Option<&Path>) -> Result<Box<dyn CodeGraphWriterBackend>> {
    self::backend::open_code_graph_writer(path_override)
}

pub fn run_index_code(input: &Path, db_path: Option<&Path>) -> Result<()> {
    if let Some(notice) = code_graph_compatibility_notice_for_input(input, db_path)? {
        eprintln!("⚠️ Compatibility mode: {notice}");
    }
    let counts = import_blarify_json(input, db_path)?;
    println!("{}", serde_json::to_string_pretty(&counts)?);
    Ok(())
}

pub fn import_scip_file(
    input_path: &Path,
    project_root: &Path,
    language_hint: Option<&str>,
    db_path: Option<&Path>,
) -> Result<CodeGraphImportCounts> {
    if !input_path.exists() {
        bail!("SCIP index not found: {}", input_path.display());
    }
    let input_path = validate_index_path(input_path)?;
    let project_root = validate_index_path(project_root)?;

    let bytes = fs::read(&input_path)
        .with_context(|| format!("failed to read {}", input_path.display()))?;
    let index = ScipIndex::decode(bytes.as_slice())
        .with_context(|| format!("invalid SCIP protobuf in {}", input_path.display()))?;
    let payload = convert_scip_to_blarify(&index, &project_root, language_hint);

    let default_db_path;
    let path_override = match db_path {
        Some(path) => Some(path),
        None => {
            default_db_path = default_code_graph_db_path_for_project(&project_root)?;
            Some(default_db_path.as_path())
        }
    };
    open_code_graph_writer(path_override)?.import_blarify_output(&payload)
}

pub fn import_blarify_json(
    input_path: &Path,
    db_path: Option<&Path>,
) -> Result<CodeGraphImportCounts> {
    if !input_path.exists() {
        bail!("blarify JSON not found: {}", input_path.display());
    }
    let input_path = validate_index_path(input_path)?;
    validate_blarify_json_size(&input_path, BLARIFY_JSON_MAX_BYTES)?;

    let raw = fs::read_to_string(&input_path)
        .with_context(|| format!("failed to read {}", input_path.display()))?;
    let payload: BlarifyOutput = serde_json::from_str(&raw)
        .with_context(|| format!("invalid JSON in {}", input_path.display()))?;

    let inferred_db_path;
    let path_override = match db_path {
        Some(path) => Some(path),
        None => {
            inferred_db_path = infer_code_graph_db_path_from_input(&input_path)?;
            Some(inferred_db_path.as_path())
        }
    };

    open_code_graph_writer(path_override)?.import_blarify_output(&payload)
}

fn default_code_graph_db_path() -> Result<PathBuf> {
    resolve_code_graph_db_path_for_project(
        &std::env::current_dir()
            .context("failed to resolve current directory for default code graph path")?,
    )
}

pub fn default_code_graph_db_path_for_project(project_root: &Path) -> Result<PathBuf> {
    Ok(project_root.join(".amplihack").join("graph_db"))
}

pub fn code_graph_compatibility_notice_for_project(
    project_root: &Path,
    db_path_override: Option<&Path>,
) -> Result<Option<String>> {
    if db_path_override.is_some() {
        return Ok(None);
    }

    let graph_override = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    if graph_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(None);
    }

    let legacy_override = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    if legacy_override
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(Some(
            "using legacy `AMPLIHACK_KUZU_DB_PATH`; prefer `AMPLIHACK_GRAPH_DB_PATH`.".to_string(),
        ));
    }

    let neutral = default_code_graph_db_path_for_project(project_root)?;
    let legacy = project_root.join(".amplihack").join("kuzu_db");
    if legacy.exists() && !neutral.exists() {
        return Ok(Some(format!(
            "using legacy code-graph store `{}` because `{}` is absent; migrate to `graph_db`.",
            legacy.display(),
            neutral.display()
        )));
    }

    Ok(None)
}

pub fn code_graph_compatibility_notice_for_input(
    input_path: &Path,
    db_path_override: Option<&Path>,
) -> Result<Option<String>> {
    let Some(parent) = input_path.parent() else {
        return code_graph_compatibility_notice_for_project(
            &std::env::current_dir().context(
                "failed to resolve current directory for code graph compatibility notice",
            )?,
            db_path_override,
        );
    };
    let is_blarify_json =
        input_path.file_name().and_then(|name| name.to_str()) == Some("blarify.json");
    let is_project_amplihack_dir =
        parent.file_name().and_then(|name| name.to_str()) == Some(".amplihack");
    if is_blarify_json
        && is_project_amplihack_dir
        && let Some(project_root) = parent.parent()
    {
        return code_graph_compatibility_notice_for_project(project_root, db_path_override);
    }
    code_graph_compatibility_notice_for_project(
        &std::env::current_dir()
            .context("failed to resolve current directory for code graph compatibility notice")?,
        db_path_override,
    )
}

fn validate_graph_db_env_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("graph DB path must be absolute: {}", path.display());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "graph DB path must not contain parent traversal: {}",
            path.display()
        );
    }
    for blocked in ["/proc", "/sys", "/dev"] {
        if path.starts_with(blocked) {
            bail!("blocked unsafe path prefix: {blocked}");
        }
    }
    Ok(path.to_path_buf())
}

fn graph_db_env_override(var_name: &str) -> Result<Option<PathBuf>> {
    let Some(path) = std::env::var_os(var_name) else {
        return Ok(None);
    };
    if path.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(path);
    let validated = validate_graph_db_env_path(&path)
        .with_context(|| format!("invalid {var_name} override: {}", path.display()))?;
    Ok(Some(validated))
}

fn safe_legacy_graph_db_path(project_root: &Path, neutral: &Path) -> Result<Option<PathBuf>> {
    let legacy = project_root.join(".amplihack").join("kuzu_db");
    if !legacy.exists() || neutral.exists() {
        return Ok(None);
    }

    let canonical_project_root = project_root.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project root while validating legacy graph DB shim: {}",
            project_root.display()
        )
    })?;
    match legacy.canonicalize() {
        Ok(canonical_legacy) if canonical_legacy.starts_with(&canonical_project_root) => {
            Ok(Some(legacy))
        }
        Ok(canonical_legacy) => bail!(
            "legacy graph DB shim escapes project root: {} -> {} (project root: {})",
            legacy.display(),
            canonical_legacy.display(),
            canonical_project_root.display()
        ),
        Err(err) => Err(err).with_context(|| {
            format!(
                "failed to canonicalize legacy graph DB shim: {}",
                legacy.display()
            )
        }),
    }
}

pub fn resolve_code_graph_db_path_for_project(project_root: &Path) -> Result<PathBuf> {
    if let Some(path) = graph_db_env_override("AMPLIHACK_GRAPH_DB_PATH")? {
        return Ok(path);
    }
    if let Some(path) = graph_db_env_override("AMPLIHACK_KUZU_DB_PATH")? {
        return Ok(path);
    }
    let neutral = default_code_graph_db_path_for_project(project_root)?;
    if let Some(legacy) = safe_legacy_graph_db_path(project_root, &neutral)? {
        return Ok(legacy);
    }
    Ok(neutral)
}

fn infer_code_graph_db_path_from_input(input_path: &Path) -> Result<PathBuf> {
    let Some(parent) = input_path.parent() else {
        return default_code_graph_db_path();
    };
    let is_blarify_json =
        input_path.file_name().and_then(|name| name.to_str()) == Some("blarify.json");
    let is_project_amplihack_dir =
        parent.file_name().and_then(|name| name.to_str()) == Some(".amplihack");
    if is_blarify_json && is_project_amplihack_dir {
        let Some(project_root) = parent.parent() else {
            return default_code_graph_db_path();
        };
        return resolve_code_graph_db_path_for_project(project_root);
    }
    default_code_graph_db_path()
}

pub fn summarize_code_graph(db_path: Option<&Path>) -> Result<Option<CodeGraphSummary>> {
    let path = match db_path {
        Some(path) => path.to_path_buf(),
        None => default_code_graph_db_path()?,
    };
    if !path.exists() {
        return Ok(None);
    }

    let stats = open_code_graph_reader(Some(&path))?.stats()?;
    Ok(Some(stats.into()))
}

fn convert_scip_to_blarify(
    index: &ScipIndex,
    project_root: &Path,
    language_hint: Option<&str>,
) -> BlarifyOutput {
    let mut payload = BlarifyOutput::default();

    for doc in &index.documents {
        let language = if doc.language.trim().is_empty() {
            language_hint.unwrap_or_default().to_string()
        } else {
            doc.language.clone()
        };
        let file_path = project_root.join(&doc.relative_path);
        let file_path = file_path.to_string_lossy().replace('\\', "/");
        let lines_of_code = doc.text.lines().count() as i64;
        payload.files.push(BlarifyFile {
            path: file_path.clone(),
            language,
            lines_of_code,
            last_modified: None,
        });

        for symbol in &doc.symbols {
            let symbol_name = symbol.symbol.trim();
            if symbol_name.is_empty() {
                continue;
            }

            let line_number = find_definition_line(symbol_name, &doc.occurrences);
            let docstring = symbol.documentation.join(" ");

            if is_function_symbol(symbol) {
                payload.functions.push(BlarifyFunction {
                    id: symbol_name.to_string(),
                    name: extract_name_from_symbol(symbol_name),
                    file_path: file_path.clone(),
                    line_number,
                    docstring,
                    parameters: Vec::new(),
                    return_type: String::new(),
                    is_async: false,
                    complexity: 0,
                    class_id: enclosing_class_id(symbol),
                });
            } else if is_class_symbol(symbol) {
                payload.classes.push(BlarifyClass {
                    id: symbol_name.to_string(),
                    name: extract_name_from_symbol(symbol_name),
                    file_path: file_path.clone(),
                    line_number,
                    docstring,
                    is_abstract: matches!(symbol.kind, SCIP_KIND_INTERFACE | SCIP_KIND_TRAIT),
                });
            }
        }
    }

    payload
}

fn find_definition_line(symbol: &str, occurrences: &[ScipOccurrence]) -> i64 {
    occurrences
        .iter()
        .find(|occ| occ.symbol == symbol && (occ.symbol_roles & SCIP_SYMBOL_ROLE_DEFINITION) != 0)
        .and_then(|occ| occ.range.first().copied())
        .map(i64::from)
        .unwrap_or(0)
}

fn extract_name_from_symbol(symbol: &str) -> String {
    if let Some(part) = symbol.rsplit('/').next() {
        return part
            .trim_end_matches('.')
            .trim_end_matches("()")
            .to_string();
    }
    if let Some(part) = symbol.split_whitespace().last() {
        return part
            .trim_end_matches('.')
            .trim_end_matches("()")
            .to_string();
    }
    symbol
        .trim_end_matches('.')
        .trim_end_matches("()")
        .to_string()
}

fn enclosing_class_id(symbol: &ScipSymbolInformation) -> Option<String> {
    let enclosing = symbol.enclosing_symbol.trim();
    if enclosing.is_empty() || !is_class_symbol_by_name(enclosing) {
        return None;
    }
    Some(enclosing.to_string())
}

fn is_function_symbol(symbol: &ScipSymbolInformation) -> bool {
    matches!(
        symbol.kind,
        SCIP_KIND_FUNCTION
            | SCIP_KIND_METHOD
            | SCIP_KIND_CONSTRUCTOR
            | SCIP_KIND_PROTOCOL_METHOD
            | SCIP_KIND_STATIC_METHOD
            | SCIP_KIND_TRAIT_METHOD
            | SCIP_KIND_ABSTRACT_METHOD
            | SCIP_KIND_PURE_VIRTUAL_METHOD
    ) || symbol.symbol.contains('(')
}

fn is_class_symbol(symbol: &ScipSymbolInformation) -> bool {
    matches!(
        symbol.kind,
        SCIP_KIND_CLASS
            | SCIP_KIND_INTERFACE
            | SCIP_KIND_STRUCT
            | SCIP_KIND_TRAIT
            | SCIP_KIND_OBJECT
            | SCIP_KIND_TYPE
            | SCIP_KIND_MODULE
            | SCIP_KIND_ENUM
    ) || is_class_symbol_by_name(&symbol.symbol)
}

fn is_class_symbol_by_name(symbol: &str) -> bool {
    if symbol.contains('(') {
        return false;
    }
    let name = extract_name_from_symbol(symbol);
    !name.is_empty()
        && name.chars().next().is_some_and(|ch| ch.is_uppercase())
        && !name.chars().all(|ch| ch.is_uppercase())
}

const SCIP_SYMBOL_ROLE_DEFINITION: i32 = 1;
const SCIP_KIND_CLASS: i32 = 7;
const SCIP_KIND_CONSTRUCTOR: i32 = 9;
const SCIP_KIND_ENUM: i32 = 11;
const SCIP_KIND_FUNCTION: i32 = 17;
const SCIP_KIND_METHOD: i32 = 26;
const SCIP_KIND_INTERFACE: i32 = 21;
const SCIP_KIND_MODULE: i32 = 29;
const SCIP_KIND_OBJECT: i32 = 33;
const SCIP_KIND_PROTOCOL_METHOD: i32 = 68;
const SCIP_KIND_PURE_VIRTUAL_METHOD: i32 = 69;
const SCIP_KIND_STATIC_METHOD: i32 = 80;
const SCIP_KIND_STRUCT: i32 = 49;
const SCIP_KIND_TRAIT: i32 = 53;
const SCIP_KIND_TRAIT_METHOD: i32 = 70;
const SCIP_KIND_TYPE: i32 = 54;
const SCIP_KIND_ABSTRACT_METHOD: i32 = 66;

#[derive(Clone, PartialEq, Message)]
struct ScipIndex {
    #[prost(message, repeated, tag = "2")]
    documents: Vec<ScipDocument>,
}

#[derive(Clone, PartialEq, Message)]
struct ScipDocument {
    #[prost(string, tag = "4")]
    language: String,
    #[prost(string, tag = "1")]
    relative_path: String,
    #[prost(message, repeated, tag = "2")]
    occurrences: Vec<ScipOccurrence>,
    #[prost(message, repeated, tag = "3")]
    symbols: Vec<ScipSymbolInformation>,
    #[prost(string, tag = "5")]
    text: String,
}

#[derive(Clone, PartialEq, Message)]
struct ScipSymbolInformation {
    #[prost(string, tag = "1")]
    symbol: String,
    #[prost(string, repeated, tag = "3")]
    documentation: Vec<String>,
    #[prost(int32, tag = "5")]
    kind: i32,
    #[prost(string, tag = "6")]
    display_name: String,
    #[prost(string, tag = "8")]
    enclosing_symbol: String,
}

#[derive(Clone, PartialEq, Message)]
struct ScipOccurrence {
    #[prost(int32, repeated, tag = "1")]
    range: Vec<i32>,
    #[prost(string, tag = "2")]
    symbol: String,
    #[prost(int32, tag = "3")]
    symbol_roles: i32,
}

// ── Issue #77 security & validation functions ─────────────────────────────
//
// These functions implement the security and validation contracts for Issue #77.
// All implementations are complete and the tests below pass.

/// Validate that `path` is safe to use as a project root or input path.
///
/// Contract:
/// - Canonicalize the path (resolve symlinks / `..` components).
/// - Return `Err` if the resolved path starts with `/proc`, `/sys`, or `/dev`.
/// - Return `Ok(canonical_path)` for all other paths.
///
/// Security note (P2-PATH): callers must use the *returned* canonical path,
/// not the original input, to prevent TOCTOU races.
pub(crate) fn validate_index_path(path: &Path) -> Result<PathBuf> {
    for blocked in [Path::new("/proc"), Path::new("/sys"), Path::new("/dev")] {
        if path.starts_with(blocked) {
            bail!("blocked unsafe path prefix: {}", blocked.display());
        }
    }

    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", path.display()))?;
    for blocked in [Path::new("/proc"), Path::new("/sys"), Path::new("/dev")] {
        if canonical.starts_with(blocked) {
            bail!("blocked unsafe path prefix: {}", blocked.display());
        }
    }
    Ok(canonical)
}

/// Assert that the Kuzu DB directory has restrictive Unix permissions.
///
/// Contract (P1-PERM, Unix only):
/// - The DB *parent* directory must be mode `0o700`.
/// - If Kuzu created a DB *file* (not a directory), that file must be `0o600`.
/// - On non-Unix platforms this is a no-op (returns `Ok(())`).
///
/// Must be called after the code-graph DB has been initialised so the path
/// exists on disk.
#[cfg_attr(not(unix), allow(unused_variables))]
pub(crate) fn enforce_db_permissions(db_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        if let Some(parent) = db_path.parent()
            && parent.exists()
        {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("failed to secure {}", parent.display()))?;
        }

        if db_path.exists() {
            let mode = if db_path.is_dir() { 0o700 } else { 0o600 };
            fs::set_permissions(db_path, fs::Permissions::from_mode(mode))
                .with_context(|| format!("failed to secure {}", db_path.display()))?;
        }
    }
    Ok(())
}

/// Guard against deserialising a pathologically large `blarify.json`.
///
/// Contract (P2-SIZE):
/// - If the file at `path` is larger than `max_bytes`, return `Err` with a
///   message containing "size" or "large".
/// - If the file does not exist, return `Err` (caller decides how to handle).
/// - If the file is within the limit, return `Ok(())`.
///
/// The production limit is 500 MiB (`500 * 1024 * 1024`).  Tests may pass a
/// smaller limit to exercise the guard without writing 500 MB of data.
pub(crate) fn validate_blarify_json_size(path: &Path, max_bytes: u64) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.len() > max_bytes {
        bail!(
            "blarify JSON size {} exceeds configured limit {} bytes",
            metadata.len(),
            max_bytes
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::memory::backend::kuzu::{
        KuzuValue, init_kuzu_backend_schema, kuzu_i64, kuzu_rows,
    };
    use crate::commands::memory::code_graph::backend::{
        initialize_test_code_graph_db, with_test_code_graph_conn,
    };
    use crate::test_support::{cwd_env_lock, restore_cwd, set_cwd};
    use tempfile::TempDir;

    fn sample_blarify_output() -> BlarifyOutput {
        serde_json::from_value(serde_json::json!({
            "files": [
                {
                    "path": "src/example/module.py",
                    "language": "python",
                    "lines_of_code": 100,
                    "last_modified": "2025-01-01T00:00:00Z"
                },
                {
                    "path": "src/example/utils.py",
                    "language": "python",
                    "lines_of_code": 50,
                    "last_modified": "2025-01-01T00:00:00Z"
                }
            ],
            "classes": [{
                "id": "class:Example",
                "name": "Example",
                "file_path": "src/example/module.py",
                "line_number": 10,
                "docstring": "Example class for testing.",
                "is_abstract": false
            }],
            "functions": [
                {
                    "id": "func:Example.process",
                    "name": "process",
                    "file_path": "src/example/module.py",
                    "line_number": 20,
                    "docstring": "Process data.",
                    "parameters": ["self", "data"],
                    "return_type": "str",
                    "is_async": false,
                    "complexity": 3,
                    "class_id": "class:Example"
                },
                {
                    "id": "func:helper",
                    "name": "helper",
                    "file_path": "src/example/utils.py",
                    "line_number": 5,
                    "docstring": "Helper function.",
                    "parameters": ["x"],
                    "return_type": "int",
                    "is_async": false,
                    "complexity": 1,
                    "class_id": null
                }
            ],
            "imports": [{
                "source_file": "src/example/module.py",
                "target_file": "src/example/utils.py",
                "symbol": "helper",
                "alias": null
            }],
            "relationships": [{
                "type": "CALLS",
                "source_id": "func:Example.process",
                "target_id": "func:helper"
            }]
        }))
        .unwrap()
    }

    fn temp_code_graph_db() -> Result<(TempDir, PathBuf)> {
        let dir = TempDir::new().map_err(|e| anyhow::anyhow!("tempdir: {e}"))?;
        let db_path = dir.path().join("code-graph.kuzu");
        Ok((dir, db_path))
    }

    #[test]
    fn import_blarify_json_populates_kuzu_code_graph() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        let json_dir = TempDir::new().unwrap();
        let json_path = json_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
        )
        .unwrap();

        let counts = import_blarify_json(&json_path, Some(&db_path)).unwrap();

        assert_eq!(
            counts,
            CodeGraphImportCounts {
                files: 2,
                classes: 1,
                functions: 2,
                imports: 1,
                relationships: 1,
            }
        );

        with_test_code_graph_conn(Some(&db_path), |conn| {
            let rows = kuzu_rows(conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)", vec![])?;
            assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 2);
            let rows = kuzu_rows(
                conn,
                "MATCH (source:CodeFunction {function_id: $source_id})-[r:CALLS]->(target:CodeFunction {function_id: $target_id}) RETURN COUNT(r)",
                vec![
                    ("source_id", KuzuValue::String("func:Example.process".to_string())),
                    ("target_id", KuzuValue::String("func:helper".to_string())),
                ],
            )?;
            assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn import_blarify_json_updates_without_duplicates() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        let json_dir = TempDir::new().unwrap();
        let json_path = json_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
        )
        .unwrap();

        let first = import_blarify_json(&json_path, Some(&db_path)).unwrap();
        let second = import_blarify_json(&json_path, Some(&db_path)).unwrap();

        assert_eq!(first.files, 2);
        assert_eq!(second.files, 2);

        with_test_code_graph_conn(Some(&db_path), |conn| {
            let rows = kuzu_rows(conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)", vec![])?;
            assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 2);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn import_blarify_json_links_semantic_memory_by_metadata_file() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        with_test_code_graph_conn(Some(&db_path), |conn| {
            init_kuzu_backend_schema(conn)?;
            let now = OffsetDateTime::now_utc();

            let mut create_memory = conn.prepare(
                "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, category: $category, confidence_score: $confidence_score, last_updated: $last_updated, version: $version, title: $title, metadata: $metadata, tags: $tags, created_at: $created_at, accessed_at: $accessed_at, agent_id: $agent_id})",
            )?;
            conn.execute(
                &mut create_memory,
                vec![
                    ("memory_id", KuzuValue::String("mem-1".to_string())),
                    ("concept", KuzuValue::String("Example memory".to_string())),
                    (
                        "content",
                        KuzuValue::String("Remember module.py".to_string()),
                    ),
                    ("category", KuzuValue::String("session_end".to_string())),
                    ("confidence_score", KuzuValue::Double(1.0)),
                    ("last_updated", KuzuValue::Timestamp(now)),
                    ("version", KuzuValue::Int64(1)),
                    ("title", KuzuValue::String("Example".to_string())),
                    (
                        "metadata",
                        KuzuValue::String(r#"{"file":"src/example/module.py"}"#.to_string()),
                    ),
                    ("tags", KuzuValue::String(r#"["learning"]"#.to_string())),
                    ("created_at", KuzuValue::Timestamp(now)),
                    ("accessed_at", KuzuValue::Timestamp(now)),
                    ("agent_id", KuzuValue::String("agent-1".to_string())),
                ],
            )?;
            Ok(())
        })
        .unwrap();

        let json_dir = TempDir::new().unwrap();
        let json_path = json_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
        )
        .unwrap();

        import_blarify_json(&json_path, Some(&db_path)).unwrap();
        import_blarify_json(&json_path, Some(&db_path)).unwrap();

        with_test_code_graph_conn(Some(&db_path), |conn| {
            let rows = kuzu_rows(
                conn,
                "MATCH (m:SemanticMemory {memory_id: $memory_id})-[r:RELATES_TO_FILE_SEMANTIC]->(cf:CodeFile {file_id: $file_id}) RETURN COUNT(r)",
                vec![
                    ("memory_id", KuzuValue::String("mem-1".to_string())),
                    (
                        "file_id",
                        KuzuValue::String("src/example/module.py".to_string()),
                    ),
                ],
            )?;
            assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn import_blarify_json_links_semantic_memory_by_function_name() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        with_test_code_graph_conn(Some(&db_path), |conn| {
            init_kuzu_backend_schema(conn)?;
            let now = OffsetDateTime::now_utc();

            let mut create_memory = conn.prepare(
                "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, category: $category, confidence_score: $confidence_score, last_updated: $last_updated, version: $version, title: $title, metadata: $metadata, tags: $tags, created_at: $created_at, accessed_at: $accessed_at, agent_id: $agent_id})",
            )?;
            conn.execute(
                &mut create_memory,
                vec![
                    ("memory_id", KuzuValue::String("mem-func".to_string())),
                    ("concept", KuzuValue::String("Helper memory".to_string())),
                    (
                        "content",
                        KuzuValue::String("Remember to call helper before returning.".to_string()),
                    ),
                    ("category", KuzuValue::String("session_end".to_string())),
                    ("confidence_score", KuzuValue::Double(1.0)),
                    ("last_updated", KuzuValue::Timestamp(now)),
                    ("version", KuzuValue::Int64(1)),
                    ("title", KuzuValue::String("Helper".to_string())),
                    ("metadata", KuzuValue::String("{}".to_string())),
                    ("tags", KuzuValue::String(r#"["learning"]"#.to_string())),
                    ("created_at", KuzuValue::Timestamp(now)),
                    ("accessed_at", KuzuValue::Timestamp(now)),
                    ("agent_id", KuzuValue::String("agent-1".to_string())),
                ],
            )?;
            Ok(())
        })
        .unwrap();

        let json_dir = TempDir::new().unwrap();
        let json_path = json_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
        )
        .unwrap();

        import_blarify_json(&json_path, Some(&db_path)).unwrap();
        import_blarify_json(&json_path, Some(&db_path)).unwrap();

        with_test_code_graph_conn(Some(&db_path), |conn| {
            let rows = kuzu_rows(
                conn,
                "MATCH (m:SemanticMemory {memory_id: $memory_id})-[r:RELATES_TO_FUNCTION_SEMANTIC]->(f:CodeFunction {function_id: $function_id}) RETURN COUNT(r)",
                vec![
                    ("memory_id", KuzuValue::String("mem-func".to_string())),
                    ("function_id", KuzuValue::String("func:helper".to_string())),
                ],
            )?;
            assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn default_code_graph_db_path_uses_project_local_store() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = TempDir::new().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };
        let previous = set_cwd(dir.path()).unwrap();

        let path = default_code_graph_db_path().unwrap();

        restore_cwd(&previous).unwrap();
        match prev_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }
        assert_eq!(path, dir.path().join(".amplihack").join("graph_db"));
    }

    #[test]
    fn default_code_graph_db_path_prefers_existing_legacy_project_store() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = TempDir::new().unwrap();
        let previous = set_cwd(dir.path()).unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };
        let legacy = dir.path().join(".amplihack").join("kuzu_db");
        fs::create_dir_all(&legacy).unwrap();

        let path = default_code_graph_db_path().unwrap();

        restore_cwd(&previous).unwrap();
        match prev_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }
        assert_eq!(path, legacy);
    }

    #[test]
    fn default_code_graph_db_path_prefers_backend_neutral_override() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = TempDir::new().unwrap();
        let previous = set_cwd(dir.path()).unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/tmp/graph-override") };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/tmp/kuzu-override") };

        let path = default_code_graph_db_path().unwrap();

        restore_cwd(&previous).unwrap();
        match prev_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }
        assert_eq!(path, PathBuf::from("/tmp/graph-override"));
    }

    #[test]
    fn summarize_code_graph_reads_imported_counts() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        let json_dir = TempDir::new().unwrap();
        let json_path = json_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
        )
        .unwrap();

        import_blarify_json(&json_path, Some(&db_path)).unwrap();
        let summary = summarize_code_graph(Some(&db_path))
            .unwrap()
            .expect("summary should exist");

        assert_eq!(
            summary,
            CodeGraphSummary {
                files: 2,
                classes: 1,
                functions: 2,
            }
        );
    }

    fn sample_scip_index() -> ScipIndex {
        ScipIndex {
            documents: vec![ScipDocument {
                language: "python".to_string(),
                relative_path: "src/example/module.py".to_string(),
                text: "class Example:\n    pass\n\ndef helper():\n    return 1\n".to_string(),
                occurrences: vec![
                    ScipOccurrence {
                        range: vec![0, 6, 0, 13],
                        symbol: "scip-python python pkg src/example/module.py/Example.".to_string(),
                        symbol_roles: SCIP_SYMBOL_ROLE_DEFINITION,
                    },
                    ScipOccurrence {
                        range: vec![3, 4, 3, 10],
                        symbol: "scip-python python pkg src/example/module.py/helper()."
                            .to_string(),
                        symbol_roles: SCIP_SYMBOL_ROLE_DEFINITION,
                    },
                ],
                symbols: vec![
                    ScipSymbolInformation {
                        symbol: "scip-python python pkg src/example/module.py/Example.".to_string(),
                        documentation: vec!["Example class".to_string()],
                        kind: SCIP_KIND_CLASS,
                        display_name: "Example".to_string(),
                        enclosing_symbol: String::new(),
                    },
                    ScipSymbolInformation {
                        symbol: "scip-python python pkg src/example/module.py/helper()."
                            .to_string(),
                        documentation: vec!["Helper".to_string()],
                        kind: SCIP_KIND_FUNCTION,
                        display_name: "helper".to_string(),
                        enclosing_symbol: String::new(),
                    },
                ],
            }],
        }
    }

    #[test]
    fn import_scip_file_populates_kuzu_code_graph() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        let project_dir = TempDir::new().unwrap();
        let src_dir = project_dir.path().join("src/example");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("module.py"), "class Example:\n    pass\n").unwrap();

        let scip_dir = TempDir::new().unwrap();
        let scip_path = scip_dir.path().join("index.scip");
        fs::write(&scip_path, sample_scip_index().encode_to_vec()).unwrap();

        let counts = import_scip_file(
            &scip_path,
            project_dir.path(),
            Some("python"),
            Some(&db_path),
        )
        .unwrap();

        assert_eq!(counts.files, 1);
        assert_eq!(counts.classes, 1);
        assert_eq!(counts.functions, 1);

        with_test_code_graph_conn(Some(&db_path), |conn| {
            let rows = kuzu_rows(conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)", vec![])?;
            assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
            let rows = kuzu_rows(conn, "MATCH (f:CodeFunction) RETURN COUNT(f)", vec![])?;
            assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
            let rows = kuzu_rows(conn, "MATCH (c:CodeClass) RETURN COUNT(c)", vec![])?;
            assert_eq!(kuzu_i64(rows[0].first()).unwrap(), 1);
            Ok(())
        })
        .unwrap();
    }

    // ── Issue #77 security & validation tests ─────────────────────────────
    //
    // These tests verify the security and validation behaviour implemented for
    // Issue #77.  All four groups pass with the current implementation:
    //
    //   1. `import_blarify_json_absent_returns_error` — PASS:
    //      absent blarify.json now returns an explicit error.
    //
    //   2. `enforce_db_permissions_sets_restrictive_unix_modes` — PASSES:
    //      `enforce_db_permissions()` sets 0o700/0o600 on DB paths (Unix).
    //
    //   3. `validate_index_path_*` — PASS: path canonicalization + blocklist
    //      for /proc, /sys, /dev is implemented and working.
    //
    //   4. `validate_blarify_json_size_*` — PASS: size guard rejects files
    //      exceeding BLARIFY_JSON_MAX_BYTES before deserialization.

    // ── (1) Missing blarify JSON must fail explicitly ──────────────────────

    /// AC7 / R5 hardening: when blarify.json does not exist, direct import must
    /// return an error instead of a success-shaped empty result. Missing input
    /// is a real failure that callers must surface or handle deliberately.
    #[test]
    fn import_blarify_json_absent_returns_error() {
        let (_dir, db_path) = temp_code_graph_db().unwrap();
        // Use a path that is guaranteed not to exist.
        let missing = PathBuf::from("/tmp/__amplihack_tdd_absent_blarify_i77__.json");
        let _ = std::fs::remove_file(&missing); // ensure it really is absent

        let result = import_blarify_json(&missing, Some(&db_path));

        assert!(
            result.is_err(),
            "Expected Err when blarify.json is absent, but got Ok: {:?}",
            result.ok()
        );
        let error = result.err().unwrap();
        assert!(
            error.to_string().contains("blarify JSON not found"),
            "missing-file error should be explicit, got: {error}"
        );
    }

    // ── (2) DB permissions enforcement ────────────────────────────────────

    /// P1-PERM: After Kuzu initialises the database the parent directory must
    /// be mode 0o700 and the DB path itself 0o600 (or 0o700 if Kuzu creates a
    /// directory rather than a flat file).
    ///
    /// P1-PERM: DB parent directory must be 0o700; DB file/dir must be 0o600/0o700.
    #[test]
    #[cfg(unix)]
    fn enforce_db_permissions_sets_restrictive_unix_modes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("secured.kuzu");

        // Initialise the DB so the path exists on disk.
        initialize_test_code_graph_db(Some(&db_path)).unwrap();

        // Call the enforcement function under test.
        enforce_db_permissions(&db_path).expect("enforce_db_permissions should succeed");

        // The parent directory must be 0o700.
        let parent_meta = fs::metadata(dir.path()).unwrap();
        let parent_mode = parent_meta.permissions().mode() & 0o777;
        assert_eq!(
            parent_mode, 0o700,
            "parent directory should be mode 0o700, got 0o{parent_mode:o}"
        );

        // The DB itself (file or directory Kuzu creates) must be 0o600 / 0o700.
        if db_path.exists() {
            let db_meta = fs::metadata(&db_path).unwrap();
            let db_mode = db_meta.permissions().mode() & 0o777;
            assert!(
                db_mode == 0o600 || db_mode == 0o700,
                "DB path should be mode 0o600 or 0o700, got 0o{db_mode:o}"
            );
        }
    }

    // ── (3) Path validation ───────────────────────────────────────────────

    /// P2-PATH: `/proc` subtrees must be rejected.
    #[test]
    fn validate_index_path_blocks_proc_prefix() {
        let result = validate_index_path(Path::new("/proc/1/mem"));
        assert!(
            result.is_err(),
            "Expected Err for /proc path, got Ok({:?})",
            result.ok()
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("/proc") || msg.to_lowercase().contains("blocked"),
            "Error message should mention the blocked prefix, got: {msg}"
        );
    }

    /// P2-PATH: `/sys` subtrees must be rejected.
    #[test]
    fn validate_index_path_blocks_sys_prefix() {
        let result = validate_index_path(Path::new("/sys/kernel/config"));
        assert!(
            result.is_err(),
            "Expected Err for /sys path, got Ok({:?})",
            result.ok()
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("/sys") || msg.to_lowercase().contains("blocked"),
            "Error message should mention the blocked prefix, got: {msg}"
        );
    }

    /// P2-PATH: `/dev` subtrees must be rejected.
    #[test]
    fn validate_index_path_blocks_dev_prefix() {
        let result = validate_index_path(Path::new("/dev/null"));
        assert!(
            result.is_err(),
            "Expected Err for /dev path, got Ok({:?})",
            result.ok()
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("/dev") || msg.to_lowercase().contains("blocked"),
            "Error message should mention the blocked prefix, got: {msg}"
        );
    }

    /// P2-PATH: Normal temp paths must be allowed through.
    #[test]
    fn validate_index_path_allows_normal_temp_path() {
        let dir = TempDir::new().unwrap();
        // Create a real subdirectory so canonicalize() can resolve it.
        let project_dir = dir.path().join("my_project");
        fs::create_dir_all(&project_dir).unwrap();

        let result = validate_index_path(&project_dir);
        assert!(
            result.is_ok(),
            "Expected Ok for a normal temp directory, got Err: {:?}",
            result.err()
        );
        // The returned path must be the canonicalized form.
        let canonical = result.unwrap();
        assert!(
            canonical.is_absolute(),
            "validate_index_path must return an absolute canonical path"
        );
    }

    /// P2-PATH: Paths that *look* like blocked prefixes but are not (e.g.
    /// `/proc_data`) must be allowed.
    #[test]
    fn validate_index_path_allows_path_with_proc_in_name_not_prefix() {
        let dir = TempDir::new().unwrap();
        // e.g. /tmp/abc/proc_data — should NOT be blocked
        let allowed = dir.path().join("proc_data");
        fs::create_dir_all(&allowed).unwrap();

        let result = validate_index_path(&allowed);
        assert!(
            result.is_ok(),
            "Path containing 'proc' as a *directory name* (not prefix) should be \
             allowed, got Err: {:?}",
            result.err()
        );
    }

    // ── (4) Blarify JSON size guard ───────────────────────────────────────

    /// P2-SIZE: A file that exceeds the configured byte limit must be rejected
    /// BEFORE serde_json deserialization to prevent memory exhaustion.
    #[test]
    fn validate_blarify_json_size_rejects_file_exceeding_limit() {
        let dir = TempDir::new().unwrap();
        let json_path = dir.path().join("blarify.json");
        // Write 100 bytes of valid-ish JSON-like content.
        fs::write(
            &json_path,
            b"{\"files\":[],\"classes\":[],\"functions\":[],\"imports\":[],\"relationships\":[]}",
        )
        .unwrap();

        // With a 0-byte limit, ANY non-empty file must be rejected.
        let result = validate_blarify_json_size(&json_path, 0);
        assert!(
            result.is_err(),
            "Expected Err when file exceeds the 0-byte limit, got Ok"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.to_lowercase().contains("size")
                || msg.to_lowercase().contains("large")
                || msg.to_lowercase().contains("exceed")
                || msg.to_lowercase().contains("limit"),
            "Error message should explain why the file was rejected, got: {msg}"
        );
    }

    /// P2-SIZE: A file that is WITHIN the configured limit must be accepted.
    #[test]
    fn validate_blarify_json_size_accepts_file_within_limit() {
        let dir = TempDir::new().unwrap();
        let json_path = dir.path().join("blarify.json");
        let content =
            b"{\"files\":[],\"classes\":[],\"functions\":[],\"imports\":[],\"relationships\":[]}";
        fs::write(&json_path, content).unwrap();

        // 500 MiB limit — content is ~80 bytes, well within bounds.
        let max: u64 = 500 * 1024 * 1024;
        let result = validate_blarify_json_size(&json_path, max);
        assert!(
            result.is_ok(),
            "Expected Ok when file is within the size limit, got Err: {:?}",
            result.err()
        );
    }

    /// P2-SIZE: A missing file must also be rejected (not silently pass the
    /// size guard to then crash in the reader).
    #[test]
    fn validate_blarify_json_size_rejects_missing_file() {
        let missing = PathBuf::from("/tmp/__amplihack_tdd_absent_size_check_i77__.json");
        let _ = std::fs::remove_file(&missing);

        let result = validate_blarify_json_size(&missing, 500 * 1024 * 1024);
        assert!(
            result.is_err(),
            "Expected Err for a missing file in validate_blarify_json_size, got Ok"
        );
    }

    // ── (5) resolve_code_graph_db_path_for_project — I77 path resolution ────────
    //
    // These tests define the full behavior contract for the two public path
    // resolution functions introduced in Issue #77:
    //
    //   default_code_graph_db_path_for_project()  — returns .amplihack/graph_db
    //   resolve_code_graph_db_path_for_project()  — 4-level precedence resolver
    //
    // These tests lock the hardening contract for graph DB path resolution:
    // unsafe env overrides are rejected, and the legacy disk shim must remain
    // contained within the project root before it can activate.

    /// I77-DEFAULT: default_code_graph_db_path_for_project() must return
    /// `.amplihack/graph_db` regardless of env vars — it is a pure default query
    /// with no env-var override semantics.
    #[test]
    fn default_code_graph_db_path_for_project_returns_graph_db() {
        let dir = TempDir::new().unwrap();
        let result = default_code_graph_db_path_for_project(dir.path()).unwrap();
        assert_eq!(
            result,
            dir.path().join(".amplihack").join("graph_db"),
            "default_code_graph_db_path_for_project must return .amplihack/graph_db (not kuzu_db)"
        );
    }

    /// I77-KUZU-ENV: When only AMPLIHACK_KUZU_DB_PATH is set (the legacy alias)
    /// and AMPLIHACK_GRAPH_DB_PATH is absent, resolve_code_graph_db_path_for_project
    /// must accept the legacy env var as the active path.
    #[test]
    fn resolve_code_graph_db_path_for_project_uses_kuzu_env_as_legacy_alias() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = TempDir::new().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
        unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/tmp/legacy-kuzu-alias") };

        let path = resolve_code_graph_db_path_for_project(dir.path()).unwrap();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert_eq!(
            path,
            PathBuf::from("/tmp/legacy-kuzu-alias"),
            "AMPLIHACK_KUZU_DB_PATH must be used as a legacy alias when AMPLIHACK_GRAPH_DB_PATH \
             is unset"
        );
    }

    /// I77-SEC-TRAVERSE: An env var whose value contains a path-traversal component
    /// (`..`) must be REJECTED. resolve_code_graph_db_path_for_project() must
    /// surface an error instead of silently falling through to the default path.
    ///
    /// Security reference: design spec validate_graph_db_env_path() requirement —
    /// "must not contain '..' components".
    ///
    #[test]
    fn resolve_code_graph_db_path_for_project_env_var_traversal_rejected() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = TempDir::new().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        // Env var value contains a ".." path traversal component.
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/tmp/../etc/shadow") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

        let error = resolve_code_graph_db_path_for_project(dir.path()).unwrap_err();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let rendered = format!("{error:#}");
        assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
        assert!(rendered.contains("graph DB path must not contain parent traversal"));
    }

    /// I77-SEC-RELATIVE: A non-absolute path in AMPLIHACK_GRAPH_DB_PATH must be
    /// rejected. resolve_code_graph_db_path_for_project() must surface an error
    /// instead of silently falling through to the default path.
    ///
    /// Security reference: design spec validate_graph_db_env_path() requirement —
    /// "must be absolute".
    ///
    #[test]
    fn resolve_code_graph_db_path_for_project_env_var_relative_path_rejected() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = TempDir::new().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        // Relative (non-absolute) path — should be rejected.
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "relative/path/to/graph_db") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

        let error = resolve_code_graph_db_path_for_project(dir.path()).unwrap_err();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let rendered = format!("{error:#}");
        assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
        assert!(rendered.contains("graph DB path must be absolute"));
    }

    /// I77-SEC-PROC: A `/proc`-prefixed path in AMPLIHACK_GRAPH_DB_PATH must be
    /// rejected. resolve_code_graph_db_path_for_project() must surface an error
    /// instead of silently falling through to the default path.
    ///
    /// Security reference: design spec validate_graph_db_env_path() requirement —
    /// "must not start with /proc, /sys, or /dev".
    ///
    #[test]
    fn resolve_code_graph_db_path_for_project_env_var_proc_prefix_rejected() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = TempDir::new().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/proc/1/mem") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

        let error = resolve_code_graph_db_path_for_project(dir.path()).unwrap_err();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let rendered = format!("{error:#}");
        assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
        assert!(rendered.contains("blocked unsafe path prefix"));
    }

    /// I77-SEC-SYMLINK: The legacy `kuzu_db` disk-shim must NOT activate when the
    /// `kuzu_db` path is a symbolic link whose canonical target resolves outside the
    /// project root. The shim must surface an error instead of silently falling
    /// through to the default path.
    ///
    /// Security reference: design spec — "Symlink attack on disk probe: legacy
    /// kuzu_db path canonicalized and verified to start_with(project_root) before
    /// the shim activates".
    ///
    #[test]
    #[cfg(unix)]
    fn resolve_code_graph_db_path_for_project_disk_shim_blocks_escaping_symlink() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
        unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

        // Create .amplihack/ inside the project root.
        let amplihack_dir = dir.path().join(".amplihack");
        fs::create_dir_all(&amplihack_dir).unwrap();

        // Create a symlink: <project>/.amplihack/kuzu_db → <outside tempdir>
        // The symlink resolves OUTSIDE the project root, simulating a symlink
        // escape / TOCTOU attack.
        let kuzu_symlink = amplihack_dir.join("kuzu_db");
        std::os::unix::fs::symlink(outside.path(), &kuzu_symlink).unwrap();

        let error = resolve_code_graph_db_path_for_project(dir.path()).unwrap_err();

        match prev_graph {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match prev_kuzu {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let rendered = format!("{error:#}");
        assert!(rendered.contains("legacy graph DB shim escapes project root"));
        assert!(rendered.contains(kuzu_symlink.to_string_lossy().as_ref()));
    }
}
