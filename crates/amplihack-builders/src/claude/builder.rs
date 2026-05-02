//! Claude transcript builder.
//!
//! Native Rust port of `claude_transcript_builder.py`. Produces markdown
//! transcripts and JSON summary structures from a Claude messages.json file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::parser::{Message, parse_messages};

#[derive(Debug, Clone, Default)]
pub struct TranscriptOptions {
    pub include_timestamps: bool,
    pub max_messages: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub message_count: usize,
    pub user_turns: usize,
    pub assistant_turns: usize,
    pub system_turns: usize,
    pub total_characters: usize,
    pub total_words: usize,
    pub tools_used: Vec<String>,
}

pub struct ClaudeTranscriptBuilder {
    session_id: String,
    #[allow(dead_code)]
    working_dir: PathBuf,
}

impl ClaudeTranscriptBuilder {
    pub fn new(session_id: impl Into<String>, working_dir: PathBuf) -> Self {
        Self {
            session_id: session_id.into(),
            working_dir,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Read `messages_path` and produce a markdown transcript string.
    pub fn build_session_transcript(
        &self,
        messages_path: &Path,
        opts: &TranscriptOptions,
    ) -> anyhow::Result<String> {
        let raw = std::fs::read_to_string(messages_path)?;
        let mut messages = parse_messages(&raw)?;
        if let Some(limit) = opts.max_messages
            && messages.len() > limit
        {
            messages.truncate(limit);
        }
        Ok(self.render_transcript(&messages, opts.include_timestamps))
    }

    /// Compute a JSON-serializable summary from the messages file.
    pub fn build_session_summary(&self, messages_path: &Path) -> anyhow::Result<SessionSummary> {
        let raw = std::fs::read_to_string(messages_path)?;
        let messages = parse_messages(&raw)?;
        Ok(self.summarize(&messages))
    }

    /// Export the transcript to disk in markdown form (Codex-friendly).
    pub fn export_for_codex(&self, messages_path: &Path, out_path: &Path) -> anyhow::Result<()> {
        let raw = std::fs::read_to_string(messages_path)?;
        let messages = parse_messages(&raw)?;
        let body = self.render_transcript(&messages, false);
        if let Some(parent) = out_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(out_path, body)?;
        Ok(())
    }

    fn render_transcript(&self, messages: &[Message], include_timestamps: bool) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "# Conversation Transcript - {}\n\n",
            self.session_id
        ));
        out.push_str(&format!("**Session ID**: {}\n", self.session_id));
        out.push_str(&format!("**Messages**: {}\n\n", messages.len()));
        out.push_str("## Conversation Flow\n\n");
        for (i, msg) in messages.iter().enumerate() {
            let role = if msg.role.is_empty() {
                "unknown"
            } else {
                &msg.role
            };
            out.push_str(&format!("### {} - {}\n", i + 1, role));
            if include_timestamps && let Some(ts) = &msg.timestamp {
                out.push_str(&format!("_{ts}_\n"));
            }
            out.push_str(&msg.content.as_plain_text());
            out.push_str("\n\n");
        }
        out
    }

    fn summarize(&self, messages: &[Message]) -> SessionSummary {
        let mut user = 0;
        let mut assistant = 0;
        let mut system = 0;
        let mut chars = 0usize;
        let mut words = 0usize;
        let mut tools = Vec::<String>::new();
        for m in messages {
            let body = m.content.as_plain_text();
            chars += body.len();
            words += body.split_whitespace().count();
            match m.role.as_str() {
                "user" => user += 1,
                "assistant" => assistant += 1,
                "system" => system += 1,
                _ => {}
            }
            // Heuristic: parts may include tool calls referenced by name.
            for token in body.split_whitespace() {
                if let Some(rest) = token.strip_prefix("tool:") {
                    let t = rest.trim_end_matches(',');
                    if !t.is_empty() && !tools.iter().any(|x| x == t) {
                        tools.push(t.to_string());
                    }
                }
            }
        }
        SessionSummary {
            session_id: self.session_id.clone(),
            message_count: messages.len(),
            user_turns: user,
            assistant_turns: assistant,
            system_turns: system,
            total_characters: chars,
            total_words: words,
            tools_used: tools,
        }
    }
}
