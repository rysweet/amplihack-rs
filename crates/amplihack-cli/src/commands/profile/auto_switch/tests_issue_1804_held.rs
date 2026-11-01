//! Issue #1804, crusty rounds 15 and 16: what auto-switch knows of the
//! rotator's live login (the ledger's `no_room` record), and how often it
//! writes its held-key ALERT to the rotate log. A real rotator cycle writing
//! that record is in `rotate::tests_no_room`.

use super::*;
use crate::commands::profile::auto_switch::per_token::{KeyHeld, record_key_held};
use crate::commands::profile::rotate::decide::Reading as R;
use crate::commands::profile::rotate::plan_room::no_room;
use crate::commands::profile::rotate::plan_room::room;

const NOW: i64 = 2_000_000_000;

/// `spare` is the rotator's live login: its login is in `~/.claude`.
fn live_spare() -> Fx {
    let f = fx("enabled=on\noverflow=console\n");
    fs::remove_file(f.root.join("spare").join(CREDS)).unwrap();
    let rs = RotateState::new(&f.root);
    rs.ensure().unwrap();
    rs.save_ledger(&RotateLedger {
        live: Some("spare".into()),
        ..RotateLedger::default()
    })
    .unwrap();
    f
}

/// What a rotator cycle that measured at `at` writes to the ledger, through
/// the rotator's own `no_room::of_cycle`: `spare` live with reading `spare`,
/// `work` and `cred` at their limits, the key with its synthetic reading.
fn rotator_measured(f: &Fx, spare: R, at: i64) {
    let others = [
        (n("cred"), R::Exhausted),
        (
            n("work"),
            R::Measured {
                session: 100.0,
                week: 40.0,
            },
        ),
        (n("console"), R::Exhausted),
    ];
    let rec = no_room::of_cycle(
        Some(&n("spare")),
        &spare,
        &others,
        Some(&n("console")),
        at,
        at,
        120,
    );
    let rs = RotateState::new(&f.root);
    let mut l = rs.load_ledger().unwrap().unwrap_or_default();
    l.no_room = Some(rec);
    rs.save_ledger(&l).unwrap();
}

/// The rotator measures its live login, which auto-switch cannot. Its
/// ledger records that login with no room whatever the capacity state, so
/// the login no longer holds the key back. A login it found with room, a
/// failed probe, or an expired record leaves the login unread.
#[test]
fn the_rotators_reading_of_its_live_login_is_used() {
    let f = live_spare();
    assert_eq!(pick_without_spare(&f, NOW), None, "no reading: held");

    rotator_measured(&f, R::Exhausted, NOW - 60);
    assert_eq!(pick_without_spare(&f, NOW).as_deref(), Some("console"));
    assert!(
        f.inputs(&cfg(&["console"]), Some(GATE), NOW)
            .no_login
            .contains(&n("spare"))
    );
    // The same at 100% of the week: `room` is the rotator's own test.
    rotator_measured(
        &f,
        R::Measured {
            session: 3.0,
            week: 100.0,
        },
        NOW - 60,
    );
    assert!(!room(3.0, 100.0));
    assert_eq!(pick_without_spare(&f, NOW).as_deref(), Some("console"));

    // Room, or no reading: the login may have room.
    rotator_measured(
        &f,
        R::Measured {
            session: 99.0,
            week: 99.0,
        },
        NOW - 60,
    );
    assert_eq!(pick_without_spare(&f, NOW), None);
    rotator_measured(&f, R::Failed, NOW - 60);
    assert_eq!(pick_without_spare(&f, NOW), None);

    // Two cycles later the record no longer holds.
    rotator_measured(&f, R::Exhausted, NOW - 241);
    assert_eq!(pick_without_spare(&f, NOW), None);
}

/// The record covers every login of the cycle, so a login the rotator has
/// moved off, unread by auto-switch, stays measured while it holds.
#[test]
fn a_login_the_rotator_moved_off_stays_measured() {
    let f = fx("enabled=on\noverflow=console\n");
    rotator_measured(&f, R::Exhausted, NOW - 60);
    let rs = RotateState::new(&f.root);
    let mut l = rs.load_ledger().unwrap().unwrap();
    l.live = Some("work".into());
    rs.save_ledger(&l).unwrap();
    assert_eq!(pick_without_spare(&f, NOW).as_deref(), Some("console"));
}

/// The rotator's capacity file is not read for this any more: one that
/// lists `spare` as full, from any writer, leaves it unread.
#[test]
fn the_capacity_file_says_nothing_about_the_live_login() {
    use amplihack_utils::claude_capacity::CapacityFile;
    let f = live_spare();
    let file = CapacityFile {
        version: cap::VERSION,
        state: Cap::Exhausted,
        threshold: 90,
        over: vec!["cred".into(), "spare".into(), "work".into()],
        earliest_reset: None,
        written_at: chrono::DateTime::from_timestamp(NOW - 60, 0).unwrap(),
        writer: "rotate".into(),
    };
    private(&cap::path(&f.root), &serde_json::to_string(&file).unwrap());
    assert_eq!(pick_without_spare(&f, NOW), None);
}

fn alert_lines(f: &Fx) -> Vec<String> {
    let log = RotateState::new(&f.root).log;
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once(' ').map(|(_, b)| b.to_string()))
        .filter(|b| b.starts_with("ALERT: API key not used"))
        .collect()
}

fn held(unread: &[&str], session: &str) -> KeyHeld {
    let unread: Vec<ProfileName> = unread.iter().map(|p| n(p)).collect();
    let line = format!(
        "{}; auto-switch from {session}",
        crate::commands::profile::rotate::alert_key_held_session(session, &unread)
    );
    KeyHeld {
        unread,
        line: Some(line),
    }
}

/// Every exhausted launch and rerun asks; the rotate log gets one line per
/// held set, as the rotator's `alert_once` gives one per key.
#[test]
fn the_held_key_alert_is_written_once_per_held_set() {
    let f = fx("enabled=on\noverflow=console\n");
    record_key_held(&f.state, &held(&["spare"], "work"));
    record_key_held(&f.state, &held(&["spare"], "work"));
    record_key_held(&f.state, &held(&["spare"], "cred"));
    assert_eq!(alert_lines(&f).len(), 1, "same set, written once");

    // Unread but not what held the key: nothing written, nothing forgotten.
    record_key_held(
        &f.state,
        &KeyHeld {
            unread: vec![n("spare")],
            line: None,
        },
    );
    record_key_held(&f.state, &held(&["spare"], "work"));
    assert_eq!(alert_lines(&f).len(), 1);

    // A new set is a new incident.
    record_key_held(&f.state, &held(&["cred", "spare"], "work"));
    assert_eq!(alert_lines(&f).len(), 2);

    // Every login measured: forgotten, so the next hold is written again.
    record_key_held(&f.state, &KeyHeld::default());
    record_key_held(&f.state, &held(&["cred", "spare"], "work"));
    let lines = alert_lines(&f);
    assert_eq!(lines.len(), 3);
    assert!(
        lines
            .iter()
            .all(|l| l.contains(" session=") && !l.contains(" live=")),
        "{lines:?}"
    );
}
