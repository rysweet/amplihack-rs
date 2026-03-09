//! Recipe commands — delegated to Python.

use anyhow::{Context, Result};
use std::process::Command;

fn delegate(args: &[&str]) -> Result<()> {
    let status = Command::new("python3")
        .arg("-m")
        .arg("amplihack.cli")
        .arg("recipe")
        .args(args)
        .status()
        .context("failed to spawn python3 -m amplihack.cli recipe")?;

    if !status.success() {
        anyhow::bail!(
            "amplihack recipe {} exited with status {}",
            args.join(" "),
            status
        );
    }
    Ok(())
}

pub fn run_recipe(name: &str, args: &[String]) -> Result<()> {
    let mut cmd_args = vec!["run", name];
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    cmd_args.extend(arg_refs);
    delegate(&cmd_args)
}

pub fn run_list() -> Result<()> {
    delegate(&["list"])
}

pub fn run_validate(file: &str) -> Result<()> {
    delegate(&["validate", file])
}

pub fn run_show(name: &str) -> Result<()> {
    delegate(&["show", name])
}
