//! The login fingerprint (issue #1618), continued from `tests_fingerprint.rs`
//! (whose helpers this uses): an explicit `CLAUDE_CONFIG_DIR`, the rotator
//! and auto-switch ledgers, and whether a rotator cycle gave capacity.
//!
//! Wired from `limit_wait/mod.rs` with
//! `#[cfg(test)] mod tests_fingerprint_ledgers;`.

use super::fingerprint::Watch;
use super::tests_fingerprint::{account, env, private, probe, rotate_ledger};
use amplihack_utils::rotator_status::Ledger;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn an_explicit_config_dir_of_home_claude_reads_its_own_claude_json() {
    // Claude reads `$CLAUDE_CONFIG_DIR/.claude.json` whenever the variable
    // is set, even to `$HOME/.claude`; the rotator ledger is still watched.
    let e = env();
    e.rotator("a");
    let cfg = e.home.join(".claude");
    private(
        &cfg.join(".claude.json"),
        &account(Some("uuid-inner"), "i@example.invalid", 0),
    );
    let w = Watch::new(Some(&e.store), None, Some(&cfg), Some(&e.home));
    let before = probe(&w);
    private(
        &e.home.join(".claude.json"),
        &account(Some("uuid-new"), "me@example.invalid", 0),
    );
    assert!(before == probe(&w), "$HOME/.claude.json is not read");
    private(
        &cfg.join(".claude.json"),
        &account(Some("uuid-inner-2"), "i@example.invalid", 0),
    );
    let after = probe(&w);
    assert!(before != after, "$HOME/.claude/.claude.json is read");
    e.rotator("b");
    assert!(after != probe(&w), "the rotator ledger is still watched");
}

#[test]
fn a_rotation_into_home_claude_is_a_change_for_home() {
    let e = env();
    e.rotator("a");
    let w = e.home_watch();
    let before = probe(&w);
    e.rotator("b");
    assert!(before != probe(&w));
}

#[test]
fn a_rotation_into_home_claude_is_ignored_for_an_inherited_profile() {
    let e = env();
    e.rotator("a");
    let w = e.profile("a");
    let before = probe(&w);
    e.rotator("b");
    assert!(before == probe(&w));
}

#[test]
fn an_untrusted_rotator_ledger_counts_as_absent() {
    let e = env();
    let w = e.home_watch();
    let absent = probe(&w);
    e.rotator("a");
    fs::set_permissions(
        rotate_ledger(e.store.root()),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    assert!(absent == probe(&w));
}

#[test]
fn a_swap_recorded_in_the_auto_switch_ledger_is_a_change_for_that_profile() {
    let e = env();
    e.auto_switch_ledger("a", "a");
    let w = e.profile("a");
    let before = probe(&w);
    e.auto_switch_ledger("a", "b");
    assert!(before != probe(&w));
}

// Issue #2857: the rotator's capacity is a reading of *room*. `rotator_capacity`
// is `CycleStatus::fresh_room`: the cycle must be fresh capacity, as before,
// and must also record `live_room: true`, the rotator's own reading that the
// login now live is below its session threshold and below 100% on its week.
// A cycle that swapped onto a login at 98% is fresh capacity for the
// limit-resume hook (whose rule is unchanged) and is not room here.
//
// Every "no capacity" case below has a positive control at the top of the
// same test or in `a_fresh_rotator_cycle_with_capacity_counts_for_home_only`,
// on a ledger that differs from it in the one field under test.

#[test]
fn a_fresh_rotator_cycle_with_capacity_counts_for_home_only() {
    let e = env();
    let now = crate::commands::profile::fsx::now();
    let start = now - 600;
    let home = e.home_watch();
    assert!(!home.rotator_capacity(start), "no ledger, no capacity");
    for outcome in ["swapped", "stayed"] {
        e.rotator_cycle_room("a", now - 10, None, outcome, false, Some(true));
        assert!(home.rotator_capacity(start), "{outcome}");
        assert!(!e.profile("a").rotator_capacity(start), "{outcome}");
    }
}

#[test]
fn a_cycle_that_records_no_room_is_not_the_rotators_capacity() {
    let e = env();
    let now = crate::commands::profile::fsx::now();
    let start = now - 600;
    let home = e.home_watch();
    e.rotator_cycle_room("a", now - 10, None, "swapped", false, Some(true));
    assert!(
        home.rotator_capacity(start),
        "positive control: room recorded"
    );
    for (what, room) in [
        ("a login that reads full", Some(false)),
        ("a rotator from before #2857 that records no room bit", None),
    ] {
        for outcome in ["swapped", "stayed"] {
            e.rotator_cycle_room("a", now - 10, None, outcome, false, room);
            assert!(!home.rotator_capacity(start), "{outcome}, {what}");
        }
    }
}

/// `live_room: true` is a property of a cycle that has capacity. A record the
/// rotator writes when it has none, or when it is not cycling at all (a stop),
/// never counts, whatever bit rides on it.
#[test]
fn a_true_room_bit_does_not_rescue_a_record_that_is_not_capacity() {
    let e = env();
    let now = crate::commands::profile::fsx::now();
    let start = now - 600;
    let home = e.home_watch();
    e.rotator_cycle_room("a", now - 10, None, "swapped", false, Some(true));
    assert!(home.rotator_capacity(start), "positive control");
    for outcome in [
        "alerted",
        "stayed-unread",
        "idle",
        "stopped",
        "held",
        "skipped",
    ] {
        e.rotator_cycle_room("a", now - 10, None, outcome, false, Some(true));
        assert!(!home.rotator_capacity(start), "{outcome}");
    }
    e.rotator_cycle_room("a", now - 10, None, "swapped", true, Some(true));
    assert!(!home.rotator_capacity(start), "an alert is set");
}

#[test]
fn a_stale_or_alerted_rotator_cycle_is_no_capacity() {
    let e = env();
    let now = crate::commands::profile::fsx::now();
    let start = now - 600;
    let home = e.home_watch();
    // Recorded within a minute of the limit: its probe may predate it.
    e.rotator_cycle_room("a", start + 60, None, "swapped", false, Some(true));
    assert!(!home.rotator_capacity(start));
    e.rotator_cycle_room("a", now - 10, None, "swapped", true, Some(true));
    assert!(!home.rotator_capacity(start), "alert set");
    e.rotator_cycle_room("a", now - 10, None, "alerted", false, Some(true));
    assert!(!home.rotator_capacity(start), "alerted outcome");
    e.rotator_cycle_room("a", now - 10, None, "swapped", false, Some(true));
    assert!(home.rotator_capacity(start), "positive control");
}

/// #1659 on #1625's wait: `measured_at` decides, not the time the cycle was
/// recorded. A cycle recorded well after the limit whose probe began before
/// it is no capacity; one whose probe began a second after the limit is.
#[test]
fn the_wait_reads_freshness_from_measured_at() {
    let e = env();
    let now = crate::commands::profile::fsx::now();
    let start = now - 600;
    let home = e.home_watch();
    e.rotator_cycle_room("a", now - 10, Some(start - 1), "swapped", false, Some(true));
    assert!(
        !home.rotator_capacity(start),
        "probe began before the limit"
    );
    e.rotator_cycle_room("a", now - 10, Some(start), "swapped", false, Some(true));
    assert!(!home.rotator_capacity(start), "probe began at the limit");
    e.rotator_cycle_room(
        "a",
        start + 5,
        Some(start + 1),
        "swapped",
        false,
        Some(true),
    );
    assert!(
        home.rotator_capacity(start),
        "probe began after the limit, recorded inside the old 60-second margin"
    );
}

/// A person's `profile switch` writes outcome `switched` and no room bit: the
/// rotator measured nothing about the login they chose. It is still the
/// rotator's capacity for a wait (#1691's escape hatch), unless the record
/// positively says `live_room: false`. The same ledger as a `swapped` cycle is
/// the control that the carve-out is for `switched` alone.
#[test]
fn a_fresh_manual_switch_is_the_rotators_capacity_unless_it_says_no_room() {
    let e = env();
    let now = crate::commands::profile::fsx::now();
    let start = now - 600;
    let home = e.home_watch();
    for (room, want) in [(None, true), (Some(true), true), (Some(false), false)] {
        e.rotator_cycle_room("a", now - 10, None, "switched", false, room);
        assert_eq!(home.rotator_capacity(start), want, "switched, {room:?}");
        assert!(
            !e.profile("a").rotator_capacity(start),
            "an inherited profile is not the rotator's to speak for"
        );
    }
    e.rotator_cycle_room("a", now - 10, None, "swapped", false, None);
    assert!(
        !home.rotator_capacity(start),
        "control: the same ledger from the rotator's own swap is not room"
    );
}

/// Freshness and the alert are still asked of a manual switch: the carve-out is
/// for the bit and for nothing else.
#[test]
fn a_stale_or_alerted_manual_switch_is_still_no_capacity() {
    let e = env();
    let now = crate::commands::profile::fsx::now();
    let start = now - 600;
    let home = e.home_watch();
    e.rotator_cycle_room("a", now - 10, Some(now - 20), "switched", false, None);
    assert!(home.rotator_capacity(start), "positive control");
    e.rotator_cycle_room("a", now - 10, Some(start), "switched", false, None);
    assert!(!home.rotator_capacity(start), "probed at the limit: stale");
    e.rotator_cycle_room("a", now - 10, Some(now - 20), "switched", true, None);
    assert!(!home.rotator_capacity(start), "alert set");
}

// --- the gate's input: the room bit, read the one way the ledger is read ---
//
// The room gate reads the rotate ledger through `Watch::rotator_state`, the
// one checked reader (O_NOFOLLOW, 64 KiB, owned by this user, not writable by
// others, version 1). What it takes from it is `last_cycle.live_room`, and
// these tests pin that reading on its own: which ledgers yield a bit, which
// yield none, and that absent is absent rather than false. What the gate then
// *does* with the bit, including the cycle's own capacity, is
// `tests_room_gate.rs`.

/// The bit the room gate reads, taken the one way the gate takes it.
fn live_room(w: &Watch) -> Option<bool> {
    match w.rotator_state() {
        Ledger::Read(c) => c.live_room,
        Ledger::Missing | Ledger::Unreadable => None,
    }
}

#[test]
fn live_room_is_the_last_cycles_bit_for_a_home_watch_only() {
    let e = env();
    let home = e.home_watch();
    assert_eq!(live_room(&home), None, "no ledger");
    e.rotator_cycle_room("a", 1, None, "stayed", false, Some(true));
    assert_eq!(live_room(&home), Some(true));
    assert_eq!(
        live_room(&e.profile("a")),
        None,
        "the rotator swaps into $HOME/.claude and nowhere else"
    );
    e.rotator_cycle_room("a", 1, None, "stayed", false, Some(false));
    assert_eq!(live_room(&home), Some(false));
    e.rotator_cycle_room("a", 1, None, "stayed", false, None);
    assert_eq!(live_room(&home), None, "absent is absent, not false");
}

/// The gate reads whatever the latest cycle says, however old: a change is
/// judged against the rotator's last word on the login, and a `true` from an
/// hour ago is still the latest word. (The rotator's capacity route adds a
/// freshness test of its own, above.)
#[test]
fn live_room_is_the_latest_cycles_whatever_its_age() {
    let e = env();
    let home = e.home_watch();
    e.rotator_cycle_room("a", 2, Some(1), "stayed", false, Some(true));
    assert_eq!(live_room(&home), Some(true));
}

/// The ledger is read only through the checked reader: a ledger the checks
/// refuse says nothing, so the gate sees no bit and stays closed.
#[test]
fn live_room_is_not_read_from_a_ledger_the_checks_refuse() {
    let e = env();
    let home = e.home_watch();
    e.rotator_cycle_room("a", 1, None, "stayed", false, Some(true));
    assert_eq!(live_room(&home), Some(true), "positive control");
    fs::set_permissions(
        rotate_ledger(e.store.root()),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    // World-writable only. The checked reader lets group write through when
    // the file's group is the user's own private group (the default on most
    // Linux hosts), so a group-writable case would pass or fail by host.
    assert_eq!(live_room(&home), None, "world-writable");
    fs::set_permissions(
        rotate_ledger(e.store.root()),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(live_room(&home), Some(true), "back to a private file");
    let ledger = rotate_ledger(e.store.root());
    let real = e.tmp.join("elsewhere.json");
    fs::rename(&ledger, &real).unwrap();
    std::os::unix::fs::symlink(&real, &ledger).unwrap();
    assert_eq!(live_room(&home), None, "a symlinked ledger");
}

#[test]
fn live_room_is_none_for_a_wrong_version_or_an_oversize_ledger() {
    let e = env();
    let home = e.home_watch();
    private(
        &rotate_ledger(e.store.root()),
        r#"{"version":2,"live":"a","last_cycle":{"at":1,"outcome":"stayed","live_room":true}}"#,
    );
    assert_eq!(live_room(&home), None, "an unknown version");
    private(
        &rotate_ledger(e.store.root()),
        &format!(
            r#"{{"version":1,"pad":"{}","last_cycle":{{"at":1,"outcome":"stayed","live_room":true}}}}"#,
            "x".repeat(70 * 1024)
        ),
    );
    assert_eq!(live_room(&home), None, "over 64 KiB");
}
