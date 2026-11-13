//! Starts the recovery recipe and returns the agent's text.
//!
//! The child is `amplihack recipe run step-failure-recovery -c
//! step_recovery_evidence=<path> --format json` with
//! `AMPLIHACK_STEP_RECOVERY_ACTIVE=1`. Its exit status is ignored: only the
//! reply line counts, and a missing reply becomes a validator handoff. So
//! does output of `OUTPUT_LIMIT` bytes or more: it is never parsed cut short.

use anyhow::{Context, Result, ensure};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

/// Child output at or over this size is refused.
pub const OUTPUT_LIMIT: usize = 1024 * 1024;
pub const ACTIVE_ENV: &str = "AMPLIHACK_STEP_RECOVERY_ACTIVE";

/// Runs the recovery recipe on `evidence` and returns the last step's output
/// (or, when the result is not JSON, the raw output) to read the reply from.
pub fn run_agent(evidence: &Path, recipe_dir: Option<&Path>) -> Result<String> {
    let exe = std::env::current_exe().context("cannot find the amplihack binary")?;
    let recipe = match recipe_dir {
        Some(dir) => dir
            .join("step-failure-recovery.yaml")
            .to_string_lossy()
            .into_owned(),
        None => "step-failure-recovery".to_string(),
    };
    eprintln!(
        "[step-recovery] starting recovery agent with evidence {}",
        evidence.display()
    );
    let mut child = Command::new(exe)
        .args(["recipe", "run", &recipe, "--format", "json", "-c"])
        .arg(format!("step_recovery_evidence={}", evidence.display()))
        .env(ACTIVE_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("cannot start the recovery recipe")?;
    let mut stdout = child.stdout.take().context("no child stdout")?;
    let mut bytes = Vec::new();
    (&mut stdout)
        .take(OUTPUT_LIMIT as u64)
        .read_to_end(&mut bytes)
        .context("cannot read the recovery recipe's output")?;
    // Drain anything past the limit so the child never blocks on a full pipe.
    std::io::copy(&mut stdout, &mut std::io::sink()).context("cannot drain recovery output")?;
    let status = child
        .wait()
        .context("cannot wait for the recovery recipe")?;
    eprintln!("[step-recovery] recovery recipe ended with {status}");
    reply_text(&bytes)
}

/// The text to read the reply from; output that reached `OUTPUT_LIMIT` may
/// have been cut, so it is refused rather than parsed.
pub fn reply_text(bytes: &[u8]) -> Result<String> {
    ensure!(
        bytes.len() < OUTPUT_LIMIT,
        "no step_recovery reply: output too large"
    );
    Ok(agent_text(&String::from_utf8_lossy(bytes)))
}

/// The output of the last step of a JSON run result; the raw text when the
/// output holds no run result. Only its final non-empty line is the reply.
pub fn agent_text(stdout: &str) -> String {
    let result = serde_json::from_str::<serde_json::Value>(stdout.trim())
        .ok()
        .filter(|v| v.get("step_results").is_some())
        .or_else(|| crate::commands::orch::extract_json_with_field(stdout, "step_results"));
    match result.as_ref().and_then(|v| v["step_results"].as_array()) {
        Some(steps) => steps
            .last()
            .and_then(|s| s["output"].as_str())
            .unwrap_or_default()
            .to_string(),
        None => stdout.to_string(),
    }
}
