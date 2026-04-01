use super::backend;
use super::paths::resolve_hierarchical_db_path;
use super::plan::resolve_transfer_backend_choice;
use super::types::{ExportResult, ImportResult};
use crate::command_error::exit_error;
use crate::commands::memory::{
    transfer_format_cli_compatibility_notice, BackendChoice, TransferFormat,
};
use anyhow::Result;
use std::io::{self, Write};

struct ResolvedTransferCliPolicy {
    choice: BackendChoice,
    format: TransferFormat,
    format_notice: Option<String>,
    storage_notice: Option<String>,
}

fn resolve_transfer_cli_policy(
    agent_name: &str,
    format: &str,
    storage_path: Option<&str>,
) -> Result<ResolvedTransferCliPolicy> {
    let choice = resolve_transfer_backend_choice();
    Ok(ResolvedTransferCliPolicy {
        choice,
        format: TransferFormat::parse(format)?,
        format_notice: transfer_format_cli_compatibility_notice(format),
        storage_notice: hierarchical_storage_compatibility_notice(
            agent_name,
            storage_path,
            choice,
        )?,
    })
}

pub fn run_export(
    agent_name: &str,
    output: &str,
    format: &str,
    storage_path: Option<&str>,
) -> Result<()> {
    let resolved = resolve_transfer_cli_policy(agent_name, format, storage_path)?;
    if let Some(notice) = resolved.format_notice.as_deref() {
        eprintln!("⚠️ Compatibility mode: {notice}");
    }
    if let Some(notice) = resolved.storage_notice.as_deref() {
        eprintln!("⚠️ Compatibility mode: {notice}");
    }
    match export_memory(
        agent_name,
        output,
        resolved.format,
        storage_path,
        resolved.choice,
    ) {
        Ok(result) => {
            println!("Exported memory for agent '{}'", result.agent_name);
            println!("  Format: {}", result.format);
            println!("  Output: {}", result.output_path);
            if let Some(size_bytes) = result.file_size_bytes {
                println!("  Size: {:.1} KB", size_bytes as f64 / 1024.0);
            }
            for (key, value) in result.statistics_lines() {
                println!("  {key}: {value}");
            }
            Ok(())
        }
        Err(error) => {
            writeln!(io::stderr(), "Error exporting memory: {error}")?;
            Err(exit_error(1))
        }
    }
}

pub fn run_import(
    agent_name: &str,
    input: &str,
    format: &str,
    merge: bool,
    storage_path: Option<&str>,
) -> Result<()> {
    let resolved = resolve_transfer_cli_policy(agent_name, format, storage_path)?;
    if let Some(notice) = resolved.format_notice.as_deref() {
        eprintln!("⚠️ Compatibility mode: {notice}");
    }
    if let Some(notice) = resolved.storage_notice.as_deref() {
        eprintln!("⚠️ Compatibility mode: {notice}");
    }
    match import_memory(
        agent_name,
        input,
        resolved.format,
        merge,
        storage_path,
        resolved.choice,
    ) {
        Ok(result) => {
            println!("Imported memory into agent '{}'", result.agent_name);
            println!("  Format: {}", result.format);
            println!(
                "  Source agent: {}",
                result
                    .source_agent
                    .clone()
                    .unwrap_or_else(|| "N/A".to_string())
            );
            println!(
                "  Merge mode: {}",
                if result.merge { "True" } else { "False" }
            );
            for (key, value) in result.statistics_lines() {
                println!("  {key}: {value}");
            }
            Ok(())
        }
        Err(error) => {
            writeln!(io::stderr(), "Error importing memory: {error}")?;
            Err(exit_error(1))
        }
    }
}

fn hierarchical_storage_compatibility_notice(
    agent_name: &str,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<Option<String>> {
    if !matches!(choice, BackendChoice::GraphDb) {
        return Ok(None);
    }

    let resolved = resolve_hierarchical_db_path(agent_name, storage_path)?;
    if resolved.file_name().and_then(|name| name.to_str()) != Some("kuzu_db") {
        return Ok(None);
    }

    let neutral = resolved.with_file_name("graph_db");
    Ok(Some(format!(
        "using legacy hierarchical store `{}` because `{}` is not active; migrate to `graph_db`.",
        resolved.display(),
        neutral.display()
    )))
}

pub(crate) fn export_memory(
    agent_name: &str,
    output: &str,
    format: TransferFormat,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ExportResult> {
    match format {
        TransferFormat::Json => {
            backend::export_hierarchical_json(agent_name, output, storage_path, choice)
        }
        TransferFormat::RawDb => {
            backend::export_hierarchical_raw_db(agent_name, output, storage_path, choice)
        }
    }
}

pub(crate) fn import_memory(
    agent_name: &str,
    input: &str,
    format: TransferFormat,
    merge: bool,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ImportResult> {
    match format {
        TransferFormat::Json => {
            backend::import_hierarchical_json(agent_name, input, merge, storage_path, choice)
        }
        TransferFormat::RawDb => {
            backend::import_hierarchical_raw_db(agent_name, input, merge, storage_path, choice)
        }
    }
}
