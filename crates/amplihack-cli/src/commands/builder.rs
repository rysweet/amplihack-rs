//! `amplihack builder` dispatch — wires `BuilderCommands` to the
//! `amplihack-builders` crate.
//!
//! Native Rust replacement for `amplifier-bundle/tools/amplihack/builders/*.py`.

use std::path::{Path, PathBuf};

use amplihack_builders::claude::{ClaudeTranscriptBuilder, TranscriptOptions};
use amplihack_builders::codex::CodexTranscriptsBuilder;
use amplihack_builders::export_on_compact::ExportOnCompactIntegration;
use anyhow::{Context, Result};
use serde_json::json;

fn print_value(value: &serde_json::Value, format: &str) -> Result<()> {
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(value)?),
        _ => println!("{}", serde_json::to_string(value)?),
    }
    Ok(())
}

pub fn run_claude(
    session: &str,
    messages: &Path,
    working_dir: Option<PathBuf>,
    out: Option<PathBuf>,
    format: &str,
) -> Result<()> {
    let working_dir = working_dir.unwrap_or_else(|| PathBuf::from("."));
    let builder = ClaudeTranscriptBuilder::new(session.to_string(), working_dir);
    let opts = TranscriptOptions::default();
    let body = builder.build_session_transcript(messages, &opts)?;
    let summary = builder.build_session_summary(messages)?;
    if let Some(out_path) = out.as_ref() {
        if let Some(parent) = out_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        std::fs::write(out_path, &body)
            .with_context(|| format!("write transcript to {}", out_path.display()))?;
    } else {
        println!("{body}");
    }
    let value = json!({
        "session_id": session,
        "out": out.as_ref().map(|p| p.display().to_string()),
        "summary": summary,
    });
    print_value(&value, format)
}

pub fn run_codex(
    input_dir: &Path,
    focus: Option<&str>,
    out: Option<PathBuf>,
    format: &str,
) -> Result<()> {
    let builder = CodexTranscriptsBuilder::new(Some(input_dir.to_path_buf()));
    let body = match focus {
        Some(f) => builder.build_focused_codex(f, None)?,
        None => builder.build_comprehensive_codex(None)?,
    };
    if let Some(out_path) = out.as_ref() {
        if let Some(parent) = out_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        std::fs::write(out_path, &body)
            .with_context(|| format!("write codex to {}", out_path.display()))?;
    } else {
        println!("{body}");
    }
    let value = json!({
        "input_dir": input_dir.display().to_string(),
        "focus": focus,
        "out": out.as_ref().map(|p| p.display().to_string()),
        "bytes": body.len(),
    });
    print_value(&value, format)
}

pub fn run_export_on_compact(input: &Path, root_dir: &Path, format: &str) -> Result<()> {
    let raw = std::fs::read_to_string(input)
        .with_context(|| format!("read input {}", input.display()))?;
    let payload: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parse JSON in {}", input.display()))?;
    let integration = ExportOnCompactIntegration::with_root(root_dir.to_path_buf());
    let result = integration.process(&payload)?;
    print_value(&result, format)
}
