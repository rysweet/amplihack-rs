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
    resolve_vm_list, vm_names_from_az_vm_list_json, vm_names_from_azlin_json,
};

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
