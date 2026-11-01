//! `rotate::handfix`: recovery after a manual `/login` in `~/.claude`
//! replaced the live login (issue #1693). Every assertion about a login is by
//! inode; login fixtures are mode 0000 (0600 under root), so any attempt to
//! read one fails. No daemon, no `claude`, no sleeps; the time is fixed.
//!
//! The fixture is the 27 Oct case: the ledger names `ryan-ryansweet` (L),
//! which has no login; `~/.claude` holds the account of `ryan-mistt-org` (M);
//! `rssourcelabs` (C) is a logged-in profile with capacity.
//!
//! Contract:
//! - `handfix::Env<'a> { liveness: Liveness, auto_switch: Option<Ledger>
//!   (None: unreadable, which refuses a move), dev: &'a dyn Fn(&Path) ->
//!   Option<u64>, after_create: &'a dyn Fn() }`: the test seam.
//! - `handfix::step_with(&ProfileStore, &RotateState, home: &Path, &mut Sink,
//!   env_for: impl FnOnce() -> Env, t: i64) -> Result<Step, ProfileError>`;
//!   `env_for` is called only after a hand change, with `Step::{Clear,
//!   Recovered, Guided}` (`Debug`, `PartialEq`). `Clear`: the ledger's account
//!   is the one in `~/.claude` (or there is no login there); lost-login lines
//!   are still written. `Guided`: guidance written; the cycle ends `alerted`.
//! - `handfix::preview(&ProfileStore, &RotateState, home, env_for, t) ->
//!   Result<Vec<String>, ProfileError>`: the `--dry-run` lines, empty when
//!   there is no hand change; writes nothing.
//! - `control::status_alerts(&RotateLedger) -> Vec<String>`: the `alert:`
//!   and `warning:` lines of `rotate status`.

use super::backup::{BackupRefusal, fresh_backup, rejected_dir};
use super::control::status_alerts;
use super::decide::{Decision, Measured, Reading, decide};
use super::guide::{Kind, Paths, lines as guide_lines};
use super::handfix::{Env as Seam, Step, preview, step_with};
use super::identify::{CandidateInput, LiveId, Skip, candidates, live};
use super::ledger::{MAX_LOST_LOGIN, Phase, RotateJournal, RotateLedger, RotateState};
use super::log::{Sink, open_or_warn, sh_quote};
use super::observe::live_alert_for;
use super::settings::Settings;
use super::swap::{Source, swap};
use super::tests_swap::{ino_at, login, n, private};
use crate::commands::profile::auto_switch::euid;
use crate::commands::profile::auto_switch::kit::{Ledger, Liveness};
use crate::commands::profile::store::ProfileStore;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// 2026-10-27T03:17:05Z, the first hand-change ALERT in the #1693 log.
pub(super) const T: i64 = 1_793_071_025;
const STAMP: &str = "20261027T031705Z";
pub(super) const L: &str = "ryan-ryansweet";
pub(super) const M: &str = "ryan-mistt-org";
pub(super) const C: &str = "rssourcelabs";
pub(super) const CREDS: &str = ".credentials.json";

pub(super) fn acct(uuid: &str, email: &str) -> String {
    serde_json::json!({"oauthAccount": {"accountUuid": uuid, "emailAddress": email}}).to_string()
}

fn email_only(email: &str) -> String {
    serde_json::json!({"oauthAccount": {"emailAddress": email}}).to_string()
}

fn real_dev(p: &Path) -> Option<u64> {
    fs::symlink_metadata(p).ok().map(|m| m.dev())
}

fn noop() {}

pub(super) fn seam() -> Seam<'static> {
    Seam {
        liveness: Liveness::Known(BTreeSet::new()),
        auto_switch: Some(Ledger::default()),
        dev: &real_dev,
        after_create: &noop,
    }
}

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
    }
}

pub(super) struct Fx {
    _tmp: tempfile::TempDir,
    pub(super) base: PathBuf,
    pub(super) home: PathBuf,
    pub(super) root: PathBuf,
    pub(super) store: ProfileStore,
    pub(super) rs: RotateState,
    /// The login in `~/.claude` (the manual `/login`).
    pub(super) h: u64,
    /// M's own login, when it has one.
    pub(super) m: Option<u64>,
    /// C's login, when C exists.
    pub(super) c: Option<u64>,
}

fn dir700(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

/// The #1693 state. `m_own`: M has its own login (case B), else case A.
/// `capacity`: C exists with a login.
pub(super) fn fx(m_own: bool, capacity: bool) -> Fx {
    fx_at("home", m_own, capacity)
}

fn fx_at(home_rel: &str, m_own: bool, capacity: bool) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join(home_rel);
    let root = base.join("root");
    let store = ProfileStore::new(root.clone());
    store.add(&n(L)).unwrap();
    store.add(&n(M)).unwrap();
    if capacity {
        store.add(&n(C)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    private(
        &root.join(L).join(".claude.json"),
        &acct("u-rs", "rs@example.com"),
    );
    private(
        &root.join(M).join(".claude.json"),
        &acct("u-mistt", "mistt@example.com"),
    );
    let m = m_own.then(|| login(&root.join(M).join(CREDS)));
    let c = capacity.then(|| {
        private(
            &root.join(C).join(".claude.json"),
            &acct("u-src", "src@example.com"),
        );
        login(&root.join(C).join(CREDS))
    });
    dir700(&home);
    dir700(&home.join(".claude"));
    private(
        &home.join(".claude.json"),
        &acct("u-mistt", "mistt@example.com"),
    );
    let h = login(&home.join(".claude").join(CREDS));
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    rs.save_ledger(&RotateLedger {
        live: Some(L.into()),
        account: Some("u-rs".into()),
        since: Some(T - 2520),
        swapped_at: Some(T - 2520),
        previous: Some(C.into()),
        ..RotateLedger::default()
    })
    .unwrap();
    Fx {
        _tmp: tmp,
        base,
        home,
        root,
        store,
        rs,
        h,
        m,
        c,
    }
}

impl Fx {
    pub(super) fn step(&self, s: &Seam<'_>, t: i64) -> Step {
        let mut out = Sink::new(open_or_warn(&self.rs.log, &self.rs.log1), false, true);
        step_with(&self.store, &self.rs, &self.home, &mut out, || s.clone(), t).unwrap()
    }

    /// Every log line, without its time.
    pub(super) fn logged(&self) -> Vec<String> {
        fs::read_to_string(&self.rs.log)
            .unwrap_or_default()
            .lines()
            .map(|l| l.split_once(' ').unwrap().1.to_string())
            .collect()
    }

    pub(super) fn ledger(&self) -> RotateLedger {
        self.rs.load_ledger().unwrap().unwrap()
    }

    pub(super) fn status(&self) -> Vec<String> {
        status_alerts(&self.ledger())
    }

    pub(super) fn live(&self) -> PathBuf {
        self.home.join(".claude").join(CREDS)
    }

    pub(super) fn slot(&self, p: &str) -> PathBuf {
        self.root.join(p).join(CREDS)
    }

    pub(super) fn rejected(&self) -> PathBuf {
        self.root.join(".rejected-logins")
    }

    fn backup(&self, suffix: &str) -> PathBuf {
        self.rejected().join(format!("rotate-{M}-{STAMP}{suffix}"))
    }

    fn manual(&self) -> String {
        format!("{}/.rejected-logins/manual-{STAMP}", self.root.display())
    }

    /// Login files under `.rejected-logins`, by inode.
    pub(super) fn backed_up(&self) -> Vec<u64> {
        let Ok(rd) = fs::read_dir(self.rejected()) else {
            return Vec::new();
        };
        rd.filter_map(|e| ino_at(&e.unwrap().path().join(CREDS)))
            .collect()
    }

    /// Every login in the tree, by inode, sorted.
    fn all_logins(&self) -> Vec<u64> {
        let mut v: Vec<u64> = [self.live(), self.slot(L), self.slot(M), self.slot(C)]
            .iter()
            .filter_map(|p| ino_at(p))
            .chain(self.backed_up())
            .collect();
        v.sort();
        v
    }

    fn originals(&self) -> Vec<u64> {
        let mut v: Vec<u64> = [Some(self.h), self.m, self.c]
            .into_iter()
            .flatten()
            .collect();
        v.sort();
        v
    }

    /// Each original login exists exactly once, and no other login appeared.
    pub(super) fn assert_each_login_once(&self) {
        assert_eq!(
            self.all_logins(),
            self.originals(),
            "no login lost or copied"
        );
    }

    /// Nothing moved, nothing was backed up, and the ledger still names L.
    pub(super) fn assert_nothing_moved(&self) {
        assert_eq!(ino_at(&self.live()), Some(self.h));
        assert_eq!(ino_at(&self.slot(M)), self.m);
        assert_eq!(ino_at(&self.slot(C)), self.c);
        assert!(self.backed_up().is_empty(), "no login backed up");
        let l = self.ledger();
        assert_eq!(l.live.as_deref(), Some(L));
        assert_eq!(l.account.as_deref(), Some("u-rs"));
        self.assert_each_login_once();
    }

    fn recorded_m(&self) {
        let l = self.ledger();
        assert_eq!(l.live.as_deref(), Some(M));
        assert_eq!(l.account.as_deref(), Some("u-mistt"));
        assert_eq!(l.since, Some(T));
        assert_eq!(l.alert, None);
        assert_eq!(l.guidance, None);
        assert_eq!(
            live(&self.store, &self.rs, &self.home).unwrap().0,
            LiveId::Known(n(M))
        );
    }
}

fn alert(middle: &str) -> String {
    format!(
        "ALERT: the account in ~/.claude is not the one the ledger names (live={L}); {middle}; no swap"
    )
}

fn login_l() -> String {
    format!("fix 1: amplihack profile login {L}")
}

pub(super) fn lost_l() -> String {
    format!(
        "ALERT: profile {L} has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login {L}"
    )
}

pub(super) fn recorded_line() -> String {
    format!("recovered: ~/.claude holds {M}'s login (ledger named {L}); recorded {M} as live")
}

pub(super) fn moved_line(to: &Path) -> String {
    format!(
        "recovered: ~/.claude and {M} held the same account; kept ~/.claude, moved {M}'s copy to {}; recorded {M} as live",
        to.display()
    )
}

/// `rotate status` repeats the guidance as `alert:` lines.
fn as_status(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| format!("alert: {}", l.strip_prefix("ALERT: ").unwrap_or(l)))
        .collect()
}

fn count(lines: &[String], want: &str) -> usize {
    lines.iter().filter(|l| *l == want).count()
}

// --- Case B: the 27 Oct reproduction ----------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn reproduction_27_oct_moves_the_duplicate_aside_and_swaps_to_capacity() {
    let f = fx(true, true);
    assert_eq!(f.step(&seam(), T), Step::Recovered);

    // Kept ~/.claude; M's own copy renamed, not copied, into a fresh backup.
    let to = f.backup("").join(CREDS);
    assert_eq!(ino_at(&f.live()), Some(f.h));
    assert_eq!(ino_at(&f.slot(M)), None);
    assert_eq!(ino_at(&to), f.m);
    assert_eq!(to.parent().unwrap().parent().unwrap(), f.rejected());
    f.assert_each_login_once();
    f.recorded_m();
    assert_eq!(f.ledger().lost_login, vec![L.to_string()]);
    assert_eq!(f.logged(), vec![moved_line(&to), lost_l()]);
    assert_eq!(
        f.status(),
        vec![format!(
            "warning: {}",
            lost_l().strip_prefix("ALERT: ").unwrap()
        )]
    );

    // The same cycle carries on: L is skipped, C is a candidate, M is exhausted.
    let input = CandidateInput {
        live: Some(&n(M)),
        exclude: Vec::new(),
        liveness: Liveness::Known(BTreeSet::new()),
        auto_switch: Ledger::default(),
    };
    let (ok, skips) = candidates(&f.store, &input).unwrap();
    assert_eq!(ok, vec![n(C)]);
    assert!(skips.contains(&(n(L), Skip::NoLogin)));
    let target = Measured {
        name: n(C),
        reading: Reading::Measured {
            session: 12.0,
            week: 40.0,
        },
        windows: Default::default(),
        spend_limit: false,
        unknown: false,
    };
    let d = decide(&settings(), &Reading::Exhausted, &[target], None, T);
    assert!(
        matches!(&d, Decision::Swap { to, .. } if *to == n(C)),
        "{d:?}"
    );
    assert_eq!(
        swap(&f.store, &f.rs, &f.home, Source::Profile(n(M)), &n(C), T).unwrap(),
        n(M)
    );
    assert_eq!(ino_at(&f.live()), f.c);
    assert_eq!(
        ino_at(&f.slot(M)),
        Some(f.h),
        "the manual login went home to M"
    );
    assert_eq!(ino_at(&to), f.m, "the backup is never touched again");
    f.assert_each_login_once();
}

#[cfg(target_os = "linux")]
#[test]
fn case_b_without_capacity_recovers_and_then_alerts_no_capacity() {
    let f = fx(true, false);
    assert_eq!(f.step(&seam(), T), Step::Recovered);
    assert_eq!(ino_at(&f.backup("").join(CREDS)), f.m);
    f.assert_each_login_once();
    f.recorded_m();
    let input = CandidateInput {
        live: Some(&n(M)),
        exclude: Vec::new(),
        liveness: Liveness::Known(BTreeSet::new()),
        auto_switch: Ledger::default(),
    };
    assert!(candidates(&f.store, &input).unwrap().0.is_empty());
    assert_eq!(
        decide(&settings(), &Reading::Exhausted, &[], None, T),
        Decision::Alert
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_backup_directories_are_private_and_owned_by_the_user() {
    let f = fx(true, false);
    f.step(&seam(), T);
    for d in [f.rejected(), f.backup("")] {
        let m = fs::symlink_metadata(&d).unwrap();
        assert!(m.is_dir(), "{}", d.display());
        assert_eq!(m.uid(), euid());
        assert_eq!(m.mode() & 0o077, 0, "{}", d.display());
    }
    assert!(
        f.root.join(M).join(".claude.json").exists(),
        "M's .claude.json is not touched"
    );
}

// --- Case A: M has no login of its own ----------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn case_a_records_the_holder_and_moves_nothing() {
    let f = fx(false, false);
    assert_eq!(f.step(&seam(), T), Step::Recovered);
    assert_eq!(ino_at(&f.live()), Some(f.h));
    assert!(!f.rejected().exists(), "case A makes no backup");
    f.assert_each_login_once();
    f.recorded_m();
    assert_eq!(f.ledger().lost_login, vec![L.to_string()]);
    assert_eq!(f.logged(), vec![recorded_line(), lost_l()]);
    assert_eq!(
        decide(&settings(), &Reading::Exhausted, &[], None, T),
        Decision::Alert
    );
}

#[cfg(target_os = "linux")]
#[test]
fn case_a_with_capacity_swaps_in_the_same_cycle() {
    let f = fx(false, true);
    assert_eq!(f.step(&seam(), T), Step::Recovered);
    f.recorded_m();
    assert_eq!(
        swap(&f.store, &f.rs, &f.home, Source::Profile(n(M)), &n(C), T).unwrap(),
        n(M)
    );
    assert_eq!(ino_at(&f.live()), f.c);
    assert_eq!(ino_at(&f.slot(M)), Some(f.h));
    f.assert_each_login_once();
}

#[cfg(target_os = "linux")]
#[test]
fn a_lost_login_already_restored_gets_no_warning() {
    // D1: recovery does not wait for L to have no login.
    let f = fx(false, false);
    let l = login(&f.slot(L));
    assert_eq!(f.step(&seam(), T), Step::Recovered);
    assert_eq!(ino_at(&f.slot(L)), Some(l), "L's login is never touched");
    assert!(f.ledger().lost_login.is_empty());
    assert_eq!(f.logged(), vec![recorded_line()]);
    assert!(f.status().is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn a_stale_ledger_account_for_the_named_profile_is_recorded_without_a_lost_login() {
    // D2: the single match is L itself.
    let f = fx(false, false);
    private(
        &f.home.join(".claude.json"),
        &acct("u-rs", "rs@example.com"),
    );
    let mut l = f.ledger();
    l.account = Some("u-old".into());
    f.rs.save_ledger(&l).unwrap();
    assert_eq!(f.step(&seam(), T), Step::Recovered);
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(L));
    assert_eq!(l.account.as_deref(), Some("u-rs"));
    assert!(l.lost_login.is_empty());
    assert_eq!(
        f.logged(),
        vec![format!(
            "recovered: ~/.claude holds {L}'s login (ledger named {L}); recorded {L} as live"
        )]
    );
}

// --- Guidance: nothing can move ---------------------------------------------

fn guided(f: &Fx, s: &Seam<'_>, want: &[String]) {
    assert_eq!(f.step(s, T), Step::Guided);
    assert_eq!(f.logged(), want);
    assert_eq!(f.status(), as_status(want));
    assert_eq!(f.ledger().guidance.as_ref().map(Vec::len), Some(want.len()));
    assert!(
        f.ledger().lost_login.is_empty(),
        "nothing was recorded in L's place"
    );
    f.assert_nothing_moved();
}

#[cfg(target_os = "linux")]
#[test]
fn several_matches_name_them_all_sorted() {
    let f = fx(true, true);
    f.store.add(&n("mistt-spare")).unwrap();
    private(
        &f.root.join("mistt-spare/.claude.json"),
        &acct("u-mistt", "mistt@example.com"),
    );
    guided(
        &f,
        &seam(),
        &[
            alert(&format!(
                "it matches more than one profile (mistt-spare, {M})"
            )),
            login_l(),
            format!(
                "fix 2: keep this account in one of mistt-spare, {M}: log the others in to another account or remove them; the next cycle then recovers"
            ),
        ],
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_login_fix_is_left_out_when_the_named_profile_has_a_login() {
    let f = fx(false, false);
    login(&f.slot(L));
    f.store.add(&n("mistt-spare")).unwrap();
    private(
        &f.root.join("mistt-spare/.claude.json"),
        &acct("u-mistt", "mistt@example.com"),
    );
    assert_eq!(f.step(&seam(), T), Step::Guided);
    let lines = f.logged();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[1].starts_with("fix 1: keep this account in one of mistt-spare, "));
}

#[cfg(target_os = "linux")]
#[test]
fn no_matching_profile_gives_the_ledger_and_set_aside_commands() {
    let f = fx(false, true);
    private(
        &f.home.join(".claude.json"),
        &acct("u-nobody", "nobody@example.com"),
    );
    let ledger = f.rs.ledger.display().to_string();
    let dir = f.manual();
    guided(
        &f,
        &seam(),
        &[
            alert("no profile has that account"),
            login_l(),
            format!(
                "fix 2: rm {ledger} (the next swap keeps the login in ~/.claude as profile default-original)"
            ),
            format!(
                "fix 3: or, to set that login aside instead: close every claude session that uses ~/.claude, then run: mkdir -m 700 {dir} && mv -n {}/.claude/.credentials.json {dir}/ && rm {ledger}; then run claude with no CLAUDE_CONFIG_DIR and /login",
                f.home.display()
            ),
        ],
    );
    assert!(!f.rejected().exists(), "guidance creates nothing");
}

#[cfg(target_os = "linux")]
#[test]
fn an_unreadable_profile_makes_the_match_uncertain_even_with_one_match() {
    // D3: old-work could hold the same account.
    let f = fx(true, false);
    f.store.add(&n("old-work")).unwrap();
    login(&f.root.join("old-work").join(CREDS));
    guided(
        &f,
        &seam(),
        &[
            alert("the account of profile old-work cannot be read, so the match is uncertain"),
            login_l(),
            "fix 2: amplihack profile login old-work; the next cycle then recovers".into(),
        ],
    );
}

/// #1702: Claude rewrites `~/.claude.json` often, so one read that finds no
/// account is not a hand change. If the ledger is right, the login in
/// `~/.claude` is L's, and a `/login` there would leave L with no login
/// anywhere: the loss #1693 is about. So nothing recovers, nothing moves, no
/// guidance is written, and the live ALERT says the next cycle reads again.
/// The only ledger change is the time the unreadable spell started, which
/// later cycles use to escalate (tests_home_unreadable.rs).
#[cfg(target_os = "linux")]
#[test]
fn an_unreadable_home_account_is_unknown_not_a_hand_change() {
    let f = fx(true, false);
    private(&f.home.join(".claude.json"), r#"{"projects":{}}"#);
    let mut before = f.ledger();
    assert_eq!(
        live(&f.store, &f.rs, &f.home).unwrap().0,
        LiveId::HomeUnreadable(n(L))
    );
    assert_eq!(f.step(&seam(), T), Step::Clear);
    assert!(f.logged().is_empty(), "{:?}", f.logged());
    before.home_unreadable_since = Some(T);
    assert_eq!(
        f.ledger(),
        before,
        "only the start of the spell is recorded"
    );
    f.assert_nothing_moved();
    assert!(
        preview(&f.store, &f.rs, &f.home, seam, T)
            .unwrap()
            .is_empty(),
        "--dry-run previews no recovery either"
    );
    let (kind, line) = live_alert_for(&LiveId::HomeUnreadable(n(L)), false).unwrap();
    assert_eq!(kind, "home-unreadable");
    assert_eq!(
        line,
        format!(
            "ALERT: the account in ~/.claude.json cannot be read (live={L}); no swap this cycle, the next cycle reads it again; do not /login in ~/.claude, it would replace {L}'s login"
        )
    );
}

/// #1702: the account was read once and did not match, then could not be
/// read for the holder check (Claude rewrote it in between). The guidance
/// still never says `/login`: it says the next cycle reads the file again.
#[test]
fn guidance_for_an_unreadable_home_account_never_says_login() {
    let paths = Paths {
        home: PathBuf::from("/h"),
        root: PathBuf::from("/r"),
        ledger: PathBuf::from("/r/ledger.json"),
        manual: PathBuf::from("/r/.rejected-logins/manual-x"),
    };
    let got = guide_lines(&n(L), false, &Kind::HomeUnreadable, &paths);
    assert_eq!(
        got,
        vec![
            alert("the account in ~/.claude.json cannot be read"),
            "fix 1: none needed; the next cycle reads ~/.claude.json again".to_string(),
        ]
    );
    assert!(got.iter().all(|l| !l.contains("/login")), "{got:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn no_login_in_home_is_left_to_the_usual_no_login_alert() {
    let f = fx(true, false);
    fs::remove_file(f.live()).unwrap();
    assert_eq!(f.step(&seam(), T), Step::Clear);
    assert!(f.logged().is_empty(), "{:?}", f.logged());
    assert_eq!(f.ledger().guidance, None);
    assert_eq!(ino_at(&f.slot(M)), f.m);
}

#[cfg(target_os = "linux")]
#[test]
fn a_matching_ledger_is_clear_and_writes_nothing() {
    let f = fx(true, false);
    let mut l = f.ledger();
    l.account = Some("u-mistt".into());
    l.live = Some(M.into());
    f.rs.save_ledger(&l).unwrap();
    let before = fs::read(&f.rs.ledger).unwrap();
    assert_eq!(f.step(&seam(), T), Step::Clear);
    assert!(f.logged().is_empty());
    assert_eq!(fs::read(&f.rs.ledger).unwrap(), before);
}

// --- Refusals: one profile matches, a check fails ----------------------------

fn refused(f: &Fx, s: &Seam<'_>, reason: &str, fix: &str) {
    let own = if f.m.is_some() {
        " and has its own login"
    } else {
        ""
    };
    guided(
        f,
        s,
        &[
            alert(&format!(
                "profile {M} holds the same account{own}; recovery refused: {reason}"
            )),
            login_l(),
            format!("fix 2: {fix}"),
        ],
    );
}

fn by_hand(f: &Fx) -> String {
    let dir = f.manual();
    format!(
        "move {M}'s copy by hand: mkdir -m 700 {dir} && mv -n {}/{M}/.credentials.json {dir}/",
        f.root.display()
    )
}

#[cfg(target_os = "linux")]
#[test]
fn a_refresh_lock_in_home_or_in_the_profile_refuses() {
    for dir in ["home", "profile"] {
        let f = fx(true, false);
        let lock = match dir {
            "home" => f.home.join(".claude/.oauth_refresh.lock"),
            _ => f.root.join(M).join(".oauth_refresh.lock"),
        };
        private(&lock, "");
        refused(
            &f,
            &seam(),
            "Claude holds its refresh lock",
            "wait; the next cycle retries",
        );
        assert!(lock.exists(), "{dir}: the lock is not ours to remove");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_profile_on_another_device_refuses() {
    let f = fx(true, false);
    let mdir = f.root.join(M);
    let dev = move |p: &Path| {
        if p.starts_with(&mdir) {
            Some(u64::MAX)
        } else {
            real_dev(p)
        }
    };
    let s = Seam {
        dev: &dev,
        ..seam()
    };
    refused(
        &f,
        &s,
        "~/.claude and the profile root are on different devices",
        &by_hand(&f),
    );
}

#[cfg(target_os = "linux")]
#[test]
fn an_unknown_device_refuses() {
    let f = fx(true, false);
    let root = f.root.clone();
    let dev = move |p: &Path| if p == root { None } else { real_dev(p) };
    let s = Seam {
        dev: &dev,
        ..seam()
    };
    refused(
        &f,
        &s,
        "~/.claude and the profile root are on different devices",
        &by_hand(&f),
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_session_running_on_the_profile_refuses() {
    let f = fx(true, false);
    let s = Seam {
        liveness: Liveness::Known([M.to_string()].into()),
        ..seam()
    };
    refused(
        &f,
        &s,
        &format!("a claude session is running on {M}"),
        &format!("close the claude sessions that use {M}; the next cycle retries"),
    );
}

#[cfg(target_os = "linux")]
#[test]
fn sessions_that_cannot_be_seen_refuse() {
    for liveness in [Liveness::Everything, Liveness::Unsupported] {
        let f = fx(true, false);
        let s = Seam { liveness, ..seam() };
        let dir = f.manual();
        refused(
            &f,
            &s,
            "cannot see running sessions",
            &format!(
                "close the claude sessions that use {M}, then run: mkdir -m 700 {dir} && mv -n {}/{M}/.credentials.json {dir}/",
                f.root.display()
            ),
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_lent_or_hosting_profile_or_an_unreadable_auto_switch_ledger_refuses() {
    let lent = serde_json::json!({"version": 1, "hosts": {"other": {"live": M, "swapped_at": 1, "pre_swap_fingerprint": null}}});
    let hosts = serde_json::json!({"version": 1, "hosts": {M: {"live": "other", "swapped_at": 1, "pre_swap_fingerprint": null}}});
    for a in [
        Some(serde_json::from_value::<Ledger>(lent).unwrap()),
        Some(serde_json::from_value::<Ledger>(hosts).unwrap()),
        None,
    ] {
        let f = fx(true, false);
        let s = Seam {
            auto_switch: a,
            ..seam()
        };
        refused(
            &f,
            &s,
            &format!("{M}'s login is lent out or it hosts another login"),
            "wait for the auto-switch session to end; the next cycle retries",
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_hard_linked_profile_login_refuses() {
    let f = fx(true, false);
    fs::hard_link(f.slot(M), f.base.join("second-name")).unwrap();
    refused(
        &f,
        &seam(),
        "a login file is not a private regular file with one link",
        &by_hand(&f),
    );
}

fn pending_journal(f: &Fx) {
    f.rs.save_journal(&RotateJournal {
        version: 1,
        from: C.into(),
        to: M.into(),
        adopt: false,
        phase: Phase::S1,
        home: f.home.to_str().unwrap().into(),
        live_ino: 1,
        target_ino: 2,
        home_lock_ino: None,
        to_lock_ino: None,
        started_at: T - 60,
        pid: 1,
    })
    .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_pending_journal_refuses_in_both_cases() {
    for own in [true, false] {
        let f = fx(own, false);
        pending_journal(&f);
        refused(
            &f,
            &seam(),
            "an earlier swap stopped half way",
            "see the Recovery section of docs/howto/claude-profile-rotation.md",
        );
        assert!(
            f.rs.journal.exists(),
            "the journal is left for swap recovery"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_shared_dot_claude_refuses_in_both_cases() {
    for own in [true, false] {
        let f = fx(own, false);
        let dot = f.home.join(".claude");
        fs::set_permissions(&dot, fs::Permissions::from_mode(0o777)).unwrap();
        refused(
            &f,
            &seam(),
            "~/.claude is not a private directory owned by you",
            &format!(
                "make {0}/.claude owned by you and run: chmod 700 {0}/.claude",
                f.home.display()
            ),
        );
        fs::set_permissions(&dot, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn an_account_change_after_the_backup_is_created_moves_nothing() {
    // S2: someone ran /login again between the checks and the move.
    let f = fx(true, false);
    let hj = f.home.join(".claude.json");
    let again = move || private(&hj, &acct("u-other", "other@example.com"));
    let s = Seam {
        after_create: &again,
        ..seam()
    };
    refused(
        &f,
        &s,
        "~/.claude changed during recovery",
        "none needed; the next cycle retries",
    );
    let left: Vec<_> = fs::read_dir(f.backup("")).unwrap().collect();
    assert!(left.is_empty(), "an empty backup directory is left");
}

// --- Interrupted recoveries (D8) -----------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn an_empty_backup_left_by_an_abort_makes_the_next_cycle_use_the_next_name() {
    let f = fx(true, false);
    // As an aborted cycle leaves them: both created 0700.
    dir700(&f.rejected());
    dir700(&f.backup(""));
    assert_eq!(f.step(&seam(), T), Step::Recovered);
    let to = f.backup("-2").join(CREDS);
    assert_eq!(ino_at(&to), f.m);
    assert_eq!(fs::read_dir(f.backup("")).unwrap().count(), 0);
    assert_eq!(f.logged()[0], moved_line(&to));
    f.assert_each_login_once();
    f.recorded_m();
}

#[cfg(target_os = "linux")]
#[test]
fn a_symlink_at_the_backup_name_is_never_followed() {
    let f = fx(true, false);
    let elsewhere = f.base.join("elsewhere");
    dir700(&elsewhere);
    dir700(&f.rejected());
    std::os::unix::fs::symlink(&elsewhere, f.backup("")).unwrap();
    assert_eq!(f.step(&seam(), T), Step::Recovered);
    assert_eq!(ino_at(&f.backup("-2").join(CREDS)), f.m);
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    assert!(
        fs::symlink_metadata(f.backup(""))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_login_moved_but_not_recorded_is_recorded_by_the_next_cycle() {
    let f = fx(true, false);
    dir700(&f.backup(""));
    fs::rename(f.slot(M), f.backup("").join(CREDS)).unwrap();
    assert_eq!(f.step(&seam(), T), Step::Recovered);
    assert_eq!(f.logged()[0], recorded_line(), "handled as case A");
    assert_eq!(
        fs::read_dir(f.rejected()).unwrap().count(),
        1,
        "no second backup"
    );
    f.assert_each_login_once();
    f.recorded_m();
}

// --- Repetition and status (D5, D6) --------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn guidance_repeats_every_cycle_until_resolved_then_the_lost_login_repeats() {
    let f = fx(false, false);
    f.store.add(&n("mistt-spare")).unwrap();
    private(
        &f.root.join("mistt-spare/.claude.json"),
        &acct("u-mistt", "mistt@example.com"),
    );
    let head = alert(&format!(
        "it matches more than one profile (mistt-spare, {M})"
    ));
    let mut status = None;
    for k in 0..3 {
        assert_eq!(f.step(&seam(), T + 600 * k), Step::Guided);
        let lines = f.logged();
        assert_eq!(count(&lines, &head), k as usize + 1, "cycle {k}: {lines:?}");
        assert_eq!(count(&lines, &login_l()), k as usize + 1);
        let now = f.status();
        assert_eq!(now.len(), 3);
        assert_eq!(now[0], as_status(std::slice::from_ref(&head))[0]);
        if let Some(prev) = &status {
            assert_eq!(&now, prev, "status shows the same guidance each cycle");
        }
        status = Some(now);
    }

    // The extra profile drops its account: exactly one match, case A.
    fs::remove_file(f.root.join("mistt-spare/.claude.json")).unwrap();
    assert_eq!(f.step(&seam(), T + 1800), Step::Recovered);
    assert_eq!(f.ledger().guidance, None);
    let warning = format!("warning: {}", lost_l().strip_prefix("ALERT: ").unwrap());
    assert_eq!(f.status(), vec![warning.clone()]);

    for k in 1..=2 {
        assert_eq!(f.step(&seam(), T + 1800 + 600 * k), Step::Clear);
        assert_eq!(count(&f.logged(), &lost_l()), k as usize + 1);
        assert_eq!(f.status(), vec![warning.clone()]);
    }
    assert_eq!(count(&f.logged(), &head), 3, "no guidance after recovery");

    login(&f.slot(L));
    assert_eq!(f.step(&seam(), T + 3600), Step::Clear);
    assert_eq!(
        count(&f.logged(), &lost_l()),
        3,
        "no warning once L has a login"
    );
    assert!(f.ledger().lost_login.is_empty());
    assert!(f.status().is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn a_removed_profile_leaves_lost_login() {
    let f = fx(false, false);
    f.step(&seam(), T);
    assert_eq!(f.ledger().lost_login, vec![L.to_string()]);
    fs::remove_dir_all(f.root.join(L)).unwrap();
    assert_eq!(f.step(&seam(), T + 600), Step::Clear);
    assert!(f.ledger().lost_login.is_empty());
    assert_eq!(count(&f.logged(), &lost_l()), 1);
}

#[test]
fn status_cleans_control_characters_again() {
    let l = RotateLedger {
        guidance: Some(vec!["ALERT: x\u{1b}[31m red\u{7}".into()]),
        ..RotateLedger::default()
    };
    let out = status_alerts(&l);
    assert_eq!(out.len(), 1);
    assert!(out[0].starts_with("alert: "), "{out:?}");
    assert!(!out[0].chars().any(char::is_control), "{out:?}");
    assert!(status_alerts(&RotateLedger::default()).is_empty());
}

// --- Dry run -----------------------------------------------------------------

/// Every path under `dir` with its inode, size, mode and mtime.
fn tree(dir: &Path, out: &mut Vec<(PathBuf, u64, u64, u32, i64)>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let m = fs::symlink_metadata(&p).unwrap();
        out.push((
            p.clone(),
            m.ino(),
            m.len(),
            m.mode(),
            m.mtime_nsec() + m.mtime() * 1_000_000_000,
        ));
        if m.is_dir() && !m.file_type().is_symlink() {
            tree(&p, out);
        }
    }
}

fn snapshot(f: &Fx) -> Vec<(PathBuf, u64, u64, u32, i64)> {
    let mut v = Vec::new();
    tree(&f.base, &mut v);
    v.sort();
    v
}

#[cfg(target_os = "linux")]
#[test]
fn preview_of_a_recovery_writes_nothing() {
    let f = fx(true, true);
    let before = snapshot(&f);
    let lines = preview(&f.store, &f.rs, &f.home, seam, T).unwrap();
    assert_eq!(snapshot(&f), before);
    assert!(!lines.is_empty());
    assert!(
        lines[0].starts_with(&format!(
            "would recover: ~/.claude and {M} held the same account; kept ~/.claude, moved {M}'s copy to "
        )),
        "{lines:?}"
    );
    assert!(lines[0].ends_with(&format!("; recorded {M} as live")));
    assert!(
        lines.iter().all(|l| !l.starts_with("recovered: ")),
        "{lines:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn preview_of_guidance_prints_the_alert_and_fix_lines() {
    let f = fx(false, false);
    private(
        &f.home.join(".claude.json"),
        &acct("u-nobody", "nobody@example.com"),
    );
    let before = snapshot(&f);
    let lines = preview(&f.store, &f.rs, &f.home, seam, T).unwrap();
    assert_eq!(snapshot(&f), before);
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(
        lines[0],
        format!(
            "would alert: {}",
            alert("no profile has that account")
                .strip_prefix("ALERT: ")
                .unwrap()
        )
    );
    assert_eq!(lines[1], login_l());
    assert!(lines[2].starts_with("fix 2: rm "));
    assert!(lines[3].starts_with("fix 3: or, to set that login aside instead: "));
}

#[cfg(target_os = "linux")]
#[test]
fn preview_without_a_hand_change_prints_nothing() {
    let f = fx(true, false);
    let mut l = f.ledger();
    l.live = Some(M.into());
    l.account = Some("u-mistt".into());
    f.rs.save_ledger(&l).unwrap();
    assert!(
        preview(&f.store, &f.rs, &f.home, seam, T)
            .unwrap()
            .is_empty()
    );
}

// --- No identity leaks, hostile paths, limits -----------------------------------

#[cfg(target_os = "linux")]
#[test]
fn accounts_never_appear_in_the_log_status_or_preview() {
    for (m_matches, own) in [(true, true), (false, false)] {
        let f = fx(own, true);
        for (p, e) in [
            (L, "rs@example.com"),
            (M, "mistt@example.com"),
            (C, "src@example.com"),
        ] {
            private(&f.root.join(p).join(".claude.json"), &email_only(e));
        }
        let home = if m_matches {
            "mistt@example.com"
        } else {
            "nobody@example.com"
        };
        private(&f.home.join(".claude.json"), &email_only(home));
        let mut l = f.ledger();
        l.account = Some("rs@example.com".into());
        f.rs.save_ledger(&l).unwrap();
        let shown = preview(&f.store, &f.rs, &f.home, seam, T).unwrap();
        f.step(&seam(), T);
        let mut text = f.logged();
        text.extend(shown);
        text.extend(f.status());
        assert!(text.len() >= 2, "{text:?}");
        for line in &text {
            assert!(!line.contains("@example.com"), "{line}");
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn paths_in_fix_lines_are_quoted_and_unsafe_ones_are_not_printed() {
    let f = fx_at("ho me", false, false);
    private(
        &f.home.join(".claude.json"),
        &acct("u-nobody", "nobody@example.com"),
    );
    assert_eq!(f.step(&seam(), T), Step::Guided);
    let fix3 = f.logged().pop().unwrap();
    let want = format!("mv -n '{}/.claude/.credentials.json' ", f.home.display());
    assert!(fix3.contains(&want), "{fix3}");

    let f = fx_at("it's", false, false);
    private(
        &f.home.join(".claude.json"),
        &acct("u-nobody", "nobody@example.com"),
    );
    f.step(&seam(), T);
    let fix3 = f.logged().pop().unwrap();
    assert!(
        fix3.contains(r"it'\''s/.claude/.credentials.json'"),
        "{fix3}"
    );

    for bad in ["new\nline", "bidi\u{202e}x"] {
        let f = fx_at(bad, false, false);
        private(
            &f.home.join(".claude.json"),
            &acct("u-nobody", "nobody@example.com"),
        );
        assert_eq!(f.step(&seam(), T), Step::Guided);
        assert_eq!(
            f.logged().pop().unwrap(),
            "fix 3: move the login in ~/.claude by hand; its path cannot be printed safely"
        );
        assert!(f.rs.load_ledger().is_ok(), "the ledger stays loadable");
    }
}

/// A home path so long that the set-aside command does not fit in one
/// line: the command is replaced, never cut (a cut path is another path).
/// `parts` of 240 bytes: 4 keeps the home under 1024 bytes, 5 goes over.
#[cfg(target_os = "linux")]
#[test]
fn guidance_for_a_very_long_home_stays_within_the_ledger_limits() {
    for parts in [4, 5] {
        let deep = vec!["d".repeat(240); parts].join("/");
        let f = fx_at(&deep, false, false);
        private(
            &f.home.join(".claude.json"),
            &acct("u-nobody", "nobody@example.com"),
        );
        assert_eq!(f.step(&seam(), T), Step::Guided);
        let l =
            f.rs.load_ledger()
                .expect("the ledger it wrote loads")
                .unwrap();
        let g = l.guidance.unwrap();
        assert!(!g.is_empty() && g.len() <= 16);
        assert!(g.iter().all(|line| line.len() <= 1024), "{g:?}");
        assert!(f.logged().iter().all(|line| line.len() <= 1024));
        let ledger = sh_quote(&f.rs.ledger).unwrap();
        assert_eq!(
            g[2],
            format!(
                "fix 2: rm {ledger} (the next swap keeps the login in ~/.claude as profile default-original)"
            ),
            "a line that fits is printed whole"
        );
        assert_eq!(
            g[3], "fix 3: move the login in ~/.claude by hand; its path cannot be printed safely",
            "{parts} parts"
        );
        for line in g.iter().chain(&f.logged()) {
            assert!(!line.contains("mv -n"), "no cut command: {line}");
        }
    }
}

// --- Review follow-ups ---------------------------------------------------------

/// The `/proc` scan and the auto-switch ledger (the `Env`) are read only
/// after a hand change, in the cycle and in `--dry-run`.
#[cfg(target_os = "linux")]
#[test]
fn the_env_is_built_only_after_a_hand_change() {
    let f = fx(true, true);
    let built = std::cell::Cell::new(0);
    let counted = || {
        built.set(built.get() + 1);
        seam()
    };
    assert!(
        !preview(&f.store, &f.rs, &f.home, counted, T)
            .unwrap()
            .is_empty()
    );
    assert_eq!(built.get(), 1);
    let mut out = Sink::new(open_or_warn(&f.rs.log, &f.rs.log1), false, true);
    let step = step_with(&f.store, &f.rs, &f.home, &mut out, counted, T).unwrap();
    assert_eq!((step, built.get()), (Step::Recovered, 2));

    let never = || -> Seam<'static> { panic!("the Env was built without a hand change") };
    assert!(
        preview(&f.store, &f.rs, &f.home, never, T)
            .unwrap()
            .is_empty()
    );
    let step = step_with(&f.store, &f.rs, &f.home, &mut out, never, T + 600).unwrap();
    assert_eq!(step, Step::Clear);
}

/// L already in `lost_login` and guided again with its own `fix 1`: one
/// "log in" line for L in that cycle, not two.
#[cfg(target_os = "linux")]
#[test]
fn a_lost_profile_guided_again_gets_one_login_line() {
    let f = fx(false, false);
    assert_eq!(f.step(&seam(), T), Step::Recovered);
    assert_eq!(f.ledger().lost_login, vec![L.to_string()]);
    let mut l = f.ledger();
    l.live = Some(L.into());
    l.account = Some("u-rs".into());
    f.rs.save_ledger(&l).unwrap();
    f.store.add(&n("mistt-spare")).unwrap();
    private(
        &f.root.join("mistt-spare/.claude.json"),
        &acct("u-mistt", "mistt@example.com"),
    );
    let before = f.logged().len();
    assert_eq!(f.step(&seam(), T + 600), Step::Guided);
    let cycle = f.logged().split_off(before);
    let login = format!("amplihack profile login {L}");
    assert_eq!(
        cycle.iter().filter(|l| l.contains(&login)).count(),
        1,
        "{cycle:?}"
    );
    assert_eq!(cycle[1], login_l());
    assert_eq!(f.ledger().lost_login, vec![L.to_string()], "L stays listed");

    // Without guidance for L, its lost-login line is back.
    let mut l = f.ledger();
    l.live = Some(M.into());
    l.account = Some("u-mistt".into());
    f.rs.save_ledger(&l).unwrap();
    let before = f.logged().len();
    assert_eq!(f.step(&seam(), T + 1200), Step::Clear);
    assert_eq!(f.logged().split_off(before), vec![lost_l()]);
}

/// A full `lost_login` list: L is not added, and one line says so, once.
#[cfg(target_os = "linux")]
#[test]
fn a_full_lost_login_list_says_so_once() {
    let f = fx(false, false);
    let others: Vec<String> = (0..MAX_LOST_LOGIN)
        .map(|i| format!("gone-{i:02}"))
        .collect();
    for o in &others {
        f.store.add(&n(o)).unwrap();
    }
    let mut l = f.ledger();
    l.lost_login = others.clone();
    f.rs.save_ledger(&l).unwrap();
    assert_eq!(f.step(&seam(), T), Step::Recovered);
    let cap = format!(
        "ALERT: profile {L} has no login anywhere, and the list of such profiles is full, so this is not repeated. Run: amplihack profile login {L}"
    );
    let lines = f.logged();
    assert_eq!(lines[..2], [recorded_line(), cap.clone()]);
    assert_eq!(f.ledger().lost_login, others, "L is not added");
    assert_eq!(f.step(&seam(), T + 600), Step::Clear);
    assert_eq!(count(&f.logged(), &cap), 1, "not repeated");
}

/// A profile directory replaced by a symlink leaves `lost_login`: nothing
/// is looked up through the link.
#[cfg(target_os = "linux")]
#[test]
fn a_symlinked_profile_dir_leaves_lost_login() {
    let f = fx(false, false);
    f.step(&seam(), T);
    assert_eq!(f.ledger().lost_login, vec![L.to_string()]);
    let elsewhere = f.base.join("elsewhere");
    dir700(&elsewhere);
    fs::remove_dir_all(f.root.join(L)).unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.root.join(L)).unwrap();
    assert_eq!(f.step(&seam(), T + 600), Step::Clear);
    assert!(f.ledger().lost_login.is_empty());
}

/// An existing `.rejected-logins` others can read refuses the move with
/// its own reason, as `profile login` requires `0700`.
#[cfg(target_os = "linux")]
#[test]
fn a_rejected_logins_dir_others_can_read_refuses_with_its_own_reason() {
    let f = fx(true, false);
    fs::create_dir(f.rejected()).unwrap();
    fs::set_permissions(f.rejected(), fs::Permissions::from_mode(0o750)).unwrap();
    assert_eq!(f.step(&seam(), T), Step::Guided);
    let head = &f.logged()[0];
    let want = format!(
        "recovery refused: backup directory is not private: {}; no swap",
        f.rejected().display()
    );
    assert!(head.ends_with(&want), "{head}");
    assert_eq!(ino_at(&f.slot(M)), f.m, "nothing moved");
    assert_eq!(fs::read_dir(f.rejected()).unwrap().count(), 0);
    assert_eq!(f.ledger().live.as_deref(), Some(L));
}

#[test]
fn backup_refusals_each_have_their_own_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();

    // Io: the parent is missing.
    let missing = base.join("no-such-root").join(".rejected-logins");
    let r = rejected_dir(&missing).unwrap_err();
    assert!(matches!(r, BackupRefusal::Io(_)), "{r:?}");
    assert!(
        r.reason()
            .starts_with("cannot create the backup directory: ")
    );
    let r = fresh_backup(&missing, &n(M), T).unwrap_err();
    assert!(matches!(r, BackupRefusal::Io(_)), "{r:?}");

    // NotPrivate: an existing directory others can read, or a symlink.
    let open = base.join("open");
    fs::create_dir(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
    let r = rejected_dir(&open).unwrap_err();
    assert_eq!(
        r.reason(),
        format!("backup directory is not private: {}", open.display())
    );
    let link = base.join("link");
    std::os::unix::fs::symlink(base.join("private"), &link).unwrap();
    dir700(&base.join("private"));
    assert!(matches!(
        rejected_dir(&link).unwrap_err(),
        BackupRefusal::NotPrivate(_)
    ));

    // A fresh one is created 0700, and a private existing one is accepted.
    let rejected = base.join(".rejected-logins");
    assert_eq!(rejected_dir(&rejected).unwrap(), rejected);
    assert_eq!(rejected_dir(&rejected).unwrap(), rejected);

    // NoFreeName: all 99 names for this second are taken.
    for k in 1..=99 {
        let suffix = if k == 1 {
            String::new()
        } else {
            format!("-{k}")
        };
        dir700(&rejected.join(format!("rotate-{M}-{STAMP}{suffix}")));
    }
    let r = fresh_backup(&rejected, &n(M), T).unwrap_err();
    assert!(matches!(r, BackupRefusal::NoFreeName), "{r:?}");
    assert_eq!(r.reason(), "no free backup name");
}
