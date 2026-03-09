//! Memory commands — delegated to Python.

use anyhow::{Context, Result};
use std::process::Command;

fn delegate(args: &[&str]) -> Result<()> {
    let status = Command::new("python3")
        .arg("-m")
        .arg("amplihack.cli")
        .arg("memory")
        .args(args)
        .status()
        .context("failed to spawn python3 -m amplihack.cli memory")?;

    if !status.success() {
        anyhow::bail!(
            "amplihack memory {} exited with status {}",
            args.join(" "),
            status
        );
    }
    Ok(())
}

pub fn run_tree() -> Result<()> {
    delegate(&["tree"])
}

pub fn run_export(output: Option<&str>) -> Result<()> {
    match output {
        Some(path) => delegate(&["export", "--output", path]),
        None => delegate(&["export"]),
    }
}

pub fn run_import(file: &str) -> Result<()> {
    delegate(&["import", file])
}

pub fn run_clean() -> Result<()> {
    delegate(&["clean"])
}
