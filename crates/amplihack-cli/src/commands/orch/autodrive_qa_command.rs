//! Execute the trusted singular QA override without changing its shell source.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Run `AUTODRIVE_QA_COMMAND` through Bash in `dir` and persist its exit code.
///
/// Only the operator override uses this executor. Repository manifests, CI and
/// detected commands retain their restricted execution path in the evidence tool.
/// Success means Bash completed and its status was written to `result_file`;
/// a nonzero QA status is a measured result, not a helper failure.
pub(super) fn run(dir: &Path, result_file: &Path) -> Result<()> {
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

    let code = status.code();
    #[cfg(unix)]
    let code = code.or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|signal| 128 + signal)
    });
    let code = code.context("Bash QA command ended without an exit code")?;
    std::fs::write(result_file, format!("{code}\n")).with_context(|| {
        format!(
            "cannot write Bash QA completion result {}",
            result_file.display()
        )
    })
}
