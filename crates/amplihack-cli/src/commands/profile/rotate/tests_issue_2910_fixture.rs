//! Shared fixture of the issue #2910 cycle tests (`tests_issue_2910_cycle`,
//! `tests_issue_2910_unknown`). Wired from `target_seen.rs`.
//!
//! The swap, journal, ledger and log are the real ones, under a temporary HOME
//! and profile root (`tests_swap::env()`: `work` live, `spare` stored, plus the
//! stored profiles a test adds). Probing is stubbed with `Ctx::measure`; the
//! stub builds the cycle's `Extra` (labels and last good readings) the way
//! `measure_and_decide` does and then runs the real decision, `decide_with`.
//! The confirmation repeats the fixture values unless a test supplies its
//! own confirmation hook. Nothing here reads a
//! login, starts `claude`, uses the network or a tmux server, or inherits a
//! profile root. The environment is guarded: `HOME` and the profile root point
//! at the test's folder, and `CLAUDE_CONFIG_DIR` and every credential variable
//! are removed for the run (#2922).
//!
//! The decision clock is synthetic (`Ctx::now`), so a hold of 600 seconds is
//! exactly 600 seconds and a block ends exactly when a test says.
//!
//! In these fixtures the live login is `work`. On 22 November it was a login
//! called `rrowan`; fixtures use neutral names.
//!
//! The API the fixture uses:
//!
//! ```text
//! target_seen::Extra::for_cycle(root: &Path,
//!         labels: Vec<(ProfileName, ProbeLabel)>, kept: Vec<(ProfileName, i64)>,
//!         overflow: &[OverflowEntry], live: Option<&ProfileName>, now: i64) -> Extra
//! observe::decide_with(ctx, snap, probe: Probed, probes: Vec<(ProfileName, Probed)>,
//!         measured_at: i64, extra: &Extra) -> Result<Measurement, ProfileError>
//! measure::ProbeLabel                 // re-exported from probe_reading
//! fresh::RereadFn = fn(&Ctx, &ProfileName, Duration) -> (Probed, Option<ProbeLabel>)
//! ```

#![allow(dead_code)]

use super::Extra;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::cred_identity::DEFAULT_READ;
use crate::commands::profile::rotate::cycle::{Ctx, CycleOutcome, MeasureFn, Mode, run_cycle};
use crate::commands::profile::rotate::decide::Probed;
use crate::commands::profile::rotate::fresh::RereadFn;
use crate::commands::profile::rotate::ledger::OverflowBlock;
use crate::commands::profile::rotate::login_identity::Identity;
use crate::commands::profile::rotate::measure::ProbeLabel;
use crate::commands::profile::rotate::observe::{Measurement, Snap, decide_with};
use crate::commands::profile::rotate::settings::Settings;
use crate::commands::profile::rotate::tests_revalidate_cause::{
    add_stored, confirm_fixture as confirm_observation, ctx_with, log_of, probes_for,
};
use crate::commands::profile::rotate::tests_swap::{CREDS, Env, env, ino_at, login, private};
use crate::commands::profile::usage_cmd::ENV_CREDENTIAL_VARS;
use crate::commands::profile::usage_history::{self, Entry, Point, Writer};
use crate::test_support::{EnvGuard, env_lock};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

// --- The stub measurement ------------------------------------------------------

#[derive(Clone, Default)]
struct Scene {
    live: Option<Probed>,
    table: Vec<(String, Probed)>,
    labels: Vec<(String, ProbeLabel)>,
}

thread_local! {
    static SCENE: RefCell<Scene> = RefCell::new(Scene::default());
    static CLOCK: Cell<i64> = const { Cell::new(0) };
}

pub(in crate::commands::profile::rotate) fn clock() -> i64 {
    CLOCK.with(Cell::get)
}

pub(in crate::commands::profile::rotate) fn set_clock(t: i64) {
    CLOCK.with(|c| c.set(t));
}

pub(in crate::commands::profile::rotate) fn name(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

/// What `measure_and_decide` does after the probes: the cycle's `Extra`, then
/// the real decision on it.
pub(in crate::commands::profile::rotate) fn stub(
    ctx: &Ctx,
    snap: &Snap,
) -> Result<Measurement, ProfileError> {
    let sc = SCENE.with(|s| s.borrow().clone());
    let at = (ctx.now)();
    let table: Vec<(&str, Probed)> = sc
        .table
        .iter()
        .map(|(who, p)| (who.as_str(), p.clone()))
        .collect();
    let probes = probes_for(snap, &table);
    let labels = sc.labels.iter().map(|(who, l)| (name(who), *l)).collect();
    let extra = Extra::for_cycle(
        ctx.store.root(),
        labels,
        ctx.stored.times(at),
        &snap.decision_overflow(),
        snap.measured_live(),
        at,
    );
    decide_with(
        ctx,
        snap,
        sc.live.clone().expect("a live probe"),
        probes,
        at,
        &extra,
    )
}

fn confirm_fixture(
    ctx: &Ctx,
    who: &ProfileName,
    left: std::time::Duration,
) -> (Probed, Option<ProbeLabel>) {
    let (probe, _) = confirm_observation(ctx, who, left);
    let label = SCENE.with(|sc| {
        sc.borrow()
            .labels
            .iter()
            .find_map(|(name, label)| (name == who.as_str()).then_some(*label))
    });
    (probe, label)
}

// --- The fleet ----------------------------------------------------------------------

pub(in crate::commands::profile::rotate) struct Fleet {
    pub(in crate::commands::profile::rotate) e: Env,
    /// Each login's inode before any swap: after a swap to `x` the live login
    /// has `inodes["x"]`.
    pub(in crate::commands::profile::rotate) inodes: BTreeMap<String, u64>,
}

fn metadata(e: &Env, who: &str, org: Option<&str>) {
    let body = serde_json::json!({"oauthAccount": {
        "accountUuid": format!("u-{who}"),
        "emailAddress": format!("{who}@example.com"),
        "organizationUuid": org
    }})
    .to_string();
    private(&e.root.join(who).join(".claude.json"), &body);
    if who == "work" {
        private(&e.home.join(".claude.json"), &body);
        login(&e.home.join(".claude").join(CREDS));
    } else {
        login(&e.root.join(who).join(CREDS));
    }
}

/// `work` is live; every name in `names` is a stored profile. `orgs` gives an
/// organisation to the logins it names; the rest have none recorded.
pub(in crate::commands::profile::rotate) fn fleet(names: &[&str], orgs: &[(&str, &str)]) -> Fleet {
    let e = env();
    // `tests_swap::env` leaves an API key in `~/.claude.json`, which raises
    // the unrelated home-key ALERT on the first cycle; these tests count
    // ALERT lines, so the live login's file holds its account only.
    private(
        &e.home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": {
            "accountUuid": "u-work",
            "emailAddress": "work@example.com"
        }})
        .to_string(),
    );
    for who in names {
        add_stored(&e, who);
    }
    for (who, org) in orgs {
        metadata(&e, who, Some(org));
    }
    let mut l = e.rs.load_ledger().unwrap().unwrap();
    l.login_identity = Some(Identity::new(
        &(DEFAULT_READ)(&e.home.join(".claude")).unwrap(),
    ));
    e.rs.save_ledger(&l).unwrap();
    let mut inodes: BTreeMap<String, u64> = names
        .iter()
        .map(|who| {
            (
                who.to_string(),
                ino_at(&e.root.join(who).join(CREDS)).unwrap(),
            )
        })
        .collect();
    inodes.insert("work".into(), ino_at(&e.live()).unwrap());
    set_clock(now());
    Fleet { e, inodes }
}

impl Fleet {
    pub(in crate::commands::profile::rotate) fn live_is(&self, who: &str) -> bool {
        ino_at(&self.e.live()) == Some(self.inodes[who])
    }

    pub(in crate::commands::profile::rotate) fn log(&self) -> String {
        log_of(&self.e)
    }

    pub(in crate::commands::profile::rotate) fn set_swapped_at(&self, t: i64) {
        let mut l = self.e.rs.load_ledger().unwrap().unwrap();
        l.swapped_at = Some(t);
        self.e.rs.save_ledger(&l).unwrap();
    }

    pub(in crate::commands::profile::rotate) fn block(&self, who: &str, kind: &str, until: i64) {
        let mut l = self.e.rs.load_ledger().unwrap().unwrap();
        l.overflow_blocked.insert(
            who.into(),
            OverflowBlock {
                at: clock() - 60,
                until,
                reason: kind.into(),
            },
        );
        self.e.rs.save_ledger(&l).unwrap();
    }

    /// A point of the usage history, `age` seconds before the decision clock.
    pub(in crate::commands::profile::rotate) fn history(&self, who: &str, age: i64, s: u8, w: u8) {
        let dir = self.e.root.join(".auto-switch.d");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let point = Point {
            s: Some(s),
            w,
            w_reset: Some(clock() + 3 * 86_400),
            s_reset: None,
            no_session_limit: false,
            passive: false,
        };
        let entry = Entry {
            at: clock() - age,
            profiles: BTreeMap::from([(who.to_string(), point)]),
            by: Some(Writer::Rotate),
            unknown: Vec::new(),
        };
        usage_history::append_with(&self.e.root, &entry, clock(), None).unwrap();
    }

    /// Issue #3347: a per-model week for `who` in the stored readings
    /// (`usage.json`), `used` percent, observed 30 minutes before the decision
    /// clock and resetting in three days, so `who`'s model weeks are known.
    pub(in crate::commands::profile::rotate) fn model_week(&self, who: &str, used: f64) {
        use crate::commands::profile::auto_switch::kit::{
            ModelWeek, Reading, ReadingSource, State,
        };
        let dir = self.e.root.join(".auto-switch.d");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let state = State::new(&self.e.root).unwrap();
        let mut file = state.try_load_usage().unwrap();
        let at = clock() - 1800;
        file.accounts.insert(
            who.to_string(),
            Reading {
                five_hour: None,
                seven_day: None,
                week_models: vec![ModelWeek {
                    label: "Fable".into(),
                    used_percentage: used,
                    resets_at: Some(clock() + 3 * 86_400),
                    observed_at: at,
                }],
                observed_at: at,
                source: ReadingSource::Statusline,
            },
        );
        state.save_usage(&file).unwrap();
    }

    /// Marks `who` as an overflow (credit) profile of tier 1.
    pub(in crate::commands::profile::rotate) fn mark_overflow(&self, who: &str) {
        use crate::commands::profile::auto_switch::config::{self, ReadMode};
        config::update(&self.e.root, ReadMode::Editable, |c| {
            c.overflow
                .push(crate::commands::profile::overflow::OverflowEntry::new(
                    name(who),
                    1,
                ));
            Ok(())
        })
        .unwrap();
    }
}

/// The reporter's flags on 22 November: 85, 90, margin 3, upgrade margin 30,
/// hold 3600, ceiling 95.
pub(in crate::commands::profile::rotate) fn reporter() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 3,
        upgrade_margin: 30,
        hold: 3600,
        interval: 60,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
        target_week_max: 95,
    }
}

/// The thresholds the hosts run, `--session-at 95 --week-at 98`, with the
/// reporter's other flags and the default ceiling of 95.
pub(in crate::commands::profile::rotate) fn reporter_deployed() -> Settings {
    Settings {
        session_at: 95,
        week_at: 98,
        ..reporter()
    }
}

/// A label for messages and the settings it names.
pub(in crate::commands::profile::rotate) type Labelled = (&'static str, fn() -> Settings);

/// Both settings the 22 November tests run at.
pub(in crate::commands::profile::rotate) const BOTH: [Labelled; 2] =
    [("85/90", reporter), ("95/98", reporter_deployed)];

// --- The 22 November readings ---------------------------------------------------

/// Names of the logins in the 05:56:57Z cycle that have a reading. The live
/// login is `work`.
pub(in crate::commands::profile::rotate) const NAMES: &[&str] = &[
    "login-aa", "login-ab", "login-ad", "login-ae", "login-af", "login-ag", "login-aj", "login-ak",
    "login-al", "login-ao", "login-aq", "login-ar", "login-at", "ssorrel", "login-bd", "login-bh",
];

/// The probes of the 05:56:57Z cycle (and of 05:48Z, which differs only in
/// the live login's reading).
pub(in crate::commands::profile::rotate) fn nov22_table() -> Vec<(&'static str, Probed)> {
    use crate::commands::profile::rotate::tests_issue_1870::{exh, rd};
    vec![
        ("login-aa", exh(0.0, 100.0)),
        ("login-ab", exh(0.0, 100.0)),
        ("login-ad", exh(100.0, 74.0)),
        ("login-ae", rd(96.0, 82.0, None)),
        ("login-af", unread()),
        ("login-ag", exh(100.0, 12.0)),
        ("login-aj", rd(90.0, 10.0, None)),
        ("login-ak", unread()),
        ("login-al", rd(0.0, 95.0, None)),
        ("login-ao", exh(100.0, 50.0)),
        ("login-aq", exh(100.0, 99.0)),
        ("login-ar", rd(88.0, 20.0, None)),
        ("login-at", exh(100.0, 22.0)),
        ("ssorrel", rd(93.0, 22.0, None)),
        ("login-bd", rd(90.0, 71.0, None)),
        ("login-bh", rd(88.0, 71.0, None)),
    ]
}

pub(in crate::commands::profile::rotate) const STILL_RUNNING: &[(&str, ProbeLabel)] = &[
    ("login-af", ProbeLabel::StillRunning),
    ("login-ak", ProbeLabel::StillRunning),
];

/// An unread probe, as a 429 or a transport failure leaves it.
pub(in crate::commands::profile::rotate) fn unread() -> Probed {
    Probed::failed()
}

/// A probe that failed for a reason about the login: failed, not unknown.
pub(in crate::commands::profile::rotate) fn expired() -> Probed {
    Probed {
        unknown: false,
        ..Probed::failed()
    }
}

/// Runs one cycle on `table`, the probes of the stored logins, with the live
/// login's probe `live` and the labels the probes' errors earned.
#[allow(clippy::too_many_arguments)]
pub(in crate::commands::profile::rotate) fn run_full(
    f: &Fleet,
    s: Settings,
    live: Probed,
    table: &[(&str, Probed)],
    labels: &[(&str, ProbeLabel)],
    mode: Mode,
    reread: Option<RereadFn>,
    measure: MeasureFn,
) -> CycleOutcome {
    run_tuned(f, s, live, table, labels, mode, &|ctx| {
        ctx.reread = reread.or(ctx.reread);
        ctx.measure = Some(measure);
    })
}

/// [`run_full`] with the stub measurement, after `tune` has adjusted the
/// cycle's context (an identity reader, for example).
///
/// Every call builds a new [`Ctx`] from the ledger, the profile root and the
/// settings alone, as `daemon::watch` does when a rotator starts: no failed
/// live probe count, outlook, in-flight read or identity budget is carried
/// from an earlier call. So each call is also the first cycle of a rotator
/// started on that ledger.
pub(in crate::commands::profile::rotate) fn run_tuned(
    f: &Fleet,
    s: Settings,
    live: Probed,
    table: &[(&str, Probed)],
    labels: &[(&str, ProbeLabel)],
    mode: Mode,
    tune: &dyn Fn(&mut Ctx),
) -> CycleOutcome {
    SCENE.with(|sc| {
        *sc.borrow_mut() = Scene {
            live: Some(live),
            table: table
                .iter()
                .map(|(who, p)| (who.to_string(), p.clone()))
                .collect(),
            labels: labels
                .iter()
                .map(|(who, l)| (who.to_string(), *l))
                .collect(),
        };
    });
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut g = EnvGuard::set([
        ("HOME", f.e.home.to_str().unwrap()),
        ("AMPLIHACK_CLAUDE_PROFILE_ROOT", f.e.root.to_str().unwrap()),
    ]);
    g.clear("CLAUDE_CONFIG_DIR");
    for var in ENV_CREDENTIAL_VARS {
        g.clear(var);
    }
    let mut ctx = ctx_with(&f.e, stub);
    ctx.reread = Some(confirm_fixture);
    ctx.settings = s;
    ctx.now = clock;
    tune(&mut ctx);
    run_cycle(&mut ctx, mode).expect("a cycle does not end in an error for this")
}

pub(in crate::commands::profile::rotate) fn run_mode(
    f: &Fleet,
    s: Settings,
    live: Probed,
    table: &[(&str, Probed)],
    labels: &[(&str, ProbeLabel)],
    mode: Mode,
) -> CycleOutcome {
    run_full(f, s, live, table, labels, mode, None, stub)
}

pub(in crate::commands::profile::rotate) fn run(
    f: &Fleet,
    s: Settings,
    live: Probed,
    table: &[(&str, Probed)],
    labels: &[(&str, ProbeLabel)],
) -> CycleOutcome {
    run_mode(f, s, live, table, labels, Mode::Watch)
}

/// The log's ALERT lines.
pub(in crate::commands::profile::rotate) fn alerts(log: &str) -> Vec<&str> {
    log.lines().filter(|l| l.contains(" ALERT: ")).collect()
}

pub(in crate::commands::profile::rotate) fn lines_with<'a>(
    log: &'a str,
    needle: &str,
) -> Vec<&'a str> {
    log.lines().filter(|l| l.contains(needle)).collect()
}
