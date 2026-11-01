//! The direct usage read (issue #1844): one `GET /api/oauth/usage` with the
//! login's own OAuth token, refreshing the token at most once under Claude's
//! `.oauth_refresh.lock`.
//!
//! The token is held in memory only, in [`Token`], which has no `Display`
//! and a redacting `Debug`. It is sent only in the `Authorization` header
//! and the refresh body, and never appears in a URL, an error, a log line or
//! a child's environment. Every failure is one of the fixed texts in
//! [`reason`]; [`map_err`] is the only code that looks at a `ureq` error, and
//! it keeps only the status code or the error kind. Nothing here reads the
//! environment except [`Endpoints::from_env`], and nothing prints.

use super::auto_switch::kit::{
    CREDS, euid, read_bounded_nofollow, release_refresh_lock_at, take_refresh_lock_at,
    writable_by_others,
};
use super::usage_parse::UsageReport;
use super::usage_request::{Limits, parse_limits};
use amplihack_utils::secure_files::PrivateTemp;
use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use ureq::Error as HttpError;
use ureq::ErrorKind;

pub(super) const API_BASE_ENV: &str = "AMPLIHACK_PROFILE_USAGE_API_BASE";
pub(super) const TOKEN_URL_ENV: &str = "AMPLIHACK_PROFILE_USAGE_TOKEN_URL";
/// Set by the test harness (#1699). With it, a read never goes to an
/// endpoint that is not loopback.
const TEST_GUARD_ENV: &str = "AMPLIHACK_TEST_HOME_GUARD";
pub(super) const DEFAULT_API_BASE: &str = "https://api.anthropic.com";
/// The token endpoint Claude Code 2.1.316 uses.
pub(super) const DEFAULT_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub(super) const USAGE_PATH: &str = "/api/oauth/usage";
/// The beta header Claude Code sends with OAuth requests.
pub(super) const BETA: &str = "oauth-2025-04-20";
pub(super) const USER_AGENT: &str = "amplihack-profile-usage";
/// Claude Code's public OAuth client id: the `CLIENT_ID` of the production
/// OAuth configuration in Claude Code 2.1.316, next to the
/// `platform.claude.com` URLs. It identifies the application, not a user.
pub(super) const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// Response bodies and the credentials file past this are not used.
pub(super) const MAX_BODY: u64 = 64 * 1024;
/// A token this close to its expiry is refreshed before it is used.
pub(super) const REFRESH_MARGIN_SECS: i64 = 60;
pub(super) const MAX_TOKEN: usize = 4096;
/// One year: no token lives longer.
pub(super) const MAX_EXPIRES_IN: i64 = 31_536_000;
/// The longest connect and read step. The whole read, every request in
/// it, has the one probe deadline ([`Deadline`]).
pub(super) const MAX_STEP: Duration = Duration::from_secs(10);
pub(super) const MIN_STEP: Duration = Duration::from_millis(500);
/// A refresh POST is sent only with at least this much of the read's
/// deadline left, and its connect and read steps are at least this long. A
/// refresh can retire the old refresh token (RFC 6749 section 6, RFC 9700
/// section 4.14.2); one given up on before its answer arrives can leave the
/// login with no working refresh token. Skipping a refresh costs one
/// `timeout` row; abandoning one can cost the login.
pub(super) const REFRESH_BUDGET: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(20);

/// The fixed failure texts. None holds a path, a token or a response body.
pub(super) mod reason {
    pub const NO_FILE: &str = "no credentials file";
    pub const REFUSED: &str = "credentials refused";
    pub const NO_TOKEN: &str = "no oauth token";
    pub const EXPIRED: &str = "token expired";
    pub const TIMEOUT: &str = "timeout";
    pub const TRANSPORT: &str = "transport";
    pub const UNPARSEABLE: &str = "unparseable";
    pub const CREDITS_ON: &str = "usage credits on";
    pub const REFRESH_BUSY: &str = "refresh busy";
    pub const REFRESH_REFUSED: &str = "refresh refused";
    pub const REFRESH_FAILED: &str = "refresh failed";
    pub const REFRESH_NOT_SAVED: &str = "refresh not saved";
    pub const HTTP_401: &str = "http 401";
    pub const HTTP_403: &str = "http 403";
    pub const HTTP_429: &str = "http 429";
    pub const HTTP_3XX: &str = "http 3xx";
    pub const HTTP_4XX: &str = "http 4xx";
    pub const HTTP_5XX: &str = "http 5xx";
}

/// Refreshes holding a lock right now, in this process.
static REFRESHING: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

/// The two URLs a read uses, checked once per command or cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Endpoints {
    api: String,
    token: String,
    usable: bool,
}

impl Endpoints {
    /// The endpoints from the two variables; a refused value gives a fixed
    /// warning and the default. Empty counts as unset.
    #[cfg(test)]
    pub(super) fn resolve(api: Option<&OsStr>, token: Option<&OsStr>) -> (Self, Vec<String>) {
        Self::build(api, token, false)
    }

    /// As [`Endpoints::resolve`], reading the variables and the test guard
    /// through `get`.
    pub(super) fn resolve_with(get: impl Fn(&str) -> Option<OsString>) -> (Self, Vec<String>) {
        let guard = get(TEST_GUARD_ENV).is_some_and(|v| !v.is_empty());
        let api = get(API_BASE_ENV);
        let token = get(TOKEN_URL_ENV);
        Self::build(api.as_deref(), token.as_deref(), guard)
    }

    /// [`Endpoints::resolve_with`] with the real environment.
    pub(super) fn from_env() -> (Self, Vec<String>) {
        Self::resolve_with(|k| std::env::var_os(k))
    }

    fn build(api: Option<&OsStr>, token: Option<&OsStr>, guard: bool) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut pick = |value: Option<&OsStr>, var: &str, default: &str| -> String {
            match value.filter(|v| !v.is_empty()) {
                None => default.to_string(),
                Some(v) => match v.to_str().filter(|s| base_ok(s)) {
                    Some(s) => s.to_string(),
                    None => {
                        warnings.push(refused_warning(var));
                        default.to_string()
                    }
                },
            }
        };
        let api = pick(api, API_BASE_ENV, DEFAULT_API_BASE)
            .trim_end_matches('/')
            .to_string();
        let token = pick(token, TOKEN_URL_ENV, DEFAULT_TOKEN_URL);
        let both_loopback = loopback(&api) && loopback(&token);
        let ep = Self {
            api,
            token,
            usable: !guard || both_loopback,
        };
        (ep, warnings)
    }

    pub(super) fn usage_url(&self) -> String {
        let mut url = self.api.clone();
        url.push_str(USAGE_PATH);
        url
    }

    pub(super) fn token_url(&self) -> &str {
        &self.token
    }

    /// Plain `http` is allowed only when both endpoints are loopback.
    pub(super) fn https_only(&self) -> bool {
        !(loopback(&self.api) && loopback(&self.token))
    }

    /// False under the test guard unless both endpoints are loopback: a
    /// test process never reaches a real endpoint.
    pub(super) fn usable(&self) -> bool {
        self.usable
    }
}

/// Never repeats the refused value.
fn refused_warning(var: &str) -> String {
    format!(
        "warning: ignored {var} (use https:// or a loopback http:// address); using the default"
    )
}

/// A value an endpoint may take: `https://<host>...`, or `http://` with the
/// host exactly `127.0.0.1`, `[::1]` or `localhost`, each with an optional
/// port and path. The scheme is matched case-sensitively. Whitespace,
/// control characters, `@`, `?` and `#` are refused anywhere.
pub(super) fn base_ok(s: &str) -> bool {
    if s.chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '@' | '?' | '#'))
    {
        return false;
    }
    if let Some(rest) = s.strip_prefix("https://") {
        let (host, port) = split_authority(rest);
        return !host.is_empty() && port_ok(port);
    }
    s.strip_prefix("http://").is_some_and(|rest| {
        let (host, port) = split_authority(rest);
        is_loopback_host(host) && port_ok(port)
    })
}

/// The host and the port text of `rest`, the URL after its scheme.
fn split_authority(rest: &str) -> (&str, Option<&str>) {
    let authority = rest.split('/').next().unwrap_or("");
    if let Some(v6) = authority.strip_prefix('[') {
        return match v6.split_once(']') {
            Some((inner, tail)) => {
                let host = &authority[..inner.len() + 2];
                match tail {
                    "" => (host, None),
                    t => match t.strip_prefix(':') {
                        Some(p) => (host, Some(p)),
                        None => ("", None),
                    },
                }
            }
            None => ("", None),
        };
    }
    match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    }
}

fn port_ok(port: Option<&str>) -> bool {
    port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()))
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "[::1]" | "localhost")
}

/// An accepted endpoint whose host is loopback, by either scheme.
fn loopback(url: &str) -> bool {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"));
    rest.is_some_and(|r| is_loopback_host(split_authority(r).0))
}

/// SR-7: the host `ureq` parsed from `url` agrees with the text check, so a
/// URL the two read differently is never used.
fn parsed_agrees(req: &ureq::Request, url: &str) -> bool {
    let Ok(parsed) = req.request_url() else {
        return false;
    };
    let host = parsed.host();
    let shown = if host.contains(':') && !host.starts_with('[') {
        let mut h = String::from("[");
        h.push_str(host);
        h.push(']');
        h
    } else {
        host.to_string()
    };
    let plain = parsed.scheme() == "http";
    match (plain, loopback(url)) {
        (true, _) => is_loopback_host(&shown),
        (false, true) => is_loopback_host(&shown),
        (false, false) => !is_loopback_host(&shown),
    }
}

// ---------------------------------------------------------------------------
// Tokens and the credentials file
// ---------------------------------------------------------------------------

/// An OAuth token: 1 to 4096 bytes, each a printable ASCII character other
/// than a space. No `Display`, and its `Debug` is redacted.
pub(super) struct Token(String);

impl Token {
    pub(super) fn parse(s: &str) -> Option<Self> {
        let ok =
            (1..=MAX_TOKEN).contains(&s.len()) && s.bytes().all(|b| (0x21..=0x7e).contains(&b));
        ok.then(|| Self(s.to_string()))
    }

    fn header(&self) -> String {
        let mut h = String::from("Bearer ");
        h.push_str(&self.0);
        h
    }

    fn same(&self, other: &Self) -> bool {
        self.0 == other.0
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// The login in a `.credentials.json`: its OAuth tokens, the expiry in
/// milliseconds, and the whole document, so a refresh keeps every other
/// field. `Debug` is redacted.
pub(super) struct Creds {
    access: Token,
    refresh: Option<Token>,
    expires_at_ms: Option<i64>,
    whole: Value,
}

impl Creds {
    /// `claudeAiOauth.accessToken` is required; `refreshToken` and
    /// `expiresAt` are used when they can be.
    pub(super) fn parse(raw: &[u8]) -> Option<Self> {
        let whole: Value = serde_json::from_slice(raw).ok()?;
        let oauth = whole.get("claudeAiOauth")?.as_object()?;
        let access = Token::parse(oauth.get("accessToken")?.as_str()?)?;
        let refresh = oauth
            .get("refreshToken")
            .and_then(Value::as_str)
            .and_then(Token::parse);
        let expires_at_ms = oauth.get("expiresAt").and_then(Value::as_i64);
        Some(Self {
            access,
            refresh,
            expires_at_ms,
            whole,
        })
    }
}

impl std::fmt::Debug for Creds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Creds(<redacted>)")
    }
}

/// `expires_in` from a token answer: a JSON integer from 1 to one year.
pub(super) fn expires_in(v: &Value) -> Option<i64> {
    if !(v.is_i64() || v.is_u64()) {
        return None;
    }
    v.as_i64().filter(|s| (1..=MAX_EXPIRES_IN).contains(s))
}

/// The credentials in `dir`, read without following a symlink and only when
/// the file is the user's own and no one else can write it.
fn load(dir: &Path) -> Result<Creds, &'static str> {
    match read_bounded_nofollow(&dir.join(CREDS), MAX_BODY) {
        Ok(None) => Err(reason::NO_FILE),
        Ok(Some((raw, meta))) => {
            if meta.uid() != euid() || writable_by_others(&meta) || meta.mode() & 0o020 != 0 {
                return Err(reason::REFUSED);
            }
            Creds::parse(&raw).ok_or(reason::NO_TOKEN)
        }
        Err(_) => Err(reason::REFUSED),
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The fixed reason for a `ureq` error: the status class, or the transport
/// error kind. The only code that looks at a `ureq::Error`; it never reads
/// a body, a URL or a message.
pub(super) fn map_err(e: &ureq::Error) -> &'static str {
    match e {
        HttpError::Status(code, _) => status_reason(*code),
        HttpError::Transport(t) => {
            let io_timeout = std::error::Error::source(t)
                .and_then(|s| s.downcast_ref::<std::io::Error>())
                .is_some_and(|io| timed_out(io.kind()));
            if t.kind() == ErrorKind::Io && io_timeout {
                reason::TIMEOUT
            } else {
                reason::TRANSPORT
            }
        }
    }
}

fn timed_out(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

/// The reason for a status other than 200.
fn status_reason(code: u16) -> &'static str {
    match code {
        401 => reason::HTTP_401,
        403 => reason::HTTP_403,
        429 => reason::HTTP_429,
        300..=399 => reason::HTTP_3XX,
        400..=499 => reason::HTTP_4XX,
        500..=599 => reason::HTTP_5XX,
        200..=299 => reason::UNPARSEABLE,
        _ => reason::TRANSPORT,
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// One read: the profile directory whose login is used, the swap journals
/// that forbid a refresh while they exist, whether a refresh is allowed,
/// the stop flag that forbids a new refresh once set, the deadline for the
/// whole read and the time now.
pub(super) struct Request<'a> {
    pub(super) creds_dir: &'a Path,
    pub(super) journals: &'a [PathBuf],
    pub(super) allow_refresh: bool,
    /// `PROBE_CANCEL` in production; tests give their own flag.
    pub(super) cancel: &'a AtomicBool,
    /// The whole read, refresh included, ends within this time.
    pub(super) timeout: Duration,
    pub(super) now_ms: i64,
}

/// The end of one read, fixed when it starts. ureq's own `timeout` starts
/// again on every request, so each request is given only the time left:
/// the GET, the refresh POST and the second GET together take at most the
/// read's one deadline, not one deadline each.
#[derive(Debug, Clone, Copy)]
struct Deadline(Instant);

impl Deadline {
    fn after(timeout: Duration) -> Self {
        let now = Instant::now();
        Self(now.checked_add(timeout).unwrap_or(now))
    }

    /// The time left, or `timeout` once none is: no request starts then.
    fn left(self) -> Result<Duration, &'static str> {
        let left = self.0.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(reason::TIMEOUT)
        } else {
            Ok(left)
        }
    }
}

/// The usage of the login in `req.creds_dir`. At most two GETs and one
/// refresh POST, all within `req.timeout` from the start of the read; a 429
/// is not retried and a redirect is not followed.
pub(super) fn read(ep: &Endpoints, req: &Request<'_>) -> Result<UsageReport, &'static str> {
    let deadline = Deadline::after(req.timeout);
    if !ep.usable() {
        return Err(reason::TRANSPORT);
    }
    let creds = load(req.creds_dir)?;
    let agent = agent(ep, req.timeout);
    let now_s = req.now_ms.div_euclid(1000);
    let margin_ms = REFRESH_MARGIN_SECS * 1000;
    let due = creds
        .expires_at_ms
        .is_some_and(|at| at <= req.now_ms.saturating_add(margin_ms));
    if due {
        if req.allow_refresh {
            let fresh = refresh(ep, req, &creds.access, deadline)?;
            return get_usage(&agent, ep, &fresh, now_s, deadline);
        }
        if creds.expires_at_ms.is_some_and(|at| at <= req.now_ms) {
            return Err(reason::EXPIRED);
        }
    }
    match get_usage(&agent, ep, &creds.access, now_s, deadline) {
        Err(r) if r == reason::HTTP_401 && req.allow_refresh => {
            let fresh = refresh(ep, req, &creds.access, deadline)?;
            get_usage(&agent, ep, &fresh, now_s, deadline)
        }
        other => other,
    }
}

fn agent(ep: &Endpoints, timeout: Duration) -> ureq::Agent {
    let step = MAX_STEP
        .min(timeout.saturating_sub(Duration::from_secs(1)))
        .max(MIN_STEP);
    agent_with_step(ep, timeout, step)
}

fn agent_with_step(ep: &Endpoints, timeout: Duration, step: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(step)
        .timeout_read(step)
        .timeout_write(step)
        .timeout(timeout)
        .redirects(0)
        .https_only(ep.https_only())
        .user_agent(USER_AGENT)
        .build()
}

/// At most [`MAX_BODY`] bytes of a 200 answer; a longer one is unparseable.
fn body(resp: ureq::Response) -> Result<Vec<u8>, &'static str> {
    let mut buf = Vec::new();
    match resp.into_reader().take(MAX_BODY + 1).read_to_end(&mut buf) {
        Ok(_) if buf.len() as u64 > MAX_BODY => Err(reason::UNPARSEABLE),
        Ok(_) => Ok(buf),
        Err(io) if timed_out(io.kind()) => Err(reason::TIMEOUT),
        Err(_) => Err(reason::TRANSPORT),
    }
}

fn get_usage(
    agent: &ureq::Agent,
    ep: &Endpoints,
    token: &Token,
    now_s: i64,
    deadline: Deadline,
) -> Result<UsageReport, &'static str> {
    let url = ep.usage_url();
    let req = agent
        .get(&url)
        .timeout(deadline.left()?)
        .set("Authorization", &token.header())
        .set("anthropic-beta", BETA)
        .set("Accept", "application/json");
    if !parsed_agrees(&req, &url) {
        return Err(reason::TRANSPORT);
    }
    let resp = match req.call() {
        Ok(resp) => resp,
        Err(e) => return Err(map_err(&e)),
    };
    if resp.status() != 200 {
        return Err(status_reason(resp.status()));
    }
    let raw = body(resp)?;
    let value: Value = serde_json::from_slice(&raw).map_err(|_| reason::UNPARSEABLE)?;
    match parse_limits(&value, now_s) {
        Limits::Report(report) => Ok(report),
        Limits::CreditsOn => Err(reason::CREDITS_ON),
        Limits::Unparseable => Err(reason::UNPARSEABLE),
    }
}

// ---------------------------------------------------------------------------
// Refresh
// ---------------------------------------------------------------------------

/// Refreshes holding or about to take a lock right now, in this process.
pub(super) fn refreshes_in_progress() -> usize {
    REFRESHING.load(Ordering::SeqCst)
}

/// Waits up to `limit` for every refresh in progress to release its lock;
/// whether none is left.
pub(super) fn wait_for_refreshes(limit: Duration) -> bool {
    let start = Instant::now();
    while refreshes_in_progress() > 0 {
        if start.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(POLL);
    }
    true
}

/// Claude's refresh lock in one directory, taken by this process. Dropping
/// it removes the lock, only if it is still the one created here.
struct Held<'a> {
    dir: &'a Path,
    ino: u64,
}

impl<'a> Held<'a> {
    /// SR-9: the count goes up before `cancel` is read, and a stopping
    /// rotator sets `cancel` before it waits for the count to reach zero
    /// (both `SeqCst`). Either the rotator sees this refresh and waits for
    /// it, or this refresh sees the stop and takes no lock.
    fn take(dir: &'a Path, cancel: &AtomicBool) -> Result<Self, &'static str> {
        REFRESHING.fetch_add(1, Ordering::SeqCst);
        if cancel.load(Ordering::SeqCst) {
            REFRESHING.fetch_sub(1, Ordering::SeqCst);
            return Err(reason::REFRESH_BUSY);
        }
        match take_refresh_lock_at(dir) {
            Ok(ino) => Ok(Self { dir, ino }),
            Err(e) => {
                REFRESHING.fetch_sub(1, Ordering::SeqCst);
                Err(if e.kind() == std::io::ErrorKind::AlreadyExists {
                    reason::REFRESH_BUSY
                } else {
                    reason::REFRESH_REFUSED
                })
            }
        }
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        // A lock that is no longer ours is left alone; one that is ours
        // but cannot be removed is reported by the release itself.
        release_refresh_lock_at(self.dir, self.ino);
        REFRESHING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// SR-6: the directory is a real directory of the user's own that no one
/// else can write.
fn dir_private(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir)
        .is_ok_and(|m| m.is_dir() && m.uid() == euid() && m.mode() & 0o022 == 0)
}

fn journal_present(journals: &[PathBuf]) -> bool {
    journals
        .iter()
        .any(|j| std::fs::symlink_metadata(j).is_ok())
}

/// A usable access token for `req.creds_dir` other than `used`: Claude's,
/// when it refreshed the login since it was read, or a new one from the
/// token endpoint, saved back before it is used.
fn refresh(
    ep: &Endpoints,
    req: &Request<'_>,
    used: &Token,
    deadline: Deadline,
) -> Result<Token, &'static str> {
    if !dir_private(req.creds_dir) {
        return Err(reason::REFRESH_REFUSED);
    }
    if journal_present(req.journals) {
        return Err(reason::REFRESH_BUSY);
    }
    let _held = Held::take(req.creds_dir, req.cancel)?;
    // Both checks again under the lock: the first pair only saves a
    // `mkdir` in a directory that would be refused anyway (SR-6).
    if !dir_private(req.creds_dir) {
        return Err(reason::REFRESH_REFUSED);
    }
    if journal_present(req.journals) {
        return Err(reason::REFRESH_BUSY);
    }
    let current = load(req.creds_dir)?;
    if !current.access.same(used) {
        return Ok(current.access);
    }
    let Some(old_refresh) = current.refresh.as_ref() else {
        return Err(reason::REFRESH_FAILED);
    };
    // SR-10: a refresh can retire the old refresh token, so it is sent only
    // when the result can be saved.
    let temp = PrivateTemp::create(req.creds_dir, CREDS).map_err(|_| reason::REFRESH_REFUSED)?;
    let url = ep.token_url();
    // No POST starts with less than REFRESH_BUDGET of the read left: a
    // refresh sent then could retire the old refresh token and not be
    // answered in time to save the new one. The POST gets its own agent so
    // that its connect and read steps are not cut below the budget by a
    // short probe deadline; it still ends by the read's deadline.
    let left = deadline.left()?;
    if left < REFRESH_BUDGET {
        return Err(reason::TIMEOUT);
    }
    let post = agent_with_step(ep, left, left.min(MAX_STEP))
        .post(url)
        .timeout(left)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .set("anthropic-beta", BETA);
    if !parsed_agrees(&post, url) {
        return Err(reason::REFRESH_FAILED);
    }
    let payload = json!({
        "grant_type": "refresh_token",
        "refresh_token": old_refresh.as_str(),
        "client_id": CLIENT_ID,
    });
    let resp = post
        .send_string(&payload.to_string())
        .map_err(|_| reason::REFRESH_FAILED)?;
    if resp.status() != 200 {
        return Err(reason::REFRESH_FAILED);
    }
    let raw = body(resp).map_err(|_| reason::REFRESH_FAILED)?;
    let answer: Value = serde_json::from_slice(&raw).map_err(|_| reason::REFRESH_FAILED)?;
    let fresh = answer
        .get("access_token")
        .and_then(Value::as_str)
        .and_then(Token::parse)
        .ok_or(reason::REFRESH_FAILED)?;
    let lifetime = answer
        .get("expires_in")
        .and_then(expires_in)
        .ok_or(reason::REFRESH_FAILED)?;
    let next_refresh = match answer.get("refresh_token") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_str()
                .and_then(Token::parse)
                .ok_or(reason::REFRESH_FAILED)?,
        ),
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let expires_at = lifetime
        .checked_mul(1000)
        .and_then(|ms| now_ms.checked_add(ms))
        .ok_or(reason::REFRESH_FAILED)?;
    let mut whole = current.whole;
    let Some(oauth) = whole
        .get_mut("claudeAiOauth")
        .and_then(Value::as_object_mut)
    else {
        return Err(reason::REFRESH_NOT_SAVED);
    };
    oauth.insert("accessToken".into(), Value::from(fresh.as_str()));
    oauth.insert("expiresAt".into(), Value::from(expires_at));
    if let Some(r) = &next_refresh {
        oauth.insert("refreshToken".into(), Value::from(r.as_str()));
    }
    let bytes = serde_json::to_vec_pretty(&whole).map_err(|_| reason::REFRESH_NOT_SAVED)?;
    temp.commit(&bytes).map_err(|_| reason::REFRESH_NOT_SAVED)?;
    Ok(fresh)
}
