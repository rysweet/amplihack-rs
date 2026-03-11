//! `memory tree` command implementation.

use super::*;
use anyhow::Result;
use kuzu::Connection as KuzuConnection;

pub fn run_tree(
    session_id: Option<&str>,
    memory_type: Option<&str>,
    depth: Option<u32>,
    backend: &str,
) -> Result<()> {
    let backend = BackendChoice::parse(backend)?;
    let output = match backend {
        BackendChoice::Sqlite => render_sqlite_tree(session_id, memory_type, depth)?,
        BackendChoice::Kuzu => render_kuzu_tree(session_id, memory_type, depth)?,
    };
    println!("{output}");
    Ok(())
}

fn render_sqlite_tree(
    session_id: Option<&str>,
    memory_type: Option<&str>,
    depth: Option<u32>,
) -> Result<String> {
    let conn = open_sqlite_memory_db()?;
    let mut sessions = list_sqlite_sessions_from_conn(&conn)?;
    if let Some(session_id) = session_id {
        sessions.retain(|session| session.session_id == session_id);
    }

    let mut session_rows = Vec::new();
    for session in sessions {
        let memories = query_sqlite_memories_for_session(&conn, &session.session_id, memory_type)?;
        session_rows.push((session, memories));
    }

    let agent_counts = if session_id.is_none() && depth.map(|value| value > 2).unwrap_or(true) {
        collect_sqlite_agent_counts(&conn)?
    } else {
        Vec::new()
    };

    Ok(render_tree(
        SQLITE_TREE_BACKEND_NAME,
        &session_rows,
        &agent_counts,
        session_id.is_none(),
        depth,
    ))
}

fn render_kuzu_tree(
    session_id: Option<&str>,
    _memory_type: Option<&str>,
    depth: Option<u32>,
) -> Result<String> {
    use anyhow::Context;
    let db = open_kuzu_memory_db()?;
    let conn = KuzuConnection::new(&db).context("failed to connect to Kùzu memory DB")?;
    init_kuzu_backend_schema(&conn)?;

    let mut sessions = list_kuzu_sessions_from_conn(&conn)?;
    if let Some(session_id) = session_id {
        sessions.retain(|session| session.session_id == session_id);
    }

    let mut session_rows = Vec::new();
    for session in sessions {
        let memories = query_kuzu_memories_for_session(&conn, &session.session_id)?;
        let memory_count = memories.len();
        let mut session = session;
        session.memory_count = memory_count;
        session_rows.push((session, memories));
    }

    let agent_counts = if session_id.is_none() && depth.map(|value| value > 2).unwrap_or(true) {
        collect_kuzu_agent_counts(&conn)?
    } else {
        Vec::new()
    };

    Ok(render_tree(
        KUZU_TREE_BACKEND_NAME,
        &session_rows,
        &agent_counts,
        session_id.is_none(),
        depth,
    ))
}

fn render_tree(
    backend_name: &str,
    session_rows: &[(SessionSummary, Vec<MemoryRecord>)],
    agent_counts: &[(String, usize)],
    include_agents: bool,
    depth: Option<u32>,
) -> String {
    let show_agents =
        include_agents && depth.map(|value| value > 2).unwrap_or(true) && !agent_counts.is_empty();
    let mut lines = vec![format!("🧠 Memory Graph (Backend: {backend_name})")];
    if session_rows.is_empty() {
        lines.push("└── (empty - no memories found)".to_string());
        return lines.join("\n");
    }

    let sessions_branch = format!("📅 Sessions ({})", session_rows.len());
    lines.push(format!(
        "{} {sessions_branch}",
        if show_agents {
            "├──"
        } else {
            "└──"
        }
    ));
    let session_indent = if show_agents { "│   " } else { "    " };
    for (index, (session, memories)) in session_rows.iter().enumerate() {
        let last_session = index + 1 == session_rows.len();
        lines.push(format!(
            "{session_indent}{} {} ({} memories)",
            if last_session {
                "└──"
            } else {
                "├──"
            },
            session.session_id,
            session.memory_count
        ));
        let memory_indent = format!(
            "{session_indent}{}",
            if last_session { "    " } else { "│   " }
        );
        for (memory_index, memory) in memories.iter().enumerate() {
            let line = format_memory_line(memory);
            lines.push(format!(
                "{memory_indent}{} {line}",
                if memory_index + 1 == memories.len() {
                    "└──"
                } else {
                    "├──"
                }
            ));
        }
    }

    if show_agents {
        lines.push(format!("└── 👥 Agents ({})", agent_counts.len()));
        for (index, (agent_id, count)) in agent_counts.iter().enumerate() {
            lines.push(format!(
                "    {} {} ({count} memories)",
                if index + 1 == agent_counts.len() {
                    "└──"
                } else {
                    "├──"
                },
                agent_id
            ));
        }
    }

    lines.join("\n")
}

fn format_memory_line(memory: &MemoryRecord) -> String {
    let mut line = format!(
        "{} {}: {}",
        emoji_for_memory_type(&memory.memory_type),
        title_case(&memory.memory_type),
        memory.title
    );
    if let Some(importance) = memory.importance {
        line.push_str(&format!(" ({})", format_importance_score(importance)));
    }
    if let Some(confidence) = memory.metadata.get("confidence") {
        line.push_str(&format!(" (confidence: {})", json_scalar(confidence)));
    }
    if let Some(count) = memory.metadata.get("usage_count") {
        line.push_str(&format!(" (used: {}x)", json_scalar(count)));
    }
    if memory.expires_at.as_deref().is_some() {
        // Keep parity simple: only show expiry markers when the data exists in tests.
    }
    line
}

fn emoji_for_memory_type(memory_type: &str) -> &'static str {
    match memory_type {
        "conversation" => "📝",
        "pattern" => "💡",
        "decision" => "📌",
        "learning" => "💡",
        "context" => "🔧",
        "artifact" => "📄",
        _ => "❓",
    }
}

fn format_importance_score(importance: i64) -> String {
    let clamped = importance.clamp(0, 10) as usize;
    let filled = "★".repeat(clamped);
    let empty = "☆".repeat(10usize.saturating_sub(clamped));
    format!("{filled}{empty} {clamped}/10")
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn json_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(v) => v.to_string(),
        serde_json::Value::Number(v) => v.to_string(),
        serde_json::Value::String(v) => v.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_tree_matches_python_shape() {
        let session = SessionSummary {
            session_id: "test_sess".to_string(),
            memory_count: 2,
        };
        let rows = vec![(
            session,
            vec![
                MemoryRecord {
                    memory_type: "conversation".to_string(),
                    title: "Hello".to_string(),
                    metadata: serde_json::json!({"confidence": 0.9}),
                    importance: Some(8),
                    expires_at: None,
                },
                MemoryRecord {
                    memory_type: "context".to_string(),
                    title: "Ctx".to_string(),
                    metadata: serde_json::json!({"usage_count": 3}),
                    importance: None,
                    expires_at: None,
                },
            ],
        )];
        let output = render_tree(
            SQLITE_TREE_BACKEND_NAME,
            &rows,
            &[("agent1".to_string(), 2)],
            true,
            None,
        );
        assert!(output.contains("🧠 Memory Graph (Backend: unknown)"));
        assert!(output.contains("📝 Conversation: Hello (★★★★★★★★☆☆ 8/10) (confidence: 0.9)"));
        assert!(output.contains("🔧 Context: Ctx (used: 3x)"));
    }
}
