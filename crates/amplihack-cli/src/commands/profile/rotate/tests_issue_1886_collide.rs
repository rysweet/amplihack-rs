//! Issue #1886, the 3 November collision (crusty round 2, concern 2):
//! nnorth and mmorrison reset at the same times. A swap leaves the live
//! profile `work` with no login of its own, its login only in `~/.claude`;
//! `spare` holds its own login of another account whose usage windows reset
//! at exactly the same moments. With identity by reset times `~/.claude`
//! read as `spare`, the suspicion stopped the cycle and then the only-login
//! ALERT stopped rotation. With identity by the OAuth profile endpoint the
//! two are two accounts, nothing is suspected, and the swap goes ahead.
//!
//! The identity reader is the real `cred_identity::read_with` against the
//! loopback `FakeHttp`, which answers both the usage and the profile
//! endpoint for each fake `fake-at-1886-<uuid>` token. Measuring is stubbed
//! (`Ctx.measure`). No `claude`, no daemon, no process variable.

#![cfg(target_os = "linux")]

use super::cred_identity::read_with;
use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::decide::{Probed, Reading, Win, Windows};
use super::identify::Account;
use super::ledger::{RotateLedger, RotateState};
use super::observe::{Measurement, Snap, decide_measured};
use super::settings;
use super::tests_issue_1886::{creds_doc, endpoints, fake_token, profile_body, usage_body};
use super::tests_swap::{ino_at, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Liveness, now};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::tests_fake_http::{FakeHttp, Reply};
use crate::commands::profile::usage_direct::{self, Endpoints, Request};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const CREDS: &str = ".credentials.json";
const PROFILE_PATH: &str = "/api/oauth/profile";

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn dir700(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

thread_local! {
    /// The fake's endpoints for this test's thread; `Ctx::identity` is a
    /// plain `fn`, so it finds them here.
    static EP: RefCell<Option<Endpoints>> = const { RefCell::new(None) };
}

fn via_fake(p: &Path) -> Result<Account, &'static str> {
    EP.with(|e| read_with(e.borrow().as_ref().expect("endpoints set"), p))
}

struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    store: ProfileStore,
    rs: RotateState,
    fake: FakeHttp,
    /// `~/.claude`'s login (`work`'s), and `spare`'s own.
    inodes: (u64, u64),
}

/// `work` is live with its login only in `~/.claude`; `spare` has its own.
/// Both tokens get identical usage reset times. `spare_uuid` is the
/// account the profile endpoint reports for `spare`'s token; `work`'s is
/// `u-work`.
fn fx(spare_uuid: &'static str) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (home, root) = (base.join("home"), base.join("root"));
    let store = ProfileStore::new(root.clone());
    store.add(&n("work")).unwrap();
    store.add(&n("spare")).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let acct = |u: &str| serde_json::json!({"oauthAccount": {"accountUuid": u}}).to_string();
    private(&root.join("work/.claude.json"), &acct("u-work"));
    private(&root.join("spare/.claude.json"), &acct(spare_uuid));
    let (tw, ts) = (fake_token(), fake_token());
    private(&root.join("spare").join(CREDS), &creds_doc(&ts, 3_600_000));
    dir700(&home);
    dir700(&home.join(".claude"));
    private(
        &home.join(".claude").join(CREDS),
        &creds_doc(&tw, 3_600_000),
    );
    private(&home.join(".claude.json"), &acct("u-work"));
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    rs.save_ledger(&RotateLedger {
        live: Some("work".into()),
        since: Some(now() - 7200),
        swapped_at: Some(now() - 7200),
        ..RotateLedger::default()
    })
    .unwrap();
    // 14:40Z and five days later, the same for both tokens.
    let (session, week) = (now() + 3600, now() + 5 * 86_400);
    let uuids = HashMap::from([(tw, "u-work"), (ts, spare_uuid)]);
    let fake = FakeHttp::start(move |seen, _| {
        let auth = seen.header("authorization").unwrap_or_default();
        let Some((_, uuid)) = uuids.iter().find(|(t, _)| auth == format!("Bearer {t}")) else {
            return Reply::json(401, &serde_json::json!({}));
        };
        match seen.path.as_str() {
            PROFILE_PATH => Reply::json(200, &profile_body(uuid)),
            _ => Reply::json(200, &usage_body(session, week)),
        }
    });
    EP.with(|e| *e.borrow_mut() = Some(endpoints(&fake)));
    let inodes = (
        ino_at(&home.join(".claude").join(CREDS)).unwrap(),
        ino_at(&root.join("spare").join(CREDS)).unwrap(),
    );
    Fx {
        _tmp: tmp,
        home,
        root,
        store,
        rs,
        fake,
        inodes,
    }
}

/// `work` exhausted; `spare` well below every threshold.
fn work_out_spare_free(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let reset = Some(now() + 3600);
    let win = |used| {
        Some(Win {
            used,
            resets_at: reset,
        })
    };
    let probed = |reading, windows| Probed {
        reading,
        windows,
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
        passive: false,
    };
    let work = probed(
        Reading::Exhausted {
            session: Some(100.0),
            week: Some(40.0),
        },
        Windows {
            session: win(100.0),
            week: win(40.0),
            no_session_limit: false,
        },
    );
    let spare = probed(
        Reading::Measured {
            session: 10.0,
            week: 10.0,
        },
        Windows::default(),
    );
    decide_measured(ctx, snap, work, vec![(n("spare"), spare)], now())
}

impl Fx {
    fn ctx(&self) -> Ctx {
        let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
        let mut ctx = Ctx::new(self.store.clone(), self.home.clone(), s);
        ctx.scan = |_| Liveness::Known(BTreeSet::new());
        ctx.measure = Some(work_out_spare_free);
        let mut l = self.rs.load_ledger().unwrap().unwrap();
        let identity = via_fake(&self.home.join(".claude")).unwrap();
        l.account = Some(identity.key().to_string());
        l.login_identity = Some(super::super::login_identity::Identity::new(&identity));
        self.rs.save_ledger(&l).unwrap();
        ctx.identity = via_fake;
        ctx
    }

    fn logged(&self) -> Vec<String> {
        fs::read_to_string(&self.rs.log)
            .unwrap_or_default()
            .lines()
            .map(|l| l.split_once(' ').unwrap().1.to_string())
            .collect()
    }

    /// The usage reset times the fake reports for the login in `dir`, by the
    /// real direct read: the signal identity used before this fix.
    fn resets(&self, dir: &Path) -> (Option<i64>, Option<i64>) {
        let cancel = AtomicBool::new(false);
        let req = Request {
            creds_dir: dir,
            journals: &[],
            allow_refresh: false,
            cancel: &cancel,
            timeout: Duration::from_secs(5),
            now_ms: chrono::Utc::now().timestamp_millis(),
        };
        let ep = EP.with(|e| e.borrow().clone().unwrap());
        let r = usage_direct::read(&ep, &req).unwrap().report;
        (
            r.session.and_then(|w| w.resets_at),
            r.week_all.and_then(|w| w.resets_at),
        )
    }
}

#[cfg(target_os = "linux")]
#[test]
fn equal_reset_times_on_two_accounts_no_longer_stop_the_swap() {
    let f = fx("u-spare");
    // The collision is real: both logins report the same reset times.
    assert_eq!(
        f.resets(&f.home.join(".claude")),
        f.resets(&f.root.join("spare"))
    );
    let mut c = f.ctx();
    for t in 0..2 {
        // Two cycles: the swap, then a cycle on the swapped-in login.
        let out = run_cycle(&mut c, Mode::Watch).unwrap();
        if t == 0 {
            assert_eq!(out, CycleOutcome::Swapped, "{:?}", f.logged());
        }
    }
    let log = f.logged();
    assert!(!log.iter().any(|l| l.starts_with("ALERT")), "{log:?}");
    assert!(
        !log.iter().any(|l| l.contains("same account as profile")),
        "{log:?}"
    );
    assert_eq!(
        f.rs.load_ledger().unwrap().unwrap().live.as_deref(),
        Some("spare")
    );
    assert_eq!(
        ino_at(&f.home.join(".claude").join(CREDS)),
        Some(f.inodes.1),
        "spare's login is now in ~/.claude"
    );
    assert_eq!(
        ino_at(&f.root.join("work").join(CREDS)),
        Some(f.inodes.0),
        "work's login is parked in its own folder"
    );
    assert!(
        f.fake.count("GET", PROFILE_PATH) >= 2,
        "identity asked the profile endpoint"
    );
}

/// The control: when the profile endpoint reports the same account for
/// both, the suspicion is real and the cycle stops before measuring.
#[cfg(target_os = "linux")]
#[test]
fn the_same_account_on_the_profile_endpoint_is_still_suspected() {
    let f = fx("u-work");
    let mut c = f.ctx();
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted,
        "{:?}",
        f.logged()
    );
    let log = f.logged();
    assert!(
        log.iter()
            .any(|l| l.contains("~/.claude holds the same account as profile spare")),
        "{log:?}"
    );
    assert_eq!(
        ino_at(&f.home.join(".claude").join(CREDS)),
        Some(f.inodes.0)
    );
    assert_eq!(ino_at(&f.root.join("spare").join(CREDS)), Some(f.inodes.1));
}
