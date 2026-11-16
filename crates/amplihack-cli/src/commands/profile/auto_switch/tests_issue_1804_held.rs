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
        (n("cred"), R::EXHAUSTED),
        (
            n("work"),
            R::Measured {
                session: 100.0,
                week: 40.0,
            },
        ),
        (n("console"), R::EXHAUSTED),
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

    rotator_measured(&f, R::EXHAUSTED, NOW - 60);
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
    rotator_measured(&f, R::EXHAUSTED, NOW - 241);
    assert_eq!(pick_without_spare(&f, NOW), None);
}

/// The record covers every login of the cycle, so a login the rotator has
/// moved off, unread by auto-switch, stays measured while it holds.
#[test]
fn a_login_the_rotator_moved_off_stays_measured() {
    let f = fx("enabled=on\noverflow=console\n");
    rotator_measured(&f, R::EXHAUSTED, NOW - 60);
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

/// Issue #2694: the usage-limit wait asks for a candidate every 30 s, for
/// the whole of a wait that may run for hours, and now does so on a reset
/// wait as well as a capacity wait. The probe therefore writes nothing: no
/// held-key ALERT to the step's stderr, no line in the rotate log and no
/// key-held marker. The same selection on the rerun path still writes one,
/// which is what the step saw once before its wait began.
#[test]
fn the_wait_probe_writes_no_held_key_alert() {
    use crate::commands::profile::auto_switch::takeover::capacity_candidate;
    use crate::commands::profile::store::ProfileStore;

    // The auto-switch setting is resolved for real here, so the rotator
    // check needs the synthetic process table (issue #2196).
    let _proc = crate::test_support::proc::fake_proc_scope();
    let _l = lock();
    let f = fx("enabled=on\noverflow=console\n");
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _toggle = EnvGuard::unset([config::TOGGLE_ENV]);
    let _gate = default_gate();
    let now = fsx::now();
    // The key passes its own login check, so the only thing keeping work
    // off it is the unread plan login (issue #1804).
    private(
        &f.root.join("console").join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"sk-test"}}"#,
    );

    // Every plan login used up and `spare` unread, so the unread login is
    // the only thing holding work off the API key: the ALERT case. The key
    // has a reading of its own, so the probe does not skip it as stale.
    let mut readings = used_up(now);
    readings.remove("spare");
    readings.insert("console".to_string(), reading(0.0, 4 * 3600, now));
    f.state
        .save_usage(&UsageFile {
            version: 1,
            accounts: readings.clone(),
        })
        .unwrap();

    let store = ProfileStore::new(f.root.clone());
    let got = capacity_candidate(&store, &f.state, &n("work"), now - 60, now);
    assert_eq!(got, None, "the unread login holds the key back");
    assert!(alert_lines(&f).is_empty(), "the probe writes no ALERT");
    assert_eq!(f.state.load_key_held(), None, "and no marker");

    // The rerun path, with the same readings and the same stale login
    // skipped, does write it: the probe's silence is the only difference.
    let got = nested_candidate(
        &f.state,
        &Ledger::default(),
        &f.profiles,
        &readings,
        &cfg(&["console"]),
        "work",
        &["spare".to_string()],
    );
    assert_eq!(got, None);
    assert_eq!(alert_lines(&f).len(), 1, "the rerun check writes it");
    assert_eq!(f.state.load_key_held().as_deref(), Some("spare"));
}
