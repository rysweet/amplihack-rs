//! Per-execution private refusal channel; stdout and old state are not authority.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, process::Command};

pub(crate) const CHANNEL_ENV: &str = "AMPLIHACK_PREFLIGHT_CHANNELS";
#[derive(Serialize, Deserialize)]
struct Channel {
    path: PathBuf,
    nonce: String,
}
pub(crate) struct ChannelOwner {
    _dir: tempfile::TempDir,
    channel: Channel,
}
impl ChannelOwner {
    pub(crate) fn attach(command: &mut Command) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let channel = Channel {
            path: dir.path().join("refusal.json"),
            nonce: uuid::Uuid::new_v4().to_string(),
        };
        let mut inherited: Vec<Channel> = match std::env::var(CHANNEL_ENV) {
            Ok(s) => serde_json::from_str(&s).context("invalid inherited preflight channels")?,
            Err(std::env::VarError::NotPresent) => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        inherited.push(Channel {
            path: channel.path.clone(),
            nonce: channel.nonce.clone(),
        });
        command.env(CHANNEL_ENV, serde_json::to_string(&inherited)?);
        Ok(Self { _dir: dir, channel })
    }
    pub(crate) fn read(&self) -> Result<Option<Value>> {
        let bytes = match fs::read(&self.channel.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let record: Value = serde_json::from_slice(&bytes)?;
        if record["nonce"] != self.channel.nonce || record["helper_exit"] != 1 {
            bail!("invalid preflight execution provenance");
        }
        let envelope = record["envelope"].clone();
        if envelope["schema_version"] != 1
            || envelope["terminal_refusal"] != true
            || !known(envelope["outcome"].as_str().unwrap_or(""))
            || !envelope["evidence"].is_object()
            || envelope["success"] == true
            || envelope.get("disposition").is_some()
        {
            bail!("invalid preflight refusal envelope");
        }
        Ok(Some(envelope))
    }
}
pub(crate) fn known(outcome: &str) -> bool {
    serde_json::from_value::<super::autodrive_preflight::RefusalOutcome>(Value::String(
        outcome.into(),
    ))
    .is_ok()
}
pub(crate) fn publish(envelope: &Value) -> Result<()> {
    let channels: Vec<Channel> = match std::env::var(CHANNEL_ENV) {
        Ok(s) => serde_json::from_str(&s)?,
        Err(std::env::VarError::NotPresent) => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for channel in channels {
        let parent = channel.path.parent().context("missing channel parent")?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(
            &mut file,
            &json!({"nonce":channel.nonce,"helper_exit":1,"envelope":envelope}),
        )?;
        file.as_file().sync_all()?;
        file.persist(channel.path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner_and_channel() -> (ChannelOwner, Channel) {
        let mut command = Command::new("unused");
        let owner = ChannelOwner::attach(&mut command).unwrap();
        let encoded = command
            .get_envs()
            .find(|(k, _)| *k == CHANNEL_ENV)
            .unwrap()
            .1
            .unwrap();
        let mut channels: Vec<Channel> = serde_json::from_str(encoded.to_str().unwrap()).unwrap();
        (owner, channels.pop().unwrap())
    }

    #[test]
    fn issue_2188_empty_current_channel_ignores_old_durable_records() {
        let (owner, _) = owner_and_channel();
        assert!(owner.read().unwrap().is_none());
    }

    #[test]
    fn issue_2188_rejects_stale_forged_and_contradictory_envelopes() {
        let (owner, channel) = owner_and_channel();
        for (nonce, exit, version, terminal, outcome) in [
            ("old-run", 1, 1, true, "pr-worktree-dirty"),
            (channel.nonce.as_str(), 0, 1, true, "pr-worktree-dirty"),
            (channel.nonce.as_str(), 1, 2, true, "pr-worktree-dirty"),
            (channel.nonce.as_str(), 1, 1, false, "pr-worktree-dirty"),
            (channel.nonce.as_str(), 1, 1, true, "ready"),
        ] {
            fs::write(&channel.path, json!({"nonce":nonce,"helper_exit":exit,"envelope":{"schema_version":version,"terminal_refusal":terminal,"outcome":outcome}}).to_string()).unwrap();
            assert!(owner.read().is_err());
        }
    }

    #[test]
    fn issue_2188_known_refusal_retains_evidence() {
        let (owner, channel) = owner_and_channel();
        let envelope = json!({"schema_version":1,"terminal_refusal":true,"outcome":"pr-worktree-interrupted","evidence":{"operations":["rebase-merge"],"no_unmerged_paths":true}});
        fs::write(
            &channel.path,
            json!({"nonce":channel.nonce,"helper_exit":1,"envelope":envelope}).to_string(),
        )
        .unwrap();
        assert_eq!(owner.read().unwrap(), Some(envelope));
    }
}
