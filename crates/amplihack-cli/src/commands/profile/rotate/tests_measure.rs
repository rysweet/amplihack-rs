//! `rotate::measure` and the decision rule on a failed or timed-out probe
//! (issue #1698).
//!
//! Contract:
//! - `measure::reading(who: &str, p: &Probe) -> Probed` is `pub(super)`.
//! - A probe that timed out, or failed with an error, is `Reading::Failed`:
//!   unknown, never 100%, never a target and never exhausted. Nothing about
//!   it is kept, so the next cycle probes it again.
//! - `Probed::unknown` is set for a timeout and for an error, except a
//!   refused profile directory, which is lasting; it is clear for every
//!   state the login reported (`not_logged_in`, `unparsed`, `spend_limit`,
//!   an `ok` with no week). `Probed::failed()` is unknown,
//!   `Probed::platform()` is not, and `Measured::new` copies the flag.
//! - `measure::measure_with` runs at most `bound` probes at once, the live
//!   one included, and a slot the live probe frees goes to the next
//!   candidate (fake `claude` scripts only; never a real one).

use super::decide::{Decision, Measured, Probed, Reading, Reason, Windows, best, decide};
use super::log::{Shown, msg};
use super::measure::reading;
use super::overflow::{plan_usable, regains};
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::usage_cmd::{ErrorReason, Probe};
use crate::commands::profile::usage_parse::{UsageReport, UsageState};

const NOW: i64 = 1_760_000_000;

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
    }
}

fn name(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn timed_out() -> Probe {
    Probe {
        report: UsageReport::empty(UsageState::Timeout),
        error: None,
        raw: None,
        source: None,
        direct_failure: None,
    }
}

fn errored() -> Probe {
    Probe {
        report: UsageReport::empty(UsageState::Error),
        error: Some(ErrorReason::ReadFailed),
        raw: None,
        source: None,
        direct_failure: None,
    }
}

fn measured(n: &str, p: &Probed) -> Measured {
    Measured::new(name(n), p)
}

fn failed_with(reason: ErrorReason) -> Probe {
    Probe {
        report: UsageReport::empty(UsageState::Error),
        error: Some(reason),
        raw: None,
        source: None,
        direct_failure: None,
    }
}

#[test]
fn a_timeout_or_an_error_report_is_unknown() {
    for state in [UsageState::Timeout, UsageState::Error] {
        let p = Probed::from_report(&UsageReport::empty(state));
        assert!(p.unknown, "{state:?}");
        assert_eq!(p.reading, Reading::Failed);
    }
}

#[test]
fn what_the_login_reported_is_never_unknown() {
    for state in [
        UsageState::NotLoggedIn,
        UsageState::Unparsed,
        UsageState::SpendLimit,
        UsageState::Ok,
        UsageState::SessionExhausted,
        UsageState::WeeklyExhausted,
    ] {
        let p = Probed::from_report(&UsageReport::empty(state));
        assert!(!p.unknown, "{state:?}");
    }
    // An `ok` with no week reads Failed, but is still what was reported.
    let no_week = Probed::from_report(&UsageReport::empty(UsageState::Ok));
    assert_eq!(no_week.reading, Reading::Failed);
}

#[test]
fn the_constructors_set_unknown_and_measured_copies_it() {
    assert!(Probed::failed().unknown);
    assert!(!Probed::platform().unknown);
    assert!(measured("slow", &Probed::failed()).unknown);
    assert!(!measured("api", &Probed::platform()).unknown);
}

#[test]
fn reading_marks_timeouts_and_transient_errors_unknown() {
    assert!(reading("slow", &timed_out()).unknown);
    for reason in [
        ErrorReason::StartFailed,
        ErrorReason::ReadFailed,
        ErrorReason::WaitFailed,
    ] {
        let p = reading("bad", &failed_with(reason));
        assert_eq!(p.reading, Reading::Failed);
        assert!(p.unknown, "{reason:?}");
    }
}

#[test]
fn a_refused_profile_directory_is_failed_and_unknown() {
    // The login behind it may have capacity, so it holds a move onto
    // overflow back in this cycle (issue #1698).
    let p = reading("odd", &failed_with(ErrorReason::ProfileRefused));
    assert_eq!(p.reading, Reading::Failed);
    assert!(p.unknown);
}

#[test]
fn a_timed_out_probe_reads_failed_with_no_windows() {
    let p = reading("slow", &timed_out());
    assert_eq!(p.reading, Reading::Failed);
    assert_eq!(p.windows, Windows::default());
    assert!(!p.spend_limit);
    assert_eq!(p.reading.headroom(), None, "unknown, not 0 and not 100");
}

#[test]
fn an_errored_probe_reads_failed() {
    assert_eq!(reading("bad", &errored()).reading, Reading::Failed);
}

#[test]
fn a_timed_out_profile_shows_failed_in_the_cycle_line() {
    let slow = reading("slow", &timed_out());
    let line = msg::cycle(
        Some(&name("work")),
        &Shown::from_reading(Reading::Measured {
            session: 40.0,
            week: 20.0,
        }),
        &[(name("slow"), Shown::of(&slow))],
        None,
    );
    assert_eq!(line, "cycle live=work work:s=40%,w=20% slow:failed");
    assert!(!line.contains("100%"), "{line}");
}

#[test]
fn a_timed_out_live_login_shows_failed_in_the_cycle_line() {
    let live = reading("~/.claude", &timed_out());
    assert_eq!(
        msg::cycle(Some(&name("work")), &Shown::of(&live), &[], None),
        "cycle live=work work:failed"
    );
}

#[test]
fn a_timed_out_candidate_is_never_the_target() {
    let slow = reading("slow", &timed_out());
    let cands = [measured("a-slow", &slow)];
    assert_eq!(best(&cands), None);
    // At a threshold with only an unknown candidate there is no swap.
    assert_eq!(
        decide(
            &settings(),
            &Reading::Measured {
                session: 95.0,
                week: 10.0
            },
            &cands,
            None,
            NOW
        ),
        Decision::Alert
    );
}

#[test]
fn a_timed_out_candidate_does_not_hide_a_usable_one() {
    let slow = reading("slow", &timed_out());
    let spare = Probed {
        reading: Reading::Measured {
            session: 10.0,
            week: 10.0,
        },
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
        still_running: false,
    };
    // "a-slow" sorts first, so a tie-break bug would pick it.
    let cands = [measured("a-slow", &slow), measured("b-spare", &spare)];
    assert_eq!(
        best(&cands).map(|(n, h)| (n.as_str(), h)),
        Some(("b-spare", 90))
    );
    assert_eq!(
        decide(
            &settings(),
            &Reading::Measured {
                session: 95.0,
                week: 10.0
            },
            &cands,
            None,
            NOW
        ),
        Decision::Swap {
            to: name("b-spare"),
            reason: Reason::Threshold
        }
    );
}

#[test]
fn a_timed_out_probe_is_not_treated_as_exhausted() {
    let slow = reading("slow", &timed_out());
    assert_ne!(slow.reading, Reading::Exhausted);
    // The overflow rule: no known reset, no plan capacity, no regain time.
    assert!(!plan_usable(&settings(), &slow.reading));
    assert_eq!(regains(&settings(), &slow.reading, &slow.windows), None);
}

// ---------------------------------------------------------------------------
// The bound counts the live probe. Each fake appends `S <who>` on start and
// `E <who>` on exit to a marks file; the live probe's `who` is `live`.
// ---------------------------------------------------------------------------

mod bound {
    use super::super::decide::{Probed, Reading};
    use super::super::measure::{Live, measure_with};
    use super::{NOW, name};
    use crate::binary_finder::BinaryInfo;
    use crate::commands::profile::probe_limits::ProbeMode;
    use crate::commands::profile::store::ProfileStore;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{Duration, Instant};

    struct Hosts {
        _dir: tempfile::TempDir,
        store: ProfileStore,
        bin: BinaryInfo,
        marks: PathBuf,
    }

    /// A store with `live` and `p0`..`p{n-1}`, and a fake that sleeps `secs`.
    fn hosts(n: usize, secs: f64) -> Hosts {
        hosts_with(n, secs, secs)
    }

    /// As [`hosts`], with the live probe sleeping `live_secs`.
    fn hosts_with(n: usize, secs: f64, live_secs: f64) -> Hosts {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path().join("root"));
        store.add(&name("live")).unwrap();
        for i in 0..n {
            store.add(&name(&format!("p{i}"))).unwrap();
        }
        let marks = dir.path().join("marks");
        let path = dir.path().join("claude");
        let body = format!(
            "#!/bin/sh\n\
             PATH=/usr/local/bin:/usr/bin:/bin; export PATH\n\
             [ \"$1\" = -p ] || exit 0\n\
             who=${{CLAUDE_CONFIG_DIR:-live}}; who=${{who##*/}}\n\
             echo \"S $who\" >> '{marks}'\n\
             case $who in live) sleep {live_secs} ;; *) sleep {secs} ;; esac\n\
             echo \"E $who\" >> '{marks}'\n\
             printf 'Current session: 37%% used · resets 4pm (UTC)\\nCurrent week (all models): 61%% used · resets Oct 22, 2am (UTC)\\n'\n",
            marks = marks.display()
        );
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        // Another test's fork can hold the write descriptor until its exec
        // (ETXTBSY); run the fake with no arguments until it starts.
        let start = Instant::now();
        while Command::new(&path).status().is_err() {
            assert!(start.elapsed() < Duration::from_secs(5), "fake never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
        let bin = BinaryInfo {
            name: "claude".into(),
            path,
            version: None,
        };
        Hosts {
            _dir: dir,
            store,
            bin,
            marks,
        }
    }

    fn run(h: &Hosts, live: Live<'_>, n: usize, bound: usize) -> (Probed, Vec<Probed>) {
        run_for(h, live, n, bound, Duration::from_secs(60))
    }

    fn run_for(
        h: &Hosts,
        live: Live<'_>,
        n: usize,
        bound: usize,
        timeout: Duration,
    ) -> (Probed, Vec<Probed>) {
        let cands: Vec<_> = (0..n).map(|i| name(&format!("p{i}"))).collect();
        let (live, others) = measure_with(
            &h.store,
            &h.bin,
            live,
            &cands,
            NOW,
            ProbeMode::Text,
            timeout,
            bound,
        );
        (live, others.into_iter().map(|(_, p)| p).collect())
    }

    /// The marks in order, and the most probes running at once.
    fn marks(h: &Hosts) -> (Vec<String>, usize) {
        let text = fs::read_to_string(&h.marks).unwrap();
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let peak = peak(&lines, |_| true);
        (lines, peak)
    }

    /// The most probes running at once among the marks `counted` keeps.
    fn peak(lines: &[String], counted: impl Fn(&str) -> bool) -> usize {
        let (mut running, mut peak) = (0usize, 0usize);
        for line in lines.iter().filter(|l| counted(l)) {
            if line.starts_with("S ") {
                running += 1;
                peak = peak.max(running);
            } else {
                running -= 1;
            }
        }
        assert_eq!(running, 0, "every probe ended: {lines:?}");
        peak
    }

    fn read(p: &Probed) -> bool {
        p.reading
            == Reading::Measured {
                session: 37.0,
                week: 61.0,
            }
    }

    #[test]
    fn the_live_probe_counts_against_the_bound() {
        let h = hosts(3, 0.5);
        let live = name("live");
        let (live, others) = run(&h, Live::Profile(&live), 3, 2);
        assert!(
            read(&live) && others.iter().all(read),
            "{live:?} {others:?}"
        );
        let (lines, peak) = marks(&h);
        assert_eq!(lines.len(), 8, "{lines:?}");
        assert_eq!(peak, 2, "four probes at a bound of 2: {lines:?}");
    }

    #[test]
    fn the_live_probes_slot_goes_to_a_candidate_when_it_ends() {
        // A quick live probe and four slow candidates at a bound of 2: once
        // the live probe ends, its slot runs candidates beside the other.
        let h = hosts_with(4, 0.5, 0.0);
        let live = name("live");
        let (live, others) = run(&h, Live::Profile(&live), 4, 2);
        assert!(
            read(&live) && others.iter().all(read),
            "{live:?} {others:?}"
        );
        let (lines, peak_all) = marks(&h);
        assert_eq!(peak_all, 2, "{lines:?}");
        let candidates = peak(&lines, |l| !l.ends_with(" live"));
        assert_eq!(candidates, 2, "two candidates at once: {lines:?}");
    }

    #[test]
    fn at_a_bound_of_one_the_live_probe_runs_first_and_alone() {
        let h = hosts(2, 0.2);
        let (live, others) = run(&h, Live::Unowned, 2, 1);
        assert!(
            read(&live) && others.iter().all(read),
            "{live:?} {others:?}"
        );
        let (lines, peak) = marks(&h);
        assert_eq!(peak, 1, "{lines:?}");
        assert_eq!(lines[..2], ["S live", "E live"]);
    }

    #[test]
    fn a_candidate_past_the_deadline_reads_failed_and_unknown() {
        // A quick live probe and a candidate that sleeps past a 1 s deadline.
        let h = hosts_with(1, 5.0, 0.0);
        let start = Instant::now();
        let (live, others) = run_for(&h, Live::Unowned, 1, 2, Duration::from_secs(1));
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "killed at the deadline"
        );
        assert!(read(&live) && !live.unknown, "{live:?}");
        assert_eq!(others[0].reading, Reading::Failed);
        assert!(others[0].unknown, "{:?}", others[0]);
    }

    #[test]
    fn an_api_key_live_login_leaves_the_whole_bound_to_the_candidates() {
        let h = hosts(2, 0.5);
        let (live, others) = run(&h, Live::Platform, 2, 2);
        assert_eq!(live, Probed::platform());
        assert!(others.iter().all(read), "{others:?}");
        let (lines, peak) = marks(&h);
        assert!(lines.iter().all(|l| !l.ends_with(" live")), "{lines:?}");
        assert_eq!(peak, 2, "{lines:?}");
    }
}

// ---------------------------------------------------------------------------
// Issue #1844: reads that outlast a cycle (AC9)
//
// Contract:
// - `inflight::InFlight<K, T>`: `new(max_age)`, `busy(&K)`, `len()`,
//   `start_all(jobs: Vec<(K, Job<T>)>, bound)` (one detached dispatcher,
//   at most `bound` jobs at once, a key already busy is skipped),
//   `collect()` (finished results, never blocks), `wait(until: Instant)`
//   (results until every job is done or `until`), `drop_stale(now)` (drops
//   and returns the keys in flight longer than `max_age`).
// - `inflight::max_age(timeout) = 2 * timeout + 5 s`.
// - `measure::cycle_deadline(start, interval)` is `start + interval - 2 s`,
//   and at least `start + 1 s`.
// - `measure::gather(&mut InFlight<K, Probe>, keys, make_job, Wait) ->
//   Vec<(K, Probed)>`, in the order of `keys`: collect, start jobs for the
//   keys with no result and nothing in flight, wait (`Wait::Until` or
//   `Wait::Full`), then read each key. A key still in flight reads
//   `Probed::failed()` with `still_running` set; its result is used by a
//   later call, and no second job is started for it meanwhile.
// ---------------------------------------------------------------------------

use super::inflight::{self, InFlight, Job};
use super::measure::{Wait, cycle_deadline, gather};
use crate::commands::profile::usage_parse::UsageWindow;
use crate::commands::profile::usage_source::Source;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn direct_probe(session: f64, week: f64) -> Probe {
    let w = |label: &str, p: f64| UsageWindow {
        label: label.into(),
        used_percentage: p,
        resets_text: None,
        resets_at: Some(NOW + 3600),
    };
    Probe {
        report: UsageReport {
            state: UsageState::Ok,
            session: Some(w("session", session)),
            week_all: Some(w("all models", week)),
            week_models: Vec::new(),
            session_limit: true,
        },
        error: None,
        raw: None,
        source: Some(Source::Direct),
        direct_failure: None,
    }
}

/// Jobs that sleep for the delay given per key and count their starts.
#[derive(Clone, Default)]
struct Jobs {
    delay: Arc<Mutex<HashMap<&'static str, Duration>>>,
    started: Arc<Mutex<HashMap<&'static str, usize>>>,
}

impl Jobs {
    fn set(&self, k: &'static str, d: Duration) {
        self.delay.lock().unwrap().insert(k, d);
    }

    fn started(&self, k: &'static str) -> usize {
        self.started.lock().unwrap().get(k).copied().unwrap_or(0)
    }

    fn make(&self) -> impl FnMut(&&'static str) -> Job<Probe> + '_ {
        move |k: &&'static str| {
            let k = *k;
            *self.started.lock().unwrap().entry(k).or_default() += 1;
            let d = self
                .delay
                .lock()
                .unwrap()
                .get(k)
                .copied()
                .unwrap_or_default();
            Box::new(move || {
                std::thread::sleep(d);
                direct_probe(10.0, 20.0)
            }) as Job<Probe>
        }
    }
}

fn by_key(v: Vec<(&'static str, Probed)>) -> HashMap<&'static str, Probed> {
    v.into_iter().collect()
}

#[test]
fn the_cycle_waits_until_two_seconds_before_the_interval_and_at_least_one() {
    let t = Instant::now();
    assert_eq!(
        cycle_deadline(t, Duration::from_secs(120)),
        t + Duration::from_secs(118)
    );
    assert_eq!(
        cycle_deadline(t, Duration::from_secs(3)),
        t + Duration::from_secs(1)
    );
    assert_eq!(
        cycle_deadline(t, Duration::from_secs(2)),
        t + Duration::from_secs(1)
    );
    assert_eq!(
        cycle_deadline(t, Duration::ZERO),
        t + Duration::from_secs(1)
    );
}

#[test]
fn reads_in_flight_are_dropped_after_twice_the_deadline_and_five_seconds() {
    assert_eq!(
        inflight::max_age(Duration::from_secs(120)),
        Duration::from_secs(245)
    );
    assert_eq!(
        inflight::max_age(Duration::from_secs(1)),
        Duration::from_secs(7)
    );
}

#[test]
fn a_stalled_read_is_still_running_then_used_by_the_next_cycle() {
    let jobs = Jobs::default();
    jobs.set("spare", Duration::from_millis(1500));
    let mut fl: InFlight<&'static str, Probe> = InFlight::new(Duration::from_secs(60));
    let keys = ["work", "spare"];

    // Cycle 1: `work` answers at once; `spare` stalls past the wait.
    let start = Instant::now();
    let got = by_key(gather(
        &mut fl,
        &keys,
        jobs.make(),
        Wait::Until(start + Duration::from_millis(400)),
    ));
    assert!(
        start.elapsed() < Duration::from_millis(1200),
        "did not wait for the stall"
    );
    assert_eq!(
        got["work"].reading,
        Reading::Measured {
            session: 10.0,
            week: 20.0
        }
    );
    assert!(!got["work"].still_running);
    assert_eq!(got["spare"].reading, Reading::Failed);
    assert!(got["spare"].unknown, "blocks overflow as a timeout does");
    assert!(got["spare"].still_running);
    assert_eq!(
        msg::cycle(
            Some(&name("work")),
            &Shown::of(&got["work"]),
            &[(name("spare"), Shown::of(&got["spare"]))],
            None
        ),
        "cycle live=work work:s=10%,w=20% spare:unknown, still running"
    );
    assert!(fl.busy(&"spare"));

    // Cycle 2, at once: `spare` is still in flight, so no second read.
    let got = by_key(gather(
        &mut fl,
        &keys,
        jobs.make(),
        Wait::Until(Instant::now() + Duration::from_millis(100)),
    ));
    assert!(got["spare"].still_running);
    assert_eq!(
        jobs.started("spare"),
        1,
        "no second request while one is outstanding"
    );
    assert_eq!(
        jobs.started("work"),
        2,
        "a login with a result is read again"
    );

    // Cycle 3, after the stall: the result is collected and used.
    std::thread::sleep(Duration::from_millis(1600));
    let got = by_key(gather(
        &mut fl,
        &keys,
        jobs.make(),
        Wait::Until(Instant::now() + Duration::from_millis(100)),
    ));
    assert_eq!(
        got["spare"].reading,
        Reading::Measured {
            session: 10.0,
            week: 20.0
        }
    );
    assert!(!got["spare"].still_running);
    assert_eq!(
        jobs.started("spare"),
        1,
        "the result in hand is used, not read again"
    );
    assert!(!fl.busy(&"spare"));
}

#[test]
fn wait_full_waits_for_every_read() {
    let jobs = Jobs::default();
    jobs.set("spare", Duration::from_millis(600));
    let mut fl: InFlight<&'static str, Probe> = InFlight::new(Duration::from_secs(60));
    let got = by_key(gather(&mut fl, &["work", "spare"], jobs.make(), Wait::Full));
    assert!(!got["spare"].still_running);
    assert_eq!(
        got["spare"].reading,
        Reading::Measured {
            session: 10.0,
            week: 20.0
        }
    );
    assert_eq!(fl.len(), 0);
}

#[test]
fn a_read_in_flight_too_long_is_dropped_and_started_again() {
    let jobs = Jobs::default();
    jobs.set("spare", Duration::from_secs(3));
    let mut fl: InFlight<&'static str, Probe> = InFlight::new(Duration::from_millis(200));
    gather(
        &mut fl,
        &["spare"],
        jobs.make(),
        Wait::Until(Instant::now() + Duration::from_millis(50)),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(fl.drop_stale(Instant::now()), ["spare"]);
    assert!(!fl.busy(&"spare"));
    gather(
        &mut fl,
        &["spare"],
        jobs.make(),
        Wait::Until(Instant::now() + Duration::from_millis(50)),
    );
    assert_eq!(jobs.started("spare"), 2);
}

#[test]
fn start_all_runs_at_most_bound_jobs_at_once() {
    let running = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut fl: InFlight<usize, usize> = InFlight::new(Duration::from_secs(60));
    let jobs: Vec<(usize, Job<usize>)> = (0..8)
        .map(|i| {
            let running = Arc::clone(&running);
            let peak = Arc::clone(&peak);
            let job: Job<usize> = Box::new(move || {
                let now = running.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                peak.fetch_max(now, AtomicOrdering::SeqCst);
                std::thread::sleep(Duration::from_millis(100));
                running.fetch_sub(1, AtomicOrdering::SeqCst);
                i
            });
            (i, job)
        })
        .collect();
    fl.start_all(jobs, 3);
    let mut got = fl.wait(Instant::now() + Duration::from_secs(5));
    got.sort_unstable();
    assert_eq!(got, (0..8).map(|i| (i, i)).collect::<Vec<_>>());
    assert!(peak.load(AtomicOrdering::SeqCst) <= 3, "{peak:?}");
    assert!(
        peak.load(AtomicOrdering::SeqCst) >= 2,
        "jobs ran in parallel"
    );
}

#[test]
fn a_busy_key_is_not_started_twice() {
    let mut fl: InFlight<&'static str, u8> = InFlight::new(Duration::from_secs(60));
    let n = Arc::new(AtomicUsize::new(0));
    let job = |n: &Arc<AtomicUsize>| -> Job<u8> {
        let n = Arc::clone(n);
        Box::new(move || {
            n.fetch_add(1, AtomicOrdering::SeqCst);
            std::thread::sleep(Duration::from_millis(300));
            1
        })
    };
    fl.start_all(vec![("a", job(&n))], 4);
    fl.start_all(vec![("a", job(&n))], 4);
    let got = fl.wait(Instant::now() + Duration::from_secs(5));
    assert_eq!(got, [("a", 1)]);
    assert_eq!(n.load(AtomicOrdering::SeqCst), 1);
}
