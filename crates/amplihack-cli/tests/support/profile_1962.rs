//! Shared fixture for the issue #1962 tests. It builds on the #1592 fixture's
//! isolated home and profile root, and answers on loopback only. Every login is
//! fake. Unlike the #1753 endpoint, each answer can be set per login: a status,
//! a hang or an account. Every request is recorded, so a test can prove the order
//! of a refresh and an identity read, and count the reads.

#![cfg(unix)]
#![allow(dead_code)]

use super::profile_rotate::{Fixture, IDENTITY_READ_TIMEOUT_ENV, private_file, text};
use super::wait::CEILING;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const TOKEN: &str = "fake-task1962-";
pub const REFRESH: &str = "fake-task1962-refresh-";
/// The expired access token `World::expire` writes. The fake never accepts it,
/// so a test that gets past it proves the refresh came first.
const OLD: &str = "-old";
/// The identity read timeout, in seconds, of a command run while a login's
/// answer is [`Answer::Hang`]: the product's 5 s. The #1592 fixture raises the
/// timeout to [`CEILING`] for load tolerance (issue #1973); a hang test needs
/// the read to time out, so it puts the product value back.
const HANG_READ_TIMEOUT_SECS: &str = "5";

/// How the fake answers a login's profile and usage requests.
#[derive(Clone)]
pub enum Answer {
    Account(serde_json::Value),
    Status(u16),
    /// Holds the connection open without an answer until the fake is
    /// dropped (at most [`CEILING`]), so the reader's own timeout always ends
    /// the read first and the cause is `timeout`, never `transport`.
    Hang,
    /// The identity answers with this account, and the usage read with this status.
    UsageStatus(serde_json::Value, u16),
}

/// The server's account for `p`, in its own organization.
pub fn account(p: &str) -> serde_json::Value {
    account_in(
        &format!("uuid-{p}"),
        &format!("{p}@example.com"),
        &format!("org-{p}"),
    )
}

pub fn account_in(uuid: &str, email: &str, org: &str) -> serde_json::Value {
    serde_json::json!({"account": {"uuid": uuid, "email": email},
        "organization": {"uuid": org}})
}

#[derive(Default)]
struct Script {
    answers: HashMap<String, Answer>,
    /// A fake refresh token's owner, mapped to the login it renews to.
    renewals: HashMap<String, String>,
    events: Vec<String>,
}

pub struct Fake {
    base: String,
    stop: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
    script: Arc<Mutex<Script>>,
}

impl Fake {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let script = Arc::new(Mutex::new(Script::default()));
        let (stopping, shared) = (Arc::clone(&stop), Arc::clone(&script));
        let acceptor = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (script, stop) = (Arc::clone(&shared), Arc::clone(&stopping));
                        // Its own thread, so a hung answer blocks no other read.
                        std::thread::spawn(move || serve(stream, &script, &stop));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("fake endpoint: {e}"),
                }
            }
        });
        Self {
            base,
            stop,
            acceptor: Some(acceptor),
            script,
        }
    }

    pub fn answer(&self, who: &str, answer: Answer) {
        self.script
            .lock()
            .unwrap()
            .answers
            .insert(who.into(), answer);
    }

    /// Whether any login's answer is [`Answer::Hang`].
    pub fn hangs(&self) -> bool {
        self.script
            .lock()
            .unwrap()
            .answers
            .values()
            .any(|a| matches!(a, Answer::Hang))
    }

    /// `who`'s fake refresh token now renews to `to`'s access token.
    pub fn renews(&self, who: &str, to: &str) {
        self.script
            .lock()
            .unwrap()
            .renewals
            .insert(who.into(), to.into());
    }

    /// Every request so far: `profile:<who>`, `usage:<who>` or `refresh:<who>`.
    pub fn events(&self) -> Vec<String> {
        self.script.lock().unwrap().events.clone()
    }

    pub fn count(&self, event: &str) -> usize {
        self.events().iter().filter(|e| *e == event).count()
    }

    pub fn refreshes(&self) -> usize {
        self.events()
            .iter()
            .filter(|e| e.starts_with("refresh:"))
            .count()
    }

    fn base(&self) -> &str {
        &self.base
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.acceptor.take().unwrap().join().unwrap();
    }
}

fn serve(mut stream: TcpStream, script: &Mutex<Script>, stop: &AtomicBool) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let (mut who, mut length) = (String::from("unknown"), 0usize);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().unwrap_or(0);
        }
        if key.eq_ignore_ascii_case("authorization")
            && let Some(token) = value.trim().strip_prefix(&format!("Bearer {TOKEN}"))
        {
            who = token.to_string();
        }
    }
    let answer = if first.starts_with("POST /v1/oauth/token ") {
        let mut raw = vec![0; length.min(65536)];
        if reader.read_exact(&mut raw).is_err() {
            return;
        }
        let request: serde_json::Value = serde_json::from_slice(&raw).unwrap_or_default();
        let owner = request["refresh_token"]
            .as_str()
            .and_then(|t| t.strip_prefix(REFRESH))
            .unwrap_or("unknown")
            .to_string();
        let mut s = script.lock().unwrap();
        s.events.push(format!("refresh:{owner}"));
        match s.renewals.get(&owner) {
            Some(to) => Answer::Account(serde_json::json!({
                "access_token": format!("{TOKEN}{to}"),
                "refresh_token": request["refresh_token"], "expires_in": 3600
            })),
            None => Answer::Status(401),
        }
    } else {
        let mut s = script.lock().unwrap();
        let usage = first.starts_with("GET /api/oauth/usage ");
        let kind = if usage { "usage" } else { "profile" };
        s.events.push(format!("{kind}:{who}"));
        match s.answers.get(&who) {
            Some(Answer::Account(_)) if usage => Answer::Account(usage_body(&who)),
            Some(Answer::UsageStatus(_, code)) if usage => Answer::Status(*code),
            Some(Answer::UsageStatus(body, _)) => Answer::Account(body.clone()),
            Some(a) => a.clone(),
            None => Answer::Status(401),
        }
    };
    let (status, body) = match answer {
        Answer::Account(body) => ("200 OK".to_string(), body),
        Answer::Status(code) => (format!("{code} Fake"), serde_json::json!({"error": "fake"})),
        Answer::UsageStatus(..) => unreachable!("resolved above"),
        Answer::Hang => {
            let until = Instant::now() + CEILING;
            while Instant::now() < until && !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(20));
            }
            return;
        }
    };
    let body = body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// The live login `work` reads full, so a cycle wants to swap away from it.
fn usage_body(who: &str) -> serde_json::Value {
    let session = if who == "work" { 90 } else { 10 };
    serde_json::json!({
        "five_hour": {"utilization": session, "resets_at":
            (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339()},
        "seven_day": {"utilization": 20, "resets_at":
            (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339()}
    })
}

/// One file or directory's identity and change time.
pub type Stamp = BTreeMap<PathBuf, (u64, u64, i64, i64)>;

/// `work` is live in `~/.claude`; `spare` and `extra` hold their own logins. All
/// three have a record in their own organization, and the ledger has a full
/// `login_identity` for `work`, last swapped 20 minutes ago.
pub struct World {
    pub f: Fixture,
    pub fake: Fake,
}

impl World {
    pub fn new() -> Self {
        let f = Fixture::new();
        f.assert_isolated();
        let fake = Fake::new();
        let w = Self { f, fake };
        // No home API key: its advisory ALERT names `~/.claude.json`, which a test
        // of the identity causes must be able to rule out.
        private_file(
            &w.f.home.join(".claude.json"),
            &serde_json::json!({"oauthAccount": {"accountUuid": "uuid-work",
                "emailAddress": "work@example.com"}})
            .to_string(),
        );
        for dir in [w.state(), w.f.rotate_dir(), w.state().join("expected")] {
            fs::create_dir_all(&dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        for p in ["work", "spare", "extra"] {
            w.login(p, p);
            w.fake.answer(p, Answer::Account(account(p)));
            w.set_record(p, &format!("uuid-{p}"), Some(&format!("org-{p}")));
        }
        w.set_ledger(&Self::ledger_for(
            "work",
            "uuid-work",
            serde_json::json!({"accountUuid": "uuid-work",
                "emailAddress": "work@example.com", "organizationUuid": "org-work"}),
        ));
        w
    }

    pub fn ledger_for(live: &str, key: &str, identity: serde_json::Value) -> serde_json::Value {
        let at = chrono::Utc::now().timestamp() - 1200;
        serde_json::json!({"version": 1, "live": live, "account": key, "since": at,
            "swapped_at": at, "login_identity": identity})
    }

    pub fn state(&self) -> PathBuf {
        self.f.root.join(".auto-switch.d")
    }

    pub fn ledger_path(&self) -> PathBuf {
        self.f.rotate_dir().join("ledger.json")
    }

    pub fn set_ledger(&self, ledger: &serde_json::Value) {
        private_file(&self.ledger_path(), &ledger.to_string());
    }

    /// A ledger from before login identities were kept.
    pub fn older_ledger(&self, key: &str) {
        let mut ledger = self.f.ledger().unwrap();
        ledger.as_object_mut().unwrap().remove("login_identity");
        ledger["account"] = key.into();
        self.set_ledger(&ledger);
    }

    pub fn damage_ledger(&self) {
        private_file(&self.ledger_path(), "{not json");
    }

    pub fn record_path(&self, p: &str) -> PathBuf {
        self.state().join("expected").join(format!("{p}.json"))
    }

    pub fn set_record(&self, p: &str, key: &str, org: Option<&str>) {
        let body = serde_json::json!({"account": key, "org": org});
        private_file(&self.record_path(p), &body.to_string());
    }

    pub fn no_record(&self, p: &str) {
        fs::remove_file(self.record_path(p)).unwrap();
    }

    pub fn record(&self, p: &str) -> Option<serde_json::Value> {
        let raw = fs::read(self.record_path(p)).ok()?;
        Some(serde_json::from_slice(&raw).unwrap())
    }

    /// The login file of `p`, or of `~/.claude` for `work`.
    pub fn creds(&self, p: &str) -> PathBuf {
        if p == "work" {
            self.f.home_login()
        } else {
            self.f.slot(p)
        }
    }

    /// Makes `p`'s login hold `who`'s unexpired fake token.
    pub fn login(&self, p: &str, who: &str) {
        private_file(
            &self.creds(p),
            &serde_json::json!({"claudeAiOauth": {
                "accessToken": format!("{TOKEN}{who}"),
                "refreshToken": format!("{REFRESH}{who}"),
                "expiresAt": chrono::Utc::now().timestamp_millis() + 3_600_000,
                "subscriptionType": "max"
            }})
            .to_string(),
        );
    }

    /// Makes `p`'s login expired, holding a token the fake never accepts. Its
    /// refresh token renews only after [`Fake::renews`].
    pub fn expire(&self, p: &str) {
        private_file(
            &self.creds(p),
            &serde_json::json!({"claudeAiOauth": {
                "accessToken": format!("{TOKEN}{p}{OLD}"),
                "refreshToken": format!("{REFRESH}{p}"),
                "expiresAt": chrono::Utc::now().timestamp_millis() - 60_000,
                "subscriptionType": "max"
            }})
            .to_string(),
        );
    }

    /// The access token now in `p`'s login file, which is a fake.
    pub fn token(&self, p: &str) -> String {
        let v: serde_json::Value =
            serde_json::from_slice(&fs::read(self.creds(p)).unwrap()).unwrap();
        v["claudeAiOauth"]["accessToken"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// An API-key login: a private JSON object with no OAuth block.
    pub fn api_key_login(&self, p: &str) {
        private_file(&self.creds(p), r#"{"apiKeyHelper": "fake-helper"}"#);
    }

    pub fn chmod(&self, path: &Path, mode: u32) {
        assert!(path.starts_with(self.f.tmp.path()));
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = self.f.cmd(args);
        c.env("AMPLIHACK_PROFILE_USAGE_API_BASE", self.fake.base())
            .env(
                "AMPLIHACK_PROFILE_USAGE_TOKEN_URL",
                format!("{}/v1/oauth/token", self.fake.base()),
            );
        if self.fake.hangs() {
            c.env(IDENTITY_READ_TIMEOUT_ENV, HANG_READ_TIMEOUT_SECS);
        }
        c
    }

    fn checked(&self, mut c: Command) -> Output {
        let out = c.output().unwrap();
        for value in [text(&out.stdout), text(&out.stderr), self.f.log()] {
            assert!(!value.contains(TOKEN), "fake token leaked: {value}");
        }
        out
    }

    /// One real cycle, reading through the direct path only.
    pub fn cycle(&self) -> Output {
        let mut c = self.command(&["profile", "rotate"]);
        c.env("AMPLIHACK_PROFILE_USAGE_PROBE", "direct");
        self.checked(c)
    }

    /// The dry run, in its default probe mode. If it starts `claude`, the fake
    /// `claude` leaves a record behind.
    pub fn dry(&self) -> Output {
        self.checked(self.command(&["profile", "rotate", "--dry-run"]))
    }

    pub fn switch(&self, to: &str) -> Output {
        self.checked(self.command(&["profile", "switch", to]))
    }

    pub fn status(&self) -> Output {
        self.checked(self.command(&["profile", "rotate", "status"]))
    }

    pub fn verify(&self) -> Output {
        self.checked(self.command(&["profile", "verify"]))
    }

    pub fn seed(&self, name: &str, write: bool) -> Output {
        let mut args = vec!["profile", "seed-record", name];
        if write {
            args.push("--write");
        }
        self.checked(self.command(&args))
    }

    /// `(inode, size, mtime, ctime)` of every login file under the home.
    pub fn logins_stamp(&self) -> Stamp {
        let mut s = Stamp::new();
        walk(&self.f.home, &mut |p, m| {
            if !m.is_dir() && p.file_name().is_some_and(|n| n == ".credentials.json") {
                s.insert(p.to_path_buf(), stamp_of(m));
            }
        });
        s
    }

    /// Everything a dry run must leave alone, directories included.
    pub fn tree_stamp(&self) -> Stamp {
        let mut s = Stamp::new();
        let mut note = |p: &Path, m: &fs::Metadata| {
            s.insert(p.to_path_buf(), stamp_of(m));
        };
        walk(&self.f.home.join(".claude"), &mut note);
        // Issue #2040 (D10): the dry run takes the run lock, which may create
        // the rotate directory and `rotate-run.lock` and rewrites its pid.
        // Every other entry in the directory is still stamped.
        let rotate = self.f.rotate_dir();
        let run_lock = rotate.join("rotate-run.lock");
        walk(&rotate, &mut |p, m| {
            if p != rotate && p != run_lock {
                note(p, m);
            }
        });
        walk(&self.state().join("expected"), &mut note);
        for p in ["work", "spare", "extra"] {
            walk(&self.f.root.join(p), &mut note);
        }
        let dot = self.f.home.join(".claude.json");
        note(&dot, &fs::symlink_metadata(&dot).unwrap());
        s
    }
}

fn stamp_of(m: &fs::Metadata) -> (u64, u64, i64, i64) {
    (
        m.ino(),
        m.len(),
        m.mtime_nsec() + m.mtime() * 1_000_000_000,
        m.ctime(),
    )
}

fn walk(path: &Path, visit: &mut dyn FnMut(&Path, &fs::Metadata)) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    visit(path, &meta);
    if meta.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            walk(&entry.unwrap().path(), visit);
        }
    }
}

/// Everything a command printed.
pub fn printed(out: &Output) -> String {
    format!("{}{}", text(&out.stdout), text(&out.stderr))
}

/// Asserts the fixed `seed-record` refusal, exit 1, and no record written.
pub fn assert_refused(w: &World, name: &str, out: &Output, reason: &str) {
    let all = printed(out);
    assert_eq!(out.status.code(), Some(1), "{all}");
    let want = format!("seed-record {name}: refused: {reason}; nothing written");
    assert!(all.contains(&want), "want {want:?}, got: {all}");
    assert_eq!(w.record(name), None, "no record for a refused seed");
}

/// Whether `dir`'s group is the user's private group: their primary group, named
/// like the user, with no listed members.
pub fn group_is_private(dir: &Path) -> bool {
    let gid = std::fs::metadata(dir).unwrap().gid();
    // SAFETY: getgid has no preconditions.
    if gid != unsafe { libc::getgid() } {
        return false;
    }
    let tool = |args: &[&str]| {
        Command::new(args[0])
            .args(&args[1..])
            .output()
            .map_or(String::new(), |o| {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            })
    };
    let user = tool(&["id", "-un"]);
    let entry = tool(&["getent", "group", &gid.to_string()]);
    let fields: Vec<&str> = entry.split(':').collect();
    fields.len() == 4 && fields[0] == user && fields[3].is_empty()
}
