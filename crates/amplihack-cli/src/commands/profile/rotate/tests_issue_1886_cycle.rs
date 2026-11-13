//! Issue #1886 through `handfix::step_with`, `handfix::preview` and the
//! real `run_cycle`: recovery decides from credential readings only, waits
//! `SWAP_GRACE` after a swap, needs two matching readings on two cycles,
//! and never leaves a profile with no login anywhere.
//!
//! Contract (in addition to `tests_issue_1886.rs`):
//! - `handfix::step_with(&ProfileStore, &RotateState, home, &mut Sink,
//!   env_for, read: &dyn Fn(&Path) -> Result<Account, &'static str>,
//!   min_gap: i64, t: i64) -> Result<Step, ProfileError>`. `min_gap` is the
//!   rotation interval, clamped to at least 1 by the caller.
//! - `handfix::preview(&ProfileStore, &RotateState, home, env_for, read,
//!   min_gap, t) -> Result<Vec<String>, ProfileError>`: the dry-run lines,
//!   `would recover: `, `would note: ` or `would alert: ` followed by the
//!   `fix` lines; never writes the ledger.
//! - `Ctx::identity: cred_identity::ReadFn`, used by the prelude.
//!
//! The reader is a fake keyed by path (`<home>/.claude` or `<root>/<name>`),
//! except in the leak test, which reads fake `fake-at-1886-<uuid>` tokens
//! through `read_with` and the loopback `FakeHttp`. Nothing here sets a
//! process variable or touches the real home.

use super::cred_identity::read_with;
#[cfg(target_os = "linux")]
use super::cycle::Ctx;
#[cfg(target_os = "linux")]
use super::cycle::{CycleOutcome, Mode, run_cycle};
#[cfg(target_os = "linux")]
use super::decide::{Decision, Reading};
use super::hand_fields::{HandSuspect, SWAP_GRACE};
use super::handfix::{Env as Seam, Step, preview, step_with};
use super::handfix_msg as msg;
use super::handfix_plan::{Input, Plan, plan};
use super::identify::Account;
#[cfg(target_os = "linux")]
use super::identify::LiveId;
use super::ledger::{RotateLedger, RotateState};
#[cfg(target_os = "linux")]
use super::log::msg as log_msg;
use super::log::{Sink, open_or_warn};
#[cfg(target_os = "linux")]
use super::observe::{Measurement, Snap};
#[cfg(target_os = "linux")]
use super::settings;
use super::tests_handfix::seam;
use super::tests_issue_1886::{
    K, L, M, creds_doc, endpoints, fake_by_token, fake_token, profile_body, server,
};
use super::tests_swap::{ino_at, n, private};
#[cfg(target_os = "linux")]
use crate::commands::profile::ProfileError;
#[cfg(target_os = "linux")]
use crate::commands::profile::auto_switch::kit::{Liveness, now};
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::tests_fake_http::Reply;
use crate::commands::profile::usage_direct::reason;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const H: &str = "hotel";
const CREDS: &str = ".credentials.json";
/// 2026-11-03T09:00:00Z.
const T: i64 = 1_793_696_400;
const GAP: i64 = 120;

type Answer = Result<Account, &'static str>;

// --- Fixture -----------------------------------------------------------------

struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
    root: PathBuf,
    store: ProfileStore,
    rs: RotateState,
    /// Fake token of each login, by place (`~/.claude` or a profile name).
    tokens: HashMap<String, String>,
    answers: RefCell<HashMap<PathBuf, Answer>>,
    reads: Cell<usize>,
}

fn dir700(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

fn acct(uuid: &str) -> String {
    serde_json::json!({"oauthAccount": {"accountUuid": uuid, "emailAddress": format!("{uuid}@example.com")}})
        .to_string()
}

/// Profiles `L`, `H`, `M` and `K`; those in `own` have a login of their
/// own. `~/.claude` holds a login; `~/.claude.json` names `home_json`. The
/// ledger names `live`, last swapped at `swapped_at`.
fn fx(own: &[&str], live: &str, swapped_at: Option<i64>, home_json: &str) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (home, root) = (base.join("home"), base.join("root"));
    let store = ProfileStore::new(root.clone());
    let mut tokens = HashMap::new();
    for p in [L, H, M, K] {
        store.add(&n(p)).unwrap();
        private(&root.join(p).join(".claude.json"), &acct(&format!("u-{p}")));
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    for p in own {
        let t = fake_token();
        private(&root.join(p).join(CREDS), &creds_doc(&t, 3_600_000));
        tokens.insert(p.to_string(), t);
    }
    dir700(&home);
    dir700(&home.join(".claude"));
    let t = fake_token();
    private(&home.join(".claude").join(CREDS), &creds_doc(&t, 3_600_000));
    tokens.insert("~/.claude".into(), t);
    private(&home.join(".claude.json"), &acct(&format!("u-{home_json}")));
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    rs.save_ledger(&RotateLedger {
        live: Some(live.into()),
        account: Some(format!("u-{home_json}")),
        since: swapped_at,
        swapped_at,
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
        tokens,
        answers: RefCell::default(),
        reads: Cell::new(0),
    }
}

fn a_l() -> Account {
    server("u-lima")
}
fn a_h() -> Account {
    server("u-hotel")
}
fn a_m() -> Account {
    server("u-mike")
}
fn a_k() -> Account {
    server("u-kilo")
}

impl Fx {
    /// Sets what the fake reader returns for `place` (`~/.claude` or a
    /// profile name).
    fn answer(&self, place: &str, a: Answer) {
        let p = if place == "~/.claude" {
            self.home.join(".claude")
        } else {
            self.root.join(place)
        };
        self.answers.borrow_mut().insert(p, a);
    }

    fn read(&self, p: &Path) -> Answer {
        self.reads.set(self.reads.get() + 1);
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        self.answers
            .borrow()
            .get(&p)
            .cloned()
            .unwrap_or(Err(reason::NO_FILE))
    }

    fn step(&self, t: i64) -> Step {
        let mut out = Sink::new(open_or_warn(&self.rs.log, &self.rs.log1), false, true);
        let read = |p: &Path| self.read(p);
        step_with(
            &self.store,
            &self.rs,
            &self.home,
            &mut out,
            seam,
            &read,
            GAP,
            t,
        )
        .unwrap()
    }

    fn preview(&self, t: i64) -> Vec<String> {
        let read = |p: &Path| self.read(p);
        preview(&self.store, &self.rs, &self.home, seam, &read, GAP, t).unwrap()
    }

    fn logged(&self) -> Vec<String> {
        fs::read_to_string(&self.rs.log)
            .unwrap_or_default()
            .lines()
            .map(|l| l.split_once(' ').unwrap().1.to_string())
            .collect()
    }

    fn ledger(&self) -> RotateLedger {
        self.rs.load_ledger().unwrap().unwrap()
    }

    fn set_ledger(&self, f: impl FnOnce(&mut RotateLedger)) {
        let mut l = self.ledger();
        f(&mut l);
        self.rs.save_ledger(&l).unwrap();
    }

    fn home_login(&self) -> PathBuf {
        self.home.join(".claude").join(CREDS)
    }

    fn slot(&self, p: &str) -> PathBuf {
        self.root.join(p).join(CREDS)
    }

    fn rejected(&self) -> PathBuf {
        self.root.join(".rejected-logins")
    }

    /// Login files under `.rejected-logins`.
    fn backed_up(&self) -> Vec<PathBuf> {
        let Ok(rd) = fs::read_dir(self.rejected()) else {
            return Vec::new();
        };
        rd.map(|e| e.unwrap().path().join(CREDS))
            .filter(|p| p.exists())
            .collect()
    }

    /// The `rotate-*` directories under `.rejected-logins`, empty or not.
    fn rotate_dirs(&self) -> Vec<PathBuf> {
        let Ok(rd) = fs::read_dir(self.rejected()) else {
            return Vec::new();
        };
        rd.map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("rotate-")
            })
            .collect()
    }

    /// The inode of every login: `~/.claude` and each profile's own.
    fn inodes(&self) -> Vec<(String, Option<u64>)> {
        let mut v = vec![("~/.claude".to_string(), ino_at(&self.home_login()))];
        for p in [L, H, M, K] {
            v.push((p.to_string(), ino_at(&self.slot(p))));
        }
        v
    }

    /// The profiles that have a login somewhere: their own file, or the one
    /// in `~/.claude` when the ledger names them as live.
    fn with_login(&self) -> BTreeSet<String> {
        let mut s: BTreeSet<String> = [L, H, M, K]
            .into_iter()
            .filter(|p| self.slot(p).exists())
            .map(String::from)
            .collect();
        if self.home_login().exists()
            && let Some(live) = self.ledger().live
        {
            s.insert(live);
        }
        s
    }
}

/// The invariant of every scenario: no profile that had a login before the
/// cycle ends it with no login anywhere.
#[track_caller]
fn assert_no_new_lost(before: &BTreeSet<String>, f: &Fx) {
    let after = f.with_login();
    let lost: Vec<&String> = before.difference(&after).collect();
    assert!(lost.is_empty(), "lost every login: {lost:?}");
}

#[track_caller]
fn assert_nothing_moved(f: &Fx, inodes: &[(String, Option<u64>)]) {
    assert_eq!(f.inodes(), inodes, "a login moved");
    assert!(f.backed_up().is_empty(), "{:?}", f.backed_up());
}

fn no_recovered(lines: &[String]) {
    assert!(
        !lines.iter().any(|l| l.starts_with("recovered:")),
        "{lines:?}"
    );
}

// --- #1886 regression ---------------------------------------------------------

/// The 3 Nov case: a swap L -> H saved L's login under L and put H's into
/// `~/.claude`; running sessions then wrote L's account back into
/// `~/.claude.json`. The old code took that file for a hand change, moved
/// H's... and in the incident filed a login under the wrong profile. Now:
/// within the grace period nothing is read; after it, the credential in
/// `~/.claude` answers as H, unlike every profile's own login, so the
/// finding is Clear.
#[test]
fn issue_1886_a_stale_claude_json_after_a_swap_changes_nothing() {
    let f = fx(&[L, K], H, Some(T), L);
    f.answer("~/.claude", Ok(a_h()));
    f.answer(L, Ok(a_l()));
    f.answer(K, Ok(a_k()));
    let (inodes, before) = (f.inodes(), f.ledger());
    let had = f.with_login();
    for t in [T + 1, T + 120, T + 599, T + 600, T + 720, T + 3600] {
        assert_eq!(f.step(t), Step::Clear, "at +{}", t - T);
        assert_no_new_lost(&had, &f);
    }
    assert_nothing_moved(&f, &inodes);
    assert!(!f.rejected().exists(), "no .rejected-logins at all");
    let l = f.ledger();
    assert_eq!(l.live, before.live);
    assert_eq!(l.account, before.account);
    assert_eq!(l.hand_suspect, None);
    assert_eq!(l.guidance, None);
    assert!(f.logged().is_empty(), "{:?}", f.logged());
}

// --- (a) recovery after two readings --------------------------------------------

/// `~/.claude` answers as M, which has its own login; L keeps a login of
/// its own, so the move leaves nobody without one.
fn case_a() -> Fx {
    let f = fx(&[L, M, K], L, Some(T - 3600), M);
    f.answer("~/.claude", Ok(a_m()));
    f.answer(L, Ok(a_l()));
    f.answer(M, Ok(a_m()));
    f.answer(K, Ok(a_k()));
    f
}

#[test]
fn a_first_reading_only_records_the_suspicion() {
    let f = case_a();
    let (inodes, had) = (f.inodes(), f.with_login());
    assert_ne!(f.step(T), Step::Recovered);
    assert_nothing_moved(&f, &inodes);
    assert_no_new_lost(&had, &f);
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(L), "not relabelled");
    assert_eq!(
        l.hand_suspect,
        Some(HandSuspect {
            profile: M.into(),
            first_seen: T
        })
    );
    assert_eq!(f.logged(), vec![msg::note_suspect(&n(M), &n(L))]);
}

#[test]
fn a_second_matching_reading_before_the_interval_moves_nothing() {
    let f = case_a();
    let inodes = f.inodes();
    f.step(T);
    assert_ne!(f.step(T + GAP - 1), Step::Recovered);
    assert_nothing_moved(&f, &inodes);
    assert_eq!(f.ledger().hand_suspect.unwrap().first_seen, T);
}

#[test]
fn a_second_matching_reading_recovers_as_before() {
    let f = case_a();
    let had = f.with_login();
    let (home_ino, m_ino, l_ino) = (
        ino_at(&f.home_login()),
        ino_at(&f.slot(M)),
        ino_at(&f.slot(L)),
    );
    f.step(T);
    assert_eq!(f.step(T + GAP), Step::Recovered);
    let backups = f.backed_up();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(ino_at(&backups[0]), m_ino, "M's own copy, moved aside");
    assert!(
        backups[0]
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(&format!("rotate-{M}-"))
    );
    assert_eq!(ino_at(&f.home_login()), home_ino, "~/.claude kept");
    assert_eq!(ino_at(&f.slot(L)), l_ino, "L keeps its own login");
    assert_eq!(ino_at(&f.slot(M)), None);
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(M));
    assert_eq!(l.since, Some(T + GAP));
    assert_eq!(
        l.account.as_deref(),
        Some("u-mike"),
        "from trusted credential evidence"
    );
    assert_eq!(l.hand_suspect, None);
    assert_eq!(l.guidance, None);
    let log = f.logged();
    assert_eq!(
        log.last().unwrap(),
        &msg::recovered_moved(&n(M), &backups[0])
    );
    assert_no_new_lost(&had, &f);
}

/// SEC-9: a login that changes after the backup directory is made stops
/// the move.
#[test]
fn a_login_replaced_during_recovery_is_not_moved() {
    let f = case_a();
    f.step(T);
    let home = f.home_login();
    let swap_in = || {
        fs::remove_file(&home).unwrap();
        private(&home, &creds_doc(&fake_token(), 3_600_000));
    };
    let s = Seam {
        after_create: &swap_in,
        ..seam()
    };
    let mut out = Sink::new(open_or_warn(&f.rs.log, &f.rs.log1), false, true);
    let read = |p: &Path| f.read(p);
    let step = step_with(
        &f.store,
        &f.rs,
        &f.home,
        &mut out,
        || s.clone(),
        &read,
        GAP,
        T + GAP,
    )
    .unwrap();
    assert_ne!(step, Step::Recovered);
    assert!(f.slot(M).exists(), "M's copy stays");
    assert!(f.rotate_dirs().is_empty(), "{:?}", f.rotate_dirs());
    assert_eq!(f.ledger().live.as_deref(), Some(L));
    assert!(
        f.logged()
            .iter()
            .any(|l| l.contains("~/.claude changed during recovery")),
        "{:?}",
        f.logged()
    );
}

/// The re-read just before the move must still say Same.
#[test]
fn a_reread_that_no_longer_matches_stops_the_move() {
    let f = case_a();
    f.step(T);
    let n_reads = Cell::new(0);
    let read = |p: &Path| {
        n_reads.set(n_reads.get() + 1);
        // Everything after the cycle's first round of reads answers as K.
        if n_reads.get() > 5 {
            Ok(a_k())
        } else {
            f.read(p)
        }
    };
    let mut out = Sink::new(open_or_warn(&f.rs.log, &f.rs.log1), false, true);
    let step = step_with(
        &f.store,
        &f.rs,
        &f.home,
        &mut out,
        seam,
        &read,
        GAP,
        T + GAP,
    )
    .unwrap();
    assert_ne!(step, Step::Recovered);
    assert!(f.slot(M).exists());
    assert!(f.backed_up().is_empty());
    assert!(f.rotate_dirs().is_empty(), "{:?}", f.rotate_dirs());
}

/// A re-read that fails refuses before the backup directory is made, so
/// each cycle's retry adds no directory under `.rejected-logins`.
#[test]
fn a_failed_reread_leaves_no_backup_directory() {
    let f = case_a();
    f.step(T);
    let ino_m = ino_at(&f.slot(M));
    for (i, t) in [T + GAP, T + 2 * GAP, T + 3 * GAP].into_iter().enumerate() {
        let n_reads = Cell::new(0);
        // The cycle's first round reads `~/.claude`, K, L and M.
        let read = |p: &Path| {
            n_reads.set(n_reads.get() + 1);
            if n_reads.get() > 4 {
                Err(reason::TRANSPORT)
            } else {
                f.read(p)
            }
        };
        let mut out = Sink::new(open_or_warn(&f.rs.log, &f.rs.log1), false, true);
        let step = step_with(&f.store, &f.rs, &f.home, &mut out, seam, &read, GAP, t).unwrap();
        assert_eq!(step, Step::Guided, "cycle {i}");
        assert_eq!(n_reads.get(), 5, "cycle {i}: one re-read, then the refusal");
        assert_eq!(ino_at(&f.slot(M)), ino_m, "M's copy stays");
        assert!(
            f.rotate_dirs().is_empty(),
            "cycle {i}: {:?}",
            f.rotate_dirs()
        );
    }
    assert_eq!(f.ledger().live.as_deref(), Some(L));
    assert!(
        f.ledger().hand_suspect.is_some(),
        "the suspicion stays open"
    );
}

/// SEC-5: the moved copy keeps mode 0600 and its backup directory is 0700.
#[cfg(unix)]
#[test]
fn the_moved_copy_and_its_directory_are_private() {
    let f = case_a();
    f.step(T);
    assert_eq!(f.step(T + GAP), Step::Recovered);
    let backups = f.backed_up();
    assert_eq!(backups.len(), 1, "{backups:?}");
    let mode = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode(&backups[0]), 0o600, "{}", backups[0].display());
    assert_eq!(mode(backups[0].parent().unwrap()), 0o700);
    assert_eq!(mode(&f.rejected()), 0o700);
}

// --- (b) cannot tell -------------------------------------------------------------

/// Each unsure case writes a note, moves nothing, relabels nothing and
/// clears an open suspicion.
#[test]
fn unsure_readings_give_a_note_and_clear_the_suspicion() {
    let cases: Vec<(&str, Vec<(&str, Answer)>)> = vec![
        ("failed read", vec![("~/.claude", Err(reason::HTTP_429))]),
        (
            "unread profile",
            vec![("~/.claude", Ok(a_h())), (M, Err(reason::HTTP_401))],
        ),
        (
            "multiple matches",
            vec![("~/.claude", Ok(a_m())), (M, Ok(a_m())), (K, Ok(a_m()))],
        ),
    ];
    for (what, answers) in cases {
        let f = fx(&[L, M, K], L, Some(T - 3600), M);
        f.answer(L, Ok(a_l()));
        f.answer(K, Ok(a_k()));
        for (place, a) in answers {
            f.answer(place, a);
        }
        f.set_ledger(|l| {
            l.hand_suspect = Some(HandSuspect {
                profile: M.into(),
                first_seen: T - 1000,
            })
        });
        let (inodes, had) = (f.inodes(), f.with_login());
        assert_ne!(f.step(T), Step::Recovered, "{what}");
        assert_nothing_moved(&f, &inodes);
        assert_no_new_lost(&had, &f);
        let l = f.ledger();
        assert_eq!(l.live.as_deref(), Some(L), "{what}");
        assert_eq!(l.hand_suspect, None, "{what}: suspicion cleared");
        let log = f.logged();
        assert_eq!(log.len(), 1, "{what}: {log:?}");
        assert!(
            log[0].starts_with("note: cannot tell whose login is in ~/.claude (live=lima): "),
            "{what}: {log:?}"
        );
    }
}

#[test]
fn a_suspect_then_another_suspect_replaces_the_record() {
    let f = case_a();
    let inodes = f.inodes();
    f.step(T);
    f.answer("~/.claude", Ok(a_k()));
    assert_ne!(f.step(T + GAP), Step::Recovered);
    assert_nothing_moved(&f, &inodes);
    assert_eq!(
        f.ledger().hand_suspect,
        Some(HandSuspect {
            profile: K.into(),
            first_seen: T + GAP
        })
    );
    // K needs its own second reading.
    assert_ne!(f.step(T + GAP + 1), Step::Recovered);
    assert_nothing_moved(&f, &inodes);
}

#[test]
fn a_suspect_then_a_clear_reading_clears_the_record() {
    let f = case_a();
    f.step(T);
    f.answer("~/.claude", Ok(a_h()));
    assert_eq!(f.step(T + GAP), Step::Clear);
    assert_eq!(f.ledger().hand_suspect, None);
    // A later match starts again from one reading.
    f.answer("~/.claude", Ok(a_m()));
    assert_ne!(f.step(T + 2 * GAP), Step::Recovered);
    assert_eq!(f.ledger().hand_suspect.unwrap().first_seen, T + 2 * GAP);
}

/// D9: the same note on consecutive cycles is written once.
#[test]
fn a_repeated_note_is_written_once() {
    let f = case_a();
    f.answer("~/.claude", Err(reason::EXPIRED));
    for i in 0..4 {
        f.step(T + i * GAP);
    }
    assert_eq!(f.logged().len(), 1, "{:?}", f.logged());
}

/// SEC-20: a `~/.claude` that cannot be read costs one request a cycle.
#[test]
fn a_failing_home_read_costs_one_request() {
    let f = case_a();
    f.answer("~/.claude", Err(reason::HTTP_401));
    f.step(T);
    assert_eq!(f.reads.get(), 1);
}

// --- (c) only-login rule ----------------------------------------------------------

/// `~/.claude` answers as M on two readings, but L has no login of its own:
/// recording M would leave L with none. Refused, both logins stay.
#[test]
fn a_confirmed_match_that_would_strand_the_ledgers_profile_is_refused() {
    let f = fx(&[M, K], L, Some(T - 3600), M);
    f.answer("~/.claude", Ok(a_m()));
    f.answer(M, Ok(a_m()));
    f.answer(K, Ok(a_k()));
    let (inodes, had) = (f.inodes(), f.with_login());
    assert_ne!(f.step(T), Step::Recovered);
    assert_eq!(f.step(T + GAP), Step::Guided);
    assert_nothing_moved(&f, &inodes);
    assert_no_new_lost(&had, &f);
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(L), "not relabelled");
    assert!(l.hand_suspect.is_some(), "the suspicion stays open");
    let log = f.logged();
    let alert = msg::only_login(&n(M), &n(L));
    let at = log
        .iter()
        .position(|x| *x == alert)
        .unwrap_or_else(|| panic!("{log:?}"));
    assert!(log[at + 1].starts_with("fix 1: "), "{log:?}");
    assert!(log[at + 2].starts_with("fix 2: "), "{log:?}");
    assert_eq!(l.guidance.as_ref().unwrap()[0], alert);
}

/// SEC-8: a symlink in L's slot is no login.
#[test]
fn a_symlinked_login_does_not_count_for_the_only_login_rule() {
    let f = fx(&[M, K], L, Some(T - 3600), M);
    let elsewhere = f.base.join("elsewhere.json");
    private(&elsewhere, &creds_doc(&fake_token(), 3_600_000));
    std::os::unix::fs::symlink(&elsewhere, f.slot(L)).unwrap();
    f.answer("~/.claude", Ok(a_m()));
    f.answer(M, Ok(a_m()));
    f.step(T);
    assert_eq!(f.step(T + GAP), Step::Guided);
    assert!(f.slot(M).exists());
    assert!(f.backed_up().is_empty());
}

/// SEC-8: an empty file in L's slot is no login.
#[test]
fn an_empty_login_file_does_not_count_for_the_only_login_rule() {
    let f = fx(&[M, K], L, Some(T - 3600), M);
    private(&f.slot(L), "");
    assert_only_login_refused(&f);
}

/// SEC-8: a login file that cannot be read is no login.
#[cfg(unix)]
#[test]
fn an_unreadable_login_file_does_not_count_for_the_only_login_rule() {
    if crate::commands::profile::auto_switch::euid() == 0 {
        return; // root reads a mode-000 file anyway.
    }
    let f = fx(&[M, K], L, Some(T - 3600), M);
    private(&f.slot(L), &creds_doc(&fake_token(), 3_600_000));
    fs::set_permissions(f.slot(L), fs::Permissions::from_mode(0o000)).unwrap();
    assert_only_login_refused(&f);
}

/// `~/.claude` answers as M on two readings and L's slot holds no usable
/// login: the only-login ALERT, and nothing moves.
#[track_caller]
fn assert_only_login_refused(f: &Fx) {
    f.answer("~/.claude", Ok(a_m()));
    f.answer(M, Ok(a_m()));
    f.answer(K, Ok(a_k()));
    let inodes = f.inodes();
    f.step(T);
    assert_eq!(f.step(T + GAP), Step::Guided);
    assert_nothing_moved(f, &inodes);
    assert!(f.rotate_dirs().is_empty(), "{:?}", f.rotate_dirs());
    assert_eq!(f.ledger().live.as_deref(), Some(L));
    let alert = msg::only_login(&n(M), &n(L));
    assert!(f.logged().contains(&alert), "{:?}", f.logged());
}

// --- (d) grace period --------------------------------------------------------------

#[test]
fn within_swap_grace_nothing_is_read_or_written() {
    let f = fx(&[L, M], L, Some(T), M);
    f.answer("~/.claude", Ok(a_m()));
    f.answer(M, Ok(a_m()));
    let (inodes, before) = (f.inodes(), f.ledger());
    for t in (T..T + SWAP_GRACE).step_by(37).chain([T + SWAP_GRACE - 1]) {
        assert_eq!(f.step(t), Step::Clear);
    }
    assert_eq!(f.reads.get(), 0, "no request inside the grace period");
    assert_nothing_moved(&f, &inodes);
    assert_eq!(f.ledger(), before);
    assert!(f.logged().is_empty());
}

#[test]
fn a_suspicion_from_before_the_last_swap_is_not_confirmed() {
    let f = fx(&[L, M], L, Some(T - SWAP_GRACE), M);
    f.answer("~/.claude", Ok(a_m()));
    f.answer(L, Ok(a_l()));
    f.answer(M, Ok(a_m()));
    f.set_ledger(|l| {
        l.hand_suspect = Some(HandSuspect {
            profile: M.into(),
            first_seen: T - SWAP_GRACE - 100,
        })
    });
    let inodes = f.inodes();
    assert_ne!(f.step(T), Step::Recovered);
    assert_nothing_moved(&f, &inodes);
    assert_eq!(f.ledger().hand_suspect.unwrap().first_seen, T);
}

#[test]
fn a_suspicion_dated_in_the_future_is_reset() {
    let f = case_a();
    f.set_ledger(|l| {
        l.hand_suspect = Some(HandSuspect {
            profile: M.into(),
            first_seen: T + 50_000,
        })
    });
    let inodes = f.inodes();
    assert_ne!(f.step(T), Step::Recovered);
    assert_nothing_moved(&f, &inodes);
    assert_eq!(f.ledger().hand_suspect.unwrap().first_seen, T);
}

#[test]
fn a_swap_time_far_in_the_future_gives_a_note_and_no_read() {
    let f = fx(&[L, M], L, Some(T + SWAP_GRACE + 1), M);
    f.answer("~/.claude", Ok(a_m()));
    f.answer(M, Ok(a_m()));
    let inodes = f.inodes();
    f.step(T);
    assert_eq!(f.reads.get(), 0);
    assert_nothing_moved(&f, &inodes);
    assert_eq!(
        f.logged(),
        vec![
            "note: cannot tell whose login is in ~/.claude (live=lima): the last swap time is later than now; nothing moved"
        ]
    );
}

/// SEC-13: no ledger time makes the plan panic. The ledger refuses to load
/// a `first_seen` at or below 0, so the plan is given the ledger directly.
#[test]
fn extreme_ledger_times_never_panic() {
    let f = case_a();
    let read = |p: &Path| f.read(p);
    let extremes = [i64::MIN, i64::MIN + 1, -1, 0, 1, T, i64::MAX - 1, i64::MAX];
    for swapped_at in extremes.iter().map(|v| Some(*v)).chain([None]) {
        for first_seen in extremes {
            for min_gap in [i64::MIN, 0, 1, GAP, i64::MAX] {
                let ledger = RotateLedger {
                    live: Some(L.into()),
                    swapped_at,
                    hand_suspect: Some(HandSuspect {
                        profile: M.into(),
                        first_seen,
                    }),
                    home_unreadable_since: Some(first_seen),
                    ..RotateLedger::default()
                };
                let input = Input {
                    store: &f.store,
                    rs: &f.rs,
                    home: &f.home,
                    ledger: &ledger,
                    read: &read,
                    min_gap,
                    t: T,
                };
                let (p, _) = plan(&input, seam);
                // A move needs a suspicion first seen within the window.
                if let Plan::Move(_) = p {
                    assert!(
                        (swapped_at.unwrap_or(i64::MIN)..=T).contains(&first_seen)
                            && T.saturating_sub(first_seen) >= min_gap.max(1),
                        "{swapped_at:?} {first_seen} {min_gap}"
                    );
                }
            }
        }
    }
    assert!(f.backed_up().is_empty(), "the plan moves nothing");
}

#[test]
fn no_live_profile_or_no_oauth_login_means_no_read() {
    let f = case_a();
    f.set_ledger(|l| l.live = None);
    f.step(T);
    assert_eq!(f.reads.get(), 0);
    let f = case_a();
    private(&f.home_login(), "{}");
    f.step(T);
    assert_eq!(f.reads.get(), 0);
    let f = case_a();
    fs::remove_file(f.home_login()).unwrap();
    f.step(T);
    assert_eq!(f.reads.get(), 0);
}

/// D8: `.claude.json` takes no part. Case A of #1693 (M has no login) and
/// the #1845 relogin no longer relabel the ledger.
#[test]
fn claude_json_alone_never_relabels() {
    let f = fx(&[K], L, Some(T - 3600), M);
    f.answer("~/.claude", Ok(a_m()));
    f.answer(K, Ok(a_k()));
    let (inodes, before) = (f.inodes(), f.ledger());
    assert_eq!(f.step(T), Step::Clear);
    assert_eq!(f.step(T + GAP), Step::Clear);
    assert_nothing_moved(&f, &inodes);
    assert_eq!(f.ledger().live, before.live);
    no_recovered(&f.logged());
}

// --- (e) no token anywhere ------------------------------------------------------------

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            files_under(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[test]
fn fake_tokens_never_leave_the_login_files() {
    let f = fx(&[L, M, K], L, Some(T - 3600), M);
    let body = |uuid: &str| Reply::json(200, &profile_body(uuid));
    let fake = fake_by_token(HashMap::from([
        (f.tokens["~/.claude"].clone(), body("u-mike")),
        (f.tokens[L].clone(), body("u-lima")),
        (f.tokens[M].clone(), body("u-mike")),
        (f.tokens[K].clone(), body("u-kilo")),
    ]));
    let ep = endpoints(&fake);
    let read = |p: &Path| read_with(&ep, p);
    let mut lines = Vec::new();
    for t in [T, T + GAP] {
        let mut out = Sink::new(open_or_warn(&f.rs.log, &f.rs.log1), false, true);
        lines.extend(preview(&f.store, &f.rs, &f.home, seam, &read, GAP, t).unwrap());
        step_with(&f.store, &f.rs, &f.home, &mut out, seam, &read, GAP, t).unwrap();
    }
    assert_eq!(
        f.ledger().live.as_deref(),
        Some(M),
        "recovered through real reads"
    );
    lines.extend(super::control::status_alerts(&f.ledger()));
    let mut files = Vec::new();
    files_under(&f.base, &mut files);
    for (place, token) in &f.tokens {
        for l in &lines {
            assert!(!l.contains(token.as_str()), "{place}'s token in {l}");
        }
        for p in &files {
            if p.file_name().is_some_and(|n| n == CREDS) {
                continue;
            }
            let text = String::from_utf8_lossy(&fs::read(p).unwrap_or_default()).into_owned();
            assert!(!text.contains("fake-at-1886"), "a token in {}", p.display());
        }
    }
}

// --- (f) the dry run agrees ------------------------------------------------------------

#[track_caller]
fn assert_preview_writes_nothing(f: &Fx, t: i64) -> Vec<String> {
    let before = fs::read(&f.rs.ledger).unwrap();
    let (inodes, log) = (f.inodes(), f.logged());
    let lines = f.preview(t);
    assert_eq!(
        fs::read(&f.rs.ledger).unwrap(),
        before,
        "preview wrote the ledger"
    );
    assert_eq!(f.inodes(), inodes);
    assert_eq!(f.logged(), log);
    lines
}

#[test]
fn dry_run_agrees_for_two_readings() {
    let f = case_a();
    assert_eq!(
        assert_preview_writes_nothing(&f, T),
        vec![format!(
            "would note: {}",
            msg::note_suspect(&n(M), &n(L))
                .strip_prefix("note: ")
                .unwrap()
        )]
    );
    // A dry run alone never confirms: the second preview says the same.
    assert_eq!(f.preview(T + GAP), f.preview(T));
    f.step(T);
    let would = assert_preview_writes_nothing(&f, T + GAP);
    assert_eq!(would.len(), 1, "{would:?}");
    assert_eq!(f.step(T + GAP), Step::Recovered);
    let done = f.logged().last().unwrap().clone();
    assert_eq!(
        would[0],
        format!(
            "would recover: {}",
            done.strip_prefix("recovered: ").unwrap()
        )
    );
}

#[test]
fn dry_run_agrees_when_it_cannot_tell() {
    let f = case_a();
    f.answer("~/.claude", Err(reason::HTTP_429));
    let would = assert_preview_writes_nothing(&f, T);
    f.step(T);
    let note = f.logged()[0].clone();
    assert_eq!(
        would,
        vec![format!(
            "would note: {}",
            note.strip_prefix("note: ").unwrap()
        )]
    );
}

#[test]
fn dry_run_agrees_for_the_only_login_alert() {
    let f = fx(&[M], L, Some(T - 3600), M);
    f.answer("~/.claude", Ok(a_m()));
    f.answer(M, Ok(a_m()));
    f.step(T);
    let would = assert_preview_writes_nothing(&f, T + GAP);
    let alert = msg::only_login(&n(M), &n(L));
    assert_eq!(
        would[0],
        format!("would alert: {}", alert.strip_prefix("ALERT: ").unwrap())
    );
    assert!(would[1].starts_with("fix 1: "), "{would:?}");
    f.step(T + GAP);
    let log = f.logged();
    assert_eq!(&would[1..], &log[log.len() - (would.len() - 1)..]);
}

#[test]
fn dry_run_agrees_within_the_grace_period() {
    let f = fx(&[L, M], L, Some(T), M);
    f.answer("~/.claude", Ok(a_m()));
    f.answer(M, Ok(a_m()));
    assert!(assert_preview_writes_nothing(&f, T + 10).is_empty());
    assert_eq!(f.reads.get(), 0);
}

// --- Through the real cycle --------------------------------------------------------------

thread_local! {
    static READINGS: RefCell<HashMap<PathBuf, Answer>> = RefCell::default();
}

#[cfg(target_os = "linux")]
fn by_path(p: &Path) -> Answer {
    let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    READINGS.with(|r| r.borrow().get(&p).cloned().unwrap_or(Err(reason::NO_FILE)))
}

#[cfg(target_os = "linux")]
fn set_reading(p: PathBuf, a: Answer) {
    READINGS.with(|r| r.borrow_mut().insert(p, a));
}

#[cfg(target_os = "linux")]
fn ctx(f: &Fx, measure: super::cycle::MeasureFn) -> Ctx {
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut ctx = Ctx::new(f.store.clone(), f.home.clone(), s);
    ctx.scan = |_| Liveness::Known(BTreeSet::new());
    ctx.measure = Some(measure);
    ctx.identity = by_path;
    ctx
}

#[cfg(target_os = "linux")]
fn never(_: &Ctx, _: &Snap) -> Result<Measurement, ProfileError> {
    panic!("this cycle must end before measuring")
}

#[cfg(target_os = "linux")]
fn stay_on_h(_: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    assert_eq!(snap.live, LiveId::Known(n(H)), "the ledger's live profile");
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

#[cfg(target_os = "linux")]
#[test]
fn issue_1886_run_cycle_stays_on_the_swapped_in_login() {
    let f = fx(&[L, K], H, Some(now() - 3600), L);
    set_reading(f.home.join(".claude"), Ok(a_h()));
    set_reading(f.root.join(L), Ok(a_l()));
    set_reading(f.root.join(K), Ok(a_k()));
    let inodes = f.inodes();
    let mut c = ctx(&f, stay_on_h);
    assert_eq!(
        run_cycle(&mut c, Mode::OneShot).unwrap(),
        CycleOutcome::Stayed
    );
    assert_nothing_moved(&f, &inodes);
    no_recovered(&f.logged());
    assert!(
        !f.logged()
            .iter()
            .any(|l| l.starts_with("ALERT:") && !l.starts_with("ALERT: needs-login ")),
        "{:?}",
        f.logged()
    );
    assert_eq!(f.ledger().live.as_deref(), Some(H));
}

/// An open suspicion: the cycle writes the note, then the hand-change ALERT,
/// measures nothing and ends `alerted`.
#[cfg(target_os = "linux")]
#[test]
fn an_open_suspicion_stops_the_cycle_before_measuring() {
    let f = fx(&[L, M], L, Some(now() - 3600), M);
    set_reading(f.home.join(".claude"), Ok(a_m()));
    set_reading(f.root.join(L), Ok(a_l()));
    set_reading(f.root.join(M), Ok(a_m()));
    let inodes = f.inodes();
    let mut c = ctx(&f, never);
    assert_eq!(
        run_cycle(&mut c, Mode::OneShot).unwrap(),
        CycleOutcome::Alerted
    );
    assert_nothing_moved(&f, &inodes);
    let log: Vec<String> = f
        .logged()
        .into_iter()
        .filter(|l| !l.starts_with("skip: ") && !l.starts_with("ALERT: needs-login "))
        .collect();
    assert_eq!(
        log,
        vec![msg::note_suspect(&n(M), &n(L)), log_msg::hand_change(&n(L))]
    );
    assert!(f.ledger().hand_suspect.is_some());
}
