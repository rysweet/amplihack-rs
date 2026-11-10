//! Issue #2013, outside in: `amplihack profile rotate` runs once as a user
//! would, against a temporary HOME and profile root, a fake `claude` that
//! prints canned `/usage` output and a loopback identity endpoint. No real
//! login, `~/.claude` or `~/.claude.json` is read.
//!
//! 1. The 6 Nov fixture: live at w=98%, candidates at w=97%, 11% and 52%.
//!    The swap goes to the 11% login.
//! 2. `~/.claude.json` names another account while the ledger and the
//!    server-verified live login say `work`, and `work` is due to swap. The
//!    cycle repairs the metadata from the verified identity, logs the
//!    evidence, refuses no swap, swaps, and keeps the file's other keys.

#![cfg(target_os = "linux")]

use crate::support::profile_1962::World;
use crate::support::profile_rotate::*;
use std::fs;

fn swaps(out: &std::process::Output) -> Vec<String> {
    text(&out.stdout)
        .lines()
        .filter(|l| l.contains(" swap "))
        .map(no_ages)
        .collect()
}

#[test]
fn plan_room_swaps_to_the_login_with_the_most_weekly_room() {
    let f = Fixture::new();
    f.add_logged_in("eels", 10, 11);
    f.usage("work", 50, 98);
    f.usage("spare", 10, 97);
    f.usage("extra", 10, 52);
    let out = f.ok(&["profile", "rotate"]);
    let all = text(&out.stdout);
    eprintln!("SCENARIO1 OUTPUT:\n{all}");
    let swaps = swaps(&out);
    assert_eq!(swaps.len(), 1, "one swap: {all}");
    assert!(swaps[0].contains("swap work->eels"), "{all}");
    assert!(!all.contains("swap refused"), "{all}");
    assert_eq!(f.home_account(), "uuid-eels");
}

#[test]
fn stale_home_account_metadata_is_repaired_and_the_swap_goes_on() {
    let w = World::new();
    // The ledger and the live login both say `work`; the home metadata names
    // `spare`, as ffarmer's block did after the 6 Nov 20:30:27Z swap.
    let path = w.f.home.join(".claude.json");
    private_file(
        &path,
        &serde_json::json!({"oauthAccount": {"accountUuid": "uuid-spare",
            "emailAddress": "spare@example.com"}, "numStartups": 7})
        .to_string(),
    );
    assert_eq!(w.f.home_account(), "uuid-spare");

    // `work` reads s=90%, so this cycle also wants to swap away.
    let out = w.cycle();
    let all = format!("{}{}\n{}", text(&out.stdout), text(&out.stderr), w.f.log());
    eprintln!("SCENARIO2 OUTPUT:\n{all}");
    assert!(out.status.success(), "{all}");
    assert!(
        all.contains(
            "repair: home account metadata named spare@example.com, \
             verified identity is work@example.com; rewritten"
        ),
        "{all}"
    );
    assert!(!all.contains("swap refused"), "{all}");
    let swaps = swaps(&out);
    assert_eq!(swaps.len(), 1, "the cycle goes on and swaps: {all}");
    assert!(swaps[0].contains("swap work->"), "{all}");
    // After the swap the home metadata names the account the ledger holds.
    let ledger = w.f.ledger().unwrap();
    assert_ne!(ledger["live"], "work", "{ledger}");
    assert_eq!(
        w.f.home_account(),
        ledger["account"].as_str().unwrap(),
        "{ledger}"
    );
    let after: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["numStartups"], 7, "other keys kept: {after}");
}
