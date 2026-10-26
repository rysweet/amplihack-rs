//! Shared fixture for the issue #1618 usage-limit wait tests.
//!
//! Runs the real `amplihack` binary with `env_clear`, a temporary `HOME`,
//! profile root and `CLAUDE_CONFIG_DIR`, and a stub `claude` named by
//! `AMPLIHACK_CLAUDE_BINARY_PATH`. No real login, `~/.claude` or
//! `~/.claude-profiles` is read or written. The debug-only seams make the
//! wait virtual: `TEST_NOW` is 2026-10-18 08:00:00 -07:00 and `TZ` is
//! `America/Los_Angeles`, so log times are exact.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const WAIT_PREFIX: &str = "amplihack.recipe.usage_limit_wait ";
/// 2026-10-18 08:00:00 -07:00.
pub const TEST_NOW: i64 = 1_792_335_600;
/// Claude's session-limit message as seen on real hosts, with a reset 30
/// minutes after `TEST_NOW`.
pub const LIMIT_TEXT: &str = "You've hit your session limit · resets 8:30am (America/Los_Angeles)";
/// Real messages: a spend limit with no reset and one with a weekly reset
/// (neither waited out), and one whose session limit resets at 8:30.
pub const SPEND_TEXT: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage";
pub const SPEND_WEEKLY_TEXT: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Oct 8, 12am (America/Los_Angeles)";
pub const SPEND_SESSION_TEXT: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your session limit resets 8:30am (America/Los_Angeles)";
pub const WEEKLY_TEXT: &str =
    "You've hit your weekly limit · resets Oct 8, 12am (America/Los_Angeles)";

/// The stub `claude`:
/// - `--version` answers like Claude Code;
/// - counts its calls per `$STEP` in `$RECORD_DIR/count.<step>`;
/// - records argv, `pwd -P`, `HOME`, `CLAUDE_CONFIG_DIR`, the inherited
///   profile and the SHA-256 of all of stdin (not read with `$SKIP_STDIN`)
///   in `call.<step>.<n>`;
/// - with `$CHANGE_LOGIN_AFTER`, on its first call, starts a detached
///   process that creates `$CLAUDE_CONFIG_DIR/.credentials.json` after that
///   many seconds (a login moved into place during the wait);
/// - prints `attempt <n> output` on stdout, then `$STDOUT_TEXT` if set;
/// - for its first `$LIMIT_CALLS` calls (default 0) in step `$LIMIT_STEP`
///   (any step when unset), and only under profile `$LIMIT_PROFILE` when that
///   is set, prints `$LIMIT_MESSAGE` on stderr and exits `$LIMIT_EXIT`
///   (default 1);
/// - otherwise exits 0.
const STUB_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
step="${STEP:-0}"
n=$(( $(cat "$RECORD_DIR/count.$step" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$RECORD_DIR/count.$step"
if [ -n "$SKIP_STDIN" ]; then sum=skipped; else sum=$(sha256sum | cut -d' ' -f1); fi
{
  printf 'ARGV='
  for a in "$@"; do printf '%s|' "$a"; done
  printf '\n'
  printf 'PWD=%s\n' "$(pwd -P)"
  printf 'HOME=%s\n' "$HOME"
  printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
  printf 'PROFILE=%s\n' "${AMPLIHACK_CLAUDE_PROFILE-<unset>}"
  printf 'STDIN=%s\n' "$sum"
} > "$RECORD_DIR/call.$step.$n"
if [ -n "$CHANGE_LOGIN_AFTER" ] && [ "$n" = 1 ]; then
  ( sleep "$CHANGE_LOGIN_AFTER"; : > "$CLAUDE_CONFIG_DIR/.credentials.json" ) </dev/null >/dev/null 2>&1 &
fi
limit="${LIMIT_CALLS:-0}"
if [ -n "$LIMIT_STEP" ] && [ "$LIMIT_STEP" != "$step" ]; then limit=0; fi
if [ -n "$LIMIT_PROFILE" ] && [ "$LIMIT_PROFILE" != "${AMPLIHACK_CLAUDE_PROFILE-}" ]; then limit=0; fi
echo "attempt $n output"
if [ -n "$STDOUT_TEXT" ]; then printf '%s\n' "$STDOUT_TEXT"; fi
if [ "$n" -le "$limit" ]; then
  printf '%s\n' "$LIMIT_MESSAGE" >&2
  exit "${LIMIT_EXIT:-1}"
fi
exit 0
"#;

pub struct Fixture {
    _tmp: tempfile::TempDir,
    /// Canonical temporary root; every path the run may touch lies under it.
    pub root: PathBuf,
    pub home: PathBuf,
    pub profiles: PathBuf,
    pub config: PathBuf,
    pub records: PathBuf,
    pub work: PathBuf,
    pub tools: PathBuf,
}

pub fn executable(path: &Path, body: &str) {
    crate::support::exec_stub::write_executable(path, body, 0o755);
}

pub fn private_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

impl Fixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let profiles = root.join("profiles");
        let config = root.join("claude-config");
        let records = root.join("records");
        let work = root.join("work");
        let tools = root.join("tools");
        for d in [&home, &config, &records, &work, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        fs::create_dir_all(&profiles).unwrap();
        fs::set_permissions(&profiles, fs::Permissions::from_mode(0o700)).unwrap();
        executable(&tools.join("claude"), STUB_CLAUDE);
        Self {
            _tmp: tmp,
            root,
            home,
            profiles,
            config,
            records,
            work,
            tools,
        }
    }

    /// `amplihack <args>`, isolated, with only the fixture's environment, the wait
    /// seams set, and a step limited once with `LIMIT_TEXT`.
    pub fn cmd(&self, args: &[&str]) -> Command {
        // `isolated` clears the environment and sets the HOME guard; the
        // fixture's own HOME, under the same temp dir, replaces its HOME.
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("TZ", "America/Los_Angeles")
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.profiles)
            .env("CLAUDE_CONFIG_DIR", &self.config)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_UNDER_TEST", amplihack_bin())
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            // Auto-switch is on in these runs, so without this every launch
            // starts a real rotator that outlives the test by an hour.
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW", TEST_NOW.to_string())
            .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "1000000")
            .env("LIMIT_CALLS", "1")
            .env("LIMIT_MESSAGE", LIMIT_TEXT);
        c
    }

    /// A recipe agent step: `amplihack claude --subprocess-safe -- -p step`.
    pub fn step(&self) -> Command {
        self.cmd(&["claude", "--subprocess-safe", "--", "-p", "step"])
    }

    pub fn run(&self, c: &mut Command) -> Output {
        let mut tries = 0;
        loop {
            match c.output() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 50 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                r => return r.expect("run amplihack"),
            }
        }
    }

    pub fn count(&self, step: &str) -> u32 {
        fs::read_to_string(self.records.join(format!("count.{step}")))
            .map(|s| s.trim().parse().unwrap())
            .unwrap_or(0)
    }

    /// The `KEY=value` record of call `n` of `step`.
    pub fn call(&self, step: &str, n: u32) -> std::collections::BTreeMap<String, String> {
        let raw = fs::read_to_string(self.records.join(format!("call.{step}.{n}")))
            .unwrap_or_else(|e| panic!("call {n} of step {step} was not recorded: {e}"));
        raw.lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Writes the user `config.toml`.
    pub fn write_config(&self, body: &str) {
        private_file(&self.home.join(".amplihack").join("config.toml"), body);
    }

    pub fn rerun_dir(&self) -> PathBuf {
        self.home.join(".amplihack").join("rerun")
    }

    /// AC13: every path a call recorded lies under the temporary root.
    pub fn assert_isolated(&self, step: &str) {
        for n in 1..=self.count(step) {
            let call = self.call(step, n);
            for key in ["PWD", "HOME", "CLAUDE_CONFIG_DIR"] {
                let v = &call[key];
                assert!(
                    Path::new(v).starts_with(&self.root),
                    "call {n} {key}={v} is outside the test root {}",
                    self.root.display()
                );
            }
        }
    }
}

pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

pub fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    )
}

/// The usage-limit wait log lines on stderr, prefix removed.
pub fn wait_lines(out: &Output) -> Vec<String> {
    text(&out.stderr)
        .lines()
        .filter_map(|l| l.strip_prefix(WAIT_PREFIX))
        .map(str::to_string)
        .collect()
}

/// `wait_lines` without the `progress` lines, whose count depends on timing.
pub fn events(out: &Output) -> Vec<String> {
    wait_lines(out)
        .into_iter()
        .filter(|l| !l.starts_with("progress "))
        .collect()
}

/// Every `progress` line has exactly the documented shape.
pub fn assert_progress_shape(out: &Output) {
    for l in wait_lines(out)
        .iter()
        .filter(|l| l.starts_with("progress "))
    {
        let secs = l
            .strip_prefix("progress remaining=")
            .and_then(|r| r.strip_suffix('s'))
            .unwrap_or_else(|| panic!("malformed progress line {l:?}"));
        assert!(
            !secs.is_empty() && secs.bytes().all(|b| b.is_ascii_digit()),
            "{l:?}"
        );
    }
}

/// What the stub printed on stdout, without amplihack's `NODE_OPTIONS` banner.
pub fn claude_stdout(out: &Output) -> String {
    text(&out.stdout)
        .lines()
        .filter(|l| !l.contains("NODE_OPTIONS"))
        .map(|l| format!("{l}\n"))
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
