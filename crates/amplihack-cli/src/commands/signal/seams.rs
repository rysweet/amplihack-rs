//! Injectable seams for the Signal fleet path (#921/#923).
//!
//! The orchestration logic depends on these traits rather than concrete Azure /
//! process calls, so `plan_rollout` and friends are testable with fakes and
//! zero cloud dependency (see `tests/signal_setup_idempotency.rs`).

use anyhow::{Context, Result};
use serde::Deserialize;

/// A single VM record from `azlin list` / `az vm list` JSON. Only `name` is
/// needed; every other field is ignored so the extractors are tolerant of the
/// full object shapes both tools emit.
#[derive(Deserialize)]
struct VmRecord {
    name: String,
}

/// Extract VM names from `azlin list --output json` (an array of objects each
/// with at least a `name`). Malformed or non-array input yields an empty list
/// (the caller decides whether that triggers the `az` fallback).
pub fn vm_names_from_azlin_json(json: &str) -> Vec<String> {
    names_from_json(json)
}

/// Extract VM names from `az vm list --output json` (an array of objects each
/// with at least a `name`). Malformed or non-array input yields an empty list.
pub fn vm_names_from_az_vm_list_json(json: &str) -> Vec<String> {
    names_from_json(json)
}

fn names_from_json(json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<VmRecord>>(json)
        .map(|records| records.into_iter().map(|r| r.name).collect())
        .unwrap_or_default()
}

/// Combine the azlin-first discovery with a generic `az vm list` fallback.
///
/// `azlin` is azlin's result. When it is a **non-empty** `Ok`, it is used
/// as-is (the fallback is never invoked). Otherwise — empty `Ok` or `Err` — the
/// `az_fallback` closure runs. A fallback failure **surfaces** as an error; a
/// total discovery failure is never silently degraded into an empty fleet.
pub fn resolve_vm_list<F>(azlin: Result<Vec<String>>, az_fallback: F) -> Result<Vec<String>>
where
    F: FnOnce() -> Result<Vec<String>>,
{
    match azlin {
        Ok(names) if !names.is_empty() => Ok(names),
        _ => az_fallback(),
    }
}

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
                "--output",
                "json",
            ])
            .output()
            .context("failed to run `az vm list` (is the Azure CLI installed and logged in?)")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("`az vm list` failed ({}): {}", output.status, stderr.trim());
        }
        // Reuse the pure, unit-tested extractor so the parse rule cannot drift.
        let json = String::from_utf8_lossy(&output.stdout);
        Ok(vm_names_from_az_vm_list_json(&json))
    }
}
