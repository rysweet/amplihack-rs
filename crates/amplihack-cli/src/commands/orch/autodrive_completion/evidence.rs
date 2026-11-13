use super::{private_state::State, secure_fs};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub const LIMIT: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub record_hash: String,
    pub sidecars: BTreeMap<String, Option<Vec<u8>>>,
}

pub fn blob_hash(dir: &Path, bytes: &[u8]) -> Result<String> {
    let mut child = amplihack_git::command_in(dir)
        .args(["hash-object", "--no-filters", "--stdin"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .context("git stdin missing")?
        .write_all(bytes)?;
    let o = child.wait_with_output()?;
    ensure!(o.status.success(), "could not hash round evidence");
    let hash = String::from_utf8(o.stdout)?.trim().to_string();
    ensure!(
        [40, 64].contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid evidence hash"
    );
    Ok(hash)
}

pub fn review_stamp(tools: &Path, dir: &Path) -> Result<String> {
    let o = Command::new("bash")
        .arg(tools.join("autodrive_worktree_guard.sh"))
        .args(["index-stamp"])
        .arg(dir)
        .output()?;
    ensure!(o.status.success(), "cannot stamp trusted review directory");
    Ok(String::from_utf8(o.stdout)?)
}

pub fn capture(state: &State) -> Result<Snapshot> {
    let bytes = secure_fs::read(secure_fs::Scope::of(&state.state_dir), &state.record, LIMIT)?;
    let mut sidecars = BTreeMap::new();
    for ext in ["findings", "minor", "severity"] {
        let path = state.record.with_extension(format!("json.{ext}"));
        let value = match std::fs::symlink_metadata(&path) {
            Ok(_) => Some(secure_fs::read(
                secure_fs::Scope::of(&state.state_dir),
                &path,
                LIMIT,
            )?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        sidecars.insert(ext.to_string(), value);
    }
    ensure!(sidecars["findings"].is_some(), "missing findings sidecar");
    Ok(Snapshot {
        record_hash: blob_hash(&state.state_dir, &bytes)?,
        sidecars,
    })
}

pub fn validate(state: &State, expected: &Snapshot) -> Result<serde_json::Value> {
    let actual = capture(state)?;
    ensure!(
        actual.record_hash == expected.record_hash && actual.sidecars == expected.sidecars,
        "round record or sidecar evidence changed"
    );
    let bytes = secure_fs::read(secure_fs::Scope::of(&state.state_dir), &state.record, LIMIT)?;
    ensure!(
        bytes
            == secure_fs::read(
                secure_fs::Scope::of(&state.state_dir),
                &state.state_dir.join("crusty-latest.json"),
                LIMIT
            )?,
        "latest round copy differs"
    );
    ensure!(
        actual.sidecars["findings"].as_ref()
            == Some(&secure_fs::read(
                secure_fs::Scope::of(&state.state_dir),
                &state.state_dir.join("crusty-latest.json.findings"),
                LIMIT
            )?),
        "latest findings copy differs"
    );
    let manifest = String::from_utf8(secure_fs::read(
        secure_fs::Scope::of(&state.state_dir),
        &state.state_dir.join("crusty-records.tsv"),
        LIMIT,
    )?)?;
    let expected_row = format!(
        "{}\t{}\t{}",
        state.round_label,
        state
            .record
            .file_name()
            .context("missing record name")?
            .to_string_lossy(),
        actual.record_hash
    );
    ensure!(
        manifest.lines().rfind(|l| !l.trim().is_empty()) == Some(expected_row.as_str()),
        "manifest does not bind this round"
    );
    ensure!(
        review_stamp(&state.launch_tools, &state.review_dir)? == state.review_stamp,
        "trusted review directory changed"
    );
    let record: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        record["round_label"].as_str() == Some(&state.round_label),
        "record round identity mismatch"
    );
    let head = amplihack_git::command_in(&state.repo)
        .args(["rev-parse", "HEAD"])
        .output()?;
    ensure!(head.status.success(), "cannot read current PR head");
    let head = String::from_utf8(head.stdout)?;
    ensure!(
        record["head_sha"].as_str() == Some(head.trim()),
        "record does not describe current head"
    );
    if matches!(
        record["crusty_verdict"].as_str(),
        Some("CLEAN" | "MINOR_ONLY")
    ) {
        let remote = Command::new("gh")
            .current_dir(&state.repo)
            .env_remove("GH_REPO")
            .args(["pr", "view", "--json", "headRefOid", "--jq", ".headRefOid"])
            .output()?;
        ensure!(
            remote.status.success() && String::from_utf8(remote.stdout)?.trim() == head.trim(),
            "current PR head differs from reviewed evidence or is unavailable"
        );
        ensure!(
            record["reviewed_head_sha"].as_str() == Some(head.trim()),
            "completion must cover the current reviewed head"
        );
        ensure!(
            actual.sidecars["findings"]
                .as_ref()
                .is_some_and(|v| v.iter().all(u8::is_ascii_whitespace)),
            "completed record contains unresolved findings"
        );
        if record["crusty_verdict"] == "MINOR_ONLY" {
            ensure!(
                actual.sidecars["minor"]
                    .as_ref()
                    .is_some_and(|v| !v.is_empty())
                    && actual.sidecars["severity"].is_some(),
                "minor-only evidence missing"
            );
        }
    }
    Ok(record)
}
