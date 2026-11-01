//! Issue #1804, crusty round 16: the rotator's record of the logins it
//! measured with no room, written by a real cycle and read by auto-switch's
//! real `OverflowInputs` and selection.
//!
//! The record is written every measuring cycle, whatever the capacity state:
//! round 15 read the capacity file instead, which the rotator writes as `ok`
//! (or not at all) whenever it can use the API key, and which the status
//! line and `profile usage` rewrite. When the rotator can use the key with
//! every plan login full it moves onto the key itself in that cycle, so its
//! live login is then the key. The selection case that remains is a rotator
//! that stays on a full plan login, as when its `~/.claude` key check fails
//! while the key profile's own files pass auto-switch's check; both cases are
//! below.

use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::config::Config;
use crate::commands::profile::auto_switch::kit::{Liveness, now};
use crate::commands::profile::auto_switch::{
    OverflowInputs, Pool, ReadingSource, SwitchReading, Window, select_with_overflow,
};
use crate::commands::profile::kind;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{Gate, OverflowEntry};
use crate::commands::profile::rotate::candidates::login_unusable;
use crate::commands::profile::rotate::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use crate::commands::profile::rotate::decide::{Probed, Reading, Win, Windows};
use crate::commands::profile::rotate::gate_record::GateSource;
use crate::commands::profile::rotate::key_check::KeyCheckFn;
use crate::commands::profile::rotate::ledger::{RotateLedger, RotateState};
use crate::commands::profile::rotate::observe::{Measurement, Snap, decide_measured};
use crate::commands::profile::rotate::plan_room::no_room::{self, MAX_SECS};
use crate::commands::profile::rotate::settings;
use crate::commands::profile::rotate::tests_swap::{login, private};
use crate::commands::profile::store::ProfileStore;
use amplihack_utils::claude_capacity as cap;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const CREDS: &str = ".credentials.json";
const KEY: &str = "api-key";
const WORK: &str = "work";
const CRED: &str = "cred";
const GATE: Gate = Gate {
    wait: 1800,
    unknown_reset: false,
    session_at: 85,
    week_at: 90,
};

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn acct(uuid: &str) -> String {
    serde_json::json!({"oauthAccount": {"accountUuid": uuid, "emailAddress": "a@example.com"}})
        .to_string()
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
}

/// `work` is live in `~/.claude`; `cred` is another plan login in its own
/// profile; `api-key` is an API-key profile marked with `overflow=api-key`.
fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (home, root) = (base.join("home"), base.join("root"));
    let store = ProfileStore::new(root.clone());
    store.add(&n(WORK)).unwrap();
    store.add(&n(KEY)).unwrap();
    store.add(&n(CRED)).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    private(&root.join(WORK).join(".claude.json"), &acct("u-work"));
    private(&root.join(CRED).join(".claude.json"), &acct("u-cred"));
    login(&root.join(CRED).join(CREDS));
    private(&root.join(KEY).join(".claude.json"), r#"{"numStartups":3}"#);
    private(&root.join(KEY).join(CREDS), "{}");
    private(&root.join(".auto-switch"), "enabled=on\noverflow=api-key\n");
    dir700(&home.join(".claude"));
    private(&home.join(".claude.json"), &acct("u-work"));
    login(&home.join(".claude").join(CREDS));
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
    }
}

/// A probe at `session`% of the session window, four hours from its reset,
/// so the overflow wait holds.
fn probed(session: f64) -> Probed {
    let resets_at = Some(now() + 4 * 3600);
    let reading = if session >= 100.0 {
        Reading::Exhausted
    } else {
        Reading::Measured {
            session,
            week: 40.0,
        }
    };
    let win = |used| Some(Win { used, resets_at });
    Probed {
        reading,
        windows: Windows {
            session: win(session),
            week: win(40.0),
        },
        spend_limit: false,
        unknown: false,
        still_running: false,
    }
}

/// `work` and `cred` at their session limits.
fn all_full(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let others = vec![(n(CRED), probed(100.0)), (n(KEY), Probed::platform())];
    decide_measured(ctx, snap, probed(100.0), others, now())
}

/// `work` well below every threshold, `cred` full.
fn work_roomy(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let others = vec![(n(CRED), probed(100.0)), (n(KEY), Probed::platform())];
    decide_measured(ctx, snap, probed(10.0), others, now())
}

impl Fx {
    fn cycle(
        &self,
        measure: crate::commands::profile::rotate::cycle::MeasureFn,
        check: KeyCheckFn,
    ) -> CycleOutcome {
        let s = settings::resolve(
            &crate::commands::profile::rotate::TuneFlags::default(),
            None,
            &|_| None,
        )
        .unwrap();
        let mut ctx = Ctx::new(self.store.clone(), self.home.clone(), s);
        ctx.scan = |_| Liveness::Known(BTreeSet::new());
        ctx.measure = Some(measure);
        ctx.key_check = check;
        run_cycle(&mut ctx, Mode::Watch).unwrap()
    }

    fn ledger(&self) -> RotateLedger {
        self.rs.load_ledger().unwrap().unwrap()
    }

    /// What auto-switch picks for a session on `cred`, whose status line
    /// recorded it at 100%. Auto-switch has no reading of `work`: no
    /// status line records the login in `~/.claude`. A profile is eligible
    /// with its login file in its own directory, as in `nested_candidate`.
    fn auto_switch_pick(&self) -> Option<String> {
        let cfg = Config {
            enabled: true,
            threshold: 90,
            exclude: Vec::new(),
            overflow: vec![OverflowEntry::new(n(KEY), 1)],
            kind: BTreeMap::new(),
        };
        let at = now();
        let gate = (Some(GATE), GateSource::Environment);
        let inputs = OverflowInputs::load_with_gate(&self.root, &cfg, gate, at);
        let profiles = vec![n(KEY), n(CRED), n(WORK)];
        let window = |used_percentage| {
            Some(Window {
                used_percentage,
                resets_at: at + 4 * 3600,
            })
        };
        let cred = SwitchReading {
            five_hour: window(100.0),
            seven_day: window(40.0),
            observed_at: at - 10,
            source: ReadingSource::Statusline,
        };
        let readings = BTreeMap::from([(CRED.to_string(), cred)]);
        let pool = Pool {
            profiles: &profiles,
            readings: &readings,
            threshold: 90,
            exclude: &[],
            now: at,
        };
        let own_login = |p: &ProfileName| self.root.join(p.as_str()).join(CREDS).exists();
        select_with_overflow(&pool, &inputs.pool(), CRED, own_login).map(|p| p.to_string())
    }
}

fn passes(_: &std::path::Path) -> Result<&'static str, String> {
    Ok("fake")
}

fn fails(_: &std::path::Path) -> Result<&'static str, String> {
    Err("fake failure".to_string())
}

fn recorded(f: &Fx) -> Vec<String> {
    let l = f.ledger();
    l.no_room
        .expect("the cycle records what it measured")
        .profiles
}

/// The rotator stays on `work`, full, because its `~/.claude` check of the
/// key fails. Auto-switch cannot read `work`, but the rotator's record says
/// it has no room, so it no longer holds the key back.
#[cfg(target_os = "linux")]
#[test]
fn a_rotator_that_finds_its_live_login_full_releases_the_key_to_auto_switch() {
    let f = fx();
    assert_eq!(f.auto_switch_pick(), None, "work unread: the key is held");
    assert_eq!(f.cycle(all_full, fails), CycleOutcome::Alerted);
    assert_eq!(f.ledger().live.as_deref(), Some(WORK), "no move");
    assert_eq!(recorded(&f), vec![CRED.to_string(), WORK.to_string()]);
    assert_eq!(f.auto_switch_pick().as_deref(), Some(KEY));
}

/// Round 15's case: every plan login full and the key usable. The rotator
/// computes `ok` and writes no capacity file, and it moves onto the key. The
/// record still names both plan logins, so `work`, now back in its own
/// directory and unread by auto-switch, is not taken as a login that may
/// have room.
#[cfg(target_os = "linux")]
#[test]
fn the_record_does_not_depend_on_the_capacity_state() {
    let f = fx();
    assert_eq!(f.cycle(all_full, passes), CycleOutcome::Swapped);
    assert!(
        cap::read_at(&f.root, chrono::Utc::now()).is_none_or(|c| c.state == cap::State::Ok),
        "the rotator computes ok when it can use the key"
    );
    let rec = f.ledger().no_room.unwrap();
    assert_eq!(rec.profiles, vec![CRED.to_string(), WORK.to_string()]);
    assert!(rec.until > rec.at && rec.until - rec.at <= MAX_SECS);
    let cfg = Config {
        enabled: true,
        threshold: 90,
        exclude: Vec::new(),
        overflow: vec![OverflowEntry::new(n(KEY), 1)],
        kind: BTreeMap::new(),
    };
    // The home directory, so the live `{}` login in `~/.claude` makes
    // `api-key` an API-key profile, as it does on the host.
    let inputs = OverflowInputs::load_at(&f.root, &cfg, now(), Some(&f.home));
    assert_eq!(inputs.platform, Some(n(KEY)));
    assert!(inputs.no_login.contains(&n(WORK)), "{:?}", inputs.no_login);
}

#[cfg(target_os = "linux")]
#[test]
fn a_login_the_rotator_finds_with_room_stays_unread_and_holds_the_key() {
    let f = fx();
    assert_eq!(f.cycle(work_roomy, fails), CycleOutcome::Stayed);
    assert_eq!(recorded(&f), vec![CRED.to_string()], "work had room");
    assert_eq!(f.auto_switch_pick(), None);
}

/// Once the record has expired auto-switch has no reading of the login
/// again.
#[cfg(target_os = "linux")]
#[test]
fn an_expired_record_says_nothing() {
    let f = fx();
    f.cycle(all_full, fails);
    let mut l = f.ledger();
    let rec = l.no_room.as_mut().unwrap();
    rec.at -= 2 * MAX_SECS;
    rec.until = rec.at + MAX_SECS;
    f.rs.save_ledger(&l).unwrap();
    assert_eq!(f.auto_switch_pick(), None);
    assert!(no_room::holding(l.no_room.as_ref(), now()).is_empty());
}

#[test]
fn a_failed_probe_and_the_api_key_are_never_recorded() {
    let others = [
        (n("cred"), Reading::Failed),
        (
            n("spare"),
            Reading::Measured {
                session: 100.0,
                week: 3.0,
            },
        ),
        (
            n("roomy"),
            Reading::Measured {
                session: 99.0,
                week: 99.0,
            },
        ),
        (n(KEY), Reading::Exhausted),
    ];
    let r = no_room::of_cycle(
        Some(&n(WORK)),
        &Reading::Exhausted,
        &others,
        Some(&n(KEY)),
        1000,
        1000,
        120,
    );
    assert_eq!(r.profiles, vec!["spare".to_string(), WORK.to_string()]);
    assert_eq!((r.at, r.until), (1000, 1240), "two intervals");
    let r = no_room::of_cycle(None, &Reading::Failed, &[], None, 1000, 1000, 86_400);
    assert_eq!(r.until, 1000 + MAX_SECS, "capped");
    assert!(no_room::valid(Some(&r)));
}

/// `until` runs from when the record is written and covers the next cycle,
/// which the daemon starts one interval after this one ended.
#[test]
fn the_span_counts_from_the_write_and_covers_a_cycle_as_slow_as_this_one() {
    let at = 1000;
    let of = |written, interval| {
        no_room::of_cycle(None, &Reading::Failed, &[], None, at, written, interval)
    };
    let (interval, length) = (120, 300);
    let written = at + length;
    let r = of(written, interval as u64);
    assert_eq!(r.at, at);
    let next_written = written + interval + length;
    assert!(r.until > written, "expired before it was written");
    assert!(r.until >= next_written, "a gap before the next record");
    assert!(no_room::valid(Some(&r)));

    // The hour after the measurement is the limit however slow the cycle.
    assert_eq!(of(at + 3599, 120).until, at + MAX_SECS);
    assert!(no_room::valid(Some(&of(at + 2 * MAX_SECS, 120))));
    // A clock stepped back mid-cycle shortens nothing.
    assert_eq!(of(at - 50, 120).until, at + 240);
}

/// The whole path on a host where measuring took five minutes, longer than
/// two intervals: the record is still in force when the cycle ends, so
/// auto-switch uses the key and does not say the login was not measured.
#[cfg(target_os = "linux")]
#[test]
fn a_cycle_slower_than_the_span_still_leaves_a_record_in_force() {
    fn slow(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
        let others = vec![(n(CRED), probed(100.0)), (n(KEY), Probed::platform())];
        decide_measured(ctx, snap, probed(100.0), others, now() - 300)
    }
    let f = fx();
    f.cycle(slow, fails);
    let l = f.ledger();
    let rec = l.no_room.as_ref().unwrap();
    assert!(now() - rec.at >= 300, "{rec:?}");
    assert!(rec.until > now() + 120, "{rec:?}");
    assert_eq!(recorded(&f), vec![CRED.to_string(), WORK.to_string()]);
    assert_eq!(f.auto_switch_pick().as_deref(), Some(KEY));
}

#[test]
fn a_ledger_with_a_record_the_rotator_could_not_write_is_refused() {
    let f = fx();
    let mut l = f.ledger();
    l.no_room = Some(no_room::NoRoom {
        at: 10,
        until: 10 + MAX_SECS + 1,
        profiles: vec![WORK.to_string()],
    });
    f.rs.save_ledger(&l).unwrap();
    assert!(f.rs.load_ledger().is_err());
    l.no_room = Some(no_room::NoRoom {
        at: 10,
        until: 20,
        profiles: vec!["../x".to_string()],
    });
    f.rs.save_ledger(&l).unwrap();
    assert!(f.rs.load_ledger().is_err());
}

/// Issue #1698 with #1804: a directory that fails the check is `directory
/// refused`, not `no login`. Its login cannot be checked, so the rotator
/// holds per-token moves for it and auto-switch must keep counting it unread.
#[test]
fn a_refused_directory_is_not_an_unusable_login() {
    let f = fx();
    let name = n(WORK);
    let dir = f.root.join(WORK);
    let resolved = kind::Ctx::load(&f.store, None).resolve_kind(&f.store, &name);
    assert!(
        login_unusable(&f.store, &name, &resolved),
        "a directory that passes the check, with no login in it, is unusable"
    );
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        !login_unusable(&f.store, &name, &resolved),
        "a refused directory says nothing about its login"
    );
}

/// Crusty round 20: the record ends at the first reset the cycle read for a
/// login it names. `cred`'s full session window resets 100 seconds after
/// the measurement, well inside the span, so the record holds until then
/// and names nothing after it: auto-switch then counts `cred` and `work`
/// unread again and holds the key back.
#[cfg(target_os = "linux")]
#[test]
fn a_record_ends_at_a_known_reset_of_a_login_it_names() {
    const SOON: i64 = 100;
    fn cred_resets_soon(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
        let at = now();
        let mut cred = probed(100.0);
        cred.windows.session = Some(Win {
            used: 100.0,
            resets_at: Some(at + SOON),
        });
        let others = vec![(n(CRED), cred), (n(KEY), Probed::platform())];
        decide_measured(ctx, snap, probed(100.0), others, at)
    }
    let f = fx();
    f.cycle(cred_resets_soon, fails);
    let l = f.ledger();
    let rec = l.no_room.as_ref().unwrap();
    assert_eq!(rec.until, rec.at + SOON, "{rec:?}");
    assert_eq!(recorded(&f), vec![CRED.to_string(), WORK.to_string()]);
    assert_eq!(
        no_room::holding(Some(rec), rec.at + SOON).len(),
        2,
        "held up to the reset"
    );
    assert!(
        no_room::holding(Some(rec), rec.at + SOON + 1).is_empty(),
        "nothing held after the reset"
    );
    assert!(no_room::valid(Some(rec)));
}

#[test]
fn room_comes_back_at_the_reset_of_the_full_window() {
    let w = |s: f64, sr: Option<i64>, k: f64, kr: Option<i64>| Windows {
        session: Some(Win {
            used: s,
            resets_at: sr,
        }),
        week: Some(Win {
            used: k,
            resets_at: kr,
        }),
    };
    let measured = |session, week| Reading::Measured { session, week };
    let back = no_room::room_back;
    let both = w(100.0, Some(50), 100.0, Some(900));
    assert_eq!(
        back(&measured(100.0, 40.0), &both),
        Some(50),
        "session full"
    );
    assert_eq!(back(&measured(40.0, 100.0), &both), Some(900), "week full");
    assert_eq!(back(&measured(100.0, 100.0), &both), Some(900), "the later");
    assert_eq!(
        back(&Reading::Exhausted, &w(100.0, Some(50), 3.0, Some(9))),
        Some(50)
    );
    assert_eq!(back(&measured(99.0, 99.0), &both), None, "room");
    assert_eq!(back(&Reading::Failed, &both), None);
    let unknown = w(100.0, None, 3.0, Some(9));
    assert_eq!(back(&Reading::Exhausted, &unknown), None, "reset unknown");

    // Only a login the record names shortens it, and never below `at`.
    let rec = no_room::of_cycle(
        Some(&n(WORK)),
        &measured(100.0, 3.0),
        &[(n(CRED), measured(10.0, 3.0))],
        None,
        1000,
        1000,
        120,
    );
    assert_eq!(rec.until, 1240);
    let cred_soon = w(10.0, Some(1010), 3.0, Some(1010));
    let cut = no_room::until_reset(rec.clone(), &[(&n(CRED), &measured(10.0, 3.0), &cred_soon)]);
    assert_eq!(cut.until, 1240, "cred is not named");
    let work_past = w(100.0, Some(500), 3.0, None);
    let cut = no_room::until_reset(rec, &[(&n(WORK), &measured(100.0, 3.0), &work_past)]);
    assert_eq!(cut.until, 1000);
    assert!(no_room::valid(Some(&cut)));
}
