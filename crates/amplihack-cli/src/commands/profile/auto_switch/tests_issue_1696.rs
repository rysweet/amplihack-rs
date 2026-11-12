//! Portable unit contracts for the shared policy and owned launch protection.
//! Keep these on the existing seams while the typed destination is introduced.

use super::candidate::{self, Pool};
use super::state::{HostEntry, Ledger, State};
use super::{Action, guard_launch, swap};
use crate::commands::profile::{ProfileError, name::ProfileName, store::ProfileStore};
use fs4::fs_std::FileExt;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn name(raw: &str) -> ProfileName {
    ProfileName::parse(raw).unwrap()
}

fn hosted() -> Ledger {
    Ledger {
        version: 1,
        hosts: BTreeMap::from([(
            "X".into(),
            HostEntry {
                live: "Y".into(),
                swapped_at: 1,
                pre_swap_fingerprint: None,
            },
        )]),
    }
}

fn fixture() -> (tempfile::TempDir, ProfileStore, State) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profiles");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let store = ProfileStore::new(root.clone()).with_seed_hooks(Err("fixture".into()));
    let state = State::new(&root).unwrap();
    state.ensure().unwrap();
    (tmp, store, state)
}

#[test]
fn hosting_conflict_retains_both_profile_identities() {
    let (_tmp, _store, state) = fixture();
    let error = swap::check_lease(
        &state,
        &hosted(),
        &name("X"),
        Action::Launch { join: false },
    )
    .unwrap_err();
    assert!(
        matches!(error, ProfileError::HostsForeignAccount { profile, live } if profile == "X" && live == "Y")
    );
}

#[test]
fn lent_login_never_becomes_a_destination_even_with_join_permission() {
    let (_tmp, _store, state) = fixture();
    for join in [false, true] {
        let error =
            swap::check_lease(&state, &hosted(), &name("Y"), Action::Launch { join }).unwrap_err();
        assert!(
            matches!(error, ProfileError::Leased { profile, host } if profile == "Y" && host == "X")
        );
    }
}

#[test]
fn hosted_directory_requires_existing_session_join_permission() {
    let (_tmp, _store, state) = fixture();
    assert!(
        swap::check_lease(&state, &hosted(), &name("X"), Action::Launch { join: true }).is_ok()
    );
    for action in [
        Action::Launch { join: false },
        Action::Login,
        Action::Remove,
    ] {
        assert!(matches!(
            swap::check_lease(&state, &hosted(), &name("X"), action),
            Err(ProfileError::HostsForeignAccount { .. })
        ));
    }
}

#[test]
fn exclusions_are_applied_before_candidate_credential_checks() {
    let profiles = [name("X"), name("Y"), name("Z")];
    let readings = BTreeMap::new();
    let exclude = [name("Y")];
    let calls = RefCell::new(Vec::new());
    let pool = Pool {
        profiles: &profiles,
        readings: &readings,
        threshold: 90,
        exclude: &exclude,
        now: 1,
    };
    let selected = candidate::select(&pool, "X", |profile| {
        calls.borrow_mut().push(profile.as_str().to_string());
        true
    });
    assert_eq!(selected, Some(&name("Z")));
    assert_eq!(
        *calls.borrow(),
        ["Z"],
        "excluded credentials must never be inspected or refreshed"
    );
}

#[test]
fn launch_hold_retains_state_protection_until_child_creation_finishes() {
    let (_tmp, store, state) = fixture();
    let hold = guard_launch(&store, &name("Z"), false).unwrap();
    let independent = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(state.dir.join("lock"))
        .unwrap();
    assert!(
        !independent.try_lock_exclusive().unwrap(),
        "protection released before spawn"
    );
    drop(hold);
    assert!(
        independent.try_lock_exclusive().unwrap(),
        "protection leaked after spawn"
    );
}

#[test]
fn invalid_state_releases_launch_protection_on_error() {
    let (_tmp, store, state) = fixture();
    let path = state.dir.join("ledger.json");
    fs::write(&path, "not json").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(guard_launch(&store, &name("Z"), false).is_err());
    let independent = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(state.dir.join("lock"))
        .unwrap();
    assert!(
        independent.try_lock_exclusive().unwrap(),
        "error path leaked launch protection"
    );
}

#[test]
fn launch_hold_releases_its_refresh_lock_without_breaking_an_existing_lock() {
    let (_tmp, store, _state) = fixture();
    let dir = store.root().join("Z");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let lock = dir.join(".oauth_refresh.lock");
    let mut hold = guard_launch(&store, &name("Z"), false).unwrap();
    hold.protect_refresh(&dir).unwrap();
    assert!(lock.is_dir());
    drop(hold);
    assert!(!lock.exists());
    fs::create_dir(&lock).unwrap();
    let ino = std::os::unix::fs::MetadataExt::ino(&fs::metadata(&lock).unwrap());
    let mut hold = guard_launch(&store, &name("Z"), false).unwrap();
    assert!(hold.protect_refresh(&dir).is_err());
    drop(hold);
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&fs::metadata(&lock).unwrap()),
        ino
    );
}

#[test]
fn supervised_contexts_use_the_existing_owner_based_scratch_cleanup() {
    let (_tmp, _store, state) = fixture();
    let scratch = state.dir.join("rerun");
    state.ensure_sub(&scratch).unwrap();
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let ended = child.id();
    child.wait().unwrap();
    let dead = scratch.join(format!("{ended}-1.launch.json"));
    let live = scratch.join(format!("{}-1.launch.json", std::process::id()));
    fs::write(&dead, "{}").unwrap();
    fs::write(&live, "{}").unwrap();
    super::sweep_rerun(&scratch);
    assert!(!dead.exists());
    assert!(live.exists());
}

#[test]
fn launch_state_protection_refuses_contention_without_waiting_for_release() {
    let (_temp, _store, state) = fixture();
    let held = state.lock().unwrap();
    assert!(state.launch_lock().is_err());
    drop(held);
    assert!(state.launch_lock().is_ok());
}
