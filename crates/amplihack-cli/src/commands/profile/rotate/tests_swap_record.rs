//! Swap publication through the production completion transaction (#2085, #2141).
//! A committed placement and ledger precede a private event. Measured finite
//! percentages are rounded up and clamped; unmeasured readings publish nothing.
//! Publication preserves a corrupt previous event and its completion journal
//! for repair, and interrupted completion must not publish a second sequence.
//!
//! Direct publication tests use synthetic committed state under a tempdir.
//! Cycle tests use `tests_swap::env()` with fake credentials and probes.

use super::super::cycle::{Ctx, CycleOutcome, MeasureFn, Mode, run_cycle};
use super::super::decide::Reading;
use super::super::ledger::{Phase, RotateJournal, RotateLedger, RotateState};
use super::super::observe::{Measurement, Snap, decide_measured};
use super::super::swap_record::{Completion, complete};
use super::super::switch::{Switched, switch_in};
use super::super::tests_issue_1870::{exh, rd};
use super::super::tests_revalidate_cause::{add_stored, ctx_with, log_of, probes_for, spent};
use super::super::tests_swap::{Env, env, ino_at, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{now, test_exchange};
use amplihack_utils::rotator_status::swap_record::{
    Judgement, SwapRecord, judge, parse_swap, read_swap,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;

const T: i64 = 1_792_592_000;

fn swap_file(rs: &RotateState) -> std::path::PathBuf {
    rs.dir.join("swap.json")
}

fn on_disk(rs: &RotateState) -> SwapRecord {
    let raw = fs::read(swap_file(rs)).expect("a swap record");
    parse_swap(&raw, now() + 1).expect("the hook parses the swap record")
}

fn measured(session: f64, week: f64) -> Reading {
    Reading::Measured { session, week }
}

fn rs_in(tmp: &tempfile::TempDir) -> RotateState {
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    RotateState::new(tmp.path())
}

/// Seed the committed state the real swap leaves before publishing.
fn publish(rs: &RotateState, reading: &Reading) -> Result<(), ProfileError> {
    let mut j = RotateJournal {
        version: 2,
        from: "work".into(),
        to: "spare".into(),
        adopt: false,
        phase: Phase::S4,
        home: "/synthetic-home".into(),
        live_ino: 1,
        target_ino: 2,
        home_lock_ino: None,
        to_lock_ino: None,
        started_at: T - 30,
        pid: std::process::id(),
        identity: None,
        from_lock_ino: None,
        parking: None,
        completion: Some(Completion::new(reading, T - 30, (None, None), None)),
        outgoing: None,
        outgoing_lock_ino: None,
        from_directory: Some((1, 1)),
        movement_directories: None,
    };
    j.completion.as_mut().unwrap().placed(T);
    rs.save_ledger(&RotateLedger {
        live: Some("spare".into()),
        ..RotateLedger::default()
    })?;
    rs.save_journal(&j)?;
    complete(rs, &mut j, T)
}

#[test]
fn a_measured_reading_writes_a_private_record() {
    let tmp = tempfile::tempdir().unwrap();
    let rs = rs_in(&tmp);
    publish(&rs, &measured(12.2, 40.0)).unwrap();
    let meta = fs::symlink_metadata(swap_file(&rs)).unwrap();
    assert!(meta.file_type().is_file());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    let r = parse_swap(&fs::read(swap_file(&rs)).unwrap(), T).unwrap();
    assert_eq!(r.seq, 1);
    assert_eq!(r.live_login, "spare");
    assert_eq!(r.from_login, "work");
    assert_eq!(r.swapped_at, T);
    assert_eq!(r.reading.measured_at, T - 30);
    assert_eq!((r.reading.session, r.reading.week), (13, 40), "ceil");
    let mut files: Vec<_> = fs::read_dir(&rs.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["ledger.json", "swap.json"],
        "no temporary files or journal remain"
    );
}

#[test]
fn the_json_key_set_is_exactly_the_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let rs = rs_in(&tmp);
    publish(&rs, &measured(1.0, 2.0)).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&fs::read(swap_file(&rs)).unwrap()).unwrap();
    let mut top: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    top.sort();
    assert_eq!(
        top,
        [
            "affinity",
            "from_login",
            "live_login",
            "reading",
            "seq",
            "swapped_at"
        ]
    );
    let mut reading: Vec<_> = v["reading"].as_object().unwrap().keys().cloned().collect();
    reading.sort();
    assert_eq!(reading, ["measured_at", "session", "week"]);
}

#[test]
fn seq_counts_up_from_the_previous_record() {
    let tmp = tempfile::tempdir().unwrap();
    let rs = rs_in(&tmp);
    for want in 1..=3 {
        publish(&rs, &measured(1.0, 1.0)).unwrap();
        assert_eq!(on_disk(&rs).seq, want);
    }
}

#[test]
fn an_unreadable_previous_record_preserves_evidence_and_completion() {
    let tmp = tempfile::tempdir().unwrap();
    let rs = rs_in(&tmp);
    rs.write("swap.json", b"not json").unwrap();
    assert!(publish(&rs, &measured(1.0, 1.0)).is_err());
    assert_eq!(fs::read(swap_file(&rs)).unwrap(), b"not json");
    let pending = rs.load_journal().unwrap().expect("completion retained");
    assert!(pending.completion.is_some());
    assert_eq!(pending.phase, Phase::S4);
}

#[test]
fn percentages_are_rounded_up_and_clamped() {
    let tmp = tempfile::tempdir().unwrap();
    let rs = rs_in(&tmp);
    for ((s, w), want) in [
        ((0.0, 0.0), (0, 0)),
        ((0.1, 99.01), (1, 100)),
        ((-3.0, 300.0), (0, 255)),
        ((100.0, 255.5), (100, 255)),
    ] {
        publish(&rs, &measured(s, w)).unwrap();
        let r = on_disk(&rs);
        assert_eq!((r.reading.session, r.reading.week), want, "{s}/{w}");
    }
}

#[test]
fn a_reading_that_is_not_measured_writes_nothing() {
    for reading in [
        Reading::Failed,
        Reading::Exhausted {
            session: Some(100.0),
            week: Some(20.0),
        },
        Reading::Exhausted {
            session: None,
            week: None,
        },
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let rs = rs_in(&tmp);
        publish(&rs, &reading).unwrap();
        assert!(!swap_file(&rs).exists(), "{reading:?}");
        assert!(
            !rs.journal.exists(),
            "unmeasured completion is still committed"
        );
    }
}

#[test]
fn a_non_finite_figure_writes_nothing() {
    for (s, w) in [
        (f64::NAN, 10.0),
        (10.0, f64::NAN),
        (f64::INFINITY, 10.0),
        (10.0, f64::NEG_INFINITY),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let rs = rs_in(&tmp);
        publish(&rs, &measured(s, w)).unwrap();
        assert!(!swap_file(&rs).exists(), "{s}/{w}");
    }
}

#[test]
fn a_skipped_reading_leaves_the_previous_record_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let rs = rs_in(&tmp);
    publish(&rs, &measured(5.0, 5.0)).unwrap();
    let before = fs::read(swap_file(&rs)).unwrap();
    publish(&rs, &Reading::Failed).unwrap();
    assert_eq!(fs::read(swap_file(&rs)).unwrap(), before);
}

#[test]
fn a_symlink_in_place_of_the_record_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let rs = rs_in(&tmp);
    rs.ensure().unwrap();
    let target = tmp.path().join("elsewhere");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, swap_file(&rs)).unwrap();
    assert!(publish(&rs, &measured(1.0, 1.0)).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    assert!(rs.journal.exists(), "retain completion for repair");
}

// ── In a real cycle ─────────────────────────────────────────────────────────

fn run(e: &Env, measure: MeasureFn) -> Result<CycleOutcome, ProfileError> {
    let mut c: Ctx = ctx_with(e, measure);
    test_exchange::within(e._tmp.path(), || run_cycle(&mut c, Mode::Watch))
}

/// `work` is spent and `spare` has room: a swap to `spare`.
fn measure_swap(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(10.0, 20.0, None))]);
    decide_measured(ctx, snap, spent(), probes, at)
}

/// After [`measure_swap`]: `spare` is spent and `third` has room.
fn measure_swap_to_third(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(
        snap,
        &[("third", rd(5.0, 7.0, None)), ("work", exh(100.0, 100.0))],
    );
    decide_measured(ctx, snap, spent(), probes, at)
}

fn measure_stay(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", rd(40.0, 30.0, None))]);
    decide_measured(ctx, snap, rd(40.0, 30.0, None), probes, at)
}

fn measure_alert(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(snap, &[("spare", exh(100.0, 100.0))]);
    decide_measured(ctx, snap, exh(100.0, 100.0), probes, at)
}

fn measure_error(_: &Ctx, _: &Snap) -> Result<Measurement, ProfileError> {
    Err(ProfileError::Io(std::io::Error::other("injected")))
}

#[test]
fn a_swap_writes_a_record_the_hook_resumes_on() {
    let e = env();
    let limit_hit = now() - 1;
    let got = run(&e, measure_swap).unwrap();
    assert_eq!(got, CycleOutcome::Swapped, "{}", log_of(&e));
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    let meta = fs::symlink_metadata(swap_file(&e.rs)).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    let r = on_disk(&e.rs);
    assert_eq!(r.seq, 1);
    assert_eq!(r.live_login, "spare");
    assert_eq!(r.from_login, "work");
    assert_eq!((r.reading.session, r.reading.week), (10, 20));
    assert!(r.reading.measured_at <= r.swapped_at, "{r:?}");
    assert!(r.swapped_at <= now(), "{r:?}");
    // The hook's view: the record is newer than the stop, with room.
    let seen = read_swap(&e.root, now() + 1).expect("the hook reads it");
    assert_eq!(seen, r);
    assert_eq!(judge(None, &seen, limit_hit, None), Judgement::Resume);
}

#[test]
fn the_record_time_is_not_before_the_swap_moved_the_login() {
    // Crusty round 1 on #2141: `swapped_at` is taken when the swap has
    // finished, not at the cycle's start. Renaming the incoming login into
    // `~/.claude` sets its ctime, so the record may not be older than that.
    use std::os::unix::fs::MetadataExt;
    let e = env();
    assert_eq!(run(&e, measure_swap).unwrap(), CycleOutcome::Swapped);
    let moved_at = fs::symlink_metadata(e.live()).unwrap().ctime();
    let r = on_disk(&e.rs);
    assert!(r.swapped_at >= moved_at, "{r:?} moved_at={moved_at}");
}

#[test]
fn the_record_names_the_login_the_ledger_shows_live() {
    let e = env();
    run(&e, measure_swap).unwrap();
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(
        l.live.as_ref().map(|p| p.to_string()),
        Some(on_disk(&e.rs).live_login)
    );
}

#[test]
fn each_swap_writes_the_next_seq() {
    let e = env();
    add_stored(&e, "third");
    assert_eq!(run(&e, measure_swap).unwrap(), CycleOutcome::Swapped);
    let first = on_disk(&e.rs);
    assert_eq!(first.seq, 1);
    let got = run(&e, measure_swap_to_third).unwrap();
    assert_eq!(got, CycleOutcome::Swapped, "{}", log_of(&e));
    let second = on_disk(&e.rs);
    assert_eq!(second.seq, 2);
    assert_eq!(second.from_login, "spare");
    assert_eq!(second.live_login, "third");
    assert_eq!((second.reading.session, second.reading.week), (5, 7));
    assert_ne!(first, second);
    assert_eq!(
        judge(Some(&first), &second, first.swapped_at - 1, None),
        Judgement::Resume
    );
    let tmp: Vec<_> = fs::read_dir(&e.rs.dir)
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|f| f.contains("swap") && f != "swap.json")
        .collect();
    assert!(tmp.is_empty(), "no temp file is left behind: {tmp:?}");
}

#[test]
fn a_corrupt_previous_record_preserves_completion_and_the_unreadable_evidence() {
    let e = env();
    e.rs.ensure().unwrap();
    e.rs.write("swap.json", b"not json").unwrap();
    assert!(run(&e, measure_swap).is_err());
    assert_eq!(fs::read(swap_file(&e.rs)).unwrap(), b"not json");
    assert!(
        e.rs.journal.exists(),
        "retain automatic completion for recovery"
    );
    assert_eq!(ino_at(&e.live()), Some(e.t));
}

#[test]
fn a_first_record_logs_no_seq_restart() {
    let e = env();
    assert_eq!(run(&e, measure_swap).unwrap(), CycleOutcome::Swapped);
    assert_eq!(on_disk(&e.rs).seq, 1);
    assert!(!log_of(&e).contains("seq restarts"), "{}", log_of(&e));
}

#[test]
fn a_stay_writes_no_record() {
    let e = env();
    assert_eq!(run(&e, measure_stay).unwrap(), CycleOutcome::Stayed);
    assert!(!swap_file(&e.rs).exists());
}

#[test]
fn an_alert_writes_no_record() {
    let e = env();
    assert_eq!(run(&e, measure_alert).unwrap(), CycleOutcome::Alerted);
    assert!(!swap_file(&e.rs).exists());
}

#[test]
fn a_refused_swap_writes_no_record() {
    let e = env();
    // A target login others can read is refused before anything moves.
    fs::set_permissions(e.slot("spare"), fs::Permissions::from_mode(0o644)).unwrap();
    let got = run(&e, measure_swap);
    assert!(
        !matches!(got, Ok(CycleOutcome::Swapped)),
        "the swap must be refused: {got:?}\n{}",
        log_of(&e)
    );
    assert_eq!(ino_at(&e.live()), Some(e.l), "work stays live");
    assert!(!swap_file(&e.rs).exists());
}

#[test]
fn a_failed_cycle_writes_no_record() {
    let e = env();
    assert!(run(&e, measure_error).is_err());
    assert!(!swap_file(&e.rs).exists());
}

#[test]
fn a_manual_switch_writes_no_record() {
    // D2: a person chose the login; the switch carries no fresh reading.
    let e = env();
    let ctx = ctx_with(&e, measure_error);
    let switched = test_exchange::within(e._tmp.path(), || switch_in(ctx, &n("spare")))
        .expect("the switch succeeds")
        .expect("ownership and default commit");
    assert_eq!(
        switched,
        Switched::Moved {
            from: n("work"),
            to: n("spare")
        }
    );
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    assert!(!swap_file(&e.rs).exists());
}

#[test]
fn non_swap_cycles_leave_an_earlier_record_unchanged() {
    let e = env();
    run(&e, measure_swap).unwrap();
    let before = fs::read(swap_file(&e.rs)).unwrap();
    // Now `spare` is live: a stay on it, then an ALERT.
    fn stay_on_spare(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
        let at = now();
        let probes = probes_for(snap, &[("work", rd(40.0, 30.0, None))]);
        decide_measured(ctx, snap, rd(40.0, 30.0, None), probes, at)
    }
    fn alert_on_spare(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
        let at = now();
        let probes = probes_for(snap, &[("work", exh(100.0, 100.0))]);
        decide_measured(ctx, snap, exh(100.0, 100.0), probes, at)
    }
    assert_eq!(run(&e, stay_on_spare).unwrap(), CycleOutcome::Stayed);
    assert_eq!(run(&e, alert_on_spare).unwrap(), CycleOutcome::Alerted);
    assert_eq!(fs::read(swap_file(&e.rs)).unwrap(), before);
}

#[test]
fn a_failed_record_write_is_logged_and_the_swap_stands() {
    let e = env();
    e.rs.ensure().unwrap();
    // A directory where the record goes: the atomic write is refused.
    fs::create_dir(swap_file(&e.rs)).unwrap();
    let got = run(&e, measure_swap);
    assert!(got.is_err(), "publication failure is recoverable");
    assert!(e.rs.journal.exists(), "retain completion intent");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    let log = log_of(&e);
    assert_eq!(
        log.lines()
            .filter(|l| l.contains("swap event publication pending"))
            .count(),
        1,
        "{log}"
    );
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(
        l.live.as_ref().map(|p| p.to_string()).as_deref(),
        Some("spare")
    );
}
