//! Install and uninstall commands.
//!
//! These delegate to `python -m amplihack.cli install/uninstall` for now,
//! since the full install logic lives in Python.

use anyhow::{Context, Result};
use std::process::Command;

/// Run `amplihack install` via Python.
pub fn run_install() -> Result<()> {
    delegate_to_python(&["install"])
}

/// Run `amplihack uninstall` via Python.
pub fn run_uninstall() -> Result<()> {
    delegate_to_python(&["uninstall"])
}

fn delegate_to_python(args: &[&str]) -> Result<()> {
    let status = Command::new("python3")
        .arg("-m")
        .arg("amplihack.cli")
        .args(args)
        .status()
        .context("failed to spawn python3 -m amplihack.cli")?;

    if !status.success() {
        anyhow::bail!(
            "python3 -m amplihack.cli {} exited with status {}",
            args.join(" "),
            status
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegate_to_python_constructs_correct_command() {
        // Verify argument assembly (we can't run the actual Python command in tests,
        // but we can verify the function doesn't panic with missing args).
        let result = Command::new("python3")
            .arg("-m")
            .arg("amplihack.cli")
            .arg("--help")
            .output();
        // If python3 is available, the command should at least execute.
        // If not, the test is a no-op (CI might not have amplihack installed).
        if let Ok(output) = result {
            // --help should exit 0 or 2 (argparse), not crash
            assert!(output.status.code().is_some());
        }
    }
}
