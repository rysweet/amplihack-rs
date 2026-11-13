//! Issue #1886: hand-change recovery identifies the login in `~/.claude` by
//! the account Anthropic's OAuth profile endpoint reports for its token,
//! compared with each profile's #1845 record; never by `.claude.json` and
//! never by usage reset times. Unit tests for `rotate::cred_identity`, the
//! new ledger field and the new log lines. Every credential is a fake
//! `fake-at-1886-<uuid>` token, every request goes to the loopback
//! `FakeHttp`, and no process environment is set.
//!
//! Contract (`rotate::cred_identity`):
//! - `ReadFn = fn(&Path) -> Result<Account, &'static str>`. Two readings are
//!   one account by `Account::same`: equal account uuid in the same
//!   organization (e-mail only when neither has a uuid).
//! - `IDENTITY_READ_TIMEOUT` = 5 s, `IDENTITY_BUDGET` = 30 s.
//! - `read_with(&Endpoints, creds_dir: &Path) -> Result<Account, &'static
//!   str>`: one `GET /api/oauth/profile` with the folder's token, never a
//!   refresh and never the usage endpoint; the error is one of
//!   `usage_direct::reason`. `read_direct(&Path)` is the same with
//!   `Endpoints::from_env()`.
//! - `Finding { Clear, Suspect(ProfileName), Unsure(Why) }` and
//!   `assess(&ProfileStore, home: &Path, live: &ProfileName, read: &dyn
//!   Fn(&Path) -> Result<Account, &'static str>, budget: Duration) ->
//!   Finding` (D4). `read` is given `<home>/.claude` and `<root>/<name>`.
//!   When `live`'s record names the account in `~/.claude`, no profile is
//!   read and the finding is Clear.
//! - `Why { HomeFailed, Many(Vec<ProfileName>), Unread(Vec<ProfileName>),
//!   OutOfTime, ClockBack, NotRecorded(ProfileName), NotLive }`, each list
//!   sorted by name.
//!
//! Contract (ledger and lines):
//! - `hand_fields::SWAP_GRACE` = 600; `hand_fields::HandSuspect { profile:
//!   String, first_seen: i64 }`; `RotateLedger::hand_suspect:
//!   Option<HandSuspect>`, absent from the JSON when `None`. A ledger
//!   without the field loads; one with an invalid name or `first_seen <= 0`
//!   does not.
//! - `handfix_msg::note_suspect(m, l)`, `handfix_msg::note_unsure(l, &Why)`
//!   and `handfix_msg::only_login(m, l)`: the texts quoted in "Hand-change
//!   notes" and "Only-login ALERT" in docs/reference/profile-command.md.

use super::cred_identity::{
    Finding, IDENTITY_BUDGET, IDENTITY_READ_TIMEOUT, Why, assess, read_with,
};
use super::hand_fields::{HandSuspect, SWAP_GRACE};
use super::handfix_msg as msg;
use super::identify::Account;
use super::ledger::{RotateLedger, RotateState};
use super::tests_swap::{n, private};
use crate::commands::profile::auto_switch::kit::State;
use crate::commands::profile::expected::record;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::tests_fake_http::{FakeHttp, Reply};
use crate::commands::profile::usage_direct::{Endpoints, reason};
use serde_json::json;
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CREDS: &str = ".credentials.json";
const TOKEN_PATH: &str = "/v1/oauth/token";
const USAGE_PATH: &str = "/api/oauth/usage";
const PROFILE_PATH: &str = "/api/oauth/profile";

// --- Server accounts ---------------------------------------------------------

/// An OAuth profile answer for `uuid` in organization `org`.
pub(super) fn profile_body_in(uuid: &str, org: &str) -> serde_json::Value {
    json!({
        "account": {"uuid": uuid, "email": format!("{uuid}@example.com"), "full_name": "Test"},
        "organization": {"uuid": org, "name": "Test org"}
    })
}

/// An OAuth profile answer for `uuid` in the one test organization.
pub(super) fn profile_body(uuid: &str) -> serde_json::Value {
    profile_body_in(uuid, "org-1886")
}

/// The account the server would report for `uuid`.
pub(super) fn server(uuid: &str) -> Account {
    Account::from_profile(&profile_body(uuid)).unwrap()
}

#[test]
fn a_profile_answer_names_the_account_and_its_organization() {
    let a = server("u-mike");
    assert_eq!(a.key(), "u-mike");
    assert_eq!(a.org(), Some("org-1886"));
    assert_eq!(a.shown(), "u-mike@example.com");
    assert!(a.same(&server("u-mike")));
    assert!(!a.same(&server("u-lima")));
    let other_org = Account::from_profile(&profile_body_in("u-mike", "org-2")).unwrap();
    assert!(
        !a.same(&other_org),
        "one user in two organizations is two accounts"
    );
}

/// The 3 November collision: nnorth and mmorrison reset at the same times.
/// Identity no longer looks at reset times, so they stay two accounts.
#[test]
fn accounts_with_equal_reset_times_are_still_two_accounts() {
    let nnorth = Account::from_profile(&json!({
        "account": {"uuid": "u-nnorth", "email": "nnorth@mistt.com"},
        "organization": {"uuid": "org-mistt"},
        "five_hour": {"resets_at": "2026-11-03T14:40:00Z"},
        "seven_day": {"resets_at": "2026-11-06T05:00:00Z"}
    }))
    .unwrap();
    let mmorrison = Account::from_profile(&json!({
        "account": {"uuid": "u-mmorrison", "email": "mmorrison@mistt.com"},
        "organization": {"uuid": "org-mistt"},
        "five_hour": {"resets_at": "2026-11-03T14:40:00Z"},
        "seven_day": {"resets_at": "2026-11-06T05:00:00Z"}
    }))
    .unwrap();
    assert!(!nnorth.same(&mmorrison));
}

#[test]
fn an_answer_without_a_uuid_or_email_is_no_account() {
    for v in [
        json!({}),
        json!({"account": {}}),
        json!({"account": {"uuid": ""}}),
        json!({"account": {"uuid": "a\u{7}b"}}),
        json!({"account": {"uuid": "x".repeat(400)}}),
        json!({"organization": {"uuid": "org"}}),
        json!([1, 2]),
    ] {
        assert!(Account::from_profile(&v).is_none(), "{v}");
    }
    let email_only =
        Account::from_profile(&json!({"account": {"email": "Ann@Example.com"}})).unwrap();
    assert_eq!(email_only.shown(), "ann@example.com");
}

#[test]
fn a_server_account_debug_shows_nothing() {
    let s = format!("{:?}", server("u-mike"));
    assert!(!s.contains("u-mike") && !s.contains("example.com"), "{s}");
}

#[test]
fn read_limits_are_the_documented_ones() {
    assert_eq!(IDENTITY_READ_TIMEOUT, Duration::from_secs(5));
    assert_eq!(IDENTITY_BUDGET, Duration::from_secs(30));
    assert_eq!(SWAP_GRACE, 600);
}

// --- read_with against the loopback fake -------------------------------------

pub(super) fn fake_token() -> String {
    format!("fake-at-1886-{}", uuid::Uuid::new_v4())
}

fn iso(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// A usage body with a session and an all-models window resetting at
/// `session` and `week`.
pub(super) fn usage_body(session: i64, week: i64) -> serde_json::Value {
    json!({
        "five_hour": {"utilization": 10.0, "resets_at": iso(session)},
        "seven_day": {"utilization": 20.0, "resets_at": iso(week)},
        "extra_usage": {"is_enabled": false}
    })
}

pub(super) fn creds_doc(token: &str, expires_in_ms: i64) -> String {
    let at = chrono::Utc::now().timestamp_millis() + expires_in_ms;
    json!({"claudeAiOauth": {
        "accessToken": token,
        "refreshToken": format!("{token}-refresh"),
        "expiresAt": at,
        "subscriptionType": "max"
    }})
    .to_string()
}

pub(super) fn endpoints(fake: &FakeHttp) -> Endpoints {
    let (base, token) = (fake.base(), fake.token_url());
    let (ep, warnings) = Endpoints::resolve(Some(OsStr::new(&base)), Some(OsStr::new(&token)));
    assert!(warnings.is_empty(), "{warnings:?}");
    ep
}

/// A fake that answers each bearer token with its body, else 401.
pub(super) fn fake_by_token(answers: HashMap<String, Reply>) -> FakeHttp {
    FakeHttp::start(move |seen, _| {
        let auth = seen.header("authorization").unwrap_or_default();
        answers
            .iter()
            .find(|(t, _)| auth == format!("Bearer {t}"))
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| Reply::json(401, &json!({})))
    })
}

struct Dir {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
}

fn creds_dir(token: &str, expires_in_ms: i64) -> Dir {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let dir = tmp.path().join("p");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    private(&dir.join(CREDS), &creds_doc(token, expires_in_ms));
    Dir { _tmp: tmp, dir }
}

fn file_state(p: &Path) -> (u64, u64, i64, i64) {
    let m = fs::symlink_metadata(p).unwrap();
    (m.ino(), m.len(), m.mtime(), m.mtime_nsec())
}

#[test]
fn read_with_asks_the_profile_endpoint_for_the_account() {
    let (ta, tb, tc) = (fake_token(), fake_token(), fake_token());
    let fake = fake_by_token(HashMap::from([
        (ta.clone(), Reply::json(200, &profile_body("u-mike"))),
        (tb.clone(), Reply::json(200, &profile_body("u-mike"))),
        (tc.clone(), Reply::json(200, &profile_body("u-lima"))),
    ]));
    let (a, b, c) = (
        creds_dir(&ta, 3_600_000),
        creds_dir(&tb, 3_600_000),
        creds_dir(&tc, 3_600_000),
    );
    let ep = endpoints(&fake);
    let fa = read_with(&ep, &a.dir).unwrap();
    let fb = read_with(&ep, &b.dir).unwrap();
    let fc = read_with(&ep, &c.dir).unwrap();
    assert!(fa.same(&fb));
    assert!(!fa.same(&fc));
    assert_eq!(fake.count("GET", PROFILE_PATH), 3);
    assert_eq!(
        fake.count("GET", USAGE_PATH),
        0,
        "reset times are never read"
    );
    let seen = fake.seen();
    assert_eq!(
        seen[0].header("authorization"),
        Some(format!("Bearer {ta}").as_str()),
        "the token goes only in the Authorization header"
    );
    for s in &seen {
        assert!(
            !s.path.contains("fake-at-1886"),
            "token in a URL: {}",
            s.path
        );
    }
}

/// SEC-2: an identity read never refreshes, so it never writes a login.
#[test]
fn a_401_never_refreshes_or_writes_the_login() {
    let t = fake_token();
    let fake = FakeHttp::start(|_, _| Reply::json(401, &json!({})));
    let d = creds_dir(&t, 3_600_000);
    let before = file_state(&d.dir.join(CREDS));
    let got = read_with(&endpoints(&fake), &d.dir);
    assert_eq!(got.err(), Some(reason::HTTP_401));
    assert_eq!(fake.count("POST", TOKEN_PATH), 0, "no refresh");
    assert_eq!(file_state(&d.dir.join(CREDS)), before);
    assert!(!d.dir.join(".oauth_refresh.lock").exists());
}

#[test]
fn an_expired_token_is_not_refreshed() {
    let t = fake_token();
    let fake = FakeHttp::start(|_, _| Reply::json(200, &profile_body("u-x")));
    let d = creds_dir(&t, -60_000);
    let before = file_state(&d.dir.join(CREDS));
    assert_eq!(
        read_with(&endpoints(&fake), &d.dir).err(),
        Some(reason::EXPIRED)
    );
    assert_eq!(fake.seen().len(), 0, "an expired token is not sent");
    assert_eq!(file_state(&d.dir.join(CREDS)), before);
}

/// SEC-3: endpoints that fail the test guard mean no request at all.
#[test]
fn unusable_endpoints_send_nothing() {
    let (ep, _) = Endpoints::resolve_with(|k| {
        (k == "AMPLIHACK_TEST_HOME_GUARD").then(|| OsString::from("1"))
    });
    assert!(!ep.usable());
    let d = creds_dir(&fake_token(), 3_600_000);
    assert!(read_with(&ep, &d.dir).is_err());
}

#[test]
fn a_stalled_endpoint_ends_within_the_read_timeout() {
    let fake = FakeHttp::start(|_, _| {
        Reply::json(200, &profile_body("u-x")).after(Duration::from_secs(20))
    });
    let d = creds_dir(&fake_token(), 3_600_000);
    let start = Instant::now();
    assert!(read_with(&endpoints(&fake), &d.dir).is_err());
    assert!(
        start.elapsed() < IDENTITY_READ_TIMEOUT + Duration::from_secs(2),
        "took {:?}",
        start.elapsed()
    );
}

#[test]
fn a_429_and_an_unparseable_body_are_errors() {
    for reply in [
        Reply::json(429, &json!({})),
        Reply::raw(200, "not json"),
        Reply::json(500, &json!({})),
        Reply::json(200, &json!({"account": {}})),
        Reply::json(200, &usage_body(1, 2)),
    ] {
        let fake = FakeHttp::start(move |_, _| reply.clone());
        let d = creds_dir(&fake_token(), 3_600_000);
        assert!(read_with(&endpoints(&fake), &d.dir).is_err());
    }
}

// --- assess ------------------------------------------------------------------

pub(super) const L: &str = "lima";
pub(super) const M: &str = "mike";
pub(super) const K: &str = "kilo";

struct Tree {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    store: ProfileStore,
}

/// `~/.claude` with a login, and each profile in `own` with a login of its
/// own (contents irrelevant: the reader is faked by path).
fn tree(profiles: &[&str], own: &[&str]) -> Tree {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (home, root) = (base.join("home"), base.join("root"));
    let store = ProfileStore::new(root.clone());
    for p in profiles {
        store.add(&n(p)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    for p in own {
        private(
            &root.join(p).join(CREDS),
            &creds_doc(&fake_token(), 3_600_000),
        );
    }
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(home.join(".claude"), fs::Permissions::from_mode(0o700)).unwrap();
    private(
        &home.join(".claude").join(CREDS),
        &creds_doc(&fake_token(), 3_600_000),
    );
    Tree {
        _tmp: tmp,
        home,
        root,
        store,
    }
}

type Answers = HashMap<PathBuf, Result<Account, &'static str>>;

impl Tree {
    fn answers(&self, home: Result<Account, &'static str>) -> Answers {
        HashMap::from([(self.home.join(".claude"), home)])
    }

    fn with(&self, mut a: Answers, p: &str, r: Result<Account, &'static str>) -> Answers {
        a.insert(self.root.join(p), r);
        a
    }

    /// Runs `assess` with the fake reader; returns the finding and every
    /// path read, in order.
    fn assess(&self, a: &Answers) -> (Finding, Vec<PathBuf>) {
        let calls = std::cell::RefCell::new(Vec::new());
        let read = |p: &Path| {
            let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            calls.borrow_mut().push(p.clone());
            a.get(&p).cloned().unwrap_or(Err(reason::NO_FILE))
        };
        let f = assess(&self.store, &self.home, &n(L), &read, IDENTITY_BUDGET);
        (f, calls.into_inner())
    }

    /// Records `uuid` (in the test organization) for profile `p` (#1845).
    fn record(&self, p: &str, uuid: &str) {
        let state = State::new(&self.root).unwrap();
        record(&state, &n(p), &server(uuid)).unwrap();
    }
}

fn acct_m() -> Account {
    server("u-mike")
}

fn acct_l() -> Account {
    server("u-lima")
}

fn acct_k() -> Account {
    server("u-kilo")
}

/// The #1886 case: `~/.claude` answers as H, who has no copy of its own;
/// L's own copy (saved by the swap) answers differently.
#[test]
fn a_reading_unlike_every_profile_is_clear() {
    let t = tree(&[L, M, K], &[L, K]);
    let a = t.with(t.answers(Ok(acct_m())), L, Ok(acct_l()));
    let a = t.with(a, K, Ok(acct_k()));
    assert_eq!(t.assess(&a).0, Finding::Clear);
}

#[test]
fn a_match_with_exactly_one_other_profile_is_a_suspect() {
    let t = tree(&[L, M, K], &[L, M, K]);
    let a = t.with(t.answers(Ok(acct_m())), L, Ok(acct_l()));
    let a = t.with(t.with(a, M, Ok(acct_m())), K, Ok(acct_k()));
    assert_eq!(t.assess(&a).0, Finding::Suspect(n(M)));
}

#[test]
fn a_match_with_the_ledgers_own_copy_is_clear() {
    let t = tree(&[L, M], &[L, M]);
    let a = t.with(t.answers(Ok(acct_l())), L, Ok(acct_l()));
    let a = t.with(a, M, Ok(acct_m()));
    assert_eq!(t.assess(&a).0, Finding::Clear);
}

#[test]
fn a_match_with_two_profiles_is_unsure() {
    let t = tree(&[L, M, K], &[M, K]);
    let a = t.with(t.answers(Ok(acct_m())), M, Ok(acct_m()));
    let a = t.with(a, K, Ok(acct_m()));
    assert_eq!(t.assess(&a).0, Finding::Unsure(Why::Many(vec![n(K), n(M)])));
}

#[test]
fn a_failed_home_read_is_unsure_and_reads_no_profile() {
    let t = tree(&[L, M], &[L, M]);
    let a = t.with(t.answers(Err(reason::HTTP_429)), M, Ok(acct_m()));
    let (f, calls) = t.assess(&a);
    assert_eq!(f, Finding::Unsure(Why::Home(reason::HTTP_429)));
    assert_eq!(calls, vec![t.home.join(".claude")], "one request only");
}

/// #1845: `~/.claude` holding the account recorded for the live profile is
/// Clear without reading any profile, even when another profile's own
/// login is the same account.
#[test]
fn the_live_profiles_recorded_account_is_clear_without_more_reads() {
    let t = tree(&[L, M], &[M]);
    t.record(L, "u-mike");
    let a = t.with(t.answers(Ok(acct_m())), M, Ok(acct_m()));
    let (f, calls) = t.assess(&a);
    assert_eq!(f, Finding::Clear);
    assert_eq!(calls, vec![t.home.join(".claude")]);
}

/// A suspect's own folder must hold its own recorded account: when `M`'s
/// login is the account in `~/.claude` but `M`'s record names another,
/// `M`'s folder is itself crossed, and recovery only notes it.
#[test]
fn a_suspect_whose_record_names_another_account_is_unsure() {
    let t = tree(&[L, M], &[M]);
    t.record(M, "u-kilo");
    let a = t.with(t.answers(Ok(acct_m())), M, Ok(acct_m()));
    assert_eq!(t.assess(&a).0, Finding::Unsure(Why::NotRecorded(n(M))));
}

#[test]
fn a_suspect_whose_record_names_the_account_is_a_suspect() {
    let t = tree(&[L, M], &[M]);
    t.record(M, "u-mike");
    let a = t.with(t.answers(Ok(acct_m())), M, Ok(acct_m()));
    assert_eq!(t.assess(&a).0, Finding::Suspect(n(M)));
}

/// `~/.claude` holds neither the live profile's recorded account nor any
/// profile's own login: a note pointing at `profile verify`, no move.
#[test]
fn an_account_other_than_the_live_record_and_no_match_is_unsure() {
    let t = tree(&[L, M], &[M]);
    t.record(L, "u-lima");
    let a = t.with(t.answers(Ok(acct_k())), M, Ok(acct_m()));
    assert_eq!(t.assess(&a).0, Finding::Unsure(Why::NotLive));
}

#[test]
fn an_unreadable_profile_and_no_match_is_unsure() {
    let t = tree(&[L, M, K], &[M, K]);
    let a = t.with(t.answers(Ok(acct_m())), M, Err(reason::HTTP_401));
    let a = t.with(a, K, Ok(acct_k()));
    assert_eq!(
        t.assess(&a).0,
        Finding::Unsure(Why::Unread(vec![(n(M), reason::HTTP_401)]))
    );
}

/// Same account uuid in another organization is another account.
#[test]
fn the_same_user_in_another_organization_is_no_match() {
    let t = tree(&[L, M], &[M]);
    let elsewhere = Account::from_profile(&profile_body_in("u-mike", "org-2")).unwrap();
    let a = t.with(t.answers(Ok(acct_m())), M, Ok(elsewhere));
    assert_eq!(t.assess(&a).0, Finding::Clear);
}

/// The findings table is read top down: "Same as exactly one other profile"
/// comes before "cannot tell against one or more profiles, and Same as
/// none". A suspect is still only a suspect; the two-reading rule decides.
#[test]
fn one_match_and_an_unreadable_profile_is_a_suspect() {
    let t = tree(&[L, M, K], &[M, K]);
    let a = t.with(t.answers(Ok(acct_m())), M, Ok(acct_m()));
    let a = t.with(a, K, Err(reason::TIMEOUT));
    assert_eq!(t.assess(&a).0, Finding::Suspect(n(M)));
}

#[test]
fn profiles_without_their_own_login_are_never_read_or_suspected() {
    // Case A of #1693: M has no login; the old code recorded M from
    // `.claude.json`. Now nothing can name M.
    let t = tree(&[L, M], &[]);
    let a = t.answers(Ok(acct_m()));
    let (f, calls) = t.assess(&a);
    assert_eq!(f, Finding::Clear);
    assert_eq!(calls, vec![t.home.join(".claude")]);
}

#[test]
fn profiles_are_read_in_name_order_after_home() {
    let t = tree(&[L, M, K], &[L, M, K]);
    let a = t.with(t.answers(Ok(acct_m())), L, Ok(acct_l()));
    let a = t.with(t.with(a, M, Ok(acct_k())), K, Ok(acct_k()));
    let (_, calls) = t.assess(&a);
    assert_eq!(
        calls,
        vec![
            t.home.join(".claude"),
            t.root.join(K),
            t.root.join(L),
            t.root.join(M)
        ]
    );
}

#[test]
fn a_profile_dir_open_to_others_is_not_read() {
    let t = tree(&[L, M], &[M]);
    fs::set_permissions(t.root.join(M), fs::Permissions::from_mode(0o755)).unwrap();
    let a = t.with(t.answers(Ok(acct_m())), M, Ok(acct_m()));
    let (f, calls) = t.assess(&a);
    assert_ne!(f, Finding::Suspect(n(M)));
    assert!(!calls.contains(&t.root.join(M)), "{calls:?}");
}

#[test]
fn a_non_oauth_login_is_not_a_candidate() {
    let t = tree(&[L, M], &[]);
    private(&t.root.join(M).join(CREDS), "{}");
    let a = t.with(t.answers(Ok(acct_m())), M, Ok(acct_m()));
    let (f, calls) = t.assess(&a);
    assert_eq!(f, Finding::Clear);
    assert!(!calls.contains(&t.root.join(M)));
}

#[test]
fn assess_stops_when_the_budget_runs_out() {
    let t = tree(&[L, M, K], &[M, K]);
    let slow = Cell::new(0);
    let read = |p: &Path| {
        slow.set(slow.get() + 1);
        std::thread::sleep(Duration::from_millis(60));
        if p.ends_with(".claude") {
            Ok(acct_m())
        } else {
            Ok(acct_k())
        }
    };
    let f = assess(&t.store, &t.home, &n(L), &read, Duration::from_millis(100));
    assert_eq!(f, Finding::Unsure(Why::OutOfTime));
    assert!(
        slow.get() < 3,
        "a read was started after the budget ran out"
    );
}

// --- Lines -------------------------------------------------------------------

fn every_why() -> Vec<Why> {
    vec![
        Why::Home(reason::TIMEOUT),
        Why::Many(vec![n(K), n(M)]),
        Why::Unread(vec![(n(K), reason::HTTP_429), (n(M), reason::TIMEOUT)]),
        Why::Unread(Vec::new()),
        Why::OutOfTime,
        Why::ClockBack,
        Why::NotRecorded(n(M)),
        Why::NotLive,
    ]
}

/// SEC-17: names and fixed text only, one printable ASCII line.
#[test]
fn every_note_is_one_line_of_printable_ascii() {
    let mut lines: Vec<String> = every_why()
        .iter()
        .map(|w| msg::note_unsure(&n(L), w))
        .collect();
    lines.push(msg::note_suspect(&n(M), &n(L)));
    lines.push(msg::only_login(&n(M), &n(L)));
    for l in lines {
        assert!(
            l.chars().all(|c| c.is_ascii() && !c.is_ascii_control()),
            "{l:?}"
        );
        assert!(!l.contains("fake-at-1886"), "{l}");
    }
}

/// Issue #1962 (S7): a note saved in the ledger's `guidance` must fit its line
/// limit, or the failed read would make the ledger unreadable.
#[test]
fn a_long_note_fits_the_guidance_line_limit() {
    let names: Vec<(ProfileName, &'static str)> = (0..200)
        .map(|i| (n(&format!("a-long-profile-name-{i:03}")), reason::HTTP_429))
        .collect();
    let note = msg::note_unsure(&n(L), &Why::Unread(names));
    assert!(
        note.len() <= super::ledger::MAX_GUIDANCE_LINE,
        "{}",
        note.len()
    );
    assert!(note.is_char_boundary(note.len()));
    assert!(note.starts_with("note: cannot tell"), "{note}");
}

#[test]
fn note_texts_are_the_documented_ones() {
    let (l, m) = (n(L), n(M));
    assert_eq!(
        msg::note_suspect(&m, &l),
        "note: ~/.claude holds the same account as profile mike (ledger names lima); nothing moved, the next cycle reads again"
    );
    let why = |w: Why| msg::note_unsure(&l, &w);
    let head = "note: cannot tell whose login is in ~/.claude (live=lima): ";
    for (w, text) in [
        (
            Why::Home(reason::HTTP_429),
            "~/.claude identity read failed: http 429",
        ),
        (
            Why::Many(vec![n(K), n(M)]),
            "it holds the same account as more than one profile (kilo, mike)",
        ),
        (
            Why::Unread(vec![(n(K), reason::HTTP_429), (n(M), reason::TIMEOUT)]),
            "profile kilo not read (http 429), mike not read (timeout)",
        ),
        (
            Why::Unread(Vec::new()),
            "the profile root could not be listed",
        ),
        (Why::OutOfTime, "the reads ran out of time"),
        (Why::ClockBack, "the last swap time is later than now"),
        (
            Why::NotRecorded(n(M)),
            "it holds the same account as profile mike, which is not the account recorded for mike; run amplihack profile verify",
        ),
        (
            Why::NotLive,
            "it holds an account other than the one recorded for lima; run amplihack profile verify",
        ),
    ] {
        assert_eq!(why(w), format!("{head}{text}; nothing moved"));
    }
    assert_eq!(
        msg::only_login(&m, &l),
        "ALERT: ~/.claude holds the same account as profile mike on two readings (ledger names lima), but lima has no login of its own, so recording mike would leave lima with no login anywhere; nothing moved, no swap"
    );
}

#[test]
fn the_recovered_line_keeps_its_wording() {
    let line = msg::recovered_moved(&n(M), Path::new("/r/.rejected-logins/x/.credentials.json"));
    assert_eq!(
        line,
        "recovered: ~/.claude and mike held the same account; kept ~/.claude, moved mike's copy to /r/.rejected-logins/x/.credentials.json; recorded mike as live"
    );
}

// --- Ledger ------------------------------------------------------------------

fn state() -> (tempfile::TempDir, RotateState) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let rs = RotateState::new(tmp.path());
    rs.ensure().unwrap();
    (tmp, rs)
}

fn write_raw(rs: &RotateState, v: &serde_json::Value) {
    private(&rs.ledger, &v.to_string());
}

#[test]
fn an_old_ledger_without_hand_suspect_still_loads() {
    let (_t, rs) = state();
    write_raw(&rs, &json!({"version": 1, "live": L, "swapped_at": 100}));
    let l = rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.hand_suspect, None);
}

#[test]
fn hand_suspect_round_trips_and_is_absent_when_none() {
    let (_t, rs) = state();
    let mut l = RotateLedger {
        live: Some(L.into()),
        ..RotateLedger::default()
    };
    rs.save_ledger(&l).unwrap();
    let raw = fs::read_to_string(&rs.ledger).unwrap();
    assert!(!raw.contains("hand_suspect"), "{raw}");
    l.hand_suspect = Some(HandSuspect {
        profile: M.into(),
        first_seen: 1_793_071_025,
    });
    rs.save_ledger(&l).unwrap();
    assert_eq!(rs.load_ledger().unwrap().unwrap(), l);
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&rs.ledger).unwrap()).unwrap();
    assert_eq!(
        raw["hand_suspect"],
        json!({"profile": M, "first_seen": 1_793_071_025})
    );
}

/// SEC-12: a damaged suspicion fails closed, as other invalid fields do.
#[test]
fn an_invalid_hand_suspect_fails_to_load() {
    for bad in [
        json!({"profile": "../etc", "first_seen": 5}),
        json!({"profile": "", "first_seen": 5}),
        json!({"profile": "mi\nke", "first_seen": 5}),
        json!({"profile": M, "first_seen": 0}),
        json!({"profile": M, "first_seen": -1}),
        json!({"profile": M}),
    ] {
        let (_t, rs) = state();
        write_raw(
            &rs,
            &json!({"version": 1, "live": L, "hand_suspect": bad.clone()}),
        );
        assert!(rs.load_ledger().is_err(), "{bad}");
    }
}

#[test]
fn hand_suspect_holds_no_reading() {
    let s = HandSuspect {
        profile: M.into(),
        first_seen: 1,
    };
    let v = serde_json::to_value(&s).unwrap();
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["first_seen", "profile"]);
}

#[test]
fn profile_names_used_here_are_valid() {
    for p in [L, M, K] {
        assert!(ProfileName::parse(p).is_ok());
    }
}

// --- overflow_record::note_live ---------------------------------------------------

mod note_live_account {
    use super::super::identify::LiveId;
    use super::super::ledger::RotateLedger;
    use super::super::observe::Snap;
    use super::super::overflow_record::note_live;
    use super::super::pin::PinState;
    use super::{L, M, n};

    const SWAPPED: i64 = 1_793_000_000;
    const NOW: i64 = SWAPPED + 300;

    /// A snapshot with `live` and the account `~/.claude.json` names;
    /// `api_key`: `~/.claude` holds an API-key login.
    fn snap(live: &str, account: Option<&str>, api_key: bool) -> Snap {
        Snap {
            live: LiveId::Known(n(live)),
            proto: None,
            account: account.map(String::from),
            trusted_live: None,
            identity_refusal: None,
            identity_cause: None,
            candidate_refusals: Vec::new(),
            candidates: Vec::new(),
            skips: Vec::new(),
            sees_sessions: true,
            overflow: Vec::new(),
            pin: PinState::None,
            ignored: Vec::new(),
            unmarked: Vec::new(),
            detected_mark: None,
            home_no_oauth: api_key,
            candidate_accounts: Vec::new(),
            live_excluded: false,
        }
    }

    /// The ledger as a swap to `L` left it, with `L`'s account recorded.
    fn swapped_to_l() -> RotateLedger {
        RotateLedger {
            live: Some(L.into()),
            account: Some("u-lima".into()),
            since: Some(SWAPPED),
            swapped_at: Some(SWAPPED),
            ..RotateLedger::default()
        }
    }

    /// Issue #1886: running sessions write the previous account into
    /// `~/.claude.json` after a swap; the account the swap recorded stays.
    #[test]
    fn the_same_live_profile_keeps_the_recorded_account() {
        let mut l = swapped_to_l();
        note_live(&mut l, &snap(L, Some("u-mike"), false), NOW);
        assert_eq!(l.live.as_deref(), Some(L));
        assert_eq!(l.account.as_deref(), Some("u-lima"));
        assert_eq!(l.since, Some(SWAPPED), "since is not moved");
    }

    #[test]
    fn the_same_live_profile_with_no_recorded_account_records_one() {
        let mut l = RotateLedger {
            account: None,
            ..swapped_to_l()
        };
        note_live(&mut l, &snap(L, Some("u-lima"), false), NOW);
        assert_eq!(l.account.as_deref(), Some("u-lima"));
        assert_eq!(l.since, Some(SWAPPED));
    }

    /// An API-key login in `~/.claude` has no account of its own: the
    /// recorded one is cleared.
    #[test]
    fn the_same_live_profile_on_an_api_key_clears_the_account() {
        let mut l = swapped_to_l();
        note_live(&mut l, &snap(L, Some("u-mike"), true), NOW);
        assert_eq!(l.account, None);
        assert!(l.live_kind.is_some());
        assert_eq!(l.since, Some(SWAPPED));
    }

    /// No readable account and no API key: nothing is recorded.
    #[test]
    fn no_account_and_no_api_key_leaves_the_ledger_alone() {
        let mut l = swapped_to_l();
        let before = l.clone();
        note_live(&mut l, &snap(L, None, false), NOW);
        assert_eq!(l, before);
    }

    #[test]
    fn a_different_live_profile_replaces_the_account_and_since() {
        let mut l = swapped_to_l();
        note_live(&mut l, &snap(M, Some("u-mike"), false), NOW);
        assert_eq!(l.live.as_deref(), Some(M));
        assert_eq!(l.account.as_deref(), Some("u-mike"));
        assert_eq!(l.since, Some(NOW));
    }
}
