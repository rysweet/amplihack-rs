//! Node.js diagnostics and conservative remediation for Copilot CLI support.

use crate::command_error::exit_error;
use crate::util::{render_diagnostic_bytes, run_output_with_timeout};
use amplihack_utils::prerequisites::{NODE_MINIMUM_MAJOR, parse_node_major_version};
use anyhow::Result;
use std::process::Command;
use std::time::Duration;

const NODE_VERSION_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run_node_diagnostic(ensure: bool) -> Result<()> {
    let mut command = Command::new("node");
    command.arg("--version");
    let output = run_output_with_timeout(command, NODE_VERSION_TIMEOUT);
    match output {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout);
            let version = version.trim();
            match parse_node_major_version(version) {
                Some(major) if major >= NODE_MINIMUM_MAJOR => {
                    println!("Node.js {version} satisfies Copilot CLI requirement >=24.0.0");
                    Ok(())
                }
                Some(major) => fail_with_remediation(
                    ensure,
                    &format!(
                        "Node.js {version} is too old. Required: >=24.0.0; found major {major}."
                    ),
                ),
                None => fail_with_remediation(
                    ensure,
                    &format!("Node.js version output is malformed or invalid: {version}"),
                ),
            }
        }
        Ok(out) => fail_with_remediation(
            ensure,
            &format!(
                "Node.js version check failed: {}",
                if out.stderr.is_empty() {
                    "node --version exited non-zero".to_string()
                } else {
                    render_diagnostic_bytes(&out.stderr, 500)
                }
            ),
        ),
        Err(err) => fail_with_remediation(ensure, &format!("Node.js is missing: {err}")),
    }
}

fn fail_with_remediation(ensure: bool, reason: &str) -> Result<()> {
    eprintln!("Node.js remediation required for Copilot CLI.");
    eprintln!("{reason}");
    eprintln!();
    eprintln!("Manual repair commands:");
    eprintln!("  nvm install 24");
    eprintln!("  nvm use 24");
    eprintln!("  node --version");
    eprintln!("  amplihack doctor node");

    if ensure {
        eprintln!();
        eprintln!(
            "Automatic install was not attempted: no safe, explicitly configured Node.js manager was detected."
        );
        eprintln!(
            "Run the manual commands above, or launch Copilot interactively so amplihack can use its managed Node.js bootstrap path."
        );
    }

    Err(exit_error(1))
}
