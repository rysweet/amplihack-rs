//! Issue #1844: `profile usage` reads usage straight from the OAuth usage
//! endpoint, without starting `claude`.
//!
//! Every run uses the shared fixture (`isolated(..)`, a temporary HOME and
//! profile root, asserted before each command, #1699). The endpoint and the
//! token URL point at a loopback fake in this process, and the profiles hold
//! fake `SENTINEL-` tokens only. `claude` is a recording wrapper around the
//! fixture's fake: it writes its argv and environment to `$RECORD_DIR`
//! before doing anything else, `--version` included.
//!
//! Checked here:
//! - 16 profiles with a 50 ms endpoint finish in under 10 s, every row is
//!   `direct`, and `claude` is never started or even looked up.
//! - A session at 100% keeps its weekly figure (55%) in the table and JSON.
//! - No sentinel token appears in stdout, stderr (with
//!   `RUST_LOG=ureq=debug,amplihack=trace`), any file under the temporary
//!   root other than the login files, or the argv and environment of any
//!   `claude` started.
//! - A `500` from the endpoint runs the fallback and marks the row
//!   `(fallback)`; `--raw` prints the direct failure note.

#![cfg(unix)]

use crate::support::exec_stub::write_executable;
use crate::support::profile_rotate::{Fixture, PROFILES, email, private_file, text};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const API_BASE_ENV: &str = "AMPLIHACK_PROFILE_USAGE_API_BASE";
const TOKEN_URL_ENV: &str = "AMPLIHACK_PROFILE_USAGE_TOKEN_URL";
const PASSIVE_ENV: &str = "AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS";

/// Logs every start of `claude`, then runs the fixture's fake.
const RECORDING_CLAUDE: &str = r#"#!/bin/sh
n="$RECORD_DIR/claude-run.$$"
{ printf 'ARGV:'; for a in "$@"; do printf ' %s' "$a"; done; printf '\n'; env; } > "$n"
exec "$(dirname "$0")/claude-real" "$@"
"#;

fn access(p: &str) -> String {
    format!("SENTINEL-ACCESS-{p}")
}

fn refresh(p: &str) -> String {
    format!("SENTINEL-REFRESH-{p}")
}

fn names() -> Vec<String> {
    let mut v: Vec<String> = PROFILES.iter().map(|p| p.to_string()).collect();
    v.extend((1..=13).map(|i| format!("p{i:02}")));
    v
}

fn iso(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// A loopback fake of the usage endpoint. Each token maps to one profile;
/// the session figure is the profile's index, the week 55%. `status` 0
/// means answer normally; anything else is sent as is.
struct Endpoint {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Endpoint {
    fn start(profiles: Vec<String>, status: u16) -> Self {
        Self::start_with(profiles, status, serde_json::json!({"is_enabled": false}))
    }

    /// Issue #1856 (D11): usage credits on, with sentinel values for every
    /// field that must never be shown: an e-mail, two identifiers, a limit,
    /// an amount and a currency.
    fn start_credits_on(profiles: Vec<String>) -> Self {
        Self::start_with(profiles, 0, credits_on_extra())
    }

    fn start_with(profiles: Vec<String>, status: u16, extra: serde_json::Value) -> Self {
        Self::serve(move |_, auth| {
            let now = chrono::Utc::now().timestamp();
            let who = profiles
                .iter()
                .position(|p| auth == format!("Bearer {}", access(p)));
            match (status, who) {
                (0, Some(i)) => (
                    200,
                    serde_json::json!({
                        "five_hour": {
                            "utilization": if i == 0 { 100.0 } else { i as f64 },
                            "resets_at": iso(now + 3600)
                        },
                        "seven_day": {"utilization": 55.0, "resets_at": iso(now + 3 * 86_400)},
                        "extra_usage": extra
                    })
                    .to_string(),
                ),
                (0, None) => (401, "{}".to_string()),
                (s, _) => (s, "{}".to_string()),
            }
        })
    }

    /// Answers each request with `route(request line, authorization)`.
    fn serve(route: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = Arc::clone(&seen);
        let route = Arc::new(route);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let route = Arc::clone(&route);
                let log = Arc::clone(&log);
                std::thread::spawn(move || {
                    let mut r = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    r.read_line(&mut first).unwrap_or(0);
                    let mut auth = String::new();
                    let mut len = 0usize;
                    loop {
                        let mut l = String::new();
                        if r.read_line(&mut l).unwrap_or(0) == 0 || l.trim().is_empty() {
                            break;
                        }
                        let lower = l.to_ascii_lowercase();
                        if lower.starts_with("authorization:") {
                            auth = l[14..].trim().to_string();
                        }
                        if let Some(v) = lower.strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0; len];
                    let _ = r.read_exact(&mut body);
                    log.lock().unwrap().push(format!("{} {auth}", first.trim()));
                    std::thread::sleep(Duration::from_millis(50));
                    let (code, reply) = route(first.trim(), &auth);
                    let mut out = stream;
                    let _ = write!(
                        out,
                        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                });
            }
        });
        Self { port, seen }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

struct Setup {
    f: Fixture,
    endpoint: Endpoint,
}

/// 16 profiles, each with its own fake login, and the recording `claude`.
/// `work` is live in `~/.claude`, so its row reads `~/.claude`'s login.
fn setup(status: u16) -> Setup {
    setup_with(|all| Endpoint::start(all, status))
}

fn setup_with(endpoint: impl FnOnce(Vec<String>) -> Endpoint) -> Setup {
    let f = Fixture::new();
    let all = names();
    for (i, p) in all.iter().enumerate().skip(PROFILES.len()) {
        f.add_logged_in(p, 10 + i as u32, 20);
    }
    let future = chrono::Utc::now().timestamp_millis() + 3_600_000;
    let creds = |p: &str| {
        serde_json::json!({"claudeAiOauth": {
            "accessToken": access(p),
            "refreshToken": refresh(p),
            "expiresAt": future,
            "subscriptionType": "max"
        }})
        .to_string()
    };
    private_file(&f.home_login(), &creds("work"));
    for p in all.iter().filter(|p| p.as_str() != "work") {
        private_file(&f.slot(p), &creds(p));
    }
    fs::rename(f.tools.join("claude"), f.tools.join("claude-real")).unwrap();
    write_executable(&f.tools.join("claude"), RECORDING_CLAUDE, 0o755);
    let endpoint = endpoint(all);
    Setup { f, endpoint }
}

impl Setup {
    fn usage(&self, args: &[&str]) -> std::process::Output {
        let mut c = self.f.cmd(&[&["profile", "usage"], args].concat());
        c.env(API_BASE_ENV, self.endpoint.base())
            .env(
                TOKEN_URL_ENV,
                format!("{}/v1/oauth/token", self.endpoint.base()),
            )
            .env(PASSIVE_ENV, "0")
            .env("RUST_LOG", "ureq=debug,amplihack=trace");
        c.output().unwrap()
    }

    fn claude_runs(&self) -> Vec<String> {
        fs::read_dir(&self.f.records)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("claude-run.")
            })
            .map(|p| fs::read_to_string(p).unwrap())
            .collect()
    }
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let ft = e.file_type().unwrap();
        if ft.is_dir() {
            files_under(&p, out);
        } else if ft.is_file() {
            out.push(p);
        }
    }
}

/// No sentinel token anywhere it could leak to.
fn assert_no_sentinel_leaks(s: &Setup, out: &std::process::Output) {
    for (what, t) in [("stdout", text(&out.stdout)), ("stderr", text(&out.stderr))] {
        assert!(!t.contains("SENTINEL"), "token in {what}: {t}");
    }
    let mut files = Vec::new();
    files_under(s.f.tmp.path(), &mut files);
    for p in files {
        if p.file_name().is_some_and(|n| n == ".credentials.json") {
            continue;
        }
        let bytes = fs::read(&p).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&bytes).contains("SENTINEL"),
            "token in {}",
            p.display()
        );
    }
    for run in s.claude_runs() {
        assert!(!run.contains("SENTINEL"), "token in a claude child: {run}");
    }
}

fn json_rows(out: &std::process::Output) -> Vec<serde_json::Value> {
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", text(&out.stdout)))
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn sixteen_profiles_are_read_directly_without_claude() {
    let s = setup(0);
    let start = Instant::now();
    let out = s.usage(&["--json"]);
    let took = start.elapsed();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(took < Duration::from_secs(10), "took {took:?}");
    let rows = json_rows(&out);
    assert_eq!(rows.len(), 16, "{rows:#?}");
    for r in &rows {
        assert_eq!(r["source"], "direct", "{r}");
        assert_eq!(r["week_all"]["used_percentage"], 55.0, "{r}");
    }
    assert!(
        s.claude_runs().is_empty(),
        "claude was started or looked up: {:?}",
        s.claude_runs()
    );
    assert_eq!(s.endpoint.requests().len(), 16);
    assert_no_sentinel_leaks(&s, &out);
}

#[test]
fn each_token_is_used_only_for_its_own_row() {
    let s = setup(0);
    let out = s.usage(&["--json"]);
    let rows = json_rows(&out);
    for (i, p) in names().iter().enumerate().skip(1) {
        let r = rows
            .iter()
            .find(|r| r["profile"] == p.as_str())
            .unwrap_or_else(|| panic!("no row for {p}"));
        assert_eq!(r["session"]["used_percentage"], i as f64, "{p}: {r}");
    }
}

#[test]
fn an_exhausted_session_keeps_its_weekly_figure() {
    let s = setup(0);
    let out = s.usage(&["--json", "work"]);
    let rows = json_rows(&out);
    assert_eq!(rows[0]["state"], "session_exhausted");
    assert_eq!(rows[0]["session"]["used_percentage"], 100.0);
    assert_eq!(rows[0]["week_all"]["used_percentage"], 55.0);
    let out = s.usage(&["work"]);
    let table = text(&out.stdout);
    let line = table
        .lines()
        .find(|l| l.starts_with("work"))
        .unwrap_or_else(|| panic!("{table}"));
    assert!(line.contains("100%") && line.contains("55%"), "{line}");
    assert!(!line.contains("(fallback)"), "{line}");
}

#[test]
fn a_failing_endpoint_falls_back_to_claude_and_marks_the_row() {
    let s = setup(500);
    let out = s.usage(&["spare"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let table = text(&out.stdout);
    let line = table
        .lines()
        .find(|l| l.starts_with("spare"))
        .unwrap_or_else(|| panic!("{table}"));
    assert!(line.contains("(fallback)"), "{line}");
    // The canned `/usage` text for spare is 10/20.
    assert!(line.contains("10%") && line.contains("20%"), "{line}");
    assert!(!s.claude_runs().is_empty(), "the fallback started claude");
    assert!(
        !text(&out.stderr).contains("direct read gave"),
        "the note is for --raw only: {}",
        text(&out.stderr)
    );
    assert_no_sentinel_leaks(&s, &out);

    let out = s.usage(&["--raw", "spare"]);
    assert!(
        text(&out.stderr)
            .contains("note: profile \"spare\": direct read gave http 5xx; using claude"),
        "{}",
        text(&out.stderr)
    );
    assert_no_sentinel_leaks(&s, &out);
    let out = s.usage(&["--json", "spare"]);
    assert_eq!(json_rows(&out)[0]["source"], "fallback");
}

#[test]
fn direct_mode_reports_the_failure_and_starts_no_claude() {
    let s = setup(500);
    let mut c = s.f.cmd(&["profile", "usage", "--json", "spare"]);
    c.env(API_BASE_ENV, s.endpoint.base())
        .env(
            TOKEN_URL_ENV,
            format!("{}/v1/oauth/token", s.endpoint.base()),
        )
        .env(PASSIVE_ENV, "0")
        .env("AMPLIHACK_PROFILE_USAGE_PROBE", "direct");
    let out = c.output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let rows = json_rows(&out);
    assert_eq!(rows[0]["state"], "error");
    assert_eq!(rows[0]["error"], "direct read: http 5xx");
    assert!(s.claude_runs().is_empty(), "{:?}", s.claude_runs());
}

#[test]
fn a_missing_claude_fails_only_the_rows_that_need_it() {
    let s = setup(500);
    fs::remove_file(s.f.tools.join("claude")).unwrap();
    let out = s.usage(&["--json", "spare"]);
    assert!(
        out.status.success(),
        "a missing claude is not a setup failure"
    );
    let rows = json_rows(&out);
    assert_eq!(rows[0]["state"], "error");
    assert_eq!(rows[0]["error"], "claude not found");
}

#[test]
fn the_fake_claude_is_the_one_on_record() {
    // Guard for this file: the wrapper records `--version` too, so an
    // empty record in the tests above means claude was never looked up.
    let s = setup(0);
    let out =
        s.f.cmd(&["profile", "usage", "spare"])
            .env(API_BASE_ENV, "http://127.0.0.1:1")
            .env(TOKEN_URL_ENV, "http://127.0.0.1:1/t")
            .env(PASSIVE_ENV, "0")
            .output()
            .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let runs = s.claude_runs();
    assert!(!runs.is_empty());
    assert!(runs.iter().all(|r| r.starts_with("ARGV:")), "{runs:?}");
}

/// SR-10: a refresh whose new token cannot be saved. Every GET answers 401;
/// the token endpoint first puts a directory where `creds` was, so the
/// rename that saves the answer fails, then answers 200.
fn unsaveable_refresh(creds: PathBuf) -> Endpoint {
    Endpoint::serve(move |line, _| {
        if !line.starts_with("POST ") {
            return (401, "{}".to_string());
        }
        let _ = fs::remove_file(&creds);
        fs::create_dir_all(creds.join("blocker")).unwrap();
        let answer = serde_json::json!({
            "access_token": "SENTINEL-ACCESS-new",
            "refresh_token": "SENTINEL-REFRESH-new",
            "expires_in": 28_800
        });
        (200, answer.to_string())
    })
}

#[test]
fn a_refresh_that_is_not_saved_warns_in_every_mode() {
    let s = setup(0);
    let creds = s.f.slot("spare");
    let original = fs::read_to_string(&creds).unwrap();
    let endpoint = unsaveable_refresh(creds.clone());
    let warning = "warning: profile \"spare\": token refresh may not have been saved; run claude /login for this profile";
    for (mode, json) in [(None, false), (None, true), (Some("direct"), false)] {
        if creds.is_dir() {
            fs::remove_dir_all(&creds).unwrap();
        }
        private_file(&creds, &original);
        let mut args = vec!["profile", "usage"];
        if json {
            args.push("--json");
        }
        args.push("spare");
        let mut c = s.f.cmd(&args);
        c.env(API_BASE_ENV, endpoint.base())
            .env(TOKEN_URL_ENV, format!("{}/v1/oauth/token", endpoint.base()))
            .env(PASSIVE_ENV, "0");
        if let Some(m) = mode {
            c.env("AMPLIHACK_PROFILE_USAGE_PROBE", m);
        }
        let out = c.output().unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        let err = text(&out.stderr);
        assert_eq!(
            err.matches(warning).count(),
            1,
            "{mode:?} json={json}: {err}"
        );
        assert!(!err.contains("SENTINEL"), "{err}");
        assert!(
            endpoint.requests().iter().any(|r| r.starts_with("POST ")),
            "the refresh was sent"
        );
        assert!(
            !creds.parent().unwrap().join(".oauth_refresh.lock").exists(),
            "the lock is released"
        );
    }
}

/// SR-10: the token endpoint rotates the refresh token but answers only after
/// the read's deadline. The answer is never read, so the new token cannot be
/// saved and the old one may already be retired: the user is told to log in
/// again, and the file is left as it was.
#[test]
fn a_refresh_answered_after_the_deadline_warns() {
    let s = setup(0);
    let creds = s.f.slot("spare");
    let original = fs::read_to_string(&creds).unwrap();
    let endpoint = Endpoint::serve(|line, _| {
        if !line.starts_with("POST ") {
            return (401, "{}".to_string());
        }
        std::thread::sleep(Duration::from_secs(8));
        let answer = serde_json::json!({
            "access_token": "SENTINEL-ACCESS-new",
            "refresh_token": "SENTINEL-REFRESH-new",
            "expires_in": 28_800
        });
        (200, answer.to_string())
    });
    let warning = "warning: profile \"spare\": token refresh may not have been saved; run claude /login for this profile";
    let start = Instant::now();
    let out =
        s.f.cmd(&["profile", "usage", "spare"])
            .env(API_BASE_ENV, endpoint.base())
            .env(TOKEN_URL_ENV, format!("{}/v1/oauth/token", endpoint.base()))
            .env(PASSIVE_ENV, "0")
            .env("AMPLIHACK_PROFILE_USAGE_PROBE", "direct")
            .env("AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS", "4")
            .output()
            .unwrap();
    let took = start.elapsed();
    let err = text(&out.stderr);
    assert!(took < Duration::from_secs(7), "{took:?}");
    assert!(
        endpoint.requests().iter().any(|r| r.starts_with("POST ")),
        "the refresh was sent"
    );
    assert_eq!(err.matches(warning).count(), 1, "{err}");
    assert!(!err.contains("SENTINEL"), "{err}");
    assert_eq!(fs::read_to_string(&creds).unwrap(), original);
    assert!(
        !creds.parent().unwrap().join(".oauth_refresh.lock").exists(),
        "the lock is released"
    );
}

// ---------------------------------------------------------------------------
// Issue #1856: usage credits on
//
// The endpoint answers every profile with usable windows (session = the
// profile's index, week 55%) and an `extra_usage` with credits on and the
// D11 sentinels. The canned `/usage` text (spare 10/20, extra 50/50, ...)
// is what the spend probe reads; its figures must never become the row's
// windows. Spend probes run only under `--spend`: a default run starts no
// `claude` for a credits-on row and gives it spend `not_probed`.
// ---------------------------------------------------------------------------

const PROBE_ENV: &str = "AMPLIHACK_PROFILE_USAGE_PROBE";
const PARALLEL_ENV: &str = "AMPLIHACK_PROFILE_USAGE_PARALLEL";

fn credits_on_extra() -> serde_json::Value {
    serde_json::json!({
        "is_enabled": true,
        "email": "SENTINEL-EMAIL@example.invalid",
        "organization_uuid": "00000000-SENTINEL-0000-0000-000000000001",
        "account_uuid": "00000000-SENTINEL-0000-0000-000000000002",
        "monthly_limit": 987654,
        "used_credits": 4321.5,
        "currency": "SENTINELCUR"
    })
}

/// The shape line of [`credits_on_extra`].
const SHAPE: &str = "extra_usage: object; keys: account_uuid, currency, email, is_enabled, monthly_limit, organization_uuid, used_credits; is_enabled: bool true";

/// The money sentinels, which `assert_no_sentinel_leaks` cannot match.
const AMOUNTS: [&str; 2] = ["987654", "4321.5"];

fn credits_on() -> Setup {
    setup_with(Endpoint::start_credits_on)
}

impl Setup {
    fn usage_with(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        let mut c = self.f.cmd(&[&["profile", "usage"], args].concat());
        c.env(API_BASE_ENV, self.endpoint.base())
            .env(
                TOKEN_URL_ENV,
                format!("{}/v1/oauth/token", self.endpoint.base()),
            )
            .env(PASSIVE_ENV, "0")
            .env("RUST_LOG", "ureq=debug,amplihack=trace");
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    /// The `/usage` text the fake prints for `p`, replacing the canned one.
    fn usage_text(&self, p: &str, body: &str) {
        fs::write(self.f.canned.join(format!("{}.out", email(p))), body).unwrap();
    }

    /// `p`'s `claude` sleeps 30 s before answering.
    fn slow(&self, p: &str) {
        fs::write(self.f.canned.join(format!("{}.slow", email(p))), "").unwrap();
    }

    /// The `-p /usage` starts among the recorded `claude` runs.
    fn usage_runs(&self) -> usize {
        self.claude_runs()
            .iter()
            .filter(|r| r.lines().next().is_some_and(|l| l.contains("/usage")))
            .count()
    }

    /// Processes still running with this fixture's `RECORD_DIR`.
    fn survivors(&self) -> Vec<String> {
        let marker = format!("RECORD_DIR={}", self.f.records.display());
        let mut found = Vec::new();
        let Ok(rd) = fs::read_dir("/proc") else {
            return found;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Ok(env) = fs::read(e.path().join("environ")) else {
                continue;
            };
            if env.split(|b| *b == 0).any(|v| v == marker.as_bytes()) {
                let cmd = fs::read(e.path().join("cmdline")).unwrap_or_default();
                found.push(format!("{name}: {}", String::from_utf8_lossy(&cmd)));
            }
        }
        found
    }

    /// Waits up to 3 s for every child to be gone.
    fn assert_no_survivors(&self) {
        let start = Instant::now();
        loop {
            let left = self.survivors();
            if left.is_empty() {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "claude children outlived the command: {left:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn row<'a>(rows: &'a [serde_json::Value], p: &str) -> &'a serde_json::Value {
    rows.iter()
        .find(|r| r["profile"] == p)
        .unwrap_or_else(|| panic!("no row for {p}: {rows:#?}"))
}

fn table_line<'a>(table: &'a str, p: &str) -> &'a str {
    table
        .lines()
        .find(|l| l.split_whitespace().next() == Some(p))
        .unwrap_or_else(|| panic!("no row for {p}: {table}"))
}

/// The value of the last column of a table row.
fn spend_cell(line: &str) -> &str {
    line.split_whitespace().last().unwrap()
}

fn assert_no_amounts(s: &Setup, out: &std::process::Output) {
    let mut texts = vec![
        ("stdout".to_string(), text(&out.stdout)),
        ("stderr".to_string(), text(&out.stderr)),
    ];
    let mut files = Vec::new();
    files_under(s.f.tmp.path(), &mut files);
    for p in files {
        if p.file_name().is_some_and(|n| n == ".credentials.json") {
            continue;
        }
        let bytes = fs::read(&p).unwrap_or_default();
        texts.push((
            p.display().to_string(),
            String::from_utf8_lossy(&bytes).into(),
        ));
    }
    for (i, run) in s.claude_runs().into_iter().enumerate() {
        texts.push((format!("claude run {i}"), run));
    }
    for (what, t) in texts {
        for a in AMOUNTS {
            assert!(!t.contains(a), "{a} in {what}: {t}");
        }
    }
}

/// AC2: with `--raw`, two notes per credits-on row, the second the shape;
/// no identifier, e-mail, currency or amount anywhere.
#[test]
fn raw_prints_the_shape_of_extra_usage_and_nothing_secret() {
    let s = credits_on();
    let out = s.usage(&["--raw", "--spend", "spare"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let err = text(&out.stderr);
    assert!(
        err.contains(
            "note: profile \"spare\": direct read gave usage credits on; windows direct, spend from claude\n"
        ),
        "{err}"
    );
    assert!(
        err.contains(&format!("note: profile \"spare\": {SHAPE}\n")),
        "{err}"
    );
    let stdout = text(&out.stdout);
    assert!(stdout.contains("(direct: no claude output)"), "{stdout}");
    assert!(
        !stdout.contains("raw stdout"),
        "the spend probe's output is not shown: {stdout}"
    );
    assert_no_sentinel_leaks(&s, &out);
    assert_no_amounts(&s, &out);

    let out = s.usage(&["spare"]);
    let err = text(&out.stderr);
    assert!(
        !err.contains("extra_usage"),
        "the shape is for --raw only: {err}"
    );
    assert!(
        !err.contains("note: profile"),
        "no source note without --raw: {err}"
    );
    assert_no_sentinel_leaks(&s, &out);
    assert_no_amounts(&s, &out);

    let out = s.usage(&["--json", "spare"]);
    let json = text(&out.stdout);
    for key in ["extra_usage", "is_enabled", "monthly_limit", "keys:"] {
        assert!(!json.contains(key), "{key} in --json: {json}");
    }
    assert_no_sentinel_leaks(&s, &out);
    assert_no_amounts(&s, &out);
    // No token reaches a spend probe's argv or environment.
    for run in s.claude_runs() {
        assert!(!run.contains("SENTINEL-ACCESS"), "{run}");
        assert!(!run.contains("SENTINEL-REFRESH"), "{run}");
    }
}

/// AC3: a credits-on row is `direct`, with the endpoint's windows and,
/// under `--spend`, the spend probe's status.
#[test]
fn credits_on_rows_keep_the_direct_windows() {
    let s = credits_on();
    let out = s.usage(&["--json", "--spend", "spare", "extra"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let rows = json_rows(&out);
    for (p, i) in [("spare", 1.0), ("extra", 2.0)] {
        let r = row(&rows, p);
        assert_eq!(r["source"], "direct", "{r}");
        assert_eq!(r["state"], "ok", "{r}");
        assert_eq!(
            r["session"]["used_percentage"], i,
            "not /usage's figure: {r}"
        );
        assert_eq!(r["week_all"]["used_percentage"], 55.0, "{r}");
        assert_eq!(r["spend"], "ok", "{r}");
        assert_eq!(r["error"], serde_json::Value::Null, "{r}");
    }
    assert_eq!(
        s.usage_runs(),
        2,
        "one spend probe per row: {:?}",
        s.claude_runs()
    );

    let out = s.usage(&["--table", "--spend", "spare"]);
    let table = text(&out.stdout);
    let header: Vec<&str> = table.lines().next().unwrap().split_whitespace().collect();
    assert_eq!(header.last(), Some(&"SPEND"), "{table}");
    let line = table_line(&table, "spare");
    assert!(!line.contains("(fallback)"), "{line}");
    assert!(line.contains("55%"), "{line}");
    assert_eq!(spend_cell(line), "ok", "{line}");

    // The bar view (issue #1813) shows the same spend as a mark.
    let out = s.usage(&["--spend", "spare"]);
    let bars = text(&out.stdout);
    let line = table_line(&bars, "spare");
    assert!(line.contains("55%"), "{line}");
    assert_eq!(spend_cell(line), "spend:ok", "{line}");
}

/// AC4 (crusty round 1): a default run reads 16 credits-on profiles whose
/// `claude` never answers without starting `claude` at all. The windows
/// come from the direct read, every spend is `not_probed` (no probe was
/// asked for, which `--json` keeps apart from a failed one), and nothing
/// waits on `/usage`.
#[test]
fn sixteen_credits_on_profiles_start_no_claude_by_default() {
    let s = credits_on();
    for p in names() {
        s.slow(&p);
    }
    let start = Instant::now();
    let out = s.usage_with(&["--json"], &[(PARALLEL_ENV, "2")]);
    let took = start.elapsed();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(took < Duration::from_secs(10), "took {took:?}");
    let rows = json_rows(&out);
    assert_eq!(rows.len(), 16, "{rows:#?}");
    for r in &rows {
        assert_eq!(r["source"], "direct", "{r}");
        assert_eq!(r["week_all"]["used_percentage"], 55.0, "{r}");
        assert_eq!(r["spend"], "not_probed", "{r}");
    }
    assert!(s.claude_runs().is_empty(), "{:?}", s.claude_runs());
    assert_no_sentinel_leaks(&s, &out);

    let out = s.usage_with(&["--table", "--raw"], &[(PARALLEL_ENV, "2")]);
    let table = text(&out.stdout);
    for p in names() {
        let line = table_line(&table, &p);
        assert_eq!(spend_cell(line), "not_probed", "{line}");
        assert!(line.contains("55%"), "{line}");
    }
    let err = text(&out.stderr);
    assert!(err.contains("windows direct, spend not probed"), "{err}");
    assert!(!err.contains("spend probe failed"), "{err}");
    assert!(s.claude_runs().is_empty(), "{:?}", s.claude_runs());
}

/// AC4 (crusty round 1): under `--spend` a `claude` that never answers is
/// stopped at the normal per-probe timeout, not at a fixed short deadline.
/// Every probe asked for runs, at most `AMPLIHACK_PROFILE_USAGE_PARALLEL`
/// at once, each row's spend is `unknown` (probed and failed, not
/// `not_probed`), and no child is left behind.
#[test]
fn spend_probes_of_a_slow_claude_end_at_the_probe_timeout() {
    let s = credits_on();
    let four = ["p01", "p02", "p03", "p04"];
    for p in four {
        s.slow(p);
    }
    let start = Instant::now();
    let out = s.usage_with(
        &[&["--json", "--spend", "--timeout-secs", "2"][..], &four[..]].concat(),
        &[(PARALLEL_ENV, "2")],
    );
    let took = start.elapsed();
    assert!(out.status.success(), "{}", text(&out.stderr));
    // Two rounds of two 2 s probes: the bound holds, and no probe was cut
    // short of its timeout.
    assert!(took >= Duration::from_secs(4), "took {took:?}");
    assert!(took < Duration::from_secs(25), "took {took:?}");
    let rows = json_rows(&out);
    assert_eq!(rows.len(), 4, "{rows:#?}");
    for r in &rows {
        assert_eq!(r["source"], "direct", "{r}");
        assert_eq!(r["week_all"]["used_percentage"], 55.0, "{r}");
        assert_eq!(r["spend"], "unknown", "{r}");
    }
    assert_eq!(s.usage_runs(), 4, "{:?}", s.claude_runs());
    s.assert_no_survivors();
    assert_no_sentinel_leaks(&s, &out);
}

/// Crusty round 5: with `--spend`, the history line's `at` and
/// `--summary-json`'s `at` are when the windows were read, not when the
/// spend pass ended. Every spend probe here runs to its 3 s deadline, so a
/// time taken after the pass would be at least 3 s after the start.
#[test]
fn spend_probes_do_not_delay_the_history_and_summary_time() {
    let s = credits_on();
    for p in names() {
        s.slow(&p);
    }
    // The history is written only into an existing private state directory.
    let state = s.f.root.join(".auto-switch.d");
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let start = chrono::Utc::now().timestamp();
    let out = s.usage_with(
        &["--summary-json", "--spend", "--timeout-secs", "3"],
        &[(PARALLEL_ENV, "16")],
    );
    let end = chrono::Utc::now().timestamp();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(end - start >= 3, "the spend pass ran: {start}..{end}");
    assert_eq!(s.usage_runs(), names().len(), "{:?}", s.claude_runs());
    let secs = |v: &serde_json::Value| {
        chrono::DateTime::parse_from_rfc3339(v.as_str().unwrap())
            .unwrap()
            .timestamp()
    };
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let at = secs(&summary["at"]);
    assert!(
        (start..=end - 2).contains(&at),
        "summary at {at} is not before the spend pass ({start}..{end})"
    );
    let history = state.join("usage-history.jsonl");
    let raw = fs::read_to_string(&history)
        .unwrap_or_else(|e| panic!("no history ({e}): {}", text(&out.stderr)));
    let line: serde_json::Value = serde_json::from_str(raw.lines().last().unwrap()).unwrap();
    assert_eq!(secs(&line["at"]), at, "{line}");
    s.assert_no_survivors();
}

/// Swaps `~/.claude` to `spare`'s login the first time a `/usage` probe
/// runs in `~/.claude`, then runs the recording `claude`.
fn swap_on_live_spend_probe(s: &Setup) {
    let d = &s.f.tools;
    fs::rename(d.join("claude"), d.join("claude-recording")).unwrap();
    let spare = s.f.root.join("spare");
    let script = format!(
        "#!/bin/sh\n\
         case \" $* \" in *\" /usage \"*)\n\
           case \"${{CLAUDE_CONFIG_DIR-}}\" in \"\"|\"$HOME/.claude\")\n\
             if [ ! -e \"$RECORD_DIR/swapped\" ]; then\n\
               : > \"$RECORD_DIR/swapped\"\n\
               cat '{json}' > \"$HOME/.claude.json\"\n\
               cat '{creds}' > \"$HOME/.claude/.credentials.json\"\n\
             fi ;;\n\
           esac ;;\n\
         esac\n\
         exec \"$(dirname \"$0\")/claude-recording\" \"$@\"\n",
        json = spare.join(".claude.json").display(),
        creds = spare.join(".credentials.json").display(),
    );
    write_executable(&d.join("claude"), &script, 0o755);
}

/// Crusty round 5: a swap of `~/.claude` during the live row's spend probe
/// leaves that row's spend `unknown` (never the new login's spend), with
/// one note, and its windows, read before the swap, are still recorded
/// with no "was not recorded" note.
#[test]
fn a_swap_during_the_live_spend_probe_shows_no_foreign_spend() {
    let s = credits_on();
    s.usage_text(
        "spare",
        "Current session: 10% used\nYour organization has reached its spend limit\n",
    );
    swap_on_live_spend_probe(&s);
    // `usage.json` is written only into an existing private state directory.
    let state = s.f.root.join(".auto-switch.d");
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let out = s.usage(&["--json", "--spend", "work", "spare"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(s.f.records.join("swapped").exists(), "the live probe ran");
    let rows = json_rows(&out);
    let work = row(&rows, "work");
    assert_eq!(work["spend"], "unknown", "not spare's spend: {work}");
    assert_eq!(work["session"]["used_percentage"], 100.0, "{work}");
    assert_eq!(work["source"], "direct", "{work}");
    assert_eq!(work["week_all"]["used_percentage"], 55.0, "{work}");
    assert_eq!(row(&rows, "spare")["spend"], "limit_reached");
    let err = text(&out.stderr);
    assert_eq!(
        err.lines()
            .filter(|l| *l
                == "note: the login in ~/.claude changed while profile \"work\" was probed \
                    for its spend; its spend is unknown this run")
            .count(),
        1,
        "{err}"
    );
    assert!(!err.contains("its usage was not recorded"), "{err}");
    let usage: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(state.join("usage.json")).unwrap()).unwrap();
    assert!(
        usage["accounts"].get("work").is_some(),
        "work's windows were read before the swap and are recorded: {usage}"
    );
    assert_no_sentinel_leaks(&s, &out);
}

/// AC5: a used-up spend limit in `/usage` gives `limit_reached`, and the
/// windows and state still come from the direct read.
#[test]
fn a_spend_limit_in_usage_text_gives_limit_reached() {
    let s = credits_on();
    s.usage_text(
        "spare",
        "Current session: 10% used\nCurrent week (all models): 20% used\n\
         Your organization has reached its spend limit\n",
    );
    let out = s.usage(&["--json", "--spend", "spare"]);
    let rows = json_rows(&out);
    let r = row(&rows, "spare");
    assert_eq!(r["spend"], "limit_reached", "{r}");
    assert_eq!(r["state"], "ok", "the spend never changes the state: {r}");
    assert_eq!(r["source"], "direct", "{r}");
    assert_eq!(r["session"]["used_percentage"], 1.0, "{r}");
    assert_eq!(r["week_all"]["used_percentage"], 55.0, "{r}");
    let out = s.usage(&["--table", "--spend", "spare"]);
    let table = text(&out.stdout);
    let line = table_line(&table, "spare");
    assert_eq!(spend_cell(line), "limit_reached", "{line}");
    assert!(line.split_whitespace().any(|c| c == "ok"), "{line}");
}

/// A2: each row gets its own spend.
#[test]
fn each_row_gets_its_own_spend() {
    let s = credits_on();
    s.usage_text(
        "spare",
        "Current session: 10% used\nYour organization has reached its spend limit\n",
    );
    s.usage_text("extra", "Not logged in. Please run /login\n");
    let out = s.usage(&["--json", "--spend", "spare", "extra", "p01"]);
    let rows = json_rows(&out);
    assert_eq!(row(&rows, "spare")["spend"], "limit_reached");
    assert_eq!(
        row(&rows, "extra")["spend"],
        "unknown",
        "a login prompt is never ok"
    );
    assert_eq!(row(&rows, "p01")["spend"], "ok");
    let out = s.usage(&["--raw", "--spend", "extra"]);
    assert!(
        text(&out.stderr).contains(
            "note: profile \"extra\": direct read gave usage credits on; windows direct, spend probe failed"
        ),
        "{}",
        text(&out.stderr)
    );
}

/// AC6: `direct` mode gives the windows, starts no `claude` even under
/// `--spend`, and says the spend was not probed.
#[test]
fn direct_mode_gives_credits_on_windows_and_starts_no_claude() {
    let s = credits_on();
    let out = s.usage_with(&["--json", "--spend", "spare"], &[(PROBE_ENV, "direct")]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let rows = json_rows(&out);
    let r = row(&rows, "spare");
    assert_eq!(r["source"], "direct", "{r}");
    assert_eq!(
        r["error"],
        serde_json::Value::Null,
        "no longer an error row: {r}"
    );
    assert_eq!(r["session"]["used_percentage"], 1.0, "{r}");
    assert_eq!(r["spend"], "not_probed", "{r}");
    let out = s.usage_with(&["--raw", "spare"], &[(PROBE_ENV, "direct")]);
    let err = text(&out.stderr);
    assert!(
        err.contains(
            "note: profile \"spare\": direct read gave usage credits on; windows direct, spend not probed\n"
        ),
        "{err}"
    );
    assert!(
        err.contains(&format!("note: profile \"spare\": {SHAPE}\n")),
        "{err}"
    );
    assert!(s.claude_runs().is_empty(), "{:?}", s.claude_runs());
    assert_no_sentinel_leaks(&s, &out);
    assert_no_amounts(&s, &out);
}

/// AC6: under `--spend`, a missing `claude` leaves the windows, with
/// spend `unknown`: the probe was asked for and gave nothing.
#[test]
fn a_missing_claude_leaves_credits_on_windows_with_an_unknown_spend() {
    let s = credits_on();
    fs::remove_file(s.f.tools.join("claude")).unwrap();
    let out = s.usage(&["--json", "--spend", "spare"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let r = json_rows(&out)[0].clone();
    assert_eq!(r["state"], "ok", "{r}");
    assert_eq!(r["error"], serde_json::Value::Null, "{r}");
    assert_eq!(r["spend"], "unknown", "{r}");
    let out = s.usage(&["--raw", "--spend", "spare"]);
    let err = text(&out.stderr);
    assert!(err.contains("windows direct, spend probe failed"), "{err}");
}

/// AC7: credits off is unchanged apart from the `SPEND` column and the
/// `spend` key, which read `n/a`; no note and no `claude`.
#[test]
fn credits_off_rows_show_n_a_and_start_no_claude() {
    let s = setup(0);
    let out = s.usage(&["--table", "--raw"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let table = text(&out.stdout);
    let header: Vec<&str> = table
        .lines()
        .next()
        .unwrap()
        .split("  ")
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect();
    assert_eq!(
        header,
        [
            "PROFILE",
            "MARKS",
            "STATE",
            "SESSION",
            "SESSION RESETS",
            "WEEK",
            "WEEK RESETS",
            "SPEND"
        ]
    );
    for p in names() {
        assert_eq!(spend_cell(table_line(&table, &p)), "n/a");
    }
    // The bar view (issue #1813) gives an `n/a` spend no mark.
    let out = s.usage(&[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let bars = text(&out.stdout);
    assert!(!bars.contains("spend:"), "{bars}");
    let err = text(&out.stderr);
    assert!(!err.contains("extra_usage"), "{err}");
    assert!(!err.contains("usage credits on"), "{err}");
    let out = s.usage(&["--json"]);
    for r in json_rows(&out) {
        assert_eq!(r["spend"], "n/a", "{r}");
        let mut keys: Vec<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "account",
                "api_key_blocked",
                "default",
                "error",
                "kind",
                "leased_to",
                "live_in",
                "overflow",
                "overflow_blocked_until",
                "profile",
                "session",
                "session_limit",
                "source",
                "spend",
                "state",
                "week_all",
                "week_models"
            ]
        );
    }
    assert!(s.claude_runs().is_empty(), "{:?}", s.claude_runs());
}
