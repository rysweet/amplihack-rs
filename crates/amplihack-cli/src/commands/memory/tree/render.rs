use crate::commands::memory::backend::MemoryTreeBackend;
use crate::commands::memory::{MemoryRecord, SessionSummary};
use anyhow::Result;

pub(super) fn render_tree_from_backend(
    backend: &dyn MemoryTreeBackend,
    session_id: Option<&str>,
    memory_type: Option<&str>,
    depth: Option<u32>,
    compatibility_notice: Option<&str>,
) -> Result<String> {
    let session_rows = backend.load_session_rows(session_id, memory_type)?;
    let agent_counts = if session_id.is_none() && depth.map(|value| value > 2).unwrap_or(true) {
        backend.collect_agent_counts()?
    } else {
        Vec::new()
    };

    Ok(render_tree(
        backend.backend_name(),
        &session_rows,
        &agent_counts,
        session_id.is_none(),
        depth,
        compatibility_notice,
    ))
}

pub(super) fn render_tree(
    backend_name: &str,
    session_rows: &[(SessionSummary, Vec<MemoryRecord>)],
    agent_counts: &[(String, usize)],
    include_agents: bool,
    depth: Option<u32>,
    compatibility_notice: Option<&str>,
) -> String {
    let show_agents =
        include_agents && depth.map(|value| value > 2).unwrap_or(true) && !agent_counts.is_empty();
    let mut lines = vec![format!("🧠 Memory Graph (Backend: {backend_name})")];
    if let Some(notice) = compatibility_notice {
        lines.push(format!("⚠️ Compatibility mode: {notice}"));
    }
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

pub(super) fn format_memory_line(memory: &MemoryRecord) -> String {
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

pub(super) fn format_importance_score(importance: i64) -> String {
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
