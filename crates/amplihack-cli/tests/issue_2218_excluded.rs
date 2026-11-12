//! Issue #2218: a profile in the auto-switch `exclude` list is never probed
//! and its token is never refreshed, by any command.
//!
//! Every test runs the real binary on the rotate `Fixture` (temporary HOME and
//! profile root, asserted isolated) with `AMPLIHACK_PROFILE_USAGE_API_BASE` and
//! `_TOKEN_URL` pointed at a loopback endpoint that records each request: its
//! method, path, bearer token and, for a token request, the refresh token sent.
//! Each fake login has its own access and refresh token, so a request "for C"
//! is one that carries C's token. Credentials are fake; nothing reads the
//! real `~/.claude`, `~/.amplihack` or profile root.
//!
//! Besides the request log, each test compares C's credential file (bytes and
//! mtime) before and after. No test sleeps or waits on a wall-clock window:
//! the endpoint blocks on `accept` and is stopped by one wake-up connection.
//!
//! Contract (see "Excluded profiles" and "Rotate excluded live profile" in
//! docs/reference/profile-command.md):
//! - `profile usage` with no names lists C as `excluded (not probed)`, makes no
//!   request for C, writes no history or `usage.json` entry for C, leaves C out
//!   of the summary and exits 0. With names, C is refused and the others are
//!   reported once all of them are done; the refusal ends with
//!   ` (amplihack profile auto-switch exclude)` and the exit is non-zero. An
//!   unknown name still wins.
//! - `profile verify` prints `C: excluded (not probed)`, and for an excluded
//!   live profile `~/.claude: excluded (not probed), live profile C`.
//! - The rotator never sends any request with C's token (usage, token or
//!   identity), on the first cycle or any later one. An excluded live C is
//!   found from local files only ("Local proof of an excluded live login":
//!   the ledger, `~/.claude.json` against C's record, or byte-identical
//!   credentials) and moved off on the first cycle; the incoming target is
//!   still verified over the network. C is never swapped back, and with no
//!   candidate the cycle raises ALERT `excluded-live` and C stays in
//!   `~/.claude`. An expired excluded live login is not refreshed. An
//!   excluded candidate gets no request at all. Under a suspected hand change
//!   C's folder is never overwritten.
//! - The status line, `rotate status` and the usage-limit wait make no request.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::{Fixture, account_json, private_file, text};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const TOKEN_PREFIX: &str = "fake-task2218-";
const REFRESH_PREFIX: &str = "fake-task2218-refresh-";
const UNTRUSTED: &str =
    "auto-switch config unsafe or invalid; exclude list applied, other settings ignored";
const UNREADABLE: &str =
    "auto-switch config unreadable; every profile treated as excluded, none probed or refreshed";

// --- the recording endpoint ---------------------------------------------------

/// One request the fake endpoint received.
#[derive(Debug, Clone)]
struct Req {
    method: String,
    path: String,
    /// The profile whose access token was the bearer, if any.
    bearer: Option<String>,
    /// The profile whose refresh token a token request sent, if any.
    refresh: Option<String>,
}

impl Req {
    /// Whether this request was made for profile `p`.
    fn for_profile(&self, p: &str) -> bool {
        self.bearer.as_deref() == Some(p) || self.refresh.as_deref() == Some(p)
    }
    fn is_usage(&self) -> bool {
        self.path.starts_with("/api/oauth/usage")
    }
    fn is_token(&self) -> bool {
        self.path.starts_with("/v1/oauth/token")
    }
    fn is_identity(&self) -> bool {
        self.method == "GET" && self.path.starts_with("/api/oauth/profile")
    }
}

/// A loopback endpoint that answers identity and usage for each fake token
/// and refuses every refresh, recording each request.
struct Endpoint {
    base: String,
    addr: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    log: Arc<Mutex<Vec<Req>>>,
}

/// `(session, week)` the endpoint reports for each fixture profile.
fn utilization(p: &str) -> (u32, u32) {
    match p {
        "work" => (90, 40),
        "spare" => (10, 20),
        "extra" => (50, 50),
        _ => (10, 10),
    }
}

fn serve(mut stream: TcpStream, log: &Mutex<Vec<Req>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).unwrap_or(0) == 0 {
        return; // the wake-up connection
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut bearer = None;
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
            if key.eq_ignore_ascii_case("authorization") {
                bearer = value
                    .trim()
                    .strip_prefix("Bearer ")
                    .and_then(|t| t.strip_prefix(TOKEN_PREFIX))
                    .filter(|p| !p.starts_with("refresh-"))
                    .map(str::to_string);
            }
        }
    }
    let mut refresh = None;
    if content_length > 0 && content_length <= 65536 {
        let mut raw = vec![0; content_length];
        if reader.read_exact(&mut raw).is_ok() {
            let body = String::from_utf8_lossy(&raw).into_owned();
            let token = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["refresh_token"].as_str().map(str::to_string))
                .or_else(|| {
                    body.split('&')
                        .find_map(|kv| kv.strip_prefix("refresh_token=").map(str::to_string))
                });
            refresh = token
                .as_deref()
                .and_then(|t| t.strip_prefix(REFRESH_PREFIX))
                .map(str::to_string);
        }
    }
    log.lock().unwrap().push(Req {
        method: method.clone(),
        path: path.clone(),
        bearer: bearer.clone(),
        refresh,
    });
    let (status, body) = match (&bearer, method.as_str()) {
        (Some(p), "GET") if path.starts_with("/api/oauth/profile") => (
            "200 OK",
            serde_json::json!({"account": {"uuid": format!("uuid-{p}"),
                "email": format!("{p}@example.com")}}),
        ),
        (Some(p), "GET") if path.starts_with("/api/oauth/usage") => {
            let (s, w) = utilization(p);
            (
                "200 OK",
                serde_json::json!({
                    "five_hour": {"utilization": s, "resets_at":
                        (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339()},
                    "seven_day": {"utilization": w, "resets_at":
                        (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339()}
                }),
            )
        }
        _ => (
            "401 Unauthorized",
            serde_json::json!({"error": "fake-only"}),
        ),
    };
    let body = body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

impl Endpoint {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(Vec::new()));
        let (stopping, logging) = (Arc::clone(&stop), Arc::clone(&log));
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(s) = stream {
                    serve(s, &logging);
                }
            }
        });
        Self {
            base: format!("http://{addr}"),
            addr,
            stop,
            thread: Some(thread),
            log,
        }
    }

    fn requests(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.log.lock().unwrap().clear();
    }

    fn for_profile(&self, p: &str) -> Vec<Req> {
        self.requests()
            .into_iter()
            .filter(|r| r.for_profile(p))
            .collect()
    }

    /// Points `c` at this endpoint, in direct probe mode.
    fn configure(&self, c: &mut Command) {
        c.env("AMPLIHACK_PROFILE_USAGE_API_BASE", &self.base)
            .env(
                "AMPLIHACK_PROFILE_USAGE_TOKEN_URL",
                format!("{}/v1/oauth/token", self.base),
            )
            .env("AMPLIHACK_PROFILE_USAGE_PROBE", "direct");
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the blocking `accept`, which then sees `stop`.
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// --- fixture ------------------------------------------------------------------

/// A credential file's bytes and mtime.
#[derive(Debug, PartialEq)]
struct Stamp(Vec<u8>, std::time::SystemTime);

fn stamp(path: &Path) -> Stamp {
    let meta = fs::metadata(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Stamp(fs::read(path).unwrap(), meta.modified().unwrap())
}

fn fake_login(path: &Path, p: &str, expires_at: i64) {
    assert!(path.starts_with(std::env::temp_dir()));
    private_file(
        path,
        &serde_json::json!({"claudeAiOauth": {
            "accessToken": format!("{TOKEN_PREFIX}{p}"),
            "refreshToken": format!("{REFRESH_PREFIX}{p}"),
            "expiresAt": expires_at,
            "subscriptionType": "max"
        }})
        .to_string(),
    );
}

fn in_an_hour() -> i64 {
    chrono::Utc::now().timestamp_millis() + 3_600_000
}

/// `work` live in `~/.claude` (90/40), `spare` (10/20) and `extra` (50/50)
/// logged in, each with its own fake tokens; the rotate ledger names `work`
/// live; recording is on.
fn fixture() -> (Fixture, Endpoint) {
    let f = Fixture::new();
    f.assert_isolated();
    fake_login(&f.home_login(), "work", in_an_hour());
    fake_login(&f.slot("spare"), "spare", in_an_hour());
    fake_login(&f.slot("extra"), "extra", in_an_hour());
    let state = f.root.join(".auto-switch.d");
    for dir in [state.clone(), state.join("rotate")] {
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let t = chrono::Utc::now().timestamp() - 1200;
    private_file(
        &f.rotate_dir().join("ledger.json"),
        &serde_json::json!({"version": 1, "live": "work", "account": "uuid-work",
            "since": t, "swapped_at": t})
        .to_string(),
    );
    (f, Endpoint::new())
}

fn exclude(f: &Fixture, names: &[&str]) {
    let mut args = vec!["profile", "auto-switch", "exclude"];
    args.extend_from_slice(names);
    f.ok(&args);
}

/// Runs `amplihack <args>` against `ep`. No fake token ever reaches output.
fn run(f: &Fixture, ep: &Endpoint, args: &[&str]) -> Output {
    let mut c = f.cmd(args);
    ep.configure(&mut c);
    let out = c.output().unwrap();
    for value in [text(&out.stdout), text(&out.stderr), f.log()] {
        assert!(!value.contains(TOKEN_PREFIX), "fake token leaked: {value}");
    }
    out
}

fn usage(f: &Fixture, ep: &Endpoint, args: &[&str]) -> Output {
    let mut all = vec!["profile", "usage"];
    all.extend_from_slice(args);
    run(f, ep, &all)
}

fn describe(out: &Output) -> String {
    format!(
        "status {}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    )
}

fn assert_no_request_for(ep: &Endpoint, p: &str) {
    let reqs = ep.for_profile(p);
    assert!(reqs.is_empty(), "requests made for excluded {p}: {reqs:?}");
}

fn history(f: &Fixture) -> PathBuf {
    f.root.join(".auto-switch.d/usage-history.jsonl")
}

fn usage_file(f: &Fixture) -> PathBuf {
    f.root.join(".auto-switch.d/usage.json")
}

/// The row of `p` in a `profile usage --json` array.
fn json_row(rows: &serde_json::Value, p: &str) -> serde_json::Value {
    rows.as_array()
        .unwrap_or_else(|| panic!("not an array: {rows}"))
        .iter()
        .find(|r| r["profile"] == p)
        .unwrap_or_else(|| panic!("no row for {p}: {rows}"))
        .clone()
}

// --- profile usage (R1, R2) ---------------------------------------------------

#[test]
fn usage_with_no_names_lists_the_excluded_profile_and_probes_nothing_for_it() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let before = stamp(&f.slot("spare"));
    let out = usage(&f, &ep, &[]);
    assert!(out.status.success(), "{}", describe(&out));
    let stdout = text(&out.stdout);
    let row = stdout
        .lines()
        .find(|l| l.trim_start().starts_with("spare"))
        .unwrap_or_else(|| panic!("spare is still listed: {stdout}"));
    assert!(row.contains("excluded (not probed)"), "{row}");
    assert_no_request_for(&ep, "spare");
    assert_eq!(f.usage_probes("spare"), 0, "no `claude /usage` for spare");
    assert_eq!(stamp(&f.slot("spare")), before, "spare's login untouched");
    // The others are still probed.
    assert!(
        ep.requests()
            .iter()
            .any(|r| r.is_usage() && r.for_profile("extra")),
        "extra probed: {:?}",
        ep.requests()
    );
}

#[test]
fn usage_table_view_shows_the_excluded_state() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let out = usage(&f, &ep, &["--table"]);
    assert!(out.status.success(), "{}", describe(&out));
    let stdout = text(&out.stdout);
    let row = stdout
        .lines()
        .find(|l| l.trim_start().starts_with("spare"))
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(row.contains("excluded (not probed)"), "{row}");
    assert!(row.contains("excluded"), "STATE column: {row}");
    assert!(row.contains("n/a"), "SPEND column: {row}");
    assert_no_request_for(&ep, "spare");
}

#[test]
fn usage_json_gives_the_excluded_row_null_fields_and_no_cached_figures() {
    let (f, ep) = fixture();
    // A reading recorded before the exclusion must not reappear.
    let first = usage(&f, &ep, &["--json"]);
    assert!(first.status.success(), "{}", describe(&first));
    exclude(&f, &["spare"]);
    ep.clear();
    let out = usage(&f, &ep, &["--json"]);
    assert!(out.status.success(), "{}", describe(&out));
    let rows: serde_json::Value =
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", describe(&out)));
    let row = json_row(&rows, "spare");
    assert_eq!(row["state"], "excluded", "{row}");
    for key in ["source", "account", "session", "week_all", "error"] {
        assert!(
            row.get(key).is_some_and(serde_json::Value::is_null),
            "{key} present and null: {row}"
        );
    }
    assert_eq!(row["week_models"], serde_json::json!([]), "{row}");
    assert_eq!(row["spend"], "n/a", "{row}");
    let shown = row.to_string();
    assert!(!shown.contains("@example.com"), "no e-mail: {shown}");
    assert!(!shown.contains("uuid-"), "no account: {shown}");
    // Excluded rows sort last.
    let last = rows.as_array().unwrap().last().unwrap();
    assert_eq!(last["profile"], "spare", "excluded row last: {rows}");
    assert_no_request_for(&ep, "spare");
}

#[test]
fn usage_naming_an_excluded_profile_refuses_it_and_reports_the_others() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let before = stamp(&f.slot("spare"));
    let out = usage(&f, &ep, &["spare", "extra"]);
    // D5: extra is read and printed, spare is refused, and the run exits
    // non-zero once every other name is done.
    assert_ne!(out.status.code(), Some(0), "{}", describe(&out));
    let stderr = text(&out.stderr);
    assert!(
        stderr.lines().any(|l| l
            == "spare: excluded (in auto-switch exclude list); not probed \
                (amplihack profile auto-switch exclude)"),
        "{}",
        describe(&out)
    );
    assert!(
        ep.requests()
            .iter()
            .any(|r| r.is_usage() && r.for_profile("extra")),
        "extra was probed before the refusal exit: {:?}",
        ep.requests()
    );
    let stdout = text(&out.stdout);
    assert!(
        stdout.lines().any(|l| l.trim_start().starts_with("extra")),
        "extra still reported: {}",
        describe(&out)
    );
    // The refused name is still listed, as not probed.
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("spare") && l.contains("excluded (not probed)")),
        "spare listed as not probed: {}",
        describe(&out)
    );
    assert_no_request_for(&ep, "spare");
    assert_eq!(f.usage_probes("spare"), 0);
    assert_eq!(stamp(&f.slot("spare")), before);
}

#[test]
fn usage_naming_only_an_excluded_profile_exits_non_zero_and_probes_nothing() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let out = usage(&f, &ep, &["spare"]);
    assert_ne!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(
        text(&out.stderr).contains(
            "spare: excluded (in auto-switch exclude list); not probed \
             (amplihack profile auto-switch exclude)"
        ),
        "{}",
        describe(&out)
    );
    assert_no_request_for(&ep, "spare");
}

#[test]
fn an_unknown_name_still_takes_precedence_over_an_excluded_one() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let out = usage(&f, &ep, &["nope", "spare"]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("nope"), "unknown profile named: {stderr}");
    assert!(
        !stderr.contains("excluded (in auto-switch exclude list)"),
        "the setup error comes first: {stderr}"
    );
    assert!(
        ep.requests().is_empty(),
        "nothing probed: {:?}",
        ep.requests()
    );
}

#[test]
fn usage_writes_no_history_and_leaves_an_old_usage_json_entry_alone() {
    let (f, ep) = fixture();
    // Before the exclusion: a whole run records spare in both files.
    let first = usage(&f, &ep, &[]);
    assert!(first.status.success(), "{}", describe(&first));
    let read_usage = |f: &Fixture| -> serde_json::Value {
        serde_json::from_slice(&fs::read(usage_file(f)).unwrap()).unwrap()
    };
    let old = read_usage(&f);
    let old_spare = old.to_string();
    assert!(old_spare.contains("\"spare\""), "recorded first: {old}");
    let lines_before = fs::read_to_string(history(&f))
        .unwrap_or_default()
        .lines()
        .count();

    exclude(&f, &["spare"]);
    ep.clear();
    let out = usage(&f, &ep, &[]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_no_request_for(&ep, "spare");

    // The history line of this run names no spare, not even as unknown.
    let hist = fs::read_to_string(history(&f)).unwrap();
    let lines: Vec<&str> = hist.lines().collect();
    assert_eq!(lines.len(), lines_before + 1, "one new line: {hist}");
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert!(last["profiles"].get("spare").is_none(), "{last}");
    assert!(
        !last
            .get("unknown")
            .is_some_and(|u| u.to_string().contains("spare")),
        "{last}"
    );
    // The older usage.json entry stays exactly as it was.
    let now = read_usage(&f);
    let pick = |v: &serde_json::Value| -> serde_json::Value {
        v.get("accounts")
            .or_else(|| v.get("profiles"))
            .and_then(|p| p.get("spare"))
            .or_else(|| v.get("spare"))
            .cloned()
            .unwrap_or_else(|| panic!("no spare entry: {v}"))
    };
    assert_eq!(pick(&now), pick(&old), "old entry left as it is");
}

#[test]
fn usage_summary_leaves_the_excluded_profile_out_of_every_figure() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let out = usage(&f, &ep, &[]);
    assert!(out.status.success(), "{}", describe(&out));
    let stdout = text(&out.stdout);
    // work 90/40 and extra 50/50: 60 + 50 points of 200.
    assert!(
        stdout.contains("\nusable now           2 of 2 logins"),
        "N counts only probed logins: {stdout}"
    );
    assert!(
        stdout.contains("\nunknown              0 profiles"),
        "an excluded profile is not unknown: {stdout}"
    );
    assert!(stdout.contains("login-weeks of 2 "), "{stdout}");
}

/// A history line written before the fix, a minute ago, that reads `spare`
/// at 0/0 beside `work` and `extra`, and one rotate line before it that
/// lists `spare` as unknown.
fn seed_old_history(f: &Fixture) {
    let now = chrono::Utc::now();
    let at = |secs: i64| (now - chrono::Duration::seconds(secs)).to_rfc3339();
    let pt = |s: u8, w: u8| serde_json::json!({"s": s, "w": w, "w_reset": null});
    let lines = [
        serde_json::json!({"at": at(3 * 3600), "by": "rotate",
            "profiles": {"work": pt(80, 30), "extra": pt(40, 45)},
            "unknown": ["spare"]}),
        serde_json::json!({"at": at(60), "by": "usage",
            "profiles": {"work": pt(90, 40), "extra": pt(50, 50), "spare": pt(0, 0)},
            "unknown": []}),
    ];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    private_file(&history(f), &body);
}

#[test]
fn rotate_status_does_not_count_an_excluded_profile_from_history_written_before_the_fix() {
    let (f, ep) = fixture();
    seed_old_history(&f);
    let seeded = run(&f, &ep, &["profile", "rotate", "status"]);
    assert!(
        text(&seeded.stdout).contains("login-weeks of 3 "),
        "the seeded line counts spare: {}",
        describe(&seeded)
    );
    exclude(&f, &["spare"]);
    let before = fs::read(history(&f)).unwrap();
    let out = run(&f, &ep, &["profile", "rotate", "status"]);
    assert!(out.status.success(), "{}", describe(&out));
    let stdout = text(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.trim_start().starts_with("capacity:"))
        .unwrap_or_else(|| panic!("no capacity line: {}", describe(&out)));
    // work 40 and extra 50 used: 60 + 50 points of 2 logins.
    assert!(line.contains("headroom 1.10 login-weeks of 2 "), "{line}");
    assert!(line.contains("usable 2 of 2 logins"), "{line}");
    assert!(!line.contains("spare"), "{line}");
    assert_eq!(
        fs::read(history(&f)).unwrap(),
        before,
        "history not rewritten"
    );
    assert_no_request_for(&ep, "spare");
}

#[test]
fn usage_does_not_count_an_excluded_profile_from_history_written_before_the_fix() {
    let (f, ep) = fixture();
    seed_old_history(&f);
    exclude(&f, &["spare"]);
    let out = usage(&f, &ep, &[]);
    assert!(out.status.success(), "{}", describe(&out));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("\nusable now           2 of 2 logins"),
        "{stdout}"
    );
    assert!(stdout.contains("login-weeks of 2 "), "{stdout}");
    assert!(
        stdout.contains("\nunknown              0 profiles"),
        "an excluded profile in an old line is not unknown: {stdout}"
    );
    assert_no_request_for(&ep, "spare");
}

#[test]
fn an_unsafe_config_still_applies_its_exclude_list() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    // Group-writable: unsafe for overflow and the other settings, but an
    // exclude entry only takes actions away, so it still applies.
    let toggle = f.root.join(".auto-switch");
    fs::set_permissions(&toggle, fs::Permissions::from_mode(0o666)).unwrap();
    let before = stamp(&f.slot("spare"));
    let out = usage(&f, &ep, &[]);
    let stderr = text(&out.stderr);
    assert_eq!(
        stderr.matches(UNTRUSTED).count(),
        1,
        "one warning: {}",
        describe(&out)
    );
    assert!(
        text(&out.stdout).contains("excluded"),
        "spare is shown excluded: {}",
        describe(&out)
    );
    assert_no_request_for(&ep, "spare");
    assert_eq!(stamp(&f.slot("spare")), before, "same bytes and mtime");
}

/// Writes a `.auto-switch` that does not parse, mode 0600.
fn corrupt_config(f: &Fixture) {
    let toggle = f.root.join(".auto-switch");
    fs::write(&toggle, b"enabled=maybe\nexclude=spare\n").unwrap();
    fs::set_permissions(&toggle, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn usage_with_an_unreadable_config_probes_and_refreshes_no_profile() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    corrupt_config(&f);
    let stamps: Vec<Stamp> = ["spare", "extra"].map(|p| stamp(&f.slot(p))).into();
    let home = stamp(&f.home_login());
    let out = usage(&f, &ep, &[]);
    let stderr = text(&out.stderr);
    assert_eq!(
        stderr.matches(UNREADABLE).count(),
        1,
        "one warning: {}",
        describe(&out)
    );
    assert!(
        ep.requests().is_empty(),
        "no request at all: {:?}\n{}",
        ep.requests(),
        describe(&out)
    );
    let after: Vec<Stamp> = ["spare", "extra"].map(|p| stamp(&f.slot(p))).into();
    assert_eq!(after, stamps, "same bytes and mtime");
    assert_eq!(stamp(&f.home_login()), home);
}

#[test]
fn verify_with_an_unreadable_config_reads_no_profile() {
    let (f, ep) = fixture();
    corrupt_config(&f);
    let out = run(&f, &ep, &["profile", "verify"]);
    assert_eq!(
        text(&out.stderr).matches(UNREADABLE).count(),
        1,
        "{}",
        describe(&out)
    );
    assert!(ep.requests().is_empty(), "{:?}", ep.requests());
}

#[test]
fn a_cycle_with_an_unreadable_config_pauses_and_sends_nothing() {
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    corrupt_config(&f);
    let home = stamp(&f.home_login());
    let spare = stamp(&f.slot("spare"));
    let _ = cycle(&f, &ep);
    let log = f.log();
    assert!(log.contains(UNREADABLE), "{log}");
    assert!(
        log.contains("ALERT: auto-switch config unreadable; rotation paused, no profile probed"),
        "{log}"
    );
    assert!(ep.requests().is_empty(), "{:?}\n{log}", ep.requests());
    assert_eq!(live(&f), "work", "nothing moved: {log}");
    assert_eq!(stamp(&f.home_login()), home, "same bytes and mtime");
    assert_eq!(stamp(&f.slot("spare")), spare);
    assert!(
        !f.rotate_dir().join("journal.json").exists(),
        "no swap began"
    );
}

// --- profile seed-record -------------------------------------------------------

const PAUSED: &str = "auto-switch config unreadable; rotation paused, no profile probed";

#[test]
fn a_dry_run_with_an_unreadable_config_would_pause_like_a_cycle() {
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    corrupt_config(&f);
    let home = stamp(&f.home_login());
    let out = run(&f, &ep, &["profile", "rotate", "--dry-run"]);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains(&format!("would alert: {PAUSED}")),
        "{}",
        describe(&out)
    );
    assert!(
        !stdout.lines().any(|l| l.contains("cycle")),
        "nothing measured: {}",
        describe(&out)
    );
    assert!(ep.requests().is_empty(), "{:?}", ep.requests());
    assert_eq!(stamp(&f.home_login()), home, "same bytes and mtime");
}

fn toggle_text(f: &Fixture) -> String {
    fs::read_to_string(f.root.join(".auto-switch")).unwrap()
}

#[test]
fn auto_switch_on_over_an_unsafe_config_keeps_its_exclude_list() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let toggle = f.root.join(".auto-switch");
    fs::set_permissions(&toggle, fs::Permissions::from_mode(0o666)).unwrap();
    f.ok(&["profile", "auto-switch", "on"]);
    assert!(
        toggle_text(&f).contains("exclude=spare\n"),
        "{}",
        toggle_text(&f)
    );
    // The rewritten file is trusted again, and `spare` is still excluded.
    let out = usage(&f, &ep, &[]);
    assert!(!text(&out.stderr).contains(UNTRUSTED), "{}", describe(&out));
    assert_no_request_for(&ep, "spare");
}

#[test]
fn auto_switch_on_or_off_over_an_unparseable_config_is_refused_without_exclude() {
    let (f, ep) = fixture();
    corrupt_config(&f);
    let before = toggle_text(&f);
    for args in [
        &["profile", "auto-switch", "on"][..],
        &["profile", "auto-switch", "off"][..],
    ] {
        let out = run(&f, &ep, args);
        assert!(!out.status.success(), "{args:?}: {}", describe(&out));
        assert!(
            text(&out.stderr).contains("exclude list is unknown"),
            "{args:?}: {}",
            describe(&out)
        );
        assert_eq!(toggle_text(&f), before, "{args:?}: file unchanged");
    }
    // Naming the list is the way out, and it is the list that is written.
    f.ok(&["profile", "auto-switch", "on", "--exclude", "spare"]);
    assert!(
        toggle_text(&f).contains("exclude=spare\n"),
        "{}",
        toggle_text(&f)
    );
}

#[test]
fn auto_switch_status_shows_the_exclude_list_the_rotator_applies() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    fs::set_permissions(
        f.root.join(".auto-switch"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    let out = run(&f, &ep, &["profile", "auto-switch", "status"]);
    assert!(
        text(&out.stdout).contains("exclude: spare"),
        "untrusted: {}",
        describe(&out)
    );
    corrupt_config(&f);
    let out = run(&f, &ep, &["profile", "auto-switch", "status"]);
    let line = text(&out.stdout)
        .lines()
        .find(|l| l.starts_with("exclude: "))
        .unwrap_or_default()
        .to_string();
    assert!(
        line.contains("spare") && line.contains("extra"),
        "unreadable: every profile: {}",
        describe(&out)
    );
}

#[test]
fn seed_record_for_an_excluded_profile_with_no_record_sends_no_request() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let record = f.root.join(".auto-switch.d/expected/spare.json");
    if record.exists() {
        fs::remove_file(&record).unwrap();
    }
    let before = stamp(&f.slot("spare"));
    let out = run(&f, &ep, &["profile", "seed-record", "spare"]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    // The identity comes from spare's local `.claude.json` only; whatever
    // it lacks, the outcome is an existing refusal and nothing is written.
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("seed-record spare: refused: ") && stderr.contains("nothing written"),
        "{}",
        describe(&out)
    );
    assert_no_request_for(&ep, "spare");
    assert_eq!(stamp(&f.slot("spare")), before);
}

#[test]
fn seed_record_for_an_excluded_live_profile_sends_no_request() {
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    let before = stamp(&f.home_login());
    let out = run(&f, &ep, &["profile", "seed-record", "work"]);
    assert_no_request_for(&ep, "work");
    assert_eq!(stamp(&f.home_login()), before, "{}", describe(&out));
}

// --- profile verify (R3) ------------------------------------------------------

#[test]
fn verify_marks_an_excluded_profile_not_probed_and_reads_nothing_for_it() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let before = stamp(&f.slot("spare"));
    let out = run(&f, &ep, &["profile", "verify"]);
    let stdout = text(&out.stdout);
    assert!(
        stdout.lines().any(|l| l == "spare: excluded (not probed)"),
        "{}",
        describe(&out)
    );
    assert_no_request_for(&ep, "spare");
    assert_eq!(stamp(&f.slot("spare")), before);
    // The other profiles are still checked.
    assert!(
        ep.requests()
            .iter()
            .any(|r| r.is_identity() && r.for_profile("extra")),
        "{:?}",
        ep.requests()
    );
}

#[test]
fn verify_skips_the_home_row_when_the_live_profile_is_excluded() {
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    let before = stamp(&f.home_login());
    let out = run(&f, &ep, &["profile", "verify"]);
    let stdout = text(&out.stdout);
    assert!(
        stdout
            .lines()
            .any(|l| l == "~/.claude: excluded (not probed), live profile work"),
        "{}",
        describe(&out)
    );
    assert_no_request_for(&ep, "work");
    assert_eq!(stamp(&f.home_login()), before);
}

/// The ledger with no `live` field, so only rule 2 or rule 3 of the local
/// proof can name the live login.
fn drop_ledger_live(f: &Fixture) {
    let t = chrono::Utc::now().timestamp() - 1200;
    private_file(
        &f.rotate_dir().join("ledger.json"),
        &serde_json::json!({"version": 1, "since": t, "swapped_at": t}).to_string(),
    );
}

#[test]
fn verify_skips_the_home_row_when_only_claude_json_names_the_excluded_profile() {
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    drop_ledger_live(&f);
    let before = stamp(&f.home_login());
    let out = run(&f, &ep, &["profile", "verify"]);
    assert!(
        text(&out.stdout)
            .lines()
            .any(|l| l == "~/.claude: excluded (not probed), live profile work"),
        "{}",
        describe(&out)
    );
    assert_no_request_for(&ep, "work");
    assert_eq!(stamp(&f.home_login()), before);
}

#[test]
fn usage_home_row_is_not_read_when_only_claude_json_names_the_excluded_profile() {
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    drop_ledger_live(&f);
    let before = stamp(&f.home_login());
    let out = usage(&f, &ep, &[]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_no_request_for(&ep, "work");
    assert_eq!(stamp(&f.home_login()), before);
}

#[test]
fn usage_marks_an_excluded_live_login_live_without_reading_it() {
    // Rule 3 only: no ledger `live`, no ~/.claude.json, and ~/.claude holds
    // a byte-for-byte copy of extra's login. The excluded row is the one
    // marked live, and nothing is read for it.
    let (f, ep) = fixture();
    exclude(&f, &["extra"]);
    drop_ledger_live(&f);
    fs::remove_file(f.home.join(".claude.json")).unwrap();
    let bytes = fs::read(f.slot("extra")).unwrap();
    fs::remove_file(f.home_login()).unwrap();
    fs::write(f.home_login(), &bytes).unwrap();
    fs::set_permissions(f.home_login(), fs::Permissions::from_mode(0o600)).unwrap();
    let before = stamp(&f.home_login());
    let out = usage(&f, &ep, &["--json"]);
    assert!(out.status.success(), "{}", describe(&out));
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let extra = json_row(&rows, "extra");
    assert_eq!(extra["state"], "excluded", "{rows}");
    assert_eq!(extra["live_in"], "~/.claude", "{rows}");
    let others_live = rows
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["profile"] != "extra" && !r["live_in"].is_null())
        .count();
    assert_eq!(others_live, 0, "only extra is live: {rows}");
    let bar = usage(&f, &ep, &[]);
    assert!(
        text(&bar.stdout)
            .lines()
            .any(|l| l.starts_with("extra (live)") && l.contains("excluded (not probed)")),
        "{}",
        describe(&bar)
    );
    assert_no_request_for(&ep, "extra");
    assert_eq!(stamp(&f.home_login()), before);
}

#[test]
fn verify_fix_never_touches_an_excluded_profile() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let before = stamp(&f.slot("spare"));
    let _ = run(&f, &ep, &["profile", "verify", "--fix"]);
    assert_no_request_for(&ep, "spare");
    assert_eq!(stamp(&f.slot("spare")), before);
    assert!(
        !f.root.join(".rejected-logins").exists()
            || fs::read_dir(f.root.join(".rejected-logins"))
                .unwrap()
                .next()
                .is_none(),
        "no backup made"
    );
}

// --- rotator (R4, R5) ---------------------------------------------------------

fn cycle(f: &Fixture, ep: &Endpoint) -> Output {
    run(f, ep, &["profile", "rotate"])
}

fn live(f: &Fixture) -> String {
    f.ledger().unwrap()["live"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn an_excluded_live_login_is_moved_off_on_the_first_cycle_without_probe_or_refresh() {
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    let before = stamp(&f.home_login());
    let out = cycle(&f, &ep);
    let log = f.log();
    assert!(out.status.success(), "{}\nlog:\n{log}", describe(&out));

    // S1: no request of any kind with work's token, the swap's identity
    // checks included: they take work's identity from local files.
    assert_no_request_for(&ep, "work");
    // S5: the incoming target is still verified over the network.
    assert!(
        ep.requests()
            .iter()
            .any(|r| r.is_identity() && r.for_profile("spare")),
        "the target's identity is read: {:?}",
        ep.requests()
    );

    // The first cycle swaps; damping, hold and the home key do not apply.
    assert_eq!(live(&f), "spare", "moved to the best candidate: {log}");
    assert!(log.contains("excluded (not probed)"), "{log}");
    assert!(
        log.contains("live profile work is excluded; moving off it"),
        "{log}"
    );
    // The fixture's `~/.claude.json` holds a key, so the home-key ALERT of
    // issue #1804 is logged; no other ALERT is.
    assert!(
        !log.lines()
            .any(|l| l.contains("ALERT") && !l.contains("ALERT: home_key")),
        "{log}"
    );
    // work's login is back in its own folder, unchanged.
    assert_eq!(stamp(&f.slot("work")), before, "same bytes and mtime");

    // Nothing measured for work is recorded anywhere.
    let hist = fs::read_to_string(history(&f)).unwrap_or_default();
    for line in hist.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(v["profiles"].get("work").is_none(), "{line}");
    }
    if let Ok(raw) = fs::read_to_string(usage_file(&f)) {
        assert!(!raw.contains("\"work\""), "no usage.json entry: {raw}");
    }

    // The next cycle never swaps work back in, and sends nothing for it.
    ep.clear();
    let out = cycle(&f, &ep);
    assert!(out.status.success(), "{}", describe(&out));
    assert_ne!(live(&f), "work", "{}", f.log());
    assert_no_request_for(&ep, "work");
    assert_eq!(stamp(&f.slot("work")), before);
}

#[test]
fn an_excluded_live_login_with_no_candidate_alerts_and_stays() {
    let (f, ep) = fixture();
    exclude(&f, &["work", "spare", "extra"]);
    let before = stamp(&f.home_login());
    let _ = cycle(&f, &ep);
    let log = f.log();
    assert!(
        log.contains("ALERT: live profile work is excluded; no eligible candidate"),
        "{log}"
    );
    assert_eq!(live(&f), "work", "{log}");
    assert_eq!(stamp(&f.home_login()), before, "~/.claude still holds work");
    assert!(
        !f.rotate_dir().join("journal.json").exists(),
        "no swap began"
    );
    for p in ["work", "spare", "extra"] {
        assert_no_request_for(&ep, p);
    }
    // D4: a second cycle tries again and still sends nothing.
    ep.clear();
    let _ = cycle(&f, &ep);
    assert_no_request_for(&ep, "work");
    assert_eq!(stamp(&f.home_login()), before);
}

#[test]
fn an_excluded_live_login_named_only_by_claude_json_is_not_moved_and_sends_no_request() {
    // Rule 2: the ledger has no `live`, ~/.claude.json matches work's record.
    // A match by the local account alone may be a stale file, so the login
    // is not moved into work's folder (S4): the cycle alerts and stays.
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    drop_ledger_live(&f);
    let (home, slot) = (stamp(&f.home_login()), fs::read(f.slot("work")).ok());
    let out = cycle(&f, &ep);
    let log = f.log();
    assert!(out.status.success(), "{}\nlog:\n{log}", describe(&out));
    assert_no_request_for(&ep, "work");
    assert!(
        log.contains(
            "ALERT: live profile work is excluded; not moved, as only ~/.claude.json names it"
        ),
        "{log}"
    );
    assert_eq!(live(&f), "", "nothing recorded live: {log}");
    assert_eq!(stamp(&f.home_login()), home, "~/.claude untouched");
    assert_eq!(
        fs::read(f.slot("work")).ok(),
        slot,
        "work's folder untouched"
    );
}

#[test]
fn a_session_still_on_a_moved_off_excluded_login_does_not_stall_rotation() {
    // After the move off excluded work, a session still running on work
    // writes work's account into ~/.claude.json. The login the swap put in
    // ~/.claude is unchanged since `swapped_at`, so the ledger's spare is
    // trusted: spare is measured and nothing is refused or sent for work.
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    let out = cycle(&f, &ep);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(live(&f), "spare", "{}", f.log());
    private_file(
        &f.home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": account_json("work")}).to_string(),
    );
    ep.clear();
    let out = cycle(&f, &ep);
    let log = f.log();
    assert!(out.status.success(), "{}\nlog:\n{log}", describe(&out));
    assert_no_request_for(&ep, "work");
    assert!(
        !log.contains("not moved, as only ~/.claude.json names it"),
        "{log}"
    );
    assert!(
        ep.requests()
            .iter()
            .any(|r| r.is_usage() && r.for_profile("spare")),
        "the live login is measured: {:?}\n{log}",
        ep.requests()
    );
    let out = run(&f, &ep, &["profile", "verify"]);
    assert!(
        !text(&out.stdout).contains("live profile work"),
        "{}",
        describe(&out)
    );
    assert_no_request_for(&ep, "work");
}

#[test]
fn an_excluded_live_login_named_only_by_its_bytes_sends_no_request() {
    // Rule 3: no ledger `live`, no ~/.claude.json account, and ~/.claude
    // holds a copy of extra's credentials byte for byte.
    let (f, ep) = fixture();
    exclude(&f, &["extra"]);
    drop_ledger_live(&f);
    fs::remove_file(f.home.join(".claude.json")).unwrap();
    let bytes = fs::read(f.slot("extra")).unwrap();
    fs::remove_file(f.home_login()).unwrap();
    fs::write(f.home_login(), &bytes).unwrap();
    fs::set_permissions(f.home_login(), fs::Permissions::from_mode(0o600)).unwrap();
    let before = stamp(&f.slot("extra"));
    let _ = cycle(&f, &ep);
    assert_no_request_for(&ep, "extra");
    assert_eq!(stamp(&f.slot("extra")), before, "{}", f.log());
}

#[test]
fn a_suspected_hand_change_never_overwrites_the_excluded_folder() {
    // S4: ~/.claude.json names excluded work, but a hand change is suspected,
    // so the local match is not proof enough to copy ~/.claude into work's
    // folder. work's folder holds an older login of its own.
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    fake_login(&f.slot("work"), "work", in_an_hour());
    let t = chrono::Utc::now().timestamp() - 1200;
    private_file(
        &f.rotate_dir().join("ledger.json"),
        &serde_json::json!({"version": 1, "live": "spare", "account": "uuid-spare",
            "since": t, "swapped_at": t,
            "hand_suspect": {"profile": "work", "first_seen": t}})
        .to_string(),
    );
    let before = stamp(&f.slot("work"));
    let _ = cycle(&f, &ep);
    assert_no_request_for(&ep, "work");
    assert_eq!(stamp(&f.slot("work")), before, "{}", f.log());
}

#[test]
fn rotate_status_names_an_excluded_live_login_from_claude_json_without_a_request() {
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    drop_ledger_live(&f);
    let before = stamp(&f.home_login());
    let out = run(&f, &ep, &["profile", "rotate", "status"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_no_request_for(&ep, "work");
    assert!(
        text(&out.stdout).lines().any(|l| l == "live: work"),
        "{}",
        describe(&out)
    );
    assert_eq!(stamp(&f.home_login()), before);
}

#[test]
fn a_dry_run_sends_no_request_for_an_excluded_live_login() {
    let (f, ep) = fixture();
    exclude(&f, &["work"]);
    let before = stamp(&f.home_login());
    let out = run(&f, &ep, &["profile", "rotate", "--dry-run"]);
    assert_no_request_for(&ep, "work");
    assert_eq!(stamp(&f.home_login()), before, "{}", describe(&out));
}

#[test]
fn an_excluded_candidate_gets_no_request_and_no_write() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let before = stamp(&f.slot("spare"));
    let out = cycle(&f, &ep);
    let log = f.log();
    assert!(out.status.success(), "{}\nlog:\n{log}", describe(&out));
    assert_no_request_for(&ep, "spare");
    assert_eq!(stamp(&f.slot("spare")), before);
    // work at 90% rotates to extra, the only eligible candidate.
    assert_eq!(live(&f), "extra", "{log}");
    assert!(log.contains("skip: spare (excluded)"), "{log}");
    assert!(!log.contains("spare (no account)"), "never unbound: {log}");
}

#[test]
fn an_expired_excluded_live_login_is_not_refreshed() {
    let (f, ep) = fixture();
    fake_login(
        &f.home_login(),
        "work",
        chrono::Utc::now().timestamp_millis() - 60_000,
    );
    exclude(&f, &["work"]);
    let before = stamp(&f.home_login());
    let _ = cycle(&f, &ep);
    let log = f.log();
    assert!(
        !ep.requests().iter().any(Req::is_token),
        "no token request at all: {:?}",
        ep.requests()
    );
    assert!(
        log.contains("live profile work is excluded; token not refreshed"),
        "{log}"
    );
    assert!(
        !log.contains("expired login in ~/.claude refreshed"),
        "{log}"
    );
    assert_no_request_for(&ep, "work");
    // Either still in ~/.claude or moved to its folder; never rewritten.
    let now = if f.home_login().exists() && live(&f) == "work" {
        stamp(&f.home_login())
    } else {
        stamp(&f.slot("work"))
    };
    assert_eq!(now, before, "work's login is not rewritten");
}

#[test]
fn an_expired_excluded_live_login_named_only_by_claude_json_is_not_refreshed() {
    // The prelude refresh gate uses the same local proof, rule 2 included.
    let (f, ep) = fixture();
    fake_login(
        &f.home_login(),
        "work",
        chrono::Utc::now().timestamp_millis() - 60_000,
    );
    exclude(&f, &["work"]);
    drop_ledger_live(&f);
    let _ = cycle(&f, &ep);
    let log = f.log();
    assert!(
        !ep.requests().iter().any(Req::is_token),
        "no token request at all: {:?}\nlog:\n{log}",
        ep.requests()
    );
    assert_no_request_for(&ep, "work");
}

// --- status line, capacity summary, limit wait (R7) ---------------------------

#[test]
fn the_status_line_makes_no_request_for_an_excluded_profile() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let before = stamp(&f.slot("spare"));
    let mut c = f.cmd(&["profile", "statusline"]);
    ep.configure(&mut c);
    c.env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let input = serde_json::json!({
        "session_id": "s", "model": {"id": "m"},
        "rate_limits": {
            "five_hour": {"used_percentage": 34.0,
                "resets_at": chrono::Utc::now().timestamp() + 3600},
            "seven_day": {"used_percentage": 43.0,
                "resets_at": chrono::Utc::now().timestamp() + 3 * 86_400}
        }
    })
    .to_string();
    let mut pipe = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = pipe.write_all(input.as_bytes());
    });
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(
        ep.requests().is_empty(),
        "{:?}\n{}",
        ep.requests(),
        describe(&out)
    );
    assert_eq!(stamp(&f.slot("spare")), before);
}

#[test]
fn rotate_status_capacity_line_makes_no_request_for_an_excluded_profile() {
    let (f, ep) = fixture();
    exclude(&f, &["spare"]);
    let before = stamp(&f.slot("spare"));
    let out = run(&f, &ep, &["profile", "rotate", "status"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        ep.requests().is_empty(),
        "{:?}\n{}",
        ep.requests(),
        describe(&out)
    );
    assert_eq!(stamp(&f.slot("spare")), before);
}

#[test]
fn the_usage_limit_wait_makes_no_request_for_an_excluded_profile() {
    use crate::issue_1618_fixture::Fixture as WaitFixture;
    use crate::issue_1618_wait_profiles::with_profiles;
    use crate::issue_1801_capacity_wait::{rotator_running, x_is_live};
    let f: WaitFixture = with_profiles();
    let ep = Endpoint::new();
    let excl = f.run(&mut f.cmd(&["profile", "auto-switch", "exclude", "Y"]));
    assert!(excl.status.success(), "{}", describe(&excl));
    let y = f.profiles.join("Y").join(".credentials.json");
    let before = stamp(&y);
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env(
            "LIMIT_MESSAGE",
            "You've hit your session limit · try again later",
        )
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "0");
    ep.configure(&mut c);
    let out = f.run(&mut c);
    assert!(
        ep.requests().is_empty(),
        "{:?}\n{}",
        ep.requests(),
        describe(&out)
    );
    assert_eq!(stamp(&y), before);
}
