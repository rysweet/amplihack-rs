//! Individual query runners and output formatting helpers.

use crate::commands::memory::code_graph::{
    CodeGraphEdgeEntry, CodeGraphNamedEntry, CodeGraphReaderBackend,
};
use anyhow::Result;

pub(super) fn run_stats(backend: &dyn CodeGraphReaderBackend, json_output: bool) -> Result<()> {
    let stats = backend.stats()?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        println!("Code Graph Statistics:");
        println!("  Files:     {}", stats.files);
        println!("  Classes:   {}", stats.classes);
        println!("  Functions: {}", stats.functions);
        println!("  Memory→File links:     {}", stats.memory_file_links);
        println!("  Memory→Function links: {}", stats.memory_function_links);
    }

    Ok(())
}

pub(super) fn run_context(
    backend: &dyn CodeGraphReaderBackend,
    memory_id: &str,
    json_output: bool,
) -> Result<()> {
    let payload = backend.context_payload(memory_id)?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("Code context for memory '{memory_id}':");

    if payload.files.is_empty() {
        println!("  Files: none");
    } else {
        println!("  Files:");
        for file in payload.files {
            println!(
                "    - {} [{}] ({} bytes)",
                file.path, file.language, file.size_bytes
            );
        }
    }

    if payload.functions.is_empty() {
        println!("  Functions: none");
    } else {
        println!("  Functions:");
        for function in payload.functions {
            println!("    - {} :: {}", function.name, function.signature);
        }
    }

    if payload.classes.is_empty() {
        println!("  Classes: none");
    } else {
        println!("  Classes:");
        for class in payload.classes {
            println!("    - {} ({})", class.name, class.fully_qualified_name);
        }
    }

    Ok(())
}

pub(super) fn run_files(
    backend: &dyn CodeGraphReaderBackend,
    pattern: Option<&str>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    print_string_list(backend.files(pattern, limit)?, json_output, limit)
}

pub(super) fn run_functions(
    backend: &dyn CodeGraphReaderBackend,
    file: Option<&str>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    print_named_entries(backend.functions(file, limit)?, json_output, limit)
}

pub(super) fn run_classes(
    backend: &dyn CodeGraphReaderBackend,
    file: Option<&str>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    print_named_entries(backend.classes(file, limit)?, json_output, limit)
}

pub(super) fn run_search(
    backend: &dyn CodeGraphReaderBackend,
    name: &str,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    let payload = backend.search(name, limit)?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if payload.is_empty() {
        println!("No results for '{name}'");
    } else {
        for item in payload {
            println!("  [{}] {}", item.kind, item.name);
        }
    }

    Ok(())
}

pub(super) fn run_callers(
    backend: &dyn CodeGraphReaderBackend,
    name: &str,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    run_edge_entries(
        backend.callers(name, limit)?,
        json_output,
        &format!("Functions calling '{name}':"),
        &format!("No callers found for '{name}'"),
    )
}

pub(super) fn run_callees(
    backend: &dyn CodeGraphReaderBackend,
    name: &str,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    run_edge_entries(
        backend.callees(name, limit)?,
        json_output,
        &format!("Functions called by '{name}':"),
        &format!("No callees found for '{name}'"),
    )
}

fn print_named_entries(
    entries: Vec<CodeGraphNamedEntry>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for entry in &entries {
            if let Some(file) = &entry.file {
                println!("  {file}::{}", entry.name);
            } else {
                println!("  {}", entry.name);
            }
        }
        print_limit_hint(entries.len(), limit);
    }
    Ok(())
}

fn run_edge_entries(
    entries: Vec<CodeGraphEdgeEntry>,
    json_output: bool,
    heading: &str,
    empty_message: &str,
) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        println!("{empty_message}");
    } else {
        println!("{heading}");
        for entry in entries {
            println!("  {} -> {}", entry.caller, entry.callee);
        }
    }

    Ok(())
}

fn print_string_list(values: Vec<String>, json_output: bool, limit: u32) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        for value in &values {
            println!("{value}");
        }
        print_limit_hint(values.len(), limit);
    }
    Ok(())
}

fn print_limit_hint(actual_len: usize, limit: u32) {
    if actual_len == limit as usize {
        println!("... (showing first {limit}, use --limit to see more)");
    }
}
