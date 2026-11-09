//! Issue #1962, D4 and S5: the dry run writes nothing, sends no refresh and never
//! starts `claude`, on every read path. Host acceptance runs this mode on real
//! logins, so this has to hold before any run on candide.

#![cfg(unix)]

use crate::support::profile_1962::{Answer, World, account};
use crate::support::profile_rotate::text;

fn assert_untouched(w: &World, before: &crate::support::profile_1962::Stamp) {
    assert_eq!(&w.tree_stamp(), before, "the dry run changed something");
    assert_eq!(w.fake.refreshes(), 0, "{:?}", w.fake.events());
    assert!(w.f.probe_envs().is_empty(), "the dry run started claude");
}

#[test]
fn expired_live_is_previewed_not_refreshed() {
    let w = World::new();
    w.expire("work");
    w.fake.renews("work", "work");
    let before = w.tree_stamp();
    let stdout = text(&w.dry().stdout);
    assert!(
        stdout.contains("would refresh: expired login in ~/.claude"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "would alert: swap refused (identity-unavailable, profile work): trusted login identity unavailable: token expired."
        ),
        "{stdout}"
    );
    assert_untouched(&w, &before);
}

#[test]
fn expired_candidate_is_not_refreshed_and_shows_unmeasured() {
    let w = World::new();
    w.expire("extra");
    w.fake.renews("extra", "extra");
    let before = w.tree_stamp();
    let stdout = text(&w.dry().stdout);
    assert!(stdout.contains("live=work"), "{stdout}");
    assert!(
        !stdout.contains("would refresh"),
        "only ~/.claude is refreshed: {stdout}"
    );
    assert_eq!(w.token("extra"), "fake-task1962-extra-old");
    assert_untouched(&w, &before);
}

#[test]
fn expired_live_and_candidate_send_no_refresh() {
    let w = World::new();
    w.expire("work");
    w.expire("spare");
    w.fake.renews("work", "work");
    w.fake.renews("spare", "spare");
    let before = w.tree_stamp();
    let _ = w.dry();
    assert_untouched(&w, &before);
}

/// A usage 401 on an unexpired token would refresh in a real cycle, and the
/// `fresh::gate` reread could allow it. The dry run must do neither.
#[test]
fn usage_401_neither_refreshes_nor_falls_back_to_claude() {
    let w = World::new();
    for p in ["work", "extra"] {
        w.fake.answer(p, Answer::UsageStatus(account(p), 401));
        w.fake.renews(p, p);
    }
    let before = w.tree_stamp();
    let stdout = text(&w.dry().stdout);
    assert!(stdout.contains("live=work"), "{stdout}");
    assert!(
        w.fake.count("usage:extra") > 0,
        "the read was made: {:?}",
        w.fake.events()
    );
    assert_untouched(&w, &before);
}

#[test]
fn swap_decision_is_only_previewed() {
    let w = World::new();
    let before = w.tree_stamp();
    let stdout = text(&w.dry().stdout);
    assert!(stdout.contains("would swap"), "{stdout}");
    assert_untouched(&w, &before);
    assert_eq!(w.f.ledger().unwrap()["live"], "work");
}

/// The real cycle still refreshes: `read_only` comes from the dry run, not from
/// anything a cycle inherits.
#[test]
fn a_real_cycle_after_a_dry_run_still_refreshes() {
    let w = World::new();
    w.expire("work");
    w.fake.renews("work", "work");
    let _ = w.dry();
    assert_eq!(w.fake.refreshes(), 0);
    let _ = w.cycle();
    assert_eq!(w.fake.count("refresh:work"), 1, "{:?}", w.fake.events());
}
