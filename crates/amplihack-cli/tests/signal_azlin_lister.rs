//! TDD (RED): VM discovery — azlin-first with generic `az vm list` fallback,
//! never ia2-hardcoded (D3/D4/#923).
//!
//! Contract — `amplihack_cli::commands::signal::seams`:
//!
//! * `vm_names_from_azlin_json` / `vm_names_from_az_vm_list_json` are pure name
//!   extractors reusing the real azlin/az JSON shapes.
//! * `resolve_vm_list(azlin, az_fallback)` uses azlin's result when it is a
//!   non-empty `Ok`; otherwise (empty or `Err`) it calls the generic
//!   `az vm list` fallback. It never hardcodes a resource group.
//! * The injected `VmLister` seam (exercised in `signal_setup_idempotency`)
//!   returns `anyhow::Result`, so the discovery combinator mirrors that: a
//!   discovery failure must SURFACE, never degrade to a silent empty list.
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::seams::{
    AzVmLister, VmLister, resolve_vm_list, vm_names_from_az_vm_list_json, vm_names_from_azlin_json,
};

#[cfg(unix)]
use crate::support::exec_stub::write_executable;
#[cfg(unix)]
use crate::support::quiet_process::{child_set_var, in_quiet_process};

#[test]
fn extracts_names_from_azlin_json() {
    let json = r#"[{"name":"vm-a","size":"s","region":"eastus"},
                   {"name":"vm-b","size":"m","region":"westus"}]"#;
    assert_eq!(vm_names_from_azlin_json(json), vec!["vm-a", "vm-b"]);
}

#[test]
fn extracts_names_from_generic_az_vm_list_json() {
    // `az vm list --output json` shape: array of objects with a `name` field.
    let json = r#"[{"name":"prod-1","resourceGroup":"rg","location":"eastus"},
                   {"name":"prod-2","resourceGroup":"rg","location":"eastus"}]"#;
    assert_eq!(
        vm_names_from_az_vm_list_json(json),
        vec!["prod-1", "prod-2"]
    );
}

#[test]
fn malformed_json_yields_no_vms() {
    assert!(vm_names_from_azlin_json("not json").is_empty());
    assert!(vm_names_from_az_vm_list_json("{}").is_empty());
}

#[test]
fn azlin_result_is_used_when_non_empty() {
    let got = resolve_vm_list(Ok(vec!["a".into(), "b".into()]), || {
        panic!("fallback must not run when azlin succeeds")
    })
    .unwrap();
    assert_eq!(got, vec!["a", "b"]);
}

#[test]
fn empty_azlin_triggers_az_fallback() {
    let got = resolve_vm_list(Ok(vec![]), || Ok(vec!["fallback-vm".into()])).unwrap();
    assert_eq!(got, vec!["fallback-vm"]);
}

#[test]
fn azlin_error_triggers_az_fallback() {
    let got = resolve_vm_list(Err(anyhow::anyhow!("azlin missing")), || {
        Ok(vec!["fallback-vm".into()])
    })
    .unwrap();
    assert_eq!(got, vec!["fallback-vm"]);
}

#[test]
fn fallback_error_surfaces_never_a_silent_empty_list() {
    // Both sources unavailable: discovery must error, not return an empty fleet.
    let res = resolve_vm_list(Ok(vec![]), || {
        anyhow::bail!("az vm list failed: not logged in")
    });
    assert!(
        res.is_err(),
        "a total discovery failure must surface as an error"
    );
}

/// `PATH` with the stub directory first. `AzVmLister` resolves `azlin` and
/// `az` through the process `PATH`, so the tests set it in-process, inside a
/// quiet child (`in_quiet_process`) where no other test runs.
#[cfg(unix)]
fn stub_path(bin: &std::path::Path) -> String {
    let old = std::env::var_os("PATH");
    format!(
        "{}:{}",
        bin.display(),
        old.as_ref()
            .map(|p| p.to_string_lossy())
            .unwrap_or_default()
    )
}

#[cfg(unix)]
#[test]
fn production_lister_uses_azlin_before_az_when_azlin_has_names() {
    in_quiet_process(
        module_path!(),
        "production_lister_uses_azlin_before_az_when_azlin_has_names",
        || {
            let root = std::env::current_dir()
                .unwrap()
                .join("target")
                .join("signal-azlin-first-test");
            let bin = root.join("bin");
            let marker = root.join("az-called");
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&bin).unwrap();
            write_executable(
                &bin.join("azlin"),
                "#!/usr/bin/env bash\nprintf '[{\"name\":\"vm-from-azlin\"}]\\n'\n",
                0o755,
            );
            write_executable(
                &bin.join("az"),
                format!(
                    "#!/usr/bin/env bash\nprintf called > '{}'\nprintf '[{{\"name\":\"vm-from-az\"}}]\\n'\n",
                    marker.display()
                ),
                0o755,
            );
            child_set_var("PATH", stub_path(&bin));

            let names = AzVmLister.list_vms("rg-test").unwrap();
            let az_called = marker.exists();

            let _ = std::fs::remove_dir_all(&root);
            assert_eq!(names, vec!["vm-from-azlin"]);
            assert!(!az_called, "`az vm list` must not run when azlin succeeds");
        },
    );
}

#[cfg(unix)]
#[test]
fn production_lister_falls_back_to_az_when_azlin_is_empty() {
    in_quiet_process(
        module_path!(),
        "production_lister_falls_back_to_az_when_azlin_is_empty",
        || {
            let root = std::env::current_dir()
                .unwrap()
                .join("target")
                .join("signal-az-fallback-test");
            let bin = root.join("bin");
            let marker = root.join("az-called");
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&bin).unwrap();
            write_executable(
                &bin.join("azlin"),
                "#!/usr/bin/env bash\nprintf '[]\\n'\n",
                0o755,
            );
            write_executable(
                &bin.join("az"),
                format!(
                    "#!/usr/bin/env bash\nprintf called > '{}'\nprintf '[{{\"name\":\"vm-from-az\"}}]\\n'\n",
                    marker.display()
                ),
                0o755,
            );
            child_set_var("PATH", stub_path(&bin));

            let names = AzVmLister.list_vms("rg-test").unwrap();
            let az_called = marker.exists();

            let _ = std::fs::remove_dir_all(&root);
            assert_eq!(names, vec!["vm-from-az"]);
            assert!(az_called, "`az vm list` must run after empty azlin");
        },
    );
}
