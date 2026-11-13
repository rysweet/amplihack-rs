//! `rotate::handfix`: recovery after a manual `/login` in `~/.claude`
//! replaced the live login (issues #1693 and #1886). Every assertion about a
//! login is by inode; login fixtures hold an empty `claudeAiOauth` block and
//! no token. Whose login is where comes from a fake credential reader keyed
//! by directory, never from `.claude.json`. No daemon, no `claude`, no
//! sleeps; the time is fixed.
//!
//! The fixture is the 27 Oct case, made recoverable: the ledger names
//! `ryan-ryansweet` (L), which keeps a login of its own; `~/.claude` answers
//! as `ryan-mistt-org` (M), which has its own copy (`m_own`);
//! `rssourcelabs` (C) is a logged-in profile with capacity. `suspected()`
//! records the first reading of M one interval before `T`, so the step at
//! `T` is the confirming second reading.
//!
//! Contract (see also `tests_issue_1886*.rs`):
//! - `handfix::Env<'a> { liveness, auto_switch, dev, after_create }`: the
//!   test seam; `env_for` is called only for a confirmed suspicion.
//! - `handfix::step_with(store, rs, home, &mut Sink, env_for, read, min_gap,
//!   t) -> Result<Step, ProfileError>`, `Step::{Clear, Noted, Recovered,
//!   Guided}`.
//! - `handfix::preview(store, rs, home, env_for, read, min_gap, t)`: the
//!   `--dry-run` lines; writes nothing.
//! - `control::status_alerts(&RotateLedger) -> Vec<String>`: the `alert:`,
//!   `note:` and `warning:` lines of `rotate status`.

use super::backup::{BackupRefusal, fresh_backup, rejected_dir};
use super::control::status_alerts;
use super::decide::{Decision, Measured, Reading, decide};
use super::hand_fields::HandSuspect;
use super::handfix::{Env as Seam, Step, preview, step_with};
use super::handfix_msg as msg;
use super::identify::Account;
use super::identify::{CandidateInput, LiveId, candidates, live};
use super::ledger::{Phase, RotateJournal, RotateLedger, RotateState};
use super::log::{Sink, open_or_warn};
use super::settings::Settings;
use super::swap::{Source, swap};
use super::tests_issue_1886::server;
use super::tests_swap::{ino_at, login, n, private};
use crate::commands::profile::auto_switch::euid;
use crate::commands::profile::auto_switch::kit::{Ledger, Liveness};
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_direct::reason;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
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
/// The rotation interval the step is given (the default `--interval`).
pub(super) const GAP: i64 = 600;

type Answer = Result<Account, &'static str>;

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

// --- Fake credential readings -------------------------------------------------

/// The account the server reports for each login: one per account.
pub(super) fn a_l() -> Account {
    server("u-lima")
}
pub(super) fn a_m() -> Account {
    server("u-mike")
}
pub(super) fn a_c() -> Account {
    server("u-charlie")
}

thread_local! {
    /// This test's readings, by canonical directory. Each test runs on its
    /// own thread, so tests never see each other's.
    static READINGS: RefCell<HashMap<PathBuf, Answer>> = RefCell::default();
}

/// The fake identity reader (`Ctx::identity` in cycle tests): what the
/// fixture says the login in `p` answers; no file, otherwise.
pub(super) fn by_path(p: &Path) -> Answer {
    let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    READINGS.with(|r| r.borrow().get(&p).cloned().unwrap_or(Err(reason::NO_FILE)))
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
    /// L's own login, when it has one.
    pub(super) l: Option<u64>,
    /// M's own login, when it has one.
    pub(super) m: Option<u64>,
    /// C's login, when C exists.
    pub(super) c: Option<u64>,
}

fn dir700(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

/// The #1693 state with L's own login kept. `m_own`: M has its own copy;
/// `capacity`: C exists with a login.
pub(super) fn fx(m_own: bool, capacity: bool) -> Fx {
    fx_at("home", true, m_own, capacity)
}

/// As [`fx`], but L has no login of its own: its only login is the one the
/// `/login` replaced (the only-login rule refuses a move).
pub(super) fn fx_no_l(m_own: bool, capacity: bool) -> Fx {
    fx_at("home", false, m_own, capacity)
}

fn fx_at(home_rel: &str, l_own: bool, m_own: bool, capacity: bool) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    // #1946: `Fx::answer` writes the synthetic identity itself, not through
    // `body()`, so the fixture registers its root with the test reader here.
    super::tests::tests_fixture_identity::own_root(&base);
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
    let l = l_own.then(|| oauth_login(&root.join(L).join(CREDS)));
    let m = m_own.then(|| oauth_login(&root.join(M).join(CREDS)));
    let c = capacity.then(|| {
        private(
            &root.join(C).join(".claude.json"),
            &acct("u-src", "src@example.com"),
        );
        oauth_login(&root.join(C).join(CREDS))
    });
    dir700(&home);
    dir700(&home.join(".claude"));
    private(
        &home.join(".claude.json"),
        &acct("u-mistt", "mistt@example.com"),
    );
    let h = oauth_login(&home.join(".claude").join(CREDS));
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
    let f = Fx {
        _tmp: tmp,
        base,
        home,
        root,
        store,
        rs,
        h,
        l,
        m,
        c,
    };
    READINGS.with(|r| r.borrow_mut().clear());
    f.answer("~/.claude", Ok(a_m()));
    f.answer(L, Ok(a_l()));
    f.answer(M, Ok(a_m()));
    f.answer(C, Ok(a_c()));
    f
}

/// A private OAuth login file with an empty `claudeAiOauth` block: it
/// holds no token, and `login_kind` (which recovery uses to choose whom to
/// read) must be able to read its keys. The fake reader decides what it
/// answers.
fn oauth_login(p: &Path) -> u64 {
    private(p, r#"{"claudeAiOauth":{}}"#);
    ino_at(p).unwrap()
}

impl Fx {
    /// Sets what the login at `place` (`~/.claude` or a profile) answers.
    pub(super) fn answer(&self, place: &str, a: Answer) {
        let p = if place == "~/.claude" {
            self.home.join(".claude")
        } else {
            self.root.join(place)
        };
        let path = p.join(CREDS);
        if path.exists() {
            let mut v: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            v["task1753FakeIdentity"] = a
                .as_ref()
                .map(|a| a.block())
                .unwrap_or(serde_json::Value::Null);
            private(&path, &v.to_string());
        }
        READINGS.with(|r| r.borrow_mut().insert(p, a));
    }

    /// The first reading of M, one interval before `T`.
    pub(super) fn suspected(&self) {
        let mut l = self.ledger();
        l.hand_suspect = Some(HandSuspect {
            profile: M.into(),
            first_seen: T - GAP,
        });
        self.rs.save_ledger(&l).unwrap();
    }

    pub(super) fn step(&self, s: &Seam<'_>, t: i64) -> Step {
        let mut out = Sink::new(open_or_warn(&self.rs.log, &self.rs.log1), false, true);
        let read = |p: &Path| by_path(p);
        let (store, rs, home) = (&self.store, &self.rs, &self.home);
        step_with(store, rs, home, &mut out, || s.clone(), &read, GAP, t).unwrap()
    }

    pub(super) fn preview_with<'a>(
        &self,
        env_for: impl FnOnce() -> Seam<'a>,
        t: i64,
    ) -> Vec<String> {
        let read = |p: &Path| by_path(p);
        preview(&self.store, &self.rs, &self.home, env_for, &read, GAP, t).unwrap()
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
        let mut v: Vec<u64> = [Some(self.h), self.l, self.m, self.c]
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
        assert_eq!(ino_at(&self.slot(L)), self.l);
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
        assert_eq!(
            l.account.as_deref(),
            Some("u-mike"),
            "from trusted credential evidence"
        );
        assert_eq!(l.since, Some(T));
        assert_eq!(l.alert, None);
        assert_eq!(l.guidance, None);
        assert_eq!(l.hand_suspect, None);
        assert_eq!(
            live(&self.store, &self.rs, &self.home).unwrap().0,
            LiveId::Known(n(M))
        );
    }
}

fn refused_head(reason: &str) -> String {
    format!(
        "ALERT: the account in ~/.claude is not the one the ledger names (live={L}); profile {M} holds the same account and has its own login; recovery refused: {reason}; no swap"
    )
}

pub(super) fn lost_l() -> String {
    format!(
        "ALERT: profile {L} has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login {L}"
    )
}

pub(super) fn moved_line(to: &Path) -> String {
    format!(
        "recovered: ~/.claude and {M} held the same account; kept ~/.claude, moved {M}'s copy to {}; recorded {M} as live",
        to.display()
    )
}

fn count(lines: &[String], want: &str) -> usize {
    lines.iter().filter(|l| *l == want).count()
}

// --- The 27 Oct reproduction, with L's own login kept ---------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_confirmed_suspicion_moves_the_duplicate_aside_and_swaps_to_capacity() {
    let f = fx(true, true);
    f.suspected();
    assert_eq!(f.step(&seam(), T), Step::Recovered);

    // Kept ~/.claude; M's own copy renamed, not copied, into a fresh backup.
    let to = f.backup("").join(CREDS);
    assert_eq!(ino_at(&f.live()), Some(f.h));
    assert_eq!(ino_at(&f.slot(M)), None);
    assert_eq!(ino_at(&f.slot(L)), f.l, "L keeps its own login");
    assert_eq!(ino_at(&to), f.m);
    assert_eq!(to.parent().unwrap().parent().unwrap(), f.rejected());
    f.assert_each_login_once();
    f.recorded_m();
    assert!(f.ledger().lost_login.is_empty());
    assert_eq!(f.logged(), vec![moved_line(&to)]);
    assert!(f.status().is_empty());

    // The same cycle carries on: L and C are candidates, M is exhausted.
    let input = CandidateInput {
        live: Some(&n(M)),
        exclude: Vec::new(),
        liveness: Liveness::Known(BTreeSet::new()),
        auto_switch: Ledger::default(),
    };
    let (ok, skips) = candidates(&f.store, &input).unwrap();
    assert_eq!(ok, vec![n(C), n(L)]);
    assert!(!skips.iter().any(|(s, _)| *s == n(L) || *s == n(C)));
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
    let d = decide(&settings(), &Reading::EXHAUSTED, &[target], None, T);
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
fn the_backup_directories_are_private_and_owned_by_the_user() {
    let f = fx(true, false);
    f.suspected();
    assert_eq!(f.step(&seam(), T), Step::Recovered);
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

/// Without a suspicion on record the first reading only notes it.
#[cfg(target_os = "linux")]
#[test]
fn a_first_reading_notes_and_moves_nothing() {
    let f = fx(true, true);
    assert_eq!(f.step(&seam(), T), Step::Noted);
    f.assert_nothing_moved();
    assert_eq!(f.logged(), vec![msg::note_suspect(&n(M), &n(L))]);
    assert_eq!(
        f.status(),
        vec![format!(
            "note: {}",
            msg::note_suspect(&n(M), &n(L))
                .strip_prefix("note: ")
                .unwrap()
        )]
    );
    assert_eq!(
        live(&f.store, &f.rs, &f.home).unwrap().0,
        LiveId::HandChanged(n(L)),
        "an open suspicion blocks swaps"
    );
}

// --- Issue #1886: `.claude.json` takes no part -------------------------------

/// Case A of #1693: M has no login of its own. The old code recorded M from
/// `~/.claude.json`; nothing can name M now, so the ledger keeps L.
#[cfg(target_os = "linux")]
#[test]
fn a_login_less_holder_is_never_recorded() {
    let f = fx(false, true);
    assert_eq!(f.step(&seam(), T), Step::Clear);
    f.assert_nothing_moved();
    assert!(f.logged().is_empty(), "{:?}", f.logged());
    assert_eq!(f.ledger().hand_suspect, None);
}

/// `~/.claude.json` naming an account no profile has, or L's own: neither
/// is read; only the credential readings decide.
#[cfg(target_os = "linux")]
#[test]
fn claude_json_contents_change_nothing() {
    for json in [
        acct("u-nobody", "nobody@example.com"),
        acct("u-rs", "rs@example.com"),
    ] {
        let f = fx(true, false);
        private(&f.home.join(".claude.json"), &json);
        f.answer("~/.claude", Ok(a_l()));
        assert_eq!(f.step(&seam(), T), Step::Clear);
        f.assert_nothing_moved();
        assert!(f.logged().is_empty());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn no_login_in_home_is_left_to_the_usual_no_login_alert() {
    let f = fx(true, false);
    f.suspected();
    fs::remove_file(f.live()).unwrap();
    assert_eq!(f.step(&seam(), T), Step::Clear);
    assert!(f.logged().is_empty());
    assert_eq!(ino_at(&f.slot(M)), f.m);
    assert_eq!(live(&f.store, &f.rs, &f.home).unwrap().0, LiveId::NoLogin);
}

// --- The only-login rule (D7) ----------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_confirmed_suspicion_that_would_strand_l_gives_the_only_login_alert() {
    let f = fx_no_l(true, false);
    f.suspected();
    assert_eq!(f.step(&seam(), T), Step::Guided);
    f.assert_nothing_moved();
    let ledger = f.rs.ledger.display().to_string();
    let want = vec![
        msg::only_login(&n(M), &n(L)),
        format!(
            "fix 1: if a /login in ~/.claude replaced {L}'s login: close every claude session that uses ~/.claude, then run: mkdir -m 700 {0} && mv -n {1}/{M}/.credentials.json {0}/ && rm {ledger}; then run: amplihack profile login {L}",
            f.manual(),
            f.root.display()
        ),
        format!(
            "fix 2: if {L} and {M} are profiles of one account, run amplihack profile verify to see the account each folder holds, then remove one of the two profiles"
        ),
    ];
    assert_eq!(f.logged(), want);
    let l = f.ledger();
    assert_eq!(l.guidance.as_ref(), Some(&want));
    assert!(l.hand_suspect.is_some(), "the suspicion stays open");
    assert!(
        l.lost_login.is_empty(),
        "recovery adds nobody to lost_login"
    );
}

/// `fix 1` as printed, run by `sh`: M's copy goes aside and the ledger
/// goes, so the login in `~/.claude` is M's only login.
#[cfg(target_os = "linux")]
#[test]
fn the_only_login_fix_sets_ms_copy_aside() {
    let f = fx_no_l(true, false);
    f.suspected();
    f.step(&seam(), T);
    let line = &f.ledger().guidance.unwrap()[1];
    // As `profile login` leaves it: `.rejected-logins` exists, private.
    dir700(&f.rejected());
    let start = line.find("then run: ").unwrap() + "then run: ".len();
    let end = line.rfind("; then run: amplihack").unwrap();
    let st = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&line[start..end])
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .status()
        .unwrap();
    assert!(st.success(), "{line}");
    assert_eq!(ino_at(&f.slot(M)), None);
    assert_eq!(ino_at(&Path::new(&f.manual()).join(CREDS)), f.m);
    assert_eq!(ino_at(&f.live()), Some(f.h));
    assert!(!f.rs.ledger.exists());
}

// --- Refusals: a confirmed suspicion, a check fails ------------------------------

fn refused(f: &Fx, s: &Seam<'_>, reason: &str, fix: &str) {
    f.suspected();
    assert_eq!(f.step(s, T), Step::Guided);
    let want = vec![refused_head(reason), format!("fix 1: {fix}")];
    assert_eq!(f.logged(), want);
    assert_eq!(f.ledger().guidance.as_ref(), Some(&want));
    assert!(
        f.ledger().hand_suspect.is_some(),
        "the suspicion stays open"
    );
    f.assert_nothing_moved();
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

#[cfg(target_os = "linux")]
#[test]
fn a_shared_dot_claude_refuses() {
    let f = fx(true, false);
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

/// A pending swap journal is a gate: recovery makes no read and leaves the
/// journal to swap recovery.
#[cfg(target_os = "linux")]
#[test]
fn a_pending_journal_means_no_read_and_no_move() {
    let f = fx(true, false);
    f.suspected();
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
        identity: None,
        from_lock_ino: None,
        parking: None,
        completion: None,
        outgoing: None,
        outgoing_lock_ino: None,
        from_directory: None,
        movement_directories: None,
    })
    .unwrap();
    assert_eq!(f.step(&seam(), T), Step::Clear);
    assert!(f.logged().is_empty());
    assert!(
        f.rs.journal.exists(),
        "the journal is left for swap recovery"
    );
    assert_eq!(ino_at(&f.slot(M)), f.m);
}

/// SEC-9: a login replaced after the backup directory is created stops the
/// move, by inode, even though the re-reads still match.
#[cfg(target_os = "linux")]
#[test]
fn a_login_replaced_after_the_backup_is_created_moves_nothing() {
    let f = fx(true, false);
    let home_login = f.live();
    let again = move || {
        fs::remove_file(&home_login).unwrap();
        private(&home_login, r#"{"claudeAiOauth":{}}"#);
    };
    let s = Seam {
        after_create: &again,
        ..seam()
    };
    f.suspected();
    assert_eq!(f.step(&s, T), Step::Guided);
    assert_eq!(
        f.logged(),
        vec![
            refused_head("~/.claude changed during recovery"),
            "fix 1: none needed; the next cycle retries".to_string(),
        ]
    );
    assert_eq!(ino_at(&f.slot(M)), f.m, "M's copy stays");
    assert!(
        !f.backup("").exists(),
        "the empty backup directory is removed"
    );
}

/// The re-reads just before the move must still compare as Same. They run
/// before the backup directory is made, so a refusal leaves none.
#[cfg(target_os = "linux")]
#[test]
fn a_reading_that_changes_before_the_move_moves_nothing() {
    let f = fx(true, false);
    f.suspected();
    // `env_for` runs after the cycle's readings and before the re-reads.
    let env_for = || {
        READINGS.with(|r| {
            let p = f.home.join(".claude");
            r.borrow_mut().insert(p, Ok(a_c()));
        });
        seam()
    };
    let mut out = Sink::new(open_or_warn(&f.rs.log, &f.rs.log1), false, true);
    let read = |p: &Path| by_path(p);
    let step = step_with(&f.store, &f.rs, &f.home, &mut out, env_for, &read, GAP, T).unwrap();
    assert_eq!(step, Step::Guided);
    assert_eq!(
        f.logged()[0],
        refused_head("~/.claude changed during recovery")
    );
    assert_eq!(ino_at(&f.slot(M)), f.m);
    assert!(!f.rejected().exists(), "no backup directory was made");
}

// --- Interrupted recoveries (D8) -----------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn an_empty_backup_left_by_an_abort_makes_the_next_cycle_use_the_next_name() {
    let f = fx(true, false);
    f.suspected();
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
    f.suspected();
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

/// Stopped after the move, before the ledger write: M is no longer
/// compared, so the finding is Clear, the suspicion is cleared and the
/// ledger keeps L. No second backup is made.
#[cfg(target_os = "linux")]
#[test]
fn a_login_moved_but_not_recorded_leaves_the_ledger_on_l() {
    let f = fx(true, false);
    f.suspected();
    dir700(&f.rejected());
    dir700(&f.backup(""));
    fs::rename(f.slot(M), f.backup("").join(CREDS)).unwrap();
    assert_eq!(f.step(&seam(), T), Step::Clear);
    assert!(f.logged().is_empty());
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(L));
    assert_eq!(l.hand_suspect, None);
    assert_eq!(
        fs::read_dir(f.rejected()).unwrap().count(),
        1,
        "no second backup"
    );
    f.assert_each_login_once();
}

// --- Repetition and status (D5, D6, D9) ----------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn guidance_repeats_every_cycle_until_resolved() {
    let f = fx(true, false);
    let busy = Seam {
        liveness: Liveness::Known([M.to_string()].into()),
        ..seam()
    };
    f.suspected();
    let head = refused_head(&format!("a claude session is running on {M}"));
    let mut status = None;
    for k in 0..3 {
        assert_eq!(f.step(&busy, T + GAP * k), Step::Guided);
        let lines = f.logged();
        assert_eq!(count(&lines, &head), k as usize + 1, "cycle {k}: {lines:?}");
        let now = f.status();
        assert_eq!(now.len(), 2);
        assert_eq!(
            now[0],
            format!("alert: {}", head.strip_prefix("ALERT: ").unwrap())
        );
        if let Some(prev) = &status {
            assert_eq!(&now, prev, "status shows the same guidance each cycle");
        }
        status = Some(now);
    }
    // The session ends: the next cycle recovers.
    assert_eq!(f.step(&seam(), T + 3 * GAP), Step::Recovered);
    assert_eq!(f.ledger().guidance, None);
    assert!(f.status().is_empty());
    assert_eq!(count(&f.logged(), &head), 3, "no guidance after recovery");
}

/// A cannot-tell note is written once while it stays the same; a Clear
/// finding clears it.
#[cfg(target_os = "linux")]
#[test]
fn a_note_is_written_once_and_cleared_by_a_clear_finding() {
    let f = fx(true, false);
    f.answer("~/.claude", Err(reason::HTTP_429));
    let note = msg::note_unsure(&n(L), &super::cred_identity::Why::Home(reason::HTTP_429));
    for k in 0..3 {
        assert_eq!(f.step(&seam(), T + GAP * k), Step::Noted);
    }
    assert_eq!(f.logged(), vec![note.clone()]);
    assert_eq!(
        f.status(),
        vec![format!("note: {}", note.strip_prefix("note: ").unwrap())]
    );
    f.answer("~/.claude", Ok(a_l()));
    assert_eq!(f.step(&seam(), T + 3 * GAP), Step::Clear);
    assert_eq!(f.ledger().guidance, None);
    assert!(f.status().is_empty());
}

/// `lost_login` entries written by an earlier release are still reported
/// every cycle, and dropped once the profile has a login or is gone.
#[cfg(target_os = "linux")]
#[test]
fn an_earlier_lost_login_repeats_until_l_has_a_login() {
    let f = fx_no_l(false, false);
    let mut l = f.ledger();
    l.lost_login = vec![L.into()];
    f.rs.save_ledger(&l).unwrap();
    let warning = format!("warning: {}", lost_l().strip_prefix("ALERT: ").unwrap());
    for k in 1..=2 {
        assert_eq!(f.step(&seam(), T + GAP * k), Step::Clear);
        assert_eq!(count(&f.logged(), &lost_l()), k as usize);
        assert_eq!(f.status(), vec![warning.clone()]);
    }
    login(&f.slot(L));
    assert_eq!(f.step(&seam(), T + 3 * GAP), Step::Clear);
    assert_eq!(
        count(&f.logged(), &lost_l()),
        2,
        "no warning once L has a login"
    );
    assert!(f.ledger().lost_login.is_empty());
    assert!(f.status().is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn a_removed_or_symlinked_profile_leaves_lost_login() {
    for symlinked in [false, true] {
        let f = fx_no_l(false, false);
        let mut l = f.ledger();
        l.lost_login = vec![L.into()];
        f.rs.save_ledger(&l).unwrap();
        fs::remove_dir_all(f.root.join(L)).unwrap();
        if symlinked {
            let elsewhere = f.base.join("elsewhere");
            dir700(&elsewhere);
            std::os::unix::fs::symlink(&elsewhere, f.root.join(L)).unwrap();
        }
        f.step(&seam(), T);
        assert!(f.ledger().lost_login.is_empty());
        assert!(f.logged().is_empty());
    }
}

#[test]
fn status_cleans_control_characters_again() {
    let l = RotateLedger {
        guidance: Some(vec![
            "ALERT: x\u{1b}[31m red\u{7}".into(),
            "note: y\u{1b}[0m".into(),
        ]),
        ..RotateLedger::default()
    };
    let out = status_alerts(&l);
    assert_eq!(out.len(), 2);
    assert!(out[0].starts_with("alert: "), "{out:?}");
    assert!(out[1].starts_with("note: "), "{out:?}");
    assert!(
        !out.iter().any(|o| o.chars().any(char::is_control)),
        "{out:?}"
    );
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
    f.suspected();
    let before = snapshot(&f);
    let lines = f.preview_with(seam, T);
    assert_eq!(snapshot(&f), before);
    assert_eq!(
        lines,
        vec![format!(
            "would recover: ~/.claude and {M} held the same account; kept ~/.claude, moved {M}'s copy to {}; recorded {M} as live",
            f.backup("").join(CREDS).display()
        )]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn preview_of_guidance_prints_the_alert_and_fix_lines() {
    let f = fx_no_l(true, false);
    f.suspected();
    let before = snapshot(&f);
    let lines = f.preview_with(seam, T);
    assert_eq!(snapshot(&f), before);
    assert_eq!(lines.len(), 3, "{lines:?}");
    let alert = msg::only_login(&n(M), &n(L));
    assert_eq!(
        lines[0],
        format!("would alert: {}", alert.strip_prefix("ALERT: ").unwrap())
    );
    assert!(lines[1].starts_with("fix 1: if a /login in ~/.claude replaced "));
    assert!(lines[2].starts_with(&format!(
        "fix 2: if {L} and {M} are profiles of one account"
    )));
}

#[cfg(target_os = "linux")]
#[test]
fn preview_without_a_hand_change_prints_nothing() {
    let f = fx(true, false);
    let mut l = f.ledger();
    l.live = Some(M.into());
    l.account = Some("u-mistt".into());
    f.rs.save_ledger(&l).unwrap();
    assert!(f.preview_with(seam, T).is_empty());
}

// --- No identity leaks, hostile paths, limits -----------------------------------

#[cfg(target_os = "linux")]
#[test]
fn accounts_never_appear_in_the_log_status_or_preview() {
    for l_own in [true, false] {
        let f = fx_at("home", l_own, true, true);
        for (p, e) in [
            (L, "rs@example.com"),
            (M, "mistt@example.com"),
            (C, "src@example.com"),
        ] {
            private(&f.root.join(p).join(".claude.json"), &email_only(e));
        }
        private(
            &f.home.join(".claude.json"),
            &email_only("mistt@example.com"),
        );
        let mut l = f.ledger();
        l.account = Some("rs@example.com".into());
        f.rs.save_ledger(&l).unwrap();
        f.step(&seam(), T - GAP);
        let shown = f.preview_with(seam, T);
        f.step(&seam(), T);
        let mut text = f.logged();
        text.extend(shown);
        text.extend(f.status());
        assert!(text.len() >= 2, "{text:?}");
        for line in &text {
            assert!(!line.contains("@example.com"), "{line}");
            assert!(
                !line.contains("u-mistt") && !line.contains("u-rs"),
                "{line}"
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn paths_in_fix_lines_are_quoted_and_unsafe_ones_are_not_printed() {
    let shared = |f: &Fx| {
        let dot = f.home.join(".claude");
        fs::set_permissions(&dot, fs::Permissions::from_mode(0o777)).unwrap();
        f.suspected();
        assert_eq!(f.step(&seam(), T), Step::Guided);
        fs::set_permissions(&dot, fs::Permissions::from_mode(0o700)).unwrap();
        f.logged().pop().unwrap()
    };
    let f = fx_at("ho me", true, true, false);
    let fix = shared(&f);
    let want = format!("chmod 700 '{}/.claude'", f.home.display());
    assert!(fix.contains(&want), "{fix}");

    let f = fx_at("it's", true, true, false);
    let fix = shared(&f);
    assert!(fix.contains(r"it'\''s/.claude'"), "{fix}");

    for bad in ["new\nline", "bidi\u{202e}x"] {
        let f = fx_at(bad, true, true, false);
        assert_eq!(
            shared(&f),
            "fix 1: move the login in ~/.claude by hand; its path cannot be printed safely"
        );
        assert!(f.rs.load_ledger().is_ok(), "the ledger stays loadable");
    }
}

/// A home path so long that the fix command does not fit in one line: the
/// command is replaced, never cut (a cut path is another path).
#[cfg(target_os = "linux")]
#[test]
fn guidance_for_a_very_long_home_stays_within_the_ledger_limits() {
    for parts in [4, 5] {
        let deep = vec!["d".repeat(240); parts].join("/");
        let f = fx_at(&deep, true, true, false);
        let dot = f.home.join(".claude");
        fs::set_permissions(&dot, fs::Permissions::from_mode(0o777)).unwrap();
        f.suspected();
        assert_eq!(f.step(&seam(), T), Step::Guided);
        fs::set_permissions(&dot, fs::Permissions::from_mode(0o700)).unwrap();
        let l =
            f.rs.load_ledger()
                .expect("the ledger it wrote loads")
                .unwrap();
        let g = l.guidance.unwrap();
        assert!(!g.is_empty() && g.len() <= 16);
        assert!(g.iter().all(|line| line.len() <= 1024), "{g:?}");
        assert!(f.logged().iter().all(|line| line.len() <= 1024));
        assert_eq!(
            g[1], "fix 1: move the login in ~/.claude by hand; its path cannot be printed safely",
            "{parts} parts"
        );
        for line in g.iter().chain(&f.logged()) {
            assert!(!line.contains("chmod"), "no cut command: {line}");
        }
    }
}

// --- Review follow-ups ---------------------------------------------------------

/// The `/proc` scan and the auto-switch ledger (the `Env`) are read only
/// for a confirmed suspicion, in the cycle and in `--dry-run`.
#[cfg(target_os = "linux")]
#[test]
fn the_env_is_built_only_for_a_confirmed_suspicion() {
    let f = fx(true, true);
    let never = || -> Seam<'static> { panic!("the Env was built without a confirmed suspicion") };
    assert_eq!(f.preview_with(never, T).len(), 1, "would note");
    let mut out = Sink::new(open_or_warn(&f.rs.log, &f.rs.log1), false, true);
    let read = |p: &Path| by_path(p);
    let step = step_with(
        &f.store,
        &f.rs,
        &f.home,
        &mut out,
        never,
        &read,
        GAP,
        T - GAP,
    )
    .unwrap();
    assert_eq!(step, Step::Noted);

    let built = std::cell::Cell::new(0);
    let counted = || {
        built.set(built.get() + 1);
        seam()
    };
    assert!(!f.preview_with(counted, T).is_empty());
    assert_eq!(built.get(), 1);
    let step = step_with(&f.store, &f.rs, &f.home, &mut out, counted, &read, GAP, T).unwrap();
    assert_eq!((step, built.get()), (Step::Recovered, 2));

    assert!(f.preview_with(never, T + GAP).is_empty());
    let step = step_with(
        &f.store,
        &f.rs,
        &f.home,
        &mut out,
        never,
        &read,
        GAP,
        T + GAP,
    )
    .unwrap();
    assert_eq!(step, Step::Clear);
}

/// An existing `.rejected-logins` others can read refuses the move with
/// its own reason, as `profile login` requires `0700`.
#[cfg(target_os = "linux")]
#[test]
fn a_rejected_logins_dir_others_can_read_refuses_with_its_own_reason() {
    let f = fx(true, false);
    f.suspected();
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
