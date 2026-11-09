//! Issue #1886 end to end, through one `amplihack profile rotate` cycle.
//!
//! Every run starts the binary through `isolated(..)` with a temporary HOME
//! (#1699). The profile root is the saved root in
//! `<HOME>/.amplihack/profile-root`; `AMPLIHACK_CLAUDE_PROFILE_ROOT` is never
//! set. The usage endpoint, the OAuth profile endpoint and the token URL are
//! a loopback fake in this process that answers by bearer token; every login
//! holds a fake `fake-at-1886-<uuid>` token. Every account reports the same
//! usage reset times (the 3 November collision); only the profile endpoint
//! tells them apart. `claude` is a recording stub: it writes its
//! argv and environment to the records directory before anything else.
//!
//! Checked:
//! - #1886: after a swap `lima -> hotel`, `~/.claude` holds `hotel` while
//!   `~/.claude.json` still names `lima`. The cycle writes no `recovered:`
//!   line, puts nothing in `.rejected-logins`, keeps the ledger's `live` and
//!   `account`, and moves no login.
//! - (a): `~/.claude` holds `mike`, which has its own login, and `lima`
//!   keeps a login of its own. The first run only notes the suspicion; a run
//!   at least one interval later recovers with the `recovered:` line.
//! - (e): no fake token appears in stdout, stderr, any file other than a
//!   login file, or the argv and environment of any `claude` started.
//! - `profile verify` prints each folder's server account against its
//!   record and exits non-zero on a mismatch; `--fix` moves the crossed
//!   login into its empty recorded folder.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

const CREDS: &str = ".credentials.json";
const L: &str = "lima";
const H: &str = "hotel";
const M: &str = "mike";

/// Records argv and environment, answers `--version`, fails anything else.
const RECORDING_CLAUDE: &str = r#"#!/bin/sh
n="$RECORD_DIR/claude-run.$$"
{ printf 'ARGV:'; for a in "$@"; do printf ' %s' "$a"; done; printf '\n'; env; } > "$n"
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
exit 1
"#;

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn private_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn dir700(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

fn ino(p: &Path) -> Option<u64> {
    fs::symlink_metadata(p).ok().map(|m| m.ino())
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn iso(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn fake_token() -> String {
    format!("fake-at-1886-{}", uuid::Uuid::new_v4())
}

fn creds(token: &str) -> String {
    serde_json::json!({"claudeAiOauth": {
        "accessToken": token,
        "refreshToken": format!("{token}-r"),
        "expiresAt": chrono::Utc::now().timestamp_millis() + 3_600_000,
        "subscriptionType": "max"
    }})
    .to_string()
}

fn account(p: &str) -> String {
    serde_json::json!({"oauthAccount": {
        "accountUuid": format!("uuid-{p}"),
        "emailAddress": format!("{p}@example.com")
    }})
    .to_string()
}

/// A loopback usage and profile endpoint. Each token answers the profile
/// endpoint with its account (`uuid-<name>`) and the usage endpoint with the
/// reset times given (session at 10%, week at 20%); an unknown token gets
/// 401.
struct Endpoint {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Endpoint {
    fn start(accounts: HashMap<String, (&'static str, (i64, i64))>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = Arc::clone(&seen);
        let accounts = Arc::new(accounts);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (log, accounts) = (Arc::clone(&log), Arc::clone(&accounts));
                std::thread::spawn(move || {
                    let mut r = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    r.read_line(&mut first).unwrap_or(0);
                    let (mut auth, mut len) = (String::new(), 0usize);
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
                    log.lock().unwrap().push(first.trim().to_string());
                    let token = auth.strip_prefix("Bearer ").unwrap_or_default();
                    let (code, reply) = match accounts.get(token) {
                        Some((name, _)) if first.starts_with("GET /api/oauth/profile ") => (
                            200,
                            serde_json::json!({
                                "account": {"uuid": format!("uuid-{name}"), "email": format!("{name}@example.com")},
                                "organization": {"uuid": "org-1886"}
                            })
                            .to_string(),
                        ),
                        Some((_, (s, w))) if first.starts_with("GET ") => (
                            200,
                            serde_json::json!({
                                "five_hour": {"utilization": 10.0, "resets_at": iso(*s)},
                                "seven_day": {"utilization": 20.0, "resets_at": iso(*w)},
                                "extra_usage": {"is_enabled": false}
                            })
                            .to_string(),
                        ),
                        _ => (401, "{}".to_string()),
                    };
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

    fn token_posts(&self) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("POST "))
            .count()
    }
}

struct Fx {
    tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    tools: PathBuf,
    records: PathBuf,
    work: PathBuf,
    tokens: Vec<String>,
    endpoint: Option<Endpoint>,
}

impl Fx {
    /// Profiles `lima`, `hotel` and `mike` under a saved root in a temporary
    /// HOME, with no logins yet.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (home, tools, records, work) = (
            base.join("home"),
            base.join("tools"),
            base.join("records"),
            base.join("cwd"),
        );
        for d in [&home, &tools, &records, &work] {
            dir700(d);
        }
        let root = home.join("profiles");
        dir700(&root);
        dir700(&home.join(".amplihack"));
        private_file(
            &home.join(".amplihack").join("profile-root"),
            &format!("{}\n", root.display()),
        );
        write_executable(&tools.join("claude"), RECORDING_CLAUDE, 0o755);
        let f = Self {
            tmp,
            home,
            root,
            tools,
            records,
            work,
            tokens: Vec::new(),
            endpoint: None,
        };
        for p in [L, H, M] {
            let out = f.run(&["profile", "add", p], &[]);
            assert!(out.status.success(), "add {p}: {}", text(&out.stderr));
            private_file(&f.root.join(p).join(".claude.json"), &account(p));
        }
        dir700(&f.home.join(".claude"));
        f
    }

    fn slot(&self, p: &str) -> PathBuf {
        self.root.join(p).join(CREDS)
    }

    fn home_login(&self) -> PathBuf {
        self.home.join(".claude").join(CREDS)
    }

    fn rotate_dir(&self) -> PathBuf {
        self.root.join(".auto-switch.d").join("rotate")
    }

    /// Writes a login holding a new fake token at `path`; returns the token.
    fn login(&mut self, path: &Path) -> String {
        let t = fake_token();
        private_file(path, &creds(&t));
        self.tokens.push(t.clone());
        t
    }

    fn ledger(&self) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(self.rotate_dir().join("ledger.json")).unwrap())
            .unwrap()
    }

    fn write_ledger(&self, v: serde_json::Value) {
        dir700(&self.root.join(".auto-switch.d"));
        dir700(&self.rotate_dir());
        private_file(&self.rotate_dir().join("ledger.json"), &v.to_string());
    }

    fn log(&self) -> String {
        fs::read_to_string(self.rotate_dir().join("rotate.log")).unwrap_or_default()
    }

    fn rejected_logins(&self) -> Vec<PathBuf> {
        let Ok(rd) = fs::read_dir(self.root.join(".rejected-logins")) else {
            return Vec::new();
        };
        rd.map(|e| e.unwrap().path().join(CREDS))
            .filter(|p| p.exists())
            .collect()
    }

    fn inodes(&self) -> Vec<Option<u64>> {
        let mut v = vec![ino(&self.home_login())];
        v.extend([L, H, M].iter().map(|p| ino(&self.slot(p))));
        v
    }

    fn run(&self, args: &[&str], extra: &[(&str, String)]) -> Output {
        assert!(self.home.starts_with(self.tmp.path()));
        if let Some(real) = std::env::var_os("HOME") {
            assert_ne!(self.home, PathBuf::from(real), "never the real HOME");
        }
        let mut c: Command = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("HOME", &self.home)
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "0")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT");
        if let Some(ep) = &self.endpoint {
            c.env("AMPLIHACK_PROFILE_USAGE_API_BASE", ep.base()).env(
                "AMPLIHACK_PROFILE_USAGE_TOKEN_URL",
                format!("{}/v1/oauth/token", ep.base()),
            );
        }
        for (k, v) in extra {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn rotate_cycle(&self, extra: &[(&str, String)]) -> Output {
        let out = self.run(&["profile", "rotate"], extra);
        assert!(
            !text(&out.stderr).contains("panicked"),
            "{}",
            text(&out.stderr)
        );
        self.assert_no_token_leaks(&out);
        out
    }

    fn assert_no_token_leaks(&self, out: &Output) {
        let mut places = vec![
            ("stdout".to_string(), text(&out.stdout)),
            ("stderr".to_string(), text(&out.stderr)),
        ];
        let mut files = Vec::new();
        files_under(self.tmp.path(), &mut files);
        for p in files {
            if p.file_name().is_some_and(|n| n == CREDS) {
                continue;
            }
            places.push((
                p.display().to_string(),
                text(&fs::read(&p).unwrap_or_default()),
            ));
        }
        for (what, body) in places {
            assert!(!body.contains("fake-at-1886"), "a token in {what}");
            for t in &self.tokens {
                assert!(!body.contains(t.as_str()), "a token in {what}");
            }
        }
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

/// Each account and its reset times, whole seconds in the future: the same
/// for all three, as for accounts created together.
fn resets() -> HashMap<&'static str, (&'static str, (i64, i64))> {
    let now = unix_now();
    let same = (now + 3_600, now + 3 * 86_400);
    HashMap::from([(L, (L, same)), (H, (H, same)), (M, (M, same))])
}

#[test]
fn issue_1886_a_swap_with_a_stale_claude_json_is_left_alone() {
    let mut f = Fx::new();
    let r = resets();
    // After the swap lima -> hotel: lima's login was saved under lima, and
    // hotel's is in ~/.claude. A running session wrote lima's account back
    // into ~/.claude.json.
    let t_home = f.login(&f.home_login());
    let t_l = f.login(&f.slot(L));
    private_file(&f.home.join(".claude.json"), &account(L));
    f.endpoint = Some(Endpoint::start(HashMap::from([
        (t_home, r[H]),
        (t_l, r[L]),
    ])));
    let swapped = unix_now() - 3600;
    f.write_ledger(serde_json::json!({
        "version": 1,
        "live": H,
        "account": "uuid-hotel",
        "since": swapped,
        "swapped_at": swapped,
        "previous": L
    }));
    let inodes = f.inodes();
    f.rotate_cycle(&[]);
    let log = f.log();
    assert!(!log.contains("recovered:"), "{log}");
    assert!(f.rejected_logins().is_empty(), "{:?}", f.rejected_logins());
    assert_eq!(f.inodes(), inodes, "a login moved:\n{log}");
    let l = f.ledger();
    assert_eq!(l["live"], H, "{l}");
    assert_eq!(l["account"], "uuid-hotel", "{l}");
    assert!(l.get("hand_suspect").is_none(), "{l}");
    assert_eq!(f.endpoint.as_ref().unwrap().token_posts(), 0, "no refresh");
}

#[test]
fn a_hand_login_matching_another_profile_recovers_on_the_second_reading() {
    let mut f = Fx::new();
    let r = resets();
    let t_home = f.login(&f.home_login());
    let t_l = f.login(&f.slot(L));
    let t_m = f.login(&f.slot(M));
    private_file(&f.home.join(".claude.json"), &account(M));
    f.endpoint = Some(Endpoint::start(HashMap::from([
        (t_home, r[M]),
        (t_l, r[L]),
        (t_m, r[M]),
    ])));
    let swapped = unix_now() - 3600;
    f.write_ledger(serde_json::json!({
        "version": 1,
        "live": L,
        "since": swapped,
        "swapped_at": swapped
    }));
    let (home_ino, m_ino, l_ino) = (ino(&f.home_login()), ino(&f.slot(M)), ino(&f.slot(L)));
    let interval = [("AMPLIHACK_ROTATE_INTERVAL", "1".to_string())];

    f.rotate_cycle(&interval);
    let log = f.log();
    assert!(
        log.contains("note: ~/.claude holds the same account as profile mike (ledger names lima)"),
        "{log}"
    );
    assert!(!log.contains("recovered:"), "{log}");
    assert!(f.rejected_logins().is_empty());
    assert_eq!(f.ledger()["live"], L);
    assert_eq!(f.ledger()["hand_suspect"]["profile"], M);

    std::thread::sleep(std::time::Duration::from_millis(1_200));
    f.rotate_cycle(&interval);
    let log = f.log();
    assert!(
        log.contains("recovered: ~/.claude and mike held the same account; kept ~/.claude, moved mike's copy to "),
        "{log}"
    );
    let moved = f.rejected_logins();
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(ino(&moved[0]), m_ino);
    assert_eq!(ino(&f.home_login()), home_ino);
    assert_eq!(ino(&f.slot(L)), l_ino, "lima keeps its own login");
    let l = f.ledger();
    assert_eq!(l["live"], M, "{l}");
    assert!(l.get("hand_suspect").is_none(), "{l}");
    assert_eq!(f.endpoint.as_ref().unwrap().token_posts(), 0, "no refresh");
}

impl Fx {
    /// Records `uuid-<account>` (organization `org-1886`) for profile `p`,
    /// as an accepted `profile login` would.
    fn record(&self, p: &str, account: &str) {
        let dir = self.root.join(".auto-switch.d").join("expected");
        dir700(&self.root.join(".auto-switch.d"));
        dir700(&dir);
        private_file(
            &dir.join(format!("{p}.json")),
            &serde_json::json!({"account": format!("uuid-{account}"), "org": "org-1886"})
                .to_string(),
        );
    }
}

/// The 3 November aftermath: `lima`'s folder holds `mike`'s login, `mike`'s
/// folder is empty, and `hotel` is live. All three reset at the same times.
#[test]
fn profile_verify_reports_and_fixes_a_crossed_folder() {
    let mut f = Fx::new();
    let r = resets();
    let t_home = f.login(&f.home_login());
    let t_l = f.login(&f.slot(L));
    f.endpoint = Some(Endpoint::start(HashMap::from([
        (t_home, r[H]),
        (t_l, r[M]),
    ])));
    for p in [L, H, M] {
        f.record(p, p);
    }
    let swapped = unix_now() - 3600;
    f.write_ledger(serde_json::json!({
        "version": 1, "live": H, "since": swapped, "swapped_at": swapped
    }));
    let crossed = ino(&f.slot(L));

    let out = f.run(&["profile", "verify"], &[]);
    f.assert_no_token_leaks(&out);
    let stdout = text(&out.stdout);
    assert!(
        !out.status.success(),
        "a mismatch exits non-zero:\n{stdout}"
    );
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![
            "~/.claude (live hotel) -> hotel@example.com: ok",
            "hotel -> no login",
            "lima -> mike@example.com: MISMATCH, recorded for mike",
            "mike -> no login",
        ]
    );
    assert_eq!(ino(&f.slot(L)), crossed, "verify alone moves nothing");

    let out = f.run(&["profile", "verify", "--fix"], &[]);
    f.assert_no_token_leaks(&out);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", text(&out.stderr));
    assert!(stdout.contains("moved lima's login to mike"), "{stdout}");
    assert_eq!(ino(&f.slot(M)), crossed, "moved, not copied");
    assert_eq!(ino(&f.slot(L)), None);

    let out = f.run(&["profile", "verify"], &[]);
    assert!(out.status.success(), "{}", text(&out.stdout));
    assert!(text(&out.stdout).contains("mike -> mike@example.com: ok"));
    let seen = f.endpoint.as_ref().unwrap().seen.lock().unwrap().clone();
    assert!(
        seen.iter().all(|l| !l.contains("/api/oauth/usage")),
        "{seen:?}"
    );
    assert_eq!(f.endpoint.as_ref().unwrap().token_posts(), 0, "no refresh");
}
