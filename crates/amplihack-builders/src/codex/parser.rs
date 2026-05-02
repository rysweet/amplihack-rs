//! Typed parser for Codex session JSON files.

use serde::{Deserialize, Serialize};

use crate::claude::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexSession {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

pub fn parse_session(raw: &str) -> anyhow::Result<CodexSession> {
    serde_json::from_str::<CodexSession>(raw)
        .map_err(|e| anyhow::anyhow!("invalid codex session: {e}"))
}
