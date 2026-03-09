//! Mode commands — delegated to Python.

use anyhow::{Context, Result};
use std::process::Command;

fn delegate(args: &[&str]) -> Result<()> {
    let status = Command::new("python3")
        .arg("-m")
        .arg("amplihack.cli")
        .arg("mode")
        .args(args)
        .status()
        .context("failed to spawn python3 -m amplihack.cli mode")?;

    if !status.success() {
        anyhow::bail!(
            "amplihack mode {} exited with status {}",
            args.join(" "),
            status
        );
    }
    Ok(())
}

pub fn run_detect() -> Result<()> {
    delegate(&["detect"])
}

pub fn run_to_plugin() -> Result<()> {
    delegate(&["to-plugin"])
}

pub fn run_to_local() -> Result<()> {
    delegate(&["to-local"])
}
