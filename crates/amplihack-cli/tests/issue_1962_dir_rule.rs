//! Issue #1962, D1: the login directory rule, through the binary. Group write is
//! accepted only in the user's own private group with no ACL. These tests hold on
//! a host with private groups and on one with shared groups, because the expected
//! answer comes from the same facts the rule reads.

#![cfg(unix)]

use crate::support::profile_1962::{World, group_is_private, printed};
use crate::support::profile_rotate::text;
use std::path::Path;
use std::process::Command;

/// Adds an ACL entry that sets the mask's write bit. `false` means this filesystem
/// or host cannot set one.
fn set_acl(dir: &Path) -> bool {
    Command::new("setfacl")
        .args(["-m", "u:root:rwx"])
        .arg(dir)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn group_writable_directory_follows_the_group() {
    let w = World::new();
    let dir = w.f.home.join(".claude");
    w.chmod(&dir, 0o775);
    let private = group_is_private(&dir);
    let stdout = text(&w.dry().stdout);
    let refused = stdout.contains("trusted login identity unavailable: directory not private");
    assert_eq!(refused, !private, "private group: {private}; {stdout}");
    if private {
        assert!(stdout.contains("live=work"), "{stdout}");
        assert!(!stdout.contains("could not be read"), "{stdout}");
    }
}

#[test]
fn group_writable_directory_can_be_refreshed_when_private() {
    let w = World::new();
    let dir = w.f.home.join(".claude");
    w.expire("work");
    w.fake.renews("work", "work");
    w.chmod(&dir, 0o775);
    let private = group_is_private(&dir);
    let _ = w.cycle();
    let refreshed = w.fake.count("refresh:work") == 1;
    assert_eq!(
        refreshed,
        private,
        "private group: {private}; {}",
        w.f.log()
    );
}

#[test]
fn group_writable_profile_directory_follows_the_group_too() {
    let w = World::new();
    w.no_record("spare");
    let dir = w.f.root.join("spare");
    w.chmod(&dir, 0o775);
    let private = group_is_private(&dir);
    let out = w.seed("spare", false);
    if private {
        assert!(out.status.success(), "{}", printed(&out));
    } else {
        assert!(
            printed(&out).contains("directory not private"),
            "{}",
            printed(&out)
        );
    }
}

#[test]
fn world_writable_directory_is_refused_on_every_host() {
    for mode in [0o777, 0o757] {
        let w = World::new();
        let dir = w.f.home.join(".claude");
        w.chmod(&dir, mode);
        let stdout = text(&w.dry().stdout);
        w.chmod(&dir, 0o700);
        assert!(
            stdout.contains("trusted login identity unavailable: directory not private"),
            "{stdout}"
        );
        assert_eq!(w.fake.count("profile:work"), 0, "no token sent");
    }
}

/// S1: an access ACL refuses the directory even in a private group, and stops the
/// refresh.
#[test]
fn directory_with_an_access_acl_is_refused() {
    let w = World::new();
    let dir = w.f.home.join(".claude");
    w.expire("work");
    w.fake.renews("work", "work");
    if !set_acl(&dir) {
        eprintln!("SKIP: setfacl is missing or this filesystem has no ACL support");
        return;
    }
    let _ = w.cycle();
    assert_eq!(w.fake.refreshes(), 0, "{:?}", w.fake.events());
    let log = w.f.log();
    assert!(log.contains("not refreshed (refresh refused)"), "{log}");
    assert!(
        log.contains("trusted login identity unavailable: directory not private"),
        "{log}"
    );
}

#[test]
fn group_writable_login_file_stays_refused_in_a_private_directory() {
    let w = World::new();
    w.chmod(&w.f.home_login(), 0o660);
    let stdout = text(&w.dry().stdout);
    assert!(stdout.contains("would alert: "), "{stdout}");
    assert_eq!(w.fake.count("profile:work"), 0, "no token sent");
}

/// Plain `profile verify` accepts the same directory the rotator accepts.
#[test]
fn verify_agrees_with_the_rotator_on_a_private_group_directory() {
    let w = World::new();
    let dir = w.f.home.join(".claude");
    w.chmod(&dir, 0o775);
    if !group_is_private(&dir) {
        eprintln!("SKIP: this host's primary group is shared");
        return;
    }
    let out = w.verify();
    assert!(out.status.success(), "{}", printed(&out));
    assert!(
        !printed(&out).contains("folder refused"),
        "{}",
        printed(&out)
    );
}
