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
use crate::support::profile_rotate::{Fixture, PROFILES, private_file, text};
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
                        "extra_usage": {"is_enabled": false}
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
    let endpoint = Endpoint::start(all, status);
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
    let warning = "warning: profile \"spare\": token refreshed but not saved; run claude /login for this profile";
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
