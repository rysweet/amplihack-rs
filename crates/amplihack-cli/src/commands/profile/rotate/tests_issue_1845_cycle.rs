//! Issue #1845 (D5, AC4): the account-mismatch refusal in a real cycle. The
//! swap, ledger and log are the real ones under a temporary HOME and profile
//! root; only probing is stubbed (`Ctx.measure`). No `claude`, no daemon, no
//! tmux. Follows `tests_issue_1804_cycle.rs`.
//!
//! Fixture: `work` is live in `~/.claude` (`u-work`) and recorded so in the
//! ledger, but its recorded account is `u-SENTINEL-recorded`; `spare` is a
//! plan login with room. `work` is exhausted, so every cycle wants
//! `work -> spare`.
//!
//! Contract:
//! - Watch: the cycle ends `Alerted`, nothing moves, and the log has exactly
//!   one `ALERT: swap refused: the login in ~/.claude is not the account
//!   recorded for profile work` line over two cycles. The ledger's ALERT key
//!   is `account_mismatch:work`.
//! - One-shot: the cycle returns the `RotateRefused` error.
//! - The documented fix, a `/login` in `~/.claude` to the recorded account,
//!   is kept as `work` by hand-change recovery, and the next swap goes
//!   ahead and writes that account into `work/.claude.json`. When that file
//!   cannot be written, the swap is refused before anything moves.
//! - No line names an account, an e-mail or a token.

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::decide::{Probed, Reading, Win, Windows};
use super::ledger::{RotateLedger, RotateState};
use super::observe::{Measurement, Snap, decide_measured};
use super::settings;
use super::tests_swap::{ino_at, login, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Liveness, State, now};
use crate::commands::profile::expected::{record, recorded};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::identify::Account;
use crate::commands::profile::store::ProfileStore;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const CREDS: &str = ".credentials.json";
/// The one-shot command's error, as `ALERT: <error>`.
const REFUSED: &str =
    "ALERT: swap refused: the login in ~/.claude is not the account recorded for profile work";
/// Issue #2013 (D10): the logged refusal, with its evidence and fix. The
/// record holds a UUID only, so it is shown as `another account`.
const ALERT: &str = "ALERT: swap refused (account-mismatch, profile work): the login in \
     ~/.claude is not the account recorded for profile work. Evidence: \
     credentials/verified=work@example.com, ledger=work@example.com, record=another account, \
     .claude.json=work@example.com. Fix: run plain claude, /login to the account recorded for \
     work, then wait for the next cycle";

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn acct(uuid: &str, email: &str) -> String {
    serde_json::json!({"oauthAccount": {"accountUuid": uuid, "emailAddress": email}}).to_string()
}

fn dir700(p: &std::path::Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    store: ProfileStore,
    rs: RotateState,
    /// `work`'s login, live in `~/.claude`.
    live: u64,
    /// `spare`'s login.
    spare: u64,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    let root = base.join("root");
    let store = ProfileStore::new(root.clone());
    store.add(&n("work")).unwrap();
    store.add(&n("spare")).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    private(
        &root.join("work/.claude.json"),
        &acct("u-work", "work@example.com"),
    );
    private(
        &root.join("spare/.claude.json"),
        &acct("u-spare", "spare@example.com"),
    );
    let spare = login(&root.join("spare").join(CREDS));
    dir700(&home);
    dir700(&home.join(".claude"));
    private(
        &home.join(".claude.json"),
        &acct("u-work", "work@example.com"),
    );
    let live = login(&home.join(".claude").join(CREDS));
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    rs.save_ledger(&RotateLedger {
        live: Some("work".into()),
        account: Some("u-work".into()),
        login_identity: Some(super::login_identity::Identity::new(
            &(super::cred_identity::DEFAULT_READ)(&home.join(".claude")).unwrap(),
        )),
        since: Some(now() - 7200),
        swapped_at: Some(now() - 7200),
        ..RotateLedger::default()
    })
    .unwrap();
    let wrong =
        Account::from_block(&serde_json::json!({"accountUuid": "u-SENTINEL-recorded"})).unwrap();
    record(&State::new(&root).unwrap(), &n("work"), &wrong).unwrap();
    Fx {
        _tmp: tmp,
        home,
        root,
        store,
        rs,
        live,
        spare,
    }
}

/// `work` exhausted; `spare` well below every threshold.
fn work_out_spare_free(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let reset = Some(now() + 4 * 3600);
    let work = Probed {
        reading: Reading::Exhausted {
            session: Some(100.0),
            week: Some(40.0),
        },
        windows: Windows {
            session: Some(Win {
                used: 100.0,
                resets_at: reset,
            }),
            week: Some(Win {
                used: 40.0,
                resets_at: reset,
            }),
            no_session_limit: false,
        },
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
        passive: false,
        failure: None,
        started_at: None,
    };
    let spare = Probed {
        reading: Reading::Measured {
            session: 10.0,
            week: 10.0,
        },
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
        passive: false,
        failure: None,
        started_at: None,
    };
    decide_measured(ctx, snap, work, vec![(n("spare"), spare)], now())
}

impl Fx {
    fn ctx(&self) -> Ctx {
        let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
        let mut ctx = Ctx::new(self.store.clone(), self.home.clone(), s);
        ctx.scan = |_| Liveness::Known(BTreeSet::new());
        ctx.measure = Some(work_out_spare_free);
        ctx
    }

    fn logged(&self) -> Vec<String> {
        fs::read_to_string(&self.rs.log)
            .unwrap_or_default()
            .lines()
            .map(|l| l.split_once(' ').unwrap().1.to_string())
            .collect()
    }

    fn assert_nothing_moved(&self) {
        assert_eq!(
            ino_at(&self.home.join(".claude").join(CREDS)),
            Some(self.live)
        );
        assert_eq!(
            ino_at(&self.root.join("spare").join(CREDS)),
            Some(self.spare)
        );
        assert_eq!(ino_at(&self.root.join("work").join(CREDS)), None);
        assert!(!self.rs.journal.exists());
    }
}

fn assert_no_account(lines: &[String]) {
    for l in lines {
        for secret in [
            "u-work",
            "u-spare",
            "SENTINEL",
            // Issue #2013 (D10): refusal evidence names e-mail addresses.
            "sk-ant",
            "accessToken",
        ] {
            assert!(!l.contains(secret), "{secret:?} in {l}");
        }
    }
}

/// The control: without the record the same cycle swaps.
#[cfg(target_os = "linux")]
#[test]
fn without_a_mismatched_record_the_cycle_swaps() {
    let f = fx();
    fs::remove_file(f.root.join(".auto-switch.d/expected/work.json")).unwrap();
    let mut c = f.ctx();
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        f.logged()
    );
    // The ledger named `work` live with `u-work`, the outgoing account, so
    // the swap backfilled `work`'s record.
    let st = State::new(&f.root).unwrap();
    assert_eq!(recorded(&st, &n("work")).as_deref(), Some("u-work"));
    assert_eq!(recorded(&st, &n("spare")), None);
}

#[cfg(target_os = "linux")]
#[test]
fn a_mismatched_record_alerts_once_over_two_watch_cycles() {
    let f = fx();
    let mut c = f.ctx();
    for _ in 0..2 {
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Alerted,
            "{:?}",
            f.logged()
        );
        f.assert_nothing_moved();
    }
    let log = f.logged();
    let alerts: Vec<&String> = log.iter().filter(|l| l.starts_with("ALERT:")).collect();
    // Issue #2013 (D11): the second refusal is within the hour, so counted.
    assert_eq!(alerts, vec![ALERT], "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("swap ")), "{log:?}");
    assert_no_account(&log);
    let l = f.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.alert.as_deref(), Some("account_mismatch:work"));
    assert_eq!(l.live.as_deref(), Some("work"));
    // The record is never touched by a refused cycle.
    let st = State::new(&f.root).unwrap();
    assert_eq!(
        recorded(&st, &n("work")).as_deref(),
        Some("u-SENTINEL-recorded")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_one_shot_cycle_returns_the_refusal() {
    let f = fx();
    let mut c = f.ctx();
    match run_cycle(&mut c, Mode::OneShot) {
        Err(e @ ProfileError::RotateRefused(_)) => {
            assert!(super::swap::is_account_mismatch(&e), "{e}");
            assert_eq!(format!("ALERT: {e}"), REFUSED);
        }
        other => panic!("expected the refusal, got {other:?}"),
    }
    f.assert_nothing_moved();
    let log = f.logged();
    assert_eq!(log.iter().filter(|l| *l == ALERT).count(), 1, "{log:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn deleting_an_out_of_date_record_lets_the_next_cycle_swap() {
    // The documented fix when `work` is now meant to hold this account.
    let f = fx();
    let mut c = f.ctx();
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    fs::remove_file(f.root.join(".auto-switch.d/expected/work.json")).unwrap();
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        f.logged()
    );
    assert_eq!(
        ino_at(&f.root.join("work").join(CREDS)),
        Some(f.live),
        "work's login went back to work"
    );
    let l = f.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.alert, None, "a completed swap clears the ALERT key");
    // No other profile holds `u-work`, so the swap backfills `work`'s record
    // with the account it now holds, as the reference row for this ALERT says.
    let st = State::new(&f.root).unwrap();
    assert_eq!(recorded(&st, &n("work")).as_deref(), Some("u-work"));
    assert_eq!(recorded(&st, &n("spare")), None);
}

/// Simulates the documented fix: a `/login` in `~/.claude` to the account
/// recorded for `work` replaces the login there and its `oauthAccount`.
/// `work/.claude.json` still names the account before. Returns the new
/// login's inode.
fn relogin_to_recorded(f: &Fx) -> u64 {
    private(
        &f.home.join(".claude.json"),
        &serde_json::json!({
            "theme": "dark",
            "oauthAccount": {"accountUuid": "u-SENTINEL-recorded"},
        })
        .to_string(),
    );
    login(&f.home.join(".claude").join(CREDS))
}

#[cfg(target_os = "linux")]
#[test]
fn a_relogin_to_the_recorded_account_lets_the_next_cycle_swap() {
    // The fix the how-to gives first (crusty round 7): refuse, `/login` in
    // `~/.claude` to the recorded account, and the next cycle swaps.
    let f = fx();
    private(
        &f.root.join("work/.claude.json"),
        &serde_json::json!({
            "keep": 1,
            "oauthAccount": {"accountUuid": "u-work", "emailAddress": "work@example.com"},
        })
        .to_string(),
    );
    let mut c = f.ctx();
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    let relogged = relogin_to_recorded(&f);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        f.logged()
    );
    let log = f.logged();
    // Issue #1886: the relogin is no hand change; `work` stays live with no
    // recovery line, and the swap away from `work` files the login there.
    assert!(!log.iter().any(|l| l.starts_with("recovered:")), "{log:?}");
    assert!(!log.iter().any(|l| l.contains("no profile has")), "{log:?}");
    assert_no_account(&log);
    assert_eq!(
        ino_at(&f.root.join("work").join(CREDS)),
        Some(relogged),
        "the corrected login went to work"
    );
    assert_eq!(ino_at(&f.home.join(".claude").join(CREDS)), Some(f.spare));
    // `work/.claude.json` now names the recorded account, other keys kept,
    // so the next swap back into `work` puts that account in ~/.claude.json.
    let work: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.root.join("work/.claude.json")).unwrap())
            .unwrap();
    assert_eq!(work["keep"], 1);
    let a = Account::of_dir(&f.root.join("work")).unwrap();
    assert!(a.matches_record("u-SENTINEL-recorded", None), "{work}");
    let l = f.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.live.as_deref(), Some("spare"));
    assert_eq!(l.previous.as_deref(), Some("work"));
    assert_eq!(l.alert, None);
    let st = State::new(&f.root).unwrap();
    assert_eq!(
        recorded(&st, &n("work")).as_deref(),
        Some("u-SENTINEL-recorded")
    );
    assert_eq!(recorded(&st, &n("spare")), None);
}

#[cfg(target_os = "linux")]
#[test]
fn a_profile_json_that_cannot_be_updated_refuses_before_anything_moves() {
    let f = fx();
    private(&f.root.join("work/.claude.json"), "[]");
    let relogged = relogin_to_recorded(&f);
    let mut c = f.ctx();
    match run_cycle(&mut c, Mode::OneShot) {
        Err(ProfileError::RotateRefused(m)) => assert_eq!(
            m,
            format!(
                "{} work: it is not a JSON object",
                super::swap_expect::NOT_RELABELLED
            )
        ),
        other => panic!("expected the refusal, got {other:?}"),
    }
    assert_eq!(ino_at(&f.home.join(".claude").join(CREDS)), Some(relogged));
    assert_eq!(ino_at(&f.root.join("spare").join(CREDS)), Some(f.spare));
    assert_eq!(ino_at(&f.root.join("work").join(CREDS)), None);
    assert!(!f.rs.journal.exists());
    assert_eq!(
        fs::read_to_string(f.root.join("work/.claude.json")).unwrap(),
        "[]"
    );
}
