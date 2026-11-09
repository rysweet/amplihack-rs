//! Issue #2013 (D8, D11): a home account repair whose write is refused.
//!
//! Contract:
//! - When the evidence agrees but `write_home_account` fails,
//!   `home_repair::step` returns `Repair::Refused`, writes nothing to
//!   `~/.claude.json`, and records no `repair:<p>` entry.
//! - It logs `repair refused: ~/.claude.json was not rewritten: <error>`,
//!   limited by the alert limit under `repair-refused:<p>`: within an hour
//!   the line is written once.
//!
//! The write is made to fail by a `$HOME` the user cannot write to, so the
//! file still reads and the evidence agrees. (A symlinked `~/.claude.json`
//! never reaches the write: `plan` already finds it unusable.)

use super::cycle::{Ctx, Mode, run_cycle};
use super::home_repair::{Repair, step};
use super::log::Sink;
use super::observe::{Measurement, Snap, decide_measured, snapshot};
use super::tests_issue_2013_common::{
    ctx_at, home_block, home_uuid, lines, rd, record, set_now, settings,
};
use super::tests_revalidate_cause::probes_for;
use super::tests_swap::{Env, env};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{State, now};
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn fx() -> Env {
    let e = env();
    record(&e.root, "work", "u-work", "work@example.com");
    home_block(&e.home, Some(("u-spare", "spare@example.com")));
    e
}

fn measure_stay(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(snap, &[("spare", rd(10.0, 10.0, None))]);
    decide_measured(ctx, snap, rd(10.0, 10.0, None), probes, at)
}

fn root_user() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// Runs `f` with `$HOME` read-only, and restores its mode afterwards.
fn with_read_only_home<T>(e: &Env, f: impl FnOnce() -> T) -> T {
    fs::set_permissions(&e.home, fs::Permissions::from_mode(0o500)).unwrap();
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    fs::set_permissions(&e.home, fs::Permissions::from_mode(0o700)).unwrap();
    out.unwrap_or_else(|p| std::panic::resume_unwind(p))
}

fn refused(e: &Env) -> Vec<String> {
    lines(e)
        .into_iter()
        .filter(|l| l.starts_with("repair refused: "))
        .collect()
}

#[cfg(target_os = "linux")]
#[test]
fn a_refused_write_returns_refused_and_writes_nothing() {
    if root_user() {
        return; // root ignores mode bits
    }
    let e = fx();
    let path = e.home.join(".claude.json");
    let before = fs::read(&path).unwrap();
    let c = ctx_at(&e, measure_stay, settings(), now());
    let state = State::new(&e.root).unwrap();
    let snap = snapshot(&c, &state).unwrap();
    let mut out = Sink::new(None, false, false);
    let got = with_read_only_home(&e, || step(&c, &state, &snap, &mut out).unwrap());
    assert_eq!(got, Repair::Refused);
    assert_eq!(fs::read(&path).unwrap(), before);
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert!(
        !l.alert_log.contains_key("repair:work"),
        "{:?}",
        l.alert_log
    );
    assert!(
        l.alert_log.contains_key("repair-refused:work"),
        "{:?}",
        l.alert_log
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_refused_repair_is_logged_once_an_hour() {
    if root_user() {
        return; // root ignores mode bits
    }
    let e = fx();
    let t = now();
    let mut c = ctx_at(&e, measure_stay, settings(), t);
    with_read_only_home(&e, || {
        for at in [t, t + 120, t + 240] {
            set_now(at);
            run_cycle(&mut c, Mode::Watch).unwrap();
        }
    });
    assert_eq!(
        home_uuid(&e.home).as_deref(),
        Some("u-spare"),
        "never rewritten"
    );
    let got = refused(&e);
    assert_eq!(got.len(), 1, "{:?}", lines(&e));
    assert!(
        got[0].starts_with("repair refused: ~/.claude.json was not rewritten: "),
        "{got:?}"
    );
    assert!(
        !lines(&e).iter().any(|l| l.starts_with("repair:")),
        "{:?}",
        lines(&e)
    );
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.alert_log["repair-refused:work"].suppressed, 2);
    assert!(!l.alert_log.contains_key("repair:work"));
}
