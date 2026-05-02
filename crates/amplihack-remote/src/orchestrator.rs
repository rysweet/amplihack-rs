//! VM lifecycle orchestration via azlin CLI.
//!
//! Manages Azure VM provisioning, reuse, and cleanup for remote
//! amplihack execution.

use std::collections::HashMap;
use std::process::Stdio;

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::azlin_parse::{parse_azlin_list_json, parse_azlin_list_text};
use crate::error::{ErrorContext, RemoteError};

/// Represents an Azure VM managed by azlin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VM {
    pub name: String,
    pub size: String,
    pub region: String,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub tags: Option<HashMap<String, String>>,
}

impl VM {
    /// Age of the VM in hours (0.0 if `created_at` is `None`).
    pub fn age_hours(&self) -> f64 {
        match self.created_at {
            Some(ts) => {
                let delta = Utc::now() - ts;
                delta.num_seconds() as f64 / 3600.0
            }
            None => 0.0,
        }
    }
}

/// Options for VM provisioning / reuse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VMOptions {
    pub size: String,
    pub region: Option<String>,
    pub vm_name: Option<String>,
    pub no_reuse: bool,
    pub keep_vm: bool,
    pub azlin_extra_args: Option<Vec<String>>,
    pub tunnel_port: Option<u16>,
}

impl Default for VMOptions {
    fn default() -> Self {
        Self {
            size: "Standard_D2s_v3".to_string(),
            region: None,
            vm_name: None,
            no_reuse: false,
            keep_vm: false,
            azlin_extra_args: None,
            tunnel_port: None,
        }
    }
}

/// Orchestrates VM lifecycle via azlin.
pub struct Orchestrator {
    username: String,
}

impl Orchestrator {
    /// Create a new orchestrator, verifying that azlin is installed.
    pub async fn new(username: Option<String>) -> Result<Self, RemoteError> {
        let username = username
            .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "amplihack".to_string()));

        let this = Self { username };
        this.verify_azlin().await?;
        Ok(this)
    }

    /// Get a VM: reuse existing or provision new.
    pub async fn provision_or_reuse(&self, options: &VMOptions) -> Result<VM, RemoteError> {
        if let Some(ref name) = options.vm_name {
            return self.get_vm_by_name(name).await;
        }

        if !options.no_reuse
            && let Some(vm) = self.find_reusable_vm(options).await?
        {
            info!(
                vm = %vm.name,
                age_hours = format!("{:.1}", vm.age_hours()),
                "reusing existing VM"
            );
            return Ok(vm);
        }

        self.provision_new_vm(options).await
    }

    /// Cleanup a VM via `azlin kill`.
    pub async fn cleanup(&self, vm: &VM, force: bool) -> Result<bool, RemoteError> {
        info!(vm = %vm.name, "cleaning up VM");

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            Command::new("azlin")
                .args(["kill", &vm.name])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;

        match output {
            Ok(Ok(o)) if o.status.success() => {
                info!(vm = %vm.name, "VM cleanup successful");
                Ok(true)
            }
            Ok(Ok(o)) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let msg = format!("VM cleanup failed: {stderr}");
                if force {
                    warn!("{msg}");
                    Ok(false)
                } else {
                    Err(RemoteError::cleanup_ctx(
                        msg,
                        ErrorContext::new().insert("vm_name", &vm.name),
                    ))
                }
            }
            Ok(Err(e)) => {
                let msg = format!("VM cleanup command failed: {e}");
                if force {
                    warn!("{msg}");
                    Ok(false)
                } else {
                    Err(RemoteError::cleanup(msg))
                }
            }
            Err(_) => {
                let msg = "VM cleanup timed out";
                if force {
                    warn!(vm = %vm.name, "{msg}");
                    Ok(false)
                } else {
                    Err(RemoteError::cleanup_ctx(
                        msg,
                        ErrorContext::new().insert("vm_name", &vm.name),
                    ))
                }
            }
        }
    }

    // ---- internal ----

    async fn verify_azlin(&self) -> Result<(), RemoteError> {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            Command::new("azlin")
                .arg("--version")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) if output.status.success() => Ok(()),
            Ok(Ok(_)) => Err(RemoteError::provisioning(
                "azlin command failed. Install: pip install azlin",
            )),
            Ok(Err(_)) => Err(RemoteError::provisioning(
                "azlin not found. Install: pip install azlin",
            )),
            Err(_) => Err(RemoteError::provisioning("azlin version check timed out")),
        }
    }

    async fn find_reusable_vm(&self, options: &VMOptions) -> Result<Option<VM>, RemoteError> {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            Command::new("azlin")
                .args(["list", "--json"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;

        let vms = match output {
            Ok(Ok(o)) if o.status.success() => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                parse_azlin_list_json(&stdout)
            }
            _ => {
                debug!("azlin list --json failed, trying text");
                let output2 = tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    Command::new("azlin")
                        .arg("list")
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .output(),
                )
                .await;
                match output2 {
                    Ok(Ok(o)) => {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        parse_azlin_list_text(&stdout)
                    }
                    _ => {
                        warn!("could not list VMs for reuse");
                        return Ok(None);
                    }
                }
            }
        };

        for vm in vms {
            if !vm.name.starts_with("amplihack-") {
                continue;
            }
            if vm.size != options.size {
                continue;
            }
            if vm.age_hours() > 24.0 {
                continue;
            }
            return Ok(Some(vm));
        }

        Ok(None)
    }

    async fn provision_new_vm(&self, options: &VMOptions) -> Result<VM, RemoteError> {
        let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
        let vm_name = format!("amplihack-{}-{timestamp}", self.username);

        info!(
            vm = %vm_name,
            size = %options.size,
            "provisioning new VM"
        );

        let mut cmd_args = vec![
            "new".to_string(),
            "--size".to_string(),
            options.size.clone(),
            "--name".to_string(),
            vm_name.clone(),
            "--yes".to_string(),
        ];
        if let Some(ref region) = options.region {
            cmd_args.push("--region".to_string());
            cmd_args.push(region.clone());
        }
        if let Some(ref extra) = options.azlin_extra_args {
            cmd_args.extend(extra.clone());
        }

        let max_retries = 3u32;
        for attempt in 0..max_retries {
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(600),
                Command::new("azlin")
                    .args(&cmd_args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output(),
            )
            .await;

            match output {
                Ok(Ok(o)) if o.status.success() => {
                    info!(vm = %vm_name, "VM provisioned");
                    let mut tags = HashMap::new();
                    tags.insert("amplihack_workflow".into(), "true".into());
                    return Ok(VM {
                        name: vm_name,
                        size: options.size.clone(),
                        region: options
                            .region
                            .clone()
                            .unwrap_or_else(|| "default".to_string()),
                        created_at: Some(Utc::now()),
                        tags: Some(tags),
                    });
                }
                Ok(Ok(o)) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    let lower = stderr.to_lowercase();
                    if lower.contains("quota") || lower.contains("limit") {
                        return Err(RemoteError::provisioning_ctx(
                            format!(
                                "Azure quota exceeded: \
                                     {stderr}"
                            ),
                            ErrorContext::new().insert("vm_name", &vm_name),
                        ));
                    }
                    if attempt < max_retries - 1 {
                        warn!(attempt = attempt + 1, "provisioning failed, retrying");
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        continue;
                    }
                    return Err(RemoteError::provisioning_ctx(
                        format!(
                            "Failed to provision VM: \
                                 {stderr}"
                        ),
                        ErrorContext::new().insert("vm_name", &vm_name),
                    ));
                }
                Ok(Err(e)) => {
                    if attempt < max_retries - 1 {
                        warn!(
                            error = %e,
                            "provisioning error, retrying"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        continue;
                    }
                    return Err(RemoteError::provisioning(format!(
                        "Provisioning command failed: {e}"
                    )));
                }
                Err(_) => {
                    if attempt < max_retries - 1 {
                        warn!("provisioning timeout, retrying");
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        continue;
                    }
                    return Err(RemoteError::provisioning_ctx(
                        format!(
                            "VM provisioning timed out \
                                 after {max_retries} attempts"
                        ),
                        ErrorContext::new().insert("vm_name", &vm_name),
                    ));
                }
            }
        }

        Err(RemoteError::provisioning_ctx(
            format!(
                "Failed to provision VM after \
                 {max_retries} attempts"
            ),
            ErrorContext::new().insert("vm_name", &vm_name),
        ))
    }

    async fn get_vm_by_name(&self, vm_name: &str) -> Result<VM, RemoteError> {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            Command::new("azlin")
                .arg("list")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;

        if let Ok(Ok(o)) = output {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stdout.contains(vm_name) {
                return Ok(VM {
                    name: vm_name.to_string(),
                    size: "unknown".into(),
                    region: "unknown".into(),
                    created_at: None,
                    tags: None,
                });
            }
        }

        Err(RemoteError::provisioning_ctx(
            format!("VM not found: {vm_name}"),
            ErrorContext::new().insert("vm_name", vm_name),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_age_without_created_at() {
        let vm = VM {
            name: "test".into(),
            size: "s".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        };
        assert_eq!(vm.age_hours(), 0.0);
    }

    #[test]
    fn vm_options_default() {
        let opts = VMOptions::default();
        assert_eq!(opts.size, "Standard_D2s_v3");
        assert!(!opts.no_reuse);
        assert!(!opts.keep_vm);
    }

    #[test]
    fn vm_serialization() {
        let vm = VM {
            name: "test-vm".into(),
            size: "Standard_D2s_v3".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        };
        let json = serde_json::to_string(&vm).unwrap();
        let vm2: VM = serde_json::from_str(&json).unwrap();
        assert_eq!(vm2.name, "test-vm");
    }
}
