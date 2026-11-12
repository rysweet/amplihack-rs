//! Issue #1962, item 2: the cause on candide, as a reproduction. On candide the
//! umask is `002` and `~/.claude` is mode `775`, owned by `ryan:ryan`, where
//! `ryan` is the user's own private group with no members and no ACL. The 1.2.99
//! login-directory check refused any directory with `mode & 0o022 != 0`, so the
//! identity read stopped before any request was sent. That gives exactly the
//! candide log: "~/.claude could not be read" and the identity alert, with no
//! refresh, 401, 429 or expiry line. An expired token would also have logged
//! "not refreshed (refresh refused)", which candide's log does not have.
//!
//! Run against the 1.2.99 release binary (`AMPLIHACK_TEST_BINARY=<1.2.99>`), the
//! two `775` tests below fail with candide's lines and no request for `work` at
//! the fake endpoint, and the `700` control passes. Against this build all three
//! pass. See
//! "Cause on candide (issue #1962)" in docs/reference/profile-command.md.
//!
//! Every login is fake and every path is under the fixture's temp home.

#![cfg(unix)]

use crate::support::profile_1962::{World, group_is_private};
use crate::support::profile_rotate::text;

/// candide's directory mode for `~/.claude`.
const CANDIDE_MODE: u32 = 0o775;

/// The fixture with `~/.claude` set to `mode`, or `None` when `775` is not in
/// the user's private group here, where the refusal is the expected answer and
/// `issue_1962_dir_rule` covers it.
fn candide_home(mode: u32) -> Option<World> {
    let w = World::new();
    let dir = w.f.home.join(".claude");
    w.chmod(&dir, mode);
    if mode & 0o020 != 0 && !group_is_private(&dir) {
        eprintln!("SKIP: this host's primary group is shared, so 775 is refused here");
        return None;
    }
    Some(w)
}

/// Fails with candide's lines and every request the fake endpoint saw, so a run
/// against 1.2.99 shows both the refusal and that no request was sent for `work`.
fn assert_no_candide_lines(w: &World, output: &str) {
    let events = w.fake.events();
    assert!(
        !output.contains("could not be read"),
        "{output}\nrequests: {events:?}"
    );
    assert!(
        !output.contains("trusted login identity unavailable"),
        "{output}\nrequests: {events:?}"
    );
}

/// candide's state, unexpired token: the identity is read over the fake endpoint
/// and the dry run names the live login without an alert.
#[test]
fn candide_mode_home_is_read_and_names_the_live_login() {
    let Some(w) = candide_home(CANDIDE_MODE) else {
        return;
    };
    let stdout = text(&w.dry().stdout);
    assert_no_candide_lines(&w, &stdout);
    assert!(!stdout.contains("would alert: "), "{stdout}");
    assert!(stdout.contains("live=work"), "{stdout}");
    assert!(
        w.fake.count("profile:work") >= 1,
        "identity requested: {:?}",
        w.fake.events()
    );
}

/// candide's state with the live token expired: the refresh is sent, then the
/// identity is read, and the cycle raises no identity alert.
#[test]
fn candide_mode_home_with_an_expired_token_is_refreshed_then_read() {
    let Some(w) = candide_home(CANDIDE_MODE) else {
        return;
    };
    w.expire("work");
    w.fake.renews("work", "work");
    let _ = w.cycle();
    let log = w.f.log();
    assert_no_candide_lines(&w, &log);
    assert_eq!(w.fake.count("refresh:work"), 1, "{:?}", w.fake.events());
    assert!(w.fake.count("profile:work") >= 1, "{:?}", w.fake.events());
}

/// The control: the same state with `~/.claude` at `700`. 1.2.99 passes this one,
/// so the directory mode is the only difference between the failing and the
/// working case.
#[test]
fn private_mode_home_is_read_on_every_build() {
    let Some(w) = candide_home(0o700) else {
        return;
    };
    let stdout = text(&w.dry().stdout);
    assert_no_candide_lines(&w, &stdout);
    assert!(stdout.contains("live=work"), "{stdout}");
    assert!(w.fake.count("profile:work") >= 1, "{:?}", w.fake.events());
}
