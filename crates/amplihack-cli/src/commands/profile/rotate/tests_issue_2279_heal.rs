//! Issue #2279 (C8, D6, C11): the rotator and the hooks block.
//!
//! (b) The real swap and stay code paths, under a temp HOME and profile root
//!     (`tests_swap::env()`), leave every settings.json byte-equal: the cycle
//!     itself never writes a settings file.
//! (heal) `hook_heal::heal` runs after a `Swapped` or `Stayed` cycle, writes
//!     `~/.claude/settings.json` and the live profile's settings.json only on
//!     drift, and skips on `read_only`, a newer install stamp, no target, or
//!     any other outcome. A refused live profile does not stop the home heal.
//!     It never creates a file.
//! (C11) A stay on a fresh live reading while a candidate's probe failed
//!     records `stayed`, which the limit-resume hook reads as capacity.
//!
//! Probing is stubbed with `Ctx::measure`; no login, `claude`, network or
//! developer profile root is touched. No sleeps.

use super::super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::super::decide::Probed;
use super::super::hook_heal::{self, HealCtx};
use super::super::observe::{Measurement, Snap, decide_measured};
use super::super::tests_issue_1870::rd;
use super::super::tests_revalidate_cause::{ctx_with, log_of, probes_for};
use super::super::tests_swap::{Env, env};
use crate::commands::install::hook_block::{self, HookTarget};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use amplihack_utils::rotator_status::parse_cycle;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn target() -> HookTarget {
    HookTarget {
        bin: PathBuf::from("/opt/amplihack-test/bin/amplihack-hooks"),
        limit_resume: true,
    }
}

fn private(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn full() -> Vec<u8> {
    let mut s = json!({"env": {"XPIA_ENABLED": "false"}});
    hook_block::merge(&mut s, &target());
    serde_json::to_vec_pretty(&s).unwrap()
}

fn stripped() -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({"env": {"XPIA_ENABLED": "false"}, "hooks": {}})).unwrap()
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn is_full(path: &Path) -> bool {
    hook_block::drift(&read(path), &target()).is_clean()
}

fn settings_files(e: &Env) -> Vec<PathBuf> {
    vec![
        e.home.join(".claude/settings.json"),
        e.root.join("work/settings.json"),
        e.root.join("spare/settings.json"),
    ]
}

fn seed_full(e: &Env) {
    for path in settings_files(e) {
        private(&path, &full());
    }
}

fn snapshot(paths: &[PathBuf]) -> Vec<Vec<u8>> {
    paths.iter().map(|p| fs::read(p).unwrap()).collect()
}

/// `spare` reads the same as `work`: no upgrade, a fresh stay.
fn measure_fresh_stay(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let probes = probes_for(snap, &[("spare", rd(40.0, 30.0, None))]);
    decide_measured(ctx, snap, rd(40.0, 30.0, None), probes, now())
}

/// `work` past `--session-at`, `spare` nearly empty: a threshold swap.
fn measure_swap(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let probes = probes_for(snap, &[("spare", rd(5.0, 5.0, None))]);
    decide_measured(ctx, snap, rd(92.0, 60.0, None), probes, now())
}

/// The live login reads fresh with room; the only candidate's probe fails,
/// so it is skipped. The 20:11:44Z shape from #2279.
fn measure_stay_skipped_candidate(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let probes = probes_for(snap, &[("spare", Probed::failed())]);
    decide_measured(ctx, snap, rd(40.0, 30.0, None), probes, now())
}

fn run(e: &Env, measure: super::super::cycle::MeasureFn) -> CycleOutcome {
    let mut c = ctx_with(e, measure);
    run_cycle(&mut c, Mode::Watch).unwrap()
}

// --- (b) the cycle itself never writes a settings file --------------------

#[test]
fn b_a_stay_cycle_leaves_every_settings_file_byte_equal() {
    let e = env();
    seed_full(&e);
    let files = settings_files(&e);
    let before = snapshot(&files);
    assert_eq!(
        run(&e, measure_fresh_stay),
        CycleOutcome::Stayed,
        "{}",
        log_of(&e)
    );
    assert_eq!(snapshot(&files), before);
    assert!(files.iter().all(|p| is_full(p)));
}

#[test]
fn b_a_swap_cycle_leaves_every_settings_file_byte_equal() {
    let e = env();
    seed_full(&e);
    let files = settings_files(&e);
    let before = snapshot(&files);
    assert_eq!(
        run(&e, measure_swap),
        CycleOutcome::Swapped,
        "{}",
        log_of(&e)
    );
    assert_eq!(snapshot(&files), before);
    assert!(files.iter().all(|p| is_full(p)));
}

// --- heal -----------------------------------------------------------------

struct Paths {
    _tmp: tempfile::TempDir,
    home_settings: PathBuf,
    live: PathBuf,
}

fn paths(home: &[u8], live_body: Option<&[u8]>) -> Paths {
    let tmp = tempfile::tempdir().unwrap();
    let home_settings = tmp.path().join("home/.claude/settings.json");
    private(&home_settings, home);
    let live = tmp.path().join("root/work");
    fs::create_dir_all(&live).unwrap();
    if let Some(body) = live_body {
        private(&live.join("settings.json"), body);
    }
    Paths {
        _tmp: tmp,
        home_settings,
        live,
    }
}

fn ctx<'a>(p: &'a Paths, t: Option<&'a HookTarget>) -> HealCtx<'a> {
    HealCtx {
        home_settings: &p.home_settings,
        live_profile_dir: Some(&p.live),
        read_only: false,
        target: t,
        newer_stamp: false,
        live_refusal: None,
    }
}

#[test]
fn heal_after_a_stay_restores_a_pre_stripped_home_file() {
    let t = target();
    let p = paths(&stripped(), Some(&full()));
    let written = hook_heal::heal(CycleOutcome::Stayed, &ctx(&p, Some(&t)));
    assert_eq!(written, vec![p.home_settings.clone()]);
    assert!(is_full(&p.home_settings));
    assert_eq!(
        read(&p.home_settings)["env"],
        json!({"XPIA_ENABLED": "false"})
    );
}

#[test]
fn heal_after_a_swap_restores_both_files() {
    let t = target();
    let p = paths(&stripped(), Some(&stripped()));
    let written = hook_heal::heal(CycleOutcome::Swapped, &ctx(&p, Some(&t)));
    assert_eq!(written.len(), 2, "{written:?}");
    assert!(is_full(&p.home_settings));
    assert!(is_full(&p.live.join("settings.json")));
}

#[test]
fn heal_on_current_files_writes_nothing() {
    let t = target();
    let p = paths(&full(), Some(&full()));
    let before = (
        fs::read(&p.home_settings).unwrap(),
        fs::metadata(&p.home_settings).unwrap().modified().unwrap(),
    );
    let written = hook_heal::heal(CycleOutcome::Stayed, &ctx(&p, Some(&t)));
    assert!(written.is_empty());
    assert_eq!(
        (
            fs::read(&p.home_settings).unwrap(),
            fs::metadata(&p.home_settings).unwrap().modified().unwrap()
        ),
        before
    );
}

#[test]
fn heal_never_creates_an_absent_live_settings_file() {
    let t = target();
    let p = paths(&full(), None);
    hook_heal::heal(CycleOutcome::Stayed, &ctx(&p, Some(&t)));
    assert!(!p.live.join("settings.json").exists());
}

#[test]
fn heal_skips_read_only() {
    let t = target();
    let p = paths(&stripped(), Some(&stripped()));
    let mut c = ctx(&p, Some(&t));
    c.read_only = true;
    assert!(hook_heal::heal(CycleOutcome::Stayed, &c).is_empty());
    assert_eq!(fs::read(&p.home_settings).unwrap(), stripped());
}

#[test]
fn heal_skips_on_a_newer_install_stamp() {
    let t = target();
    let p = paths(&stripped(), Some(&stripped()));
    let mut c = ctx(&p, Some(&t));
    c.newer_stamp = true;
    assert!(hook_heal::heal(CycleOutcome::Swapped, &c).is_empty());
    assert_eq!(fs::read(&p.home_settings).unwrap(), stripped());
    assert_eq!(fs::read(p.live.join("settings.json")).unwrap(), stripped());
}

#[test]
fn heal_skips_without_a_target() {
    let p = paths(&stripped(), Some(&stripped()));
    assert!(hook_heal::heal(CycleOutcome::Stayed, &ctx(&p, None)).is_empty());
    assert_eq!(fs::read(&p.home_settings).unwrap(), stripped());
}

#[test]
fn heal_skips_every_other_outcome() {
    let t = target();
    for outcome in [
        CycleOutcome::StayedUnread,
        CycleOutcome::Alerted,
        CycleOutcome::Skipped,
        CycleOutcome::Held,
        CycleOutcome::Stopped,
    ] {
        let p = paths(&stripped(), Some(&stripped()));
        assert!(
            hook_heal::heal(outcome, &ctx(&p, Some(&t))).is_empty(),
            "{outcome:?}"
        );
        assert_eq!(fs::read(&p.home_settings).unwrap(), stripped());
    }
}

#[test]
fn a_refused_live_profile_does_not_stop_the_home_heal() {
    let t = target();
    let p = paths(&stripped(), Some(&stripped()));
    let mut c = ctx(&p, Some(&t));
    c.live_refusal = Some("hooks binary is under a temporary directory".into());
    let written = hook_heal::heal(CycleOutcome::Stayed, &c);
    assert_eq!(written, vec![p.home_settings.clone()]);
    assert!(is_full(&p.home_settings));
    assert_eq!(fs::read(p.live.join("settings.json")).unwrap(), stripped());
}

#[test]
fn heal_never_touches_login_files() {
    let t = target();
    let p = paths(&stripped(), Some(&stripped()));
    let creds = p.live.join(".credentials.json");
    let state = p.live.join(".claude.json");
    private(&creds, b"{\"k\":\"SECRET\"}");
    private(&state, b"{\"oauthAccount\":{}}");
    hook_heal::heal(CycleOutcome::Swapped, &ctx(&p, Some(&t)));
    assert_eq!(fs::read(&creds).unwrap(), b"{\"k\":\"SECRET\"}");
    assert_eq!(fs::read(&state).unwrap(), b"{\"oauthAccount\":{}}");
}

// --- (C11) a stay with a skipped candidate records `stayed` ----------------

#[test]
fn c11_a_fresh_stay_with_a_skipped_candidate_records_stayed() {
    let e = env();
    let start = now() - 1;
    let got = run(&e, measure_stay_skipped_candidate);
    assert_eq!(got, CycleOutcome::Stayed, "{}", log_of(&e));
    let raw = fs::read(&e.rs.ledger).unwrap();
    let v = parse_cycle(&raw, now() + 1).unwrap();
    assert_eq!(v.outcome.as_deref(), Some("stayed"), "{v:?}");
    assert!(v.has_capacity(), "{v:?}");
    assert!(v.fresh_capacity(start), "{v:?}");
}
