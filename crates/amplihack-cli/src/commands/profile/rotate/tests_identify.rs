//! `rotate::identify`: which profile is live in `~/.claude`, and which
//! profiles may receive it (D11, D13, D16, issue #1592). Only `.claude.json`
//! files and metadata are read, and login files only for their top-level keys.
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

use super::identify::{Account, CandidateInput, LiveId, Skip, candidates, live, refused_dirs};
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

/// An OAuth login (issue #1700: its top-level keys are read for the kind).
fn login(path: &Path) {
    private(path, r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#);
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

fn acct_org(uuid: &str, org: &str, email: &str) -> String {
    serde_json::json!({"oauthAccount": {
        "accountUuid": uuid,
        "organizationUuid": org,
        "emailAddress": email,
    }})
    .to_string()
}

fn parse(body: &str) -> Account {
    Account::from_claude_json(body.as_bytes()).unwrap()
}

// --- Identity rule (D2, bug 1) ----------------------------------------------
// One user can belong to several organizations, each with its own
// subscription and limits: the same uuid in a different org is another
// account.

#[test]
fn same_uuid_and_same_org_is_the_same_account() {
    let a = parse(&acct_org("u1", "o1", "me@example.com"));
    let b = parse(&acct_org("u1", "o1", "other@example.com"));
    assert!(a.same(&b));
    assert!(b.same(&a), "symmetric");
}

#[test]
fn same_uuid_in_another_org_is_another_account() {
    let team = parse(&acct_org("u1", "o-team", "me@example.com"));
    let personal = parse(&acct_org("u1", "o-personal", "me@example.com"));
    assert!(!team.same(&personal));
    assert!(!personal.same(&team));
}

#[test]
fn an_org_on_one_side_only_is_another_account() {
    let with_org = parse(&acct_org("u1", "o1", "me@example.com"));
    let without = parse(&acct("u1", "me@example.com"));
    assert!(!with_org.same(&without));
    assert!(!without.same(&with_org));
}

#[test]
fn same_uuid_and_no_org_on_either_side_is_the_same_account() {
    let a = parse(&acct("u1", "me@example.com"));
    let b = parse(&acct("u1", "other@example.com"));
    assert!(a.same(&b));
}

#[test]
fn different_uuids_are_different_accounts_whatever_the_org() {
    for (oa, ob) in [("o1", "o1"), ("o1", "o2"), ("", "")] {
        let a = parse(&acct_org("u1", oa, "me@example.com"));
        let b = parse(&acct_org("u2", ob, "me@example.com"));
        assert!(!a.same(&b), "orgs {oa:?}/{ob:?}");
    }
}

#[test]
fn without_uuids_the_emails_decide_ignoring_case() {
    let a = parse(r#"{"oauthAccount":{"emailAddress":"Me@Example.com"}}"#);
    let b = parse(r#"{"oauthAccount":{"emailAddress":"me@EXAMPLE.com"}}"#);
    let c = parse(r#"{"oauthAccount":{"emailAddress":"you@example.com"}}"#);
    assert!(a.same(&b));
    assert!(!a.same(&c));
}

#[test]
fn a_uuid_on_one_side_only_is_another_account() {
    // Was an e-mail fallback before D2: a missing uuid never makes two
    // logins look like one account.
    let a = parse(&acct("u1", "me@example.com"));
    let email_only = parse(r#"{"oauthAccount":{"emailAddress":"me@EXAMPLE.com"}}"#);
    assert!(!a.same(&email_only));
    assert!(!email_only.same(&a));
}

#[test]
fn the_org_does_not_change_the_ledger_key() {
    // D3: the ledger still records the user only.
    let a = parse(&acct_org("u1", "o1", "me@example.com"));
    assert_eq!(a.key(), "u1");
    assert!(a.matches_key("u1"));
    assert!(a.matches_key("ME@example.com"));
    assert!(!a.matches_key("o1"), "an org id is never a key");
}

#[test]
fn an_unsafe_org_counts_as_missing() {
    let plain = parse(&acct("u1", "me@example.com"));
    for bad in ["o".repeat(321), "o\u{1b}1".to_string(), String::new()] {
        let a = parse(&acct_org("u1", &bad, "me@example.com"));
        assert!(a.same(&plain), "org {bad:?} must count as missing");
    }
    let non_string = parse(
        r#"{"oauthAccount":{"accountUuid":"u1","organizationUuid":7,"emailAddress":"me@example.com"}}"#,
    );
    assert!(
        non_string.same(&plain),
        "a non-string org counts as missing"
    );
}

#[test]
fn from_block_reads_a_bare_oauth_account_block() {
    let block = serde_json::json!({"accountUuid": "u1", "organizationUuid": "o1"});
    let a = Account::from_block(&block).unwrap();
    assert!(a.same(&parse(&acct_org("u1", "o1", ""))));
    assert!(!a.same(&parse(&acct_org("u1", "o2", ""))));
    assert!(Account::from_block(&serde_json::json!("not an object")).is_none());
    assert!(Account::from_block(&serde_json::json!({})).is_none());
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
    let a = parse(&acct_org("uuid-secret", "org-secret", "secret@example.com"));
    let shown = format!("{a:?}");
    assert!(
        !shown.contains("uuid-secret")
            && !shown.contains("secret@")
            && !shown.contains("org-secret"),
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
fn login_less_profiles_of_one_user_in_two_orgs_are_not_ambiguous() {
    // ryan-mistt-org and ryan-mistt-personal: one user, two subscriptions.
    let e = env();
    e.store.add(&n("personal")).unwrap();
    private(
        &e.root().join("work/.claude.json"),
        &acct_org("u-work", "o-team", "work@example.com"),
    );
    private(
        &e.root().join("personal/.claude.json"),
        &acct_org("u-work", "o-personal", "work@example.com"),
    );
    private(
        &e.home.join(".claude.json"),
        &acct_org("u-work", "o-personal", "work@example.com"),
    );
    assert_eq!(e.live(), (LiveId::Known(n("personal")), None));
}

#[test]
fn login_less_profiles_with_the_same_user_and_org_are_ambiguous() {
    let e = env();
    e.store.add(&n("twin")).unwrap();
    for p in ["work", "twin"] {
        private(
            &e.root().join(p).join(".claude.json"),
            &acct_org("u-work", "o1", "work@example.com"),
        );
    }
    private(
        &e.home.join(".claude.json"),
        &acct_org("u-work", "o1", "work@example.com"),
    );
    assert_eq!(e.live().0, LiveId::Ambiguous);
}

#[test]
fn a_prototype_ledger_whose_profile_is_in_another_org_is_ignored() {
    let e = env();
    private(
        &e.root().join("work/.claude.json"),
        &acct_org("u-work", "o-team", "work@example.com"),
    );
    private(
        &e.home.join(".claude.json"),
        &acct_org("u-work", "o-personal", "work@example.com"),
    );
    e.prototype(r#"{"live":"work","since":1}"#);
    assert_eq!(e.live().1, Some("account does not match"));
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
fn a_refused_directory_is_skipped_as_directory_refused_not_no_login() {
    // Issue #1698: a directory open to other users fails the check, so its
    // login cannot be looked at. That is not a missing login: the overflow
    // rule holds a move onto credits while a plan profile is refused.
    let e = env();
    for p in ["g-open", "h-excl"] {
        e.store.add(&n(p)).unwrap();
        private(
            &e.root().join(p).join(".claude.json"),
            &acct(p, "x@example.com"),
        );
        login(&e.root().join(p).join(".credentials.json"));
        fs::set_permissions(e.root().join(p), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut input = e.input(None);
    input.exclude = vec![n("h-excl")];
    let (c, skips) = candidates(&e.store, &input).unwrap();
    assert_eq!(c, vec![n("extra"), n("spare")]);
    assert_eq!(skipped(&skips, "g-open"), Some("directory refused"));
    // Out of rotation anyway, so it holds nothing.
    assert_eq!(skipped(&skips, "h-excl"), Some("excluded"));
    // A missing login is still `no login`, and is not refused.
    assert_eq!(skipped(&skips, "work"), Some("no login"));
    assert_eq!(refused_dirs(&skips), vec![n("g-open")]);

    // Fixed, it is a candidate again.
    fs::set_permissions(e.root().join("g-open"), fs::Permissions::from_mode(0o700)).unwrap();
    let (c, skips) = candidates(&e.store, &input).unwrap();
    assert_eq!(c, vec![n("extra"), n("g-open"), n("spare")]);
    assert!(refused_dirs(&skips).is_empty());
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

// --- Issue #1693: who holds the account in ~/.claude ------------------------
//
// Contract: `home_holders(&ProfileStore, home: &Path) -> Result<Holders,
// ProfileError>`, with `Holders::{None, One { name, own_login }, Many(Vec),
// Unreadable(Vec), HomeUnreadable, HomeNoLogin}` (lists sorted by name).
// Every verified profile counts, with or without its own login. Precedence:
// HomeNoLogin, HomeUnreadable, Unreadable, then None / One / Many.
// `home_still_matches(home, key) -> bool` re-reads `~/.claude.json`.

mod holders {
    use super::super::identify::{Holders, home_holders, home_still_matches};
    use super::*;

    fn holders(e: &Env) -> Holders {
        home_holders(&e.store, &e.home).unwrap()
    }

    #[test]
    fn one_login_less_profile_holds_the_account() {
        let e = env();
        assert_eq!(
            holders(&e),
            Holders::One {
                name: n("work"),
                own_login: false
            }
        );
    }

    #[test]
    fn a_profile_with_its_own_login_counts_too() {
        let e = env();
        login(&e.root().join("work/.credentials.json"));
        assert_eq!(
            holders(&e),
            Holders::One {
                name: n("work"),
                own_login: true
            }
        );
    }

    #[test]
    fn a_dangling_symlink_is_an_own_login() {
        let e = env();
        std::os::unix::fs::symlink("/nonexistent", e.root().join("work/.credentials.json"))
            .unwrap();
        assert!(matches!(
            holders(&e),
            Holders::One {
                own_login: true,
                ..
            }
        ));
    }

    #[test]
    fn several_holders_are_listed_sorted() {
        let e = env();
        for p in ["b-twin", "a-twin"] {
            e.store.add(&n(p)).unwrap();
            private(
                &e.root().join(p).join(".claude.json"),
                &acct("u-work", "work@example.com"),
            );
        }
        login(&e.root().join("b-twin/.credentials.json"));
        assert_eq!(
            holders(&e),
            Holders::Many(vec![n("a-twin"), n("b-twin"), n("work")])
        );
    }

    #[test]
    fn an_account_nobody_has_is_none() {
        let e = env();
        private(
            &e.home.join(".claude.json"),
            &acct("u-nobody", "nobody@example.com"),
        );
        assert_eq!(holders(&e), Holders::None);
    }

    #[test]
    fn the_same_user_in_another_organization_is_not_a_holder() {
        let e = env();
        private(
            &e.root().join("work/.claude.json"),
            &serde_json::json!({"oauthAccount": {"accountUuid": "u-work",
                "organizationUuid": "o-2", "emailAddress": "work@example.com"}})
            .to_string(),
        );
        assert_eq!(holders(&e), Holders::None);
    }

    #[test]
    fn a_logged_in_profile_without_an_account_makes_it_unreadable() {
        let e = env();
        fs::remove_file(e.root().join("extra/.claude.json")).unwrap();
        assert_eq!(holders(&e), Holders::Unreadable(vec![n("extra")]));
    }

    #[test]
    fn a_login_less_profile_without_an_account_is_skipped() {
        let e = env();
        e.store.add(&n("blank")).unwrap();
        assert!(
            matches!(holders(&e), Holders::One { .. }),
            "{:?}",
            holders(&e)
        );
    }

    #[test]
    fn home_problems_come_first() {
        let e = env();
        fs::remove_file(e.root().join("extra/.claude.json")).unwrap();
        private(&e.home.join(".claude.json"), r#"{"projects":{}}"#);
        assert_eq!(holders(&e), Holders::HomeUnreadable);
        fs::remove_file(e.home.join(".claude/.credentials.json")).unwrap();
        assert_eq!(holders(&e), Holders::HomeNoLogin);
    }

    #[test]
    fn a_symlinked_home_login_is_no_login() {
        let e = env();
        let creds = e.home.join(".claude/.credentials.json");
        fs::remove_file(&creds).unwrap();
        std::os::unix::fs::symlink(e.root().join("spare/.credentials.json"), &creds).unwrap();
        assert_eq!(holders(&e), Holders::HomeNoLogin);
    }

    #[test]
    fn still_matches_compares_the_key_again() {
        let e = env();
        assert!(home_still_matches(&e.home, "u-work"));
        assert!(!home_still_matches(&e.home, "u-spare"));
        private(&e.home.join(".claude.json"), "not json");
        assert!(!home_still_matches(&e.home, "u-work"));
    }

    #[test]
    fn the_holder_query_reads_no_login_file() {
        // The login is mode 0000 here (issue #1700's `login` fixture is
        // readable): a read would fail, so a result that names an own login
        // came from metadata only.
        let e = env();
        login(&e.root().join("work/.credentials.json"));
        if crate::commands::profile::auto_switch::euid() != 0 {
            fs::set_permissions(
                e.root().join("work/.credentials.json"),
                fs::Permissions::from_mode(0o000),
            )
            .unwrap();
            assert!(fs::read(e.root().join("work/.credentials.json")).is_err());
        }
        assert!(matches!(
            holders(&e),
            Holders::One {
                own_login: true,
                ..
            }
        ));
    }
}
