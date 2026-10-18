//! `rotate::identify`: which profile is live in `~/.claude`, and which
//! profiles may receive it (D11, D13, D16, issue #1592). Only `.claude.json`
//! files and metadata are read; the login fixtures are mode 0000.
//!
//! Contract:
//! - `Account::from_claude_json(&[u8]) -> Option<Account>`; `Account::same`
//!   compares uuids when both have one, else e-mails case-insensitively.
//!   Values over 320 bytes or with control characters count as missing.
//! - `live(&ProfileStore, &RotateState, home: &Path) -> Result<(LiveId,
//!   Option<&'static str>), ProfileError>`: the second value is the reason the
//!   prototype ledger was ignored (`profile has a login`, `account does not
//!   match`, `untrusted`, `invalid name`).
//! - `LiveId::{Known(ProfileName), Adopt, HandChanged(ProfileName), NoLogin}`.
//! - `candidates(&ProfileStore, &CandidateInput) -> Result<(Vec<ProfileName>,
//!   Vec<(ProfileName, Skip)>), ProfileError>`; a root that cannot be listed
//!   is an error. `Skip::as_str()` gives the documented reason.

use super::identify::{Account, CandidateInput, LiveId, Skip, candidates, live};
use super::ledger::{RotateLedger, RotateState};
use crate::commands::profile::auto_switch::kit::{Ledger, Liveness};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    store: ProfileStore,
    rs: RotateState,
}

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn login(path: &Path) {
    private(path, "SENTINEL");
    if crate::commands::profile::auto_switch::euid() != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    }
}

fn acct(uuid: &str, email: &str) -> String {
    serde_json::json!({"oauthAccount": {"accountUuid": uuid, "emailAddress": email}}).to_string()
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

/// Profiles `work` (no login, account u-work) and `spare`, `extra` (logged
/// in). `~/.claude` holds u-work's login.
fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("root");
    let store = ProfileStore::new(root.clone());
    for p in ["work", "spare", "extra"] {
        store.add(&n(p)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    private(
        &root.join("work/.claude.json"),
        &acct("u-work", "work@example.com"),
    );
    private(
        &root.join("spare/.claude.json"),
        &acct("u-spare", "spare@example.com"),
    );
    private(
        &root.join("extra/.claude.json"),
        &acct("u-extra", "extra@example.com"),
    );
    login(&root.join("spare/.credentials.json"));
    login(&root.join("extra/.credentials.json"));
    login(&home.join(".claude/.credentials.json"));
    private(
        &home.join(".claude.json"),
        &acct("u-work", "work@example.com"),
    );
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    Env {
        _tmp: tmp,
        home,
        store,
        rs,
    }
}

impl Env {
    fn root(&self) -> &Path {
        self.store.root()
    }

    fn live(&self) -> (LiveId, Option<&'static str>) {
        live(&self.store, &self.rs, &self.home).unwrap()
    }

    fn prototype(&self, body: &str) {
        let dir = self.root().join(".rotator");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        private(&dir.join("ledger.json"), body);
    }

    fn input<'a>(&self, live: Option<&'a ProfileName>) -> CandidateInput<'a> {
        CandidateInput {
            live,
            exclude: Vec::new(),
            liveness: Liveness::Known(BTreeSet::new()),
            auto_switch: Ledger::default(),
        }
    }
}

// --- Accounts --------------------------------------------------------------

#[test]
fn accounts_match_by_uuid_then_email() {
    let a = Account::from_claude_json(acct("u1", "Me@Example.com").as_bytes()).unwrap();
    let same_uuid = Account::from_claude_json(acct("u1", "other@example.com").as_bytes()).unwrap();
    let other_uuid = Account::from_claude_json(acct("u2", "me@example.com").as_bytes()).unwrap();
    let email_only =
        Account::from_claude_json(br#"{"oauthAccount":{"emailAddress":"me@EXAMPLE.com"}}"#)
            .unwrap();
    assert!(a.same(&same_uuid));
    assert!(!a.same(&other_uuid), "uuids decide when both have one");
    assert!(
        a.same(&email_only),
        "e-mail, case-insensitive, when one has no uuid"
    );
}

#[test]
fn missing_or_unsafe_account_values_count_as_missing() {
    assert!(Account::from_claude_json(b"{}").is_none());
    assert!(Account::from_claude_json(b"not json").is_none());
    let long = acct(&"u".repeat(321), "");
    assert!(Account::from_claude_json(long.as_bytes()).is_none());
    let ctl = acct("u\u{1b}1", "");
    assert!(Account::from_claude_json(ctl.as_bytes()).is_none());
}

#[test]
fn account_debug_output_hides_the_values() {
    let a =
        Account::from_claude_json(acct("uuid-secret", "secret@example.com").as_bytes()).unwrap();
    let shown = format!("{a:?}");
    assert!(
        !shown.contains("uuid-secret") && !shown.contains("secret@"),
        "{shown}"
    );
}

// --- Live profile ----------------------------------------------------------

#[test]
fn no_login_in_home_is_no_login() {
    let e = env();
    fs::remove_file(e.home.join(".claude/.credentials.json")).unwrap();
    assert_eq!(e.live().0, LiveId::NoLogin);
    std::os::unix::fs::symlink("/nonexistent", e.home.join(".claude/.credentials.json")).unwrap();
    assert_eq!(e.live().0, LiveId::NoLogin, "a symlink is not a login");
}

#[test]
fn the_rotate_ledger_decides_when_its_account_matches() {
    let e = env();
    e.rs.save_ledger(&RotateLedger {
        live: Some("work".into()),
        account: Some("u-work".into()),
        ..RotateLedger::default()
    })
    .unwrap();
    assert_eq!(e.live(), (LiveId::Known(n("work")), None));
}

#[test]
fn a_ledger_account_that_does_not_match_is_a_hand_change() {
    let e = env();
    e.rs.save_ledger(&RotateLedger {
        live: Some("work".into()),
        account: Some("u-someone-else".into()),
        ..RotateLedger::default()
    })
    .unwrap();
    assert_eq!(e.live().0, LiveId::HandChanged(n("work")));
}

#[test]
fn without_a_ledger_the_single_login_less_match_is_live() {
    let e = env();
    assert_eq!(e.live(), (LiveId::Known(n("work")), None));
}

#[test]
fn without_any_match_the_login_is_adopted() {
    let e = env();
    private(
        &e.home.join(".claude.json"),
        &acct("u-new", "new@example.com"),
    );
    assert_eq!(e.live().0, LiveId::Adopt);
}

#[test]
fn a_matching_profile_that_has_a_login_is_not_live() {
    let e = env();
    private(
        &e.home.join(".claude.json"),
        &acct("u-spare", "spare@example.com"),
    );
    assert_eq!(e.live().0, LiveId::Adopt, "spare still has its own login");
}

#[test]
fn a_trusted_prototype_ledger_names_the_live_profile() {
    let e = env();
    e.prototype(r#"{"live":"work","since":5,"live_hash":"SECRET-HASH"}"#);
    assert_eq!(e.live(), (LiveId::Known(n("work")), None));
}

#[test]
fn a_prototype_ledger_naming_a_logged_in_profile_is_ignored() {
    let e = env();
    e.prototype(r#"{"live":"spare","since":5}"#);
    assert_eq!(
        e.live(),
        (LiveId::Known(n("work")), Some("profile has a login"))
    );
}

#[test]
fn a_prototype_ledger_whose_profile_has_another_account_is_ignored() {
    let e = env();
    fs::remove_file(e.root().join("extra/.credentials.json")).unwrap();
    e.prototype(r#"{"live":"extra","since":5}"#);
    assert_eq!(
        e.live(),
        (LiveId::Known(n("work")), Some("account does not match"))
    );
}

#[test]
fn a_prototype_ledger_whose_profile_has_no_account_is_used() {
    let e = env();
    fs::remove_file(e.root().join("extra/.credentials.json")).unwrap();
    fs::remove_file(e.root().join("extra/.claude.json")).unwrap();
    e.prototype(r#"{"live":"extra","since":5}"#);
    assert_eq!(e.live(), (LiveId::Known(n("extra")), None));
}

#[test]
fn a_prototype_ledger_with_a_bad_name_or_unsafe_mode_is_ignored() {
    let e = env();
    e.prototype(r#"{"live":"../x","since":5}"#);
    assert_eq!(e.live().1, Some("invalid name"));
    let e = env();
    e.prototype(r#"{"live":"work","since":5}"#);
    fs::set_permissions(
        e.root().join(".rotator/ledger.json"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    assert_eq!(e.live().1, Some("untrusted"));
}

#[test]
fn identification_never_writes_under_the_prototype_directory() {
    let e = env();
    e.prototype(r#"{"live":"work","since":5}"#);
    let before: Vec<_> = fs::read_dir(e.root().join(".rotator")).unwrap().collect();
    e.live();
    let after: Vec<_> = fs::read_dir(e.root().join(".rotator")).unwrap().collect();
    assert_eq!(before.len(), after.len());
}

// --- Candidates ------------------------------------------------------------

fn skipped(skips: &[(ProfileName, Skip)], name: &str) -> Option<&'static str> {
    skips
        .iter()
        .find(|(n, _)| n.as_str() == name)
        .map(|(_, s)| s.as_str())
}

#[test]
fn logged_in_profiles_other_than_live_are_candidates() {
    let e = env();
    let work = n("work");
    let (c, skips) = candidates(&e.store, &e.input(Some(&work))).unwrap();
    assert_eq!(c, vec![n("extra"), n("spare")], "sorted by name");
    assert_eq!(skipped(&skips, "work"), Some("live"));
}

#[test]
fn every_documented_exclusion_applies() {
    let e = env();
    for p in ["a-excl", "b-run", "c-leased", "d-host", "e-park"] {
        e.store.add(&n(p)).unwrap();
        login(&e.root().join(p).join(".credentials.json"));
    }
    login(&e.root().join("f-link-target"));
    e.store.add(&n("f-linked")).unwrap();
    fs::hard_link(
        e.root().join("f-link-target"),
        e.root().join("f-linked/.credentials.json"),
    )
    .unwrap();
    let parked = e.root().join(".auto-switch.d/parked/e-park");
    fs::create_dir_all(&parked).unwrap();

    let mut input = e.input(None);
    input.exclude = vec![n("a-excl")];
    input.liveness = Liveness::Known(["b-run".to_string()].into());
    input.auto_switch = serde_json::from_value(serde_json::json!({
        "version": 1,
        "hosts": {"d-host": {"live": "c-leased", "swapped_at": 1, "pre_swap_fingerprint": null}}
    }))
    .unwrap();
    let (c, skips) = candidates(&e.store, &input).unwrap();
    assert_eq!(c, vec![n("extra"), n("spare")]);
    for (name, why) in [
        ("a-excl", "excluded"),
        ("b-run", "session running"),
        ("c-leased", "leased"),
        ("d-host", "hosting"),
        ("e-park", "parked"),
        ("f-linked", "no login"),
        ("work", "no login"),
    ] {
        assert_eq!(skipped(&skips, name), Some(why), "{name}");
    }
}

#[test]
fn unknown_liveness_leaves_no_candidates() {
    let e = env();
    for l in [Liveness::Everything, Liveness::Unsupported] {
        let mut input = e.input(None);
        input.liveness = l;
        let (c, skips) = candidates(&e.store, &input).unwrap();
        assert!(c.is_empty());
        assert_eq!(skipped(&skips, "spare"), Some("session running"));
    }
}

#[test]
fn a_root_that_cannot_be_listed_is_an_error_not_an_empty_list() {
    let e = env();
    let file = e._tmp.path().join("root-is-a-file");
    fs::write(&file, "").unwrap();
    let store = ProfileStore::new(file);
    assert!(candidates(&store, &e.input(None)).is_err());
}
