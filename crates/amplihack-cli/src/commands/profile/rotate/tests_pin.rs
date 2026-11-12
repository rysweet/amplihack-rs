//! `rotate::pin`: the rotation pin written by `profile switch NAME` and
//! removed by `profile switch --none` (issue #1691, D1-D3). Uses the fixture
//! in `tests_swap.rs`; nothing here touches a real root or `~/.claude`.
//!
//! Contract:
//! - `RotateState::pin` is `<root>/.auto-switch.d/rotate/pin`.
//! - `pointer::parse_name(&[u8]) -> Option<ProfileName>`: UTF-8, at most one
//!   trailing newline, a valid name. Shared by `pointer::read` and `pin::read`.
//! - `PinState::{None, Pinned(ProfileName), Missing(ProfileName), Unreadable}`.
//! - `pin::read(&RotateState) -> PinState` never fails and never keeps the
//!   raw bytes: a symlink, a non-file, over 256 bytes, someone else's, group-
//!   or world-writable, or not a name is `Unreadable`; a name with no
//!   directory under the root is `Missing`.
//! - `pin::write(&RotateState, &ProfileName) -> Result<(), ProfileError>`:
//!   atomic, mode 0600, creates the state directories.
//! - `pin::clear(&RotateState) -> Result<bool, ProfileError>`: `lstat` first;
//!   removes a file, a symlink (not its target) or an empty directory;
//!   refuses anything else with `RotateRefused`; `true` when it removed one.
//! - `Gate::{Open, Ignored(String), Held(String)}` and the pure
//!   `pin::gate(&PinState) -> Gate`, whose strings are the `msg::` lines.

use super::cycle::CycleOutcome;
use super::ledger::RotateState;
use super::log::msg;
use super::pin::{self, Gate, PinState};
use super::tests_swap::{env, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::euid;
use crate::commands::profile::pointer::parse_name;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

/// Writes raw bytes at the pin path, mode 0600, as a person or a bug might.
fn raw_pin(rs: &RotateState, bytes: &[u8]) {
    rs.ensure().unwrap();
    fs::write(&rs.pin, bytes).unwrap();
    fs::set_permissions(&rs.pin, fs::Permissions::from_mode(0o600)).unwrap();
}

// --- The shared name parser -------------------------------------------------

#[test]
fn parse_name_accepts_a_name_with_at_most_one_newline() {
    assert_eq!(parse_name(b"work"), Some(n("work")));
    assert_eq!(parse_name(b"work\n"), Some(n("work")));
    for bad in [
        &b""[..],
        b"\n",
        b"work\n\n",
        b"../outside",
        b"a/b",
        b"work\nspare\n",
        b"\xff\xfe",
    ] {
        assert_eq!(parse_name(bad), None, "{bad:?}");
    }
}

// --- The state path ---------------------------------------------------------

#[test]
fn the_pin_lives_in_the_rotate_state_directory() {
    let e = env();
    assert_eq!(e.rs.pin, e.rs.dir.join("pin"));
    assert_eq!(
        e.rs.pin,
        e.root.join(".auto-switch.d").join("rotate").join("pin")
    );
}

// --- read and write ---------------------------------------------------------

#[test]
fn a_missing_pin_is_none() {
    let e = env();
    assert_eq!(pin::read(&e.rs), PinState::None);
}

#[test]
fn a_written_pin_reads_back_private_and_holding_only_the_name() {
    let e = env();
    pin::write(&e.rs, &n("spare")).unwrap();
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));
    let m = fs::symlink_metadata(&e.rs.pin).unwrap();
    assert!(m.is_file());
    assert_eq!(m.mode() & 0o777, 0o600);
    assert_eq!(m.uid(), euid());
    assert_eq!(fs::read(&e.rs.pin).unwrap(), b"spare\n");
    let leftovers: Vec<_> = fs::read_dir(&e.rs.dir)
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|f| f.contains("tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");
}

#[test]
fn writing_again_moves_the_pin() {
    let e = env();
    pin::write(&e.rs, &n("spare")).unwrap();
    pin::write(&e.rs, &n("work")).unwrap();
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("work")));
}

#[test]
fn write_creates_the_state_directories() {
    let e = env();
    fs::remove_dir_all(e.root.join(".auto-switch.d")).unwrap();
    pin::write(&e.rs, &n("spare")).unwrap();
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));
    let m = fs::symlink_metadata(&e.rs.dir).unwrap();
    assert_eq!(m.mode() & 0o777, 0o700);
}

#[test]
fn write_replaces_a_symlink_without_touching_its_target() {
    let e = env();
    e.rs.ensure().unwrap();
    let target = e.root.join("target-file");
    fs::write(&target, "keep me").unwrap();
    std::os::unix::fs::symlink(&target, &e.rs.pin).unwrap();
    pin::write(&e.rs, &n("spare")).unwrap();
    assert!(fs::symlink_metadata(&e.rs.pin).unwrap().is_file());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));
}

#[test]
fn a_pin_naming_a_removed_profile_is_missing() {
    let e = env();
    e.store.add(&n("gone")).unwrap();
    pin::write(&e.rs, &n("gone")).unwrap();
    fs::remove_dir_all(e.root.join("gone")).unwrap();
    assert_eq!(pin::read(&e.rs), PinState::Missing(n("gone")));
    assert!(e.rs.pin.exists(), "reading never removes the pin");
}

#[test]
fn a_pin_naming_a_file_or_symlink_instead_of_a_profile_dir_is_missing() {
    let e = env();
    fs::write(e.root.join("plainfile"), "x").unwrap();
    raw_pin(&e.rs, b"plainfile\n");
    assert_eq!(pin::read(&e.rs), PinState::Missing(n("plainfile")));
    std::os::unix::fs::symlink(e.root.join("spare"), e.root.join("linked")).unwrap();
    raw_pin(&e.rs, b"linked\n");
    assert_eq!(pin::read(&e.rs), PinState::Missing(n("linked")));
}

#[test]
fn garbage_and_oversized_pins_are_unreadable() {
    let e = env();
    for body in [
        &b"../outside\n"[..],
        b"",
        b"work\n\n",
        b"\xff\xfe\n",
        b"has space\n",
    ] {
        raw_pin(&e.rs, body);
        assert_eq!(pin::read(&e.rs), PinState::Unreadable, "{body:?}");
    }
    raw_pin(&e.rs, "a".repeat(300).as_bytes());
    assert_eq!(pin::read(&e.rs), PinState::Unreadable, "over 256 bytes");
}

#[test]
fn a_symlinked_pin_is_unreadable_even_to_a_valid_name() {
    let e = env();
    e.rs.ensure().unwrap();
    let elsewhere = e.root.join("elsewhere");
    fs::write(&elsewhere, "spare\n").unwrap();
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &e.rs.pin).unwrap();
    assert_eq!(pin::read(&e.rs), PinState::Unreadable);
}

#[test]
fn a_directory_at_the_pin_path_is_unreadable() {
    let e = env();
    e.rs.ensure().unwrap();
    fs::create_dir(&e.rs.pin).unwrap();
    assert_eq!(pin::read(&e.rs), PinState::Unreadable);
}

#[test]
fn a_pin_others_can_write_is_unreadable() {
    let e = env();
    for mode in [0o620, 0o602, 0o666] {
        raw_pin(&e.rs, b"spare\n");
        fs::set_permissions(&e.rs.pin, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(pin::read(&e.rs), PinState::Unreadable, "mode {mode:o}");
    }
}

#[test]
fn a_pin_owned_by_someone_else_is_unreadable() {
    if euid() != 0 {
        return; // only root can give a file away
    }
    let e = env();
    raw_pin(&e.rs, b"spare\n");
    std::os::unix::fs::chown(&e.rs.pin, Some(65534), None).unwrap();
    assert_eq!(pin::read(&e.rs), PinState::Unreadable);
}

#[test]
fn an_unreadable_pin_never_echoes_its_bytes() {
    let e = env();
    raw_pin(&e.rs, b"\x1b[31mTOKEN-sk-ant-abc123\x07\n");
    let state = pin::read(&e.rs);
    assert_eq!(state, PinState::Unreadable);
    let Gate::Held(line) = pin::gate(&state) else {
        panic!("an unreadable pin holds");
    };
    for shown in [format!("{state:?}"), line.clone()] {
        assert!(!shown.contains("TOKEN"), "{shown}");
        assert!(!shown.contains('\x1b'), "{shown:?}");
    }
    assert_eq!(line, msg::PIN_UNREADABLE);
}

// --- The gate ---------------------------------------------------------------

#[test]
fn the_gate_holds_for_a_pin_and_for_an_unreadable_one() {
    assert_eq!(pin::gate(&PinState::None), Gate::Open);
    assert_eq!(
        pin::gate(&PinState::Pinned(n("spare"))),
        Gate::Held(msg::held(&n("spare")))
    );
    assert_eq!(
        pin::gate(&PinState::Unreadable),
        Gate::Held(msg::PIN_UNREADABLE.to_string())
    );
    assert_eq!(
        pin::gate(&PinState::Missing(n("gone"))),
        Gate::Ignored(msg::pin_missing(&n("gone")))
    );
}

#[test]
fn the_pin_lines_are_the_documented_ones() {
    assert_eq!(
        msg::held(&n("spare")),
        "skip: held by switch to spare; run `amplihack profile switch --none` to release"
    );
    assert_eq!(
        msg::PIN_UNREADABLE,
        "skip: pin unreadable; run `amplihack profile switch --none`"
    );
    assert_eq!(
        msg::pin_missing(&n("gone")),
        "skip: pin names missing profile gone; ignored"
    );
    assert_eq!(msg::switched(&n("work"), &n("spare")), "switch work->spare");
    assert_eq!(
        msg::switched_already_live(&n("spare")),
        "skip: spare is already the live login; nothing moved"
    );
    assert!(
        !msg::switched(&n("work"), &n("spare")).starts_with("swap "),
        "a search for rotator swaps must not match a switch"
    );
}

#[test]
fn a_held_cycle_is_recorded_as_held() {
    assert_eq!(CycleOutcome::Held.as_str(), "held");
}

// --- clear ------------------------------------------------------------------

#[test]
fn clearing_reports_whether_a_pin_was_there() {
    let e = env();
    assert!(!pin::clear(&e.rs).unwrap(), "nothing to clear");
    pin::write(&e.rs, &n("spare")).unwrap();
    assert!(pin::clear(&e.rs).unwrap());
    assert!(fs::symlink_metadata(&e.rs.pin).is_err());
    assert_eq!(pin::read(&e.rs), PinState::None);
    assert!(!pin::clear(&e.rs).unwrap(), "idempotent");
}

#[test]
fn clearing_an_unreadable_pin_removes_it() {
    let e = env();
    raw_pin(&e.rs, b"\x00garbage");
    fs::set_permissions(&e.rs.pin, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(pin::clear(&e.rs).unwrap());
    assert_eq!(pin::read(&e.rs), PinState::None);
}

#[test]
fn clearing_a_symlink_removes_the_link_not_its_target() {
    let e = env();
    e.rs.ensure().unwrap();
    let target = e.root.join("spare").join("keep.txt");
    fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, &e.rs.pin).unwrap();
    assert!(pin::clear(&e.rs).unwrap());
    assert!(fs::symlink_metadata(&e.rs.pin).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep");

    // A symlink to a directory: the directory and its contents stay.
    std::os::unix::fs::symlink(e.root.join("spare"), &e.rs.pin).unwrap();
    assert!(pin::clear(&e.rs).unwrap());
    assert!(e.root.join("spare").is_dir());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
}

#[test]
fn clearing_an_empty_directory_removes_it() {
    let e = env();
    e.rs.ensure().unwrap();
    fs::create_dir(&e.rs.pin).unwrap();
    assert!(pin::clear(&e.rs).unwrap());
    assert!(fs::symlink_metadata(&e.rs.pin).is_err());
}

#[test]
fn clearing_a_non_empty_directory_is_refused_and_deletes_nothing() {
    let e = env();
    e.rs.ensure().unwrap();
    fs::create_dir(&e.rs.pin).unwrap();
    fs::write(e.rs.pin.join("inside"), "data").unwrap();
    match pin::clear(&e.rs) {
        Err(ProfileError::RotateRefused(m)) => {
            assert!(m.contains("is not a file; remove it by hand"), "{m}");
            assert!(m.contains("pin"), "names the path: {m}");
        }
        other => panic!("expected RotateRefused, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(e.rs.pin.join("inside")).unwrap(), "data");
}
