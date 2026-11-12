//! Isolated, event-driven fixtures for #2242. No shell probes or timed waits.
#![cfg(unix)]
#![allow(dead_code)]

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub const CREDS: &str = ".credentials.json";

pub fn private(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

pub fn inode(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().ino()
}

pub fn value(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[derive(Clone, Copy)]
pub enum Reply {
    Healthy,
    Rejected,
    Unavailable,
}

#[derive(Default)]
struct Responses {
    requests: Vec<(String, String)>,
    replies: BTreeMap<String, Reply>,
}

pub struct Fixture {
    pub tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub root: PathBuf,
    address: SocketAddr,
    responses: Arc<Mutex<Responses>>,
    worker: Option<JoinHandle<()>>,
}

impl Fixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let responses = Arc::new(Mutex::new(Responses::default()));
        let state = Arc::clone(&responses);
        let worker = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut input = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                input.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                if path == "/__shutdown" {
                    break;
                }
                let mut name = String::new();
                let mut length = 0;
                loop {
                    line.clear();
                    if input.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((key, val)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("authorization") {
                            name = val
                                .trim()
                                .strip_prefix("Bearer synthetic-2242-")
                                .unwrap_or("unknown")
                                .to_owned();
                        } else if key.eq_ignore_ascii_case("content-length") {
                            length = val.trim().parse::<usize>().unwrap();
                        }
                    }
                }
                let mut body = vec![0; length];
                input.read_exact(&mut body).unwrap();
                if path == "/v1/oauth/token" {
                    let body: Value = serde_json::from_slice(&body).unwrap_or_default();
                    name = body["refresh_token"]
                        .as_str()
                        .unwrap_or("unknown")
                        .strip_prefix("synthetic-2242-refresh-")
                        .unwrap_or("unknown")
                        .to_owned();
                }
                let reply = {
                    let mut state = state.lock().unwrap();
                    state.requests.push((path.clone(), name.clone()));
                    state.replies.get(&name).copied().unwrap_or(Reply::Healthy)
                };
                let (status, body) = match reply {
                    Reply::Rejected => ("400 Bad Request", json!({"error":"invalid_grant"})),
                    Reply::Unavailable => (
                        "503 Service Unavailable",
                        json!({"error":"temporarily_unavailable"}),
                    ),
                    Reply::Healthy if path == "/api/oauth/profile" => (
                        "200 OK",
                        json!({
                            "account":{"uuid":format!("uuid-{name}"),"email":format!("{name}@example.com")},
                            "organization":{"uuid":null}
                        }),
                    ),
                    Reply::Healthy if path == "/v1/oauth/token" => (
                        "200 OK",
                        json!({
                            "access_token":format!("synthetic-2242-{name}"),
                            "refresh_token":format!("synthetic-2242-refresh-{name}"),"expires_in":3600
                        }),
                    ),
                    Reply::Healthy => (
                        "200 OK",
                        json!({
                            "five_hour":{"utilization":if name == "work" {90} else if name == "spare" {5} else {25},
                                "resets_at":"2099-01-01T00:00:00Z"},
                            "seven_day":{"utilization":20,"resets_at":"2099-01-02T00:00:00Z"},
                            "extra_usage":{"is_enabled":false}
                        }),
                    ),
                };
                let body = body.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let f = Self {
            root: home.join("profiles"),
            tmp,
            home,
            address,
            responses,
            worker: Some(worker),
        };
        for name in ["work", "spare", "extra"] {
            f.ok(&["profile", "add", name]);
            private(
                &f.root.join(name).join(".claude.json"),
                json!({"oauthAccount":{"accountUuid":format!("uuid-{name}"),
                    "emailAddress":format!("{name}@example.com")}})
                .to_string()
                .as_bytes(),
            );
            f.credentials(&f.slot(name), name, false, true);
        }
        private(
            &f.home.join(".claude.json"),
            &fs::read(f.root.join("work/.claude.json")).unwrap(),
        );
        fs::create_dir_all(f.home.join(".claude")).unwrap();
        fs::set_permissions(f.home.join(".claude"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(f.slot("work"), f.live()).unwrap();
        for dir in [
            f.root.join(".auto-switch.d"),
            f.root.join(".auto-switch.d/expected"),
        ] {
            fs::create_dir_all(&dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        for name in ["work", "spare", "extra"] {
            private(
                &f.root
                    .join(".auto-switch.d/expected")
                    .join(format!("{name}.json")),
                json!({"account":format!("uuid-{name}"),"org":null})
                    .to_string()
                    .as_bytes(),
            );
        }
        f
    }

    pub fn slot(&self, name: &str) -> PathBuf {
        self.root.join(name).join(CREDS)
    }
    pub fn live(&self) -> PathBuf {
        self.home.join(".claude").join(CREDS)
    }
    pub fn state(&self) -> PathBuf {
        self.root.join(".auto-switch.d/rotate")
    }
    pub fn log(&self) -> String {
        fs::read_to_string(self.state().join("rotate.log")).unwrap_or_default()
    }
    pub fn credentials(&self, path: &Path, name: &str, expired: bool, refresh: bool) {
        let mut oauth = json!({"accessToken":format!("synthetic-2242-{name}"),
            "expiresAt":if expired {1_i64} else {4_102_444_800_000_i64}});
        if refresh {
            oauth["refreshToken"] = format!("synthetic-2242-refresh-{name}").into();
        }
        private(path, json!({"claudeAiOauth":oauth}).to_string().as_bytes());
    }
    pub fn reply(&self, name: &str, reply: Reply) {
        self.responses
            .lock()
            .unwrap()
            .replies
            .insert(name.into(), reply);
    }
    pub fn requests(&self, name: &str) -> Vec<String> {
        self.responses
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|(_, n)| n == name)
            .map(|(p, _)| p.clone())
            .collect()
    }
    pub fn command(&self, args: &[&str]) -> Command {
        assert!(self.home.starts_with(self.tmp.path()) && self.root.starts_with(self.tmp.path()));
        let mut c = crate::isolated_amplihack::isolated(super::amplihack_bin::amplihack_bin());
        c.args(args)
            .current_dir(self.tmp.path())
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("CODEX_PROFILES_ROOT", self.home.join("codex-profiles"))
            .env("CODEX_BIN", self.home.join("no-codex"))
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root)
            .env(
                "AMPLIHACK_PROFILE_USAGE_API_BASE",
                format!("http://{}", self.address),
            )
            .env(
                "AMPLIHACK_PROFILE_USAGE_TOKEN_URL",
                format!("http://{}/v1/oauth/token", self.address),
            )
            .env("AMPLIHACK_PROFILE_USAGE_PROBE", "direct")
            .env("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "0")
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.home.join("no-claude"))
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_ROTATE_HOLD", "0");
        c
    }
    pub fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    pub fn ok(&self, args: &[&str]) -> Output {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        o
    }
    pub fn rotate(&self) -> Output {
        self.run(&["profile", "rotate"])
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream
            .write_all(b"GET /__shutdown HTTP/1.1\r\n\r\n")
            .unwrap();
        self.worker.take().unwrap().join().unwrap();
    }
}
