//! Injectable seams for the Signal fleet path (#921/#923).
//!
//! The orchestration logic depends on these traits rather than concrete Azure /
//! process calls, so `plan_rollout` and friends are testable with fakes and
//! zero cloud dependency (see `tests/signal_setup_idempotency.rs`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::error::SignalOpError;

/// Result alias mirroring `run.rs`: Signal orchestration errors carry the
/// stable exit-code taxonomy (see [`super::error::SignalOpError`]).
type OpResult<T> = Result<T, SignalOpError>;

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

/// Real [`VmLister`] backed by azlin-first discovery with Azure CLI fallback.
pub struct AzVmLister;

impl VmLister for AzVmLister {
    fn list_vms(&self, resource_group: &str) -> Result<Vec<String>> {
        super::validate::validate_resource_group(resource_group)
            .context("resource group failed validation")?;
        let azlin = list_vms_with_azlin();
        match &azlin {
            Ok(names) if names.is_empty() => eprintln!(
                "signal distribute: `azlin list --json` returned no VMs; falling back to `az vm list`"
            ),
            Err(err) => eprintln!(
                "signal distribute: `azlin list --json` failed; falling back to `az vm list`: {err}"
            ),
            _ => {}
        }
        resolve_vm_list(azlin, || list_vms_with_az(resource_group))
    }
}

fn list_vms_with_azlin() -> Result<Vec<String>> {
    let output = std::process::Command::new("azlin")
        .args(["list", "--json"])
        .output()
        .context("failed to run `azlin list --json` (is azlin installed and authenticated?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "`azlin list --json` failed ({}): {}",
            output.status,
            stderr.trim()
        );
    }
    let json = String::from_utf8_lossy(&output.stdout);
    Ok(vm_names_from_azlin_json(&json))
}

fn list_vms_with_az(resource_group: &str) -> Result<Vec<String>> {
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

// ---------------------------------------------------------------------------
// Injectable I/O seams for `run_setup` (#921/#971 R3).
//
// The interactive host-onboarding path in `run.rs` drives three external
// effects: locating the `signal-cli` binary, running the device-link handshake,
// and pacing the daemon-readiness poll. Each is abstracted behind an
// object-safe trait so `run_setup_with` accepts `&dyn` collaborators. Real
// impls delegate verbatim to the (unchanged) `run.rs` functions so the existing
// green contracts are preserved; test fakes are injected only in new seam-level
// tests (see `tests/signal_seams_injection.rs`) — no real cloud, process, or
// device-link URI is ever produced by a fake.
// ---------------------------------------------------------------------------

/// Locates the `signal-cli` binary. Real impl inspects `PATH`; a discovery
/// failure surfaces install guidance rather than silently degrading.
pub trait SignalCliInvoker {
    /// Return the path to a usable `signal-cli`, or a `SignalCli` error.
    fn detect(&self) -> OpResult<PathBuf>;
}

/// Runs the interactive `signal-cli link` device-link handshake and returns the
/// linked account (E.164).
///
/// SECURITY (R9): implementations MUST keep the device-link URI on **stderr
/// only** and never persist or log it — it is a bearer secret. Test fakes must
/// not emit a real URI.
pub trait LinkSession {
    /// Link a new device named `device_name` (or a default), returning the
    /// associated E.164 account on success.
    fn link(&self, signal_cli: &Path, device_name: Option<&str>) -> OpResult<String>;
}

/// Time seam for the daemon-readiness poll. Deliberately minimal (ruthless
/// simplicity): only the blocking `sleep` used by `wait_for_daemon` is
/// abstracted, so tests can inject a non-blocking [`FakeClock`]-style stub and
/// keep the poll deterministic and instant.
pub trait Clock {
    /// Block for `dur`. The real clock sleeps; fakes typically no-op.
    fn sleep(&self, dur: Duration);
}

/// Real [`SignalCliInvoker`] delegating to the production detection logic.
pub struct SignalCliBin;

impl SignalCliInvoker for SignalCliBin {
    fn detect(&self) -> OpResult<PathBuf> {
        super::run::detect_signal_cli()
    }
}

/// Real [`LinkSession`] delegating to the production `signal-cli link` driver.
pub struct CliLinkSession;

impl LinkSession for CliLinkSession {
    fn link(&self, signal_cli: &Path, device_name: Option<&str>) -> OpResult<String> {
        super::run::link_device(signal_cli, device_name)
    }
}

/// Real [`Clock`] backed by `std::thread::sleep` (wall-clock pacing).
pub struct SystemClock;

impl Clock for SystemClock {
    fn sleep(&self, dur: Duration) {
        std::thread::sleep(dur);
    }
}
