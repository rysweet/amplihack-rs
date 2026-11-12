//! The direct usage read (issue #1844): one `GET /api/oauth/usage` with the
//! login's own OAuth token, refreshing the token at most once under Claude's
//! `.oauth_refresh.lock`. Beside it, the server identity read (issue #1886,
//! [`account`]): one `GET /api/oauth/profile` with the same token handling
//! and no refresh.
//!
//! The token is held in memory only, in [`Token`], which has no `Display`
//! and a redacting `Debug`. It is sent only in the `Authorization` header
//! and the refresh body, and never appears in a URL, an error, a log line or
//! a child's environment. Every failure is one of the fixed texts in
//! [`reason`]; [`map_err`] is the only code that looks at a `ureq` error, and
//! it keeps only the status code or the error kind. Nothing here reads the
//! environment except [`Endpoints::from_env`], and nothing prints.

use super::auto_switch::kit::{
    CREDS, dir_writable_by_others, euid, read_bounded_nofollow, release_refresh_lock_at,
    take_refresh_lock_at, writable_by_others,
};
use super::rotate::identify::Account;
use super::usage_parse::UsageReport;
use super::usage_request::{ExtraUsageShape, Limits, parse_limits};
use amplihack_utils::clock::{Clock, SystemClock};
use amplihack_utils::secure_files::PrivateTemp;
use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
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
/// Issue #1886: the OAuth profile endpoint Claude Code's `/status` reads.
/// It reports the account and organization of the token it is sent.
pub(super) const PROFILE_PATH: &str = "/api/oauth/profile";
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
/// `timeout` row. The budget makes an abandoned refresh less likely but
/// cannot rule it out; one that is abandoned is reported as
/// `refresh not saved` ([`post_failure`]), with the warning to log in again.
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

    pub(super) fn profile_url(&self) -> String {
        let mut url = self.api.clone();
        url.push_str(PROFILE_PATH);
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

/// The reason for a refresh POST that ended in an error.
///
/// `refresh failed` (the old refresh token is still in force) covers only
/// what provably did not reach the token server or was refused by it: a 4xx
/// status, and a failure before or during the connect (DNS, TCP, TLS
/// handshake, a bad URL or proxy), which comes before any of the request is
/// written. The DNS lookup is bounded by [`bounded_lookup`], so a stalled
/// resolver ends as `ErrorKind::Dns` and not as an I/O timeout.
///
/// Everything else is `refresh not saved`, which comes with the warning to
/// log in again, because it can follow a request the server received and
/// acted on: a 5xx (a 502, 504, 520 or 524 from a proxy in front of the
/// token server can follow a request the origin received and handled), a
/// timeout or I/O error while writing the request or waiting for the answer,
/// and a malformed answer.
pub(super) fn post_failure(e: &ureq::Error) -> &'static str {
    match e {
        HttpError::Status(code, _) => refresh_status(*code),
        HttpError::Transport(t) => match t.kind() {
            ErrorKind::InvalidUrl
            | ErrorKind::UnknownScheme
            | ErrorKind::Dns
            | ErrorKind::InsecureRequestHttpsOnly
            | ErrorKind::ConnectionFailed
            | ErrorKind::InvalidProxyUrl
            | ErrorKind::ProxyConnect
            | ErrorKind::ProxyUnauthorized => reason::REFRESH_FAILED,
            ErrorKind::TooManyRedirects
            | ErrorKind::BadStatus
            | ErrorKind::BadHeader
            | ErrorKind::Io
            | ErrorKind::HTTP => reason::REFRESH_NOT_SAVED,
        },
    }
}

/// The reason for a refresh answer with a status other than 200. A 3xx (a
/// redirect, not followed) or 4xx is the request turned away: the old
/// refresh token is still in force. A 5xx, another 2xx, or any other code
/// may follow a request the server acted on.
pub(super) fn refresh_status(code: u16) -> &'static str {
    match code {
        300..=499 => reason::REFRESH_FAILED,
        _ => reason::REFRESH_NOT_SAVED,
    }
}

/// A DNS resolver for `ureq` that gives up at `until`.
///
/// `ureq` 2 puts no deadline on the lookup (`connect_host` in its
/// `stream.rs`), and fixes the connect deadline before the lookup starts, so
/// a slow lookup would end as an I/O timeout, which [`post_failure`] cannot
/// tell from one after the request was written. Here `lookup` runs on its
/// own thread; if it has not answered by `until`, the resolver returns a
/// `TimedOut` error, which `ureq` reports as `ErrorKind::Dns`, and no byte
/// of the request is sent. A lookup given up on keeps its thread until the
/// system resolver returns; its answer is dropped.
///
/// `until` must be a real [`Instant`] (from `Instant::now()`), never one from
/// an injected [`Clock`]: the wait is measured against the real clock.
pub(super) fn bounded_lookup<L>(
    until: Instant,
    lookup: L,
) -> impl Fn(&str) -> std::io::Result<Vec<SocketAddr>> + Send + Sync + 'static
where
    L: Fn(&str) -> std::io::Result<Vec<SocketAddr>> + Send + Sync + Clone + 'static,
{
    move |netloc: &str| {
        let (tx, rx) = mpsc::channel();
        let name = netloc.to_owned();
        let lookup = lookup.clone();
        std::thread::Builder::new()
            .name("usage-dns".into())
            .spawn(move || {
                let _ = tx.send(lookup(&name));
            })?;
        match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(answer) => answer,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "dns lookup timed out",
            )),
        }
    }
}

/// The system resolver, as `ureq` uses it.
pub(super) fn system_lookup(netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
    netloc.to_socket_addrs().map(Iterator::collect)
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

/// What a read takes from its surroundings (issue #2019): the clock its
/// deadline is measured on and the count of refreshes holding a lock.
/// [`read`] and [`refresh_expired`] use [`Ctx::system`]; tests pass their own
/// clock and counter to [`read_with`] and [`refresh_expired_with`]. Real
/// socket timeouts stay real in production. Tests can supply an independent
/// transport guard or an in-process transport so virtual budgets never become
/// short real socket deadlines.
pub(super) struct Ctx<'a> {
    clock: &'a dyn Clock,
    refreshing: &'a AtomicUsize,
    #[cfg(test)]
    transport_guard: Option<Duration>,
    #[cfg(test)]
    transport: Option<&'a dyn TestTransport>,
}

#[cfg(test)]
pub(super) trait TestTransport {
    fn send(
        &self,
        request: ureq::Request,
        payload: Option<&str>,
    ) -> Result<ureq::Response, Box<HttpError>>;
}

impl<'a> Ctx<'a> {
    /// A context with `clock` and the counter `refreshing`.
    #[cfg(test)]
    pub(super) fn new(clock: &'a dyn Clock, refreshing: &'a AtomicUsize, guard: Duration) -> Self {
        Self {
            clock,
            refreshing,
            transport_guard: Some(guard),
            transport: None,
        }
    }

    #[cfg(test)]
    pub(super) fn with_transport(mut self, transport: &'a dyn TestTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    fn send(
        &self,
        request: ureq::Request,
        payload: Option<&str>,
    ) -> Result<ureq::Response, Box<HttpError>> {
        #[cfg(test)]
        if let Some(transport) = self.transport {
            return transport.send(request, payload);
        }
        match payload {
            Some(payload) => request.send_string(payload),
            None => request.call(),
        }
        .map_err(Box::new)
    }

    fn deadline(&self, timeout: Duration) -> Deadline<'a> {
        let deadline = Deadline::after(timeout, self.clock);
        #[cfg(test)]
        let deadline = Deadline {
            transport_guard: self.transport_guard,
            ..deadline
        };
        deadline
    }

    /// The real clock and the process-wide counter that
    /// `refreshes_in_progress` and [`wait_for_refreshes`] read (SR-9).
    pub(super) fn system() -> Ctx<'static> {
        Ctx {
            clock: &SystemClock,
            refreshing: &REFRESHING,
            #[cfg(test)]
            transport_guard: None,
            #[cfg(test)]
            transport: None,
        }
    }
}

/// The end of one read, fixed when it starts. ureq's own `timeout` starts
/// again on every request, so each request is given only the time left:
/// the GET, the refresh POST and the second GET together take at most the
/// read's one deadline, not one deadline each.
#[derive(Clone, Copy)]
struct Deadline<'a> {
    end: Instant,
    clock: &'a dyn Clock,
    #[cfg(test)]
    transport_guard: Option<Duration>,
}

impl<'a> Deadline<'a> {
    /// Fails closed: a timeout too large to add ends the read at once.
    fn after(timeout: Duration, clock: &'a dyn Clock) -> Self {
        let now = clock.now();
        Self {
            end: now.checked_add(timeout).unwrap_or(now),
            clock,
            #[cfg(test)]
            transport_guard: None,
        }
    }

    /// The time left, or `timeout` once none is: no request starts then.
    fn left(self) -> Result<Duration, &'static str> {
        let left = self.end.saturating_duration_since(self.clock.now());
        if left.is_zero() {
            Err(reason::TIMEOUT)
        } else {
            Ok(left)
        }
    }

    /// Check the logical deadline even when a test gives blocking I/O a
    /// separate hang guard. Production always uses the remaining budget.
    fn transport_timeout(self) -> Result<Duration, &'static str> {
        let left = self.left()?;
        Ok(self.transport_step(left))
    }

    fn transport_step(self, step: Duration) -> Duration {
        #[cfg(test)]
        if let Some(guard) = self.transport_guard {
            return guard;
        }
        step
    }
}

/// A successful direct read (issue #1856): the windows, and the redacted
/// shape of `extra_usage` when usage credits are on. The answer's body
/// itself never leaves [`get_usage`].
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Direct {
    pub(super) report: UsageReport,
    pub(super) credits_on: Option<ExtraUsageShape>,
}

/// The usage of the login in `req.creds_dir`. At most two GETs and one
/// refresh POST, all within `req.timeout` from the start of the read; a 429
/// is not retried and a redirect is not followed.
pub(super) fn read(ep: &Endpoints, req: &Request<'_>) -> Result<Direct, &'static str> {
    read_with(ep, req, &Ctx::system())
}

/// [`read`] with the deadline measured on `ctx.clock` and the refresh
/// counted in `ctx.refreshing`.
pub(super) fn read_with(
    ep: &Endpoints,
    req: &Request<'_>,
    ctx: &Ctx<'_>,
) -> Result<Direct, &'static str> {
    let deadline = ctx.deadline(req.timeout);
    if !ep.usable() {
        return Err(reason::TRANSPORT);
    }
    let creds = load(req.creds_dir)?;
    let agent = agent(ep, req.timeout, deadline);
    let now_s = req.now_ms.div_euclid(1000);
    let margin_ms = REFRESH_MARGIN_SECS * 1000;
    let due = creds
        .expires_at_ms
        .is_some_and(|at| at <= req.now_ms.saturating_add(margin_ms));
    if due {
        if req.allow_refresh {
            let fresh = refresh(ep, req, ctx, &creds.access, deadline)?;
            return get_usage(&agent, ep, &fresh, now_s, deadline, ctx);
        }
        if creds.expires_at_ms.is_some_and(|at| at <= req.now_ms) {
            return Err(reason::EXPIRED);
        }
    }
    match get_usage(&agent, ep, &creds.access, now_s, deadline, ctx) {
        Err(r) if r == reason::HTTP_401 && req.allow_refresh => {
            let fresh = refresh(ep, req, ctx, &creds.access, deadline)?;
            get_usage(&agent, ep, &fresh, now_s, deadline, ctx)
        }
        other => other,
    }
}

/// Issue #1886: the account the server reports for the login in
/// `creds_dir`, by one `GET /api/oauth/profile` with its OAuth token, within
/// `timeout`. Never refreshes and so never writes the login: a token at or
/// past its expiry is `token expired` and nothing is sent. The answer's body
/// never leaves this function; only the account it names does.
pub(super) fn account(
    ep: &Endpoints,
    creds_dir: &Path,
    timeout: Duration,
    now_ms: i64,
) -> Result<Account, &'static str> {
    let deadline = Deadline::after(timeout, &SystemClock);
    if !ep.usable() {
        return Err(reason::TRANSPORT);
    }
    let creds = load(creds_dir)?;
    if creds.expires_at_ms.is_some_and(|at| at <= now_ms) {
        return Err(reason::EXPIRED);
    }
    let agent = agent(ep, timeout, deadline);
    let url = ep.profile_url();
    let req = agent
        .get(&url)
        .timeout(deadline.left()?)
        .set("Authorization", &creds.access.header())
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
    Account::from_profile(&value).ok_or(reason::UNPARSEABLE)
}

/// Issue #1753 (R1): refreshes the login in `req.creds_dir` once, under
/// Claude's `.oauth_refresh.lock`, only when its token is at or past its
/// `expiresAt`. `Ok(false)`: the token has not expired and nothing was sent
/// or written. `Ok(true)`: the file now holds a fresh token, written by this
/// call or by Claude meanwhile. Every refusal of [`read`]'s refresh applies:
/// a pending swap journal, a directory others can write, or less than the
/// refresh budget left. No usage or identity request is made here; the
/// caller reads the refreshed login's identity afresh.
pub(super) fn refresh_expired(ep: &Endpoints, req: &Request<'_>) -> Result<bool, &'static str> {
    refresh_expired_with(ep, req, &Ctx::system())
}

/// [`refresh_expired`] with the deadline measured on `ctx.clock` and the
/// refresh counted in `ctx.refreshing`.
pub(super) fn refresh_expired_with(
    ep: &Endpoints,
    req: &Request<'_>,
    ctx: &Ctx<'_>,
) -> Result<bool, &'static str> {
    let deadline = ctx.deadline(req.timeout);
    let creds = load(req.creds_dir)?;
    // Expiry first: a token that has not expired needs no endpoint, so
    // unusable endpoints must not be reported as a failed refresh.
    if !creds.expires_at_ms.is_some_and(|at| at <= req.now_ms) {
        return Ok(false);
    }
    if !ep.usable() {
        return Err(reason::TRANSPORT);
    }
    if !req.allow_refresh {
        return Err(reason::EXPIRED);
    }
    refresh(ep, req, ctx, &creds.access, deadline).map(|_| true)
}

/// Issue #1962: whether the token in `creds_dir` is at or past its expiry, so a
/// `--dry-run` can say that a real cycle would refresh it. Sends nothing.
pub(super) fn expired(creds_dir: &Path, now_ms: i64) -> bool {
    load(creds_dir).is_ok_and(|c| c.expires_at_ms.is_some_and(|at| at <= now_ms))
}

/// The connect and read step of the `GET` agent for a read of `timeout`:
/// a second under the timeout, from [`MIN_STEP`] to [`MAX_STEP`].
pub(super) fn get_step(timeout: Duration) -> Duration {
    MAX_STEP
        .min(timeout.saturating_sub(Duration::from_secs(1)))
        .max(MIN_STEP)
}

/// The connect and read step of a refresh POST sent with `left` of the read
/// to go: all of it, up to [`MAX_STEP`]. Never cut down to [`get_step`], so a
/// short probe deadline does not abandon a refresh the budget admitted.
pub(super) fn post_step(left: Duration) -> Duration {
    left.min(MAX_STEP)
}

/// The agent for the `GET`s. Its DNS lookups end once the time left on the
/// read's deadline has passed in real time; with none left they end at once,
/// and the caller's own check of [`Deadline::left`] sends no request then.
fn agent(ep: &Endpoints, timeout: Duration, deadline: Deadline<'_>) -> ureq::Agent {
    let left = deadline.transport_timeout().unwrap_or_default();
    let now = Instant::now();
    agent_builder(
        ep,
        deadline.transport_step(timeout),
        deadline.transport_step(get_step(timeout)),
    )
    .resolver(bounded_lookup(
        now.checked_add(left).unwrap_or(now),
        system_lookup,
    ))
    .build()
}

fn agent_builder(ep: &Endpoints, timeout: Duration, step: Duration) -> ureq::AgentBuilder {
    ureq::AgentBuilder::new()
        .timeout_connect(step)
        .timeout_read(step)
        .timeout_write(step)
        .timeout(timeout)
        .redirects(0)
        .https_only(ep.https_only())
        .user_agent(USER_AGENT)
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
    deadline: Deadline<'_>,
    ctx: &Ctx<'_>,
) -> Result<Direct, &'static str> {
    let url = ep.usage_url();
    let req = agent
        .get(&url)
        .timeout(deadline.transport_timeout()?)
        .set("Authorization", &token.header())
        .set("anthropic-beta", BETA)
        .set("Accept", "application/json");
    if !parsed_agrees(&req, &url) {
        return Err(reason::TRANSPORT);
    }
    let resp = match ctx.send(req, None) {
        Ok(resp) => resp,
        Err(e) => return Err(map_err(&e)),
    };
    if resp.status() != 200 {
        return Err(status_reason(resp.status()));
    }
    let raw = body(resp)?;
    let value: Value = serde_json::from_slice(&raw).map_err(|_| reason::UNPARSEABLE)?;
    match parse_limits(&value, now_s) {
        Limits::Report { report, credits_on } => Ok(Direct { report, credits_on }),
        Limits::CreditsOn => Err(reason::CREDITS_ON),
        Limits::Unparseable => Err(reason::UNPARSEABLE),
    }
}

// ---------------------------------------------------------------------------
// Refresh
// ---------------------------------------------------------------------------

/// Refreshes holding or about to take a lock right now, in this process.
#[cfg(test)]
pub(super) fn refreshes_in_progress() -> usize {
    REFRESHING.load(Ordering::SeqCst)
}

/// The process-wide refresh count itself, for a test that waits on it with a
/// clock of its own (issue #2019).
#[cfg(test)]
pub(super) fn refresh_counter() -> &'static AtomicUsize {
    &REFRESHING
}

/// Waits up to `limit` for every refresh in progress to release its lock;
/// whether none is left.
pub(super) fn wait_for_refreshes(limit: Duration) -> bool {
    wait_for_refreshes_on(&REFRESHING, limit, &SystemClock)
}

/// [`wait_for_refreshes`] on any counter and clock (issue #2019): `true` once
/// `counter` is 0, `false` once `limit` has passed on `clock` with refreshes
/// still running. A counter already at 0 returns without sleeping. Between
/// checks it sleeps [`POLL`] on `clock`. A `limit` too large to add to the
/// clock's present has no end.
pub(crate) fn wait_for_refreshes_on(
    counter: &AtomicUsize,
    limit: Duration,
    clock: &dyn Clock,
) -> bool {
    let deadline = clock.now().checked_add(limit);
    while counter.load(Ordering::SeqCst) > 0 {
        if deadline.is_some_and(|end| clock.now() >= end) {
            return false;
        }
        clock.sleep(POLL);
    }
    true
}

/// Claude's refresh lock in one directory, taken by this process. Dropping
/// it removes the lock, only if it is still the one created here, and takes
/// the refresh off the counter it was added to.
struct Held<'a> {
    dir: &'a Path,
    ino: u64,
    refreshing: &'a AtomicUsize,
}

impl<'a> Held<'a> {
    /// SR-9: the count goes up before `cancel` is read, and a stopping
    /// rotator sets `cancel` before it waits for the count to reach zero
    /// (both `SeqCst`). Either the rotator sees this refresh and waits for
    /// it, or this refresh sees the stop and takes no lock. Every change to
    /// the count is made on `refreshing`, the one counter of this read.
    fn take(
        dir: &'a Path,
        cancel: &AtomicBool,
        refreshing: &'a AtomicUsize,
    ) -> Result<Self, &'static str> {
        refreshing.fetch_add(1, Ordering::SeqCst);
        if cancel.load(Ordering::SeqCst) {
            refreshing.fetch_sub(1, Ordering::SeqCst);
            return Err(reason::REFRESH_BUSY);
        }
        match take_refresh_lock_at(dir) {
            Ok(ino) => Ok(Self {
                dir,
                ino,
                refreshing,
            }),
            Err(e) => {
                refreshing.fetch_sub(1, Ordering::SeqCst);
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
        self.refreshing.fetch_sub(1, Ordering::SeqCst);
    }
}

/// SR-6: the directory is a real directory of the user's own that no one
/// else can write.
fn dir_private(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir)
        .is_ok_and(|m| m.is_dir() && m.uid() == euid() && !dir_writable_by_others(dir, &m))
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
    ctx: &Ctx<'_>,
    used: &Token,
    deadline: Deadline<'_>,
) -> Result<Token, &'static str> {
    if !dir_private(req.creds_dir) {
        return Err(reason::REFRESH_REFUSED);
    }
    if journal_present(req.journals) {
        return Err(reason::REFRESH_BUSY);
    }
    let _held = Held::take(req.creds_dir, req.cancel, ctx.refreshing)?;
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
    // short probe deadline; it still ends by the read's deadline, and an
    // answer that misses it is reported as `refresh not saved` below.
    let left = deadline.left()?;
    if left < REFRESH_BUDGET {
        return Err(reason::TIMEOUT);
    }
    // The DNS lookup ends at least MIN_STEP before the connect deadline,
    // which `ureq` fixes before the lookup starts, so a slow lookup is
    // `ErrorKind::Dns` (`refresh failed`, nothing sent) and never the
    // connect deadline's I/O timeout.
    let step = deadline.transport_step(post_step(left));
    let dns = bounded_lookup(
        Instant::now() + step.saturating_sub(MIN_STEP),
        system_lookup,
    );
    let transport_timeout = deadline.transport_step(left);
    let post = agent_builder(ep, transport_timeout, step)
        .resolver(dns)
        .build()
        .post(url)
        .timeout(transport_timeout)
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
    // From here on the request may have reached the token server, which can
    // retire the old refresh token as soon as it answers. The POST still ends
    // at the read's deadline, so an answer can arrive too late to be read.
    // Every failure that can follow a sent request is therefore
    // `refresh not saved`, which tells the user to log in again; only a
    // failure that provably came before the request was written, or a 3xx or
    // 4xx status the server turned it away with, is `refresh failed`.
    let resp = ctx
        .send(post, Some(&payload.to_string()))
        .map_err(|e| post_failure(&e))?;
    if resp.status() != 200 {
        return Err(refresh_status(resp.status()));
    }
    // A 200 means the server issued new tokens; an answer that cannot be
    // used leaves the login with a refresh token that may be retired.
    let raw = body(resp).map_err(|_| reason::REFRESH_NOT_SAVED)?;
    let answer: Value = serde_json::from_slice(&raw).map_err(|_| reason::REFRESH_NOT_SAVED)?;
    let fresh = answer
        .get("access_token")
        .and_then(Value::as_str)
        .and_then(Token::parse)
        .ok_or(reason::REFRESH_NOT_SAVED)?;
    let lifetime = answer
        .get("expires_in")
        .and_then(expires_in)
        .ok_or(reason::REFRESH_NOT_SAVED)?;
    let next_refresh = match answer.get("refresh_token") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_str()
                .and_then(Token::parse)
                .ok_or(reason::REFRESH_NOT_SAVED)?,
        ),
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let expires_at = lifetime
        .checked_mul(1000)
        .and_then(|ms| now_ms.checked_add(ms))
        .ok_or(reason::REFRESH_NOT_SAVED)?;
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

#[cfg(test)]
mod transport_deadline_tests {
    use super::*;
    use crate::test_support::virtual_clock::VirtualClock;

    #[test]
    fn transport_guard_is_independent_but_never_admits_a_spent_logical_budget() {
        let clock = VirtualClock::new();
        let counter = AtomicUsize::new(0);
        let budget = Duration::from_secs(5);
        let guard = Duration::from_secs(120);
        let deadline = Ctx::new(&clock, &counter, guard).deadline(budget);
        assert_eq!(deadline.transport_timeout(), Ok(guard));
        assert_eq!(deadline.transport_step(get_step(budget)), guard);
        assert_eq!(deadline.transport_step(post_step(budget)), guard);
        clock.advance(Duration::from_secs(4));
        assert_eq!(deadline.left(), Ok(Duration::from_secs(1)));
        assert_eq!(deadline.transport_timeout(), Ok(guard));
        clock.advance(Duration::from_secs(1));
        assert_eq!(deadline.transport_timeout(), Err(reason::TIMEOUT));
    }

    #[test]
    fn system_transport_keeps_production_steps_and_remaining_budget() {
        let clock = VirtualClock::new();
        let budget = Duration::from_secs(5);
        // No test override: the same Deadline constructor account() and the
        // system context use, with a controlled clock for exact assertions.
        let deadline = Deadline::after(budget, &clock);
        assert_eq!(Ctx::system().transport_guard, None);
        assert_eq!(
            deadline.transport_step(get_step(budget)),
            Duration::from_secs(4)
        );
        assert_eq!(deadline.transport_step(post_step(budget)), budget);
        clock.advance(Duration::from_secs(4));
        assert_eq!(deadline.transport_timeout(), Ok(Duration::from_secs(1)));
    }
}
