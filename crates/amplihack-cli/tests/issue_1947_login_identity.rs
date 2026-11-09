//! Issue #1947: a rotate ledger whose `login_identity` is malformed is
//! refused when it is loaded. The cycle and `profile switch` stop with the
//! unreadable-ledger line, which names the refused field, as do `--dry-run`
//! and `profile rotate status`. Nothing moves, no record is written, the file
//! is kept byte for byte on the same inode, and the value never reaches the
//! log or the output. Runs under `World`, which is isolated from the real
//! `~/.claude`, `~/.amplihack` and profile root.

#![cfg(unix)]

use crate::support::profile_1962::{World, printed};
use crate::support::profile_rotate::ino;
use std::fs;

const SENTINEL: &str = "S1947SENTINEL";

/// The unreadable-ledger stop, naming the refused field (issue #1962 item 3:
/// a cause the code knows is reported as itself).
fn stop_line() -> String {
    "rotate ledger unreadable (invalid login_identity); live identity: readable; nothing moved, no record written".into()
}

/// The ledger `World::new` writes, except that the e-mail has no `@`.
fn malformed() -> World {
    let w = World::new();
    w.set_ledger(&World::ledger_for(
        "work",
        "uuid-work",
        serde_json::json!({"accountUuid": "uuid-work", "organizationUuid": "org-work",
            "emailAddress": format!("{SENTINEL}-no-at")}),
    ));
    w
}

fn assert_spares_in_place(w: &World) {
    assert_eq!(ino(&w.f.slot("spare")), Some(w.f.spare_ino));
    assert_eq!(ino(&w.f.slot("extra")), Some(w.f.extra_ino));
    assert_eq!(ino(&w.f.slot("work")), None, "no login moved to the slot");
}

/// The ledger's bytes and inode, so a silent atomic rewrite is caught too.
fn ledger_stamp(w: &World) -> (Vec<u8>, Option<u64>) {
    (fs::read(w.ledger_path()).unwrap(), ino(&w.ledger_path()))
}

#[test]
fn cycle_stops_on_a_malformed_login_identity() {
    let w = malformed();
    let before = ledger_stamp(&w);
    let out = w.cycle();

    let log = w.f.log();
    assert!(log.contains(&format!("ALERT: {}", stop_line())), "{log}");
    assert!(!log.contains(SENTINEL), "the value reached the log");
    assert!(
        !printed(&out).contains(SENTINEL),
        "the value reached the output"
    );
    assert!(ledger_stamp(&w) == before, "the ledger was rewritten");
    assert_spares_in_place(&w);
}

#[test]
fn switch_refuses_a_malformed_login_identity_with_the_same_text() {
    let w = malformed();
    let before = ledger_stamp(&w);
    let out = w.switch("extra");

    let all = printed(&out);
    assert!(!out.status.success(), "{all}");
    assert!(all.contains(&stop_line()), "{all}");
    assert!(!all.contains(SENTINEL), "the value reached the output");
    assert!(!w.f.log().contains(SENTINEL), "the value reached the log");
    assert!(ledger_stamp(&w) == before, "the ledger was rewritten");
    assert_spares_in_place(&w);
}

#[test]
fn a_malformed_login_identity_writes_no_record() {
    let w = malformed();
    let record = w.record("work");
    let _ = w.cycle();
    assert_eq!(w.record("work"), record);
    assert!(!w.f.log().contains("seeded:"), "{}", w.f.log());
}

/// The stop is not a one-off: every cycle reports it until the file is fixed.
#[test]
fn every_cycle_reports_the_stop() {
    let w = malformed();
    let line = stop_line();
    for cycle in 1..=2 {
        let _ = w.cycle();
        assert_eq!(w.f.log().matches(&line).count(), cycle, "{}", w.f.log());
    }
}

#[test]
fn dry_run_names_the_refused_field() {
    let w = malformed();
    let before = ledger_stamp(&w);
    let out = w.dry();

    let all = printed(&out);
    assert!(
        all.contains(&format!("would alert: {}", stop_line())),
        "{all}"
    );
    assert!(!all.contains(SENTINEL), "the value reached the output");
    assert!(ledger_stamp(&w) == before, "the ledger was rewritten");
}

#[test]
fn status_names_the_refused_field() {
    let w = malformed();
    let before = ledger_stamp(&w);
    let out = w.status();

    let all = printed(&out);
    assert!(
        all.contains("live: - (ledger unreadable: invalid login_identity)"),
        "{all}"
    );
    assert!(!all.contains(SENTINEL), "the value reached the output");
    assert!(ledger_stamp(&w) == before, "the ledger was rewritten");
}
