//! Injectable seams for the Signal fleet path (#921/#923).
//!
//! The orchestration logic depends on these traits rather than concrete Azure /
//! process calls, so `plan_rollout` and friends are testable with fakes and
//! zero cloud dependency (see `tests/signal_setup_idempotency.rs`).

use anyhow::{Context, Result};

/// Enumerates the VM names in an operator resource group. Real impl shells out
/// to `az vm list`; tests inject a fake.
pub trait VmLister {
    /// List VM names in `resource_group`. Errors propagate (no silent empty
    /// fallback): a discovery failure must surface, never masquerade as "no
    /// VMs".
    fn list_vms(&self, resource_group: &str) -> Result<Vec<String>>;
}

/// Real [`VmLister`] backed by the Azure CLI (`az vm list`).
pub struct AzVmLister;

impl VmLister for AzVmLister {
    fn list_vms(&self, resource_group: &str) -> Result<Vec<String>> {
        super::validate::validate_resource_group(resource_group)
            .context("resource group failed validation")?;
        let output = std::process::Command::new("az")
            .args([
                "vm",
                "list",
                "--resource-group",
                resource_group,
                "--query",
                "[].name",
                "--output",
                "json",
            ])
            .output()
            .context("failed to run `az vm list` (is the Azure CLI installed and logged in?)")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("`az vm list` failed ({}): {}", output.status, stderr.trim());
        }
        let names: Vec<String> = serde_json::from_slice(&output.stdout)
            .context("failed to parse `az vm list` JSON output")?;
        Ok(names)
    }
}
