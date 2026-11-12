//! `usage_direct` (issue #1844): reading a login's usage from the OAuth
//! usage endpoint, with at most one token refresh under Claude's lock.
//!
//! Contract:
//! - Constants: `API_BASE_ENV` = `AMPLIHACK_PROFILE_USAGE_API_BASE`,
//!   `TOKEN_URL_ENV` = `AMPLIHACK_PROFILE_USAGE_TOKEN_URL`,
//!   `DEFAULT_API_BASE` = `https://api.anthropic.com`, `DEFAULT_TOKEN_URL`
//!   = `https://platform.claude.com/v1/oauth/token`, `USAGE_PATH` =
//!   `/api/oauth/usage`, `BETA` = `oauth-2025-04-20`, `USER_AGENT` =
//!   `amplihack-profile-usage`, `CLIENT_ID` (non-empty), `MAX_BODY` = 64 KiB,
//!   `REFRESH_MARGIN_SECS` = 60, `MAX_TOKEN` = 4096, `MAX_EXPIRES_IN` =
//!   31,536,000.
//! - `base_ok(&str) -> bool`: `https://...`, or `http://` with the host
//!   exactly `127.0.0.1`, `[::1]` or `localhost` (optionally a port and a
//!   path). The scheme is matched case-sensitively; whitespace, control
//!   characters, `@`, `?` and `#` are refused.
//! - `Endpoints::resolve(api, token) -> (Endpoints, Vec<String>)`: each
//!   refused value gives one fixed warning that never repeats it, and the
//!   default. Empty counts as unset. `Endpoints::usage_url()` is the base
//!   with `USAGE_PATH`; `Endpoints::token_url()` is the token URL as given;
//!   `Endpoints::https_only()` is false only when both are loopback.
//! - `Token::parse(&str) -> Option<Token>`: 1 to 4096 bytes, every byte in
//!   0x21 to 0x7E. `Debug` prints `<redacted>`; there is no `Display`.
//! - `Creds::parse(&[u8]) -> Option<Creds>`: `claudeAiOauth.accessToken`
//!   (required), `refreshToken` and `expiresAt` (ms). `Debug` is redacted.
//! - `expires_in(&Value) -> Option<i64>`: a JSON integer from 1 to
//!   31,536,000.
//! - `map_err(&ureq::Error) -> &'static str`: the only place that looks at a
//!   `ureq::Error`. Statuses map to `http 401`, `http 403`, `http 429`,
//!   `http 3xx`, `http 4xx` or `http 5xx`; a timeout to `timeout`; anything
//!   else to `transport`.
//! - `read(&Endpoints, &Request) -> Result<Direct, &'static str>`, where
//!   `Direct { report, credits_on: Option<ExtraUsageShape> }` (issue #1856:
//!   credits on with a readable window is a success carrying the shape), with
//!   `Request { creds_dir, journals, allow_refresh, cancel, timeout, now_ms }`;
//!   `timeout` is one deadline for the whole read, refresh included: each
//!   request is given only the time left of it, and a refresh POST is sent
//!   only with at least `REFRESH_BUDGET` (3 s) left, with connect and read
//!   steps of at least that budget; once the POST may have reached the
//!   server (any failure after the connect, or a 200 that cannot be used or
//!   saved) the reason is `refresh not saved`;
//!   with `cancel` set, a refresh takes no lock and gives `refresh busy`.
//!   Errors are the fixed texts in `reason`.
//! - `refreshes_in_progress() -> usize` counts refreshes holding a lock;
//!   `wait_for_refreshes(limit) -> bool` waits up to `limit` for it to
//!   reach 0.
//! - Issue #2019: `read_with(&Endpoints, &Request, &Ctx)` and
//!   `refresh_expired_with(&Endpoints, &Request, &Ctx)` take the clock the
//!   read's deadline is measured on and the refresh counter, through
//!   `Ctx::new(&dyn Clock, &AtomicUsize, HANG_GUARD)`.
//!   `read` and `refresh_expired` are
//!   the same with `Ctx::system()`: `SystemClock` and the process-wide
//!   counter `refreshes_in_progress` reads. `get_step(timeout)` is the `GET`
//!   agent's connect and read step, `MAX_STEP.min(timeout - 1 s)
//!   .max(MIN_STEP)`; `post_step(left)` is the refresh POST's,
//!   `left.min(MAX_STEP)`.
//!
//! No refresh or deadline test asserts on real elapsed time. Virtual time
//! moves only through `VirtualClock` (`test_support::virtual_clock`).
//! Logical deadline tests inject a scripted transport; loopback tests
//! configure real I/O limits independently through `HANG_GUARD`, a hang cap
//! no test asserts on.
//!
//! Tests use scripted responses or a loopback fake server and fake tokens;
//! nothing reads a real credential or reaches a real endpoint.

use super::tests_fake_http::{FakeHttp, Reply, Seen, closed_base};
use super::usage_direct::Direct;
use super::usage_direct::{
    API_BASE_ENV, BETA, CLIENT_ID, Creds, DEFAULT_API_BASE, DEFAULT_TOKEN_URL, Endpoints, MAX_BODY,
    MAX_EXPIRES_IN, MAX_STEP, MAX_TOKEN, MIN_STEP, REFRESH_BUDGET, REFRESH_MARGIN_SECS, Request,
    TOKEN_URL_ENV, Token, USAGE_PATH, USER_AGENT, base_ok, bounded_lookup, expires_in, map_err,
    post_failure, read, reason, refresh_counter, refresh_status, refreshes_in_progress,
};
use super::usage_direct::{Ctx, get_step, post_step, read_with, refresh_expired_with};
use super::usage_parse::UsageState;
use crate::test_support::virtual_clock::VirtualClock;
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// A stop flag that is never set.
static NO_CANCEL: AtomicBool = AtomicBool::new(false);

const ACCESS: &str = "FAKE-ACCESS-1844-a";
const REFRESH: &str = "FAKE-REFRESH-1844-a";
const NEW_ACCESS: &str = "FAKE-ACCESS-1844-b";
const NEW_REFRESH: &str = "FAKE-REFRESH-1844-b";
const CREDS: &str = ".credentials.json";
const TOKEN_PATH: &str = "/v1/oauth/token";

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn iso(at_secs: i64) -> String {
    chrono::DateTime::from_timestamp(at_secs, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// A usage body: session 100%, week 55%, credits off.
fn body_100_55() -> Value {
    let now = chrono::Utc::now().timestamp();
    json!({
        "five_hour": {"utilization": 100.0, "resets_at": iso(now + 3600)},
        "seven_day": {"utilization": 55.0, "resets_at": iso(now + 4 * 86_400)},
        "extra_usage": {"is_enabled": false},
        "seven_day_opus": null,
        "something_new": {"x": 1}
    })
}

struct Login {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    root: PathBuf,
}

impl Login {
    fn path(&self) -> PathBuf {
        self.dir.join(CREDS)
    }

    fn doc(&self) -> Value {
        serde_json::from_slice(&fs::read(self.path()).unwrap()).unwrap()
    }

    fn lock(&self) -> PathBuf {
        self.dir.join(".oauth_refresh.lock")
    }

    fn journals(&self) -> Vec<PathBuf> {
        vec![
            self.root.join(".auto-switch.d").join("journal.json"),
            self.root
                .join(".auto-switch.d")
                .join("rotate")
                .join("journal.json"),
        ]
    }

    fn entries(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

/// A private profile directory holding a login that expires in an hour,
/// with extra fields that a refresh must keep.
fn login_expiring_in(ms: i64) -> Login {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = tmp.path().join("root");
    let dir = root.join("spare");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let doc = json!({
        "claudeAiOauth": {
            "accessToken": ACCESS,
            "refreshToken": REFRESH,
            "expiresAt": now_ms() + ms,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max"
        },
        "mcpOAuth": {"keep": true}
    });
    let path = dir.join(CREDS);
    fs::write(&path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    Login {
        _tmp: tmp,
        dir,
        root,
    }
}

fn login() -> Login {
    login_expiring_in(3_600_000)
}

fn endpoints(fake: &FakeHttp) -> Endpoints {
    let base = fake.base();
    let token = fake.token_url();
    let (ep, warnings) = Endpoints::resolve(Some(OsStr::new(&base)), Some(OsStr::new(&token)));
    assert!(warnings.is_empty(), "{warnings:?}");
    ep
}

fn request<'a>(l: &'a Login, journals: &'a [PathBuf], allow_refresh: bool) -> Request<'a> {
    Request {
        creds_dir: &l.dir,
        journals,
        allow_refresh,
        cancel: &NO_CANCEL,
        timeout: Duration::from_secs(5),
        now_ms: now_ms(),
    }
}

fn run(
    fake: &FakeHttp,
    l: &Login,
    allow_refresh: bool,
) -> Result<super::usage_parse::UsageReport, &'static str> {
    run_direct(fake, l, allow_refresh).map(|d| d.report)
}

/// The whole success: the report and, when credits are on, the shape. The
/// read runs on a clock that does not move (`run_with`), so a loaded host
/// cannot use up its deadline before the fake answers.
fn run_direct(fake: &FakeHttp, l: &Login, allow_refresh: bool) -> Result<Direct, &'static str> {
    run_with(fake, l, allow_refresh)
}

fn bearer(s: &Seen) -> Option<&str> {
    s.header("authorization")
}

fn gets(fake: &FakeHttp) -> Vec<Seen> {
    fake.seen()
        .into_iter()
        .filter(|s| s.method == "GET" && s.path == USAGE_PATH)
        .collect()
}

fn posts(fake: &FakeHttp) -> usize {
    fake.count("POST", TOKEN_PATH)
}

/// Usage answers 200 to the new token and 401 to anything else; the token
/// endpoint answers with `refresh`.
/// A fake server for a test that starts a token refresh. It holds
/// [`crate::test_support::refresh_count_lock`] while it lives, so no other
/// refresh runs beside a test that checks the process-wide refresh count.
struct Refreshing {
    fake: FakeHttp,
    _serial: std::sync::MutexGuard<'static, ()>,
}

impl std::ops::Deref for Refreshing {
    type Target = FakeHttp;
    fn deref(&self) -> &FakeHttp {
        &self.fake
    }
}

fn refreshing(refresh: Reply) -> Refreshing {
    let serial = crate::test_support::refresh_count_lock();
    let fake = FakeHttp::start(move |s, _| {
        if s.path == TOKEN_PATH {
            return refresh.clone();
        }
        if bearer(s) == Some(&format!("Bearer {NEW_ACCESS}")) {
            Reply::json(200, &body_100_55())
        } else {
            Reply::json(401, &json!({"error": "expired"}))
        }
    });
    Refreshing {
        fake,
        _serial: serial,
    }
}

fn token_answer() -> Value {
    json!({
        "access_token": NEW_ACCESS,
        "refresh_token": NEW_REFRESH,
        "expires_in": 28_800,
        "token_type": "Bearer"
    })
}

// ---------------------------------------------------------------------------
// Issue #2019: an injected clock and a per-test refresh counter
// ---------------------------------------------------------------------------

/// The real socket limit for reads whose deadline is not under test, and
/// the longest a test waits on a channel. A hang cap only: no test asserts
/// on it, and a correct run never comes near it.
const HANG_GUARD: Duration = Duration::from_secs(120);

/// A request whose deadline is `HANG_GUARD`: only the fake clock can use
/// it up.
fn request_guarded<'a>(l: &'a Login, journals: &'a [PathBuf], allow_refresh: bool) -> Request<'a> {
    Request {
        timeout: HANG_GUARD,
        ..request(l, journals, allow_refresh)
    }
}

/// `read_with` on a clock that does not move, with a counter of the test's
/// own; the counter must be back at zero when the read returns.
fn read_frozen(ep: &Endpoints, req: &Request) -> Result<Direct, &'static str> {
    let clock = VirtualClock::shared();
    let counter = AtomicUsize::new(0);
    let r = read_with(ep, req, &Ctx::new(&*clock, &counter, HANG_GUARD));
    assert_eq!(counter.load(Ordering::SeqCst), 0, "a refresh count leaked");
    r
}

/// `read_frozen` against `fake` with a `HANG_GUARD` deadline.
fn run_with(fake: &FakeHttp, l: &Login, allow_refresh: bool) -> Result<Direct, &'static str> {
    let journals = l.journals();
    read_frozen(
        &endpoints(fake),
        &request_guarded(l, &journals, allow_refresh),
    )
}

/// A gate a fake server's handler blocks on until the test opens it. The
/// sender drops on unwind, which also opens it, and a handler never waits
/// past `HANG_GUARD`.
struct Gate {
    open: mpsc::SyncSender<()>,
    wait: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl Gate {
    fn new() -> Self {
        let (open, wait) = mpsc::sync_channel(64);
        Self {
            open,
            wait: Arc::new(Mutex::new(wait)),
        }
    }

    fn waiter(&self) -> impl Fn() + Send + Sync + 'static {
        let wait = Arc::clone(&self.wait);
        move || {
            let _ = wait.lock().unwrap().recv_timeout(HANG_GUARD);
        }
    }

    fn release(&self) {
        let _ = self.open.try_send(());
    }

    /// Releases the gate when dropped. Declared after the fake server, it
    /// is dropped before it, so a failed assertion frees the held handler
    /// and the server's drop does not wait out [`HANG_GUARD`].
    fn release_on_drop(&self) -> Release<'_> {
        Release(self)
    }
}

struct Release<'a>(&'a Gate);

impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

// ---------------------------------------------------------------------------
// Constants and endpoint validation
// ---------------------------------------------------------------------------

#[test]
fn constants_are_the_documented_values() {
    assert_eq!(API_BASE_ENV, "AMPLIHACK_PROFILE_USAGE_API_BASE");
    assert_eq!(TOKEN_URL_ENV, "AMPLIHACK_PROFILE_USAGE_TOKEN_URL");
    assert_eq!(DEFAULT_API_BASE, "https://api.anthropic.com");
    assert_eq!(
        DEFAULT_TOKEN_URL,
        "https://platform.claude.com/v1/oauth/token"
    );
    assert_eq!(USAGE_PATH, "/api/oauth/usage");
    assert_eq!(BETA, "oauth-2025-04-20");
    assert_eq!(USER_AGENT, "amplihack-profile-usage");
    assert!(!CLIENT_ID.is_empty());
    assert_eq!(MAX_BODY, 64 * 1024);
    assert_eq!(REFRESH_MARGIN_SECS, 60);
    assert_eq!(MAX_TOKEN, 4096);
    assert_eq!(MAX_EXPIRES_IN, 31_536_000);
    assert_eq!(MAX_STEP, Duration::from_secs(10));
    assert_eq!(MIN_STEP, Duration::from_millis(500));
    assert_eq!(REFRESH_BUDGET, Duration::from_secs(3));
}

#[test]
fn base_ok_accepts_https_and_loopback_http() {
    for ok in [
        "https://api.anthropic.com",
        "https://relay.example.com:8443/prefix",
        "http://127.0.0.1",
        "http://127.0.0.1:8080",
        "http://localhost:9",
        "http://[::1]:4000",
        "http://localhost/v1/oauth/token",
    ] {
        assert!(base_ok(ok), "{ok}");
    }
}

#[test]
fn base_ok_refuses_everything_else() {
    for bad in [
        "",
        "http://api.anthropic.com",
        "http://localhost.evil.com",
        "http://localhost.evil.com:80",
        "http://127.0.0.1@evil.com",
        "http://127.0.0.1.evil.com",
        "http://127.0.0.2",
        "HTTP://localhost",
        "Https://api.anthropic.com",
        "ftp://localhost",
        "file:///etc/passwd",
        "https://api.anthropic.com\n",
        "https://api.anthropic.com/\nx",
        "https://api.anthropic.com x",
        " https://api.anthropic.com",
        "https://user@api.anthropic.com",
        "https://api.anthropic.com/?q=1",
        "https://api.anthropic.com/#frag",
        "https://api.anthropic.com/\u{7f}",
        "localhost:8080",
    ] {
        assert!(!base_ok(bad), "{bad:?} must be refused");
    }
}

#[test]
fn resolve_uses_the_defaults_when_unset_or_empty() {
    for (api, token) in [(None, None), (Some(OsStr::new("")), Some(OsStr::new("")))] {
        let (ep, warnings) = Endpoints::resolve(api, token);
        assert!(warnings.is_empty());
        assert_eq!(ep.usage_url(), "https://api.anthropic.com/api/oauth/usage");
        assert_eq!(ep.token_url(), DEFAULT_TOKEN_URL);
        assert!(ep.https_only());
    }
}

#[test]
fn resolve_warns_without_repeating_a_refused_value() {
    let secretish = "http://evil.example.com/SHOULD-NOT-ECHO";
    let (ep, warnings) =
        Endpoints::resolve(Some(OsStr::new(secretish)), Some(OsStr::new(secretish)));
    assert_eq!(ep.usage_url(), "https://api.anthropic.com/api/oauth/usage");
    assert_eq!(ep.token_url(), DEFAULT_TOKEN_URL);
    assert_eq!(
        warnings,
        [
            "warning: ignored AMPLIHACK_PROFILE_USAGE_API_BASE (use https:// or a loopback http:// address); using the default",
            "warning: ignored AMPLIHACK_PROFILE_USAGE_TOKEN_URL (use https:// or a loopback http:// address); using the default",
        ]
    );
    for w in &warnings {
        assert!(!w.contains("evil") && !w.contains("SHOULD-NOT-ECHO"), "{w}");
    }
}

#[test]
fn resolve_strips_a_trailing_slash_from_the_base() {
    let (ep, _) = Endpoints::resolve(Some(OsStr::new("http://127.0.0.1:9/")), None);
    assert_eq!(ep.usage_url(), "http://127.0.0.1:9/api/oauth/usage");
}

#[test]
fn https_is_required_unless_both_endpoints_are_loopback() {
    let (both, _) = Endpoints::resolve(
        Some(OsStr::new("http://127.0.0.1:9")),
        Some(OsStr::new("http://localhost:9/t")),
    );
    assert!(!both.https_only());
    let (one, _) = Endpoints::resolve(Some(OsStr::new("http://127.0.0.1:9")), None);
    assert!(one.https_only(), "the default token URL is not loopback");
}

// ---------------------------------------------------------------------------
// Tokens and credentials
// ---------------------------------------------------------------------------

#[test]
fn token_parse_accepts_printable_ascii_up_to_4096_bytes() {
    assert!(Token::parse("a").is_some());
    assert!(Token::parse(&"x".repeat(MAX_TOKEN)).is_some());
    assert!(Token::parse("sk-ant-oat01-AbC_d.e~f+/=!").is_some());
}

#[test]
fn token_parse_refuses_empty_long_or_unprintable_tokens() {
    for bad in [
        String::new(),
        "x".repeat(MAX_TOKEN + 1),
        "has space".into(),
        "tab\there".into(),
        "nl\n".into(),
        "cr\r".into(),
        "nul\0".into(),
        "del\u{7f}".into(),
        "caf\u{e9}".into(),
    ] {
        assert!(Token::parse(&bad).is_none(), "{bad:?}");
    }
}

#[test]
fn token_and_creds_debug_are_redacted() {
    let t = Token::parse(ACCESS).unwrap();
    let shown = format!("{t:?}");
    assert!(!shown.contains(ACCESS), "{shown}");
    assert!(shown.contains("<redacted>"), "{shown}");
    let l = login();
    let c = Creds::parse(&fs::read(l.path()).unwrap()).expect("parsed");
    let shown = format!("{c:?} {c:#?}");
    assert!(
        !shown.contains(ACCESS) && !shown.contains(REFRESH),
        "{shown}"
    );
}

#[test]
fn creds_parse_needs_a_usable_access_token() {
    for bad in [
        b"not json".to_vec(),
        b"[]".to_vec(),
        br#"{}"#.to_vec(),
        br#"{"claudeAiOauth":{}}"#.to_vec(),
        br#"{"claudeAiOauth":{"accessToken":""}}"#.to_vec(),
        br#"{"claudeAiOauth":{"accessToken":"a b"}}"#.to_vec(),
        br#"{"claudeAiOauth":{"accessToken":17}}"#.to_vec(),
        br#"{"primaryApiKey":"sk-not-oauth"}"#.to_vec(),
    ] {
        assert!(
            Creds::parse(&bad).is_none(),
            "{}",
            String::from_utf8_lossy(&bad)
        );
    }
    assert!(Creds::parse(br#"{"claudeAiOauth":{"accessToken":"abc"}}"#).is_some());
}

#[test]
fn expires_in_must_be_a_whole_number_in_range() {
    assert_eq!(expires_in(&json!(1)), Some(1));
    assert_eq!(expires_in(&json!(28_800)), Some(28_800));
    assert_eq!(expires_in(&json!(MAX_EXPIRES_IN)), Some(MAX_EXPIRES_IN));
    for bad in [
        json!(0),
        json!(-1),
        json!(MAX_EXPIRES_IN + 1),
        json!(3600.5),
        json!("3600"),
        json!(null),
        json!(i64::MAX),
        json!(u64::MAX),
    ] {
        assert_eq!(expires_in(&bad), None, "{bad}");
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

fn status(code: u16) -> &'static str {
    let resp = ureq::Response::new(code, "X", "secret body FAKE-ACCESS").unwrap();
    map_err(&ureq::Error::Status(code, resp))
}

#[test]
fn statuses_map_to_fixed_reasons() {
    assert_eq!(status(401), "http 401");
    assert_eq!(status(403), "http 403");
    assert_eq!(status(429), "http 429");
    assert_eq!(status(301), "http 3xx");
    assert_eq!(status(307), "http 3xx");
    assert_eq!(status(400), "http 4xx");
    assert_eq!(status(404), "http 4xx");
    assert_eq!(status(500), "http 5xx");
    assert_eq!(status(503), "http 5xx");
}

#[test]
fn a_refused_connection_is_transport() {
    let e = ureq::get(&format!("{}/x", closed_base()))
        .timeout(Duration::from_secs(2))
        .call()
        .unwrap_err();
    assert_eq!(map_err(&e), "transport");
}

#[test]
fn reasons_are_the_documented_texts() {
    assert_eq!(reason::NO_FILE, "no credentials file");
    assert_eq!(reason::REFUSED, "credentials refused");
    assert_eq!(reason::NO_TOKEN, "no oauth token");
    assert_eq!(reason::EXPIRED, "token expired");
    assert_eq!(reason::TIMEOUT, "timeout");
    assert_eq!(reason::TRANSPORT, "transport");
    assert_eq!(reason::UNPARSEABLE, "unparseable");
    assert_eq!(reason::CREDITS_ON, "usage credits on");
    assert_eq!(reason::REFRESH_BUSY, "refresh busy");
    assert_eq!(reason::REFRESH_REFUSED, "refresh refused");
    assert_eq!(reason::REFRESH_FAILED, "refresh failed");
    assert_eq!(reason::REFRESH_NOT_SAVED, "refresh not saved");
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[test]
fn a_read_sends_one_get_with_the_documented_headers() {
    let fake = FakeHttp::start(|_, _| Reply::json(200, &body_100_55()));
    let l = login();
    let r = run(&fake, &l, true).expect("read");
    assert_eq!(r.state, UsageState::SessionExhausted);
    assert_eq!(r.session.as_ref().map(|w| w.used_percentage), Some(100.0));
    assert_eq!(
        r.week_all.as_ref().map(|w| w.used_percentage),
        Some(55.0),
        "the weekly figure as reported"
    );
    let seen = fake.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let s = &seen[0];
    assert_eq!(s.method, "GET");
    assert_eq!(s.path, USAGE_PATH);
    assert_eq!(bearer(s), Some(&*format!("Bearer {ACCESS}")));
    assert_eq!(s.header("anthropic-beta"), Some(BETA));
    assert_eq!(s.header("accept"), Some("application/json"));
    assert_eq!(s.header("user-agent"), Some(USER_AGENT));
    assert!(s.body.is_empty());
    assert_eq!(posts(&fake), 0);
}

#[test]
fn the_token_never_appears_in_the_url() {
    let fake = FakeHttp::start(|_, _| Reply::json(200, &body_100_55()));
    let l = login();
    run(&fake, &l, true).unwrap();
    for s in fake.seen() {
        assert!(!s.path.contains("FAKE"), "{}", s.path);
    }
}

#[test]
fn a_missing_file_is_no_credentials_file() {
    let fake = FakeHttp::start(|_, _| Reply::json(200, &body_100_55()));
    let l = login();
    fs::remove_file(l.path()).unwrap();
    assert_eq!(run(&fake, &l, true), Err(reason::NO_FILE));
    assert!(fake.seen().is_empty());
}

#[test]
fn a_symlinked_or_group_writable_file_is_refused() {
    let fake = FakeHttp::start(|_, _| Reply::json(200, &body_100_55()));
    let l = login();
    let real = l.dir.join("real.json");
    fs::rename(l.path(), &real).unwrap();
    std::os::unix::fs::symlink(&real, l.path()).unwrap();
    assert_eq!(run(&fake, &l, true), Err(reason::REFUSED));
    fs::remove_file(l.path()).unwrap();
    fs::rename(&real, l.path()).unwrap();
    for mode in [0o620, 0o602, 0o666] {
        fs::set_permissions(l.path(), fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(run(&fake, &l, true), Err(reason::REFUSED), "{mode:o}");
    }
    assert!(fake.seen().is_empty());
}

#[test]
fn a_file_over_64_kib_is_refused() {
    let fake = FakeHttp::start(|_, _| Reply::json(200, &body_100_55()));
    let l = login();
    let mut doc = l.doc();
    doc["padding"] = json!("x".repeat(70 * 1024));
    fs::write(l.path(), doc.to_string()).unwrap();
    assert_eq!(run(&fake, &l, true), Err(reason::REFUSED));
}

#[test]
fn no_usable_token_is_no_oauth_token() {
    let fake = FakeHttp::start(|_, _| Reply::json(200, &body_100_55()));
    let l = login();
    for body in [
        "not json",
        r#"{"primaryApiKey":"sk-x"}"#,
        r#"{"claudeAiOauth":{"accessToken":"with space"}}"#,
    ] {
        fs::write(l.path(), body).unwrap();
        assert_eq!(run(&fake, &l, true), Err(reason::NO_TOKEN), "{body}");
    }
    assert!(fake.seen().is_empty());
}

#[test]
fn server_errors_map_to_their_class_and_are_not_retried() {
    for (code, want) in [
        (500, "http 5xx"),
        (403, "http 403"),
        (429, "http 429"),
        (404, "http 4xx"),
    ] {
        let fake = FakeHttp::start(move |_, _| Reply::raw(code, "FAKE-ACCESS body text"));
        let l = login();
        assert_eq!(run(&fake, &l, true), Err(want), "{code}");
        assert_eq!(gets(&fake).len(), 1, "{code}: no retry");
        assert_eq!(posts(&fake), 0, "{code}: no refresh");
    }
}

#[test]
fn a_redirect_is_not_followed() {
    let fake = FakeHttp::start(|s, _| {
        if s.path == USAGE_PATH {
            Reply::raw(307, "").header("Location", "/elsewhere")
        } else {
            Reply::json(200, &body_100_55())
        }
    });
    let l = login();
    assert_eq!(run(&fake, &l, true), Err("http 3xx"));
    assert_eq!(fake.count("GET", "/elsewhere"), 0);
}

#[test]
fn a_body_over_64_kib_is_unparseable() {
    let fake = FakeHttp::start(|_, _| {
        let mut b = body_100_55();
        b["padding"] = json!("x".repeat(MAX_BODY as usize));
        Reply::json(200, &b)
    });
    let l = login();
    assert_eq!(run(&fake, &l, true), Err(reason::UNPARSEABLE));
}

#[test]
fn a_body_that_is_not_an_object_or_has_no_window_is_unparseable() {
    for body in [json!([]), json!("x"), json!({}), json!({"five_hour": null})] {
        let b = body.clone();
        let fake = FakeHttp::start(move |_, _| Reply::json(200, &b));
        let l = login();
        assert_eq!(run(&fake, &l, true), Err(reason::UNPARSEABLE), "{body}");
    }
    let fake = FakeHttp::start(|_, _| Reply::raw(200, "<html>not json</html>"));
    let l = login();
    assert_eq!(run(&fake, &l, true), Err(reason::UNPARSEABLE));
}

/// Issue #1856: credits on with a readable window is a success. The
/// windows are the direct read's; the shape names keys and flags only.
#[test]
fn credits_on_with_a_window_gives_the_windows_and_the_shape() {
    let fake = FakeHttp::start(|_, _| {
        let mut b = body_100_55();
        b["extra_usage"] = json!({
            "is_enabled": true,
            "monthly_limit": 987_654,
            "used_credits": 4321.5,
            "currency": "SENTINELCUR"
        });
        Reply::json(200, &b)
    });
    let l = login();
    let d = run_direct(&fake, &l, true).expect("credits on with windows is read");
    assert_eq!(d.report.state, UsageState::SessionExhausted);
    assert_eq!(
        d.report.week_all.as_ref().map(|w| w.used_percentage),
        Some(55.0)
    );
    let shape = d.credits_on.as_ref().expect("credits on carries the shape");
    assert_eq!(
        shape.line(),
        "extra_usage: object; keys: currency, is_enabled, monthly_limit, used_credits; is_enabled: bool true"
    );
    for leak in ["SENTINEL", "987654", "987_654", "4321.5", ACCESS] {
        assert!(!shape.line().contains(leak), "{leak} in {}", shape.line());
        assert!(!format!("{d:?}").contains(leak), "{leak} in Debug");
    }
}

/// Credits off: no shape.
#[test]
fn credits_off_gives_no_shape() {
    let fake = FakeHttp::start(|_, _| Reply::json(200, &body_100_55()));
    let l = login();
    let d = run_direct(&fake, &l, true).expect("read");
    assert_eq!(d.credits_on, None);
    assert_eq!(d.report.state, UsageState::SessionExhausted);
}

/// Credits on with neither window readable is still its own reason.
#[test]
fn credits_on_with_no_window_is_its_own_reason() {
    let fake = FakeHttp::start(|_, _| {
        let mut b = body_100_55();
        b["five_hour"] = json!(null);
        b["seven_day"] = json!({"utilization": "high"});
        b["extra_usage"] = json!({"is_enabled": true, "monthly_limit": 5000});
        Reply::json(200, &b)
    });
    let l = login();
    assert_eq!(run(&fake, &l, true), Err(reason::CREDITS_ON));
}

#[test]
fn a_weekly_only_answer_follows_1818() {
    let fake = FakeHttp::start(|_, _| {
        let mut b = body_100_55();
        b["five_hour"] = json!(null);
        Reply::json(200, &b)
    });
    let l = login();
    let r = run(&fake, &l, true).unwrap();
    assert_eq!(r.state, UsageState::Ok);
    assert_eq!(r.session, None);
    assert!(!r.session_limit);
    assert_eq!(r.week_all.map(|w| w.used_percentage), Some(55.0));
}

// Deadline tests (issue #1962, round 3; issue #2019). Outcomes are decided
// on the injected `VirtualClock` or by a server that is held at a gate until
// the read has returned, never by how long the read took in real time. The
// time bounds themselves are pinned on the pure step functions below.

/// Virtual time left after the initial 401 beyond `REFRESH_BUDGET`.
const V_SEND_MARGIN: Duration = Duration::from_secs(4);
/// The virtual read deadline for the refresh tests:
/// `REFRESH_BUDGET + V_SEND_MARGIN`.
const V_REFRESH_T: Duration = REFRESH_BUDGET.saturating_add(V_SEND_MARGIN);

#[test]
fn get_step_mirrors_the_agent_formula() {
    assert_eq!(get_step(V_REFRESH_T), Duration::from_secs(6));
    assert_eq!(get_step(Duration::from_secs(60)), MAX_STEP);
    assert_eq!(get_step(Duration::from_millis(1200)), MIN_STEP);
    assert_eq!(get_step(Duration::ZERO), MIN_STEP);
}

/// The POST step is the time left, capped at `MAX_STEP`: never more than
/// is left (SEC-9), and never cut down to the `GET` agent's step.
#[test]
fn post_step_is_the_time_left_capped_at_max_step() {
    assert_eq!(post_step(Duration::ZERO), Duration::ZERO);
    assert_eq!(post_step(REFRESH_BUDGET), REFRESH_BUDGET);
    assert_eq!(post_step(V_REFRESH_T), V_REFRESH_T);
    assert_eq!(post_step(MAX_STEP), MAX_STEP);
    assert_eq!(post_step(Duration::from_secs(60)), MAX_STEP);
    assert_eq!(post_step(HANG_GUARD), MAX_STEP);
    for ms in [
        0, 1, 499, 500, 2_999, 3_000, 6_050, 9_999, 10_000, 10_001, 120_000,
    ] {
        let left = Duration::from_millis(ms);
        assert!(post_step(left) <= left, "{left:?}");
        assert!(post_step(left) <= MAX_STEP, "{left:?}");
    }
}

/// A refresh sent with the budget left has a step of at least the budget,
/// and with `T` = `V_REFRESH_T` its step outlasts the `GET` agent's: the
/// property `a_refresh_with_the_budget_left_saves_the_rotated_token` used
/// to show by timing a token answer between the two.
#[test]
fn a_refresh_step_is_not_cut_to_the_get_step() {
    assert!(post_step(V_REFRESH_T) > get_step(V_REFRESH_T));
    assert_eq!(
        post_step(V_REFRESH_T) - get_step(V_REFRESH_T),
        Duration::from_secs(1)
    );
    for ms in (REFRESH_BUDGET.as_millis() as u64..=MAX_STEP.as_millis() as u64).step_by(250) {
        let left = Duration::from_millis(ms);
        assert!(post_step(left) >= REFRESH_BUDGET, "{left:?}");
        assert!(post_step(left) >= get_step(left), "{left:?}");
    }
}

/// A server that answers nothing in time gives `timeout`. The server is held
/// at a gate until the read has returned, so the read cannot have waited for
/// the answer, however long the host took to reach the socket timeout.
#[test]
fn a_stalled_server_is_a_timeout_within_the_deadline() {
    const STALL_T: Duration = Duration::from_secs(2);
    let gate = Gate::new();
    let hold = gate.waiter();
    let fake = FakeHttp::start(move |_, _| {
        hold();
        Reply::json(200, &body_100_55())
    });
    let _release = gate.release_on_drop();
    let l = login();
    let journals = l.journals();
    let mut req = request(&l, &journals, true);
    req.timeout = STALL_T;
    let r = read(&endpoints(&fake), &req);
    gate.release();
    assert_eq!(r, Err(reason::TIMEOUT));
}

/// The deadline covers the whole read, not each request. With `T` =
/// `V_REFRESH_T` on a fake clock:
/// - the 401 comes at once, virtual time has not moved, so the refresh is
///   admitted against `REFRESH_BUDGET`;
/// - the token server's answer moves virtual time to `T`, the end of the one
///   deadline, and the rotated tokens are still saved;
/// - the retry `GET` is then refused before any request is sent. Per-request
///   deadlines would have given it a fresh `T`, sent it, and read the `200`
///   the fake server gives the new token.
///
/// Issue #2019: an earlier form left the retry `RETRY_LEFT` (300 ms) and held
/// it at a gate until its socket timed out. Under host load its connect could
/// overrun those 300 ms of real time first and end as `transport`. Here no
/// outcome depends on how long anything takes in real time.
#[test]
fn a_401_refresh_and_retry_share_one_deadline() {
    let t = V_REFRESH_T;
    assert!(t >= REFRESH_BUDGET + V_SEND_MARGIN);
    let clock = VirtualClock::shared();
    let c = Arc::clone(&clock);
    let fake = FakeHttp::scripted(move |s, _| {
        if s.path == TOKEN_PATH {
            c.advance(t);
            Reply::json(200, &token_answer())
        } else if bearer(s) == Some(&format!("Bearer {NEW_ACCESS}")) {
            Reply::json(200, &body_100_55())
        } else {
            Reply::json(401, &json!({"error": "expired"}))
        }
    });
    let l = login();
    let journals = l.journals();
    let mut req = request(&l, &journals, true);
    req.timeout = t;
    let counter = AtomicUsize::new(0);
    let r = read_with(
        &endpoints(&fake),
        &req,
        &Ctx::new(&*clock, &counter, HANG_GUARD).with_transport(&fake),
    );
    assert_eq!(r, Err(reason::TIMEOUT));
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    // The refresh was answered in time, so the rotated tokens are saved
    // even though no time was left for the second GET.
    assert_eq!(posts(&fake), 1);
    let gets = gets(&fake);
    assert_eq!(gets.len(), 1, "the retry GET must not be sent");
    assert_eq!(l.doc()["claudeAiOauth"]["accessToken"], NEW_ACCESS);
    assert_eq!(l.doc()["claudeAiOauth"]["refreshToken"], NEW_REFRESH);
    assert_eq!(l.entries(), [CREDS]);
}

/// With enough time for all three, the same slow sequence reads: each
/// answer takes 300 ms of virtual time, 900 ms in all, inside 5 s.
#[test]
fn a_slow_refresh_within_the_deadline_still_reads() {
    let d = Duration::from_millis(300);
    let clock = VirtualClock::shared();
    let c = Arc::clone(&clock);
    let fake = FakeHttp::scripted(move |s, _| {
        c.advance(d);
        if s.path == TOKEN_PATH {
            Reply::json(200, &token_answer())
        } else if bearer(s) == Some(&format!("Bearer {NEW_ACCESS}")) {
            Reply::json(200, &body_100_55())
        } else {
            Reply::json(401, &json!({"error": "expired"}))
        }
    });
    let l = login();
    let journals = l.journals();
    let mut req = request(&l, &journals, true);
    req.timeout = Duration::from_secs(5);
    let counter = AtomicUsize::new(0);
    let r = read_with(
        &endpoints(&fake),
        &req,
        &Ctx::new(&*clock, &counter, HANG_GUARD).with_transport(&fake),
    )
    .expect("read after a slow refresh");
    assert_eq!(r.report.week_all.map(|w| w.used_percentage), Some(55.0));
    assert_eq!(clock.elapsed(), d * 3, "three answers, 300 ms each");
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

/// The refresh budget is measured on the injected clock: a 401 that takes
/// all but just under `REFRESH_BUDGET` of the read's virtual deadline sends
/// no refresh, and the read is `timeout` with the file untouched.
#[test]
fn the_refresh_budget_is_checked_against_the_injected_clock() {
    let t = V_REFRESH_T;
    let clock = VirtualClock::shared();
    let c = Arc::clone(&clock);
    let fake = FakeHttp::scripted(move |s, _| {
        if s.path == TOKEN_PATH {
            Reply::json(200, &token_answer())
        } else {
            c.advance(t - REFRESH_BUDGET + Duration::from_millis(1));
            Reply::json(401, &json!({"error": "expired"}))
        }
    });
    let l = login();
    let before = fs::read(l.path()).unwrap();
    let journals = l.journals();
    let mut req = request(&l, &journals, true);
    req.timeout = t;
    let counter = AtomicUsize::new(0);
    assert_eq!(
        read_with(
            &endpoints(&fake),
            &req,
            &Ctx::new(&*clock, &counter, HANG_GUARD).with_transport(&fake)
        ),
        Err(reason::TIMEOUT)
    );
    assert_eq!(posts(&fake), 0);
    assert_eq!(fs::read(l.path()).unwrap(), before);
    assert_eq!(l.entries(), [CREDS]);
    assert!(!l.lock().exists());
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

/// A deadline used up on the injected clock before a request starts sends
/// that request no more: the second `GET` is never made.
#[test]
fn a_deadline_spent_on_the_injected_clock_sends_nothing_more() {
    let clock = VirtualClock::shared();
    let c = Arc::clone(&clock);
    let fake = FakeHttp::scripted(move |s, _| {
        if s.path == TOKEN_PATH {
            c.advance(HANG_GUARD);
            Reply::json(200, &token_answer())
        } else if bearer(s) == Some(&format!("Bearer {NEW_ACCESS}")) {
            Reply::json(200, &body_100_55())
        } else {
            Reply::json(401, &json!({"error": "expired"}))
        }
    });
    let l = login();
    let journals = l.journals();
    let req = request_guarded(&l, &journals, true);
    let counter = AtomicUsize::new(0);
    assert_eq!(
        read_with(
            &endpoints(&fake),
            &req,
            &Ctx::new(&*clock, &counter, HANG_GUARD).with_transport(&fake)
        ),
        Err(reason::TIMEOUT)
    );
    assert_eq!(posts(&fake), 1);
    assert_eq!(
        gets(&fake).len(),
        1,
        "no GET starts once the deadline is spent"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

/// A 401 that leaves less than `REFRESH_BUDGET` of the read sends no
/// refresh: the read is `timeout`, the old refresh token is never offered
/// to the server, the file is untouched and the lock released. The read
/// deadline `SHORT_T` is itself below the budget and the 401 comes at once,
/// so the read ends early and `SHORT_T` bounds it with all of it as slack.
#[test]
fn a_refresh_with_too_little_time_left_is_not_sent() {
    const SHORT_T: Duration = REFRESH_BUDGET.saturating_sub(Duration::from_secs(1));
    assert!(SHORT_T < REFRESH_BUDGET);
    let fake = FakeHttp::scripted(|s, _| {
        if s.path == TOKEN_PATH {
            Reply::json(200, &token_answer())
        } else {
            Reply::json(401, &json!({"error": "expired"}))
        }
    });
    let l = login();
    let before = fs::read(l.path()).unwrap();
    let journals = l.journals();
    let mut req = request(&l, &journals, true);
    req.timeout = SHORT_T;
    let clock = VirtualClock::shared();
    let counter = AtomicUsize::new(0);
    assert_eq!(
        read_with(
            &endpoints(&fake),
            &req,
            &Ctx::new(&*clock, &counter, HANG_GUARD).with_transport(&fake)
        ),
        Err(reason::TIMEOUT)
    );
    // The read ended without waiting out any of its virtual deadline.
    assert!(clock.elapsed() < SHORT_T, "{:?}", clock.elapsed());
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert_eq!(posts(&fake), 0);
    assert_eq!(fs::read(l.path()).unwrap(), before);
    assert_eq!(l.entries(), [CREDS]);
    assert!(!l.lock().exists());
}

/// A refresh sent with the budget left is waited for and its rotated
/// tokens saved. The token answer takes longer, in virtual time, than the
/// `GET` agent's step for `V_REFRESH_T`; the step property itself is proved
/// on the pure step functions (`a_refresh_step_is_not_cut_to_the_get_step`).
#[test]
fn a_refresh_with_the_budget_left_saves_the_rotated_token() {
    let token_d = get_step(V_REFRESH_T) + Duration::from_millis(50);
    assert!(token_d < MAX_STEP);
    let clock = VirtualClock::shared();
    let c = Arc::clone(&clock);
    let fake = FakeHttp::scripted(move |s, _| {
        if s.path == TOKEN_PATH {
            c.advance(token_d);
            Reply::json(200, &token_answer())
        } else if bearer(s) == Some(&format!("Bearer {NEW_ACCESS}")) {
            Reply::json(200, &body_100_55())
        } else {
            Reply::json(401, &json!({"error": "expired"}))
        }
    });
    let l = login();
    let journals = l.journals();
    let req = request_guarded(&l, &journals, true);
    let counter = AtomicUsize::new(0);
    let r = read_with(
        &endpoints(&fake),
        &req,
        &Ctx::new(&*clock, &counter, HANG_GUARD).with_transport(&fake),
    )
    .expect("read after a slow refresh");
    assert_eq!(r.report.week_all.map(|w| w.used_percentage), Some(55.0));
    // The retry carried the rotated token.
    let retries = gets(&fake);
    assert_eq!(retries.len(), 2, "one 401 and one retry");
    assert_eq!(
        bearer(&retries[1]),
        Some(format!("Bearer {NEW_ACCESS}").as_str())
    );
    assert_eq!(posts(&fake), 1);
    assert_eq!(l.doc()["claudeAiOauth"]["accessToken"], NEW_ACCESS);
    assert_eq!(l.doc()["claudeAiOauth"]["refreshToken"], NEW_REFRESH);
    assert_eq!(l.entries(), [CREDS]);
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

/// A refresh POST that is sent but answered only after the read's deadline
/// still ends at the deadline: the file is untouched and the lock released.
/// The token server may rotate the refresh token before the transport loses
/// its answer, so the old one on file may be retired: the reason is `refresh not saved`, which prints
/// the warning to log in again, not `refresh failed`. The 401 comes at once
/// on a virtual clock, so the POST is sent with `V_SEND_MARGIN` to spare.
/// The injected transport then advances that clock and returns a transport
/// timeout after recording the POST. No socket or real timeout is involved.
#[test]
fn a_refresh_answered_after_the_deadline_is_not_saved() {
    let t = V_REFRESH_T;
    let clock = VirtualClock::shared();
    let c = Arc::clone(&clock);
    let fake = FakeHttp::scripted(move |s, _| {
        if s.path == TOKEN_PATH {
            c.advance(t);
            Reply::timed_out()
        } else if bearer(s) == Some(&format!("Bearer {NEW_ACCESS}")) {
            Reply::json(200, &body_100_55())
        } else {
            Reply::json(401, &json!({"error": "expired"}))
        }
    });
    let l = login();
    let before = fs::read(l.path()).unwrap();
    let journals = l.journals();
    let mut req = request(&l, &journals, true);
    req.timeout = t;
    let counter = AtomicUsize::new(0);
    let r = read_with(
        &endpoints(&fake),
        &req,
        &Ctx::new(&*clock, &counter, HANG_GUARD).with_transport(&fake),
    );
    assert_eq!(r, Err(reason::REFRESH_NOT_SAVED));
    assert!(super::usage_source::not_saved(reason::REFRESH_NOT_SAVED));
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert_eq!(posts(&fake), 1);
    assert_eq!(fs::read(l.path()).unwrap(), before);
    assert_eq!(l.entries(), [CREDS]);
    assert!(!l.lock().exists());
}

#[test]
fn a_refused_connection_reads_transport() {
    let l = login();
    let base = closed_base();
    let (ep, _) = Endpoints::resolve(Some(OsStr::new(&base)), Some(OsStr::new(&base)));
    let journals = l.journals();
    assert_eq!(
        read_frozen(&ep, &request(&l, &journals, true)),
        Err(reason::TRANSPORT)
    );
}

// ---------------------------------------------------------------------------
// Refresh
// ---------------------------------------------------------------------------

#[test]
fn a_401_refreshes_once_writes_back_and_reads_again() {
    let fake = refreshing(Reply::json(200, &token_answer()));
    let l = login();
    let before = now_ms();
    let r = run_with(&fake, &l, true)
        .expect("read after refresh")
        .report;
    let after = now_ms();
    assert_eq!(r.week_all.map(|w| w.used_percentage), Some(55.0));

    assert_eq!(posts(&fake), 1);
    let post = fake
        .seen()
        .into_iter()
        .find(|s| s.method == "POST")
        .unwrap();
    let sent: Value = serde_json::from_str(&post.body).expect("JSON body");
    assert_eq!(sent["grant_type"], "refresh_token");
    assert_eq!(sent["refresh_token"], REFRESH);
    assert_eq!(sent["client_id"], CLIENT_ID);
    assert!(
        post.header("content-type")
            .unwrap_or("")
            .starts_with("application/json")
    );

    let g = gets(&fake);
    assert_eq!(g.len(), 2);
    assert_eq!(bearer(&g[0]), Some(&*format!("Bearer {ACCESS}")));
    assert_eq!(bearer(&g[1]), Some(&*format!("Bearer {NEW_ACCESS}")));

    let doc = l.doc();
    let o = &doc["claudeAiOauth"];
    assert_eq!(o["accessToken"], NEW_ACCESS);
    assert_eq!(o["refreshToken"], NEW_REFRESH);
    let exp = o["expiresAt"].as_i64().expect("integer milliseconds");
    assert!(
        exp >= before + 28_800_000 && exp <= after + 28_800_000,
        "{exp}"
    );
    assert_eq!(o["scopes"], json!(["user:inference", "user:profile"]));
    assert_eq!(o["subscriptionType"], "max");
    assert_eq!(doc["mcpOAuth"], json!({"keep": true}));
    let mode = fs::metadata(l.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(l.entries(), [CREDS], "no lock or temporary file left");
}

#[test]
fn a_token_expiring_within_60_seconds_is_refreshed_before_the_first_get() {
    let fake = refreshing(Reply::json(200, &token_answer()));
    let l = login_expiring_in(30_000);
    run_with(&fake, &l, true).expect("read");
    let g = gets(&fake);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(bearer(&g[0]), Some(&*format!("Bearer {NEW_ACCESS}")));
    assert_eq!(posts(&fake), 1);
}

#[test]
fn a_refresh_answer_with_no_refresh_token_keeps_the_old_one() {
    let mut answer = token_answer();
    answer.as_object_mut().unwrap().remove("refresh_token");
    let fake = refreshing(Reply::json(200, &answer));
    let l = login();
    run_with(&fake, &l, true).expect("read");
    assert_eq!(l.doc()["claudeAiOauth"]["refreshToken"], REFRESH);
    assert_eq!(l.doc()["claudeAiOauth"]["accessToken"], NEW_ACCESS);
}

#[test]
fn a_second_401_after_the_refresh_is_not_refreshed_again() {
    let fake = FakeHttp::start(|s, _| {
        if s.path == TOKEN_PATH {
            Reply::json(200, &token_answer())
        } else {
            Reply::json(401, &json!({}))
        }
    });
    let l = login();
    assert_eq!(run_with(&fake, &l, true), Err("http 401"));
    assert_eq!(posts(&fake), 1);
    assert_eq!(gets(&fake).len(), 2);
}

#[test]
fn a_held_lock_is_refresh_busy_and_left_alone() {
    let fake = refreshing(Reply::json(200, &token_answer()));
    let l = login();
    fs::create_dir(l.lock()).unwrap();
    // `run_with` checks the test's own counter: it went up before the
    // `mkdir` and must be back at zero, or it would never reach zero.
    assert_eq!(run_with(&fake, &l, true), Err(reason::REFRESH_BUSY));
    assert_eq!(posts(&fake), 0);
    assert!(
        l.lock().is_dir(),
        "a lock held by someone else is never broken"
    );
    assert_eq!(l.doc()["claudeAiOauth"]["accessToken"], ACCESS);
}

#[test]
fn a_set_stop_flag_takes_no_lock_and_sends_no_post() {
    // SR-9: once the rotator is stopping, a 401 does not start a refresh.
    let fake = refreshing(Reply::json(200, &token_answer()));
    let l = login();
    let cancel = AtomicBool::new(true);
    let journals = l.journals();
    let req = Request {
        cancel: &cancel,
        ..request_guarded(&l, &journals, true)
    };
    let clock = VirtualClock::shared();
    let counter = AtomicUsize::new(0);
    assert_eq!(
        read_with(
            &endpoints(&fake),
            &req,
            &Ctx::new(&*clock, &counter, HANG_GUARD)
        ),
        Err(reason::REFRESH_BUSY)
    );
    // A count left raised by this read would keep the rotator's wait from
    // ever ending.
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert_eq!(posts(&fake), 0);
    assert!(!l.lock().exists(), "no lock was taken");
    assert_eq!(l.entries(), [CREDS]);
    assert_eq!(l.doc()["claudeAiOauth"]["accessToken"], ACCESS);
}

#[test]
fn a_swap_journal_is_refresh_busy() {
    for which in 0..2 {
        let fake = refreshing(Reply::json(200, &token_answer()));
        let l = login();
        let journal = &l.journals()[which];
        fs::create_dir_all(journal.parent().unwrap()).unwrap();
        fs::write(journal, "{}").unwrap();
        assert_eq!(
            run(&fake, &l, true),
            Err(reason::REFRESH_BUSY),
            "{journal:?}"
        );
        assert_eq!(posts(&fake), 0);
        assert!(!l.lock().exists(), "the lock is released");
    }
}

#[test]
fn allow_refresh_false_never_posts() {
    let fake = refreshing(Reply::json(200, &token_answer()));
    let l = login();
    assert_eq!(run(&fake, &l, false), Err("http 401"));
    let l = login_expiring_in(-1000);
    assert_eq!(run(&fake, &l, false), Err(reason::EXPIRED));
    assert_eq!(posts(&fake), 0);
    assert!(!l.lock().exists());
}

#[test]
fn a_token_changed_under_the_lock_is_used_without_a_post() {
    // Claude refreshes the login between the first read and the lock: the
    // first GET's 401 handler rewrites the file, as Claude would.
    let l = login();
    let path = l.path();
    let fake = FakeHttp::start(move |s, n| {
        if s.path == TOKEN_PATH {
            return Reply::json(200, &token_answer());
        }
        if n == 0 {
            let mut doc: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            doc["claudeAiOauth"]["accessToken"] = json!(NEW_ACCESS);
            doc["claudeAiOauth"]["refreshToken"] = json!(NEW_REFRESH);
            fs::write(&path, doc.to_string()).unwrap();
            return Reply::json(401, &json!({}));
        }
        if bearer(s) == Some(&format!("Bearer {NEW_ACCESS}")) {
            Reply::json(200, &body_100_55())
        } else {
            Reply::json(401, &json!({}))
        }
    });
    run(&fake, &l, true).expect("read with Claude's new token");
    assert_eq!(posts(&fake), 0, "no refresh request");
    assert_eq!(gets(&fake).len(), 2);
    assert!(!l.lock().exists());
}

/// A 200 means the server issued new tokens and may have retired the old
/// refresh token; an answer that cannot be used is `refresh not saved`.
#[test]
fn an_unusable_new_token_is_refresh_not_saved_and_the_file_is_untouched() {
    for bad in [
        json!("has space"),
        json!(""),
        json!("x".repeat(MAX_TOKEN + 1)),
        json!(42),
    ] {
        let mut answer = token_answer();
        answer["access_token"] = bad.clone();
        let fake = refreshing(Reply::json(200, &answer));
        let l = login();
        let before = fs::read(l.path()).unwrap();
        assert_eq!(
            run(&fake, &l, true),
            Err(reason::REFRESH_NOT_SAVED),
            "{bad}"
        );
        assert_eq!(fs::read(l.path()).unwrap(), before);
        assert_eq!(l.entries(), [CREDS]);
    }
}

#[test]
fn a_bad_expires_in_is_refresh_not_saved_and_the_file_is_untouched() {
    for bad in [
        json!(0),
        json!(-1),
        json!(MAX_EXPIRES_IN + 1),
        json!("3600"),
        json!(3600.5),
    ] {
        let mut answer = token_answer();
        answer["expires_in"] = bad.clone();
        let fake = refreshing(Reply::json(200, &answer));
        let l = login();
        let before = fs::read(l.path()).unwrap();
        assert_eq!(
            run(&fake, &l, true),
            Err(reason::REFRESH_NOT_SAVED),
            "{bad}"
        );
        assert_eq!(fs::read(l.path()).unwrap(), before);
    }
}

/// A 3xx or 4xx status the server turned the refresh away with leaves the
/// old refresh token in force: `refresh failed`.
#[test]
fn a_failed_refresh_request_leaves_the_file_and_releases_the_lock() {
    for reply in [
        Reply::json(400, &json!({"error": "invalid_grant"})),
        Reply::json(401, &json!({})),
        Reply::json(429, &json!({})),
        Reply::raw(302, "").header("Location", "/elsewhere"),
    ] {
        let fake = refreshing(reply);
        let l = login();
        let before = fs::read(l.path()).unwrap();
        assert_eq!(run(&fake, &l, true), Err(reason::REFRESH_FAILED));
        assert_eq!(fs::read(l.path()).unwrap(), before);
        assert_eq!(l.entries(), [CREDS]);
        assert_eq!(fake.count("GET", "/elsewhere"), 0);
    }
}

/// A 5xx can come from a proxy in front of the token server after the
/// server received and handled the request (Cloudflare's 524 is exactly
/// that), and a 2xx other than 200 is the server accepting it: the old
/// refresh token may be retired, so these are `refresh not saved`, with the
/// warning to log in again. The file is left as it was.
#[test]
fn a_5xx_or_other_2xx_refresh_answer_is_refresh_not_saved() {
    for code in [500, 502, 503, 504, 520, 524, 201, 204] {
        let fake = refreshing(Reply::json(code, &json!({})));
        let l = login();
        let before = fs::read(l.path()).unwrap();
        assert_eq!(
            run(&fake, &l, true),
            Err(reason::REFRESH_NOT_SAVED),
            "{code}"
        );
        assert!(super::usage_source::not_saved(reason::REFRESH_NOT_SAVED));
        assert_eq!(fs::read(l.path()).unwrap(), before, "{code}");
        assert_eq!(l.entries(), [CREDS], "{code}");
    }
}

#[test]
fn refresh_status_splits_turned_away_from_maybe_handled_by_class() {
    for code in [300, 302, 399, 400, 401, 403, 404, 429, 499] {
        assert_eq!(refresh_status(code), reason::REFRESH_FAILED, "{code}");
    }
    for code in [100, 201, 204, 299, 500, 502, 504, 520, 524, 599, 600] {
        assert_eq!(refresh_status(code), reason::REFRESH_NOT_SAVED, "{code}");
    }
}

/// A lookup that has not answered by its bound is a `TimedOut` error at the
/// bound, not a wait for the system resolver.
#[test]
fn a_stalled_lookup_gives_up_at_its_bound() {
    // The resolver answers only once the test lets it, after the lookup has
    // returned, so the result cannot be the resolver's answer.
    let (release, stall) = held_resolver();
    let r = bounded_lookup(Instant::now() + Duration::from_millis(200), stall)("token.invalid:443");
    drop(release);
    assert_eq!(r.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
}

/// A resolver that answers only after the returned sender is dropped.
fn held_resolver() -> (
    mpsc::Sender<()>,
    impl Fn(&str) -> std::io::Result<Vec<std::net::SocketAddr>> + Send + Sync + Clone + 'static,
) {
    let (release, held) = mpsc::channel::<()>();
    let held = Arc::new(Mutex::new(held));
    let stall = move |_: &str| -> std::io::Result<Vec<std::net::SocketAddr>> {
        let _ = held.lock().unwrap().recv_timeout(HANG_GUARD);
        Ok(vec!["127.0.0.1:1".parse().unwrap()])
    };
    (release, stall)
}

#[test]
fn a_prompt_lookup_is_returned_as_is() {
    let addr: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
    let quick = move |n: &str| -> std::io::Result<Vec<std::net::SocketAddr>> {
        assert_eq!(n, "token.invalid:443");
        Ok(vec![addr])
    };
    let r = bounded_lookup(Instant::now() + Duration::from_secs(5), quick)("token.invalid:443");
    assert_eq!(r.unwrap(), vec![addr]);
}

/// A resolver that stalls past the connect timeout. Without the bound,
/// `ureq` 2.12.1 would wait for the lookup and then fail the connect
/// deadline it fixed before the lookup as an I/O timeout (`ErrorKind::Io`),
/// which is `refresh not saved` and a false warning to log in again. With
/// the bound the error is `ErrorKind::Dns`, nothing is sent, and the reason
/// is `refresh failed`.
#[test]
fn a_stalled_token_host_lookup_is_refresh_failed_not_refresh_not_saved() {
    let (release, stall) = held_resolver();
    let connect = Duration::from_millis(600);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(connect)
        .timeout(Duration::from_secs(10))
        .resolver(bounded_lookup(
            Instant::now() + connect.saturating_sub(MIN_STEP),
            stall,
        ))
        .build();
    let err = agent
        .post("http://token.invalid/v1/oauth/token")
        .send_string("{}")
        .unwrap_err();
    drop(release);
    assert!(matches!(&err, ureq::Error::Transport(t) if t.kind() == ureq::ErrorKind::Dns));
    assert_eq!(post_failure(&err), reason::REFRESH_FAILED);
    assert!(!super::usage_source::not_saved(post_failure(&err)));
}

#[test]
fn a_200_that_is_not_json_is_refresh_not_saved_and_leaves_the_file() {
    let fake = refreshing(Reply::raw(200, "not json"));
    let l = login();
    let before = fs::read(l.path()).unwrap();
    assert_eq!(run(&fake, &l, true), Err(reason::REFRESH_NOT_SAVED));
    assert_eq!(fs::read(l.path()).unwrap(), before);
    assert_eq!(l.entries(), [CREDS]);
    assert!(!l.lock().exists());
}

/// A token URL that refuses the connection never received the request, so
/// the old refresh token still works: `refresh failed`, no warning.
#[test]
fn a_refused_token_connection_is_refresh_failed() {
    let token = format!("{}{TOKEN_PATH}", closed_base());
    let fake = FakeHttp::start(|_, _| Reply::json(401, &json!({})));
    let (ep, warnings) =
        Endpoints::resolve(Some(OsStr::new(&fake.base())), Some(OsStr::new(&token)));
    assert!(warnings.is_empty(), "{warnings:?}");
    let l = login();
    let before = fs::read(l.path()).unwrap();
    let journals = l.journals();
    let req = request(&l, &journals, true);
    assert_eq!(read_frozen(&ep, &req), Err(reason::REFRESH_FAILED));
    assert!(!super::usage_source::not_saved(reason::REFRESH_FAILED));
    assert_eq!(fs::read(l.path()).unwrap(), before);
    assert!(!l.lock().exists());
}

#[test]
fn a_directory_others_can_write_is_refresh_refused_without_a_post() {
    let fake = refreshing(Reply::json(200, &token_answer()));
    let l = login();
    fs::set_permissions(&l.dir, fs::Permissions::from_mode(0o777)).unwrap();
    let r = run(&fake, &l, true);
    fs::set_permissions(&l.dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(r, Err(reason::REFRESH_REFUSED));
    assert_eq!(posts(&fake), 0);
    assert!(!l.lock().exists());
}

#[test]
fn a_read_only_directory_is_refresh_refused_and_leaves_nothing() {
    let fake = refreshing(Reply::json(200, &token_answer()));
    let l = login();
    fs::set_permissions(&l.dir, fs::Permissions::from_mode(0o500)).unwrap();
    // Root ignores the mode bits; the check means nothing there.
    let writable = fs::write(l.dir.join("probe"), b"").is_ok();
    let r = run(&fake, &l, true);
    fs::set_permissions(&l.dir, fs::Permissions::from_mode(0o700)).unwrap();
    if writable {
        return;
    }
    assert_eq!(r, Err(reason::REFRESH_REFUSED));
    assert_eq!(posts(&fake), 0, "nothing is sent that could not be saved");
    assert_eq!(l.entries(), [CREDS], "no temporary file and no lock");
}

#[test]
fn errors_and_results_never_hold_a_token() {
    let fake = refreshing(Reply::json(400, &json!({"echo": ACCESS})));
    let l = login();
    let shown = format!("{:?}", run(&fake, &l, true));
    for secret in [ACCESS, REFRESH, NEW_ACCESS, NEW_REFRESH, "FAKE-"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    let fake = FakeHttp::start(|_, _| Reply::json(200, &body_100_55()));
    let shown = format!("{:?}", run(&fake, &l, true));
    assert!(!shown.contains("FAKE-"), "{shown}");
}

/// A token server that reports each POST on `arrived`, then holds its
/// answer until `gate` opens.
fn refreshing_held(arrived: mpsc::SyncSender<()>, gate: &Gate) -> FakeHttp {
    let hold = gate.waiter();
    let arrived = Mutex::new(arrived);
    FakeHttp::start(move |s, _| {
        if s.path == TOKEN_PATH {
            let _ = arrived.lock().unwrap().try_send(());
            hold();
            return Reply::json(200, &token_answer());
        }
        if bearer(s) == Some(&format!("Bearer {NEW_ACCESS}")) {
            Reply::json(200, &body_100_55())
        } else {
            Reply::json(401, &json!({"error": "expired"}))
        }
    })
}

#[test]
fn the_refresh_counter_covers_a_refresh_in_progress() {
    let (arrived, posted) = mpsc::sync_channel(8);
    let gate = Gate::new();
    let fake = refreshing_held(arrived, &gate);
    let _release = gate.release_on_drop();
    let l = login();
    let dir = l.dir.clone();
    let root = l.root.clone();
    let ep = endpoints(&fake);
    let clock = VirtualClock::shared();
    let counter = Arc::new(AtomicUsize::new(0));
    let (c, n) = (Arc::clone(&clock), Arc::clone(&counter));
    let reader = std::thread::spawn(move || {
        let journals = vec![root.join(".auto-switch.d/journal.json")];
        let req = Request {
            creds_dir: &dir,
            journals: &journals,
            allow_refresh: true,
            cancel: &NO_CANCEL,
            timeout: HANG_GUARD,
            now_ms: now_ms(),
        };
        read_with(&ep, &req, &Ctx::new(&*c, &n, HANG_GUARD)).is_ok()
    });
    posted
        .recv_timeout(HANG_GUARD)
        .expect("the refresh POST reached the token server");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "the refresh holding the lock is counted"
    );
    assert!(l.lock().is_dir(), "the refresh holds Claude's lock");
    gate.release();
    assert!(reader.join().unwrap(), "the read completes once answered");
    assert_eq!(counter.load(Ordering::SeqCst), 0, "released with the lock");
    assert!(!l.lock().exists());
}

/// SR-9: the read counts its refresh on the process-wide counter that
/// `Ctx::system`, `refreshes_in_progress` and `wait_for_refreshes` use, so the
/// rotator's wait still covers it. Only the clock and transport guard differ
/// from production. The shared lock excludes other process-wide refreshes.
#[test]
fn read_uses_the_process_wide_counter() {
    let _serial = crate::test_support::refresh_count_lock();
    let (arrived, posted) = mpsc::sync_channel(8);
    let gate = Gate::new();
    let fake = refreshing_held(arrived, &gate);
    let _release = gate.release_on_drop();
    let l = login();
    let dir = l.dir.clone();
    let root = l.root.clone();
    let ep = endpoints(&fake);
    let reader = std::thread::spawn(move || {
        let clock = VirtualClock::new();
        let journals = vec![root.join(".auto-switch.d/journal.json")];
        let req = Request {
            creds_dir: &dir,
            journals: &journals,
            allow_refresh: true,
            cancel: &NO_CANCEL,
            timeout: HANG_GUARD,
            now_ms: now_ms(),
        };
        read_with(&ep, &req, &Ctx::new(&clock, refresh_counter(), HANG_GUARD)).is_ok()
    });
    posted
        .recv_timeout(HANG_GUARD)
        .expect("the refresh POST reached the token server");
    assert_eq!(refreshes_in_progress(), 1, "the held refresh is counted");
    assert!(l.lock().is_dir(), "the refresh holds Claude's lock");
    gate.release();
    assert!(reader.join().unwrap());
    assert_eq!(refreshes_in_progress(), 0, "released with the lock");
    assert!(!l.lock().exists());
}

/// `refresh_expired_with` refreshes an expired login on the injected clock
/// and counter, and leaves the counter at zero.
#[test]
fn refresh_expired_with_uses_the_injected_counter() {
    let fake = refreshing(Reply::json(200, &token_answer()));
    let l = login_expiring_in(-1_000);
    let journals = l.journals();
    let req = request_guarded(&l, &journals, true);
    let clock = VirtualClock::shared();
    let counter = AtomicUsize::new(0);
    assert_eq!(
        refresh_expired_with(
            &endpoints(&fake),
            &req,
            &Ctx::new(&*clock, &counter, HANG_GUARD)
        ),
        Ok(true)
    );
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert_eq!(posts(&fake), 1);
    assert_eq!(l.doc()["claudeAiOauth"]["accessToken"], NEW_ACCESS);
    assert_eq!(l.entries(), [CREDS]);
}

#[test]
fn wait_for_refreshes_returns_at_once_when_none_run() {
    // A counter of the test's own, so refreshes in parallel tests cannot
    // hold it, and a zero limit on a clock that never moves: with nothing
    // running it is true without waiting at all.
    let clock = VirtualClock::shared();
    assert!(super::usage_direct::wait_for_refreshes_on(
        &AtomicUsize::new(0),
        Duration::ZERO,
        &*clock
    ));
    assert_eq!(clock.elapsed(), Duration::ZERO);
}

// ---------------------------------------------------------------------------
// Test isolation: a process started by a test never reaches a real endpoint
//
// Contract:
// - `Endpoints::resolve_with(get)` reads `API_BASE_ENV`, `TOKEN_URL_ENV` and
//   `AMPLIHACK_TEST_HOME_GUARD` through `get` (as `resolve` does with the
//   real environment).
// - `Endpoints::usable()` is false when the test guard is set and either
//   endpoint is not loopback; `read` then fails with `transport` without
//   opening a connection. Without the guard, or with loopback overrides,
//   it is true. This keeps every integration test that does not point the
//   reader at a fake on the `claude` fallback, as before #1844.
// ---------------------------------------------------------------------------

fn getter(
    pairs: &'static [(&'static str, &'static str)],
) -> impl Fn(&str) -> Option<std::ffi::OsString> {
    move |k| {
        pairs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| std::ffi::OsString::from(v))
    }
}

#[test]
fn under_the_test_guard_real_endpoints_are_never_used() {
    let (ep, _) = Endpoints::resolve_with(getter(&[("AMPLIHACK_TEST_HOME_GUARD", "1")]));
    assert!(!ep.usable());
    let (ep, _) = Endpoints::resolve_with(getter(&[
        ("AMPLIHACK_TEST_HOME_GUARD", "1"),
        ("AMPLIHACK_PROFILE_USAGE_API_BASE", "http://127.0.0.1:9"),
    ]));
    assert!(!ep.usable(), "the default token URL is still real");
    let (ep, _) = Endpoints::resolve_with(getter(&[
        ("AMPLIHACK_TEST_HOME_GUARD", "1"),
        ("AMPLIHACK_PROFILE_USAGE_API_BASE", "http://127.0.0.1:9"),
        ("AMPLIHACK_PROFILE_USAGE_TOKEN_URL", "http://localhost:9/t"),
    ]));
    assert!(ep.usable());
    let (ep, _) = Endpoints::resolve_with(getter(&[]));
    assert!(ep.usable());
}

#[test]
fn an_unusable_endpoint_fails_without_connecting() {
    let l = login();
    let (ep, _) = Endpoints::resolve_with(getter(&[("AMPLIHACK_TEST_HOME_GUARD", "1")]));
    let journals = l.journals();
    // The credentials directory does not exist: a read that got past the
    // endpoint check would fail on loading them (`no file`) before it could
    // connect, so `transport` proves it stopped before both.
    let missing = l.path().with_file_name("no-such-dir");
    let mut req = request(&l, &journals, true);
    req.creds_dir = &missing;
    let clock = VirtualClock::shared();
    let counter = AtomicUsize::new(0);
    assert_eq!(
        read_with(&ep, &req, &Ctx::new(&*clock, &counter, HANG_GUARD)),
        Err(reason::TRANSPORT)
    );
    assert_eq!(clock.elapsed(), Duration::ZERO);
    assert_eq!(l.entries(), [CREDS]);
}
