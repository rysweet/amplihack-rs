//! Issue #1804, crusty round 15: what auto-switch knows of the rotator's live
//! login, and how often it writes its held-key ALERT to the rotate log.

use super::*;
use crate::commands::profile::auto_switch::capacity::ROTATE_WRITER;
use crate::commands::profile::auto_switch::per_token::{KeyHeld, record_key_held};
use amplihack_utils::claude_capacity::CapacityFile;
use chrono::DateTime;

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

fn capacity_file(f: &Fx, writer: &str, over: &[&str], written_at: i64) {
    let file = CapacityFile {
        version: cap::VERSION,
        state: Cap::Exhausted,
        threshold: 90,
        over: over.iter().map(ToString::to_string).collect(),
        earliest_reset: None,
        written_at: DateTime::from_timestamp(written_at, 0).unwrap(),
        writer: writer.to_string(),
    };
    private(&cap::path(&f.root), &serde_json::to_string(&file).unwrap());
}

/// The rotator measures its live login, which auto-switch cannot. When its
/// capacity file lists that login as full, the login is measured with no
/// room and no longer holds the key back. Any other evidence leaves it
/// unread.
#[test]
fn the_rotators_reading_of_its_live_login_is_used() {
    let f = live_spare();
    assert_eq!(pick_without_spare(&f, NOW), None, "no reading: held");

    capacity_file(&f, ROTATE_WRITER, &["work", "spare", "cred"], NOW - 60);
    assert_eq!(pick_without_spare(&f, NOW).as_deref(), Some("console"));
    assert!(
        f.inputs(&cfg(&["console"]), Some(GATE), NOW)
            .no_login
            .contains(&n("spare"))
    );

    // Not listed as over: the rotator found room, or did not say.
    capacity_file(&f, ROTATE_WRITER, &["work", "cred"], NOW - 60);
    assert_eq!(pick_without_spare(&f, NOW), None);

    // Another writer cannot read `~/.claude`.
    capacity_file(&f, "statusline", &["work", "spare", "cred"], NOW - 60);
    assert_eq!(pick_without_spare(&f, NOW), None);

    // Past the time an unknown reset is trusted: the reading has expired.
    let old = NOW - cap::UNKNOWN_RESET_TTL_SECS - 1;
    capacity_file(&f, ROTATE_WRITER, &["work", "spare", "cred"], old);
    assert_eq!(pick_without_spare(&f, NOW), None);
}

/// A capacity file naming a login that is not the rotator's live one says
/// nothing about a login auto-switch could have read itself.
#[test]
fn only_the_live_login_takes_the_rotators_reading() {
    let f = fx("enabled=on\noverflow=console\n");
    capacity_file(&f, ROTATE_WRITER, &["work", "spare", "cred"], NOW - 60);
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
