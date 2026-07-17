//! Unit tests for the `vm_pool` module.
//!
//! Kept in a sibling file (wired via `#[path]` from `vm_pool.rs`) so the
//! parent module stays within the issue #536 <=500-line module budget
//! while remaining a child module with access to private helpers such as
//! `VMPoolManager::apply_cleanup_result`.

use super::*;

#[test]
fn vm_size_capacity() {
    assert_eq!(VMSize::S.capacity(), 1);
    assert_eq!(VMSize::M.capacity(), 2);
    assert_eq!(VMSize::L.capacity(), 4);
    assert_eq!(VMSize::XL.capacity(), 8);
}

#[test]
fn vm_size_azure_mapping() {
    assert_eq!(VMSize::S.azure_size(), "Standard_D8s_v3");
    assert_eq!(VMSize::XL.azure_size(), "Standard_E32s_v5");
}

#[test]
fn pool_entry_available_capacity() {
    let entry = VMPoolEntry {
        vm: VM {
            name: "vm1".into(),
            size: "s".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        },
        capacity: 4,
        active_sessions: vec!["s1".into(), "s2".into()],
        region: "eastus".into(),
    };
    assert_eq!(entry.available_capacity(), 2);
}

#[test]
fn pool_status_serialization() {
    let status = PoolStatus {
        total_vms: 2,
        total_capacity: 8,
        active_sessions: 3,
        available_capacity: 5,
    };
    let json = serde_json::to_string(&status).unwrap();
    let s2: PoolStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(s2.total_vms, 2);
}

#[test]
fn vm_size_display() {
    assert_eq!(VMSize::S.to_string(), "s");
    assert_eq!(VMSize::M.to_string(), "m");
    assert_eq!(VMSize::L.to_string(), "l");
    assert_eq!(VMSize::XL.to_string(), "xl");
}

#[test]
fn vm_size_from_str() {
    assert_eq!("s".parse::<VMSize>().unwrap(), VMSize::S);
    assert_eq!("S".parse::<VMSize>().unwrap(), VMSize::S);
    assert_eq!("m".parse::<VMSize>().unwrap(), VMSize::M);
    assert_eq!("M".parse::<VMSize>().unwrap(), VMSize::M);
    assert_eq!("l".parse::<VMSize>().unwrap(), VMSize::L);
    assert_eq!("L".parse::<VMSize>().unwrap(), VMSize::L);
    assert_eq!("xl".parse::<VMSize>().unwrap(), VMSize::XL);
    assert_eq!("XL".parse::<VMSize>().unwrap(), VMSize::XL);
}

#[test]
fn vm_size_from_str_invalid() {
    assert!("xxl".parse::<VMSize>().is_err());
    assert!("".parse::<VMSize>().is_err());
    assert!("large".parse::<VMSize>().is_err());
}

#[test]
fn vm_size_azure_mapping_all() {
    assert_eq!(VMSize::S.azure_size(), "Standard_D8s_v3");
    assert_eq!(VMSize::M.azure_size(), "Standard_E8s_v5");
    assert_eq!(VMSize::L.azure_size(), "Standard_E16s_v5");
    assert_eq!(VMSize::XL.azure_size(), "Standard_E32s_v5");
}

#[test]
fn vm_size_serialization() {
    let size = VMSize::L;
    let json = serde_json::to_string(&size).unwrap();
    let s2: VMSize = serde_json::from_str(&json).unwrap();
    assert_eq!(s2, VMSize::L);
}

#[test]
fn pool_entry_zero_capacity() {
    let entry = VMPoolEntry {
        vm: VM {
            name: "vm1".into(),
            size: "s".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        },
        capacity: 2,
        active_sessions: vec!["s1".into(), "s2".into()],
        region: "eastus".into(),
    };
    assert_eq!(entry.available_capacity(), 0);
}

#[test]
fn pool_entry_overflow_sessions() {
    let entry = VMPoolEntry {
        vm: VM {
            name: "vm1".into(),
            size: "s".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        },
        capacity: 1,
        active_sessions: vec!["s1".into(), "s2".into(), "s3".into()],
        region: "eastus".into(),
    };
    // saturating_sub prevents underflow
    assert_eq!(entry.available_capacity(), 0);
}

#[test]
fn pool_entry_full_capacity() {
    let entry = VMPoolEntry {
        vm: VM {
            name: "vm1".into(),
            size: "s".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        },
        capacity: 4,
        active_sessions: vec![],
        region: "eastus".into(),
    };
    assert_eq!(entry.available_capacity(), 4);
}

#[test]
fn pool_entry_serialization() {
    let entry = VMPoolEntry {
        vm: VM {
            name: "vm1".into(),
            size: "Standard_D8s_v3".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        },
        capacity: 4,
        active_sessions: vec!["s1".into()],
        region: "eastus".into(),
    };
    let json = serde_json::to_string(&entry).unwrap();
    let e2: VMPoolEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(e2.vm.name, "vm1");
    assert_eq!(e2.capacity, 4);
    assert_eq!(e2.active_sessions.len(), 1);
}

#[test]
fn dirs_home_returns_path() {
    let home = dirs_home();
    assert!(!home.to_str().unwrap_or("").is_empty());
}

// ---- issue #870: cleanup result must not be silently discarded ----
//
// These tests pin the contract for `VMPoolManager::apply_cleanup_result`,
// the pure helper that maps one `Orchestrator::cleanup` outcome onto the
// pool + `removed` list. Only a confirmed reclaim (`Ok(true)`) may drop a
// VM from tracking; every other outcome must retain the VM so a billable
// cloud resource is never orphaned by a swallowed failure.

fn make_entry(name: &str) -> VMPoolEntry {
    VMPoolEntry {
        vm: VM {
            name: name.into(),
            size: "s".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        },
        capacity: 1,
        active_sessions: vec![],
        region: "eastus".into(),
    }
}

#[test]
fn apply_cleanup_result_ok_true_removes() {
    // Ok(true) => deallocation confirmed: VM leaves the pool and is
    // recorded in `removed` (the only truthful "reclaimed" signal).
    let mut pool: HashMap<String, VMPoolEntry> = HashMap::new();
    let mut removed: Vec<String> = Vec::new();
    let vm_name = "vm-confirmed".to_string();
    let entry = make_entry(&vm_name);

    VMPoolManager::apply_cleanup_result(&mut pool, &mut removed, vm_name.clone(), entry, Ok(true));

    assert!(
        !pool.contains_key(&vm_name),
        "confirmed-reclaimed VM must not be retained in the pool"
    );
    assert!(
        removed.contains(&vm_name),
        "confirmed-reclaimed VM must be recorded in `removed`"
    );
}

#[test]
fn apply_cleanup_result_ok_false_retains() {
    // Ok(false) => cleanup ran but did not confirm deallocation: retain the
    // VM for a later retry and do NOT claim it was removed.
    let mut pool: HashMap<String, VMPoolEntry> = HashMap::new();
    let mut removed: Vec<String> = Vec::new();
    let vm_name = "vm-unconfirmed".to_string();
    let entry = make_entry(&vm_name);

    VMPoolManager::apply_cleanup_result(&mut pool, &mut removed, vm_name.clone(), entry, Ok(false));

    assert!(
        pool.contains_key(&vm_name),
        "unconfirmed cleanup must retain the VM in the pool for retry"
    );
    assert!(
        !removed.contains(&vm_name),
        "unconfirmed cleanup must not be reported as removed"
    );
}

#[test]
fn apply_cleanup_result_err_retains() {
    // Err(_) => hard cleanup failure: retain the VM so the billable
    // resource is never orphaned, and do NOT claim it was removed.
    let mut pool: HashMap<String, VMPoolEntry> = HashMap::new();
    let mut removed: Vec<String> = Vec::new();
    let vm_name = "vm-failed".to_string();
    let entry = make_entry(&vm_name);

    VMPoolManager::apply_cleanup_result(
        &mut pool,
        &mut removed,
        vm_name.clone(),
        entry,
        Err(RemoteError::cleanup("azlin deallocate failed")),
    );

    assert!(
        pool.contains_key(&vm_name),
        "failed cleanup must retain the VM in the pool"
    );
    assert!(
        !removed.contains(&vm_name),
        "failed cleanup must not be reported as removed"
    );
}
