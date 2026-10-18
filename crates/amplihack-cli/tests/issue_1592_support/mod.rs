//! Shared fixture for the issue #1592 (`profile rotate`) integration tests.
//! Included with `#[path = "issue_1592_support/mod.rs"] mod support;`.
//!
//! Every run uses a temporary `HOME` and profile root, asserted before each
//! command, so nothing touches the real `~/.claude`, `~/.claude-profiles` or
//! a running `claude-rotator`. The fake `claude` never reads a login file:
//! it reads the account e-mail from `$CLAUDE_CONFIG_DIR/.claude.json` (or
//! `$HOME/.claude.json` when the variable is unset) and prints canned
//! `/usage` output for that e-mail. Login fixtures are mode 0000 (0600 under
//! root), so any attempt by amplihack to read one fails.

#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const CREDS: &str = ".credentials.json";
pub const ABORT_ENV: &str = "AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER";
pub const PROFILES: [&str; 3] = ["work", "spare", "extra"];
const BANNER: &str = "You are currently using your subscription to power your Claude Code usage";

/// See `issue_1569_auto_switch.rs`: build the binary once, use the debug one
/// next to this test's `deps` (abort points exist only in debug builds).
pub fn amplihack_bin() -> PathBuf {
    static BIN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(p) = std::env::var_os("AMPLIHACK_TEST_BINARY") {
            return PathBuf::from(p);
        }
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "--quiet", "-p", "amplihack", "--bin", "amplihack"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("failed to run cargo build for the amplihack binary");
        assert!(status.success(), "cargo build -p amplihack failed");
        let bin = std::env::current_exe()
            .unwrap()
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("amplihack");
        assert!(bin.is_file(), "no amplihack binary at {}", bin.display());
        bin
    })
    .clone()
}

const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
if [ -n "${CLAUDE_CONFIG_DIR+x}" ]; then cj="$CLAUDE_CONFIG_DIR/.claude.json"; else cj="$HOME/.claude.json"; fi
email=$(sed -n 's/.*"emailAddress" *: *"\([^"]*\)".*/\1/p' "$cj" 2>/dev/null | head -n 1)
[ -n "$email" ] || email=none
{
  printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
  env | grep '^AMPLIHACK_ROTATE' || true
} > "$RECORD_DIR/env.$email.$$"
[ -f "$CANNED/$email.slow" ] && sleep 30
[ -f "$CANNED/$email.out" ] && cat "$CANNED/$email.out"
exit 0
"#;

pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

pub fn private_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// A login file amplihack cannot read: only its inode identifies it.
pub fn login(path: &Path) -> u64 {
    private_file(path, "SENTINEL-TOKEN");
    if euid() != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    }
    ino(path).unwrap()
}

pub fn ino(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path).ok().map(|m| m.ino())
}

pub fn email(p: &str) -> String {
    format!("{p}@example.com")
}

pub fn account_json(p: &str) -> serde_json::Value {
    serde_json::json!({"accountUuid": format!("uuid-{p}"), "emailAddress": email(p)})
}

pub struct Fixture {
    pub tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub root: PathBuf,
    pub records: PathBuf,
    pub canned: PathBuf,
    pub work_dir: PathBuf,
    pub tools: PathBuf,
    /// Login inodes at setup: `~/.claude` (work's) and each logged-in profile.
    pub live_ino: u64,
    pub spare_ino: u64,
    pub extra_ino: u64,
}

impl Fixture {
    /// `work` is live in `~/.claude` (90/40: over the session threshold);
    /// `spare` 10/20 and `extra` 50/50 are logged in. No rotate ledger.
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let home = tmp.path().join("home");
        let records = tmp.path().join("records");
        let canned = tmp.path().join("canned");
        let work_dir = tmp.path().join("cwd");
        let tools = tmp.path().join("tools");
        for d in [&home, &records, &canned, &work_dir, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let claude = tools.join("claude");
        fs::write(&claude, FAKE_CLAUDE).unwrap();
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
        let mut f = Self {
            root: home.join("profiles"),
            tmp,
            home,
            records,
            canned,
            work_dir,
            tools,
            live_ino: 0,
            spare_ino: 0,
            extra_ino: 0,
        };
        for p in PROFILES {
            f.ok(&["profile", "add", p]);
        }
        f.root = f.root.canonicalize().unwrap();
        fs::set_permissions(&f.root, fs::Permissions::from_mode(0o700)).unwrap();
        for p in PROFILES {
            private_file(
                &f.root.join(p).join(".claude.json"),
                &serde_json::json!({"oauthAccount": account_json(p)}).to_string(),
            );
        }
        fs::create_dir_all(f.home.join(".claude")).unwrap();
        fs::set_permissions(f.home.join(".claude"), fs::Permissions::from_mode(0o700)).unwrap();
        f.live_ino = login(&f.home_login());
        f.spare_ino = login(&f.slot("spare"));
        f.extra_ino = login(&f.slot("extra"));
        private_file(
            &f.home.join(".claude.json"),
            &serde_json::json!({
                "oauthAccount": account_json("work"),
                "primaryApiKey": "SECRET-KEY-HOME",
                "numStartups": 7
            })
            .to_string(),
        );
        f.usage("work", 90, 40);
        f.usage("spare", 10, 20);
        f.usage("extra", 50, 50);
        f
    }

    pub fn home_login(&self) -> PathBuf {
        self.home.join(".claude").join(CREDS)
    }

    pub fn slot(&self, p: &str) -> PathBuf {
        self.root.join(p).join(CREDS)
    }

    pub fn rotate_dir(&self) -> PathBuf {
        self.root.join(".auto-switch.d/rotate")
    }

    pub fn log(&self) -> String {
        fs::read_to_string(self.rotate_dir().join("rotate.log")).unwrap_or_default()
    }

    pub fn ledger(&self) -> Option<serde_json::Value> {
        let raw = fs::read_to_string(self.rotate_dir().join("ledger.json")).ok()?;
        Some(serde_json::from_str(&raw).unwrap())
    }

    pub fn usage(&self, p: &str, session: u32, week: u32) {
        fs::write(
            self.canned.join(format!("{}.out", email(p))),
            format!(
                "{BANNER}\n\nCurrent session: {session}% used\n\
                 Current week (all models): {week}% used\n"
            ),
        )
        .unwrap();
    }

    pub fn home_account(&self) -> String {
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(self.home.join(".claude.json")).unwrap())
                .unwrap();
        v["oauthAccount"]["accountUuid"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Where each setup login is now: `(path, inode)` for every login file.
    pub fn logins(&self) -> Vec<(String, u64)> {
        let mut v = Vec::new();
        if let Some(i) = ino(&self.home_login()) {
            v.push(("~/.claude".to_string(), i));
        }
        for e in fs::read_dir(&self.root).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(i) = ino(&e.path().join(CREDS)) {
                v.push((name, i));
            }
        }
        v.sort();
        v
    }

    /// Each setup login is in exactly one place and nothing else exists.
    pub fn assert_each_login_once(&self) {
        let mut inos: Vec<u64> = self.logins().iter().map(|(_, i)| *i).collect();
        inos.sort();
        let mut want = vec![self.live_ino, self.spare_ino, self.extra_ino];
        want.sort();
        assert_eq!(inos, want, "logins: {:?}", self.logins());
    }

    pub fn where_is(&self, inode: u64) -> Option<String> {
        self.logins()
            .into_iter()
            .find(|(_, i)| *i == inode)
            .map(|(p, _)| p)
    }

    fn assert_isolated(&self) {
        let real = std::env::var_os("HOME").map(PathBuf::from);
        assert!(self.home.starts_with(self.tmp.path()));
        assert!(self.root.starts_with(self.tmp.path()));
        if let Some(real) = real {
            assert_ne!(self.home, real, "never the real HOME");
        }
    }

    pub fn cmd(&self, args: &[&str]) -> Command {
        self.assert_isolated();
        let mut c = Command::new(amplihack_bin());
        c.args(args)
            .current_dir(&self.work_dir)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("RECORD_DIR", &self.records)
            .env("CANNED", &self.canned)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root);
        c
    }

    pub fn run(&self, args: &[&str]) -> Output {
        let out = self.cmd(args).output().unwrap();
        assert_no_secrets(&out);
        out
    }

    pub fn ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "`amplihack {}` failed: {}\nstdout: {}\nstderr: {}",
            args.join(" "),
            out.status,
            text(&out.stdout),
            text(&out.stderr),
        );
        out
    }

    /// Recorded environments of every `claude` probe so far.
    pub fn probe_envs(&self) -> Vec<String> {
        fs::read_dir(&self.records)
            .unwrap()
            .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
            .collect()
    }
}

/// Neither a token, a secret key, an account uuid nor an e-mail is printed.
pub fn assert_no_secrets(out: &Output) {
    for s in [text(&out.stdout), text(&out.stderr)] {
        assert_text_has_no_secrets(&s);
    }
}

pub fn assert_text_has_no_secrets(s: &str) {
    for secret in ["SENTINEL", "SECRET-KEY", "uuid-", "@example.com"] {
        assert!(!s.contains(secret), "secret {secret:?} printed: {s}");
    }
}

/// The documented log grammar: one-shot and `watch` lines start with a UTC
/// time; `--dry-run` lines do not.
pub fn assert_log_grammar(line: &str, timed: bool) {
    let msg = if timed {
        let re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z (.+)$").unwrap();
        let caps = re
            .captures(line)
            .unwrap_or_else(|| panic!("no timestamp: {line:?}"));
        caps.get(1).unwrap().as_str().to_string()
    } else {
        line.to_string()
    };
    let kinds = regex::Regex::new(
        r"^(cycle live=\S+( \S+:(s=\d+%,w=\d+%|failed))*|swap \S+->\S+ live s=\d+% w=\d+% target s=\d+% w=\d+% reason=(threshold|upgrade)|ALERT: .+|recovery: (rolled back|finished) an interrupted swap \S+->\S+|skip: .+|stop: signal received; cycle abandoned|idle: auto-switch is off|would .+)$",
    )
    .unwrap();
    assert!(kinds.is_match(&msg), "line outside the grammar: {line:?}");
    assert_text_has_no_secrets(line);
}
