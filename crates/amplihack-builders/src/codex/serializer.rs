//! Markdown serialization helpers for the codex builder.

use crate::claude::Message;

use super::parser::CodexSession;

pub fn render_session_block(session: &CodexSession) -> String {
    let mut out = String::new();
    out.push_str(&format!("## Session: {}\n\n", session.session_id));
    for m in &session.messages {
        let role = if m.role.is_empty() {
            "unknown"
        } else {
            &m.role
        };
        out.push_str(&format!("**{role}**: {}\n\n", m.content.as_plain_text()));
    }
    out
}

pub fn render_assistant_corpus(sessions: &[CodexSession]) -> String {
    let mut out = String::new();
    for s in sessions {
        for m in s.messages.iter().filter(|m| m.role == "assistant") {
            out.push_str(&m.content.as_plain_text());
            out.push_str("\n\n---\n\n");
        }
    }
    out
}

pub fn render_insights_markdown(sessions: &[CodexSession]) -> String {
    let mut out = String::new();
    out.push_str("# Codex Insights\n\n");
    out.push_str(&format!("Sessions analyzed: {}\n\n", sessions.len()));
    out.push_str("## Per-session Highlights\n\n");
    for s in sessions {
        out.push_str(&format!(
            "- **{}** — {} messages\n",
            s.session_id,
            s.messages.len()
        ));
    }
    out.push_str("\n## Tool Usage\n\n");
    let tools = collect_tools(sessions);
    if tools.is_empty() {
        out.push_str("_No explicit tool annotations found._\n");
    } else {
        for t in tools {
            out.push_str(&format!("- {t}\n"));
        }
    }
    out
}

fn collect_tools(sessions: &[CodexSession]) -> Vec<String> {
    let mut acc = Vec::<String>::new();
    for s in sessions {
        for m in &s.messages {
            extract_tools(&m.content.as_plain_text(), &mut acc);
        }
    }
    acc
}

fn extract_tools(text: &str, acc: &mut Vec<String>) {
    for token in text.split_whitespace() {
        if let Some(rest) = token.strip_prefix("tool:") {
            let t = rest.trim_end_matches(|c: char| !c.is_alphanumeric());
            if !t.is_empty() && !acc.iter().any(|x| x == t) {
                acc.push(t.to_string());
            }
        }
    }
}

pub fn message_role(m: &Message) -> &str {
    &m.role
}
