//! VM lookup helpers backed by the `azlin list` CLI.
//!
//! Selecting a VM by name uses the authoritative, exact structured `name`
//! field parsed from the CLI output — never a fragile substring match — so a
//! query for a name that is a substring of another VM's name (e.g. "vm1" vs
//! "vm10") never yields a false-positive.

use std::process::Stdio;

use tokio::process::Command;
use tracing::debug;

use crate::azlin_parse::{parse_azlin_list_json, parse_azlin_list_text};
use crate::error::{ErrorContext, RemoteError};
use crate::orchestrator::VM;
use crate::redact::redact_sensitive;

/// Look up a single VM by its exact name via `azlin list`.
pub(crate) async fn get_vm_by_name(vm_name: &str) -> Result<VM, RemoteError> {
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
        Ok(Ok(o)) => {
            // JSON invocation ran but failed; fall back to text listing.
            let stderr = String::from_utf8_lossy(&o.stderr);
            debug!(stderr = %redact_sensitive(&stderr), "azlin list --json failed, trying text");
            list_vms_text().await?
        }
        Ok(Err(e)) => {
            return Err(RemoteError::provisioning_ctx(
                format!("Failed to run 'azlin list': {e}"),
                ErrorContext::new().insert("vm_name", vm_name),
            ));
        }
        Err(_) => {
            return Err(RemoteError::provisioning_ctx(
                "'azlin list' timed out",
                ErrorContext::new().insert("vm_name", vm_name),
            ));
        }
    };

    select_vm_by_name(&vms, vm_name).cloned().ok_or_else(|| {
        RemoteError::provisioning_ctx(
            format!("VM not found: {vm_name}"),
            ErrorContext::new().insert("vm_name", vm_name),
        )
    })
}

/// Fetch the VM list via the plain-text `azlin list` output.
async fn list_vms_text() -> Result<Vec<VM>, RemoteError> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        Command::new("azlin")
            .arg("list")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;

    match output {
        Ok(Ok(o)) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            Ok(parse_azlin_list_text(&stdout))
        }
        Ok(Ok(o)) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(RemoteError::provisioning(format!(
                "'azlin list' failed: {}",
                redact_sensitive(&stderr)
            )))
        }
        Ok(Err(e)) => Err(RemoteError::provisioning(format!(
            "Failed to run 'azlin list': {e}"
        ))),
        Err(_) => Err(RemoteError::provisioning("'azlin list' timed out")),
    }
}

/// Select a VM by its exact structured name.
///
/// Uses an exact equality comparison on the parsed `name` field so that a
/// query for a name that is a substring of another VM's name never yields a
/// false-positive match.
fn select_vm_by_name<'a>(vms: &'a [VM], name: &str) -> Option<&'a VM> {
    vms.iter().find(|vm| vm.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm_named(name: &str) -> VM {
        VM {
            name: name.into(),
            size: "Standard_D2s_v3".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        }
    }

    #[test]
    fn select_vm_by_name_exact_match() {
        let vms = vec![vm_named("vm1"), vm_named("vm10"), vm_named("vm100")];
        let found = select_vm_by_name(&vms, "vm10").expect("vm10 exists");
        assert_eq!(found.name, "vm10");
    }

    #[test]
    fn select_vm_by_name_rejects_substring_false_positive() {
        // "vm1" is a substring of "vm10"/"vm100"; exact matching must NOT
        // select either of those when only "vm10" and "vm100" are present.
        let vms = vec![vm_named("vm10"), vm_named("vm100")];
        assert!(
            select_vm_by_name(&vms, "vm1").is_none(),
            "substring must not yield a false-positive match"
        );

        // And when the exact name is present alongside its superstrings, the
        // exact one is selected.
        let vms = vec![vm_named("vm100"), vm_named("vm1"), vm_named("vm10")];
        let found = select_vm_by_name(&vms, "vm1").expect("exact vm1 exists");
        assert_eq!(found.name, "vm1");
    }

    #[test]
    fn select_vm_by_name_returns_real_struct_not_placeholder() {
        let vms = vec![VM {
            name: "amplihack-user-1".into(),
            size: "Standard_D4s_v3".into(),
            region: "westus".into(),
            created_at: None,
            tags: None,
        }];
        let found = select_vm_by_name(&vms, "amplihack-user-1").unwrap();
        assert_eq!(found.size, "Standard_D4s_v3");
        assert_eq!(found.region, "westus");
    }
}
