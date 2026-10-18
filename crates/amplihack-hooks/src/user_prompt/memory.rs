//! Agent memory injection and framework file detection.

mod language;
mod scoring;
mod text;

use crate::agent_memory::{
    detect_agent_references, detect_slash_command_agent, without_definition_references,
};
use amplihack_memory::cli_memory::{PromptContextMemory, retrieve_prompt_context_memories};
use amplihack_types::ProjectDirs;
use language::reads_as_english;
use scoring::{
    MIN_SHARED_TERMS, RELEVANCE_THRESHOLD, ignored_terms, memory_relevance, scored_terms,
    topic_terms,
};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use text::turns;

/// At most this many memories are injected for one prompt.
const MAX_INJECTED_MEMORIES: usize = 5;
/// Each injected memory is cut to this many characters, so a stored
/// transcript never lands in the prompt whole.
const MAX_MEMORY_CHARS: usize = 400;

/// The agents `prompt` invokes, by name or by slash command.
fn prompt_agents(prompt: &str) -> Vec<String> {
    let mut agent_types = detect_agent_references(prompt);
    if let Some(agent) = detect_slash_command_agent(prompt)
        && !agent_types.iter().any(|existing| existing == agent)
    {
        agent_types.push(agent.to_string());
    }
    agent_types
}

pub(crate) fn inject_memory(prompt: &str, session_id: Option<&str>) -> Option<String> {
    let agent_types = prompt_agents(prompt);
    if agent_types.is_empty() {
        return None;
    }

    let session_id_owned: String;
    let session_id = match session_id.filter(|value| !value.trim().is_empty()) {
        Some(id) => id,
        None => {
            session_id_owned = generate_unique_session_id();
            tracing::warn!(
                "No session_id provided to user_prompt memory hook; using generated fallback: {}",
                session_id_owned
            );
            &session_id_owned
        }
    };
    let query_text = prompt.chars().take(500).collect::<String>();
    match retrieve_prompt_context_memories(session_id, &query_text, 2000) {
        Ok(memories) => format_agent_memory_context(&query_text, &agent_types, &memories),
        Err(error) => {
            tracing::warn!("Memory injection failed: {}", error);
            None
        }
    }
}

/// Format the memories relevant to `prompt` as one context section.
///
/// Each memory is scored against the prompt with [`memory_relevance`];
/// memories below [`RELEVANCE_THRESHOLD`] or sharing fewer than
/// [`MIN_SHARED_TERMS`] topic words are dropped. Memories with the same
/// text, ignoring the `Agent <name>:` prefix and whitespace, are printed
/// once, and each entry is bounded to [`MAX_MEMORY_CHARS`].
///
/// Returns `None`, so nothing is injected, when no memory is relevant.
pub fn format_agent_memory_context(
    prompt: &str,
    agent_types: &[String],
    memories: &[PromptContextMemory],
) -> Option<String> {
    let ignored = ignored_terms(agent_types);
    // Agent definition references are left out of the comparison, on both
    // sides: a memory stored from a prompt that used the same reference
    // would otherwise share its directory names (`claude`, `amplihack`).
    let prompt = without_definition_references(prompt);
    // The prompt is scored as written: a quoted command (`cargo test
    // sqlite timeout`) is what it is about, and its own foreign text is
    // kept from matching by the stop words.
    let prompt_terms = topic_terms(&prompt, &ignored);

    let mut scored: Vec<(f64, String, &PromptContextMemory)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for memory in memories {
        // Deduplicate on the whole text with whitespace collapsed, so copies
        // that differ only in layout are one memory, while memories that
        // differ anywhere (even past the printed cut) stay distinct.
        let text = single_line(strip_agent_prefix(&memory.content));
        if seen.contains(&text) {
            continue;
        }
        // Only the turns that read as English are understood, so only they
        // contribute topic words: in a transcript where the user wrote
        // another language and the assistant answered in English, the
        // user's function words must not count.
        let body = without_definition_references(strip_agent_prefix(&memory.content));
        let memory_terms: HashSet<String> = turns(&body)
            .iter()
            .filter(|turn| reads_as_english(turn))
            .flat_map(|turn| scored_terms(turn, &ignored))
            .collect();
        let shared = prompt_terms.intersection(&memory_terms).count();
        let relevance = memory_relevance(&prompt_terms, &memory_terms);
        if shared >= MIN_SHARED_TERMS && relevance >= RELEVANCE_THRESHOLD {
            seen.insert(text.clone());
            scored.push((relevance, text, memory));
        }
    }
    if scored.is_empty() {
        return None;
    }
    scored.sort_by(|left, right| right.0.total_cmp(&left.0));
    scored.truncate(MAX_INJECTED_MEMORIES);

    let heading = if agent_types.is_empty() {
        "\n## Relevant Memory\n".to_string()
    } else {
        format!(
            "\n## Relevant Memory (agents: {})\n",
            agent_types.join(", ")
        )
    };
    let mut lines = vec![heading];
    for (relevance, text, memory) in scored {
        lines.push(format!(
            "- {} (relevance: {relevance:.2})",
            bounded_memory_text(&text)
        ));
        if let Some(code_context) = memory.code_context.as_deref()
            && !code_context.trim().is_empty()
        {
            lines.push(code_context.to_string());
        }
    }
    Some(lines.join("\n"))
}

/// The memory without the `Agent <name>: ` prefix session-stop stores it
/// with. The same summary is stored once per agent, so the prefix must not
/// count toward relevance or make copies look distinct.
fn strip_agent_prefix(content: &str) -> &str {
    let content = content.trim();
    content
        .strip_prefix("Agent ")
        .and_then(|rest| rest.split_once(": "))
        .filter(|(name, _)| !name.is_empty() && !name.contains(char::is_whitespace))
        .map_or(content, |(_, body)| body.trim())
}

/// `content` with every run of whitespace collapsed to one space.
fn single_line(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A single-line memory cut to [`MAX_MEMORY_CHARS`] characters.
fn bounded_memory_text(line: &str) -> String {
    if line.chars().count() <= MAX_MEMORY_CHARS {
        return line.to_string();
    }
    let mut cut = line.chars().take(MAX_MEMORY_CHARS).collect::<String>();
    cut.push('…');
    cut
}

/// Generate a unique session ID from PID and current timestamp.
fn generate_unique_session_id() -> String {
    format!(
        "hook-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    )
}

/// Check if AMPLIHACK.md should be injected (differs from CLAUDE.md).
pub(crate) fn check_framework_injection(dirs: &ProjectDirs) -> Option<String> {
    let amplihack_path = find_amplihack_md(dirs)?;
    let claude_path = dirs.claude_md();

    let amplihack_content = fs::read_to_string(&amplihack_path).ok()?;
    let claude_content = fs::read_to_string(&claude_path).ok().unwrap_or_default();

    // Normalize whitespace for comparison.
    let norm_amplihack: String = amplihack_content.split_whitespace().collect();
    let norm_claude: String = claude_content.split_whitespace().collect();

    if norm_amplihack == norm_claude {
        return None; // Already identical.
    }

    Some(amplihack_content)
}

fn find_amplihack_md(dirs: &ProjectDirs) -> Option<PathBuf> {
    // Check CLAUDE_PLUGIN_ROOT env var first.
    if let Ok(root) = std::env::var("CLAUDE_PLUGIN_ROOT") {
        let path = PathBuf::from(root).join("AMPLIHACK.md");
        if path.exists() {
            return Some(path);
        }
    }

    // Check .claude/AMPLIHACK.md.
    let path = dirs.amplihack_md();
    if path.exists() {
        return Some(path);
    }

    None
}

#[cfg(test)]
mod tests;
