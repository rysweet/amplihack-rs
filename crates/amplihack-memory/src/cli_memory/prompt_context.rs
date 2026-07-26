use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value as JsonValue;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::Path;

use super::backend;
use super::backend::graph_db::resolve_memory_graph_db_path;
use super::code_graph;
use super::resolve::resolve_memory_backend_preference;
use super::types::{BackendChoice, MemoryRecord, PromptContextMemory, SelectedPromptContextMemory};

fn load_runtime_memories_from_backend(
    choice: BackendChoice,
    session_id: &str,
) -> Result<Vec<MemoryRecord>> {
    backend::open_runtime_backend(choice)?.load_prompt_context_memories(session_id)
}

fn is_prompt_context_memory(memory: &MemoryRecord) -> bool {
    matches!(
        memory
            .metadata
            .get("new_memory_type")
            .and_then(JsonValue::as_str),
        Some("episodic" | "semantic" | "procedural")
    )
}

pub(crate) fn parse_memory_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc())
        })
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc())
        })
        .or_else(|| {
            // Handle time::OffsetDateTime Display format: "{date} {time} {offset}"
            // e.g. "1971-01-01 0:00:00.0 +00:00:00"
            // Strip the timezone offset token (third space-delimited part) and
            // parse the date+time as UTC.
            let mut parts = value.splitn(3, ' ');
            if let (Some(date), Some(time_str), Some(_offset)) =
                (parts.next(), parts.next(), parts.next())
            {
                let candidate = format!("{date} {time_str}");
                NaiveDateTime::parse_from_str(&candidate, "%Y-%m-%d %H:%M:%S%.f")
                    .ok()
                    .map(|dt| dt.and_utc())
            } else {
                None
            }
        })
}

/// Score a single memory record against a pre-lowercased query string.
///
/// `query_lower` **must** already be lowercase.  `query_words` must be the
/// set of whitespace-split tokens from `query_lower`, also pre-computed by
/// the caller so it is not re-allocated once per memory record.
fn memory_relevance_score(
    memory: &MemoryRecord,
    query_lower: &str,
    query_words: &HashSet<&str>,
) -> f64 {
    let content_lower = memory.content.to_lowercase();
    let mut score = 0.0;

    if !query_lower.is_empty() && content_lower.contains(query_lower) {
        score += 10.0;
    }

    let content_words: HashSet<&str> = content_lower.split_whitespace().collect();
    score += query_words.intersection(&content_words).count() as f64 * 2.0;

    if let Some(accessed_at) = memory.accessed_at.as_deref()
        && let Some(timestamp) = parse_memory_timestamp(accessed_at)
    {
        let age_days = (Utc::now() - timestamp).num_days().max(0) as f64;
        score += (5.0 - (age_days * 0.1)).max(0.0);
    }

    if let Some(importance) = memory.importance {
        score += importance as f64;
    }

    score
}

pub(super) fn select_prompt_context_memories(
    memories: Vec<MemoryRecord>,
    query_text: &str,
    token_budget: usize,
) -> Vec<SelectedPromptContextMemory> {
    if token_budget == 0 {
        return Vec::new();
    }

    // Pre-compute once: lower-cased query string and its word set.
    // `memory_relevance_score` accepts both so neither is rebuilt per record.
    let query_lower = query_text.to_lowercase();
    let query_words: HashSet<&str> = query_lower.split_whitespace().collect();

    let mut ranked = memories
        .into_iter()
        .filter(is_prompt_context_memory)
        .map(|memory| {
            let score = memory_relevance_score(&memory, &query_lower, &query_words);
            (memory, score)
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));

    let mut total_tokens = 0usize;
    let mut selected = Vec::new();
    for (memory, _) in ranked {
        // Use byte length / 4 as a token budget approximation — identical to
        // the previous chars().count() / 4 for ASCII, a slight overestimate
        // for multibyte UTF-8, and O(1) instead of O(n).
        let memory_tokens = memory.content.len() / 4;
        if total_tokens + memory_tokens > token_budget {
            break;
        }
        selected.push(SelectedPromptContextMemory {
            memory_id: memory.memory_id,
            content: memory.content,
            code_context: None,
        });
        total_tokens += memory_tokens;
    }

    selected
}

fn format_code_context(payload: &code_graph::CodeGraphContextPayload) -> Option<String> {
    if payload.files.is_empty() && payload.functions.is_empty() && payload.classes.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    if !payload.files.is_empty() {
        lines.push("**Related Files:**".to_string());
        for file in payload.files.iter().take(5) {
            lines.push(format!("- {} ({})", file.path, file.language));
        }
    }

    if !payload.functions.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("**Related Functions:**".to_string());
        for function in payload.functions.iter().take(5) {
            let signature = if function.signature.trim().is_empty() {
                function.name.as_str()
            } else {
                function.signature.as_str()
            };
            lines.push(format!("- `{signature}`"));
            if !function.docstring.trim().is_empty() {
                let doc_preview = if function.docstring.len() > 100 {
                    let truncated = function.docstring.chars().take(100).collect::<String>();
                    format!("{truncated}...")
                } else {
                    function.docstring.clone()
                };
                lines.push(format!("  {doc_preview}"));
            }
            if function.complexity > 0 {
                lines.push(format!("  (complexity: {})", function.complexity));
            }
        }
    }

    if !payload.classes.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("**Related Classes:**".to_string());
        for class in payload.classes.iter().take(3) {
            let name = if class.fully_qualified_name.trim().is_empty() {
                class.name.as_str()
            } else {
                class.fully_qualified_name.as_str()
            };
            lines.push(format!("- {name}"));
            if !class.docstring.trim().is_empty() {
                let doc_preview = if class.docstring.len() > 100 {
                    let truncated = class.docstring.chars().take(100).collect::<String>();
                    format!("{truncated}...")
                } else {
                    class.docstring.clone()
                };
                lines.push(format!("  {doc_preview}"));
            }
        }
    }

    Some(lines.join("\n"))
}

pub(super) fn enrich_prompt_context_memories_with_reader(
    selected: Vec<SelectedPromptContextMemory>,
    reader: &dyn code_graph::CodeGraphReaderBackend,
) -> Result<Vec<SelectedPromptContextMemory>> {
    if selected.is_empty() {
        return Ok(selected);
    }

    let mut enriched = Vec::with_capacity(selected.len());
    for mut memory in selected {
        if memory.memory_id.trim().is_empty() {
            enriched.push(memory);
            continue;
        }

        let payload = reader.context_payload(&memory.memory_id).with_context(|| {
            format!(
                "failed to load prompt memory code context for {}",
                memory.memory_id
            )
        })?;
        memory.code_context = format_code_context(&payload);
        enriched.push(memory);
    }
    Ok(enriched)
}

pub(super) fn enrich_prompt_context_memories_with_code_context_at_path(
    selected: Vec<SelectedPromptContextMemory>,
    db_path: &Path,
) -> Result<Vec<SelectedPromptContextMemory>> {
    let reader = code_graph::open_code_graph_reader(Some(db_path)).with_context(|| {
        format!(
            "prompt memory code-context enrichment unavailable for {}",
            db_path.display()
        )
    })?;
    enrich_prompt_context_memories_with_reader(selected, reader.as_ref())
}

fn enrich_prompt_context_memories_with_code_context(
    selected: Vec<SelectedPromptContextMemory>,
) -> Result<Vec<SelectedPromptContextMemory>> {
    let db_path = resolve_memory_graph_db_path()?;
    enrich_prompt_context_memories_with_code_context_at_path(selected, &db_path)
}

pub(super) fn retrieve_prompt_context_memories_from_backend(
    choice: BackendChoice,
    session_id: &str,
    query_text: &str,
    token_budget: usize,
) -> Result<Vec<PromptContextMemory>> {
    let memories = load_runtime_memories_from_backend(choice, session_id)?;
    let selected = select_prompt_context_memories(memories, query_text, token_budget);
    let selected = match choice {
        BackendChoice::GraphDb => enrich_prompt_context_memories_with_code_context(selected)?,
        BackendChoice::Sqlite => selected,
    };
    Ok(selected
        .into_iter()
        .map(|memory| PromptContextMemory {
            content: memory.content,
            code_context: memory.code_context,
        })
        .collect())
}

pub(super) fn resolve_runtime_memory_backend_choice() -> Result<BackendChoice> {
    Ok(resolve_memory_backend_preference()?.unwrap_or(BackendChoice::GraphDb))
}
