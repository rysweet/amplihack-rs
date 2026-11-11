//! Issue #2211 one-shot CLI coverage with blocking request handling and
//! explicit server shutdown. Watch histories retain one Ctx in the unit tests.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::{amplihack_bin, assert_log_grammar, ino, private_file};
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    address: SocketAddr,
    high_usage: Arc<AtomicBool>,
    /// A request carried a bearer token that is not a fixture login's live
    /// or spare credential.
    unknown_token: Arc<AtomicBool>,
    /// A request reached the OAuth token endpoint, i.e. something refreshed.
    token_hit: Arc<AtomicBool>,
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
        let high_usage = Arc::new(AtomicBool::new(false));
        let usage = Arc::clone(&high_usage);
        let unknown_token = Arc::new(AtomicBool::new(false));
        let token_hit = Arc::new(AtomicBool::new(false));
        let (unknown, refreshed) = (Arc::clone(&unknown_token), Arc::clone(&token_hit));
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
                if line.contains("/v1/oauth/token") {
                    refreshed.store(true, Ordering::SeqCst);
                }
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
                            "Bearer synthetic-work-2211" => Some("work"),
                            "Bearer synthetic-spare-2211" => Some("spare"),
                            _ => None,
                        };
                    }
                }
                // Never panic here: a panic in this thread would hide the
                // test's own assertion behind a join error. Record and refuse.
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
                let session = match (name, usage.load(Ordering::SeqCst)) {
                    ("work", false) => 50,
                    ("work", true) => 90,
                    (_, false) => 45,
                    (_, true) => 10,
                };
                let body = if profile {
                    json!({"account":{"uuid":format!("uuid-{name}"),
                        "email":format!("{name}@example.com")}, "organization":{"uuid":null}})
                } else {
                    json!({"five_hour":{"utilization":session,"resets_at":"2099-01-01T00:00:00Z"},
                        "seven_day":{"utilization":40,"resets_at":"2099-01-02T00:00:00Z"},
                        "extra_usage":{"is_enabled":false}})
                }
                .to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
                stream.flush().unwrap();
            }
        });
        let f = Self {
            root: home.join("profiles"),
            _tmp: tmp,
            home,
            address,
            high_usage,
            unknown_token,
            token_hit,
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
                "accessToken":format!("synthetic-{name}-2211"),
                "refreshToken":"synthetic-refresh-2211","expiresAt":4_102_444_800_000_i64
            }})
            .to_string(),
        );
    }

    fn run(&self, args: &[&str]) {
        self.run_with(args, &[]);
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
    /// The age only grows while the test runs, so no timing window exists.
    fn passive(&self, name: &str, session: u8, week: u8, age: i64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
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

    fn log(&self) -> String {
        fs::read_to_string(self.root.join(".auto-switch.d/rotate/rotate.log")).unwrap()
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

#[test]
fn one_shot_fresh_live_reading_stays_and_threshold_still_swaps() {
    let f = Fixture::new();
    let live_path = f.home.join(".claude/.credentials.json");
    let live = ino(&live_path).unwrap();
    let spare = ino(&f.root.join("spare/.credentials.json")).unwrap();
    f.run(&["profile", "rotate", "--session-at", "70"]);
    let log = f.log();
    assert!(log.contains(" cycle live=work"), "{log}");
    assert!(!log.contains("stale live reading"), "{log}");
    assert!(!log.contains(" swap "), "{log}");
    assert_eq!(ino(&live_path), Some(live));

    f.high_usage.store(true, Ordering::SeqCst);
    f.run(&["profile", "rotate", "--session-at", "70"]);
    let log = f.log();
    assert!(
        log.lines()
            .any(|l| l.contains(" swap work->spare ") && l.ends_with("reason=threshold")),
        "{log}"
    );
    assert!(!log.contains("stale live reading"), "{log}");
    for line in log.lines() {
        assert_log_grammar(line, true);
    }
    assert_eq!(ino(&live_path), Some(spare));
    assert_eq!(ino(&f.root.join("work/.credentials.json")), Some(live));
    assert!(!f.root.join("spare/.credentials.json").exists());
}

/// Issue #2211 B1/B2 out of process: a passive reading of the live login
/// older than one `--interval` is read again once, directly, from the login
/// in `HOME/.claude`, and the decision uses that figure.
///
/// The stale passive figure (20%) would stay under `--session-at 70`; the
/// server's figure for the home credential (90%) swaps. A `would swap` with
/// `live s=90%` therefore proves the decision used the re-read. The `cycle`
/// line records the figures as measured, before any re-read, so it still
/// shows 20%. `<profile-root>/work` holds no credential while `work` is live
/// (a stored copy would mark it parked), so a read of the profile copy could
/// only fail and could never give 90%.
///
/// The passive reading is 200 s old: above the 120 s interval and below the
/// usage history's 300 s limit, past which a reading is dropped as unknown
/// rather than re-read. The age only grows during the run.
#[test]
fn one_shot_old_passive_live_reading_is_reread_from_home() {
    let f = Fixture::new();
    let live_path = f.home.join(".claude/.credentials.json");
    let live_before = fs::read(&live_path).unwrap();
    let live_mtime = fs::metadata(&live_path).unwrap().modified().unwrap();
    f.passive("work", 20, 40, 200);
    f.high_usage.store(true, Ordering::SeqCst);

    let out = f.run_with(
        &["profile", "rotate", "--dry-run", "--session-at", "70"],
        &[
            ("AMPLIHACK_ROTATE_INTERVAL", "120"),
            ("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "3600"),
        ],
    );

    let stale = out
        .lines()
        .filter(|l| *l == "stale live reading; re-read")
        .count();
    assert_eq!(stale, 1, "exactly one stale line:\n{out}");
    assert!(
        out.lines().any(|l| l.starts_with("cycle live=work ")),
        "no cycle line for work:\n{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with("would swap work->spare live s=90% ")
                && l.ends_with("reason=threshold")),
        "the decision must use the re-read 90%, not the stale 20%:\n{out}"
    );
    assert!(
        !out.lines()
            .any(|l| l == "would stay" || l.contains(" live s=20%")),
        "decided on the stale passive figure:\n{out}"
    );
    assert!(
        !out.contains("reread: work failed"),
        "the live re-read failed:\n{out}"
    );
    for line in out.lines() {
        assert_log_grammar(line, false);
    }
    assert!(
        !f.unknown_token.load(Ordering::SeqCst),
        "a request used a credential that is not a fixture login's"
    );
    assert!(
        !f.token_hit.load(Ordering::SeqCst),
        "the dry run refreshed a token"
    );
    assert!(
        !f.root.join("work/.credentials.json").exists(),
        "--dry-run parked nothing"
    );
    assert_eq!(
        fs::read(&live_path).unwrap(),
        live_before,
        "live bytes changed"
    );
    assert_eq!(
        fs::metadata(&live_path).unwrap().modified().unwrap(),
        live_mtime,
        "live credential rewritten"
    );
    assert!(
        !f.root.join(".auto-switch.d/rotate/rotate.log").exists()
            || !f.log().contains("stale live reading"),
        "--dry-run writes no rotate log"
    );
}
