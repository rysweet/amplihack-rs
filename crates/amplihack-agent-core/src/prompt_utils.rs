//! Prompt construction and parsing utilities.
//!
//! Provides helpers for building system prompts, truncating conversation
//! context, and extracting fenced code blocks from LLM responses.

use serde::{Deserialize, Serialize};

/// A fenced code block extracted from text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeBlock {
    /// Language tag from the opening fence (e.g. `"rust"`, `"python"`).
    /// Empty string if no language was specified.
    pub language: String,
    /// The content between the fences.
    pub content: String,
}

/// Build a system prompt from structured components.
///
/// Concatenates role description, context, and constraints into a
/// well-formatted system prompt string.
pub fn build_system_prompt(role: &str, context: &str, constraints: &[String]) -> String {
    let mut parts = Vec::with_capacity(3);

    if !role.is_empty() {
        parts.push(format!("You are {role}."));
    }

    if !context.is_empty() {
        parts.push(format!("\n## Context\n{context}"));
    }

    if !constraints.is_empty() {
        let constraint_list: String = constraints
            .iter()
            .map(|c| format!("- {c}"))
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("\n## Constraints\n{constraint_list}"));
    }

    parts.join("\n")
}

/// A message in a conversation (role + content).
#[derive(Debug, Clone)]
pub struct Message {
    /// Role identifier (e.g. `"user"`, `"assistant"`, `"system"`).
    pub role: String,
    /// Message content.
    pub content: String,
}

/// Format a conversation into a single string, truncating to fit within
/// an approximate token budget.
///
/// Uses the heuristic of ~4 characters per token. Keeps the most recent
/// messages first, trimming from the oldest.
pub fn format_conversation_context(messages: &[Message], max_tokens: usize) -> String {
    let max_chars = max_tokens * 4;
    let mut parts = Vec::new();
    let mut total_chars = 0;

    // Walk messages from newest to oldest, collecting until budget is hit
    for msg in messages.iter().rev() {
        let formatted = format!("[{}]: {}", msg.role, msg.content);
        let len = formatted.len();
        if total_chars + len > max_chars && !parts.is_empty() {
            break;
        }
        total_chars += len;
        parts.push(formatted);
    }

    // Reverse so oldest-first ordering is preserved
    parts.reverse();

    if parts.len() < messages.len() {
        let mut result = format!(
            "... ({} earlier messages truncated)\n",
            messages.len() - parts.len()
        );
        result.push_str(&parts.join("\n"));
        result
    } else {
        parts.join("\n")
    }
}

/// Extract all fenced code blocks from text.
///
/// Recognises triple-backtick fences with optional language tags.
pub fn extract_code_blocks(text: &str) -> Vec<CodeBlock> {
    let mut blocks = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if let Some(after_fence) = trimmed.strip_prefix("```") {
            let language = after_fence.trim().to_string();
            let mut content_lines = Vec::new();

            for inner in lines.by_ref() {
                let inner_trimmed = inner.trim();
                if inner_trimmed == "```" {
                    break;
                }
                content_lines.push(inner);
            }

            blocks.push(CodeBlock {
                language,
                content: content_lines.join("\n"),
            });
        }
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_all_parts() {
        let prompt = build_system_prompt(
            "a helpful assistant",
            "The user is working on a Rust project.",
            &["Be concise".to_string(), "Use idiomatic Rust".to_string()],
        );
        assert!(prompt.contains("You are a helpful assistant."));
        assert!(prompt.contains("## Context"));
        assert!(prompt.contains("Rust project"));
        assert!(prompt.contains("## Constraints"));
        assert!(prompt.contains("- Be concise"));
    }

    #[test]
    fn build_prompt_empty_parts() {
        let prompt = build_system_prompt("", "", &[]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn build_prompt_role_only() {
        let prompt = build_system_prompt("a code reviewer", "", &[]);
        assert_eq!(prompt, "You are a code reviewer.");
    }

    #[test]
    fn format_conversation_fits() {
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "Hello".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "Hi there".to_string(),
            },
        ];
        let formatted = format_conversation_context(&messages, 1000);
        assert!(formatted.contains("[user]: Hello"));
        assert!(formatted.contains("[assistant]: Hi there"));
        assert!(!formatted.contains("truncated"));
    }

    #[test]
    fn format_conversation_truncates() {
        let messages: Vec<Message> = (0..100)
            .map(|i| Message {
                role: "user".to_string(),
                content: format!("Message number {i} with some padding text here"),
            })
            .collect();
        let formatted = format_conversation_context(&messages, 50);
        assert!(formatted.contains("truncated"));
        // Should keep some recent messages
        assert!(formatted.contains("Message number 99"));
    }

    #[test]
    fn extract_single_code_block() {
        let text = "Some text\n```rust\nfn main() {}\n```\nMore text";
        let blocks = extract_code_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].language, "rust");
        assert_eq!(blocks[0].content, "fn main() {}");
    }

    #[test]
    fn extract_multiple_code_blocks() {
        let text = "```python\nprint('hello')\n```\n\n```javascript\nconsole.log('hi')\n```";
        let blocks = extract_code_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].language, "python");
        assert_eq!(blocks[1].language, "javascript");
    }

    #[test]
    fn extract_code_block_no_language() {
        let text = "```\nplain text\n```";
        let blocks = extract_code_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].language, "");
        assert_eq!(blocks[0].content, "plain text");
    }

    #[test]
    fn extract_no_code_blocks() {
        let text = "Just plain text\nwith no code blocks";
        let blocks = extract_code_blocks(text);
        assert!(blocks.is_empty());
    }

    #[test]
    fn code_block_serde_roundtrip() {
        let block = CodeBlock {
            language: "rust".to_string(),
            content: "fn main() {}".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let parsed: CodeBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, block);
    }
}
