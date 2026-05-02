//! `amplihack reflect` dispatch — wires `ReflectCommands` to the
//! `amplihack-reflection` crate.
//!
//! Native Rust replacement for `amplifier-bundle/tools/amplihack/reflection/*.py`.

use std::path::{Path, PathBuf};

use amplihack_reflection::lightweight_analyzer::Message as ReflectMessage;
use amplihack_reflection::reflection::ReflectionOrchestrator;
use amplihack_reflection::state_machine::ReflectionStateMachine;
use anyhow::{Context, Result};
use serde_json::json;

fn default_runtime_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".amplihack")
        .join("runtime")
}

fn resolve_runtime_dir(runtime_dir: Option<PathBuf>) -> PathBuf {
    runtime_dir.unwrap_or_else(default_runtime_dir)
}

fn ensure_runtime_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).with_context(|| format!("create runtime dir {}", p.display()))?;
    Ok(())
}

fn load_messages(path: &Path) -> Result<Vec<ReflectMessage>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read messages file {}", path.display()))?;
    let msgs: Vec<ReflectMessage> = serde_json::from_str(&raw)
        .with_context(|| format!("parse messages JSON in {}", path.display()))?;
    Ok(msgs)
}

fn print_value(value: &serde_json::Value, format: &str) -> Result<()> {
    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(value)?);
        }
        _ => {
            // text mode — render compact human-friendly summary
            println!("{}", serde_json::to_string(value)?);
        }
    }
    Ok(())
}

pub fn run_analyze(
    session: &str,
    messages_path: &Path,
    error: Option<&str>,
    runtime_dir: Option<PathBuf>,
    format: &str,
) -> Result<()> {
    let runtime_dir = resolve_runtime_dir(runtime_dir);
    ensure_runtime_dir(&runtime_dir)?;
    let messages = load_messages(messages_path)?;
    let orch = ReflectionOrchestrator::new(session.to_string(), &runtime_dir);
    let report = orch.run(&messages, &[], error)?;
    let value = serde_json::to_value(&report)?;
    print_value(&value, format)
}

pub fn run_state(session: &str, runtime_dir: Option<PathBuf>, format: &str) -> Result<()> {
    let runtime_dir = resolve_runtime_dir(runtime_dir);
    ensure_runtime_dir(&runtime_dir)?;
    let sm = ReflectionStateMachine::new(session.to_string(), &runtime_dir)?;
    let data = sm.read_state()?;
    let value = json!({
        "session_id": session,
        "state_file": sm.state_file_path().display().to_string(),
        "data": data,
    });
    print_value(&value, format)
}

pub fn run_clear(session: &str, runtime_dir: Option<PathBuf>, format: &str) -> Result<()> {
    let runtime_dir = resolve_runtime_dir(runtime_dir);
    ensure_runtime_dir(&runtime_dir)?;
    let sm = ReflectionStateMachine::new(session.to_string(), &runtime_dir)?;
    sm.reset()?;
    let value = json!({
        "session_id": session,
        "cleared": true,
        "state_file": sm.state_file_path().display().to_string(),
    });
    print_value(&value, format)
}
