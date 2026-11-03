//! Issue #1886: hand-change recovery identifies the login in `~/.claude` by
//! what its credential returns (the reset times of a direct usage read),
//! never by `.claude.json`. Unit tests for `rotate::cred_identity`, the new
//! ledger field and the new log lines. Every credential is a fake
//! `fake-at-1886-<uuid>` token, every request goes to the loopback
//! `FakeHttp`, and no process environment is set.
//!
//! Contract (`rotate::cred_identity`):
//! - `Fingerprint`: `Clone`; built by `Fingerprint::from_report(&UsageReport,
//!   now)` from the `resets_at` of `session`, `week_all` and each
//!   `week_models` window, keyed by window. A window with no reset time, one
//!   at or before 0, or one outside `now - 1 day ..= now + 400 days`
//!   (seconds) is left out; labels over 64 bytes are left out; at most 32
//!   windows are kept. `len()` counts the windows kept. `Debug` prints no
//!   reset time and no label; there is no `Display` and no `Serialize`.
//! - `Verdict { Same, Different, Unsure }` and `compare(&Fingerprint,
//!   &Fingerprint) -> Verdict` (D3).
//! - `IDENTITY_READ_TIMEOUT` = 5 s, `IDENTITY_BUDGET` = 30 s.
//! - `read_with(&Endpoints, creds_dir: &Path) -> Result<Fingerprint,
//!   &'static str>`: one direct read with `allow_refresh = false`; the
//!   error is one of `usage_direct::reason`. `read_direct(&Path)` is the
//!   same with `Endpoints::from_env()`; `ReadFn` is its type.
//! - `Finding { Clear, Suspect(ProfileName), Unsure(Why) }` and
//!   `assess(&ProfileStore, home: &Path, live: &ProfileName, read: &dyn
//!   Fn(&Path) -> Result<Fingerprint, &'static str>, budget: Duration) ->
//!   Finding` (D4). `read` is given `<home>/.claude` and `<root>/<name>`.
//! - `Why { HomeFailed, HomeTooFew, Many(Vec<ProfileName>),
//!   Unread(Vec<ProfileName>), Windows(Vec<ProfileName>), OutOfTime,
//!   ClockBack }`, each list sorted by name.
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
    Finding, Fingerprint, IDENTITY_BUDGET, IDENTITY_READ_TIMEOUT, Verdict, Why, assess, compare,
    read_with,
};
use super::hand_fields::{HandSuspect, SWAP_GRACE};
use super::handfix_msg as msg;
use super::ledger::{RotateLedger, RotateState};
use super::tests_swap::{n, private};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::tests_fake_http::{FakeHttp, Reply};
use crate::commands::profile::usage_direct::{Endpoints, reason};
use crate::commands::profile::usage_parse::{UsageReport, UsageState, UsageWindow};
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

// --- Fingerprints -----------------------------------------------------------

fn window(label: &str, at: Option<i64>) -> UsageWindow {
    UsageWindow {
        label: label.to_string(),
        used_percentage: 10.0,
        resets_text: None,
        resets_at: at,
    }
}

/// A report with the given windows: `session` and `all models` go to their
/// own fields, any other label to `week_models`.
fn report(windows: &[(&str, Option<i64>)]) -> UsageReport {
    let mut r = UsageReport::empty(UsageState::Ok);
    for (label, at) in windows {
        match *label {
            "session" => r.session = Some(window(label, *at)),
            "all models" => r.week_all = Some(window(label, *at)),
            _ => r.week_models.push(window(label, *at)),
        }
    }
    r
}

/// A fingerprint read at the earliest of its reset times, so every window
/// given is within the plausible range.
pub(super) fn fp(windows: &[(&str, i64)]) -> Fingerprint {
    let w: Vec<(&str, Option<i64>)> = windows.iter().map(|(l, a)| (*l, Some(*a))).collect();
    let now = windows.iter().map(|(_, a)| *a).min().unwrap_or(0);
    Fingerprint::from_report(&report(&w), now)
}

/// The time the fixed fingerprints below are read at.
const NOW: i64 = 1_793_070_000;
const S1: i64 = 1_793_080_000;
const W1: i64 = 1_793_300_000;
const S2: i64 = 1_793_090_000;
const W2: i64 = 1_793_400_000;

#[test]
fn equal_reset_times_on_two_windows_are_the_same() {
    let a = fp(&[("session", S1), ("all models", W1)]);
    let b = fp(&[("session", S1), ("all models", W1)]);
    assert_eq!(compare(&a, &b), Verdict::Same);
    assert_eq!(compare(&b, &a), Verdict::Same);
}

#[test]
fn a_shared_window_with_another_reset_time_is_different() {
    let a = fp(&[("session", S1), ("all models", W1)]);
    for b in [
        fp(&[("session", S2), ("all models", W1)]),
        fp(&[("session", S1), ("all models", W2)]),
        fp(&[("session", S2), ("all models", W2)]),
    ] {
        assert_eq!(compare(&a, &b), Verdict::Different);
        assert_eq!(compare(&b, &a), Verdict::Different);
    }
}

#[test]
fn reset_times_are_compared_to_the_second() {
    let a = fp(&[("session", S1), ("all models", W1)]);
    let b = fp(&[("session", S1 + 1), ("all models", W1)]);
    assert_eq!(compare(&a, &b), Verdict::Different);
}

#[test]
fn a_different_window_wins_over_a_missing_one() {
    // `session` is missing from b (Unsure on its own), but `all models`
    // differs: a window both report disagrees, so they are Different.
    let a = fp(&[("session", S1), ("all models", W1), ("Opus", W1 + 10)]);
    let b = fp(&[("all models", W2), ("Opus", W1 + 10)]);
    assert_eq!(compare(&a, &b), Verdict::Different);
}

#[test]
fn one_comparable_window_is_unsure() {
    // #1818: a weekly-only login has a single window.
    let a = fp(&[("all models", W1)]);
    let b = fp(&[("all models", W1)]);
    assert_eq!(compare(&a, &b), Verdict::Unsure);
}

#[test]
fn a_window_in_only_one_reading_is_unsure() {
    // A session may have started between the two reads.
    let a = fp(&[("session", S1), ("all models", W1), ("Opus", W2)]);
    let b = fp(&[("all models", W1), ("Opus", W2)]);
    assert_eq!(compare(&a, &b), Verdict::Unsure);
    assert_eq!(compare(&b, &a), Verdict::Unsure);
}

#[test]
fn empty_fingerprints_are_unsure() {
    let empty = Fingerprint::from_report(&UsageReport::empty(UsageState::Ok), NOW);
    assert_eq!(empty.len(), 0);
    let a = fp(&[("session", S1), ("all models", W1)]);
    assert_eq!(compare(&empty, &empty), Verdict::Unsure);
    assert_eq!(compare(&empty, &a), Verdict::Unsure);
    assert_eq!(compare(&a, &empty), Verdict::Unsure);
}

#[test]
fn windows_without_a_plausible_reset_time_are_left_out() {
    let f = Fingerprint::from_report(
        &report(&[
            ("session", None),
            ("all models", Some(0)),
            ("Opus", Some(-5)),
            ("Sonnet", Some(i64::MIN)),
            ("Haiku", Some(W1)),
            ("Two days ago", Some(NOW - 2 * 86_400)),
            ("In 500 days", Some(NOW + 500 * 86_400)),
            ("Far ahead", Some(i64::MAX)),
        ]),
        NOW,
    );
    assert_eq!(f.len(), 1, "only Haiku is kept");
    // The bounds themselves are kept.
    let f = Fingerprint::from_report(
        &report(&[
            ("session", Some(NOW - 86_400)),
            ("all models", Some(NOW + 400 * 86_400)),
        ]),
        NOW,
    );
    assert_eq!(f.len(), 2);
    // An extreme `now` neither panics nor keeps anything implausible.
    for now in [i64::MIN, i64::MAX] {
        let f = Fingerprint::from_report(&report(&[("session", Some(S1))]), now);
        assert_eq!(f.len(), 0);
    }
}

/// SEC-14: a hostile reply cannot make the fingerprint grow without bound.
#[test]
fn fingerprint_labels_and_size_are_bounded() {
    let long = "x".repeat(65);
    let f = Fingerprint::from_report(
        &report(&[("session", Some(S1)), (long.as_str(), Some(W1))]),
        NOW,
    );
    assert_eq!(f.len(), 1, "a 65-byte label is left out");
    let labels: Vec<String> = (0..100).map(|i| format!("model-{i}")).collect();
    let w: Vec<(&str, Option<i64>)> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| (l.as_str(), Some(W1 + i as i64)))
        .collect();
    let f = Fingerprint::from_report(&report(&w), NOW);
    assert!(f.len() <= 32, "{} windows kept", f.len());
    assert!(f.len() >= 2);
}

#[test]
fn fingerprint_debug_shows_no_reset_time_or_label() {
    let f = fp(&[("session", S1), ("Opus Secret Label", W1)]);
    let shown = format!("{f:?}");
    assert!(!shown.contains(&S1.to_string()), "{shown}");
    assert!(!shown.contains(&W1.to_string()), "{shown}");
    assert!(!shown.contains("Opus Secret Label"), "{shown}");
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
fn read_with_turns_a_direct_read_into_a_fingerprint() {
    let now = chrono::Utc::now().timestamp();
    let (s, w) = (now + 3600, now + 3 * 86_400);
    let (ta, tb) = (fake_token(), fake_token());
    let fake = fake_by_token(HashMap::from([
        (ta.clone(), Reply::json(200, &usage_body(s, w))),
        (tb.clone(), Reply::json(200, &usage_body(s, w))),
    ]));
    let (a, b) = (creds_dir(&ta, 3_600_000), creds_dir(&tb, 3_600_000));
    let ep = endpoints(&fake);
    let fa = read_with(&ep, &a.dir).unwrap();
    let fb = read_with(&ep, &b.dir).unwrap();
    assert_eq!(fa.len(), 2);
    assert_eq!(compare(&fa, &fb), Verdict::Same);
    assert_eq!(fake.count("GET", USAGE_PATH), 2);
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
    let fake = FakeHttp::start(|_, _| Reply::json(200, &usage_body(1, 2)));
    let d = creds_dir(&t, -60_000);
    let before = file_state(&d.dir.join(CREDS));
    assert!(read_with(&endpoints(&fake), &d.dir).is_err());
    assert_eq!(fake.count("POST", TOKEN_PATH), 0);
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
    let fake =
        FakeHttp::start(|_, _| Reply::json(200, &usage_body(1, 2)).after(Duration::from_secs(20)));
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

type Answers = HashMap<PathBuf, Result<Fingerprint, &'static str>>;

impl Tree {
    fn answers(&self, home: Result<Fingerprint, &'static str>) -> Answers {
        HashMap::from([(self.home.join(".claude"), home)])
    }

    fn with(&self, mut a: Answers, p: &str, r: Result<Fingerprint, &'static str>) -> Answers {
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
}

fn acct_m() -> Fingerprint {
    fp(&[("session", S1), ("all models", W1)])
}

fn acct_l() -> Fingerprint {
    fp(&[("session", S2), ("all models", W2)])
}

fn acct_k() -> Fingerprint {
    fp(&[("session", S1 + 77), ("all models", W1 + 77)])
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
    assert_eq!(f, Finding::Unsure(Why::HomeFailed));
    assert_eq!(calls, vec![t.home.join(".claude")], "one request only");
}

#[test]
fn a_home_reading_with_one_window_is_unsure_and_reads_no_profile() {
    let t = tree(&[L, M], &[M]);
    let a = t.with(t.answers(Ok(fp(&[("all models", W1)]))), M, Ok(acct_m()));
    let (f, calls) = t.assess(&a);
    assert_eq!(f, Finding::Unsure(Why::HomeTooFew));
    assert_eq!(calls.len(), 1);
}

#[test]
fn an_unreadable_profile_and_no_match_is_unsure() {
    let t = tree(&[L, M, K], &[M, K]);
    let a = t.with(t.answers(Ok(acct_m())), M, Err(reason::HTTP_401));
    let a = t.with(a, K, Ok(acct_k()));
    assert_eq!(t.assess(&a).0, Finding::Unsure(Why::Unread(vec![n(M)])));
}

#[test]
fn a_window_presence_mismatch_and_no_match_is_unsure() {
    let t = tree(&[L, M], &[M]);
    let three = fp(&[("session", S1), ("all models", W1), ("Opus", W2)]);
    let a = t.with(t.answers(Ok(three)), M, Ok(acct_m()));
    assert_eq!(t.assess(&a).0, Finding::Unsure(Why::Windows(vec![n(M)])));
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
        Why::HomeFailed,
        Why::HomeTooFew,
        Why::Many(vec![n(K), n(M)]),
        Why::Unread(vec![n(K), n(M)]),
        Why::Windows(vec![n(M)]),
        Why::OutOfTime,
        Why::ClockBack,
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

#[test]
fn note_texts_are_the_documented_ones() {
    let (l, m) = (n(L), n(M));
    assert_eq!(
        msg::note_suspect(&m, &l),
        "note: ~/.claude answers as profile mike (ledger names lima); nothing moved, the next cycle reads again"
    );
    let why = |w: Why| msg::note_unsure(&l, &w);
    let head = "note: cannot tell whose login is in ~/.claude (live=lima): ";
    for (w, text) in [
        (Why::HomeFailed, "~/.claude could not be read"),
        (
            Why::HomeTooFew,
            "~/.claude reported fewer than two reset times",
        ),
        (
            Why::Many(vec![n(K), n(M)]),
            "it matches more than one profile (kilo, mike)",
        ),
        (
            Why::Unread(vec![n(K), n(M)]),
            "profile kilo, mike could not be read",
        ),
        (
            Why::Windows(vec![n(M)]),
            "the readings of ~/.claude and profile mike report different windows",
        ),
        (Why::OutOfTime, "the reads ran out of time"),
        (Why::ClockBack, "the last swap time is later than now"),
    ] {
        assert_eq!(why(w), format!("{head}{text}; nothing moved"));
    }
    assert_eq!(
        msg::only_login(&m, &l),
        "ALERT: ~/.claude answers as profile mike on two readings (ledger names lima), but lima has no login of its own, so recording mike would leave lima with no login anywhere; nothing moved, no swap"
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
