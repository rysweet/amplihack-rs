//! Issue #1846: an exhausted login keeps the figures it was read with.
//!
//! `amplihack profile rotate` runs once against a temporary HOME and
//! profile root and a fake `claude` that prints canned `/usage` output.
//! A login at its session limit is logged with its measured weekly figure
//! (`s=100% w=62%`), not `w=100%`, and is still never chosen as the swap
//! target; a login at its weekly limit is logged with `w=100%` and the
//! session it read (`s=30% w=100%`).

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;

fn lines(out: &std::process::Output) -> Vec<String> {
    text(&out.stdout).lines().map(str::to_string).collect()
}

#[test]
fn a_session_exhausted_live_login_logs_its_weekly_figure() {
    let f = Fixture::new();
    f.usage("work", 100, 62);
    let out = f.ok(&["profile", "rotate"]);
    let ls = lines(&out);
    assert!(
        ls.iter()
            .any(|l| l.contains("swap work->spare live s=100% w=62% target s=10% w=20%")),
        "{ls:?}"
    );
    assert!(
        ls.iter()
            .any(|l| l.contains("cycle live=work work:s=100%,w=62%")),
        "{ls:?}"
    );
    assert!(!f.log().contains("w=100%"), "{}", f.log());
}

#[test]
fn an_exhausted_candidate_is_logged_as_read_and_never_chosen() {
    let f = Fixture::new();
    // spare has the most weekly room but no session room; extra is at its
    // weekly limit. Only spare and extra are candidates, so neither may be
    // chosen and the live login stays.
    f.usage("spare", 100, 5);
    f.usage("extra", 30, 100);
    let out = f.ok(&["profile", "rotate"]);
    let ls = lines(&out);
    let cycle = ls
        .iter()
        .find(|l| l.contains("cycle live=work"))
        .unwrap_or_else(|| panic!("no cycle line: {ls:?}"));
    assert!(cycle.contains("spare:s=100%,w=5%"), "{cycle}");
    assert!(cycle.contains("extra:s=30%,w=100%"), "{cycle}");
    assert!(
        !ls.iter().any(|l| l.contains(" swap ")),
        "an exhausted login was chosen: {ls:?}"
    );
    assert_eq!(f.home_account(), "uuid-work");
}
