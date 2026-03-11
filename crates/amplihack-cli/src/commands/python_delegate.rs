//! Delegate unported subcommands to the Python CLI.
//!
//! Some amplihack subcommands (fleet, new, RustyClawd, uvx-help) are only
//! implemented in Python. When the Rust binary encounters one of these, it
//! spawns `python3 -m amplihack.cli <subcommand> <args>` and propagates the
//! exit code.

use anyhow::{Context, Result};
use std::process::Command;

/// Execute a subcommand via the Python CLI, propagating its exit code.
///
/// This spawns `python3 -m amplihack.cli <subcommand> [args...]` and returns
/// Ok(()) on success or exits with the Python process's exit code on failure.
pub fn delegate_to_python(subcommand: &str, args: &[String]) -> Result<()> {
    tracing::debug!(
        subcommand,
        ?args,
        "delegating to Python CLI: python3 -m amplihack.cli"
    );

    let mut cmd = Command::new("python3");
    cmd.arg("-m").arg("amplihack.cli").arg(subcommand);
    cmd.args(args);

    let status = cmd
        .status()
        .with_context(|| {
            format!(
                "failed to exec python3 -m amplihack.cli {subcommand} — \
                 is python3 installed and amplihack available?"
            )
        })?;

    if status.success() {
        Ok(())
    } else {
        let code = status.code().unwrap_or(1);
        std::process::exit(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegate_returns_error_when_python3_missing() {
        // Use a nonexistent interpreter to verify error path.
        let mut cmd = Command::new("__nonexistent_python3__");
        cmd.arg("-m").arg("amplihack.cli").arg("fleet");
        let result = cmd.status();
        assert!(result.is_err(), "should fail when binary doesn't exist");
    }
}
