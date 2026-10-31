//! Issue #1804 (R4, R5): the login check in a real cycle, with the check
//! replaced through `Ctx.key_check`. The swap, journal, ledger and log are
//! the real ones, under a temporary HOME and profile root; only probing is
//! stubbed (`Ctx.measure`) and the decision is the real one
//! ([`decide_measured`]). No `claude`, no daemon, no tmux.
//!
//! Fixture: `work` is live in `~/.claude` (an OAuth login) and recorded so
//! in the ledger; `api-key` is an API-key profile (`{}` login, no
//! `oauthAccount`) marked with `overflow=api-key`. Every plan login is at
//! 100% and resets in four hours.
//!
//! Contract:
//! - `cycle::Ctx.key_check: key_check::KeyCheckFn`, defaulting to
//!   `key_check::reachable`.
//! - Pass (T3, D4): the swap runs. Before the `swap` line the log has one
//!   `per-token api-key: no plan login below 100%; overflow_wait elapsed
//!   (earliest reset <time>); marked overflow tier 10 via profile overflow
//!   add; login check passed (<detail>)` line, and after it `moved
//!   ~/.claude onto API key profile api-key (billed per token)`.
//! - Fail (T4, D3): no swap, no journal, `~/.claude` unchanged, no
//!   `per-token`, `swap` or `moved` line; a `key_check_refused api-key:
//!   <reason>` line and one ALERT, written once over two identical cycles;
//!   the cycle ends `alerted` and records no `overflow=in`.
//! - A move onto a credit profile writes the same `per-token` line with the
//!   credit tier and `probe ok, no spend limit`, and runs no login check.

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::decide::{Probed, Reading, Win, Windows};
use super::identify::LiveId;
use super::ledger::{RotateLedger, RotateState};
use super::observe::{Measurement, Snap, decide_measured};
use super::settings;
use super::tests_swap::{ino_at, login, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Liveness, now};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

const CREDS: &str = ".credentials.json";
const KEY: &str = "api-key";
const WORK: &str = "work";

const MOVED: &str = "moved ~/.claude onto API key profile api-key (billed per token)";
const REASON_HEAD: &str =
    "per-token api-key: no plan login below 100%; overflow_wait elapsed (earliest reset ";
const REASON_TAIL: &str =
    "); marked overflow tier 10 via profile overflow add; login check passed (fake)";
const REFUSED: &str = "key_check_refused api-key: fake failure";
const ALERT: &str =
    "ALERT: API key profile api-key failed its login check (fake failure); not moving onto it";

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
    /// `work`'s OAuth login, live in `~/.claude`.
    live: u64,
    /// `api-key`'s `{}` login file.
    key: u64,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    let root = base.join("root");
    let store = ProfileStore::new(root.clone());
    store.add(&n(WORK)).unwrap();
    store.add(&n(KEY)).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    private(
        &root.join(WORK).join(".claude.json"),
        &acct("u-work", "work@example.com"),
    );
    private(&root.join(KEY).join(".claude.json"), r#"{"numStartups":3}"#);
    private(&root.join(KEY).join(CREDS), "{}");
    let key = fs::symlink_metadata(root.join(KEY).join(CREDS))
        .unwrap()
        .ino();
    private(
        &root.join(".auto-switch"),
        "enabled=off\noverflow=api-key\n",
    );
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
        live: Some(WORK.into()),
        account: Some("u-work".into()),
        since: Some(now() - 7200),
        swapped_at: Some(now() - 7200),
        ..RotateLedger::default()
    })
    .unwrap();
    Fx {
        _tmp: tmp,
        home,
        root,
        store,
        rs,
        live,
        key,
    }
}

impl Fx {
    fn ctx(&self, check: super::key_check::KeyCheckFn) -> Ctx {
        let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
        let mut ctx = Ctx::new(self.store.clone(), self.home.clone(), s);
        ctx.scan = |_| Liveness::Known(BTreeSet::new());
        ctx.measure = Some(all_plan_logins_out);
        ctx.key_check = check;
        ctx
    }

    fn logged(&self) -> Vec<String> {
        fs::read_to_string(&self.rs.log)
            .unwrap_or_default()
            .lines()
            .map(|l| l.split_once(' ').unwrap().1.to_string())
            .collect()
    }

    fn home_login(&self) -> Option<u64> {
        ino_at(&self.home.join(".claude").join(CREDS))
    }

    fn key_slot(&self) -> Option<u64> {
        ino_at(&self.root.join(KEY).join(CREDS))
    }
}

/// `work` exhausted and resetting in four hours; `api-key` with its
/// synthetic reading. The snapshot must hold the marked API-key profile.
fn all_plan_logins_out(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    assert_eq!(snap.live, LiveId::Known(n(WORK)));
    assert_eq!(snap.platform(), Some(&n(KEY)), "api-key is marked");
    let reset = Some(now() + 4 * 3600);
    let out = Probed {
        reading: Reading::Exhausted,
        windows: Windows {
            session: Some(Win {
                used: 100.0,
                resets_at: reset,
            }),
            week: Some(Win {
                used: 40.0,
                resets_at: reset,
            }),
        },
        spend_limit: false,
    };
    decide_measured(ctx, snap, out, vec![(n(KEY), Probed::platform())], now())
}

#[cfg(target_os = "linux")]
#[test]
fn t3_a_passing_login_check_moves_onto_the_api_key_and_logs_why() {
    let f = fx();
    let mut c = f.ctx(|_| Ok("fake"));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = f.logged();
    assert_eq!(got, CycleOutcome::Swapped, "{log:?}");
    assert_eq!(f.home_login(), Some(f.key), "the {{}} login is live");
    assert_eq!(
        ino_at(&f.root.join(WORK).join(CREDS)),
        Some(f.live),
        "work's login went back to its profile"
    );
    let pos = |pred: &dyn Fn(&String) -> bool| log.iter().position(pred);
    let reason = pos(&|l| l.starts_with(REASON_HEAD) && l.ends_with(REASON_TAIL))
        .unwrap_or_else(|| panic!("no per-token line: {log:?}"));
    let swap = pos(&|l| l.starts_with("swap work->api-key") && l.ends_with("reason=overflow-in"))
        .unwrap_or_else(|| panic!("no swap line: {log:?}"));
    let moved = pos(&|l| l == MOVED).unwrap_or_else(|| panic!("no moved line: {log:?}"));
    assert!(reason < swap && swap < moved, "D4 order: {log:?}");
    assert_eq!(
        log.iter().filter(|l| l.starts_with("per-token ")).count(),
        1,
        "{log:?}"
    );
    let l = f.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.live.as_deref(), Some(KEY));
    assert_eq!(l.last_cycle.unwrap().overflow.as_deref(), Some("in"));
}

#[cfg(target_os = "linux")]
#[test]
fn t4_a_failing_login_check_moves_nothing_and_alerts_once() {
    let f = fx();
    let creds = f.home.join(".claude").join(CREDS);
    let before = fs::symlink_metadata(&creds).unwrap();
    let listing = |p: &std::path::Path| {
        let mut v: Vec<String> = fs::read_dir(p)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    let home_listing = listing(&f.home.join(".claude"));
    let mut c = f.ctx(|_| Err("fake failure".to_string()));

    for cycle in 1..=2 {
        let got = run_cycle(&mut c, Mode::Watch).unwrap();
        assert_eq!(
            got,
            CycleOutcome::Alerted,
            "cycle {cycle}: {:?}",
            f.logged()
        );
    }

    let after = fs::symlink_metadata(&creds).unwrap();
    assert_eq!(after.ino(), before.ino(), "~/.claude keeps work's login");
    assert_eq!(after.mtime(), before.mtime());
    assert_eq!(after.mtime_nsec(), before.mtime_nsec());
    assert_eq!(listing(&f.home.join(".claude")), home_listing);
    assert_eq!(f.home_login(), Some(f.live));
    assert_eq!(f.key_slot(), Some(f.key), "api-key keeps its {{}} login");
    assert!(!f.rs.journal.exists(), "no journal was written");

    let log = f.logged();
    assert!(
        !log.iter().any(|l| l.starts_with("swap ")
            || l.starts_with("moved ")
            || l.starts_with("per-token ")),
        "{log:?}"
    );
    assert!(log.iter().any(|l| l == REFUSED), "{log:?}");
    assert_eq!(
        log.iter().filter(|l| *l == ALERT).count(),
        1,
        "one ALERT over two identical cycles: {log:?}"
    );
    let l = f.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.live.as_deref(), Some(WORK));
    let last = l.last_cycle.unwrap();
    assert_eq!(last.outcome, "alerted");
    assert_ne!(
        last.overflow.as_deref(),
        Some("in"),
        "no move onto overflow recorded"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_default_check_is_the_real_one_and_refuses_a_profile_with_no_key() {
    let f = fx();
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut c = Ctx::new(f.store.clone(), f.home.clone(), s);
    c.scan = |_| Liveness::Known(BTreeSet::new());
    c.measure = Some(all_plan_logins_out);
    // `key_check` left at its default.
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    assert_eq!(f.home_login(), Some(f.live));
    let log = f.logged();
    assert!(
        log.iter().any(|l| l
            == "key_check_refused api-key: \
                no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting \
                in ~/.claude.json or ~/.claude/settings.json"),
        "{log:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_default_check_refuses_a_key_the_swap_would_not_deliver() {
    // The outage: the profile names a key, ~/.claude does not. Moving would
    // leave plain `claude` with a `{}` login and no key: "not logged in".
    let f = fx();
    private(
        &f.root.join(KEY).join(".claude.json"),
        r#"{"primaryApiKey":"SENTINEL-1804-KEY"}"#,
    );
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut c = Ctx::new(f.store.clone(), f.home.clone(), s);
    c.scan = |_| Liveness::Known(BTreeSet::new());
    c.measure = Some(all_plan_logins_out);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    assert_eq!(f.home_login(), Some(f.live));
    assert!(
        !f.logged().iter().any(|l| l.contains("login check passed")),
        "{:?}",
        f.logged()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn after_a_swap_onto_an_api_key_the_key_is_where_claude_reads_it() {
    let f = fx();
    let mut home: serde_json::Value =
        serde_json::from_slice(&fs::read(f.home.join(".claude.json")).unwrap()).unwrap();
    home["primaryApiKey"] = "SENTINEL-1804-KEY".into();
    private(&f.home.join(".claude.json"), &home.to_string());
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut c = Ctx::new(f.store.clone(), f.home.clone(), s);
    c.scan = |_| Liveness::Known(BTreeSet::new());
    c.measure = Some(all_plan_logins_out);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );
    assert_eq!(f.home_login(), Some(f.key), "the {{}} login is now live");
    // Read after the swap, from the paths plain `claude` uses.
    assert_eq!(
        super::key_check::reachable(&f.home),
        Ok("primaryApiKey set")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_default_check_passes_a_key_in_home_and_never_logs_the_value() {
    let f = fx();
    private(
        &f.home.join(".claude.json"),
        r#"{"primaryApiKey":"SENTINEL-1804-KEY","oauthAccount":{"accountUuid":"u-work","emailAddress":"work@example.com"}}"#,
    );
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut c = Ctx::new(f.store.clone(), f.home.clone(), s);
    c.scan = |_| Liveness::Known(BTreeSet::new());
    c.measure = Some(all_plan_logins_out);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );
    let raw = fs::read_to_string(&f.rs.log).unwrap();
    assert!(
        raw.contains("login check passed (primaryApiKey set)"),
        "{raw}"
    );
    assert!(!raw.contains("SENTINEL-1804-KEY"), "the key value leaked");
    let ledger = fs::read_to_string(&f.rs.ledger).unwrap();
    assert!(!ledger.contains("SENTINEL-1804-KEY"));
}

#[cfg(target_os = "linux")]
#[test]
fn a_failing_check_leaves_home_claude_json_byte_for_byte_unchanged() {
    let f = fx();
    let before = fs::read(f.home.join(".claude.json")).unwrap();
    let mut c = f.ctx(|_| Err("fake failure".to_string()));
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    assert_eq!(fs::read(f.home.join(".claude.json")).unwrap(), before);
    assert_eq!(
        fs::read(f.root.join(KEY).join(".claude.json")).unwrap(),
        br#"{"numStartups":3}"#.to_vec(),
        "the API-key profile's own files are untouched"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_failing_check_never_puts_a_key_value_in_the_log_or_ledger() {
    // The real check, with the key in a file it refuses to trust.
    let f = fx();
    private(
        &f.home.join(".claude/settings.json"),
        r#"{"apiKeyHelper":"echo SENTINEL-1804-KEY"}"#,
    );
    fs::set_permissions(
        f.home.join(".claude/settings.json"),
        fs::Permissions::from_mode(0o620),
    )
    .unwrap();
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut c = Ctx::new(f.store.clone(), f.home.clone(), s);
    c.scan = |_| Liveness::Known(BTreeSet::new());
    c.measure = Some(all_plan_logins_out);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    let log = f.logged();
    assert!(
        log.iter()
            .any(|l| l == "key_check_refused api-key: settings.json is writable by group or others"),
        "{log:?}"
    );
    let raw = fs::read_to_string(&f.rs.log).unwrap();
    assert!(!raw.contains("SENTINEL-1804-KEY"));
    // The refusal writes no journal entry, so the ledger may not exist.
    match fs::read_to_string(&f.rs.ledger) {
        Ok(ledger) => assert!(!ledger.contains("SENTINEL-1804-KEY")),
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
    }
}

// --- A move onto a credit profile (D4) ------------------------------------------------

const CREDITS: &str = "team-credits";

/// The fixture with `team-credits`, an OAuth credit profile, marked at
/// tier 2 ahead of the API key.
fn fx_credits() -> Fx {
    let f = fx();
    f.store.add(&n(CREDITS)).unwrap();
    private(
        &f.root.join(CREDITS).join(".claude.json"),
        &acct("u-credits", "credits@example.com"),
    );
    login(&f.root.join(CREDITS).join(CREDS));
    private(
        &f.root.join(".auto-switch"),
        "enabled=off\noverflow=team-credits:2,api-key\n",
    );
    f
}

/// As [`all_plan_logins_out`], with `team-credits` usable.
fn plan_out_credits_usable(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    assert_eq!(snap.live, LiveId::Known(n(WORK)));
    let reset = Some(now() + 4 * 3600);
    let win = |used| {
        Some(Win {
            used,
            resets_at: reset,
        })
    };
    let out = Probed {
        reading: Reading::Exhausted,
        windows: Windows {
            session: win(100.0),
            week: win(40.0),
        },
        spend_limit: false,
    };
    let credits = Probed {
        reading: Reading::Measured {
            session: 10.0,
            week: 10.0,
        },
        windows: Windows {
            session: win(10.0),
            week: win(10.0),
        },
        spend_limit: false,
    };
    decide_measured(
        ctx,
        snap,
        out,
        vec![(n(CREDITS), credits), (n(KEY), Probed::platform())],
        now(),
    )
}

#[cfg(target_os = "linux")]
#[test]
fn a_move_onto_credits_writes_the_per_token_line_with_its_tier_and_no_login_check() {
    let f = fx_credits();
    let mut c = f.ctx(|_| panic!("no login check for a credit profile"));
    c.measure = Some(plan_out_credits_usable);
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = f.logged();
    assert_eq!(got, CycleOutcome::Swapped, "{log:?}");
    let pos = |pred: &dyn Fn(&String) -> bool| log.iter().position(pred);
    let reason = pos(&|l| {
        l.starts_with(
            "per-token team-credits: no plan login below 100%; overflow_wait elapsed \
             (earliest reset ",
        ) && l.ends_with(
            "); marked overflow tier 2 via profile overflow add; probe ok, no spend limit",
        )
    })
    .unwrap_or_else(|| panic!("no per-token line: {log:?}"));
    let swap =
        pos(&|l| l.starts_with("swap work->team-credits") && l.ends_with("reason=overflow-in"))
            .unwrap_or_else(|| panic!("no swap line: {log:?}"));
    let on = pos(&|l| {
        l.starts_with("overflow: on team-credits; no plan login below 100%, next plan reset ")
    })
    .unwrap_or_else(|| panic!("no overflow: on line: {log:?}"));
    assert!(reason < swap && swap < on, "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("moved ")), "{log:?}");
}

/// The rotator with a corrupt `.auto-switch`: `tiers_detail` gives no tiers,
/// so a marked credit profile is a candidate like a plan login. This pins
/// today's behaviour, which issue #1805 tracks; API-key profiles stay out.
#[cfg(target_os = "linux")]
#[test]
fn a_corrupt_auto_switch_file_loses_the_marks_for_the_rotator() {
    let f = fx_credits();
    private(&f.root.join(".auto-switch"), "this is not a setting\n");
    let c = f.ctx(|_| panic!("no login check"));
    let state = crate::commands::profile::auto_switch::kit::State::new(&f.root).unwrap();
    let snap = super::observe::snapshot(&c, &state).unwrap();
    assert!(snap.overflow.is_empty(), "{:?}", snap.overflow);
    assert!(
        snap.candidates.contains(&n(CREDITS)),
        "{:?}",
        snap.candidates
    );
    assert!(!snap.candidates.contains(&n(KEY)), "{:?}", snap.candidates);
}
