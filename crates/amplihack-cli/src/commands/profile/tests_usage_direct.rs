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
//! - `read(&Endpoints, &Request) -> Result<UsageReport, &'static str>`, with
//!   `Request { creds_dir, journals, allow_refresh, timeout, now_ms }`.
//!   Errors are the fixed texts in `reason`.
//! - `refreshes_in_progress() -> usize` counts refreshes holding a lock;
//!   `wait_for_refreshes(limit) -> bool` waits up to `limit` for it to
//!   reach 0.
//!
//! Every test uses a loopback fake server and fake tokens; nothing reads a
//! real credential or reaches a real endpoint.

use super::tests_fake_http::{FakeHttp, Reply, Seen, closed_base};
use super::usage_direct::{
    API_BASE_ENV, BETA, CLIENT_ID, Creds, DEFAULT_API_BASE, DEFAULT_TOKEN_URL, Endpoints, MAX_BODY,
    MAX_EXPIRES_IN, MAX_TOKEN, REFRESH_MARGIN_SECS, Request, TOKEN_URL_ENV, Token, USAGE_PATH,
    USER_AGENT, base_ok, expires_in, map_err, read, reason, refreshes_in_progress,
    wait_for_refreshes,
};
use super::usage_parse::UsageState;
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
        timeout: Duration::from_secs(5),
        now_ms: now_ms(),
    }
}

fn run(
    fake: &FakeHttp,
    l: &Login,
    allow_refresh: bool,
) -> Result<super::usage_parse::UsageReport, &'static str> {
    let journals = l.journals();
    read(&endpoints(fake), &request(l, &journals, allow_refresh))
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
fn refreshing(refresh: Reply) -> FakeHttp {
    FakeHttp::start(move |s, _| {
        if s.path == TOKEN_PATH {
            return refresh.clone();
        }
        if bearer(s) == Some(&format!("Bearer {NEW_ACCESS}")) {
            Reply::json(200, &body_100_55())
        } else {
            Reply::json(401, &json!({"error": "expired"}))
        }
    })
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

#[test]
fn credits_on_is_its_own_reason() {
    let fake = FakeHttp::start(|_, _| {
        let mut b = body_100_55();
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

#[test]
fn a_stalled_server_is_a_timeout_within_the_deadline() {
    let fake =
        FakeHttp::start(|_, _| Reply::json(200, &body_100_55()).after(Duration::from_secs(4)));
    let l = login();
    let journals = l.journals();
    let mut req = request(&l, &journals, true);
    req.timeout = Duration::from_secs(2);
    let start = Instant::now();
    assert_eq!(read(&endpoints(&fake), &req), Err(reason::TIMEOUT));
    assert!(
        start.elapsed() < Duration::from_millis(3500),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn a_refused_connection_reads_transport() {
    let l = login();
    let base = closed_base();
    let (ep, _) = Endpoints::resolve(Some(OsStr::new(&base)), Some(OsStr::new(&base)));
    let journals = l.journals();
    assert_eq!(
        read(&ep, &request(&l, &journals, true)),
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
    let r = run(&fake, &l, true).expect("read after refresh");
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
    run(&fake, &l, true).expect("read");
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
    run(&fake, &l, true).expect("read");
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
    assert_eq!(run(&fake, &l, true), Err("http 401"));
    assert_eq!(posts(&fake), 1);
    assert_eq!(gets(&fake).len(), 2);
}

#[test]
fn a_held_lock_is_refresh_busy_and_left_alone() {
    let fake = refreshing(Reply::json(200, &token_answer()));
    let l = login();
    fs::create_dir(l.lock()).unwrap();
    assert_eq!(run(&fake, &l, true), Err(reason::REFRESH_BUSY));
    assert_eq!(posts(&fake), 0);
    assert!(
        l.lock().is_dir(),
        "a lock held by someone else is never broken"
    );
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

#[test]
fn an_unusable_new_token_is_refresh_failed_and_the_file_is_untouched() {
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
        assert_eq!(run(&fake, &l, true), Err(reason::REFRESH_FAILED), "{bad}");
        assert_eq!(fs::read(l.path()).unwrap(), before);
        assert_eq!(l.entries(), [CREDS]);
    }
}

#[test]
fn a_bad_expires_in_is_refresh_failed_and_the_file_is_untouched() {
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
        assert_eq!(run(&fake, &l, true), Err(reason::REFRESH_FAILED), "{bad}");
        assert_eq!(fs::read(l.path()).unwrap(), before);
    }
}

#[test]
fn a_failed_refresh_request_leaves_the_file_and_releases_the_lock() {
    for reply in [
        Reply::json(400, &json!({"error": "invalid_grant"})),
        Reply::json(500, &json!({})),
        Reply::raw(200, "not json"),
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

#[test]
fn the_refresh_counter_covers_a_refresh_in_progress() {
    let fake = refreshing(Reply::json(200, &token_answer()).after(Duration::from_millis(1500)));
    let l = login();
    let dir = l.dir.clone();
    let root = l.root.clone();
    let base = fake.base();
    let token = fake.token_url();
    let reader = std::thread::spawn(move || {
        let (ep, _) = Endpoints::resolve(Some(OsStr::new(&base)), Some(OsStr::new(&token)));
        let journals = vec![root.join(".auto-switch.d/journal.json")];
        let req = Request {
            creds_dir: &dir,
            journals: &journals,
            allow_refresh: true,
            timeout: Duration::from_secs(5),
            now_ms: now_ms(),
        };
        read(&ep, &req).is_ok()
    });
    let start = Instant::now();
    while posts(&fake) == 0 && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        refreshes_in_progress() >= 1,
        "a refresh is holding the lock"
    );
    assert!(l.lock().is_dir());
    assert!(wait_for_refreshes(Duration::from_secs(5)));
    assert!(reader.join().unwrap());
    assert!(!l.lock().exists());
}

#[test]
fn wait_for_refreshes_returns_at_once_when_none_run() {
    let start = Instant::now();
    // Other tests may refresh in parallel; with none of ours running this
    // returns true well before the limit.
    assert!(wait_for_refreshes(Duration::from_secs(10)));
    assert!(start.elapsed() < Duration::from_secs(10));
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
    let start = Instant::now();
    assert_eq!(
        read(&ep, &request(&l, &journals, true)),
        Err(reason::TRANSPORT)
    );
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(l.entries(), [CREDS]);
}
