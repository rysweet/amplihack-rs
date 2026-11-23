//! Issue #3105, pure units: a standing block rules a login out before any
//! reading is looked at (`target::fit_blocked`), one test says whether a
//! block stands (`target::standing_block`), a kept reading is never
//! replaced by an older one (`Stored::keep`), a reading older than one
//! already held is superseded (`Extra::older_by`, `substitute`,
//! `reading_used`), the ledger written for an older reader keeps
//! `overflow_blocked` (`for_older_readers`), and the two new `reject:` lines
//! carry only a name, a time, a fixed label and a number.
//!
//! P1 of the 23 November incident is reproduced on the pure seams: after
//! the binary is replaced in place, a running watch counts as a rotator of
//! another binary, both to a step on the new binary and to itself (its
//! executable path then ends in " (deleted)"), so every save writes the
//! older-reader ledger; before this change that dropped every block.
//!
//! Contract:
//!
//! ```text
//! type Block = (i64 /* until */, &'static str /* kind_label */)
//! Unfit::Blocked { until: i64, kind: &'static str }   // first variant
//! fit_blocked(m: &Measured, block: Option<Block>, week_max: u8) -> Result<u8, Unfit>
//! standing_block(blocks, name, now) -> Option<Block>   // now < until, fixed label
//! Stored::times(now: i64) -> Vec<(ProfileName, i64)>   // kept times not after now
//! Extra::older_by(name, at) -> Option<u64>             // strictly older only
//! target_msg::blocked(&Seen, &Decision) -> Vec<String> // none for a stay
//! ```
//!
//! Stand-in logins only, in temporary folders made 0700; no real home,
//! login, process or rotator is touched, and the fake process uses a pid
//! above any the kernel gives.

use super::{Unfit, fit, fit_blocked, standing_block};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::cycle::event_read::Stored;
use crate::commands::profile::rotate::decide::last_good::LastGood;
use crate::commands::profile::rotate::decide::target_confirm::Why;
use crate::commands::profile::rotate::decide::target_msg::{blocked, reject, utc_minute};
use crate::commands::profile::rotate::decide::target_seen::{
    Basis, Extra, Seen, Standing, reading_used,
};
use crate::commands::profile::rotate::decide::{
    Decision, Measured, Probed, Reading, Reason, Win, Windows,
};
use crate::commands::profile::rotate::ledger::{OverflowBlock, RotateLedger, RotateState};
use crate::commands::profile::rotate::measure::ProbeLabel;
use crate::commands::profile::rotate::older_readers::for_older_readers;
use crate::commands::profile::rotate::spend_blocks::kind_label;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const NOW: i64 = 1_795_414_578;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn probe(session: f64, week: f64, observed_at: Option<i64>) -> Probed {
    let win = |used| Win {
        used,
        resets_at: Some(NOW + 3 * 86_400),
    };
    Probed {
        reading: Reading::Measured { session, week },
        windows: Windows {
            session: Some(win(session)),
            week: Some(win(week)),
            no_session_limit: false,
        },
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at,
        carried: false,
        passive: false,
    }
}

fn kept(s: &Stored, who: &str) -> Option<f64> {
    match s.usable(&n(who), NOW, 5 * 3600)?.reading {
        Reading::Measured { week, .. } => Some(week),
        _ => None,
    }
}

// --- U1: the block is the first part of the rule ------------------------------

#[test]
fn a_block_in_force_rules_out_a_roomy_reading_for_every_kind() {
    let m = Measured::new(n("held"), &probe(0.0, 33.0, Some(NOW)));
    for kind in [
        "spend_limit",
        "session_limit",
        "week_limit",
        "credit_limit",
        "something_else",
    ] {
        let label = kind_label(kind);
        assert_eq!(
            fit_blocked(&m, Some((NOW + 60, label)), 95),
            Err(Unfit::Blocked {
                until: NOW + 60,
                kind: label
            }),
            "{kind}"
        );
    }
}

#[test]
fn a_block_wins_over_every_other_reason_to_reject() {
    let mut m = Measured::new(n("held"), &probe(100.0, 100.0, Some(NOW)));
    m.spend_limit = true;
    let b = Some((NOW + 1, "spend limit"));
    assert!(matches!(fit_blocked(&m, b, 95), Err(Unfit::Blocked { .. })));
}

#[test]
fn with_no_block_fit_blocked_is_fit() {
    for (s, w) in [(0.0, 33.0), (96.0, 10.0), (100.0, 1.0), (10.0, 96.0)] {
        let m = Measured::new(n("held"), &probe(s, w, Some(NOW)));
        assert_eq!(fit_blocked(&m, None, 95), fit(&m, 95), "{s}/{w}");
    }
}

// --- U3: a kept reading is never replaced by an older one ----------------------

#[test]
fn an_older_reading_does_not_replace_a_newer_kept_one() {
    let mut st = Stored::default();
    let newer = (probe(100.0, 44.0, Some(NOW - 120)), vec![]);
    st.keep(Some(&n("work")), Some(&newer), NOW - 120);
    let older = (
        probe(0.0, 33.0, Some(NOW - 2000)),
        vec![(n("held"), probe(0.0, 33.0, Some(NOW - 2000)))],
    );
    st.keep(None, Some(&older), NOW);
    assert_eq!(kept(&st, "held"), Some(33.0), "first reading of a login");
    let newest = (
        probe(0.0, 0.0, None),
        vec![(n("held"), probe(100.0, 44.0, Some(NOW - 120)))],
    );
    st.keep(None, Some(&newest), NOW);
    let stale = (
        probe(0.0, 0.0, None),
        vec![(n("held"), probe(0.0, 33.0, Some(NOW - 2000)))],
    );
    st.keep(None, Some(&stale), NOW);
    assert_eq!(kept(&st, "held"), Some(44.0), "the newer reading stays");
}

#[test]
fn an_equal_time_replaces_the_held_reading() {
    let mut st = Stored::default();
    let a = (
        probe(0.0, 0.0, None),
        vec![(n("held"), probe(1.0, 10.0, Some(NOW - 60)))],
    );
    st.keep(None, Some(&a), NOW);
    let b = (
        probe(0.0, 0.0, None),
        vec![(n("held"), probe(2.0, 20.0, Some(NOW - 60)))],
    );
    st.keep(None, Some(&b), NOW);
    assert_eq!(kept(&st, "held"), Some(20.0));
}

#[test]
fn a_held_time_after_the_measuring_time_is_replaced_and_times_drops_it() {
    // The clock stepped back: the held time is in the future of this cycle.
    let mut st = Stored::default();
    let future = (
        probe(0.0, 0.0, None),
        vec![(n("held"), probe(1.0, 10.0, Some(NOW + 5000)))],
    );
    st.keep(None, Some(&future), NOW + 5000);
    assert!(
        st.times(NOW).iter().all(|(who, _)| who != &n("held")),
        "times(now) leaves out a time after now"
    );
    let real = (
        probe(0.0, 0.0, None),
        vec![(n("held"), probe(2.0, 20.0, Some(NOW - 10)))],
    );
    st.keep(None, Some(&real), NOW);
    assert!(
        st.times(NOW)
            .iter()
            .any(|(who, at)| who == &n("held") && *at == NOW - 10)
    );
}

#[test]
fn a_failed_probe_never_moves_the_kept_time_forward() {
    let mut st = Stored::default();
    let good = (
        probe(0.0, 0.0, None),
        vec![(n("held"), probe(1.0, 10.0, Some(NOW - 600)))],
    );
    st.keep(None, Some(&good), NOW - 600);
    let failed = (probe(0.0, 0.0, None), vec![(n("held"), Probed::failed())]);
    st.keep(None, Some(&failed), NOW);
    let t = st.times(NOW);
    assert_eq!(
        t.iter().find(|(who, _)| who == &n("held")).map(|x| x.1),
        Some(NOW - 600)
    );
}

// --- U5: the ledger for an older reader keeps the block ---------------------------

#[test]
fn a_ledger_for_an_older_reader_keeps_overflow_blocked() {
    let mut l = RotateLedger::default();
    l.overflow_blocked.insert(
        "held".into(),
        OverflowBlock {
            at: NOW - 3600,
            until: NOW + 3600,
            reason: "spend_limit".into(),
        },
    );
    l.swapped_away.insert("held".into(), NOW - 9);
    let out = for_older_readers(&l, true);
    assert_eq!(out.overflow_blocked, l.overflow_blocked, "the block stays");
    assert!(
        out.swapped_away.is_empty(),
        "other fields are still cleared"
    );
    assert_eq!(for_older_readers(&l, false), l);
}

// --- Expiry: until == now is over, until == now + 1 stands --------------------------

#[test]
fn a_block_ends_exactly_at_its_recorded_time() {
    use crate::commands::profile::rotate::spend_blocks::active_blocks;
    let mut l = RotateLedger::default();
    let b = OverflowBlock {
        at: NOW - 60,
        until: NOW,
        reason: "spend_limit".into(),
    };
    l.overflow_blocked.insert("held".into(), b.clone());
    let held = n("held");
    let stands = |l: &RotateLedger, now| standing_block(&l.overflow_blocked, &held, now);
    assert!(active_blocks(&l, NOW).is_empty(), "until == now is expired");
    assert_eq!(stands(&l, NOW), None, "until == now is expired");
    assert_eq!(active_blocks(&l, NOW - 1).len(), 1);
    assert_eq!(stands(&l, NOW - 1), Some((NOW, "spend limit")));
    l.overflow_blocked.insert(
        "held".into(),
        OverflowBlock {
            until: NOW + 1,
            ..b.clone()
        },
    );
    assert_eq!(active_blocks(&l, NOW).len(), 1, "until == now + 1 stands");
    assert_eq!(stands(&l, NOW), Some((NOW + 1, "spend limit")));
    assert_eq!(
        standing_block(&l.overflow_blocked, &n("free"), NOW),
        None,
        "a login with no entry has no block"
    );
    // The kind is the fixed label, never the stored text.
    l.overflow_blocked.insert(
        "held".into(),
        OverflowBlock {
            until: NOW + 1,
            reason: "made_up_kind".into(),
            ..b
        },
    );
    assert_eq!(stands(&l, NOW), Some((NOW + 1, "a recorded limit")));
}

// --- U4: a reading older than one already held is superseded -------------------------

fn extra_with(newest: i64, last_good_at: Option<i64>) -> Extra {
    Extra {
        labels: BTreeMap::from([(n("held"), ProbeLabel::Http429)]),
        last_good: last_good_at.map(|at| {
            BTreeMap::from([(
                n("held"),
                LastGood {
                    reading: Reading::Measured {
                        session: 0.0,
                        week: 33.0,
                    },
                    at,
                },
            )])
        }),
        newest: BTreeMap::from([(n("held"), newest)]),
        ..Extra::default()
    }
}

#[test]
fn older_by_is_some_only_for_a_strictly_older_reading() {
    let x = extra_with(NOW - 120, None);
    assert_eq!(x.older_by(&n("held"), NOW - 2000), Some(1880));
    assert_eq!(x.older_by(&n("held"), NOW - 121), Some(1));
    assert_eq!(
        x.older_by(&n("held"), NOW - 120),
        None,
        "equal is not older"
    );
    assert_eq!(x.older_by(&n("held"), NOW), None);
    assert_eq!(x.older_by(&n("other"), i64::MIN), None, "nothing held");
    assert_eq!(
        Extra {
            newest: BTreeMap::from([(n("held"), i64::MAX)]),
            ..Extra::default()
        }
        .older_by(&n("held"), i64::MIN),
        Some(u64::MAX),
        "no overflow at the extremes"
    );
}

/// The 06:18:21Z reading: the probe failed, and the history's last good
/// point (0/33 from 05:44:55Z) is older than the 06:16:18Z reading the watch
/// kept, so it does not stand in.
#[test]
fn a_history_point_older_than_the_kept_reading_does_not_stand_in() {
    let failed = Probed::failed();
    let x = extra_with(NOW - 123, Some(NOW - 2006));
    assert!(x.substitute(&n("held"), &failed).is_none());
    assert_eq!(reading_used(&x, &n("held"), &failed), Reading::Failed);
    // The same point with nothing newer held still stands in.
    let x = extra_with(NOW - 2006, Some(NOW - 2006));
    assert!(x.substitute(&n("held"), &failed).is_some());
}

#[test]
fn a_passive_reading_older_than_the_kept_one_is_superseded() {
    let x = extra_with(NOW - 30, None);
    let passive = probe(0.0, 33.0, Some(NOW - 200));
    assert_eq!(reading_used(&x, &n("held"), &passive), Reading::Failed);
    let direct = probe(0.0, 33.0, None);
    assert_eq!(reading_used(&x, &n("held"), &direct), direct.reading);
    let newer = probe(0.0, 33.0, Some(NOW - 10));
    assert_eq!(reading_used(&x, &n("held"), &newer), newer.reading);
}

#[test]
fn a_carried_kept_reading_is_never_superseded_by_itself() {
    let mut st = Stored::default();
    let read = (
        probe(0.0, 0.0, None),
        vec![(n("held"), probe(10.0, 20.0, Some(NOW - 30)))],
    );
    st.keep(None, Some(&read), NOW - 30);
    let carried = st.usable(&n("held"), NOW, 600).expect("kept");
    let x = Extra {
        newest: st.times(NOW).into_iter().collect(),
        ..Extra::default()
    };
    assert_eq!(reading_used(&x, &n("held"), &carried), carried.reading);
}

// --- U6: P1, the block lost by a save beside a replaced binary ---------------------------

/// Above `PID_MAX_LIMIT`: never a real process.
const FAKE_PID: u32 = 4_200_000;

fn private_dir(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

fn private_file(p: &Path, body: &[u8]) {
    private_dir(p.parent().unwrap());
    fs::write(p, body).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn after_an_in_place_upgrade_the_watch_counts_as_foreign_to_everyone_and_the_block_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    private_dir(tmp.path());
    let (root, bins, proc_root) = (
        tmp.path().join("root"),
        tmp.path().join("bin"),
        tmp.path().join("proc"),
    );
    private_dir(&root);
    private_dir(&bins);
    private_dir(&proc_root);
    let exe = bins.join("amplihack");
    private_file(&exe, b"build 1.2.206");
    // The watch started from `exe`: its /proc entry links the old file.
    let entry = proc_root.join(FAKE_PID.to_string());
    private_dir(&entry);
    private_file(
        &entry.join("cmdline"),
        b"/opt/bin/amplihack\0profile\0rotate\0watch\0",
    );
    fs::hard_link(&exe, entry.join("exe")).unwrap();
    let rs = RotateState::new(&root);
    private_file(&rs.pid, format!("{FAKE_PID}\n").as_bytes());
    assert_eq!(
        rs.foreign_rotator_at(&proc_root, Some(&exe)),
        None,
        "same build"
    );
    // The upgrade writes the new build beside it and renames it over `exe`.
    let staged = bins.join("amplihack.new");
    private_file(&staged, b"build 1.2.207");
    fs::rename(&staged, &exe).unwrap();
    assert_eq!(
        rs.foreign_rotator_at(&proc_root, Some(&exe)),
        Some(FAKE_PID),
        "a step on the new build sees the watch as foreign"
    );
    let deleted = bins.join("amplihack (deleted)");
    assert_eq!(
        rs.foreign_rotator_at(&proc_root, Some(&deleted)),
        Some(FAKE_PID),
        "the watch sees itself as foreign"
    );
    // So `save_ledger` writes the older-reader form, which keeps the block.
    let mut l = RotateLedger::default();
    let block = OverflowBlock {
        at: NOW - 1077,
        until: NOW + 15_822,
        reason: "spend_limit".into(),
    };
    l.overflow_blocked.insert("held".into(), block);
    let older = rs.foreign_rotator_at(&proc_root, Some(&deleted)).is_some();
    assert_eq!(
        for_older_readers(&l, older).overflow_blocked,
        l.overflow_blocked
    );
}

// --- The two new reject: lines ------------------------------------------------------------

#[test]
fn the_block_reject_line_holds_the_name_a_minute_and_the_fixed_label() {
    for kind in ["spend_limit", "week_limit", "x_unheard_of"] {
        let label = kind_label(kind);
        let why = Why::Unfit(Unfit::Blocked {
            until: NOW,
            kind: label,
        });
        assert_eq!(
            reject(&n("held"), &why, 95),
            format!("reject: held blocked until {} ({label})", utc_minute(NOW))
        );
    }
    assert_eq!(
        kind_label("x_unheard_of"),
        "a recorded limit",
        "no stored text"
    );
}

#[test]
fn the_superseded_reject_line_holds_the_name_and_a_number() {
    assert_eq!(
        reject(&n("held"), &Why::Older(140), 95),
        "reject: held reading 140s older than one already held"
    );
}

fn seen_with(blocks: &[(&str, Option<(i64, &'static str)>)]) -> Seen {
    let logins = blocks
        .iter()
        .map(|(who, block)| {
            let fit = match block {
                Some((until, kind)) => Err(Unfit::Blocked {
                    until: *until,
                    kind,
                }),
                None => Ok(40),
            };
            (
                n(who),
                Standing {
                    basis: Basis::Probe,
                    fit,
                    block: *block,
                },
            )
        })
        .collect();
    Seen {
        logins,
        week_max: 95,
        ..Seen::default()
    }
}

#[test]
fn one_block_line_per_blocked_login_in_name_order_and_none_for_a_stay() {
    let seen = seen_with(&[
        ("zed", Some((NOW + 60, "week limit"))),
        ("free", None),
        ("held", Some((NOW + 60, "spend limit"))),
    ]);
    let swap = Decision::Swap {
        to: n("free"),
        reason: Reason::Threshold,
    };
    for d in [&swap, &Decision::Alert] {
        let lines = blocked(&seen, d);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].starts_with("reject: held blocked until "),
            "{lines:?}"
        );
        assert!(
            lines[1].starts_with("reject: zed blocked until "),
            "{lines:?}"
        );
        assert!(lines[1].ends_with("(week limit)"), "{lines:?}");
    }
    assert!(blocked(&seen, &Decision::Stay).is_empty());
}
