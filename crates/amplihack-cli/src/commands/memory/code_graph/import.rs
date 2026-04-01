//! Import and summarization entry points for the code-graph subsystem.

use super::paths::{
    code_graph_compatibility_notice_for_input, default_code_graph_db_path,
    default_code_graph_db_path_for_project, infer_code_graph_db_path_from_input,
};
use super::scip::{ScipIndex, convert_scip_to_blarify};
use super::types::{
    BlarifyOutput, CodeGraphImportCounts, CodeGraphReaderBackend, CodeGraphSummary,
    CodeGraphWriterBackend, BLARIFY_JSON_MAX_BYTES,
};
use super::validation::{validate_blarify_json_size, validate_index_path};

use anyhow::{Context, Result, bail};
use prost::Message;
use std::fs;
use std::path::Path;

pub(crate) fn open_code_graph_reader(
    path_override: Option<&Path>,
) -> Result<Box<dyn CodeGraphReaderBackend>> {
    super::backend::open_code_graph_reader(path_override)
}

fn open_code_graph_writer(
    path_override: Option<&Path>,
) -> Result<Box<dyn CodeGraphWriterBackend>> {
    super::backend::open_code_graph_writer(path_override)
}

pub fn run_index_code(
    input: &Path,
    db_path: Option<&Path>,
    legacy_kuzu_path_used: bool,
) -> Result<()> {
    if legacy_kuzu_path_used {
        eprintln!(
            "⚠️ Compatibility mode: CLI flag `--kuzu-path` is a legacy compatibility alias; prefer `--db-path`."
        );
    }
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
