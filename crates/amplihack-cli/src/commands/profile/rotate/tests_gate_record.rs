//! The running rotator's overflow gate, as auto-switch reads it (issue
//! #1606), and agreement between the rotator and auto-switch on when money
//! is spent.
//!
//! Contract:
//! - `rotate watch` records `gate.json` with its pid and its gate; while that
//!   rotator runs, auto-switch uses the recorded gate and ignores the
//!   `AMPLIHACK_ROTATE_*` variables.
//! - A running rotator without a matching, valid record, or a rotator that
//!   cannot be verified, closes the gate.
//! - With no rotator, the four gate variables (or defaults) give the gate;
//!   any other `AMPLIHACK_ROTATE_*` variable plays no part.
//! - On the same readings, with the same gate, the rotator moves onto an
//!   overflow profile exactly when auto-switch picks one, including across
//!   the band between `--session-at` (85) and auto-switch's threshold (90).

use super::decide::{Measured, Probed, Reading, Reason, Win, Windows};
use super::gate_record::{self, GateRecord, GateSource};
use super::ledger::RotateState;
use super::overflow::{OverflowInput, Tag, decide_overflow, gate};
use super::settings::Settings;
use crate::commands::profile::auto_switch::config::Config;
use crate::commands::profile::auto_switch::{
    OverflowInputs, OverflowPool, Pool, ReadingSource, SwitchReading, Window, select_with_overflow,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{Gate, OverflowEntry};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const EXE: &str = "amplihack";
const PID: u32 = 31337;
const NOW: i64 = 1_760_000_000;
const HOUR: i64 = 3600;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn settings(overflow_wait: u64, unknown_reset: bool) -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait,
        overflow_unknown_reset: unknown_reset,
    }
}

struct Fake {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    rs: RotateState,
    proc_root: PathBuf,
}

fn fake() -> Fake {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    let proc_root = tmp.path().join("proc");
    fs::create_dir(&proc_root).unwrap();
    Fake {
        root: rs.root.clone(),
        _tmp: tmp,
        rs,
        proc_root,
    }
}

impl Fake {
    /// A verified `amplihack profile rotate watch` with `pid`, named in the
    /// pid file.
    fn rotator_runs(&self, pid: u32) {
        fs::write(&self.rs.pid, format!("{pid}\n")).unwrap();
        fs::set_permissions(&self.rs.pid, fs::Permissions::from_mode(0o600)).unwrap();
        let d = self.proc_root.join(pid.to_string());
        fs::create_dir(&d).unwrap();
        let mut argv = Vec::new();
        for p in ["/usr/bin/amplihack", "profile", "rotate", "watch"] {
            argv.extend_from_slice(p.as_bytes());
            argv.push(0);
        }
        fs::write(d.join("cmdline"), argv).unwrap();
    }

    fn gate(&self, env: &[(&str, &str)]) -> (Option<Gate>, GateSource) {
        let vars: HashMap<String, OsString> = env
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        super::overflow_gate_at(&self.root, &self.proc_root, OsStr::new(EXE), &|k| {
            vars.get(k).cloned()
        })
    }
}

// --- Where the gate comes from ---------------------------------------------

#[test]
fn a_running_rotator_s_record_is_used_and_the_environment_is_not() {
    let f = fake();
    f.rotator_runs(PID);
    gate_record::write(&f.rs, PID, &settings(7200, true)).unwrap();
    let (g, src) = f.gate(&[
        ("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "60"),
        ("AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET", "0"),
        ("AMPLIHACK_ROTATE_SESSION_AT", "50"),
    ]);
    assert_eq!(src, GateSource::Rotator(PID));
    assert_eq!(g, Some(gate(&settings(7200, true))));
}

#[test]
fn the_record_is_written_private_and_reads_back() {
    let f = fake();
    gate_record::write(&f.rs, PID, &settings(7200, false)).unwrap();
    let path = f.rs.dir.join(gate_record::FILE);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        gate_record::load(&f.rs).unwrap(),
        Some(GateRecord::new(PID, &settings(7200, false)))
    );
}

#[test]
fn a_running_rotator_without_its_own_record_closes_the_gate() {
    let f = fake();
    f.rotator_runs(PID);
    // No record: an older rotator, or one that could not write it.
    let (g, src) = f.gate(&[]);
    assert_eq!(g, None);
    assert!(
        matches!(src, GateSource::Closed(ref why) if why.contains("31337")),
        "{src:?}"
    );
    // A record left by an earlier rotator.
    gate_record::write(&f.rs, PID + 1, &settings(0, true)).unwrap();
    assert_eq!(f.gate(&[]).0, None);
}

#[test]
fn a_damaged_or_out_of_range_record_closes_the_gate() {
    let f = fake();
    f.rotator_runs(PID);
    let path = f.rs.dir.join(gate_record::FILE);
    for body in [
        "not json".to_string(),
        format!(
            r#"{{"version":1,"pid":{PID},"overflow_wait":1800,"overflow_unknown_reset":false,"session_at":0,"week_at":90}}"#
        ),
        format!(
            r#"{{"version":1,"pid":{PID},"overflow_wait":9999999,"overflow_unknown_reset":false,"session_at":85,"week_at":90}}"#
        ),
        format!(
            r#"{{"version":2,"pid":{PID},"overflow_wait":1800,"overflow_unknown_reset":false,"session_at":85,"week_at":90}}"#
        ),
    ] {
        fs::write(&path, &body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let (g, src) = f.gate(&[]);
        assert_eq!(g, None, "{body}");
        assert!(matches!(src, GateSource::Closed(_)), "{body}: {src:?}");
    }
}

#[test]
fn an_unverifiable_rotator_closes_the_gate() {
    let f = fake();
    f.rotator_runs(PID);
    gate_record::write(&f.rs, PID, &settings(1800, false)).unwrap();
    fs::remove_dir_all(&f.proc_root).unwrap();
    let (g, src) = f.gate(&[]);
    assert_eq!(g, None);
    assert!(matches!(src, GateSource::Closed(_)), "{src:?}");
}

#[test]
fn without_a_rotator_the_gate_variables_or_defaults_apply() {
    let f = fake();
    // A record without a running rotator is not used.
    gate_record::write(&f.rs, PID, &settings(7200, true)).unwrap();
    let (g, src) = f.gate(&[]);
    assert_eq!(src, GateSource::Environment);
    assert_eq!(g, Some(gate(&settings(1800, false))));
    let (g, _) = f.gate(&[
        ("AMPLIHACK_ROTATE_OVERFLOW_WAIT", "600"),
        ("AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET", "1"),
        ("AMPLIHACK_ROTATE_WEEK_AT", "95"),
    ]);
    assert_eq!(
        g,
        Some(Gate {
            wait: 600,
            unknown_reset: true,
            session_at: 85,
            week_at: 95,
        })
    );
}

#[test]
fn an_unrelated_invalid_variable_does_not_close_the_gate() {
    let f = fake();
    for var in [
        "AMPLIHACK_ROTATE_INTERVAL",
        "AMPLIHACK_ROTATE_HOLD",
        "AMPLIHACK_ROTATE_MARGIN",
        "AMPLIHACK_ROTATE_UPGRADE_MARGIN",
    ] {
        let (g, src) = f.gate(&[(var, "abc")]);
        assert_eq!(src, GateSource::Environment, "{var}");
        assert_eq!(g, Some(gate(&settings(1800, false))), "{var}");
    }
}

#[test]
fn an_invalid_gate_variable_closes_the_gate_and_says_which() {
    let f = fake();
    for var in [
        "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
        "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
        "AMPLIHACK_ROTATE_SESSION_AT",
        "AMPLIHACK_ROTATE_WEEK_AT",
    ] {
        let (g, src) = f.gate(&[(var, "abc")]);
        assert_eq!(g, None, "{var}");
        assert!(
            matches!(src, GateSource::Closed(ref why) if why.contains(var)),
            "{var}: {src:?}"
        );
        let line = gate_record::status_line(g, &src);
        assert!(
            line.starts_with("overflow gate: closed (") && line.contains(var),
            "{line}"
        );
    }
}

#[test]
fn the_status_line_names_the_source() {
    let g = gate(&settings(7200, false));
    assert_eq!(
        gate_record::status_line(Some(g), &GateSource::Rotator(PID)),
        "overflow gate: wait 7200s, unknown reset waits, plan capacity below 85% session and 90% week (from the rotator, pid 31337)"
    );
    assert!(
        gate_record::status_line(Some(g), &GateSource::Environment).contains("no rotator runs")
    );
}

// --- Auto-switch follows the running rotator --------------------------------

fn sw(five: f64, resets_in: i64) -> SwitchReading {
    SwitchReading {
        five_hour: Some(Window {
            used_percentage: five,
            resets_at: NOW + resets_in,
        }),
        seven_day: Some(Window {
            used_percentage: 10.0,
            resets_at: NOW + 100 * HOUR,
        }),
        observed_at: NOW - 10,
        source: ReadingSource::Statusline,
    }
}

/// Auto-switch's pick for a session on `work` at 95%, `spare` at
/// `spare_pct` resetting in `resets_in`, `cred` overflow, with the gate as
/// auto-switch loads it under `f`.
fn auto_switch_pick(f: &Fake, spare_pct: f64, resets_in: i64) -> Option<String> {
    let cfg = Config {
        enabled: true,
        threshold: 90,
        exclude: Vec::new(),
        overflow: vec![OverflowEntry::new(n("cred"), 1)],
    };
    let inputs = OverflowInputs::load_with_gate(&f.root, &cfg, f.gate(&[]), NOW);
    let profiles = vec![n("work"), n("spare"), n("cred")];
    let mut readings = BTreeMap::new();
    readings.insert("work".to_string(), sw(95.0, resets_in));
    readings.insert("spare".to_string(), sw(spare_pct, resets_in));
    readings.insert("cred".to_string(), sw(0.0, 5 * HOUR));
    let pool = Pool {
        profiles: &profiles,
        readings: &readings,
        threshold: cfg.threshold,
        exclude: &cfg.exclude,
        now: NOW,
    };
    select_with_overflow(&pool, &inputs.pool(), "work", |_| true).map(|p| p.to_string())
}

#[test]
fn auto_switch_applies_the_rotator_s_overflow_wait_not_the_default() {
    // Every plan profile is out and resets in an hour. With the default
    // 30-minute wait that is far enough to spend; the rotator was started
    // with --overflow-wait 7200, and no variable is set.
    let f = fake();
    assert_eq!(
        auto_switch_pick(&f, 99.0, HOUR).as_deref(),
        Some("cred"),
        "no rotator: the default wait"
    );
    f.rotator_runs(PID);
    gate_record::write(&f.rs, PID, &settings(7200, false)).unwrap();
    assert_eq!(
        auto_switch_pick(&f, 99.0, HOUR),
        None,
        "the rotator's two-hour wait"
    );
    assert_eq!(
        auto_switch_pick(&f, 99.0, 3 * HOUR).as_deref(),
        Some("cred"),
        "a reset beyond the rotator's wait"
    );
}

#[test]
fn auto_switch_closes_overflow_while_the_running_rotator_has_no_record() {
    let f = fake();
    f.rotator_runs(PID);
    assert_eq!(auto_switch_pick(&f, 99.0, 4 * HOUR), None);
}

// --- Agreement on when money is spent ---------------------------------------

fn probed(session: f64, resets_at: i64) -> Probed {
    let reading = if session >= 100.0 {
        Reading::Exhausted
    } else {
        Reading::Measured {
            session,
            week: 10.0,
        }
    };
    Probed {
        reading,
        windows: Windows {
            session: Some(Win {
                used: session,
                resets_at: Some(resets_at),
            }),
            week: Some(Win {
                used: 10.0,
                resets_at: Some(NOW + 100 * HOUR),
            }),
        },
        spend_limit: false,
    }
}

fn measured(name: &str, p: &Probed) -> Measured {
    Measured {
        name: n(name),
        reading: p.reading,
        windows: p.windows.clone(),
        spend_limit: false,
    }
}

/// Whether the rotator, with `work` live at `live`%, moves onto `cred`.
fn rotator_spends(s: &Settings, live: f64, spare: f64, resets_in: i64) -> bool {
    let live_p = probed(live, NOW + resets_in);
    let candidates = vec![
        measured("spare", &probed(spare, NOW + resets_in)),
        measured("cred", &probed(0.0, NOW + 5 * HOUR)),
    ];
    let v = decide_overflow(&OverflowInput {
        settings: s,
        live_name: Some(&n("work")),
        live: &live_p,
        candidates: &candidates,
        overflow: &[OverflowEntry::new(n("cred"), 1)],
        blocks: &BTreeMap::new(),
        swapped_at: None,
        now: NOW,
    });
    let spends = v.tag == Some(Tag::In);
    if spends {
        assert_eq!(
            v.decision,
            super::decide::Decision::Swap {
                to: n("cred"),
                reason: Reason::OverflowIn
            }
        );
    }
    spends
}

/// Whether auto-switch, for a session on `work` at `live`%, picks `cred`.
fn auto_switch_spends(g: Gate, live: f64, spare: f64, resets_in: i64) -> bool {
    let profiles = vec![n("work"), n("spare"), n("cred")];
    let mut readings = BTreeMap::new();
    readings.insert("work".to_string(), sw(live, resets_in));
    readings.insert("spare".to_string(), sw(spare, resets_in));
    readings.insert("cred".to_string(), sw(0.0, 5 * HOUR));
    let order = vec![n("cred")];
    let pool = Pool {
        profiles: &profiles,
        readings: &readings,
        threshold: 90,
        exclude: &[],
        now: NOW,
    };
    let ov = OverflowPool {
        order: &order,
        blocked: &[],
        gate: Some(g),
    };
    select_with_overflow(&pool, &ov, "work", |_| true).map(|p| p.to_string())
        == Some("cred".to_string())
}

/// The case the review raised: a plan login at 87% session. Neither side
/// counts it as plan capacity (below 85% session is), so both spend when
/// the plan reset is far, and neither does when it is near; then both use
/// that plan login rather than an ALERT.
#[test]
fn the_rotator_and_auto_switch_agree_across_the_85_to_90_band() {
    let s = settings(1800, false);
    let g = gate(&s);
    for live in [90.0, 95.0, 100.0] {
        for spare in [10.0, 80.0, 84.9, 85.0, 87.0, 89.9, 90.0, 95.0, 100.0] {
            for resets_in in [10 * 60, 4 * HOUR] {
                let r = rotator_spends(&s, live, spare, resets_in);
                let a = auto_switch_spends(g, live, spare, resets_in);
                assert_eq!(
                    r, a,
                    "live {live}%, spare {spare}%, reset in {resets_in}s: rotator {r}, auto-switch {a}"
                );
                let expected = spare >= 85.0 && resets_in > 1800;
                assert_eq!(
                    r, expected,
                    "live {live}%, spare {spare}%, reset in {resets_in}s"
                );
            }
        }
    }
}

#[test]
fn agreement_holds_with_non_default_rotator_thresholds() {
    let mut s = settings(600, false);
    s.session_at = 70;
    s.week_at = 80;
    let g = gate(&s);
    for spare in [60.0, 69.9, 70.0, 75.0, 89.0, 95.0] {
        for resets_in in [5 * 60, 2 * HOUR] {
            assert_eq!(
                rotator_spends(&s, 95.0, spare, resets_in),
                auto_switch_spends(g, 95.0, spare, resets_in),
                "spare {spare}%, reset in {resets_in}s"
            );
        }
    }
}
