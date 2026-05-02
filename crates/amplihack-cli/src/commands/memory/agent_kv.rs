//! Agent-memory key/value commands (`memory get/store/list/delete`).
//!
//! Wires the new MemoryCommands variants to `amplihack_memory::AgentMemory`.

use std::path::PathBuf;

use amplihack_memory::agent_memory::{AgentMemory, MemoryType};
use anyhow::Result;
use serde_json::json;

fn build_agent(
    agent: &str,
    session: Option<String>,
    db_path: Option<PathBuf>,
) -> Result<AgentMemory> {
    let session_id = session.unwrap_or_else(|| {
        chrono::Utc::now()
            .format("session-%Y%m%d-%H%M%S")
            .to_string()
    });
    let mut b = AgentMemory::builder()
        .agent_name(agent.to_string())
        .session_id(session_id)
        .enabled(true);
    if let Some(p) = db_path {
        b = b.db_path(p);
    }
    b.build()
}

fn print_value(value: &serde_json::Value, format: &str) -> Result<()> {
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(value)?),
        _ => println!("{}", serde_json::to_string(value)?),
    }
    Ok(())
}

pub fn run_get(
    agent: &str,
    session: Option<String>,
    key: &str,
    db_path: Option<PathBuf>,
    format: &str,
) -> Result<()> {
    let mem = build_agent(agent, session, db_path)?;
    let value = mem.retrieve(key)?;
    let out = json!({
        "agent": agent,
        "session_id": mem.session_id(),
        "key": key,
        "value": value,
    });
    print_value(&out, format)
}

pub fn run_store(
    agent: &str,
    session: Option<String>,
    key: &str,
    value: &str,
    db_path: Option<PathBuf>,
    memory_type: &str,
    format: &str,
) -> Result<()> {
    let mem = build_agent(agent, session, db_path)?;
    let mt = MemoryType::parse(memory_type);
    let stored = mem.store(key, value, mt)?;
    let out = json!({
        "agent": agent,
        "session_id": mem.session_id(),
        "key": key,
        "type": mt.as_str(),
        "stored": stored,
    });
    print_value(&out, format)
}

pub fn run_list(
    agent: &str,
    session: Option<String>,
    db_path: Option<PathBuf>,
    format: &str,
) -> Result<()> {
    let mem = build_agent(agent, session, db_path)?;
    let records = mem.list()?;
    let out = json!({
        "agent": agent,
        "session_id": mem.session_id(),
        "count": records.len(),
        "records": records,
    });
    print_value(&out, format)
}

pub fn run_delete(
    agent: &str,
    session: Option<String>,
    key: &str,
    db_path: Option<PathBuf>,
    format: &str,
) -> Result<()> {
    let mem = build_agent(agent, session, db_path)?;
    let deleted = mem.delete(key)?;
    let out = json!({
        "agent": agent,
        "session_id": mem.session_id(),
        "key": key,
        "deleted": deleted,
    });
    print_value(&out, format)
}
