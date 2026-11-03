//! Issue #1890: `--recovery act|log|off` (`AMPLIHACK_ROTATE_RECOVERY`) for
//! hand-change recovery. Built on the 27 Oct fixture from
//! `tests_handfix.rs`: the ledger names `ryan-ryansweet` (L), which has no
//! login; `~/.claude` holds the account of `ryan-mistt-org` (M). Every login
//! is checked by inode. Everything is under a temporary directory; no test
//! sets a process variable, starts a child or reads the real HOME.
//!
//! Contract:
//! - `settings::Recovery::{Act (default), Log, Off}`, `Copy + Eq + Debug`,
//!   and `Settings.recovery`.
//! - `handfix::step_mode(store, rs, home, &mut Sink, env_for, t, Recovery)
//!   -> Result<Step, ProfileError>`; `step_with` is `step_mode(.., Act)`.
//!   Under `Log`, where `Act` would record or move, it writes one
//!   `recovery (log only): ` line, changes no `live`, `account` or
//!   `lost_login`, creates nothing and returns `Step::Clear`. Guidance is
//!   written as under `Act`.
//! - `handfix::step` returns `Step::Clear` at once under `Off`.
//! - `handfix::preview_mode(store, rs, home, Recovery, env_for, t)`: `Act`
//!   is `preview`; `Log` gives the log-only line and the `would alert:` line;
//!   `Off` gives nothing and never calls `env_for`.
//! - `handfix_msg::LOG_ONLY == "recovery (log only): "`.
//! - `Settings::to_args` (so `daemon::spawned_args`) holds `--recovery <v>`
//!   for `log` and `off` and no `--recovery` for `act`.

use super::cycle::{CycleOutcome, Mode, run_cycle};
use super::handfix::{Env as Seam, Step, preview, preview_mode, step_mode, step_with};
use super::handfix_msg::LOG_ONLY;
use super::log::{Sink, msg, open_or_warn};
use super::settings::{self, Recovery, Settings};
use super::tests_handfix::{CREDS, Fx, L, M, T, acct, fx, lost_l, seam};
use super::tests_handfix_cycle::{ctx, never};
use super::tests_swap::{ino_at, n, private};
use crate::commands::profile::auto_switch::kit::{State, now};
use crate::commands::profile::expected::record;
use crate::commands::profile::rotate::identify::Account;
use std::path::Path;

/// `rotate::backup::stamp(T)`.
const STAMP: &str = "20261027T031705Z";

fn step_in(f: &Fx, mode: Recovery, t: i64) -> Step {
    let mut out = Sink::new(open_or_warn(&f.rs.log, &f.rs.log1), false, true);
    step_mode(&f.store, &f.rs, &f.home, &mut out, seam, t, mode).unwrap()
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

fn log_record() -> String {
    format!("recovery (log only): would record {M} as live (ledger named {L})")
}

fn log_relogin() -> String {
    format!(
        "recovery (log only): would record {L} as live again with its own login (ledger account differs)"
    )
}

fn log_moved(root: &Path) -> String {
    format!(
        "recovery (log only): would move {M}'s login to {}/.rejected-logins/rotate-{M}-{STAMP}/.credentials.json and record {M} as live",
        root.display()
    )
}

/// No login moved, nothing under `.rejected-logins/`, and the ledger still
/// names L with L's account and adds no lost login.
fn untouched(f: &Fx) {
    f.assert_nothing_moved();
    assert!(!f.rejected().exists(), "no .rejected-logins/ is created");
    assert!(
        f.ledger().lost_login.is_empty(),
        "{:?}",
        f.ledger().lost_login
    );
}

/// No line from the step that acts: nothing recovered, nothing swapped.
fn no_action(lines: &[String]) {
    for l in lines {
        assert!(
            !l.starts_with("recovered:") && !l.starts_with("swap "),
            "unexpected line: {l}\n{lines:?}"
        );
    }
}

/// L's own record names the account now in `~/.claude.json` (#1845), and
/// the ledger's account for L differs: `act` keeps L live with that login.
fn relogin_fx() -> Fx {
    let f = fx(false, false);
    let a = Account::from_block(&serde_json::json!({"accountUuid": "u-recorded"})).unwrap();
    record(&State::new(&f.root).unwrap(), &n(L), &a).unwrap();
    private(
        &f.home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": {"accountUuid": "u-recorded"}}).to_string(),
    );
    f
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

#[cfg(target_os = "linux")]
#[test]
fn log_case_b_moves_nothing_records_nothing_and_writes_the_log_only_line() {
    let f = fx(true, true);
    assert_eq!(step_in(&f, Recovery::Log, T), Step::Clear);
    untouched(&f);
    assert_eq!(ino_at(&f.slot(M)), f.m, "M's own copy stays where it was");
    assert_eq!(f.logged(), vec![log_moved(&f.root)]);
}

#[cfg(target_os = "linux")]
#[test]
fn log_case_a_writes_the_record_line_and_no_lost_login() {
    let f = fx(false, false);
    assert_eq!(step_in(&f, Recovery::Log, T), Step::Clear);
    untouched(&f);
    assert_eq!(f.logged(), vec![log_record()], "no lost-login line for L");
    assert!(!f.logged().contains(&lost_l()));
}

#[cfg(target_os = "linux")]
#[test]
fn log_own_record_writes_the_relogin_line_and_keeps_the_ledger() {
    // The fixture really is the #1845 own-record path under act.
    let acted = relogin_fx();
    assert_eq!(step_in(&acted, Recovery::Act, T), Step::Recovered);
    assert_eq!(
        acted.logged()[0],
        format!("recovered: ~/.claude holds the account recorded for {L}; kept {L} as live")
    );

    let f = relogin_fx();
    assert_eq!(step_in(&f, Recovery::Log, T), Step::Clear);
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(L));
    assert_eq!(l.account.as_deref(), Some("u-rs"), "account unchanged");
    assert_eq!(f.logged(), vec![log_relogin()]);
}

#[cfg(target_os = "linux")]
#[test]
fn log_lines_start_with_the_fixed_prefix() {
    assert_eq!(LOG_ONLY, "recovery (log only): ");
    for f in [fx(true, false), fx(false, false), relogin_fx()] {
        step_in(&f, Recovery::Log, T);
        let lines = f.logged();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with(LOG_ONLY), "{lines:?}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn log_writes_guidance_as_act_does_when_recovery_could_not_act() {
    // No profile has the account in ~/.claude: guidance, not a recovery.
    let mk = || {
        let f = fx(false, false);
        private(
            &f.home.join(".claude.json"),
            &acct("u-nobody", "nobody@example.com"),
        );
        f
    };
    let (a, g) = (mk(), mk());
    assert_eq!(step_in(&a, Recovery::Act, T), Step::Guided);
    assert_eq!(step_in(&g, Recovery::Log, T), Step::Guided);
    let fixed = |f: &Fx| -> Vec<String> {
        let root = f.base.display().to_string();
        f.logged()
            .into_iter()
            .map(|l| l.replace(&root, "<base>"))
            .collect()
    };
    assert_eq!(fixed(&g), fixed(&a));
    assert!(!fixed(&g).iter().any(|l| l.starts_with(LOG_ONLY)));
    assert_eq!(g.ledger().guidance.is_some(), a.ledger().guidance.is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn log_still_prunes_lost_login_entries() {
    let f = fx(false, false);
    let mut l = f.ledger();
    l.lost_login = vec!["gone-profile".into()];
    f.rs.save_ledger(&l).unwrap();
    step_in(&f, Recovery::Log, T);
    assert!(
        f.ledger().lost_login.is_empty(),
        "a removed profile is pruned under log as under act: {:?}",
        f.ledger().lost_login
    );
}

#[cfg(target_os = "linux")]
#[test]
fn log_lines_contain_no_account_identity() {
    for f in [fx(true, true), fx(false, false), relogin_fx()] {
        step_in(&f, Recovery::Log, T);
        let mut text = f.logged();
        text.extend(preview_mode(&f.store, &f.rs, &f.home, Recovery::Log, seam, T).unwrap());
        assert!(!text.is_empty());
        for line in &text {
            for secret in ["@example.com", "u-mistt", "u-src", "u-recorded"] {
                assert!(!line.contains(secret), "{secret} in {line}");
            }
        }
    }
}

// --- explicit act -----------------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn explicit_act_behaves_exactly_as_step_with() {
    for own in [true, false] {
        let (a, b) = (fx(own, true), fx(own, true));
        let mut out = Sink::new(open_or_warn(&a.rs.log, &a.rs.log1), false, true);
        let unset = step_with(&a.store, &a.rs, &a.home, &mut out, seam, T).unwrap();
        let act = step_in(&b, Recovery::Act, T);
        assert_eq!(act, unset);
        assert_eq!(act, Step::Recovered);
        let fixed = |f: &Fx| -> Vec<String> {
            let root = f.base.display().to_string();
            f.logged()
                .into_iter()
                .map(|l| l.replace(&root, "<base>"))
                .collect()
        };
        assert_eq!(fixed(&b), fixed(&a));
        let (la, lb) = (a.ledger(), b.ledger());
        assert_eq!(
            (lb.live, lb.account, lb.lost_login),
            (la.live, la.account, la.lost_login)
        );
        assert_eq!(a.backed_up().len(), b.backed_up().len());
    }
}

// --- log and off: the cycle -------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn log_cycle_moves_nothing_records_nothing_and_alerts() {
    let f = fx(true, true);
    let mut c = ctx(&f, never);
    c.settings.recovery = Recovery::Log;
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    untouched(&f);
    assert_eq!(ino_at(&f.live()), Some(f.h), "~/.claude keeps its login");
    assert_eq!(ino_at(&f.slot(M)), f.m, "M keeps its own copy");
    let log = f.logged();
    let at = log
        .iter()
        .position(|l| l.starts_with(LOG_ONLY))
        .unwrap_or_else(|| panic!("no log-only line: {log:?}"));
    let line = &log[at];
    let head = format!(
        "recovery (log only): would move {M}'s login to {}/.rejected-logins/rotate-{M}-",
        f.root.display()
    );
    assert!(line.starts_with(&head), "{line}");
    assert!(
        line.ends_with(&format!("/{CREDS} and record {M} as live")),
        "{line}"
    );
    assert!(
        log[at + 1..].contains(&hand_change()),
        "the hand-change ALERT follows: {log:?}"
    );
    no_action(&log);
    assert_eq!(f.ledger().last_cycle.unwrap().outcome, "alerted");
}

#[cfg(target_os = "linux")]
#[test]
fn log_cycle_repeats_every_cycle_and_never_swaps() {
    let f = fx(false, true);
    let mut c = ctx(&f, never);
    c.settings.recovery = Recovery::Log;
    for _ in 0..2 {
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Alerted
        );
    }
    untouched(&f);
    let log = f.logged();
    assert_eq!(
        log.iter().filter(|l| **l == log_record()).count(),
        2,
        "{log:?}"
    );
    no_action(&log);
}

#[cfg(target_os = "linux")]
#[test]
fn off_cycle_logs_nothing_from_the_step_and_alerts() {
    let f = fx(true, true);
    // L is already in lost_login: under act each cycle repeats its line.
    let mut l = f.ledger();
    l.lost_login = vec![L.into()];
    f.rs.save_ledger(&l).unwrap();
    let mut c = ctx(&f, never);
    c.settings.recovery = Recovery::Off;
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    f.assert_nothing_moved();
    assert!(!f.rejected().exists());
    let log = f.logged();
    for line in &log {
        assert!(
            !line.starts_with(LOG_ONLY)
                && !line.starts_with("recovered:")
                && !line.starts_with("fix ")
                && *line != lost_l(),
            "a line from the skipped step: {line}\n{log:?}"
        );
    }
    assert!(log.contains(&hand_change()), "{log:?}");
    no_action(&log);
    let l = f.ledger();
    assert_eq!(l.lost_login, vec![L.to_string()], "lost_login untouched");
    assert_eq!(l.last_cycle.unwrap().outcome, "alerted");
}

#[cfg(target_os = "linux")]
#[test]
fn off_with_a_stuck_home_json_writes_only_the_unreadable_alert() {
    let f = fx(true, true);
    private(&f.home.join(".claude.json"), "{");
    let since = now() - 3600;
    let mut l = f.ledger();
    l.home_unreadable_since = Some(since);
    f.rs.save_ledger(&l).unwrap();
    let mut c = ctx(&f, never);
    c.settings.recovery = Recovery::Off;
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    f.assert_nothing_moved();
    let log: Vec<String> = f
        .logged()
        .into_iter()
        .filter(|l| !l.starts_with("skip: "))
        .collect();
    assert_eq!(log, vec![msg::home_unreadable(&n(L))], "no stuck guidance");
    let l = f.ledger();
    assert_eq!(l.guidance, None);
    assert_eq!(
        l.home_unreadable_since,
        Some(since),
        "off neither sets nor clears it"
    );
}

// --- preview and --dry-run --------------------------------------------------

/// Paths, inodes, sizes and times of everything under the fixture.
fn tree(f: &Fx) -> Vec<(String, u64, u64, i64)> {
    fn walk(p: &Path, out: &mut Vec<(String, u64, u64, i64)>) {
        use std::os::unix::fs::MetadataExt;
        let Ok(m) = std::fs::symlink_metadata(p) else {
            return;
        };
        out.push((p.display().to_string(), m.ino(), m.len(), m.mtime_nsec()));
        if m.is_dir() {
            let mut kids: Vec<_> = std::fs::read_dir(p)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            kids.sort();
            for k in kids {
                walk(&k, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&f.base, &mut out);
    out
}

#[cfg(target_os = "linux")]
#[test]
fn log_preview_shows_the_log_only_line_and_would_alert() {
    let f = fx(true, true);
    let before = tree(&f);
    let lines = preview_mode(&f.store, &f.rs, &f.home, Recovery::Log, seam, T).unwrap();
    assert_eq!(lines, vec![log_moved(&f.root), would_alert()]);
    assert_eq!(tree(&f), before, "the preview writes nothing");

    let a = fx(false, false);
    let lines = preview_mode(&a.store, &a.rs, &a.home, Recovery::Log, seam, T).unwrap();
    assert_eq!(lines, vec![log_record(), would_alert()]);
}

#[cfg(target_os = "linux")]
#[test]
fn log_preview_of_guidance_is_the_act_preview() {
    let f = fx(false, false);
    private(
        &f.home.join(".claude.json"),
        &acct("u-nobody", "nobody@example.com"),
    );
    assert_eq!(
        preview_mode(&f.store, &f.rs, &f.home, Recovery::Log, seam, T).unwrap(),
        preview(&f.store, &f.rs, &f.home, seam, T).unwrap()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn act_preview_mode_is_preview() {
    for own in [true, false] {
        let f = fx(own, true);
        assert_eq!(
            preview_mode(&f.store, &f.rs, &f.home, Recovery::Act, seam, T).unwrap(),
            preview(&f.store, &f.rs, &f.home, seam, T).unwrap()
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn off_preview_is_empty_and_reads_no_profile() {
    fn panics<'a>() -> Seam<'a> {
        panic!("env_for must not be called under off")
    }
    let f = fx(true, true);
    let before = tree(&f);
    assert!(
        preview_mode(&f.store, &f.rs, &f.home, Recovery::Off, panics, T)
            .unwrap()
            .is_empty()
    );
    assert_eq!(tree(&f), before);
}

#[cfg(target_os = "linux")]
#[test]
fn dry_run_ends_alerted_and_writes_nothing_in_every_mode() {
    for mode in [Recovery::Act, Recovery::Log, Recovery::Off] {
        let f = fx(true, true);
        let before = tree(&f);
        let mut c = ctx(&f, never);
        c.settings.recovery = mode;
        assert_eq!(
            run_cycle(&mut c, Mode::DryRun).unwrap(),
            CycleOutcome::Alerted,
            "{mode:?}"
        );
        assert_eq!(tree(&f), before, "{mode:?}: --dry-run writes nothing");
    }
}
