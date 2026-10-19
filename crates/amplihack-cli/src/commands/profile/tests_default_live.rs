//! `default_live::detect`: the rotator ledger first, then the account match.
use super::default_live::{
    AMBIGUOUS_WARNING, LEDGER_WARNING, LIST_WARNING, OWN_LOGIN_WARNING, detect, still_live,
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

    let e = env();
    e.claude_json("a", &account("", "ME@Example.com"));
    assert_eq!(e.live(), (Some("a".into()), None), "e-mail, any case");
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
