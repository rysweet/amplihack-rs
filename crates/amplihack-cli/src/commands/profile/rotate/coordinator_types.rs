//! Non-secret policy inputs, requests and effects for event-driven rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LoginInput {
    pub profile: String,
    pub account_id: String,
    pub generation: String,
    pub eligible: bool,
    pub organisation: Option<String>,
    pub live: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct EvidenceInput {
    pub profile: String,
    pub account_id: String,
    pub generation: String,
    pub observation_id: String,
    pub ticket: u64,
    pub captured_at: i64,
    pub session_window: amplihack_utils::usage_observation::SessionWindow,
    pub session_reset_at: Option<i64>,
    pub week_reset_at: Option<i64>,
    pub session_used: Option<f64>,
    pub week_used: Option<f64>,
    pub limit_reached: bool,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct InputState {
    pub logins: Vec<LoginInput>,
    pub observations: Vec<EvidenceInput>,
    pub enabled: bool,
    pub held: bool,
    pub report_revision: u64,
    /// Validated policy/control inputs; reporting metadata is kept separate.
    pub control: String,
    pub directories: Vec<std::path::PathBuf>,
    pub records: Vec<amplihack_utils::usage_observation::Record>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProbeJob {
    pub namespace: String,
    pub decision_id: u64,
    pub profile: String,
    pub account_id: String,
    pub generation: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ProbeResult {
    Published,
    Failed,
    PublicationFailed,
    Superseded,
    RetiredGeneration,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Effect {
    Decision { id: u64 },
    Probe(ProbeJob),
    Revalidate { id: u64, inputs: InputState },
}
impl ProbeJob {
    pub fn observation_id(&self) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "probe-{:x}",
            Sha256::digest(format!(
                "{}:{}:{}:{}",
                self.namespace, self.decision_id, self.profile, self.generation
            ))
        )
    }
}
