//! Execute the trusted singular QA override without changing its shell source.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Run `AUTODRIVE_QA_COMMAND` through Bash in `dir`, returning its exit code.
///
/// Only the operator override uses this executor. Repository manifests, CI and
/// detected commands retain their restricted execution path in the evidence tool.
pub(super) fn run(dir: &Path) -> Result<i32> {
    let source = std::env::var_os("AUTODRIVE_QA_COMMAND")
        .context("AUTODRIVE_QA_COMMAND must be set for autodrive-qa-command")?;
    let status = Command::new("bash")
        .arg("-c")
        .arg(source)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("cannot run Bash QA command in {}", dir.display()))?;

    if let Some(code) = status.code() {
        return Ok(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return Ok(128 + signal);
        }
    }
    anyhow::bail!("Bash QA command ended without an exit code")
}
