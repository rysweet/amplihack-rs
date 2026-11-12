//! Disposable launch fixture for #1696. No real homes, endpoints or processes.
#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::profile_default::set_default;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub const INVALIDATIONS: [&str; 9] = [
    "credentials",
    "identity",
    "directory",
    "directory-replacement",
    "root",
    "lease",
    "policy",
    "capacity",
    "refresh-lock",
];

pub const PROFILES: [&str; 3] = ["X", "Y", "Z"];

pub fn private(path: &Path, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[derive(Clone)]
struct IdentityAnswer {
    account: String,
    organisation: String,
}

/// Blocking acceptance; an explicit HTTP shutdown wakes and joins the worker.
/// Neither socket timeouts nor interval probes are needed. Tokens are fake.
struct IdentityServer {
    addr: SocketAddr,
    worker: Option<JoinHandle<()>>,
    identities: Arc<Mutex<BTreeMap<String, Option<IdentityAnswer>>>>,
    requests: Arc<Mutex<Vec<String>>>,
    protected_requests: Arc<Mutex<Vec<String>>>,
}

impl IdentityServer {
    fn new(root: &Path) -> Self {
        use fs4::fs_std::FileExt;
        let root = root.to_path_buf();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let identities = Arc::new(Mutex::new(BTreeMap::<String, Option<IdentityAnswer>>::new()));
        let answers = Arc::clone(&identities);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = Arc::clone(&requests);
        let protected_requests = Arc::new(Mutex::new(Vec::new()));
        let protected = Arc::clone(&protected_requests);
        let worker = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap() == 0 {
                    continue;
                }
                let mut token = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("authorization")
                    {
                        token = value.trim().to_string();
                    }
                }
                if first.starts_with("GET /shutdown ") {
                    break;
                }
                let name = ["X", "Y", "Z", "W"]
                    .into_iter()
                    .find(|name| token == format!("Bearer SENTINEL-1696-{name}"));
                let identity = name.and_then(|name| {
                    if first.starts_with("GET /api/oauth/profile ") {
                        received.lock().unwrap().push(name.to_string());
                        let state_held = fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .open(root.join(".auto-switch.d/lock"))
                            .ok()
                            .is_some_and(|file| !file.try_lock_exclusive().unwrap());
                        if state_held || root.join(name).join(".oauth_refresh.lock").exists() {
                            protected.lock().unwrap().push(name.to_string());
                        }
                    }
                    answers
                        .lock()
                        .unwrap()
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| {
                            Some(IdentityAnswer {
                                account: format!("uuid-{name}"),
                                organisation: "org-team".into(),
                            })
                        })
                });
                let (status, body) = match (first.split_whitespace().nth(1), identity) {
                    (Some("/api/oauth/profile"), Some(identity)) => (
                        "200 OK",
                        json!({
                            "account": {"uuid": identity.account,
                                "email": format!("{}@example.invalid", name.unwrap())},
                            "organization": {"uuid": identity.organisation}
                        }),
                    ),
                    // Unknown and usage requests fail locally, never externally.
                    _ => ("401 Unauthorized", json!({"error":"fixture-only"})),
                };
                let body = body.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            addr,
            worker: Some(worker),
            identities,
            requests,
            protected_requests,
        }
    }
}

impl Drop for IdentityServer {
    fn drop(&mut self) {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        stream
            .write_all(b"GET /shutdown HTTP/1.1\r\nHost: fixture\r\n\r\n")
            .unwrap();
        self.worker.take().unwrap().join().unwrap();
    }
}

const CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version)
    if [ -n "$MUTATE_AT_DISCOVERY" ] && [ ! -f "$RECORD_DIR/invalidated" ]; then
      /bin/sh "$MUTATE_AT_DISCOVERY" || exit "$?"
      : > "$RECORD_DIR/invalidated"
    fi
    echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
p="${AMPLIHACK_CLAUDE_PROFILE-ambient}"
printf '%s\n' "$p" >> "$RECORD_DIR/attempts"
{
  printf '%s\n' "$CLAUDE_CONFIG_DIR"
  printf '%s\n' "$AMPLIHACK_CLAUDE_PROFILE"
  printf '%s\n' "$AMPLIHACK_CLAUDE_PROFILE_ROOT"
} > "$RECORD_DIR/env.$p"
printf '%s\n' "$AMPLIHACK_CLAUDE_PROFILE_JOIN" > "$RECORD_DIR/join.$p"
printf '%s\000' "$@" > "$RECORD_DIR/args.$p"
if [ -n "$AMPLIHACK_CLAUDE_LAUNCH_CONTEXT" ]; then
  cat "$AMPLIHACK_CLAUDE_LAUNCH_CONTEXT" > "$RECORD_DIR/context.$p"
fi
if [ -f "$CLAUDE_CONFIG_DIR/.credentials.json" ]; then
  /usr/bin/cksum < "$CLAUDE_CONFIG_DIR/.credentials.json" > "$RECORD_DIR/credential.$p"
fi
if [ -n "$RECORD_STDIN" ]; then cat > "$RECORD_DIR/stdin.$p"; fi
if [ -n "$SIGNAL_CONTRACT" ]; then
  trap 'printf "stopped\n" >&9; echo "Claude AI usage limit reached" >&2; exit 37' INT TERM HUP
  printf 'ready\n' >&9
  IFS= read -r release <&9
  exit 91
fi
if [ -n "$DESCEND_ONCE" ]; then
  unset DESCEND_ONCE
  if [ -n "$DESCEND_MUTATION" ]; then /bin/sh "$DESCEND_MUTATION"; fi
  if [ -n "$DESCEND_PRINT" ]; then exec "$AMPLIHACK_UNDER_TEST" claude --subprocess-safe -- -p 'descendant step'; fi
  exec "$AMPLIHACK_UNDER_TEST" claude --subprocess-safe -- --resume 'descendant session'
fi
if [ "$p" = "$LIMIT_PROFILE" ]; then
  echo 'Claude AI usage limit reached' >&2
  exit 1
fi
exit "${CLAUDE_EXIT:-0}"
"#;

pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub home: PathBuf,
    pub root: PathBuf,
    pub work: PathBuf,
    pub records: PathBuf,
    tools: PathBuf,
    identity: IdentityServer,
    pub proc: crate::support::fake_proc::FakeProc,
}

impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let home = base.join("home");
        let root = home.join("profiles");
        let work = base.join("work");
        let records = base.join("records");
        let tools = base.join("tools");
        for dir in [&home, &root, &work, &records, &tools, &home.join("runtime")] {
            fs::create_dir_all(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        // The writer is a joined subprocess: this test process never owns
        // an executable write descriptor that sibling forks could inherit.
        crate::support::exec_stub::write_executable(&tools.join("claude"), CLAUDE, 0o700);
        let proc = crate::support::fake_proc::FakeProc::new_in(&base);
        let identity = IdentityServer::new(&root);
        let f = Self {
            temp,
            home,
            root,
            work,
            records,
            tools,
            identity,
            proc,
        };
        for name in PROFILES {
            f.add(name);
        }
        set_default(&f.root, "X");
        private(
            &f.root.join(".auto-switch"),
            "enabled=on\nthreshold=90\nexclude=\noverflow=\n",
        );
        f.seed_usage(&[("X", 100.0, 100.0), ("Y", 20.0, 20.0), ("Z", 10.0, 10.0)]);
        // Contract for a debug-only credential-storage injection. When set,
        // every Keychain lookup must use this store, without system fallback.
        // Directory keys avoid assuming Claude's native Keychain service name.
        private(
            &f.home.join("keychain.json"),
            json!({"version":1,"entries":{}}).to_string(),
        );
        f
    }

    #[cfg(target_os = "macos")]
    pub fn keychain_login(&self, name: &str) -> PathBuf {
        let credentials = self.dir(name).join(".credentials.json");
        let body: serde_json::Value =
            serde_json::from_slice(&fs::read(&credentials).unwrap()).unwrap();
        let path = self.home.join("keychain.json");
        let mut store: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        store["entries"][self.dir(name).to_str().unwrap()] = json!({
            "credentials": body,
            "continuity": "fixture-revision-1"
        });
        private(&path, store.to_string());
        fs::remove_file(credentials).unwrap();
        path
    }

    pub fn add(&self, name: &str) {
        let dir = self.dir(name);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        private(
            &dir.join(".credentials.json"),
            json!({"claudeAiOauth": {
                "accessToken":format!("SENTINEL-1696-{name}"),
                "refreshToken":"fixture-unused-refresh", "expiresAt":4_102_444_800_000_i64
            }})
            .to_string(),
        );
        private(
            &dir.join(".claude.json"),
            json!({"oauthAccount": {
                "accountUuid":format!("uuid-{name}"),
                "emailAddress":format!("{name}@example.invalid"), "organizationUuid":"org-team"
            }})
            .to_string(),
        );
        private(
            &self.state().join(format!("expected/{name}.json")),
            json!({"account":format!("uuid-{name}"),"org":"org-team"}).to_string(),
        );
        for path in [self.state(), self.state().join("expected")] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    pub fn set_organisation(&self, name: &str, organisation: &str) {
        private(
            &self.state().join(format!("expected/{name}.json")),
            json!({"account":format!("uuid-{name}"),"org":organisation}).to_string(),
        );
        private(
            &self.dir(name).join(".claude.json"),
            json!({"oauthAccount":{"accountUuid":format!("uuid-{name}"),
                "organizationUuid":organisation,"emailAddress":format!("{name}@example.invalid")}})
            .to_string(),
        );
        self.set_remote_identity(name, Some((&format!("uuid-{name}"), organisation)));
    }

    /// Change the authenticated answer without changing credential bytes,
    /// local display metadata or expected-account records.
    pub fn set_remote_identity(&self, name: &str, identity: Option<(&str, &str)>) {
        self.identity.identities.lock().unwrap().insert(
            name.to_string(),
            identity.map(|(account, org)| IdentityAnswer {
                account: account.to_string(),
                organisation: org.to_string(),
            }),
        );
    }

    pub fn identity_requests(&self) -> Vec<String> {
        self.identity.requests.lock().unwrap().clone()
    }

    pub fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
    pub fn state(&self) -> PathBuf {
        self.root.join(".auto-switch.d")
    }

    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = crate::isolated_amplihack::isolated(amplihack_bin());
        cmd.env_clear()
            .args(args)
            .current_dir(&self.work)
            .stdin(Stdio::null())
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env(crate::isolated_amplihack::PROC_ROOT_ENV, self.proc.path())
            .env("HOME", &self.home)
            .env("TMPDIR", self.temp.path())
            .env("XDG_RUNTIME_DIR", self.home.join("runtime"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_STATE_HOME", self.home.join(".state"))
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.tools.display()))
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root)
            .env(
                "AMPLIHACK_TEST_KEYCHAIN_STORE",
                self.home.join("keychain.json"),
            )
            .env("AMPLIHACK_UNDER_TEST", amplihack_bin())
            .env(
                "AMPLIHACK_PROFILE_USAGE_API_BASE",
                format!("http://{}", self.identity.addr),
            )
            .env(
                "AMPLIHACK_PROFILE_USAGE_TOKEN_URL",
                format!("http://{}/v1/oauth/token", self.identity.addr),
            )
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_GOVERNOR", "off")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1");
        crate::isolated_amplihack::no_git_prompt(&mut cmd);
        cmd
    }

    pub fn run(&self, cmd: &mut Command) -> Output {
        let out = cmd.output().unwrap();
        assert!(
            self.identity.protected_requests.lock().unwrap().is_empty(),
            "identity requests must finish before state and refresh protection is acquired"
        );
        for bytes in [&out.stdout, &out.stderr] {
            assert!(
                !String::from_utf8_lossy(bytes).contains("SENTINEL-1696-"),
                "credential leaked"
            );
        }
        for entry in fs::read_dir(&self.records).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("context.")
            {
                let bytes = fs::read(path).unwrap();
                let context = String::from_utf8_lossy(&bytes);
                assert!(
                    !context.contains("SENTINEL-1696-")
                        && !context.contains("fixture-unused-refresh"),
                    "credentials stored in supervisor context"
                );
            }
        }
        out
    }

    pub fn launch(&self, entry: &str) -> Command {
        self.cmd(&[
            entry,
            "--subprocess-safe",
            "--",
            "--resume",
            "session with spaces",
        ])
    }

    pub fn seed_usage(&self, readings: &[(&str, f64, f64)]) {
        let at = now();
        let accounts: serde_json::Map<String, serde_json::Value> = readings
            .iter()
            .map(|(name, five, week)| {
                (
                    name.to_string(),
                    json!({
                        "five_hour":{"used_percentage":five,"resets_at":at+3600},
                        "seven_day":{"used_percentage":week,"resets_at":at+86400},
                        "observed_at":at,"source":"statusline"
                    }),
                )
            })
            .collect();
        private(
            &self.state().join("usage.json"),
            json!({"version":1,"accounts":accounts}).to_string(),
        );
    }

    /// Portable conflict fixture. macOS conservatively treats the recorded
    /// host as live; successful liveness-based selection belongs to Linux.
    pub fn seed_hosted_lease(&self) {
        let parked = self.state().join("parked/X");
        fs::create_dir_all(&parked).unwrap();
        for dir in [self.state().join("parked"), parked.clone()] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::rename(
            self.dir("X").join(".credentials.json"),
            parked.join(".credentials.json"),
        )
        .unwrap();
        fs::rename(
            self.dir("Y").join(".credentials.json"),
            self.dir("X").join(".credentials.json"),
        )
        .unwrap();
        private(
            &self.state().join("ledger.json"),
            json!({"version":1,"hosts":{
                "X":{"live":"Y","swapped_at":now(),"pre_swap_fingerprint":null}
            }})
            .to_string(),
        );
        #[cfg(target_os = "linux")]
        self.hold_host();
    }

    #[cfg(target_os = "linux")]
    pub fn hold_host(&self) {
        self.proc
            .process(crate::support::fake_proc::FAKE_PID_BASE)
            .cmdline(&["claude", "--resume"])
            .status('S')
            .environ(&[("CLAUDE_CONFIG_DIR", self.dir("X").to_str().unwrap())]);
    }

    #[cfg(target_os = "linux")]
    pub fn swap(&self) {
        self.hold_host();
        // Real status-line swap: Y wins over Z for the initial lease.
        self.seed_usage(&[("Y", 1.0, 1.0), ("Z", 50.0, 50.0)]);
        let mut cmd = self.cmd(&[
            "profile",
            "statusline",
            "--root",
            self.root.to_str().unwrap(),
            "--owner",
            "X",
        ]);
        cmd.env("CLAUDE_CONFIG_DIR", self.dir("X"))
            .stdin(Stdio::piped());
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                json!({"session_id":"issue-1696", "rate_limits":{
                    "five_hour":{"used_percentage":100.0,"resets_at":now()+3600},
                    "seven_day":{"used_percentage":10.0,"resets_at":now()+86400}
                }})
                .to_string()
                .as_bytes(),
            )
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let ledger: serde_json::Value =
            serde_json::from_slice(&fs::read(self.state().join("ledger.json")).unwrap()).unwrap();
        assert_eq!(ledger["hosts"]["X"]["live"], "Y", "real lease setup failed");
        self.seed_usage(&[("X", 100.0, 100.0), ("Y", 20.0, 20.0), ("Z", 10.0, 10.0)]);
    }

    /// Binary discovery is the readiness event between selection and spawn.
    pub fn assert_invalidation_refused(&self, invalidation: &str) {
        // Binary discovery is synchronous and follows initial resolution.
        // The immutable stub applies this mutation before returning --version.
        let mutation = self.home.join("mutate.sh");
        private(
            &mutation,
            r#"case "$INVALIDATION" in
credentials) printf '{}' > "$SELECTED_DIR/.credentials.json" ;;
identity) printf '{}' > "$SELECTED_DIR/.claude.json" ;;
directory) chmod 0777 "$SELECTED_DIR" ;;
directory-replacement) mv "$SELECTED_DIR" "$SELECTED_DIR.old"; mkdir -m 700 "$SELECTED_DIR"; cp "$SELECTED_DIR.old/.credentials.json" "$SELECTED_DIR/.credentials.json"; cp "$SELECTED_DIR.old/.claude.json" "$SELECTED_DIR/.claude.json" ;;
root) chmod 0777 "$AMPLIHACK_CLAUDE_PROFILE_ROOT" ;;
lease) cp "$LEASE_MUTATION" "$AMPLIHACK_CLAUDE_PROFILE_ROOT/.auto-switch.d/ledger.json" ;;
policy) printf 'enabled=on\nexclude=Z\n' > "$AMPLIHACK_CLAUDE_PROFILE_ROOT/.auto-switch" ;;
capacity) cp "$CAPACITY_MUTATION" "$AMPLIHACK_CLAUDE_PROFILE_ROOT/.auto-switch.d/usage.json" ;;
refresh-lock) mkdir "$SELECTED_DIR/.oauth_refresh.lock" ;;
esac
"#,
        );
        let original_usage = fs::read(self.state().join("usage.json")).unwrap();
        self.seed_usage(&[
            ("X", 100.0, 100.0),
            ("Y", 100.0, 100.0),
            ("Z", 100.0, 100.0),
        ]);
        let exhausted = self.home.join("exhausted.json");
        private(
            &exhausted,
            fs::read(self.state().join("usage.json")).unwrap(),
        );
        private(&self.state().join("usage.json"), original_usage);
        let pointer = fs::read(self.root.join(".default")).unwrap();
        let ledger = fs::read(self.state().join("ledger.json")).unwrap();
        let mut changed_ledger: serde_json::Value = serde_json::from_slice(&ledger).unwrap();
        changed_ledger["hosts"]["X"]["live"] = json!("Z");
        let lease_mutation = self.home.join("changed-lease.json");
        private(&lease_mutation, changed_ledger.to_string());
        let borrowed = fs::read(self.dir("X").join(".credentials.json")).unwrap();
        let out = self.run(
            self.launch("claude")
                .env("MUTATE_AT_DISCOVERY", mutation)
                .env("INVALIDATION", invalidation)
                .env("SELECTED_DIR", self.dir("Z"))
                .env("CAPACITY_MUTATION", exhausted)
                .env("LEASE_MUTATION", &lease_mutation),
        );
        assert!(
            self.records.join("invalidated").exists(),
            "must reach selection/discovery: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !out.status.success(),
            "{invalidation} invalidation must fail before spawn"
        );
        assert!(
            self.attempts().is_empty(),
            "{invalidation} launched stale credentials"
        );
        assert_eq!(fs::read(self.root.join(".default")).unwrap(), pointer);
        let expected_ledger = if invalidation == "lease" {
            fs::read(lease_mutation).unwrap()
        } else {
            ledger
        };
        assert_eq!(
            fs::read(self.state().join("ledger.json")).unwrap(),
            expected_ledger
        );
        assert_eq!(
            fs::read(self.dir("X").join(".credentials.json")).unwrap(),
            borrowed
        );
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("using profile 'Z'"),
            "do not announce invalid selection"
        );
    }

    pub fn attempts(&self) -> Vec<String> {
        fs::read_to_string(self.records.join("attempts"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    pub fn assert_selected(&self, out: &Output, name: &str) {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(self.attempts(), [name]);
        let env = fs::read_to_string(self.records.join(format!("env.{name}"))).unwrap();
        let want = format!(
            "{}\n{name}\n{}\n",
            self.dir(name).display(),
            self.root.display()
        );
        assert_eq!(env, want, "actual child received the wrong destination");
        self.assert_child_login(name, &self.dir(name));
    }

    pub fn assert_child_login(&self, name: &str, directory: &Path) {
        let expected = Command::new("/usr/bin/cksum")
            .stdin(fs::File::open(directory.join(".credentials.json")).unwrap())
            .output()
            .unwrap();
        assert!(expected.status.success());
        assert_eq!(
            fs::read(self.records.join(format!("credential.{name}"))).unwrap(),
            expected.stdout,
            "actual child read different credentials"
        );
    }

    pub fn assert_recovery(&self, out: &Output) {
        assert!(!out.status.success());
        assert!(self.attempts().is_empty(), "Claude started despite refusal");
        let err = String::from_utf8_lossy(&out.stderr);
        for phrase in [
            "X",
            "Y",
            "amplihack profile switch --none",
            "amplihack profile run <other>",
            "amplihack profile auto-switch status",
            "usable",
        ] {
            assert!(err.contains(phrase), "missing {phrase:?}: {err}");
        }
        assert!(
            err.contains("ambient") || err.contains("~/.claude"),
            "{err}"
        );
    }
}

/// Identity, location, inode and bytes must survive every attempted launch.
struct FileSnapshot {
    path: PathBuf,
    contents: Option<(u64, Vec<u8>)>,
}

pub struct Preserved(Vec<FileSnapshot>);

impl Preserved {
    pub fn capture(f: &Fixture) -> Self {
        let mut paths = vec![
            f.root.join(".default"),
            f.root.join(".auto-switch"),
            f.state().join("ledger.json"),
            f.dir("X").join(".credentials.json"),
            f.state().join("parked/X/.credentials.json"),
            f.dir("Z").join(".credentials.json"),
        ];
        for name in ["X", "Y", "Z", "W"] {
            for file in [".credentials.json", ".claude.json"] {
                paths.push(f.dir(name).join(file));
                paths.push(f.state().join("parked").join(name).join(file));
            }
        }
        paths.sort();
        paths.dedup();
        Self(
            paths
                .into_iter()
                .map(|p| {
                    let contents = fs::read(&p)
                        .ok()
                        .map(|bytes| (fs::metadata(&p).unwrap().ino(), bytes));
                    FileSnapshot { path: p, contents }
                })
                .collect(),
        )
    }

    pub fn assert_unchanged(&self) {
        for file in &self.0 {
            let after = fs::read(&file.path)
                .ok()
                .map(|bytes| (fs::metadata(&file.path).unwrap().ino(), bytes));
            assert_eq!(
                after,
                file.contents,
                "changed, moved or replaced {}",
                file.path.display()
            );
        }
    }
}
