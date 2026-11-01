//! Shared fixture for the issue #1592 (`profile rotate`) integration tests,
//! used as `crate::support::profile_rotate` by the `issue_1592_*` modules.
//!
//! Every run uses a temporary `HOME` and profile root, asserted before each
//! command, so nothing touches the real `~/.claude`, `~/.claude-profiles` or
//! a running `claude-rotator`. The fake `claude` never reads a login file:
//! it reads the account e-mail from `$CLAUDE_CONFIG_DIR/.claude.json` (or
//! `$HOME/.claude.json` when the variable is unset) and prints canned
//! `/usage` output for that e-mail. Login fixtures are private OAuth JSON
//! objects holding `SENTINEL-TOKEN` values, which must never be printed.
//!
//! Issue #1698: the usage request (`--input-format` in argv) is answered only
//! when `<email>.json` exists; without it the fake exits 1 at once, before
//! recording anything, so every probe falls back to `/usage`. With
//! `$CANNED/marks`, each `/usage` probe appends `S <email>` and `E <email>`
//! to `$RECORD_DIR/marks` around a 0.5 s sleep.

#![cfg(unix)]
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub use super::amplihack_bin::amplihack_bin;
use super::exec_stub::write_executable;

pub const CREDS: &str = ".credentials.json";
pub const ABORT_ENV: &str = "AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER";
pub const PROFILES: [&str; 3] = ["work", "spare", "extra"];
const BANNER: &str = "You are currently using your subscription to power your Claude Code usage";

const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
if [ -n "${CLAUDE_CONFIG_DIR+x}" ]; then cj="$CLAUDE_CONFIG_DIR/.claude.json"; else cj="$HOME/.claude.json"; fi
email=$(sed -n 's/.*"emailAddress" *: *"\([^"]*\)".*/\1/p' "$cj" 2>/dev/null | head -n 1)
[ -n "$email" ] || email=none
case " $* " in
  *" --input-format "*)
    [ -f "$CANNED/$email.json" ] || exit 1
    cat > "$RECORD_DIR/stdin.$email.$$"
    cat "$CANNED/$email.json"
    exit 0 ;;
esac
{
  printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
  env | grep '^AMPLIHACK_ROTATE' || true
} > "$RECORD_DIR/env.$email.$$"
if [ -f "$CANNED/marks" ]; then
  echo "S $email" >> "$RECORD_DIR/marks"
  sleep 0.5
  echo "E $email" >> "$RECORD_DIR/marks"
fi
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

/// An OAuth login file. Issue #1700: amplihack reads which top-level keys
/// a login file has (`claudeAiOauth` or not) to tell a subscription login
/// from an API-key profile, so the file is a readable private JSON object.
/// Its values are sentinels that must never be printed; its inode
/// identifies it.
pub fn login(path: &Path) -> u64 {
    private_file(
        path,
        r#"{"claudeAiOauth":{"accessToken":"SENTINEL-TOKEN","refreshToken":"SENTINEL-TOKEN"}}"#,
    );
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
        write_executable(&tools.join("claude"), FAKE_CLAUDE, 0o755);
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

    /// Like [`Self::usage`], with both windows resetting `hours` from now
    /// (1 to 23), written the way Claude writes reset times, in UTC.
    pub fn usage_resetting(&self, p: &str, session: u32, week: u32, hours: i64) {
        assert!(
            (1..24).contains(&hours),
            "a time-only reset is within a day"
        );
        let at = chrono::Utc::now() + chrono::Duration::hours(hours);
        let time = at.format("%-I:%M%P");
        let date = at.format("%b %-d, %-I:%M%P");
        fs::write(
            self.canned.join(format!("{}.out", email(p))),
            format!(
                "{BANNER}\n\nCurrent session: {session}% used · resets {time} (UTC)\n\
                 Current week (all models): {week}% used · resets {date} (UTC)\n"
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

    pub fn assert_isolated(&self) {
        let real = std::env::var_os("HOME").map(PathBuf::from);
        assert!(self.home.starts_with(self.tmp.path()));
        assert!(self.root.starts_with(self.tmp.path()));
        if let Some(real) = real {
            assert_ne!(self.home, real, "never the real HOME");
        }
    }

    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut c = self.base(args);
        c.env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root);
        c
    }

    /// Like `cmd`, without `AMPLIHACK_CLAUDE_PROFILE_ROOT`: the root comes
    /// from `~/.amplihack/profile-root` or the default under the temp HOME.
    pub fn cmd_no_root_env(&self, args: &[&str]) -> Command {
        self.base(args)
    }

    fn base(&self, args: &[&str]) -> Command {
        self.assert_isolated();
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work_dir)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
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
            // Launches never start a real background rotator here (autostart).
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_SKIP_REFLECTION", "1");
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

    /// Recorded environments of every `/usage` probe so far.
    pub fn probe_envs(&self) -> Vec<String> {
        self.records_named("env.")
            .into_iter()
            .map(|(_, body)| body)
            .collect()
    }

    /// How many `/usage` probes ran for profile `p`'s account so far.
    pub fn usage_probes(&self, p: &str) -> usize {
        self.records_named(&format!("env.{}.", email(p))).len()
    }

    /// Record files whose name starts with `prefix`, with their contents.
    fn records_named(&self, prefix: &str) -> Vec<(String, String)> {
        fs::read_dir(&self.records)
            .unwrap()
            .map(|e| e.unwrap())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(name, path)| (name, fs::read_to_string(path).unwrap()))
            .collect()
    }

    /// Adds profile `p` with a login and an account, reading
    /// `session`/`week`; it is not counted in `assert_each_login_once`.
    pub fn add_logged_in(&self, p: &str, session: u32, week: u32) {
        self.ok(&["profile", "add", p]);
        private_file(
            &self.root.join(p).join(".claude.json"),
            &serde_json::json!({"oauthAccount": account_json(p)}).to_string(),
        );
        login(&self.slot(p));
        self.usage(p, session, week);
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
/// time; `--dry-run` lines do not. `switch <from>-><to>` is the line a
/// `profile switch` swap writes (issue #1691).
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
        r"^(cycle live=\S+( \S+:(s=\d+%,w=\d+%|failed|per-token))*|swap \S+->\S+ live s=\d+% w=\d+% target s=\d+% w=\d+% reason=(threshold|upgrade)|switch \S+->\S+|ALERT: .+|recovery: (rolled back|finished) an interrupted swap \S+->\S+|skip: .+|stop: signal received; cycle abandoned|idle: auto-switch is off|sessions archive: .+|would .+)$",
    )
    .unwrap();
    assert!(kinds.is_match(&msg), "line outside the grammar: {line:?}");
    assert_text_has_no_secrets(line);
}
