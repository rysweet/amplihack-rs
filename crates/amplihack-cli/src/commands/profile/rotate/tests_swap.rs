//! `rotate::swap`: the journalled exchange and its recovery (D9, D10, D12,
//! issue #1592). Every assertion about a login is by inode; the login
//! fixtures are mode 0000 (0600 under root) so any attempt to read one fails.
//!
//! Recovery tests are in `tests_recover.rs` and share this fixture.
//!
//! Contract:
//! - `Source::{Profile(ProfileName), Adopt}`.
//! - `swap(&ProfileStore, &RotateState, home: &Path, from: Source, to:
//!   &ProfileName, now: i64) -> Result<ProfileName, ProfileError>` returns the
//!   profile the outgoing login went to.
//! - `recover(&ProfileStore, &RotateState, home: &Path) ->
//!   Result<RecoverOutcome, ProfileError>`, with `RecoverOutcome::{None,
//!   RolledBack { from, to }, RolledForward { from, to }, Inconsistent(String)}`.

use super::ledger::{Phase, RotateJournal, RotateState};
use super::swap::{RecoverOutcome, Source, recover, swap};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::euid;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(super) const CREDS: &str = ".credentials.json";
pub(super) const LOCK: &str = ".oauth_refresh.lock";
pub(super) const NOW: i64 = 1_760_000_000;

pub(super) fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

pub(super) fn login(path: &Path) -> u64 {
    if path.exists() {
        fs::remove_file(path).unwrap();
    }
    private(path, "SENTINEL");
    if euid() != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    }
    ino(path)
}

pub(super) fn ino(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().ino()
}

pub(super) fn ino_at(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path).ok().map(|m| m.ino())
}

pub(super) fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

pub(super) fn json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn acct(uuid: &str, email: &str) -> serde_json::Value {
    serde_json::json!({"accountUuid": uuid, "emailAddress": email})
}

/// `~/.claude` holds `work`'s login (`l`); `spare` has its own (`t`).
pub(super) struct Env {
    pub(super) _tmp: tempfile::TempDir,
    pub(super) home: PathBuf,
    pub(super) root: PathBuf,
    pub(super) store: ProfileStore,
    pub(super) rs: RotateState,
    pub(super) l: u64,
    pub(super) t: u64,
}

pub(super) fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("root");
    let store = ProfileStore::new(root.clone());
    for p in ["work", "spare"] {
        store.add(&n(p)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    private(
        &root.join("work/.claude.json"),
        &serde_json::json!({"oauthAccount": acct("u-work", "work@example.com")}).to_string(),
    );
    private(
        &root.join("spare/.claude.json"),
        &serde_json::json!({
            "oauthAccount": acct("u-spare", "spare@example.com"),
            "primaryApiKey": "SECRET-KEY-SPARE",
            "numStartups": 3
        })
        .to_string(),
    );
    private(
        &home.join(".claude.json"),
        &serde_json::json!({
            "oauthAccount": acct("u-work", "work@example.com"),
            "primaryApiKey": "SECRET-KEY-HOME",
            "projects": {"/x": {"allowedTools": []}}
        })
        .to_string(),
    );
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::set_permissions(home.join(".claude"), fs::Permissions::from_mode(0o700)).unwrap();
    let l = login(&home.join(".claude").join(CREDS));
    let t = login(&root.join("spare").join(CREDS));
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    Env {
        _tmp: tmp,
        home,
        root,
        store,
        rs,
        l,
        t,
    }
}

impl Env {
    pub(super) fn live(&self) -> PathBuf {
        self.home.join(".claude").join(CREDS)
    }

    pub(super) fn slot(&self, p: &str) -> PathBuf {
        self.root.join(p).join(CREDS)
    }

    pub(super) fn swap(&self) -> Result<ProfileName, ProfileError> {
        swap(
            &self.store,
            &self.rs,
            &self.home,
            Source::Profile(n("work")),
            &n("spare"),
            NOW,
        )
    }

    pub(super) fn recover(&self) -> Result<RecoverOutcome, ProfileError> {
        recover(&self.store, &self.rs, &self.home)
    }

    pub(super) fn journal(&self, phase: Phase) -> RotateJournal {
        RotateJournal {
            version: 1,
            from: "work".into(),
            to: "spare".into(),
            adopt: false,
            phase,
            home: self.home.to_str().unwrap().into(),
            live_ino: self.l,
            target_ino: self.t,
            home_lock_ino: None,
            to_lock_ino: None,
            started_at: NOW,
            pid: std::process::id(),
        }
    }

    /// Every login file in the tree, by inode.
    pub(super) fn all_logins(&self) -> Vec<u64> {
        let mut v: Vec<u64> = [self.live(), self.slot("work"), self.slot("spare")]
            .iter()
            .filter_map(|p| ino_at(p))
            .collect();
        v.sort();
        v
    }

    pub(super) fn assert_unchanged(&self) {
        assert_eq!(ino_at(&self.live()), Some(self.l));
        assert_eq!(ino_at(&self.slot("spare")), Some(self.t));
        assert_eq!(ino_at(&self.slot("work")), None);
        assert!(!self.rs.journal.exists(), "no journal left");
        assert_eq!(
            json(&self.home.join(".claude.json"))["oauthAccount"]["accountUuid"],
            "u-work"
        );
    }

    /// The state after `s1`: logins exchanged, nothing else done.
    pub(super) fn exchanged(&self) {
        let aside = self.root.join("aside");
        fs::rename(self.live(), &aside).unwrap();
        fs::rename(self.slot("spare"), self.live()).unwrap();
        fs::rename(&aside, self.slot("spare")).unwrap();
    }

    pub(super) fn assert_finished(&self) {
        assert_eq!(ino_at(&self.live()), Some(self.t));
        assert_eq!(ino_at(&self.slot("work")), Some(self.l));
        assert_eq!(ino_at(&self.slot("spare")), None);
        assert!(!self.rs.journal.exists());
        let home = json(&self.home.join(".claude.json"));
        assert_eq!(home["oauthAccount"]["accountUuid"], "u-spare");
        let ledger = self.rs.load_ledger().unwrap().unwrap();
        assert_eq!(ledger.live.as_deref(), Some("spare"));
        assert_eq!(ledger.previous.as_deref(), Some("work"));
    }
}

fn refused<T: std::fmt::Debug>(r: Result<T, ProfileError>) {
    assert!(matches!(r, Err(ProfileError::RotateRefused(_))), "{r:?}");
}

// --- The swap --------------------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_swap_moves_both_logins_and_patches_only_the_account() {
    let e = env();
    let mode_before = fs::metadata(e.home.join(".claude.json")).unwrap().mode();
    assert_eq!(e.swap().unwrap(), n("work"));
    e.assert_finished();
    assert_eq!(e.all_logins().len(), 2, "no copy was made");

    let home = json(&e.home.join(".claude.json"));
    assert_eq!(home["oauthAccount"]["emailAddress"], "spare@example.com");
    assert_eq!(
        home["primaryApiKey"], "SECRET-KEY-HOME",
        "only oauthAccount moves"
    );
    assert!(home["projects"]["/x"].is_object(), "other keys kept");
    assert_eq!(
        fs::metadata(e.home.join(".claude.json")).unwrap().mode(),
        mode_before
    );

    let ledger = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!((ledger.since, ledger.swapped_at), (Some(NOW), Some(NOW)));
    assert_eq!(ledger.account.as_deref(), Some("u-spare"));
    assert_eq!(ledger.alert, None);
    assert!(!e.home.join(".claude").join(LOCK).exists());
    assert!(!e.root.join("spare").join(LOCK).exists());
}

#[cfg(target_os = "linux")]
#[test]
fn swapping_back_returns_each_login_home() {
    let e = env();
    e.swap().unwrap();
    swap(
        &e.store,
        &e.rs,
        &e.home,
        Source::Profile(n("spare")),
        &n("work"),
        NOW + 10,
    )
    .unwrap();
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), None);
    assert_eq!(
        json(&e.home.join(".claude.json"))["oauthAccount"]["accountUuid"],
        "u-work"
    );
}

#[test]
fn a_swap_is_refused_when_the_source_already_has_a_login() {
    let e = env();
    let s = login(&e.slot("work"));
    refused(e.swap());
    assert_eq!(ino_at(&e.slot("work")), Some(s));
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert!(!e.rs.journal.exists());
}

#[test]
fn a_swap_is_refused_for_a_missing_or_hard_linked_target() {
    let e = env();
    fs::hard_link(e.slot("spare"), e.root.join("spare/extra-link")).unwrap();
    refused(e.swap());
    e.assert_unchanged();
    fs::remove_file(e.root.join("spare/extra-link")).unwrap();
    fs::rename(e.slot("spare"), e.root.join("spare/aside")).unwrap();
    refused(e.swap());
    assert_eq!(ino_at(&e.live()), Some(e.l));
}

#[test]
fn a_swap_is_refused_for_an_unsafe_home() {
    let e = env();
    fs::set_permissions(e.home.join(".claude"), fs::Permissions::from_mode(0o777)).unwrap();
    refused(e.swap());
    fs::set_permissions(e.home.join(".claude"), fs::Permissions::from_mode(0o700)).unwrap();
    e.assert_unchanged();

    let link = e.home.with_file_name("home-link");
    std::os::unix::fs::symlink(&e.home, &link).unwrap();
    refused(swap(
        &e.store,
        &e.rs,
        &link,
        Source::Profile(n("work")),
        &n("spare"),
        NOW,
    ));
    refused(swap(
        &e.store,
        &e.rs,
        Path::new("relative/home"),
        Source::Profile(n("work")),
        &n("spare"),
        NOW,
    ));
    e.assert_unchanged();
}

#[test]
fn a_swap_is_refused_for_a_profile_root_others_can_write() {
    let e = env();
    fs::set_permissions(&e.root, fs::Permissions::from_mode(0o777)).unwrap();
    refused(e.swap());
    fs::set_permissions(&e.root, fs::Permissions::from_mode(0o700)).unwrap();
    e.assert_unchanged();
}

#[test]
fn a_swap_is_refused_while_a_journal_is_pending() {
    let e = env();
    e.rs.save_journal(&e.journal(Phase::Inconsistent)).unwrap();
    assert!(e.swap().is_err());
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
}

#[test]
fn a_refresh_lock_held_in_the_target_refuses_the_swap_and_is_left_in_place() {
    let e = env();
    fs::create_dir(e.root.join("spare").join(LOCK)).unwrap();
    let foreign = ino(&e.root.join("spare").join(LOCK));
    refused(e.swap());
    e.assert_unchanged();
    assert_eq!(ino_at(&e.root.join("spare").join(LOCK)), Some(foreign));
    assert!(!e.home.join(".claude").join(LOCK).exists(), "no lock taken");
}

#[test]
fn a_refresh_lock_held_by_the_live_session_refuses_the_swap() {
    // A refresh during the exchange would write the outgoing token over
    // the incoming login, so the swap waits for the next cycle.
    let e = env();
    let lock = e.home.join(".claude").join(LOCK);
    fs::create_dir(&lock).unwrap();
    let foreign = ino(&lock);
    refused(e.swap());
    e.assert_unchanged();
    assert_eq!(
        ino_at(&lock),
        Some(foreign),
        "Claude's lock is never broken"
    );
    assert!(!e.root.join("spare").join(LOCK).exists(), "no lock taken");
}

#[cfg(target_os = "linux")]
#[test]
fn a_refused_adopting_swap_leaves_no_empty_profile() {
    let e = env();
    fs::create_dir(e.home.join(".claude").join(LOCK)).unwrap();
    refused(swap(
        &e.store,
        &e.rs,
        &e.home,
        Source::Adopt,
        &n("spare"),
        NOW,
    ));
    e.assert_unchanged();
    assert!(!e.root.join("default-original").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn a_failed_move_to_the_source_releases_the_locks() {
    let e = env();
    // The source profile cannot be written, so the second rename fails
    // after the exchange.
    fs::set_permissions(e.root.join("work"), fs::Permissions::from_mode(0o500)).unwrap();
    let r = e.swap();
    fs::set_permissions(e.root.join("work"), fs::Permissions::from_mode(0o700)).unwrap();
    if euid() == 0 {
        return; // root ignores the directory's mode, so the move succeeds.
    }
    assert!(
        matches!(r, Err(ProfileError::RotateInconsistent(_))),
        "{r:?}"
    );
    assert!(
        !e.home.join(".claude").join(LOCK).exists(),
        "home lock released"
    );
    assert!(
        !e.root.join("spare").join(LOCK).exists(),
        "target lock released"
    );
    assert_eq!(ino_at(&e.live()), Some(e.t), "the incoming login is live");
    assert!(e.rs.journal.exists(), "recovery finishes the swap later");
}

// --- Adoption --------------------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn adoption_creates_default_original_with_only_the_account() {
    let e = env();
    fs::remove_file(e.root.join("work/.claude.json")).unwrap();
    let from = swap(&e.store, &e.rs, &e.home, Source::Adopt, &n("spare"), NOW).unwrap();
    assert_eq!(from, n("default-original"));
    let dir = e.root.join("default-original");
    assert_eq!(ino_at(&dir.join(CREDS)), Some(e.l), "moved, not copied");
    let cj = json(&dir.join(".claude.json"));
    let keys: Vec<_> = cj.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, vec!["oauthAccount".to_string()]);
    assert_eq!(cj["oauthAccount"]["accountUuid"], "u-work");
    assert_eq!(ino_at(&e.live()), Some(e.t));
}

#[cfg(target_os = "linux")]
#[test]
fn adoption_takes_the_first_free_name() {
    let e = env();
    e.store.add(&n("default-original")).unwrap();
    let from = swap(&e.store, &e.rs, &e.home, Source::Adopt, &n("spare"), NOW).unwrap();
    assert_eq!(from, n("default-original-2"));
    assert!(!e.root.join("default-original").join(CREDS).exists());
}

#[test]
fn adding_a_profile_never_creates_a_login_file() {
    let e = env();
    e.store.add(&n("fresh")).unwrap();
    assert!(!e.root.join("fresh").join(CREDS).exists());
}
