//! `default_live::detect`: the rotator ledger first, then the account match.
use super::default_live::{
    AMBIGUOUS_WARNING, LEDGER_WARNING, LIST_WARNING, OWN_LOGIN_WARNING, detect, ledger_live_name,
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

#[test]
fn a_rotate_ledger_for_another_account_is_ignored_after_a_hand_change() {
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    e.rotate_ledger(r#"{"version":1,"live":"b","account":"u-other","since":10}"#);
    assert_eq!(
        e.live(),
        (Some("a".into()), None),
        "the account match decides"
    );
}

// ---------------------------------------------------------------------------
// Issue #1693: `profile usage` reports the account actually in `~/.claude`
// while the rotate ledger names a profile whose account is not there.
//
// Contract: `default_live::detect_for_usage(&ProfileStore, Option<&Path>) ->
// UsageLive`, with `UsageLive::Profile(ProfileName)` (exactly one profile
// holds the account, with or without its own login), `UsageLive::Home` (no
// profile holds it) and a fallback to `detect` otherwise. `detect` and
// `live_name` are unchanged.
// ---------------------------------------------------------------------------

mod usage_live {
    use super::super::default_live::{UsageLive, detect_for_usage, live_name};
    use super::*;

    /// The rotate ledger names `a`, whose account is not the one in
    /// `~/.claude.json` (u1).
    fn hand_changed() -> Env {
        let e = env();
        e.claude_json("a", &account("u-a", "a@example.com"));
        e.rotate_ledger(r#"{"version":1,"live":"a","account":"u-a","since":10}"#);
        e
    }

    fn usage(e: &Env) -> UsageLive {
        detect_for_usage(&e.store, Some(&e.home))
    }

    #[test]
    fn the_one_holder_is_live_even_with_its_own_login() {
        let e = hand_changed();
        e.claude_json("b", &account("u1", "me@example.com"));
        private(&e.store.root().join("b/.credentials.json"), "{}");
        assert!(matches!(usage(&e), UsageLive::Profile(n) if n.as_str() == "b"));
        // The `profile login` refusal and the duplicate warning still go by
        // the ledger: in this state nobody is live for them.
        assert_eq!(live_name(&e.store, Some(&e.home)), None);
    }

    #[test]
    fn the_one_holder_without_a_login_is_live() {
        let e = hand_changed();
        e.claude_json("b", &account("u1", "me@example.com"));
        assert!(matches!(usage(&e), UsageLive::Profile(n) if n.as_str() == "b"));
    }

    #[test]
    fn no_holder_gives_the_home_row() {
        let e = hand_changed();
        assert!(matches!(usage(&e), UsageLive::Home));
    }

    #[test]
    fn several_holders_mark_nothing_new() {
        let e = hand_changed();
        e.store.add(&ProfileName::parse("c").unwrap()).unwrap();
        e.claude_json("b", &account("u1", "me@example.com"));
        e.claude_json("c", &account("u1", "me@example.com"));
        assert!(!matches!(
            usage(&e),
            UsageLive::Profile(_) | UsageLive::Home
        ));
    }

    #[test]
    fn a_matching_or_missing_rotate_ledger_falls_back_to_detect() {
        let e = env();
        e.claude_json("b", &account("u1", "me@example.com"));
        assert!(!matches!(usage(&e), UsageLive::Home), "no rotate ledger");
        e.rotate_ledger(&rotate_ledger("b", Some(10)));
        assert!(!matches!(usage(&e), UsageLive::Home), "the ledger matches");
        assert!(!matches!(
            detect_for_usage(&e.store, None),
            UsageLive::Profile(_) | UsageLive::Home
        ));
    }

    fn holder(e: &Env) -> super::super::default_live::Live {
        match usage(e) {
            UsageLive::Profile(name) => super::super::default_live::Live::holder(name),
            other => panic!("not a holder: {other:?}"),
        }
    }

    /// `usage` starts with `b` as the holder; a rotate cycle then recovers
    /// and records `b` as live. Same profile, new evidence: not a swap.
    #[test]
    fn the_holder_recorded_as_live_meanwhile_is_still_live() {
        let e = hand_changed();
        e.claude_json("b", &account("u1", "me@example.com"));
        let live = holder(&e);
        assert!(still_live(&e.store, Some(&e.home), &live));
        e.rotate_ledger(r#"{"version":1,"live":"b","account":"u1","since":20}"#);
        assert!(!matches!(usage(&e), UsageLive::Profile(_)));
        assert!(still_live(&e.store, Some(&e.home), &live));
    }

    #[test]
    fn the_holder_against_another_recorded_profile_is_a_swap() {
        let e = hand_changed();
        e.claude_json("b", &account("u1", "me@example.com"));
        let live = holder(&e);
        e.rotate_ledger(r#"{"version":1,"live":"a","account":"u1","since":20}"#);
        assert!(!still_live(&e.store, Some(&e.home), &live));
    }

    #[test]
    fn the_holder_gone_is_a_swap() {
        let e = hand_changed();
        e.claude_json("b", &account("u1", "me@example.com"));
        let live = holder(&e);
        e.claude_json("b", &account("u-b", "b@example.com"));
        assert!(matches!(usage(&e), UsageLive::Home));
        assert!(!still_live(&e.store, Some(&e.home), &live));
    }

    #[test]
    fn detecting_for_usage_writes_nothing() {
        let e = hand_changed();
        let p = e.store.root().join(".auto-switch.d/rotate/ledger.json");
        let before = fs::read(&p).unwrap();
        let _ = usage(&e);
        assert_eq!(fs::read(&p).unwrap(), before);
        assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
    }
}

// ---------------------------------------------------------------------------
// Issue #1745: `ledger_live_name`, the status line's owner. The ledgers
// only: no account match, no listing, no warning, and no `.claude.json` read
// but `~/.claude.json` for the rotate ledger's `account` check.
// ---------------------------------------------------------------------------

impl Env {
    fn ledger_live(&self) -> Option<String> {
        ledger_live_name(&self.store, Some(&self.home)).map(|n| n.as_str().to_string())
    }
}

#[test]
fn ledger_live_name_follows_either_ledger() {
    let e = env();
    e.ledger(LEDGER_A);
    assert_eq!(e.ledger_live(), Some("a".into()));
    let e = env();
    e.rotate_ledger(r#"{"version":1,"live":"b","since":10}"#);
    assert_eq!(e.ledger_live(), Some("b".into()));
}

#[test]
fn ledger_live_name_with_both_ledgers_takes_the_later_since() {
    let e = env();
    e.ledger(r#"{"live":"a","since":20}"#);
    e.rotate_ledger(r#"{"version":1,"live":"b","since":10}"#);
    assert_eq!(e.ledger_live(), Some("a".into()));
    e.ledger(r#"{"live":"a","since":5}"#);
    assert_eq!(e.ledger_live(), Some("b".into()));
}

#[test]
fn ledger_live_name_never_falls_back_to_the_account_match() {
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    assert_eq!(
        e.live(),
        (Some("a".into()), None),
        "detect matches accounts"
    );
    assert_eq!(e.ledger_live(), None, "the status line does not");
}

#[test]
fn ledger_live_name_compares_the_rotate_account() {
    // `~/.claude.json` holds u1. A rotate ledger recording another account
    // is a hand change (`claude /login` run in `~/.claude`): no owner.
    let e = env();
    e.rotate_ledger(r#"{"version":1,"live":"b","account":"someone-else","since":10}"#);
    assert_eq!(e.ledger_live(), None, "account differs");
    e.rotate_ledger(r#"{"version":1,"live":"b","account":"u1","since":10}"#);
    assert_eq!(e.ledger_live(), Some("b".into()), "account matches");
    fs::remove_file(e.home.join(".claude.json")).unwrap();
    assert_eq!(e.ledger_live(), None, "no `~/.claude.json` to compare");
    e.rotate_ledger(r#"{"version":1,"live":"b","since":10}"#);
    assert_eq!(e.ledger_live(), Some("b".into()), "no account recorded");
}

#[test]
fn ledger_live_name_reads_no_profile_claude_json() {
    // A profile's `.claude.json` that would match by account changes
    // nothing: only `~/.claude.json` is read.
    let e = env();
    e.claude_json("a", &account("u1", "me@example.com"));
    e.rotate_ledger(r#"{"version":1,"live":"b","account":"u1","since":10}"#);
    assert_eq!(e.ledger_live(), Some("b".into()));
}

#[test]
fn ledger_live_name_is_none_without_home_or_a_login_in_the_default_dir() {
    let e = env();
    e.ledger(LEDGER_A);
    assert_eq!(ledger_live_name(&e.store, None), None);
    fs::remove_file(e.home.join(".claude/.credentials.json")).unwrap();
    assert_eq!(e.ledger_live(), None);
}

#[test]
fn ledger_live_name_keeps_the_trust_rules() {
    let e = env();
    let p = e.ledger(LEDGER_A);
    fs::set_permissions(&p, fs::Permissions::from_mode(0o602)).unwrap();
    assert_eq!(e.ledger_live(), None, "other-writable ledger");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    let dir = e.store.root().join(".rotator");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o702)).unwrap();
    assert_eq!(e.ledger_live(), None, "other-writable directory");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(e.ledger_live(), Some("a".into()));
    fs::remove_file(&p).unwrap();
    let target = e.store.root().join("elsewhere.json");
    private(&target, LEDGER_A);
    std::os::unix::fs::symlink(&target, &p).unwrap();
    assert_eq!(e.ledger_live(), None, "symlinked ledger");
}

#[test]
fn ledger_live_name_refuses_damaged_idle_or_unknown_names() {
    let e = env();
    for body in [
        "not json",
        r#"{"live":null}"#,
        r#"{"since":1}"#,
        r#"{"live":"nobody","since":1}"#,
        r#"{"live":"../a","since":1}"#,
        r#"{"live":7,"since":1}"#,
    ] {
        e.ledger(body);
        assert_eq!(e.ledger_live(), None, "{body}");
    }
}

#[test]
fn ledger_live_name_refuses_a_profile_with_its_own_login() {
    let e = env();
    e.ledger(LEDGER_A);
    private(&e.store.root().join("a/.credentials.json"), "{}");
    assert_eq!(e.ledger_live(), None);
}
