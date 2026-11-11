//! `profile verify [--fix]` (issue #1886, owner decision point 3).
//!
//! Contract (`verify_cmd`):
//! - `run_with(&ProfileStore, home, fix, read, t, out) -> Result<Summary,
//!   ProfileError>` prints one line per place, `~/.claude` first, then
//!   each profile in name order: `<place> -> <account>: <status>`, where
//!   the account is the e-mail the server reports (else its uuid) and the
//!   status is `ok`, `MISMATCH, recorded for <names>|no profile`, `no
//!   record, ...` or `no live profile, ...`; or `<place> -> no login`, `->
//!   not an OAuth login`, `-> not read (<reason>)`.
//! - `--fix` moves a profile's own login into the folder of the one other
//!   profile whose record names its account, only when that folder holds
//!   no login file, is not the live profile, and no other move goes there.
//!   It never copies, never replaces, never moves `~/.claude`, writes the
//!   `moves` list (0600, in a fresh 0700 `.rejected-logins/verify-<time>/`)
//!   before the first move, reads the source again just before moving it,
//!   and refuses while the rotate lock is busy or a swap journal exists.
//!
//! Every reader is a fake by path, except the last test, which reads fake
//! `fake-at-1886-<uuid>` tokens through `read_with` and the loopback fake.

use super::ProfileError;
use super::auto_switch::kit::State;
use super::expected::record;
use super::name::ProfileName;
use super::rotate::cred_identity::read_with;
use super::rotate::identify::Account;
use super::rotate::ledger::{RotateLedger, RotateState};
use super::store::ProfileStore;
use super::tests_fake_http::{FakeHttp, Reply};
use super::verify_cmd::{Summary, run_with};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const CREDS: &str = ".credentials.json";
/// 2026-11-03T12:00:00Z.
const T: i64 = 1_793_707_200;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn private(p: &Path, body: &str) {
    fs::write(p, body).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

fn dir700(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

fn server(uuid: &str) -> Account {
    Account::from_profile(&serde_json::json!({
        "account": {"uuid": uuid, "email": format!("{uuid}@mistt.com")},
        "organization": {"uuid": "org-mistt"}
    }))
    .unwrap()
}

fn login_doc(token: &str) -> String {
    let at = chrono::Utc::now().timestamp_millis() + 3_600_000;
    serde_json::json!({"claudeAiOauth": {"accessToken": token, "expiresAt": at}}).to_string()
}

struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    store: ProfileStore,
    answers: RefCell<HashMap<PathBuf, Result<Account, &'static str>>>,
    /// After this many reads, `changed` answers replace the first ones.
    flip_after: Cell<Option<usize>>,
    changed: RefCell<HashMap<PathBuf, Result<Account, &'static str>>>,
    reads: Cell<usize>,
}

/// Profiles `names`, each recorded for `u-<name>`; `live` is the ledger's
/// live profile. `~/.claude` holds a login; nothing else does yet.
fn fx(names: &[&str], live: Option<&str>) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (home, root) = (base.join("home"), base.join("root"));
    let store = ProfileStore::new(root.clone());
    for p in names {
        store.add(&n(p)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let state = State::new(&root).unwrap();
    for p in names {
        record(&state, &n(p), &server(&format!("u-{p}"))).unwrap();
    }
    dir700(&home);
    dir700(&home.join(".claude"));
    private(
        &home.join(".claude").join(CREDS),
        &login_doc("fake-at-home"),
    );
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    rs.save_ledger(&RotateLedger {
        live: live.map(String::from),
        ..RotateLedger::default()
    })
    .unwrap();
    Fx {
        _tmp: tmp,
        home,
        root,
        store,
        answers: RefCell::default(),
        flip_after: Cell::new(None),
        changed: RefCell::default(),
        reads: Cell::new(0),
    }
}

impl Fx {
    fn place(&self, p: &str) -> PathBuf {
        if p == "~/.claude" {
            self.home.join(".claude")
        } else {
            self.root.join(p)
        }
    }

    /// Gives `p` a login file the server reports as `uuid`.
    fn holds(&self, p: &str, uuid: &str) {
        private(
            &self.place(p).join(CREDS),
            &login_doc(&format!("fake-at-{p}")),
        );
        self.answers
            .borrow_mut()
            .insert(self.place(p), Ok(server(uuid)));
    }

    fn ino(&self, p: &str) -> Option<u64> {
        fs::symlink_metadata(self.place(p).join(CREDS))
            .ok()
            .map(|m| m.ino())
    }

    fn read(&self, p: &Path) -> Result<Account, &'static str> {
        self.reads.set(self.reads.get() + 1);
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if self.flip_after.get().is_some_and(|k| self.reads.get() > k)
            && let Some(a) = self.changed.borrow().get(&p)
        {
            return a.clone();
        }
        self.answers
            .borrow()
            .get(&p)
            .cloned()
            .unwrap_or(Err("no answer"))
    }

    fn run(&self, fix: bool) -> (Result<Summary, ProfileError>, Vec<String>) {
        let mut lines = Vec::new();
        let read = |p: &Path| self.read(p);
        let r = run_with(&self.store, &self.home, fix, &read, T, &mut |l| {
            lines.push(l.to_string())
        });
        (r, lines)
    }

    fn backups(&self) -> Vec<PathBuf> {
        let Ok(rd) = fs::read_dir(self.root.join(".rejected-logins")) else {
            return Vec::new();
        };
        rd.map(|e| e.unwrap().path()).collect()
    }
}

/// The 3 November aftermath: `nnorth`'s folder holds mmorrison's account,
/// `mmorrison`'s folder is empty, the ledger names ffarmer.
fn crossed() -> Fx {
    let f = fx(&["ffarmer", "mmorrison", "nnorth"], Some("ffarmer"));
    f.holds("~/.claude", "u-ffarmer");
    f.holds("nnorth", "u-mmorrison");
    f
}

#[test]
fn the_report_names_each_folders_server_account_against_its_record() {
    let f = fx(&["ffarmer", "kk", "mmorrison", "nnorth"], Some("ffarmer"));
    f.holds("~/.claude", "u-ffarmer");
    f.holds("nnorth", "u-mmorrison");
    f.holds("mmorrison", "u-mmorrison");
    private(&f.place("kk").join(CREDS), &login_doc("fake-at-kk"));
    let (r, lines) = f.run(false);
    assert_eq!(
        r.unwrap(),
        Summary {
            mismatches: 1,
            moved: 0
        }
    );
    assert_eq!(
        lines,
        vec![
            "~/.claude (live ffarmer) -> u-ffarmer@mistt.com: ok",
            "ffarmer -> no login",
            "kk -> not read (no answer)",
            "mmorrison -> u-mmorrison@mistt.com: ok",
            "nnorth -> u-mmorrison@mistt.com: MISMATCH, recorded for mmorrison",
        ]
    );
}

#[test]
fn accounts_without_records_and_home_without_a_live_profile_are_reported() {
    let f = fx(&["aa"], None);
    f.holds("~/.claude", "u-zz");
    f.holds("aa", "u-aa");
    fs::remove_file(f.root.join(".auto-switch.d/expected/aa.json")).unwrap();
    let (r, lines) = f.run(false);
    assert_eq!(r.unwrap().mismatches, 0);
    assert_eq!(
        lines,
        vec![
            "~/.claude -> u-zz@mistt.com: no live profile, recorded for no profile",
            "aa -> u-aa@mistt.com: no record, recorded for no profile",
        ]
    );
}

#[test]
fn without_fix_nothing_moves() {
    let f = crossed();
    let before = f.ino("nnorth");
    let (r, _) = f.run(false);
    assert_eq!(
        r.unwrap(),
        Summary {
            mismatches: 1,
            moved: 0
        }
    );
    assert_eq!(f.ino("nnorth"), before);
    assert_eq!(f.ino("mmorrison"), None);
    assert!(f.backups().is_empty());
}

#[test]
fn fix_moves_a_crossed_login_into_its_empty_recorded_folder() {
    let f = crossed();
    let ino = f.ino("nnorth").unwrap();
    let (r, lines) = f.run(true);
    assert_eq!(
        r.unwrap(),
        Summary {
            mismatches: 1,
            moved: 1
        }
    );
    assert_eq!(f.ino("mmorrison"), Some(ino), "moved, not copied");
    assert_eq!(f.ino("nnorth"), None);
    assert!(
        lines.contains(&"moved nnorth's login to mmorrison".to_string()),
        "{lines:?}"
    );
    // The backup is the list of moves, written before the move, private.
    let dirs = f.backups();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    assert!(
        dirs[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("verify-20261103T120000Z")
    );
    let mode = |p: &Path| fs::symlink_metadata(p).unwrap().mode() & 0o777;
    assert_eq!(mode(&dirs[0]), 0o700);
    let list = dirs[0].join("moves");
    assert_eq!(mode(&list), 0o600);
    let text = fs::read_to_string(&list).unwrap();
    let undo = format!(
        "mv -n '{}' '{}'",
        f.place("mmorrison").join(CREDS).display(),
        f.place("nnorth").join(CREDS).display()
    );
    assert!(
        text.contains("# nnorth -> mmorrison") && text.contains(&undo),
        "{text}"
    );
    assert!(!text.contains("fake-at"), "no token in the backup list");
    // Exactly one file anywhere holds the moved token.
    let holders: Vec<_> = walk(&f.root)
        .into_iter()
        .filter(|p| fs::read_to_string(p).is_ok_and(|t| t.contains("fake-at-nnorth")))
        .collect();
    assert_eq!(holders, vec![f.place("mmorrison").join(CREDS)]);
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn fix_refuses_a_target_that_holds_a_login() {
    for (target, why) in [
        ("u-other", "mmorrison holds a different account"),
        ("u-mmorrison", "mmorrison already holds that account"),
    ] {
        let f = crossed();
        f.holds("mmorrison", target);
        let (inos, n_ino) = (f.ino("mmorrison"), f.ino("nnorth"));
        let (r, lines) = f.run(true);
        assert_eq!(r.unwrap().moved, 0);
        assert!(
            lines.contains(&format!("nnorth: not moved: {why}")),
            "{lines:?}"
        );
        assert_eq!((f.ino("mmorrison"), f.ino("nnorth")), (inos, n_ino));
        assert!(f.backups().is_empty(), "no backup when nothing moves");
    }
}

#[test]
fn fix_never_moves_into_the_live_profile_or_out_of_home() {
    let f = fx(&["ffarmer", "nnorth"], Some("ffarmer"));
    f.holds("~/.claude", "u-nnorth");
    f.holds("nnorth", "u-ffarmer");
    let (h, n_ino) = (f.ino("~/.claude"), f.ino("nnorth"));
    let (r, lines) = f.run(true);
    assert_eq!(
        r.unwrap(),
        Summary {
            mismatches: 2,
            moved: 0
        }
    );
    assert!(
        lines
            .contains(&"nnorth: not moved: ffarmer is live; its login is in ~/.claude".to_string()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"~/.claude: not moved by --fix; use amplihack profile switch".to_string())
    );
    assert_eq!((f.ino("~/.claude"), f.ino("nnorth")), (h, n_ino));
}

#[test]
fn fix_refuses_an_account_recorded_for_no_profile_or_several() {
    let f = fx(&["aa", "bb", "cc"], None);
    f.holds("aa", "u-zz");
    let (_, lines) = f.run(true);
    assert!(
        lines.contains(&"aa: not moved: its account is recorded for no profile".to_string()),
        "{lines:?}"
    );
    let state = State::new(&f.root).unwrap();
    record(&state, &n("bb"), &server("u-zz")).unwrap();
    record(&state, &n("cc"), &server("u-zz")).unwrap();
    let (_, lines) = f.run(true);
    assert!(
        lines.contains(
            &"aa: not moved: its account is recorded for several profiles (bb, cc)".to_string()
        ),
        "{lines:?}"
    );
    assert!(f.ino("aa").is_some());
}

#[test]
fn two_logins_for_one_empty_folder_move_only_the_first() {
    let f = fx(&["aa", "bb", "cc"], None);
    f.holds("aa", "u-cc");
    f.holds("bb", "u-cc");
    let (r, lines) = f.run(true);
    assert_eq!(r.unwrap().moved, 1);
    assert!(
        lines.contains(&"moved aa's login to cc".to_string()),
        "{lines:?}"
    );
    assert!(lines.contains(&"bb: not moved: another login is moving to cc".to_string()));
    assert!(f.ino("bb").is_some());
}

#[test]
fn a_login_that_changes_before_the_move_stays() {
    let f = crossed();
    // Survey: ~/.claude, ffarmer (no login: not read), mmorrison (none),
    // nnorth: two reads. The re-read before the move sees another account.
    f.flip_after.set(Some(2));
    f.changed
        .borrow_mut()
        .insert(f.place("nnorth"), Ok(server("u-else")));
    let before = f.ino("nnorth");
    let (r, lines) = f.run(true);
    assert_eq!(r.unwrap().moved, 0);
    assert!(
        lines.contains(&"nnorth: not moved: its login changed since it was read".to_string()),
        "{lines:?}"
    );
    assert_eq!(f.ino("nnorth"), before);
    assert_eq!(f.ino("mmorrison"), None);
}

#[test]
fn fix_refuses_during_a_swap_and_moves_nothing() {
    let f = crossed();
    let rs = RotateState::new(&f.root);
    private(&rs.journal, "{}");
    let before = f.ino("nnorth");
    let (r, lines) = f.run(true);
    assert!(matches!(r, Err(ProfileError::Verify(_))), "{r:?}");
    assert!(lines.is_empty());
    assert_eq!(f.ino("nnorth"), before);
}

#[test]
fn fix_refuses_while_the_rotate_lock_is_held() {
    let f = crossed();
    let state = State::new(&f.root).unwrap();
    let _held = state.lock().unwrap();
    let before = f.ino("nnorth");
    // `flock` locks of two open files conflict within one process too.
    let (r, _) = f.run(true);
    assert!(matches!(r, Err(ProfileError::Verify(_))), "{r:?}");
    assert_eq!(f.ino("nnorth"), before);
}

/// The real reader against the loopback fake: one profile request per
/// login, no usage request, no refresh, and no token in any line.
#[test]
fn verify_reads_the_profile_endpoint_and_prints_no_token() {
    let f = fx(&["mmorrison", "nnorth"], None);
    private(
        &f.place("~/.claude").join(CREDS),
        &login_doc("fake-at-1886-home"),
    );
    private(
        &f.place("nnorth").join(CREDS),
        &login_doc("fake-at-1886-nnorth"),
    );
    let fake = FakeHttp::start(|seen, _| {
        let uuid = match seen.header("authorization") {
            Some("Bearer fake-at-1886-home") => "u-ffarmer",
            Some("Bearer fake-at-1886-nnorth") => "u-mmorrison",
            _ => return Reply::json(401, &serde_json::json!({})),
        };
        Reply::json(
            200,
            &serde_json::json!({"account": {"uuid": uuid, "email": format!("{uuid}@mistt.com")}, "organization": {"uuid": "org-mistt"}}),
        )
    });
    let (base, token) = (fake.base(), fake.token_url());
    let (ep, _) =
        super::usage_direct::Endpoints::resolve(Some(OsStr::new(&base)), Some(OsStr::new(&token)));
    let read = |p: &Path| read_with(&ep, p);
    let mut lines = Vec::new();
    let s = run_with(&f.store, &f.home, false, &read, T, &mut |l| {
        lines.push(l.to_string())
    })
    .unwrap();
    assert_eq!(s.mismatches, 1);
    assert!(
        lines.contains(
            &"nnorth -> u-mmorrison@mistt.com: MISMATCH, recorded for mmorrison".to_string()
        ),
        "{lines:?}"
    );
    assert_eq!(fake.count("GET", "/api/oauth/profile"), 2);
    assert_eq!(fake.count("GET", "/api/oauth/usage"), 0);
    assert_eq!(fake.count("POST", "/v1/oauth/token"), 0);
    assert!(lines.iter().all(|l| !l.contains("fake-at")), "{lines:?}");
}

// --- Issue #2218 (D7, C4): an excluded profile is never read ------------------

mod issue_2218 {
    use super::*;
    use crate::commands::profile::auto_switch::config::{self, ReadMode};

    fn exclude(f: &Fx, names: &[&str]) {
        config::update(&f.root, ReadMode::Editable, |c| {
            for p in names {
                c.exclude.push(n(p));
            }
            Ok(())
        })
        .unwrap();
    }

    /// `~/.claude.json` naming `uuid` in the fixture's organization.
    fn home_json(f: &Fx, uuid: &str) {
        private(
            &f.home.join(".claude.json"),
            &serde_json::json!({"oauthAccount": {"accountUuid": uuid,
                "emailAddress": format!("{uuid}@mistt.com"),
                "organizationUuid": "org-mistt"}})
            .to_string(),
        );
    }

    /// Runs verify with a reader that records every path it is asked for.
    fn run_counting(f: &Fx, fix: bool) -> (Vec<PathBuf>, Vec<String>) {
        let asked = RefCell::new(Vec::new());
        let read = |p: &Path| {
            asked
                .borrow_mut()
                .push(p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
            f.read(p)
        };
        let mut lines = Vec::new();
        let _ = run_with(&f.store, &f.home, fix, &read, T, &mut |l| {
            lines.push(l.to_string())
        });
        (asked.into_inner(), lines)
    }

    fn fixture() -> Fx {
        let f = fx(&["alpha", "bravo"], Some("bravo"));
        assert!(
            f.root
                .starts_with(std::env::temp_dir().canonicalize().unwrap())
        );
        f.holds("~/.claude", "u-bravo");
        f.holds("alpha", "u-alpha");
        f
    }

    #[test]
    fn an_excluded_folder_is_never_read() {
        let f = fixture();
        exclude(&f, &["alpha"]);
        let before = fs::read(f.place("alpha").join(CREDS)).unwrap();
        let (asked, lines) = run_counting(&f, false);
        assert!(!asked.contains(&f.place("alpha")), "{asked:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("alpha") && l.contains("excluded")),
            "{lines:?}"
        );
        assert_eq!(fs::read(f.place("alpha").join(CREDS)).unwrap(), before);
        assert!(asked.contains(&f.place("~/.claude")), "home still read");
    }

    #[test]
    fn home_is_not_read_when_the_ledger_names_an_excluded_profile_live() {
        let f = fixture();
        exclude(&f, &["bravo"]);
        let (asked, lines) = run_counting(&f, false);
        assert!(!asked.contains(&f.place("~/.claude")), "{asked:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("~/.claude") && l.contains("excluded")),
            "{lines:?}"
        );
    }

    #[test]
    fn home_is_not_read_when_a_hand_change_is_suspected_and_claude_json_names_an_excluded_profile()
    {
        // A hand swap: the ledger still names bravo but suspects a hand
        // change, and ~/.claude.json now matches alpha's record (rule 2).
        let f = fixture();
        f.holds("~/.claude", "u-alpha");
        home_json(&f, "u-alpha");
        suspect_hand_change(&f, "alpha");
        exclude(&f, &["alpha"]);
        let before = fs::read(f.place("~/.claude").join(CREDS)).unwrap();
        let (asked, lines) = run_counting(&f, false);
        assert!(
            asked.is_empty() || !asked.contains(&f.place("~/.claude")),
            "{asked:?}"
        );
        assert!(!asked.contains(&f.place("alpha")), "{asked:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("~/.claude") && l.contains("excluded")),
            "{lines:?}"
        );
        assert_eq!(fs::read(f.place("~/.claude").join(CREDS)).unwrap(), before);
    }

    #[test]
    fn home_is_read_when_claude_json_names_a_profile_that_is_not_excluded() {
        let f = fixture();
        home_json(&f, "u-bravo");
        exclude(&f, &["alpha"]);
        let (asked, _) = run_counting(&f, false);
        assert!(asked.contains(&f.place("~/.claude")), "{asked:?}");
    }

    #[test]
    fn an_unsafe_claude_json_does_not_mark_home_excluded() {
        let f = fixture();
        home_json(&f, "u-alpha");
        fs::set_permissions(
            f.home.join(".claude.json"),
            fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        exclude(&f, &["alpha"]);
        let (asked, _) = run_counting(&f, false);
        assert!(
            asked.contains(&f.place("~/.claude")),
            "an unsafe file is missing, so home is read: {asked:?}"
        );
    }

    #[test]
    fn fix_neither_moves_out_of_nor_into_an_excluded_folder() {
        // alpha's folder holds bravo's account, and charlie's folder is
        // empty while holding... charlie's record: no move involves alpha.
        let f = fx(&["alpha", "bravo", "charlie"], Some("charlie"));
        f.holds("~/.claude", "u-charlie");
        f.holds("alpha", "u-bravo");
        exclude(&f, &["alpha"]);
        let ino = f.ino("alpha");
        let (asked, _) = run_counting(&f, true);
        assert!(!asked.contains(&f.place("alpha")), "{asked:?}");
        assert_eq!(f.ino("alpha"), ino, "alpha's login stays");
        assert!(f.ino("bravo").is_none(), "nothing moved into bravo");
        assert!(f.backups().is_empty(), "no move list written");
    }

    /// The ledger keeps `live`, and records a suspected hand change to `p`.
    fn suspect_hand_change(f: &Fx, p: &str) {
        let rs = RotateState::new(&f.root);
        let l = rs.load_ledger().unwrap().unwrap_or_default();
        let mut v = serde_json::to_value(&l).unwrap();
        v["hand_suspect"] = serde_json::json!({"profile": p, "first_seen": 1_760_000_000});
        let l: RotateLedger = serde_json::from_value(v).unwrap();
        rs.save_ledger(&l).unwrap();
    }

    /// Drops the ledger's `live` field.
    fn no_ledger_live(f: &Fx) {
        let rs = RotateState::new(&f.root);
        rs.save_ledger(&RotateLedger::default()).unwrap();
    }

    #[test]
    fn the_excluded_lines_use_the_usage_cell_text() {
        let f = fixture();
        exclude(&f, &["alpha", "bravo"]);
        let (_, lines) = run_counting(&f, false);
        assert!(
            lines.iter().any(|l| l == "alpha: excluded (not probed)"),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l == "~/.claude: excluded (not probed), live profile bravo"),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("not checked")),
            "{lines:?}"
        );
    }

    #[test]
    fn home_is_not_read_when_no_ledger_live_and_claude_json_names_an_excluded_profile() {
        let f = fixture();
        no_ledger_live(&f);
        home_json(&f, "u-alpha");
        exclude(&f, &["alpha"]);
        let (asked, lines) = run_counting(&f, false);
        assert!(!asked.contains(&f.place("~/.claude")), "{asked:?}");
        assert!(
            lines
                .iter()
                .any(|l| l == "~/.claude: excluded (not probed), live profile alpha"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_ledger_naming_a_profile_that_is_not_excluded_with_no_hand_change_reads_home() {
        // The documented limit of the local proof: only ~/.claude.json points
        // at excluded alpha, the ledger names bravo and no hand change is
        // suspected, so the login is taken to be bravo's.
        let f = fixture();
        home_json(&f, "u-alpha");
        exclude(&f, &["alpha"]);
        let (asked, _) = run_counting(&f, false);
        assert!(asked.contains(&f.place("~/.claude")), "{asked:?}");
    }

    #[test]
    fn excluded_rows_alone_do_not_fail_verify() {
        let f = fixture();
        exclude(&f, &["alpha"]);
        let read = |p: &Path| f.read(p);
        let got = run_with(&f.store, &f.home, false, &read, T, &mut |_| {});
        assert_eq!(got.unwrap().mismatches, 0, "the user chose to exclude it");
    }
}
