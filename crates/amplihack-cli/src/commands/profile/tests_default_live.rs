//! `default_live::detect`: the rotator ledger first, then the account match.
use super::default_live::{
    AMBIGUOUS_WARNING, LEDGER_WARNING, LIST_WARNING, OWN_LOGIN_WARNING, detect, ledger_live,
    live_name, still_live,
};
use super::name::ProfileName;
use super::store::ProfileStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    store: ProfileStore,
}

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn account(uuid: &str, email: &str) -> String {
    serde_json::json!({"oauthAccount": {"accountUuid": uuid, "emailAddress": email}}).to_string()
}

/// Profiles `a` and `b`, neither logged in; `~/.claude` has a login whose
/// account is `u1` / `me@example.com`.
fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let store = ProfileStore::new(tmp.path().join("root"));
    for n in ["a", "b"] {
        store.add(&ProfileName::parse(n).unwrap()).unwrap();
    }
    private(&home.join(".claude/.credentials.json"), "{}");
    private(&home.join(".claude.json"), &account("u1", "me@example.com"));
    Env {
        _tmp: tmp,
        home,
        store,
    }
}

impl Env {
    fn ledger(&self, body: &str) -> PathBuf {
        let dir = self.store.root().join(".rotator");
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("ledger.json");
        private(&p, body);
        p
    }

    fn claude_json(&self, profile: &str, body: &str) {
        private(&self.store.root().join(profile).join(".claude.json"), body);
    }

    fn live(&self) -> (Option<String>, Option<&'static str>) {
        let (live, w) = detect(&self.store, Some(&self.home));
        (live.map(|l| l.name.as_str().to_string()), w)
    }
}

const LEDGER_A: &str =
    r#"{"duplicate":null,"live":"a","live_hash":"h","live_week_reset":0,"since":1}"#;

#[test]
fn ledger_names_the_live_profile() {
    let e = env();
    e.ledger(LEDGER_A);
    assert_eq!(e.live(), (Some("a".into()), None));
}

#[test]
fn nothing_is_live_without_a_login_in_default_dir_or_home() {
    let e = env();
    e.ledger(LEDGER_A);
    assert_eq!(detect(&e.store, None).0, None);
    fs::remove_file(e.home.join(".claude/.credentials.json")).unwrap();
    assert_eq!(e.live(), (None, None));
}

#[test]
fn ledger_profile_with_its_own_login_is_not_live_and_warns() {
    let e = env();
    e.ledger(LEDGER_A);
    let creds = e.store.root().join("a/.credentials.json");
    private(&creds, "{}");
    assert_eq!(e.live(), (None, Some(OWN_LOGIN_WARNING)));
    fs::remove_file(&creds).unwrap();
    std::os::unix::fs::symlink("/nonexistent", &creds).unwrap();
    assert_eq!(
        e.live(),
        (None, Some(OWN_LOGIN_WARNING)),
        "dangling symlink"
    );
}

#[test]
fn untrusted_or_malformed_ledger_warns_and_skips_the_account_match() {
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    let p = e.ledger("not json");
    assert_eq!(e.live(), (None, Some(LEDGER_WARNING)));
    private(&p, LEDGER_A);
    fs::set_permissions(&p, fs::Permissions::from_mode(0o620)).unwrap();
    assert_eq!(e.live(), (None, Some(LEDGER_WARNING)));
    fs::remove_file(&p).unwrap();
    let target = e.store.root().join("elsewhere.json");
    private(&target, LEDGER_A);
    std::os::unix::fs::symlink(&target, &p).unwrap();
    assert_eq!(e.live(), (None, Some(LEDGER_WARNING)));
}

#[test]
fn ledger_in_a_shared_directory_or_root_is_refused() {
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    e.ledger(LEDGER_A);
    let dir = e.store.root().join(".rotator");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o702)).unwrap();
    assert_eq!(e.live(), (None, Some(LEDGER_WARNING)), "other-writable");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(e.live(), (Some("a".into()), None));
    let root = e.store.root();
    let before = fs::metadata(root).unwrap().permissions().mode();
    fs::set_permissions(root, fs::Permissions::from_mode(0o707)).unwrap();
    assert_eq!(e.live(), (None, Some(LEDGER_WARNING)));
    fs::set_permissions(root, fs::Permissions::from_mode(before)).unwrap();
}

#[test]
fn idle_ledger_decides_nothing_is_live_quietly() {
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    for body in [r#"{"live":null}"#, r#"{"since":1}"#] {
        e.ledger(body);
        assert_eq!(e.live(), (None, None), "{body}");
    }
}

#[test]
fn ledger_with_a_bad_or_unknown_live_name_warns() {
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    for body in [r#"{"live":"../x"}"#, r#"{"live":7}"#, r#"{"live":"ghost"}"#] {
        e.ledger(body);
        assert_eq!(e.live(), (None, Some(LEDGER_WARNING)), "{body}");
    }
}

#[test]
fn a_new_since_or_account_counts_as_a_swap() {
    let e = env();
    e.ledger(LEDGER_A);
    let live = detect(&e.store, Some(&e.home)).0.unwrap();
    assert!(still_live(&e.store, Some(&e.home), &live));
    e.ledger(&LEDGER_A.replace(r#""since":1"#, r#""since":2"#));
    assert!(!still_live(&e.store, Some(&e.home), &live));

    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    let live = detect(&e.store, Some(&e.home)).0.unwrap();
    assert!(still_live(&e.store, Some(&e.home), &live));
    private(
        &e.home.join(".claude.json"),
        &account("u9", "x@example.com"),
    );
    assert!(!still_live(&e.store, Some(&e.home), &live));
}

#[test]
fn without_a_ledger_the_matching_account_is_live() {
    let e = env();
    e.claude_json("a", &account("u2", "me@example.com"));
    e.claude_json("b", &account("u1", "other@example.com"));
    assert_eq!(e.live(), (Some("b".into()), None), "uuids decide");

    // Without a uuid on either side the e-mails decide (D2).
    let e = env();
    private(
        &e.home.join(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"me@example.com"}}"#,
    );
    e.claude_json("a", &account("", "ME@Example.com"));
    assert_eq!(e.live(), (Some("a".into()), None), "e-mail, any case");
}

#[test]
fn a_uuid_on_one_side_only_does_not_match() {
    // `~/.claude.json` has u1; a profile with only an e-mail is not the
    // same account (D2).
    let e = env();
    e.claude_json("a", &account("", "ME@Example.com"));
    assert_eq!(e.live(), (None, None));
}

fn account_org(uuid: &str, org: &str, email: &str) -> String {
    serde_json::json!({"oauthAccount": {
        "accountUuid": uuid,
        "organizationUuid": org,
        "emailAddress": email,
    }})
    .to_string()
}

#[test]
fn one_user_in_two_orgs_marks_the_matching_org_live_without_a_warning() {
    // D22: ryan-mistt-org and ryan-mistt-personal.
    let e = env();
    private(
        &e.home.join(".claude.json"),
        &account_org("u1", "o-personal", "me@example.com"),
    );
    e.claude_json("a", &account_org("u1", "o-team", "me@example.com"));
    e.claude_json("b", &account_org("u1", "o-personal", "me@example.com"));
    assert_eq!(e.live(), (Some("b".into()), None));
}

#[test]
fn one_user_in_two_orgs_with_neither_matching_marks_none() {
    let e = env();
    private(
        &e.home.join(".claude.json"),
        &account_org("u1", "o-third", "me@example.com"),
    );
    e.claude_json("a", &account_org("u1", "o-team", "me@example.com"));
    e.claude_json("b", &account_org("u1", "o-personal", "me@example.com"));
    assert_eq!(e.live(), (None, None));
}

#[test]
fn the_same_user_and_org_twice_is_still_ambiguous() {
    let e = env();
    private(
        &e.home.join(".claude.json"),
        &account_org("u1", "o1", "me@example.com"),
    );
    e.claude_json("a", &account_org("u1", "o1", ""));
    e.claude_json("b", &account_org("u1", "o1", ""));
    assert_eq!(e.live(), (None, Some(AMBIGUOUS_WARNING)));
}

#[test]
fn an_org_on_one_side_only_does_not_match() {
    let e = env();
    private(
        &e.home.join(".claude.json"),
        &account_org("u1", "o1", "me@example.com"),
    );
    e.claude_json("a", &account("u1", "me@example.com"));
    assert_eq!(e.live(), (None, None));
}

#[test]
fn the_default_dir_account_is_read_through_the_safe_filter() {
    // D4: the shared `identify::Account` replaces the private copy, so an
    // over-long uuid counts as missing here too.
    let e = env();
    let long = "u".repeat(321);
    private(&e.home.join(".claude.json"), &account(&long, ""));
    e.claude_json("a", &account(&long, ""));
    assert_eq!(e.live(), (None, None));
}

// --- live_name (bug 3) ------------------------------------------------------

#[test]
fn live_name_is_the_detected_profile() {
    let e = env();
    e.ledger(LEDGER_A);
    assert_eq!(
        live_name(&e.store, Some(&e.home)).map(|n| n.as_str().to_string()),
        Some("a".into())
    );
}

#[test]
fn live_name_is_none_without_home_detection_or_when_ambiguous() {
    let e = env();
    assert_eq!(live_name(&e.store, None), None, "no $HOME");
    assert_eq!(live_name(&e.store, Some(&e.home)), None, "nothing matches");
    e.claude_json("a", &account("u1", ""));
    e.claude_json("b", &account("u1", ""));
    assert_eq!(live_name(&e.store, Some(&e.home)), None, "ambiguous");
    e.ledger("not json");
    assert_eq!(live_name(&e.store, Some(&e.home)), None, "bad ledger");
}

#[test]
fn two_matching_accounts_mark_none_and_warn() {
    let e = env();
    e.claude_json("a", &account("u1", ""));
    e.claude_json("b", &account("u1", ""));
    assert_eq!(e.live(), (None, Some(AMBIGUOUS_WARNING)));
}

#[test]
fn an_unlistable_profile_root_marks_none_and_warns() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return; // root ignores mode bits
    }
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    let root = e.store.root().to_path_buf();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
    let got = e.live();
    // Readable but not searchable: the profiles can be listed but not used.
    fs::set_permissions(&root, fs::Permissions::from_mode(0o600)).unwrap();
    let got_600 = e.live();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(got, (None, Some(LIST_WARNING)));
    assert_eq!(got_600, (None, Some(LIST_WARNING)), "mode 600");
    assert_eq!(e.live(), (Some("a".into()), None), "listable again");
}

#[test]
fn a_ledger_that_cannot_be_looked_at_is_unreadable_not_missing() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return; // root can search any directory
    }
    let e = env();
    // Without the ledger, `a` would be live by the account match.
    e.claude_json("a", &account("u1", "me@example.com"));
    e.ledger(r#"{"live":null}"#);
    let dir = e.store.root().join(".rotator");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
    let got = e.live();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(got, (None, Some(LEDGER_WARNING)));
}

#[test]
fn a_symlinked_login_in_default_dir_is_no_login() {
    let e = env();
    e.ledger(LEDGER_A);
    let creds = e.home.join(".claude/.credentials.json");
    let target = e.home.join("real-credentials.json");
    fs::rename(&creds, &target).unwrap();
    std::os::unix::fs::symlink(&target, &creds).unwrap();
    assert_eq!(e.live(), (None, None));
}

#[test]
fn a_rotator_symlink_even_dangling_is_an_untrusted_ledger() {
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    let dir = e.store.root().join(".rotator");
    std::os::unix::fs::symlink(e.store.root().join("missing"), &dir).unwrap();
    assert_eq!(e.live(), (None, Some(LEDGER_WARNING)), "dangling");
    fs::remove_file(&dir).unwrap();
    let real = e.store.root().join("real-rotator");
    fs::create_dir(&real).unwrap();
    private(&real.join("ledger.json"), LEDGER_A);
    std::os::unix::fs::symlink(&real, &dir).unwrap();
    assert_eq!(e.live(), (None, Some(LEDGER_WARNING)), "to a directory");
}

// ---------------------------------------------------------------------------
// Issue #1592 (D14): the `profile rotate` ledger and the prototype's.
// ---------------------------------------------------------------------------

impl Env {
    fn rotate_ledger(&self, body: &str) -> PathBuf {
        let dir = self.store.root().join(".auto-switch.d/rotate");
        fs::create_dir_all(&dir).unwrap();
        for d in [dir.parent().unwrap(), dir.as_path()] {
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let p = dir.join("ledger.json");
        private(&p, body);
        p
    }
}

fn rotate_ledger(live: &str, since: Option<i64>) -> String {
    let mut v = serde_json::json!({"version": 1, "live": live, "account": "u1"});
    if let Some(s) = since {
        v["since"] = s.into();
    }
    v.to_string()
}

#[test]
fn the_rotate_ledger_alone_names_the_live_profile() {
    let e = env();
    e.rotate_ledger(&rotate_ledger("b", Some(10)));
    assert_eq!(e.live(), (Some("b".into()), None));
}

#[test]
fn the_ledger_with_the_later_since_decides() {
    let e = env();
    e.ledger(r#"{"live":"a","since":5}"#);
    e.rotate_ledger(&rotate_ledger("b", Some(10)));
    assert_eq!(e.live(), (Some("b".into()), None), "rotate is later");

    let e = env();
    e.ledger(r#"{"live":"a","since":20}"#);
    e.rotate_ledger(&rotate_ledger("b", Some(10)));
    assert_eq!(e.live(), (Some("a".into()), None), "prototype is later");
}

#[test]
fn a_tie_goes_to_the_rotate_ledger() {
    let e = env();
    e.ledger(r#"{"live":"a","since":10}"#);
    e.rotate_ledger(&rotate_ledger("b", Some(10)));
    assert_eq!(e.live(), (Some("b".into()), None));
}

#[test]
fn a_ledger_without_since_is_older() {
    let e = env();
    e.ledger(r#"{"live":"a","since":1}"#);
    e.rotate_ledger(&rotate_ledger("b", None));
    assert_eq!(e.live(), (Some("a".into()), None));
}

#[test]
fn a_malformed_rotate_ledger_warns_and_the_prototype_decides() {
    let e = env();
    e.ledger(r#"{"live":"a","since":1}"#);
    e.rotate_ledger("not json");
    assert_eq!(e.live(), (Some("a".into()), Some(LEDGER_WARNING)));
}

#[test]
fn an_untrusted_rotate_ledger_warns_and_the_prototype_decides() {
    let e = env();
    e.ledger(r#"{"live":"a","since":1}"#);
    let p = e.rotate_ledger(&rotate_ledger("b", Some(10)));
    fs::set_permissions(&p, fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(e.live(), (Some("a".into()), Some(LEDGER_WARNING)));
}

#[test]
fn a_rotate_ledger_that_names_nobody_leaves_the_account_match() {
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    for body in [
        r#"{"version":1,"last_cycle":{"at":1,"outcome":"alerted"},"alert":"no-capacity:-"}"#,
        r#"{"version":1,"live":null}"#,
    ] {
        e.rotate_ledger(body);
        assert_eq!(e.live(), (Some("a".into()), None), "{body}");
    }
}

#[test]
fn a_rotate_ledger_that_names_nobody_does_not_override_the_prototype() {
    let e = env();
    e.ledger(r#"{"live":"b"}"#);
    e.rotate_ledger(r#"{"version":1,"alert":"no-capacity:-"}"#);
    assert_eq!(e.live(), (Some("b".into()), None));
}

#[test]
fn reading_the_rotate_ledger_writes_nothing() {
    let e = env();
    let p = e.rotate_ledger(&rotate_ledger("b", Some(10)));
    let before = fs::read(&p).unwrap();
    let _ = e.live();
    assert_eq!(fs::read(&p).unwrap(), before);
    assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn a_new_rotate_since_counts_as_a_swap() {
    let e = env();
    e.rotate_ledger(&rotate_ledger("b", Some(10)));
    let live = detect(&e.store, Some(&e.home)).0.unwrap();
    assert!(still_live(&e.store, Some(&e.home), &live));
    e.rotate_ledger(&rotate_ledger("b", Some(11)));
    assert!(!still_live(&e.store, Some(&e.home), &live));
}

#[test]
fn a_prototype_iso_since_is_compared_as_a_time() {
    let e = env();
    // The prototype writes `datetime.now().isoformat()`, in local time.
    e.ledger(r#"{"live":"a","since":"2999-01-01T00:00:00"}"#);
    e.rotate_ledger(&rotate_ledger("b", Some(2_000_000_000)));
    assert_eq!(e.live(), (Some("a".into()), None), "prototype is later");
    e.ledger(r#"{"live":"a","since":"2000-01-01T00:00:00"}"#);
    assert_eq!(e.live(), (Some("b".into()), None), "rotate is later");
}

/// Issue #1886: running sessions rewrite `~/.claude.json` from memory, so
/// after a swap it can still name the previous account. The rotate ledger
/// is taken at its word; `.claude.json` no longer overrides it.
#[test]
fn a_rotate_ledger_is_taken_at_its_word_whatever_claude_json_names() {
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    e.rotate_ledger(r#"{"version":1,"live":"b","account":"u-other","since":10}"#);
    assert_eq!(e.live(), (Some("b".into()), None), "the ledger decides");
}

// ---------------------------------------------------------------------------
// `still_live` (`profile usage`): the same detection repeated after the
// probes. Issue #1886: by the ledgers only, with no holder evidence from
// `.claude.json`.
// ---------------------------------------------------------------------------

mod usage_live {
    use super::super::default_live::{detect, live_name};
    use super::*;

    fn detected(e: &Env) -> super::super::default_live::Live {
        detect(&e.store, Some(&e.home)).0.expect("a live profile")
    }

    #[test]
    fn the_ledgers_profile_is_still_live_until_the_ledger_changes() {
        let e = env();
        e.rotate_ledger(r#"{"version":1,"live":"a","since":10}"#);
        let live = detected(&e);
        assert_eq!(live.name.as_str(), "a");
        assert!(still_live(&e.store, Some(&e.home), &live));
        e.rotate_ledger(r#"{"version":1,"live":"b","since":20}"#);
        assert!(!still_live(&e.store, Some(&e.home), &live));
    }

    #[test]
    fn claude_json_naming_another_account_is_not_a_swap() {
        let e = env();
        e.claude_json("b", &account("u1", "me@example.com"));
        e.rotate_ledger(r#"{"version":1,"live":"a","account":"u-a","since":10}"#);
        let live = detected(&e);
        assert_eq!(live.name.as_str(), "a");
        assert_eq!(live_name(&e.store, Some(&e.home)), Some(live.name.clone()));
        assert!(still_live(&e.store, Some(&e.home), &live));
    }

    #[test]
    fn no_home_is_never_still_live() {
        let e = env();
        e.rotate_ledger(r#"{"version":1,"live":"a","since":10}"#);
        let live = detected(&e);
        assert!(!still_live(&e.store, None, &live));
    }

    #[test]
    fn detecting_writes_nothing() {
        let e = env();
        e.rotate_ledger(r#"{"version":1,"live":"a","account":"u-a","since":10}"#);
        let p = e.store.root().join(".auto-switch.d/rotate/ledger.json");
        let before = fs::read(&p).unwrap();
        let _ = detect(&e.store, Some(&e.home));
        assert_eq!(fs::read(&p).unwrap(), before);
        assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
    }
}

// ---------------------------------------------------------------------------
// Issue #1745: `ledger_live`, the status line's login. The two ledgers only:
// no login file, no `.claude.json`, no listing, no warning.
// ---------------------------------------------------------------------------

impl Env {
    fn ledger_live(&self) -> Option<(String, Option<f64>)> {
        ledger_live(&self.store).map(|l| (l.name.as_str().to_string(), l.since))
    }

    fn ledger_live_name(&self) -> Option<String> {
        self.ledger_live().map(|(n, _)| n)
    }
}

#[test]
fn ledger_live_follows_either_ledger_and_reports_its_since() {
    let e = env();
    e.ledger(LEDGER_A);
    assert_eq!(e.ledger_live(), Some(("a".into(), Some(1.0))));
    let e = env();
    e.rotate_ledger(r#"{"version":1,"live":"b","since":10}"#);
    assert_eq!(e.ledger_live(), Some(("b".into(), Some(10.0))));
}

#[test]
fn ledger_live_with_both_ledgers_takes_the_later_since() {
    let e = env();
    e.ledger(r#"{"live":"a","since":20}"#);
    e.rotate_ledger(r#"{"version":1,"live":"b","since":10}"#);
    assert_eq!(e.ledger_live(), Some(("a".into(), Some(20.0))));
    e.ledger(r#"{"live":"a","since":5}"#);
    assert_eq!(e.ledger_live(), Some(("b".into(), Some(10.0))));
}

#[test]
fn ledger_live_tie_goes_to_the_rotate_ledger() {
    let e = env();
    e.ledger(r#"{"live":"a","since":10}"#);
    e.rotate_ledger(r#"{"version":1,"live":"b","since":10}"#);
    assert_eq!(e.ledger_live_name(), Some("b".into()));
    // Two missing `since` values tie too.
    e.ledger(r#"{"live":"a"}"#);
    e.rotate_ledger(r#"{"version":1,"live":"b"}"#);
    assert_eq!(e.ledger_live(), Some(("b".into(), None)));
}

#[test]
fn ledger_live_counts_a_missing_since_as_older() {
    let e = env();
    e.ledger(r#"{"live":"a","since":1}"#);
    e.rotate_ledger(r#"{"version":1,"live":"b"}"#);
    assert_eq!(e.ledger_live_name(), Some("a".into()));
    e.ledger(r#"{"live":"a"}"#);
    e.rotate_ledger(r#"{"version":1,"live":"b","since":1}"#);
    assert_eq!(e.ledger_live_name(), Some("b".into()));
}

#[test]
fn ledger_live_skips_a_ledger_that_names_nobody_or_is_damaged() {
    let e = env();
    e.ledger(r#"{"live":"a","since":1}"#);
    for rotate in [
        r#"{"version":1,"live":null,"since":99}"#,
        r#"{"version":1,"since":99}"#,
        "not json",
    ] {
        e.rotate_ledger(rotate);
        assert_eq!(e.ledger_live_name(), Some("a".into()), "{rotate}");
    }
}

#[test]
fn ledger_live_never_compares_accounts_or_matches_them() {
    // Without a ledger `detect` matches profile a by account; the status
    // line does not, because it reads no `.claude.json` at all. With a
    // rotate ledger both take it at its word (issue #1886).
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    assert_eq!(
        e.live(),
        (Some("a".into()), None),
        "detect matches accounts"
    );
    assert_eq!(e.ledger_live(), None, "the status line does not");
    e.rotate_ledger(r#"{"version":1,"live":"b","account":"someone-else","since":10}"#);
    assert_eq!(e.ledger_live_name(), Some("b".into()));
    assert_eq!(e.live(), (Some("b".into()), None));
}

#[test]
fn ledger_live_needs_no_login_in_the_default_dir() {
    let e = env();
    e.ledger(LEDGER_A);
    fs::remove_file(e.home.join(".claude/.credentials.json")).unwrap();
    fs::remove_file(e.home.join(".claude.json")).unwrap();
    assert_eq!(e.ledger_live_name(), Some("a".into()));
}

#[test]
fn ledger_live_keeps_the_trust_rules() {
    let e = env();
    let p = e.ledger(LEDGER_A);
    fs::set_permissions(&p, fs::Permissions::from_mode(0o602)).unwrap();
    assert_eq!(e.ledger_live(), None, "other-writable ledger");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    let dir = e.store.root().join(".rotator");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o702)).unwrap();
    assert_eq!(e.ledger_live(), None, "other-writable directory");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(e.ledger_live_name(), Some("a".into()));
    fs::remove_file(&p).unwrap();
    let target = e.store.root().join("elsewhere.json");
    private(&target, LEDGER_A);
    std::os::unix::fs::symlink(&target, &p).unwrap();
    assert_eq!(e.ledger_live(), None, "symlinked ledger");
}

#[test]
fn an_empty_live_counts_as_no_ledger_whichever_ledger_is_later() {
    // Each way round: the empty ledger is later, and must not hide the other.
    let e = env();
    e.ledger(r#"{"live":"","since":2}"#);
    e.rotate_ledger(r#"{"live":"a","since":1}"#);
    assert_eq!(e.ledger_live_name(), Some("a".into()));
    e.ledger(r#"{"live":"a","since":1}"#);
    e.rotate_ledger(r#"{"live":"","since":2}"#);
    assert_eq!(e.ledger_live_name(), Some("a".into()));
}

#[test]
fn ledger_live_refuses_names_that_are_not_profile_names() {
    let e = env();
    for body in [
        r#"{"live":"../a","since":1}"#,
        r#"{"live":7,"since":1}"#,
        r#"{"live":"","since":1}"#,
        r#"{"live":"a\u001b[2J","since":1}"#,
    ] {
        e.ledger(body);
        assert_eq!(e.ledger_live(), None, "{body}");
    }
}
