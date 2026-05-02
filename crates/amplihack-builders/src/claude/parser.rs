//! Typed parser for Claude messages.json files.
//!
//! Fail-closed: malformed input returns an error rather than silently
//! producing an empty transcript.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: MessageContent,
    #[serde(default)]
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<MessagePart>),
    Other(serde_json::Value),
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Text(String::new())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessagePart {
    Text { text: String },
    Other(serde_json::Value),
}

impl MessageContent {
    pub fn as_plain_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Parts(parts) => parts
                .iter()
                .map(|p| match p {
                    MessagePart::Text { text } => text.clone(),
                    MessagePart::Other(v) => v.to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            MessageContent::Other(v) => v.to_string(),
        }
    }
}

pub fn parse_messages(raw: &str) -> anyhow::Result<Vec<Message>> {
    serde_json::from_str::<Vec<Message>>(raw)
        .map_err(|e| anyhow::anyhow!("invalid messages.json: {e}"))
}
