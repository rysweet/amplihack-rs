//! Durable transaction and preflight-lock state. Older journals remain readable.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Owned locks are journalled; no login movement is authorized yet.
    Checking,
    J0,
    S1,
    S2,
    S3,
    S4,
    Inconsistent,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::J0 => "j0",
            Self::S1 => "s1",
            Self::S2 => "s2",
            Self::S3 => "s3",
            Self::S4 => "s4",
            Self::Inconsistent => "inconsistent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotateJournal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning_transaction: Option<String>,
    pub version: u32,
    pub from: String,
    pub to: String,
    /// `from` was created by adopting the login in `~/.claude`.
    pub adopt: bool,
    pub phase: Phase,
    /// `$HOME` when the swap started. Only ever compared, never a path.
    pub home: String,
    /// Inode of the outgoing login (`~/.claude/.credentials.json`).
    pub live_ino: u64,
    /// Inode of the incoming login (`<root>/<to>/.credentials.json`).
    pub target_ino: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_lock_ino: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_lock_ino: Option<u64>,
    pub started_at: i64,
    pub pid: u32,
    /// Full identities accepted before exchange; absent in legacy journals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<super::super::login_identity::SwapIdentity>,
}
