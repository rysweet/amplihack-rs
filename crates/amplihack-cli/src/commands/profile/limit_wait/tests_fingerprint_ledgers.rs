//! The login fingerprint (issue #1618), continued from `tests_fingerprint.rs`
//! (whose helpers this uses): an explicit `CLAUDE_CONFIG_DIR`, the rotator
//! and auto-switch ledgers, and whether a rotator cycle gave capacity.
//!
//! Wired from `limit_wait/mod.rs` with
//! `#[cfg(test)] mod tests_fingerprint_ledgers;`.

use super::fingerprint::Watch;
use super::tests_fingerprint::{account, env, private, probe, rotate_ledger};
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

#[test]
fn a_fresh_rotator_cycle_with_capacity_counts_for_home_only() {
    let e = env();
    let now = crate::commands::profile::fsx::now();
    let start = now - 600;
    let home = e.home_watch();
    assert!(!home.rotator_capacity(start), "no ledger, no capacity");
    for outcome in ["swapped", "stayed"] {
        e.rotator_cycle("a", now - 10, outcome, false);
        assert!(home.rotator_capacity(start), "{outcome}");
        assert!(!e.profile("a").rotator_capacity(start), "{outcome}");
    }
}

#[test]
fn a_stale_or_alerted_rotator_cycle_is_no_capacity() {
    let e = env();
    let now = crate::commands::profile::fsx::now();
    let start = now - 600;
    let home = e.home_watch();
    // Recorded within a minute of the limit: its probe may predate it.
    e.rotator_cycle("a", start + 60, "swapped", false);
    assert!(!home.rotator_capacity(start));
    e.rotator_cycle("a", now - 10, "swapped", true);
    assert!(!home.rotator_capacity(start), "alert set");
    e.rotator_cycle("a", now - 10, "alerted", false);
    assert!(!home.rotator_capacity(start), "alerted outcome");
}
