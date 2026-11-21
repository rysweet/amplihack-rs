//! Issue #2889 out of process: the rotator decides on the freshest reading
//! it holds for the live login, and says when the reading it decided on was
//! cached and how old it is.
//!
//! These are outside-in scenarios. They run the installed `amplihack`
//! binary against a temporary `HOME`, a temporary profile root and a
//! loopback usage endpoint, and read only what an operator reads: the
//! `profile rotate` output and the rotate log. Nothing reaches into the
//! rotator's own types.
//!
//! The incident (candide, 21 Nov) was a live login whose `cycle` line
//! alternated between a fresh reading and the cached one taken when the
//! login was swapped in, with nothing on the line to tell them apart. The
//! two scenarios below pin the two halves of the fix at that boundary:
//!
//! - a cached reading the cycle decides on is labelled `cached=<n>s`, on
//!   the `cycle` line and on the decision line;
//! - a fresh re-read beats an older cached reading, and the line the
//!   operator reads shows the fresh figures, with no `cached=` on them.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::{amplihack_bin, assert_log_grammar, private_file};
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// The live login's session figure the usage endpoint reports when it is
/// asked directly. Chosen well above the `--session-at 70` these scenarios
/// run with, so a decision made on it is unmistakable.
const LIVE_DIRECT_SESSION: u8 = 90;
/// The live login's weekly figure from the endpoint, distinct from the
/// weekly figure the cached readings carry.
const LIVE_DIRECT_WEEK: u8 = 40;
/// What the spare login reports; under the threshold, so it has room.
const SPARE_SESSION: u8 = 45;

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    address: SocketAddr,
    /// A usage request carried a bearer token that is not a fixture
    /// login's. Recorded rather than panicked on, so a fixture fault never
    /// hides the test's own assertion.
    unknown_token: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let unknown_token = Arc::new(AtomicBool::new(false));
        let unknown = Arc::clone(&unknown_token);
        let worker = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut input = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                input.read_line(&mut line).unwrap();
                if line.contains(" /__shutdown ") {
                    break;
                }
                let profile = line.starts_with("GET /api/oauth/profile ");
                let known = profile || line.starts_with("GET /api/oauth/usage ");
                let mut name = None;
                loop {
                    line.clear();
                    if input.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("authorization")
                    {
                        name = match value.trim() {
                            "Bearer synthetic-work-2889" => Some("work"),
                            "Bearer synthetic-spare-2889" => Some("spare"),
                            _ => None,
                        };
                    }
                }
                let Some(name) = name.filter(|_| known) else {
                    if known {
                        unknown.store(true, Ordering::SeqCst);
                    }
                    let _ = write!(
                        stream,
                        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = stream.flush();
                    continue;
                };
                let session = if name == "work" {
                    LIVE_DIRECT_SESSION
                } else {
                    SPARE_SESSION
                };
                let body = if profile {
                    json!({"account":{"uuid":format!("uuid-{name}"),
                        "email":format!("{name}@example.com")}, "organization":{"uuid":null}})
                } else {
                    json!({"five_hour":{"utilization":session,
                            "resets_at":"2099-01-01T00:00:00Z"},
                        "seven_day":{"utilization":LIVE_DIRECT_WEEK,
                            "resets_at":"2099-01-02T00:00:00Z"},
                        "extra_usage":{"is_enabled":false}})
                }
                .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
                stream.flush().unwrap();
            }
        });
        let f = Self {
            root: home.join("profiles"),
            _tmp: tmp,
            home,
            address,
            unknown_token,
            worker: Some(worker),
        };
        for name in ["work", "spare"] {
            f.run(&["profile", "add", name]);
            private_file(
                &f.root.join(name).join(".claude.json"),
                &json!({"oauthAccount":{"accountUuid":format!("uuid-{name}"),
                    "emailAddress":format!("{name}@example.com")}})
                .to_string(),
            );
            private_file(
                &f.root.join(format!(".auto-switch.d/expected/{name}.json")),
                &json!({"account":format!("uuid-{name}"),"org":null}).to_string(),
            );
        }
        for dir in [
            &f.root,
            &f.root.join(".auto-switch.d"),
            &f.root.join(".auto-switch.d/expected"),
        ] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self::credentials(&f.home.join(".claude/.credentials.json"), "work");
        Self::credentials(&f.root.join("spare/.credentials.json"), "spare");
        fs::set_permissions(f.home.join(".claude"), fs::Permissions::from_mode(0o700)).unwrap();
        private_file(
            &f.home.join(".claude.json"),
            &json!({"oauthAccount":{"accountUuid":"uuid-work","emailAddress":"work@example.com"}})
                .to_string(),
        );
        f
    }

    fn credentials(path: &Path, name: &str) {
        private_file(
            path,
            &json!({"claudeAiOauth":{
                "accessToken":format!("synthetic-{name}-2889"),
                "refreshToken":"synthetic-refresh-2889","expiresAt":4_102_444_800_000_i64
            }})
            .to_string(),
        );
    }

    fn run(&self, args: &[&str]) -> String {
        self.run_with(args, &[])
    }

    /// Runs the CLI with the fixture's environment, then `env` on top (a
    /// later value for the same key wins), and returns stdout.
    fn run_with(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let mut cmd = crate::isolated_amplihack::isolated(amplihack_bin());
        cmd.args(args)
            .current_dir(&self.home)
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root)
            .env(
                "AMPLIHACK_PROFILE_USAGE_API_BASE",
                format!("http://{}", self.address),
            )
            .env("AMPLIHACK_PROFILE_USAGE_PROBE", "direct")
            .env(
                "AMPLIHACK_PROFILE_USAGE_TOKEN_URL",
                format!("http://{}/v1/oauth/token", self.address),
            )
            .env("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "0")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("CODEX_PROFILES_ROOT", self.home.join("no-codex-profiles"))
            .env("CODEX_BIN", self.home.join("no-codex"));
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "CLI failed with {}: {stderr}",
            out.status
        );
        assert!(!stderr.contains("synthetic-"));
        assert!(!stdout.contains("synthetic-"));
        crate::isolated_amplihack::assert_proc_root_honoured(&stderr);
        stdout
    }

    /// Plants a passive (status-line) reading for `name` in auto-switch's
    /// `usage.json`, observed `age` seconds before the fixture's own clock.
    /// This is the cached reading of the incident: taken earlier, carried
    /// into the cycle, and never re-measured by the cycle itself. The age
    /// only grows while the test runs, so no timing window exists.
    fn passive(&self, name: &str, session: u8, week: u8, age: i64) {
        let now = now();
        private_file(
            &self.root.join(".auto-switch.d/usage.json"),
            &json!({"version": 1, "accounts": {name: {
                "five_hour": {"used_percentage": session, "resets_at": now + 3 * 3600},
                "seven_day": {"used_percentage": week, "resets_at": now + 3 * 86_400},
                "observed_at": now - age,
                "source": "statusline",
            }}})
            .to_string(),
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        TcpStream::connect(self.address)
            .unwrap()
            .write_all(b"GET /__shutdown HTTP/1.1\r\n\r\n")
            .unwrap();
        self.worker.take().unwrap().join().unwrap();
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// The one line of `out` starting with `prefix`, or a panic naming the
/// output. Every scenario asserts on exactly one such line.
fn one_line<'a>(out: &'a str, prefix: &str) -> &'a str {
    let mut found = out.lines().filter(|l| l.starts_with(prefix));
    let line = found
        .next()
        .unwrap_or_else(|| panic!("no line starting {prefix:?}:\n{out}"));
    assert!(
        found.next().is_none(),
        "more than one line starting {prefix:?}:\n{out}"
    );
    line
}

/// The age in seconds that `line` reports for a cached reading, from the
/// first `cached=<n>s` on it.
fn cached_age(line: &str) -> i64 {
    let re = regex::Regex::new(r"cached=(\d+)s").unwrap();
    let caps = re
        .captures(line)
        .unwrap_or_else(|| panic!("no cached=<n>s on: {line}"));
    caps[1].parse().unwrap()
}

/// Scenario 1, simple. A cached reading the cycle decides on is labelled
/// with its age, where before #2889 it was shown as if it had just been
/// read.
///
/// The live login's only reading is a passive one taken `AGE` seconds ago.
/// It is younger than one `--interval`, so the cycle does not re-read it
/// (#2211) and decides on it as it stands. The operator must be able to see
/// that: both the `cycle` line and the decision line carry `cached=<n>s`,
/// and the number is at least the planted age.
///
/// The figures themselves (80%, well over `--session-at 70`) are the ones
/// the decision is made on, so the same lines prove the cache was used and
/// not quietly replaced.
#[test]
fn a_cached_live_reading_is_shown_with_its_age() {
    const AGE: i64 = 60;
    let f = Fixture::new();
    f.passive("work", 80, 15, AGE);

    let out = f.run_with(
        &["profile", "rotate", "--dry-run", "--session-at", "70"],
        &[
            ("AMPLIHACK_ROTATE_INTERVAL", "120"),
            ("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "3600"),
        ],
    );

    assert!(
        !out.contains("stale live reading"),
        "a reading younger than the interval must not be re-read:\n{out}"
    );
    let cycle = one_line(&out, "cycle live=work ");
    assert!(
        cycle.contains("work:s=80%,w=15%,cached="),
        "the cycle line must show the cached figures and say they are cached:\n{out}"
    );
    let decision = one_line(&out, "would swap work->spare ");
    assert!(
        decision.starts_with("would swap work->spare live s=80% w=15% cached=")
            && decision.ends_with("reason=threshold"),
        "the decision line must show the cached figures and say they are cached:\n{out}"
    );
    // Under `--nocapture` the two lines the scenario is about are the
    // evidence an operator would read.
    println!("[#2889 cached] {cycle}\n[#2889 cached] {decision}");
    for line in [cycle, decision] {
        let age = cached_age(line);
        assert!(
            (AGE..AGE + 600).contains(&age),
            "the age must be the reading's own, from {AGE}s: {line}"
        );
    }
    assert!(
        !out.contains("spare:s=45%,w=40%,cached="),
        "no candidate may be labelled cached:\n{out}"
    );
    for line in out.lines() {
        assert_log_grammar(line, false);
    }
    assert!(
        !f.unknown_token.load(Ordering::SeqCst),
        "a usage request used a credential that is not a fixture login's"
    );
}

/// Scenario 2, the incident. With a fresh reading and an older cached one
/// for the live login, the cycle decides on the fresh one and logs it.
///
/// The cached reading (20% of the session, 15% of the week) is older than
/// one `--interval`, so the cycle re-reads the live login directly and gets
/// 90%/40% from the usage endpoint. That is the freshest reading it holds,
/// so every line the operator reads must show it: the `cycle` line, which
/// before #2889 showed the cached figures the cycle had already discarded,
/// and the decision line. Neither may carry `cached=`, because neither
/// shows a cached reading.
///
/// Both windows are checked. The cached week (15%) differs from the fresh
/// week (40%), so a line that mixed a fresh session figure with a cached
/// week would fail here.
///
/// The cached 20% would stay under `--session-at 70`; the fresh 90% swaps.
/// The rotator therefore cannot reach this outcome on the cached reading.
#[test]
fn a_fresh_reading_beats_an_older_cached_reading_on_every_line() {
    let f = Fixture::new();
    f.passive("work", 20, 15, 200);

    let out = f.run_with(
        &["profile", "rotate", "--dry-run", "--session-at", "70"],
        &[
            ("AMPLIHACK_ROTATE_INTERVAL", "120"),
            ("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "3600"),
        ],
    );

    assert_eq!(
        out.lines()
            .filter(|l| *l == "stale live reading; re-read")
            .count(),
        1,
        "exactly one re-read of the stale cached reading:\n{out}"
    );
    assert!(
        !out.contains("reread: work failed"),
        "the live re-read failed, so this run proves nothing:\n{out}"
    );

    let cycle = one_line(&out, "cycle live=work ");
    assert!(
        cycle.contains(&format!(
            "work:s={LIVE_DIRECT_SESSION}%,w={LIVE_DIRECT_WEEK}%"
        )),
        "the cycle line must report the freshest reading:\n{out}"
    );
    assert!(
        !cycle.contains("cached="),
        "a reading read in this cycle is not cached: {cycle}"
    );
    let decision = one_line(&out, "would swap work->spare ");
    assert!(
        decision.starts_with(&format!(
            "would swap work->spare live s={LIVE_DIRECT_SESSION}% w={LIVE_DIRECT_WEEK}% target "
        )) && decision.ends_with("reason=threshold"),
        "the decision must be made on the freshest reading:\n{out}"
    );

    println!("[#2889 freshest] {cycle}\n[#2889 freshest] {decision}");
    for stale in ["s=20%", "w=15%"] {
        assert!(
            !out.contains(stale),
            "the cached figure {stale} outlived the fresh reading:\n{out}"
        );
    }
    assert!(
        !out.contains("would stay"),
        "decided on the cached reading:\n{out}"
    );
    for line in out.lines() {
        assert_log_grammar(line, false);
    }
    assert!(
        !f.unknown_token.load(Ordering::SeqCst),
        "a usage request used a credential that is not a fixture login's"
    );
}
