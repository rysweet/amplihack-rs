//! `rotate::measure` and the decision rule on a failed or timed-out probe
//! (issue #1698).
//!
//! Contract:
//! - `measure::reading(who: &str, p: &Probe) -> Probed` is `pub(super)`.
//! - A probe that timed out, or failed with an error, is `Reading::Failed`:
//!   unknown, never 100%, never a target and never exhausted. Nothing about
//!   it is kept, so the next cycle probes it again.
//! - `Reading::Exhausted { session, week }` carries the windows it read and
//!   still has headroom `Some(0)` when they are missing.

use super::decide::{Decision, Measured, Probed, Reading, Reason, Windows, best, decide};
use super::log::msg;
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
    }
}

fn errored() -> Probe {
    Probe {
        report: UsageReport::empty(UsageState::Error),
        error: Some(ErrorReason::ReadFailed),
        raw: None,
    }
}

fn measured(n: &str, p: &Probed) -> Measured {
    Measured::new(name(n), p)
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
        &Reading::Measured {
            session: 40.0,
            week: 20.0,
        },
        &[(name("slow"), slow.reading)],
        None,
    );
    assert_eq!(line, "cycle live=work work:s=40%,w=20% slow:failed");
    assert!(!line.contains("100%"), "{line}");
}

#[test]
fn a_timed_out_live_login_shows_failed_in_the_cycle_line() {
    let live = reading("~/.claude", &timed_out());
    assert_eq!(
        msg::cycle(Some(&name("work")), &live.reading, &[], None),
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
    assert!(!matches!(slow.reading, Reading::Exhausted { .. }));
    // The overflow rule: no known reset, no plan capacity, no regain time.
    assert!(!plan_usable(&settings(), &slow.reading));
    assert_eq!(regains(&settings(), &slow.reading, &slow.windows), None);
}

#[test]
fn an_exhausted_reading_with_no_windows_still_has_no_headroom() {
    let r = Reading::Exhausted {
        session: None,
        week: None,
    };
    assert_eq!(r.headroom(), Some(0));
    let p = Probed {
        reading: r,
        windows: Windows::default(),
        spend_limit: false,
    };
    assert_eq!(best(&[measured("gone", &p)]), None);
}

#[test]
fn the_exhausted_test_constant_reads_one_hundred_in_both_windows() {
    assert_eq!(
        Reading::EXHAUSTED,
        Reading::Exhausted {
            session: Some(100.0),
            week: Some(100.0)
        }
    );
}
