//! Plugin commands — delegated to Python.

use anyhow::{Context, Result};
use std::process::Command;

fn delegate(args: &[&str]) -> Result<()> {
    let status = Command::new("python3")
        .arg("-m")
        .arg("amplihack.cli")
        .arg("plugin")
        .args(args)
        .status()
        .context("failed to spawn python3 -m amplihack.cli plugin")?;

    if !status.success() {
        anyhow::bail!(
            "amplihack plugin {} exited with status {}",
            args.join(" "),
            status
        );
    }
    Ok(())
}

pub fn run_install(name: &str) -> Result<()> {
    delegate(&["install", name])
}

pub fn run_uninstall(name: &str) -> Result<()> {
    delegate(&["uninstall", name])
}

pub fn run_link(path: &str) -> Result<()> {
    delegate(&["link", path])
}

pub fn run_verify() -> Result<()> {
    delegate(&["verify"])
}
