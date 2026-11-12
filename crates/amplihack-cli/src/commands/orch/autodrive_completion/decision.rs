use super::{Write, evidence, private_state, secure_fs};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Complete,
    Evaluate,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub schema_version: u32,
    pub run_id: String,
    pub round_label: String,
    pub round_record_hash: String,
    pub action: Action,
    pub reason: String,
}

impl Decision {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported decision schema version"
        );
        ensure!(
            !self.reason.is_empty()
                && self.reason.len() <= 1024
                && !self.reason.chars().any(char::is_control),
            "decision reason must be 1..1024 bytes without control characters"
        );
        Ok(())
    }
}

pub fn write(args: Write) -> Result<()> {
    let state = private_state::load(&args.private_state)?;
    let snapshot = private_state::snapshot(&args.private_state)?;
    let hash = evidence::blob_hash(
        &state.state_dir,
        &secure_fs::read(&state.record, evidence::LIMIT)?,
    )?;
    ensure!(
        hash == snapshot.record_hash,
        "round record changed after sealing"
    );
    let decision = Decision {
        schema_version: 1,
        run_id: state.run_id,
        round_label: state.round_label,
        round_record_hash: hash,
        action: args.action,
        reason: args.reason,
    };
    decision.validate()?;
    secure_fs::create(&state.decision_file, &serde_json::to_vec(&decision)?)?;
    println!("{}", state.decision_file.display());
    Ok(())
}
