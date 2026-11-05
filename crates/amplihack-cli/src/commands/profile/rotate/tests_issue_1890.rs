//! Issue #1890: `--recovery act|log|off` (`AMPLIHACK_ROTATE_RECOVERY`) for
//! hand-change recovery, on the credential-reading recovery of #1886. Built
//! on the fixture from `tests_issue_1886_cycle.rs`: the ledger names `L`,
//! and `~/.claude` answers as `M`, which has a login of its own. Every login
//! is checked by inode. Everything is under a temporary directory; no test
//! sets a process variable, starts a child or reads the real HOME.
//!
//! Contract:
//! - `settings::Recovery::{Act (default), Log, Off}`, `Copy + Eq + Debug`,
//!   and `Settings.recovery`.
//! - `handfix::step_mode(store, rs, home, &mut Sink, env_for, read,
//!   min_gap, t, Recovery) -> Result<Step, ProfileError>`; `step_with` is
//!   `step_mode(.., Act)`. Under `Log`, where `Act` would move a login, it
//!   writes one `recovery (log only): ` line, moves and creates nothing,
//!   keeps `live`, `account` and the suspicion, clears saved guidance and
//!   returns `Step::Noted`. Notes and guidance are written as under `Act`.
//! - `handfix::step` returns `Step::Clear` at once under `Off`.
//! - `handfix::preview_mode(.., Recovery)`: `Act` is `preview`; `Log` gives
//!   the log-only line and the `would alert:` line for a confirmed move;
//!   `Off` gives nothing and reads nothing.
//! - `handfix_msg::LOG_ONLY == "recovery (log only): "`.
//! - `Settings::to_args` (so `daemon::spawned_args`) holds `--recovery <v>`
//!   for `log` and `off` and no `--recovery` for `act`.

use super::backup::stamp;
use super::cycle::{CycleOutcome, Mode, run_cycle};
use super::decide::{Decision, Reading};
use super::hand_fields::HandSuspect;
use super::handfix::{Env as Seam, Step, preview, preview_mode, step_mode, step_with};
use super::handfix_msg::{self as hmsg, LOG_ONLY};
use super::identify::LiveId;
use super::log::{Sink, msg, open_or_warn};
use super::observe::{Measurement, Snap};
use super::settings::{self, Recovery, Settings};
use super::tests_handfix::seam;
use super::tests_issue_1886::{K, L, M};
use super::tests_issue_1886_cycle::{
    Fx, GAP, T, a_k, a_l, a_m, assert_nothing_moved, case_a, ctx, fx, never, set_reading,
};
use super::tests_swap::{n, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use std::path::Path;

fn step_in(f: &Fx, mode: Recovery, t: i64) -> Step {
    let mut out = Sink::new(open_or_warn(&f.rs.log, &f.rs.log1), false, true);
    let read = |p: &Path| f.read(p);
    step_mode(
        &f.store, &f.rs, &f.home, &mut out, seam, &read, GAP, t, mode,
    )
    .unwrap()
}

fn preview_in(f: &Fx, mode: Recovery, t: i64) -> Vec<String> {
    let read = |p: &Path| f.read(p);
    preview_mode(&f.store, &f.rs, &f.home, seam, &read, GAP, t, mode).unwrap()
}

fn hand_change() -> String {
    msg::hand_change(&n(L))
}

fn would_alert() -> String {
    format!(
        "would alert: {}",
        hand_change().strip_prefix("ALERT: ").unwrap()
    )
}

/// The log-only line for a move at `t`.
fn log_moved(root: &Path, t: i64) -> String {
    format!(
        "recovery (log only): would move {M}'s login to {}/.rejected-logins/rotate-{M}-{}/.credentials.json and record {M} as live",
        root.display(),
        stamp(t)
    )
}

/// `case_a` with the suspicion already read once at `T - GAP`, so the next
/// step at `T` is a confirmed move.
fn confirmed() -> Fx {
    let f = case_a();
    f.set_ledger(|l| {
        l.hand_suspect = Some(HandSuspect {
            profile: M.into(),
            first_seen: T - GAP,
        });
    });
    f
}

/// No login moved, nothing under `.rejected-logins/`, and the ledger still
/// names L with its account.
#[track_caller]
fn untouched(f: &Fx, inodes: &[(String, Option<u64>)]) {
    assert_nothing_moved(f, inodes);
    assert!(!f.rejected().exists(), "no .rejected-logins/ is created");
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(L), "live unchanged");
    assert_eq!(l.account.as_deref(), Some("u-mike"), "account unchanged");
}

/// No line from a step that acts: nothing recovered, nothing swapped.
#[track_caller]
fn no_action(lines: &[String]) {
    for l in lines {
        assert!(
            !l.starts_with("recovered:") && !l.starts_with("swap "),
            "unexpected line: {l}\n{lines:?}"
        );
    }
}

// --- Settings reach the daemon ---------------------------------------------

#[test]
fn spawned_args_forward_recovery_only_when_not_act() {
    let base = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    assert_eq!(base.recovery, Recovery::Act, "the default is act");
    let act = super::daemon::spawned_args(&base);
    assert!(!act.iter().any(|a| a == "--recovery"), "{act:?}");
    for (mode, word) in [(Recovery::Log, "log"), (Recovery::Off, "off")] {
        let s = Settings {
            recovery: mode,
            ..base
        };
        let args = super::daemon::spawned_args(&s);
        let i = args
            .iter()
            .position(|a| a == "--recovery")
            .unwrap_or_else(|| panic!("--recovery in {args:?}"));
        assert_eq!(args[i + 1], word, "{args:?}");
        // The argv still parses as `profile rotate watch --spawned`.
        let cli = crate::Cli::try_parse_from(
            std::iter::once("amplihack".to_string()).chain(args.clone()),
        )
        .unwrap_or_else(|e| panic!("{args:?} does not parse: {e}"));
        drop(cli);
    }
}

#[test]
fn explicit_act_resolves_to_the_same_settings_and_daemon_argv_as_unset() {
    let unset = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let flags = super::TuneFlags {
        recovery: Some("act".into()),
        ..Default::default()
    };
    let flag = settings::resolve(&flags, None, &|_| None).unwrap();
    let var = settings::resolve(&super::TuneFlags::default(), None, &|k| {
        (k == "AMPLIHACK_ROTATE_RECOVERY").then(|| "act".into())
    })
    .unwrap();
    assert_eq!(flag, unset);
    assert_eq!(var, unset);
    assert_eq!(
        super::daemon::spawned_args(&flag),
        super::daemon::spawned_args(&unset)
    );
}

#[test]
fn the_recovery_flag_is_accepted_by_the_one_shot_start_and_watch() {
    for argv in [
        vec!["profile", "rotate", "--recovery", "log"],
        vec!["profile", "rotate", "start", "--recovery", "off"],
        vec!["profile", "rotate", "watch", "--recovery", "act"],
    ] {
        let full = std::iter::once("amplihack").chain(argv.iter().copied());
        crate::Cli::try_parse_from(full).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    }
}

// --- log: the step ----------------------------------------------------------

#[test]
fn log_writes_the_first_reading_note_as_act_does() {
    let f = case_a();
    let inodes = f.inodes();
    assert_eq!(step_in(&f, Recovery::Log, T), Step::Noted);
    untouched(&f, &inodes);
    assert_eq!(f.logged(), vec![hmsg::note_suspect(&n(M), &n(L))]);
    assert_eq!(
        f.ledger().hand_suspect,
        Some(HandSuspect {
            profile: M.into(),
            first_seen: T
        })
    );
}

#[test]
fn log_confirmed_move_moves_nothing_and_writes_the_log_only_line() {
    let f = case_a();
    let inodes = f.inodes();
    assert_eq!(step_in(&f, Recovery::Log, T), Step::Noted);
    assert_eq!(step_in(&f, Recovery::Log, T + GAP), Step::Noted);
    untouched(&f, &inodes);
    let log = f.logged();
    assert_eq!(log.last().unwrap(), &log_moved(&f.root, T + GAP), "{log:?}");
    no_action(&log);
    let l = f.ledger();
    assert_eq!(
        l.hand_suspect,
        Some(HandSuspect {
            profile: M.into(),
            first_seen: T
        }),
        "the suspicion is kept, so the cycle stops"
    );
    assert_eq!(l.guidance, None);
}

#[test]
fn log_writes_the_log_only_line_on_every_confirmed_cycle() {
    let f = confirmed();
    let inodes = f.inodes();
    for k in 0..3 {
        assert_eq!(step_in(&f, Recovery::Log, T + k * GAP), Step::Noted);
    }
    untouched(&f, &inodes);
    let want: Vec<String> = (0..3).map(|k| log_moved(&f.root, T + k * GAP)).collect();
    assert_eq!(f.logged(), want);
}

#[test]
fn log_with_no_free_backup_name_writes_the_refusal_line() {
    use std::os::unix::fs::PermissionsExt;
    let f = confirmed();
    let s = stamp(T);
    let taken: Vec<_> = (1..=99)
        .map(|k| match k {
            1 => format!("rotate-{M}-{s}"),
            _ => format!("rotate-{M}-{s}-{k}"),
        })
        .collect();
    for d in &taken {
        let p = f.rejected().join(d);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let inodes = f.inodes();
    assert_eq!(step_in(&f, Recovery::Log, T), Step::Noted);
    assert_eq!(f.inodes(), inodes, "a login moved");
    let mut left: Vec<_> = std::fs::read_dir(f.rejected())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    let mut want = taken.clone();
    want.sort();
    assert_eq!(left, want, "nothing is added under .rejected-logins/");
    let line = format!(
        "{LOG_ONLY}would move {M}'s login aside and record {M} as live, but no backup name is free"
    );
    assert_eq!(f.logged(), vec![line]);
}

#[test]
fn log_clears_guidance_saved_by_an_earlier_act_cycle() {
    let f = confirmed();
    f.set_ledger(|l| l.guidance = Some(vec!["ALERT: saved earlier".into()]));
    assert_eq!(step_in(&f, Recovery::Log, T), Step::Noted);
    assert_eq!(f.ledger().guidance, None);
}

#[test]
fn log_lines_start_with_the_fixed_prefix_and_name_no_account() {
    assert_eq!(LOG_ONLY, "recovery (log only): ");
    let f = confirmed();
    step_in(&f, Recovery::Log, T);
    let log = f.logged();
    assert_eq!(log.len(), 1, "{log:?}");
    for l in &log {
        assert!(l.starts_with(LOG_ONLY), "{l}");
        for secret in ["u-mike", "u-lima", "example.com", "fake-at"] {
            assert!(!l.contains(secret), "{secret} in {l}");
        }
    }
}

/// L has no login of its own, so recording M would leave L with none: under
/// `act` that is guidance, not a move, and `log` writes the same lines.
#[test]
fn log_writes_guidance_as_act_does_when_recovery_could_not_act() {
    let run = |mode: Recovery| {
        let f = fx(&[M, K], L, Some(T - 3600), M);
        f.answer("~/.claude", Ok(a_m()));
        f.answer(M, Ok(a_m()));
        f.answer(K, Ok(a_k()));
        f.set_ledger(|l| {
            l.hand_suspect = Some(HandSuspect {
                profile: M.into(),
                first_seen: T - GAP,
            });
        });
        let inodes = f.inodes();
        let step = step_in(&f, mode, T);
        assert_nothing_moved(&f, &inodes);
        let root = f.root.display().to_string();
        let log: Vec<String> = f
            .logged()
            .into_iter()
            .map(|l| l.replace(&root, "<root>"))
            .collect();
        (step, log, f.ledger().guidance.is_some())
    };
    let act = run(Recovery::Act);
    assert_eq!(act.0, Step::Guided);
    assert!(act.1[0].starts_with("ALERT: "), "{:?}", act.1);
    assert!(act.2, "guidance saved");
    let log = run(Recovery::Log);
    assert_eq!(log.0, act.0);
    assert_eq!(log.1, act.1);
    assert!(log.2);
    assert!(!log.1.iter().any(|l| l.starts_with(LOG_ONLY)));
}

#[test]
fn explicit_act_behaves_exactly_as_step_with() {
    let run = |explicit: bool| {
        let f = case_a();
        let mut steps = Vec::new();
        for t in [T, T + GAP] {
            let mut out = Sink::new(open_or_warn(&f.rs.log, &f.rs.log1), false, true);
            let read = |p: &Path| f.read(p);
            let (s, rs, h) = (&f.store, &f.rs, &f.home);
            steps.push(if explicit {
                step_mode(s, rs, h, &mut out, seam, &read, GAP, t, Recovery::Act).unwrap()
            } else {
                step_with(s, rs, h, &mut out, seam, &read, GAP, t).unwrap()
            });
        }
        let root = f.root.display().to_string();
        let log: Vec<String> = f
            .logged()
            .into_iter()
            .map(|l| l.replace(&root, "<root>"))
            .collect();
        let l = f.ledger();
        (steps, log, l.live, l.hand_suspect, f.backed_up().len())
    };
    let act = run(true);
    assert_eq!(act.0, vec![Step::Noted, Step::Recovered]);
    assert_eq!(act, run(false));
}

// --- log and off: the real cycle ---------------------------------------------

/// The confirmed case through `run_cycle`: readings by path, and a
/// suspicion first read an hour ago, after the last swap.
fn cycle_fx() -> Fx {
    let f = fx(&[L, M, K], L, Some(now() - 7200), M);
    set_reading(f.home.join(".claude"), Ok(a_m()));
    set_reading(f.root.join(L), Ok(a_l()));
    set_reading(f.root.join(M), Ok(a_m()));
    set_reading(f.root.join(K), Ok(a_k()));
    f.set_ledger(|l| {
        l.hand_suspect = Some(HandSuspect {
            profile: M.into(),
            first_seen: now() - 3600,
        });
    });
    f
}

fn logged_lines(f: &Fx) -> Vec<String> {
    f.logged()
        .into_iter()
        .filter(|l| !l.starts_with("skip: "))
        .collect()
}

#[cfg(target_os = "linux")]
#[test]
fn log_cycle_writes_the_line_every_cycle_alerts_once_and_never_swaps() {
    let f = cycle_fx();
    let inodes = f.inodes();
    let mut c = ctx(&f, never);
    c.settings.recovery = Recovery::Log;
    for _ in 0..2 {
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Alerted
        );
    }
    untouched(&f, &inodes);
    let log = logged_lines(&f);
    let prefix = format!("{LOG_ONLY}would move {M}'s login to ");
    assert_eq!(
        log.iter().filter(|l| l.starts_with(&prefix)).count(),
        2,
        "{log:?}"
    );
    // The ALERT goes through `alert_once`: logged once per key, not per cycle.
    assert_eq!(
        log.iter().filter(|l| **l == hand_change()).count(),
        1,
        "{log:?}"
    );
    no_action(&log);
    assert!(f.ledger().hand_suspect.is_some());
}

/// Under `off` the step reads nothing and writes nothing. A suspicion left
/// by an earlier `act` or `log` cycle still stops the cycle.
#[cfg(target_os = "linux")]
#[test]
fn off_cycle_logs_nothing_from_the_step_and_a_kept_suspicion_alerts() {
    fn no_read(_: &Path) -> Result<super::identify::Account, &'static str> {
        panic!("off reads no credential")
    }
    let f = cycle_fx();
    let (inodes, before) = (f.inodes(), f.ledger());
    let mut c = ctx(&f, never);
    c.identity = no_read;
    c.settings.recovery = Recovery::Off;
    for _ in 0..2 {
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Alerted
        );
    }
    untouched(&f, &inodes);
    let log = logged_lines(&f);
    assert_eq!(log, vec![hand_change()], "one ALERT, no step line");
    let l = f.ledger();
    assert_eq!(l.hand_suspect, before.hand_suspect, "off never clears it");
    assert_eq!(l.guidance, before.guidance);
}

fn stay_on_l(_: &super::cycle::Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    assert_eq!(snap.live, LiveId::Known(n(L)), "the ledger, at its word");
    Ok(Measurement {
        live: Reading::Measured {
            session: 10.0,
            week: 10.0,
        },
        others: Vec::new(),
        decision: Decision::Stay,
        note: None,
        failed: 0,
        tag: None,
        new_blocks: Vec::new(),
        next_plan_reset: None,
        unknown_plan: Vec::new(),
        key_held: Vec::new(),
        measured_at: now(),
        probed: None,
        fresh_lines: Vec::new(),
    })
}

/// Under `off` nothing detects a hand change: with no suspicion in the
/// ledger, the cycle takes the ledger at its word and goes on to measure.
#[cfg(target_os = "linux")]
#[test]
fn off_without_a_suspicion_takes_the_ledger_at_its_word() {
    fn no_read(_: &Path) -> Result<super::identify::Account, &'static str> {
        panic!("off reads no credential")
    }
    let f = cycle_fx();
    f.set_ledger(|l| l.hand_suspect = None);
    let inodes = f.inodes();
    let mut c = ctx(&f, stay_on_l);
    c.identity = no_read;
    c.settings.recovery = Recovery::Off;
    assert_eq!(
        run_cycle(&mut c, Mode::OneShot).unwrap(),
        CycleOutcome::Stayed
    );
    untouched(&f, &inodes);
    let log = logged_lines(&f);
    assert!(!log.iter().any(|l| l.starts_with("ALERT:")), "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("note:")), "{log:?}");
    no_action(&log);
}

#[cfg(target_os = "linux")]
#[test]
fn off_with_a_stuck_home_json_writes_only_the_unreadable_alert() {
    let f = cycle_fx();
    f.set_ledger(|l| l.hand_suspect = None);
    private(&f.home.join(".claude.json"), "{");
    let since = now() - 3600;
    f.set_ledger(|l| l.home_unreadable_since = Some(since));
    let inodes = f.inodes();
    let mut c = ctx(&f, never);
    c.settings.recovery = Recovery::Off;
    for _ in 0..2 {
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Alerted
        );
    }
    assert_nothing_moved(&f, &inodes);
    // Two cycles, one line: the ALERT is logged once per key.
    assert_eq!(
        logged_lines(&f),
        vec![msg::home_unreadable(&n(L))],
        "no stuck guidance"
    );
    let l = f.ledger();
    assert_eq!(l.guidance, None);
    assert_eq!(
        l.home_unreadable_since,
        Some(since),
        "off neither sets nor clears it"
    );
}

// --- preview and --dry-run --------------------------------------------------

#[test]
fn log_preview_shows_the_log_only_line_and_would_alert() {
    let f = confirmed();
    let (inodes, before) = (f.inodes(), f.ledger());
    assert_eq!(
        preview_in(&f, Recovery::Log, T),
        vec![log_moved(&f.root, T), would_alert()]
    );
    assert_eq!(f.ledger(), before, "the preview writes no ledger");
    untouched(&f, &inodes);
    assert!(f.logged().is_empty());
}

#[test]
fn log_preview_of_a_note_is_the_act_preview() {
    let f = case_a();
    let read = |p: &Path| f.read(p);
    let act = preview(&f.store, &f.rs, &f.home, seam, &read, GAP, T).unwrap();
    assert!(act[0].starts_with("would note: "), "{act:?}");
    assert_eq!(preview_in(&f, Recovery::Log, T), act);
}

#[test]
fn act_preview_mode_is_preview() {
    for f in [case_a(), confirmed()] {
        let read = |p: &Path| f.read(p);
        assert_eq!(
            preview_in(&f, Recovery::Act, T),
            preview(&f.store, &f.rs, &f.home, seam, &read, GAP, T).unwrap()
        );
    }
}

#[test]
fn off_preview_is_empty_and_reads_nothing() {
    fn panics<'a>() -> Seam<'a> {
        panic!("env_for must not be called under off")
    }
    let f = confirmed();
    let read = |p: &Path| f.read(p);
    let lines = preview_mode(
        &f.store,
        &f.rs,
        &f.home,
        panics,
        &read,
        GAP,
        T,
        Recovery::Off,
    )
    .unwrap();
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(f.reads.get(), 0, "no credential read");
}

#[cfg(target_os = "linux")]
#[test]
fn dry_run_with_an_open_suspicion_ends_alerted_and_moves_nothing_in_every_mode() {
    for mode in [Recovery::Act, Recovery::Log, Recovery::Off] {
        let f = cycle_fx();
        let (inodes, before) = (f.inodes(), f.ledger());
        let mut c = ctx(&f, never);
        c.settings.recovery = mode;
        assert_eq!(
            run_cycle(&mut c, Mode::DryRun).unwrap(),
            CycleOutcome::Alerted,
            "{mode:?}"
        );
        assert_nothing_moved(&f, &inodes);
        assert!(!f.rejected().exists(), "{mode:?}");
        assert_eq!(f.ledger(), before, "{mode:?}: --dry-run writes no ledger");
    }
}
