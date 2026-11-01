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
//!   `per-token api-key: no measured plan login below 100%, 0 not measured; earliest reset <time> beyond overflow_wait; marked overflow tier 10 via profile overflow
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
use super::overflow_record::refresh_capacity;
use super::settings;
use super::tests_swap::{ino_at, login, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Liveness, now};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use amplihack_utils::claude_capacity as cap;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

const CREDS: &str = ".credentials.json";
const KEY: &str = "api-key";
const WORK: &str = "work";

const MOVED: &str = "moved ~/.claude onto API key profile api-key (billed per token)";
const REASON_HEAD: &str =
    "per-token api-key: no measured plan login below 100%, 0 not measured; earliest reset ";
const REASON_TAIL: &str =
    " beyond overflow_wait; marked overflow tier 10; login check passed (fake)";
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

/// `work`'s `~/.claude.json` with a key in it.
const HOME_WITH_KEY: &str = r#"{"oauthAccount":{"accountUuid":"u-work",
    "emailAddress":"work@example.com"},"primaryApiKey":"SENTINEL-1804-KEY"}"#;

/// `work` measured well below every threshold, so the cycle stays.
fn plenty_of_room(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let live = Probed {
        reading: Reading::Measured {
            session: 10.0,
            week: 10.0,
        },
        windows: Windows::default(),
        spend_limit: false,
    };
    decide_measured(ctx, snap, live, vec![(n(KEY), Probed::platform())], now())
}

/// A home key bills a plan session per token with no move at all, so the
/// ALERT cannot depend on one, and a stay must not repeat it.
#[cfg(target_os = "linux")]
#[test]
fn a_home_key_under_a_live_plan_login_alerts_once_and_again_when_it_returns() {
    let f = fx();
    let mut c = f.ctx(|_| panic!("a stay runs no login check"));
    c.measure = Some(plenty_of_room);
    let home_json = f.home.join(".claude.json");
    let without_key = fs::read_to_string(&home_json).unwrap();
    let with_key = HOME_WITH_KEY;
    private(&home_json, with_key);
    let alerts = |f: &Fx| {
        f.logged()
            .iter()
            .filter(|l| l.starts_with("ALERT: home_key (primaryApiKey set): "))
            .count()
    };
    for _ in 0..3 {
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Stayed
        );
    }
    assert_eq!(alerts(&f), 1, "{:?}", f.logged());
    private(&home_json, &without_key);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Stayed
    );
    assert_eq!(alerts(&f), 1, "{:?}", f.logged());
    private(&home_json, with_key);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Stayed
    );
    assert_eq!(alerts(&f), 2, "{:?}", f.logged());
    assert!(!f.logged().iter().any(|l| l.contains("SENTINEL-1804-KEY")));
}

/// A settings file the login check rejects still costs money if Claude reads
/// it, so the ALERT cannot depend on the check.
#[cfg(target_os = "linux")]
#[test]
fn a_symlinked_settings_key_still_alerts() {
    let f = fx();
    let target = f.home.join("dotfiles-settings.json");
    private(&target, r#"{"apiKeyHelper":"echo SENTINEL-1804-KEY"}"#);
    std::os::unix::fs::symlink(&target, f.home.join(".claude/settings.json")).unwrap();
    assert!(super::key_check::reachable(&f.home).is_err());
    let mut c = f.ctx(|_| panic!("a stay runs no login check"));
    c.measure = Some(plenty_of_room);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Stayed
    );
    let log = f.logged();
    let alerts = log
        .iter()
        .filter(|l| l.starts_with("ALERT: home_key (apiKeyHelper set): "));
    assert_eq!(alerts.count(), 1, "{log:?}");
    assert!(!log.iter().any(|l| l.contains("SENTINEL-1804-KEY")));
}

/// `work`'s probe fails and `spare`, a plan login with room, reads well. A
/// home key does not change that: `spare` is probed through its own directory
/// and fails only once it is live. So the swap looks useful, and is not.
fn live_probe_fails(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let failed = Probed {
        reading: Reading::Failed,
        windows: Windows::default(),
        spend_limit: false,
    };
    let spare = Probed {
        reading: Reading::Measured {
            session: 10.0,
            week: 10.0,
        },
        windows: Windows::default(),
        spend_limit: false,
    };
    decide_measured(ctx, snap, failed, vec![(n("spare"), spare)], now())
}

/// `f` with a plan login `spare`, and a context three failed cycles in, past
/// the hold (`fx` swapped two hours ago).
fn failing(f: &Fx) -> Ctx {
    let spare = f.root.join("spare");
    f.store.add(&n("spare")).unwrap();
    private(
        &spare.join(".claude.json"),
        &acct("u-spare", "spare@example.com"),
    );
    login(&spare.join(CREDS));
    let mut c = f.ctx(|_| panic!("no move onto the API key here"));
    c.measure = Some(live_probe_fails);
    c.failed_live = 2;
    c
}

/// The control for the test below: without the key this swap does happen.
#[cfg(target_os = "linux")]
#[test]
fn failed_probes_swap_when_no_home_key_explains_them() {
    let f = fx();
    let mut c = failing(&f);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        f.logged()
    );
    assert!(
        f.logged()
            .iter()
            .any(|l| l.ends_with("reason=failed-probe"))
    );
}

/// A home key applies to every login once it is live, so the swap changes
/// nothing, however many cycles pass: no hourly churn between logins.
#[cfg(target_os = "linux")]
#[test]
fn a_home_key_stops_failed_probe_swaps_for_good() {
    let f = fx();
    private(&f.home.join(".claude.json"), HOME_WITH_KEY);
    let mut c = failing(&f);
    for _ in 0..4 {
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Stayed
        );
    }
    assert_eq!(f.home_login(), Some(f.live), "work's login stays live");
    let log = f.logged();
    assert!(!log.iter().any(|l| l.starts_with("swap ")), "{log:?}");
    let stays = log.iter().filter(|l| {
        *l == "stay: live probe failed; home key (primaryApiKey set) applies to every login once live, a swap changes nothing"
    });
    assert_eq!(stays.count(), 4, "{log:?}");
    assert!(c.failed_live >= 6, "the count goes on: {}", c.failed_live);
}

/// A symlinked key may never reach Claude, so the warning reader's view of it
/// is no ground to stay. The live login may simply be dead: the swap goes
/// ahead, and the ALERT still says the key might be billing.
#[cfg(target_os = "linux")]
#[test]
fn a_key_the_login_check_rejects_does_not_stop_a_failed_probe_swap() {
    let f = fx();
    let target = f.home.join("dotfiles-settings.json");
    private(&target, r#"{"apiKeyHelper":"echo SENTINEL-1804-KEY"}"#);
    std::os::unix::fs::symlink(&target, f.home.join(".claude/settings.json")).unwrap();
    assert!(super::key_check::reachable(&f.home).is_err());
    assert!(super::key_check::home_key(&f.home).is_some());
    let mut c = failing(&f);
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = f.logged();
    assert_eq!(got, CycleOutcome::Swapped, "{log:?}");
    assert!(
        log.iter().any(|l| l.ends_with("reason=failed-probe")),
        "{log:?}"
    );
    assert!(!log.iter().any(|l| l.contains("home key (")), "{log:?}");
    assert!(
        log.iter()
            .any(|l| l.starts_with("ALERT: home_key (apiKeyHelper set): ")),
        "{log:?}"
    );
}

/// On a live API-key profile the key is the login in use, not a surprise.
#[cfg(target_os = "linux")]
#[test]
fn a_live_api_key_raises_no_home_key_alert() {
    let f = fx();
    private(&f.home.join(".claude.json"), HOME_WITH_KEY);
    let mut c = f.ctx(|_| Ok("fake"));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    assert_eq!(got, CycleOutcome::Swapped);
    // A later process has no memory of the first ALERT.
    c.home_key = None;
    c.measure = Some(plenty_of_room);
    run_cycle(&mut c, Mode::Watch).unwrap();
    let log = f.logged();
    let alerts = log.iter().filter(|l| l.starts_with("ALERT: home_key"));
    assert_eq!(
        alerts.count(),
        1,
        "only the cycle on the plan login: {log:?}"
    );
}

/// The 30 Oct state with the key gone while the API-key profile is live:
/// being on overflow is no reason to tell consumers work can go on.
#[cfg(target_os = "linux")]
#[test]
fn a_live_api_key_that_fails_its_check_does_not_read_as_ok() {
    let f = fx();
    private(
        &f.root.join(".auto-switch"),
        "enabled=on\noverflow=api-key\n",
    );
    let t = now();
    let plan = [(n(WORK), Reading::Exhausted)];
    let key = n(KEY);
    let write = |c: &Ctx| {
        let live = Reading::Measured {
            session: 0.0,
            week: 0.0,
        };
        let tag = Some(super::overflow::Tag::On);
        refresh_capacity(c, Some(&key), &live, &plan, t, tag, Some(&key));
    };
    let read = || {
        let at = chrono::DateTime::from_timestamp(t, 0).unwrap();
        cap::read_at(&f.root, at).map(|file| file.state)
    };
    let c = f.ctx(|_| panic!("the capacity file uses the real check"));
    write(&c);
    assert_eq!(read(), Some(cap::State::Exhausted));
    private(&f.home.join(".claude.json"), HOME_WITH_KEY);
    write(&c);
    assert_eq!(read(), Some(cap::State::Ok), "a passing check keeps `ok`");
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
            "per-token team-credits: no measured plan login below 100%, 0 not measured; earliest reset ",
        ) && l.ends_with(
            " beyond overflow_wait; marked overflow tier 2; probe ok, no spend limit",
        )
    })
    .unwrap_or_else(|| panic!("no per-token line: {log:?}"));
    let swap =
        pos(&|l| l.starts_with("swap work->team-credits") && l.ends_with("reason=overflow-in"))
            .unwrap_or_else(|| panic!("no swap line: {log:?}"));
    let on = pos(&|l| {
        l.starts_with(
            "overflow: on team-credits; no measured plan login below 100%, next plan reset ",
        )
    })
    .unwrap_or_else(|| panic!("no overflow: on line: {log:?}"));
    assert!(reason < swap && swap < on, "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("moved ")), "{log:?}");
}

/// Crusty round 13, through a whole cycle: every measured plan login is at
/// 100% and the API key is marked, but `busy` has a session on it, so it is
/// skipped and may have room. The key stays out and the ALERT names it.
#[cfg(target_os = "linux")]
#[test]
fn a_plan_login_with_a_session_on_it_keeps_the_rotator_off_the_api_key() {
    let f = fx();
    f.store.add(&n("busy")).unwrap();
    login(&f.root.join("busy").join(CREDS));
    let mut c = f.ctx(|_| panic!("no login check: the key must not be a target"));
    c.scan = |_| Liveness::Known(BTreeSet::from(["busy".to_string()]));
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    let log = f.logged();
    assert_eq!(got, CycleOutcome::Alerted, "{log:?}");
    assert!(
        log.iter()
            .any(|l| l.starts_with("ALERT: API key not used live=work: ")
                && l.ends_with("may have: busy")),
        "{log:?}"
    );
    assert!(!log.iter().any(|l| l.starts_with("per-token ")), "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("swap ")), "{log:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_mark_on_a_detected_key_is_reported_and_a_kind_set_one_is_not() {
    let f = fx();
    let c = f.ctx(|_| panic!("no login check"));
    let state = crate::commands::profile::auto_switch::kit::State::new(&f.root).unwrap();
    let snap = super::observe::snapshot(&c, &state).unwrap();
    assert_eq!(snap.detected_mark, Some(n(KEY)));
    private(
        &f.root.join(".auto-switch"),
        "enabled=off\noverflow=api-key\nkind=api-key:platform\n",
    );
    let snap = super::observe::snapshot(&c, &state).unwrap();
    assert_eq!(snap.overflow.len(), 1, "{:?}", snap.overflow);
    assert_eq!(snap.detected_mark, None);
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

#[path = "tests_no_room.rs"]
mod tests_no_room;
