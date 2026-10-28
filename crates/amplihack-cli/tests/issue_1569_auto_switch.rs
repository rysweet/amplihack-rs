//! Issue #1569: automatic switching between Claude login profiles.
//!
//! Runs the real `amplihack` binary with an isolated `HOME` and profile root,
//! a fake `claude` on `PATH`, and sentinel credential files. The sentinels
//! stand in for tokens: the tests prove each one is in exactly one file at
//! every step (a swap moves, never copies) and never appears in anything
//! amplihack prints.
//!
//! The swap tests need `/proc`, so they are Linux-only. A "holder" process
//! (`sleep` with `CLAUDE_CONFIG_DIR` set) stands in for a running Claude
//! session, which is how auto-switch decides a directory is live.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::profile_default::set_default;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
const SUPERVISED_ENV: &str = "AMPLIHACK_AUTO_SWITCH_SUPERVISED";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the Linux-only swap tests
const ABORT_ENV: &str = "AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER";
const LIMIT_MARKER: &str = "Claude AI usage limit reached";
const PROFILES: [&str; 3] = ["X", "Y", "Z"];

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn executable(path: &Path, body: &str) {
    crate::support::exec_stub::write_executable(path, body, 0o755);
}

fn private_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// The fake `claude`:
/// - `--version` answers like Claude Code;
/// - records its environment and the inode of the live credentials file;
/// - runs `$STATUSLINE_CMD` (the installed wrapper) with `$STATUSLINE_JSON`
///   on stdin, the way Claude Code runs a status line after a turn;
/// - appends its profile to `attempts`;
/// - for profiles listed in `$LIMITED` prints the usage-limit marker on its
///   last stderr line and exits 1;
/// - with `$KILL_PARENT`, prints the marker and kills its launcher (the
///   supervised attempt) with SIGKILL;
/// - with `$INT_SUPERVISOR`, sends SIGINT to its launcher's parent (the
///   rerun supervisor, which records it without passing it on), then prints
///   the marker and exits 1;
/// - with `$INJECT`, prints that text to stderr, then `done as <profile>`,
///   and exits `$EXIT_CODE` (default 1);
/// - otherwise prints `done as <profile>` and exits `$EXIT_CODE`.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
p="${AMPLIHACK_CLAUDE_PROFILE-<unset>}"
echo "$p" >> "$RECORD_DIR/attempts"
{
  printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
  printf 'AMPLIHACK_CLAUDE_PROFILE=%s\n' "$p"
  printf 'AMPLIHACK_AUTO_SWITCH_SUPERVISED=%s\n' "${AMPLIHACK_AUTO_SWITCH_SUPERVISED-<unset>}"
  if [ -e "$CLAUDE_CONFIG_DIR/.credentials.json" ]; then
    printf 'CRED_INODE=%s\n' "$(stat -c %i "$CLAUDE_CONFIG_DIR/.credentials.json")"
  fi
} > "$RECORD_DIR/env.$p"
if [ -n "$STATUSLINE_CMD" ]; then
  printf '%s' "$STATUSLINE_JSON" | sh -c "$STATUSLINE_CMD" > "$RECORD_DIR/statusline.out" 2> "$RECORD_DIR/statusline.err"
  if [ -e "$CLAUDE_CONFIG_DIR/.credentials.json" ]; then
    printf 'CRED_INODE_AFTER=%s\n' "$(stat -c %i "$CLAUDE_CONFIG_DIR/.credentials.json")" >> "$RECORD_DIR/env.$p"
  fi
fi
for l in $LIMITED; do
  if [ "$l" = "$p" ]; then
    echo "partial output from $p"
    echo "working..." >&2
    echo "Claude AI usage limit reached|$LIMIT_RESET" >&2
    exit 1
  fi
done
if [ -n "$KILL_PARENT" ]; then
  echo "Claude AI usage limit reached|$LIMIT_RESET" >&2
  kill -KILL "$PPID"
  exit 1
fi
if [ -n "$INT_SUPERVISOR" ]; then
  kill -INT "$(cut -d' ' -f4 "/proc/$PPID/stat")"
  echo "Claude AI usage limit reached|$LIMIT_RESET" >&2
  exit 1
fi
if [ -n "$INJECT" ]; then
  printf '%s\n' "$INJECT" >&2
  echo "done as $p"
  exit "${EXIT_CODE:-1}"
fi
echo "done as $p"
exit "${EXIT_CODE:-0}"
"#;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the Linux-only swap tests
struct Holder(Child);

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    records: PathBuf,
    work: PathBuf,
    tools: PathBuf,
    /// Everything amplihack printed, checked for sentinels by `assert_no_leak`.
    printed: RefCell<Vec<u8>>,
    /// Sentinel credential contents, by profile.
    creds: BTreeMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let root = home.join("profiles");
        let records = tmp.path().join("records");
        let work = tmp.path().join("work");
        let tools = tmp.path().join("tools");
        for d in [&home, &records, &work, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        executable(&tools.join("claude"), FAKE_CLAUDE);
        let mut f = Self {
            _tmp: tmp,
            home,
            root,
            records,
            work,
            tools,
            printed: RefCell::new(Vec::new()),
            creds: BTreeMap::new(),
        };
        for name in PROFILES {
            f.ok(&["profile", "add", name]);
        }
        // Root canonical from here on: profile dirs are compared as paths.
        f.root = f.root.canonicalize().unwrap();
        for name in PROFILES {
            let secret = format!("SENTINEL-TOKEN-{name}-7f3a9c");
            private_file(
                &f.dir(name).join(".credentials.json"),
                &format!(
                    "{{\"claudeAiOauth\":{{\"accessToken\":\"{secret}\",\"refreshToken\":\"{secret}-r\"}}}}"
                ),
            );
            private_file(
                &f.dir(name).join(".claude.json"),
                &format!(
                    "{{\"primaryApiKey\":\"SENTINEL-APIKEY-{name}\",\"numStartups\":3,\
                     \"oauthAccount\":{{\"emailAddress\":\"{}@example.invalid\",\
                     \"accountUuid\":\"uuid-{name}\",\"organizationUuid\":\"org-{name}\"}}}}",
                    name.to_lowercase()
                ),
            );
            f.creds.insert(name.to_string(), secret);
        }
        f
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn state(&self) -> PathBuf {
        self.root.join(".auto-switch.d")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        self.cmd_bin(&amplihack_bin(), args)
    }

    /// Like `cmd`, but running another copy of the binary.
    fn cmd_bin(&self, bin: &Path, args: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(bin);
        c.args(args)
            .current_dir(&self.work)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_UNDER_TEST", amplihack_bin())
            .env("RECORD_DIR", &self.records)
            .env("LIMIT_RESET", (now() + 3600).to_string())
            // A secret in the environment must not be echoed either.
            .env("ANTHROPIC_API_KEY", "SENTINEL-ENV-KEY")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            // These tests cover auto-switch alone; the #1618 usage-limit wait
            // (on by default for recipe steps) has its own tests.
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            // Launches never start a real background rotator here (autostart).
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root);
        c
    }

    fn run(&self, c: &mut Command) -> Output {
        // A test that has just written an executable (a copied amplihack, a
        // stub script) can see exec fail with ETXTBSY: another test thread's
        // fork can inherit the still-open write descriptor until its own exec
        // closes it. The window is brief, so retry instead of failing the test.
        let mut attempts = 0;
        let out = loop {
            match c.output() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                r => break r.unwrap(),
            }
        };
        let mut p = self.printed.borrow_mut();
        p.extend_from_slice(&out.stdout);
        p.extend_from_slice(&out.stderr);
        out
    }

    fn run_args(&self, args: &[&str]) -> Output {
        self.run(&mut self.cmd(args))
    }

    fn ok(&self, args: &[&str]) -> Output {
        let out = self.run_args(args);
        assert!(
            out.status.success(),
            "`amplihack {}` failed: {}\nstdout: {}\nstderr: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        out
    }

    fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8(self.ok(args).stdout).unwrap()
    }

    fn switch_on(&self) {
        self.ok(&["profile", "auto-switch", "on"]);
    }

    /// A process standing in for a Claude session running in `name`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the Linux-only swap tests
    fn hold(&self, name: &str) -> Holder {
        let child = Command::new("sleep")
            .arg("120")
            .env_clear()
            .env("CLAUDE_CONFIG_DIR", self.dir(name))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Make sure /proc/<pid>/environ is the child's, not the pre-exec copy.
        for _ in 0..100 {
            let exe = fs::read_link(format!("/proc/{}/exe", child.id())).unwrap_or_default();
            if exe.file_name().is_some_and(|n| n == "sleep") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Holder(child)
    }

    /// Status-line input like Claude Code's, with the given usage.
    fn statusline_json(five_hour: f64, seven_day: f64) -> String {
        format!(
            "{{\"session_id\":\"s\",\"model\":{{\"id\":\"m\"}},\"rate_limits\":{{\
             \"five_hour\":{{\"used_percentage\":{five_hour},\"resets_at\":{}}},\
             \"seven_day\":{{\"used_percentage\":{seven_day},\"resets_at\":{}}}}}}}",
            now() + 3600,
            now() + 3 * 86400
        )
    }

    /// One status-line tick of the wrapper for a session running in `owner`.
    fn tick_cmd(&self, owner: &str) -> Command {
        let mut c = self.cmd(&[
            "profile",
            "statusline",
            "--root",
            self.root.to_str().unwrap(),
            "--owner",
            owner,
        ]);
        c.env("CLAUDE_CONFIG_DIR", self.dir(owner))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    fn tick_with(&self, mut c: Command, json: &str) -> Output {
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(json.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let mut p = self.printed.borrow_mut();
        p.extend_from_slice(&out.stdout);
        p.extend_from_slice(&out.stderr);
        out
    }

    fn tick(&self, owner: &str, five_hour: f64) -> Output {
        let json = Self::statusline_json(five_hour, 10.0);
        self.tick_with(self.tick_cmd(owner), &json)
    }

    /// Every regular file under `dir` (recursively) holding `needle`.
    fn files_containing(dir: &Path, needle: &str) -> Vec<PathBuf> {
        let mut hits = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return hits;
        };
        for e in entries.flatten() {
            let p = e.path();
            let meta = fs::symlink_metadata(&p).unwrap();
            if meta.is_dir() {
                hits.extend(Self::files_containing(&p, needle));
            } else if meta.is_file()
                && fs::read(&p)
                    .map(|b| String::from_utf8_lossy(&b).contains(needle))
                    .unwrap_or(false)
            {
                hits.push(p);
            }
        }
        hits
    }

    /// Where account `name`'s login is now; panics unless it is in exactly
    /// one file under the whole temp tree (root, home, records).
    fn where_is(&self, name: &str) -> PathBuf {
        let secret = &self.creds[name];
        let hits = Self::files_containing(self._tmp.path(), secret);
        assert_eq!(
            hits.len(),
            1,
            "login of {name} must be in exactly one place, found {hits:?}"
        );
        hits.into_iter().next().unwrap()
    }

    fn assert_one_place_each(&self) {
        for name in PROFILES {
            self.where_is(name);
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the Linux-only swap tests
    fn parked(&self, name: &str) -> PathBuf {
        self.state()
            .join("parked")
            .join(name)
            .join(".credentials.json")
    }

    fn inode(path: &Path) -> u64 {
        fs::symlink_metadata(path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            .ino()
    }

    fn ledger(&self) -> serde_json::Value {
        let raw = fs::read_to_string(self.state().join("ledger.json")).unwrap_or_default();
        if raw.is_empty() {
            return serde_json::json!({"hosts": {}});
        }
        serde_json::from_str(&raw).unwrap()
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the Linux-only swap tests
    fn hosts(&self) -> BTreeMap<String, String> {
        self.ledger()["hosts"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v["live"].as_str().unwrap().to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the Linux-only swap tests
    fn oauth_uuid(&self, name: &str) -> String {
        let raw = fs::read_to_string(self.dir(name).join(".claude.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        v["oauthAccount"]["accountUuid"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn assert_no_leak(&self) {
        let printed = String::from_utf8_lossy(&self.printed.borrow()).to_string();
        let mut secrets: Vec<String> = self.creds.values().cloned().collect();
        for n in PROFILES {
            secrets.push(format!("SENTINEL-APIKEY-{n}"));
            secrets.push(format!("{}@example.invalid", n.to_lowercase()));
            secrets.push(format!("org-{n}"));
        }
        secrets.push("SENTINEL-ENV-KEY".into());
        secrets.push("SENTINEL-SETTINGS-ENV".into());
        for s in secrets {
            assert!(
                !printed.contains(&s),
                "amplihack printed secret {s:?}; output:\n{printed}"
            );
        }
    }

    /// All paths under the root with their mode and content, for comparing
    /// the tree before and after.
    fn snapshot(&self) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<PathBuf, (u32, Vec<u8>)>) {
            for e in fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                let m = fs::symlink_metadata(&p).unwrap();
                let rel = p.strip_prefix(base).unwrap().to_path_buf();
                if m.is_dir() {
                    out.insert(rel, (m.mode(), Vec::new()));
                    walk(&p, base, out);
                } else {
                    out.insert(rel, (m.mode(), fs::read(&p).unwrap_or_default()));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.root, &self.root, &mut out);
        out
    }

    /// Seeds `usage.json` directly, in the documented format, for tests that
    /// need an exhausted account without running a swap.
    fn seed_usage(&self, readings: &[(&str, f64)]) {
        let weeks: Vec<(&str, f64, f64)> = readings.iter().map(|(n, p)| (*n, *p, 5.0)).collect();
        self.seed_usage_weeks(&weeks);
    }

    /// [`Self::seed_usage`] with each account's (five-hour, seven-day) use.
    fn seed_usage_weeks(&self, readings: &[(&str, f64, f64)]) {
        let state = self.state();
        fs::create_dir_all(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let accounts: serde_json::Map<String, serde_json::Value> = readings
            .iter()
            .map(|(n, pct, week)| {
                (
                    n.to_string(),
                    serde_json::json!({
                        "five_hour": {"used_percentage": pct, "resets_at": now() + 3600},
                        "seven_day": {"used_percentage": week, "resets_at": now() + 86400},
                        "observed_at": now(),
                        "source": "statusline",
                    }),
                )
            })
            .collect();
        private_file(
            &state.join("usage.json"),
            &serde_json::json!({"version": 1, "accounts": accounts}).to_string(),
        );
    }

    /// Moves every recorded swap back past the five-minute cooldown, so the
    /// next status-line reading is attributed to the account now live.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the Linux-only swap tests
    fn age_swaps(&self) {
        let mut ledger = self.ledger();
        if let Some(hosts) = ledger["hosts"].as_object_mut() {
            for h in hosts.values_mut() {
                h["swapped_at"] = serde_json::json!(now() - 600);
            }
        }
        private_file(&self.state().join("ledger.json"), &ledger.to_string());
    }

    fn attempts(&self) -> Vec<String> {
        fs::read_to_string(self.records.join("attempts"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn record(&self, profile: &str) -> String {
        fs::read_to_string(self.records.join(format!("env.{profile}")))
            .unwrap_or_else(|e| panic!("no record for {profile}: {e}"))
    }

    /// A nested `amplihack claude -p` launch inheriting `profile`, the way a
    /// recipe agent step runs inside a session.
    fn nested(&self, profile: &str) -> Command {
        let mut c = self.cmd(&["claude", "--subprocess-safe", "--", "-p", "step"]);
        c.env("AMPLIHACK_CLAUDE_PROFILE", profile)
            .env("CLAUDE_CONFIG_DIR", self.dir(profile));
        c
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

/// A launcher's stdout without amplihack's own `NODE_OPTIONS` banner, which
/// it prints today with or without auto-switch: what the fake `claude` wrote.
fn claude_stdout(out: &Output) -> String {
    text(&out.stdout)
        .lines()
        .filter(|l| !l.contains("NODE_OPTIONS"))
        .map(|l| format!("{l}\n"))
        .collect()
}

/// A refusal by auto-switch itself: non-zero, an `error:`/`hint:` pair, and
/// `needle` in stderr. A clap "unrecognized subcommand" does not count.
fn assert_refused(out: &Output, needle: &str) {
    let stderr = text(&out.stderr);
    assert!(!out.status.success(), "must be refused: {stderr}");
    assert!(!stderr.contains("unrecognized subcommand"), "{stderr}");
    assert!(
        stderr.contains(needle),
        "stderr must mention {needle:?}: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Toggle, precedence and settings file
// ---------------------------------------------------------------------------

#[test]
fn toggle_defaults_off_and_persists_on_and_off() {
    let f = Fixture::new();
    assert!(
        f.stdout(&["profile", "auto-switch", "status"])
            .contains("switch: off (default)")
    );
    assert!(!f.root.join(".auto-switch").exists());
    assert!(
        !f.state().exists(),
        "status while off must not create state"
    );

    f.ok(&[
        "profile",
        "auto-switch",
        "on",
        "--threshold",
        "80",
        "--exclude",
        "Z",
    ]);
    let file = f.root.join(".auto-switch");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "enabled=on\nthreshold=80\nexclude=Z\n"
    );
    assert_eq!(fs::symlink_metadata(&file).unwrap().mode() & 0o777, 0o600);
    let status = f.stdout(&["profile", "auto-switch", "status"]);
    assert!(status.contains("switch: on (file)"), "{status}");
    assert!(status.contains("threshold: 80"), "{status}");

    // `off` keeps the threshold and exclusions.
    f.ok(&["profile", "auto-switch", "off"]);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "enabled=off\nthreshold=80\nexclude=Z\n"
    );
}

#[test]
fn threshold_and_exclude_rewrite_only_their_key_and_are_byte_stable() {
    let f = Fixture::new();
    // Missing file: `threshold` creates it with enabled=off.
    f.ok(&["profile", "auto-switch", "threshold", "75"]);
    let file = f.root.join(".auto-switch");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "enabled=off\nthreshold=75\nexclude=\n"
    );
    f.ok(&["profile", "auto-switch", "on"]);
    f.ok(&["profile", "auto-switch", "exclude", "Y", "Z"]);
    let first = fs::read(&file).unwrap();
    assert_eq!(text(&first), "enabled=on\nthreshold=75\nexclude=Y,Z\n");
    f.ok(&["profile", "auto-switch", "exclude", "Y", "Z"]);
    assert_eq!(
        fs::read(&file).unwrap(),
        first,
        "rewrites must be byte-stable"
    );
    // No names clears the list.
    f.ok(&["profile", "auto-switch", "exclude"]);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "enabled=on\nthreshold=75\nexclude=\n"
    );
}

#[test]
fn invalid_threshold_and_exclude_names_are_refused() {
    let f = Fixture::new();
    for bad in ["0", "101", "-1", "ninety"] {
        let out = f.run_args(&["profile", "auto-switch", "threshold", "--", bad]);
        assert_refused(&out, "threshold");
    }
    let out = f.run_args(&["profile", "auto-switch", "exclude", "../etc"]);
    assert_refused(&out, "invalid profile name");
    assert!(!f.root.join(".auto-switch").exists());
}

#[test]
fn env_overrides_file_in_both_directions() {
    let f = Fixture::new();
    f.switch_on();
    let out = f.run(
        f.cmd(&["profile", "auto-switch", "status"])
            .env(TOGGLE_ENV, "0"),
    );
    assert!(
        text(&out.stdout).contains("switch: off (env)"),
        "{}",
        text(&out.stdout)
    );

    f.ok(&["profile", "auto-switch", "off"]);
    let out = f.run(
        f.cmd(&["profile", "auto-switch", "status"])
            .env(TOGGLE_ENV, "1"),
    );
    assert!(
        text(&out.stdout).contains("switch: on (env)"),
        "{}",
        text(&out.stdout)
    );

    // Empty means unset: the file decides.
    let out = f.run(
        f.cmd(&["profile", "auto-switch", "status"])
            .env(TOGGLE_ENV, ""),
    );
    assert!(
        text(&out.stdout).contains("switch: off (file)"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn unrecognised_env_value_warns_and_lets_the_file_decide() {
    let f = Fixture::new();
    f.switch_on();
    let out = f.run(
        f.cmd(&["profile", "auto-switch", "status"])
            .env(TOGGLE_ENV, "yes"),
    );
    assert!(out.status.success());
    assert!(
        text(&out.stdout).contains("switch: on (file)"),
        "{}",
        text(&out.stdout)
    );
    let stderr = text(&out.stderr);
    assert_eq!(
        stderr.matches(TOGGLE_ENV).count(),
        1,
        "exactly one warning naming the variable: {stderr}"
    );
}

#[test]
fn corrupt_settings_file_means_off_and_is_replaced_by_on() {
    let f = Fixture::new();
    let file = f.root.join(".auto-switch");
    for corrupt in [
        "enabled=maybe\n",
        "threshold=90\n",              // no enabled
        "enabled=on\nenabled=off\n",   // repeated key
        "enabled=on\ncolour=blue\n",   // unknown key
        "enabled=on\nthreshold=500\n", // out of range
    ] {
        private_file(&file, corrupt);
        let s = f.stdout(&["profile", "auto-switch", "status"]);
        assert!(s.contains("switch: off (corrupt)"), "{corrupt:?}: {s}");
    }
    private_file(&file, &"x".repeat(5000));
    assert!(
        f.stdout(&["profile", "auto-switch", "status"])
            .contains("switch: off (corrupt)")
    );

    // `threshold` refuses a corrupt file; `on` replaces it.
    let out = f.run_args(&["profile", "auto-switch", "threshold", "50"]);
    assert!(!out.status.success());
    f.switch_on();
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "enabled=on\nthreshold=90\nexclude=\n"
    );
}

#[test]
fn symlinked_settings_file_is_never_written_through() {
    let f = Fixture::new();
    let target = f.home.join("elsewhere");
    fs::write(&target, "enabled=off\n").unwrap();
    std::os::unix::fs::symlink(&target, f.root.join(".auto-switch")).unwrap();
    let out = f.run_args(&["profile", "auto-switch", "on"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("hint:"), "{}", text(&out.stderr));
    assert_eq!(fs::read_to_string(&target).unwrap(), "enabled=off\n");
    assert!(
        f.stdout(&["profile", "auto-switch", "status"])
            .contains("switch: off (corrupt)")
    );
}

#[test]
fn state_directory_never_appears_in_profile_list() {
    let f = Fixture::new();
    f.switch_on();
    f.seed_usage(&[("X", 10.0)]);
    let list = f.stdout(&["profile", "list"]);
    assert!(!list.contains(".auto-switch"), "{list}");
    let n = list.lines().filter(|l| !l.starts_with("root: ")).count();
    assert_eq!(n, PROFILES.len(), "{list}");
}

// ---------------------------------------------------------------------------
// Status-line wrapper and install
// ---------------------------------------------------------------------------

fn settings(f: &Fixture, name: &str) -> serde_json::Value {
    let raw = fs::read_to_string(f.dir(name).join("settings.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn write_settings(f: &Fixture, name: &str, v: &serde_json::Value) {
    private_file(&f.dir(name).join("settings.json"), &v.to_string());
}

fn canonical_wrapper(f: &Fixture, name: &str) -> String {
    format!(
        "'{}' profile statusline --root '{}' --owner '{}'",
        amplihack_bin().canonicalize().unwrap().display(),
        f.root.display(),
        name
    )
}

#[test]
fn install_handles_all_four_cases_and_is_idempotent() {
    let f = Fixture::new();
    // X: no settings.json at all.
    let _ = fs::remove_file(f.dir("X").join("settings.json"));
    // Y: someone else's status line, plus a secret-bearing key to keep.
    write_settings(
        &f,
        "Y",
        &serde_json::json!({
            "statusLine": {"type": "command", "command": "echo mine", "padding": 2},
            "env": {"TOKEN": "SENTINEL-SETTINGS-ENV"},
            "model": "opus",
        }),
    );
    // The user chose 0644 for this one; install keeps it (D5).
    fs::set_permissions(
        f.dir("Y").join("settings.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    // Z: an old wrapper pointing at another binary.
    write_settings(
        &f,
        "Z",
        &serde_json::json!({"statusLine": {"type": "command",
            "command": "'/old/amplihack' profile statusline --root '/old' --owner 'Z'"}}),
    );
    // ~/.claude must never be touched.
    let dot_claude = f.home.join(".claude");
    fs::create_dir_all(&dot_claude).unwrap();
    fs::write(dot_claude.join("settings.json"), "{\"sentinel\":true}").unwrap();

    let out = f.ok(&["profile", "auto-switch", "install"]);
    assert!(
        text(&out.stdout).contains(&f.root.display().to_string()),
        "install must print the root it acts on: {}",
        text(&out.stdout)
    );

    for name in ["X", "Y"] {
        let s = settings(&f, name);
        assert_eq!(s["statusLine"]["type"], "command", "{name}");
        assert_eq!(
            s["statusLine"]["command"].as_str().unwrap(),
            canonical_wrapper(&f, name),
            "{name}"
        );
    }
    // A stale wrapper is reported, not rewritten (that is `repair`'s job).
    assert!(
        settings(&f, "Z")["statusLine"]["command"]
            .as_str()
            .unwrap()
            .starts_with("'/old/amplihack'")
    );
    assert!(
        text(&out.stdout).contains("stale      Z"),
        "{}",
        text(&out.stdout)
    );
    // A new settings.json is private; an existing one keeps its mode.
    let mode = |n: &str| fs::metadata(f.dir(n).join("settings.json")).unwrap().mode() & 0o7777;
    assert_eq!(mode("X"), 0o600);
    assert_eq!(mode("Y"), 0o644);
    let y = settings(&f, "Y");
    assert_eq!(y["statusLine"]["padding"], 2, "padding is kept");
    assert_eq!(
        y["env"]["TOKEN"], "SENTINEL-SETTINGS-ENV",
        "other keys kept"
    );
    assert_eq!(y["model"], "opus");
    let saved = fs::read_to_string(f.state().join("statusline").join("Y.json")).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(saved["command"], "echo mine", "original saved for chaining");
    let saved_mode = fs::metadata(f.state().join("statusline").join("Y.json"))
        .unwrap()
        .mode();
    assert_eq!(saved_mode & 0o777, 0o600);

    // Second run writes nothing.
    let before: Vec<_> = PROFILES
        .iter()
        .map(|n| {
            let p = f.dir(n).join("settings.json");
            (fs::read(&p).unwrap(), Fixture::inode(&p))
        })
        .collect();
    f.ok(&["profile", "auto-switch", "install"]);
    let after: Vec<_> = PROFILES
        .iter()
        .map(|n| {
            let p = f.dir(n).join("settings.json");
            (fs::read(&p).unwrap(), Fixture::inode(&p))
        })
        .collect();
    assert_eq!(before, after, "a second install must not write");

    assert_eq!(
        fs::read_to_string(dot_claude.join("settings.json")).unwrap(),
        "{\"sentinel\":true}"
    );
    assert_eq!(fs::read_dir(&dot_claude).unwrap().count(), 1);
    f.assert_no_leak();
}

#[test]
fn install_refuses_symlinked_settings() {
    let f = Fixture::new();
    let outside = f.home.join("outside.json");
    fs::write(&outside, "{}").unwrap();
    let _ = fs::remove_file(f.dir("X").join("settings.json"));
    std::os::unix::fs::symlink(&outside, f.dir("X").join("settings.json")).unwrap();
    let out = f.run_args(&["profile", "auto-switch", "install"]);
    assert_refused(&out, "settings.json");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "{}");
}

#[test]
fn install_refuses_an_amplihack_binary_in_a_writable_directory() {
    let f = Fixture::new();
    let shared = f.home.join("shared-bin");
    fs::create_dir_all(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
    let exe = shared.join("amplihack");
    crate::support::exec_stub::copy_executable(&amplihack_bin(), &exe);
    let before = fs::read(f.dir("Y").join("settings.json")).ok();
    let mut c = f.cmd_bin(&exe, &["profile", "auto-switch", "install"]);
    let out = f.run(&mut c);
    assert_refused(&out, &shared.display().to_string());
    assert_eq!(fs::read(f.dir("Y").join("settings.json")).ok(), before);
}

#[test]
fn wrapper_passes_inner_output_and_exit_code_through_unchanged() {
    let f = Fixture::new();
    write_settings(
        &f,
        "X",
        &serde_json::json!({"statusLine": {"type": "command",
            "command": "cat; printf '\\001\\377 tail'; exit 3"}}),
    );
    f.ok(&["profile", "auto-switch", "install"]);
    f.switch_on();
    let json = Fixture::statusline_json(20.0, 10.0);
    let out = f.tick_with(f.tick_cmd("X"), &json);
    let mut want = json.clone().into_bytes();
    want.extend_from_slice(b"\x01\xff tail");
    assert_eq!(
        out.stdout, want,
        "stdout must be the inner command's, byte for byte"
    );
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn wrapper_without_inner_command_prints_nothing_and_exits_zero() {
    let f = Fixture::new();
    f.ok(&["profile", "auto-switch", "install"]);
    f.switch_on();
    let out = f.tick("X", 20.0);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn wrapper_on_an_unsafe_root_prints_nothing_and_says_why_on_stderr() {
    let f = Fixture::new();
    f.ok(&["profile", "auto-switch", "install"]);
    f.switch_on();
    let mode = fs::metadata(&f.root).unwrap().mode() & 0o7777;
    fs::set_permissions(&f.root, fs::Permissions::from_mode(0o777)).unwrap();
    let out = f.tick("X", 20.0);
    fs::set_permissions(&f.root, fs::Permissions::from_mode(mode)).unwrap();
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert_eq!(out.status.code(), Some(0));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("status line disabled")
            && stderr.contains(&f.root.display().to_string())
            && stderr.contains("writable by other users"),
        "{stderr}"
    );
}

#[test]
fn wrapper_records_a_reading_while_on() {
    let f = Fixture::new();
    f.switch_on();
    let out = f.tick("X", 42.0);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let usage: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.state().join("usage.json")).unwrap()).unwrap();
    assert_eq!(usage["version"], 1);
    let x = &usage["accounts"]["X"];
    assert_eq!(x["five_hour"]["used_percentage"].as_f64(), Some(42.0));
    assert_eq!(x["source"], "statusline");
    assert!(x["observed_at"].as_i64().unwrap() >= now() - 60);
    for p in [f.state(), f.state().join("usage.json")] {
        let mode = fs::metadata(&p).unwrap().mode();
        assert_eq!(mode & 0o077, 0, "{} must be private", p.display());
    }
}

#[test]
fn wrapper_accepts_iso_reset_times_and_ignores_input_without_rate_limits() {
    let f = Fixture::new();
    f.switch_on();
    let json = "{\"rate_limits\":{\"five_hour\":{\"used_percentage\":33,\
                \"resets_at\":\"2099-01-01T00:00:00Z\"}}}";
    // Out of range (more than eight days ahead): the window is dropped.
    let out = f.tick_with(f.tick_cmd("X"), json);
    assert!(out.status.success());
    let json = "{\"model\":{\"id\":\"m\"}}";
    let out = f.tick_with(f.tick_cmd("Y"), json);
    assert!(out.status.success());
    let usage = fs::read_to_string(f.state().join("usage.json")).unwrap_or_default();
    assert!(
        !usage.contains("\"Y\""),
        "no rate_limits must record nothing: {usage}"
    );
    assert!(
        !usage.contains("33"),
        "out-of-range resets_at must be dropped: {usage}"
    );

    let iso = time_iso(now() + 1800);
    let json = format!(
        "{{\"rate_limits\":{{\"five_hour\":{{\"used_percentage\":33,\"resets_at\":\"{iso}\"}}}}}}"
    );
    let out = f.tick_with(f.tick_cmd("X"), &json);
    assert!(out.status.success());
    let usage = fs::read_to_string(f.state().join("usage.json")).unwrap();
    assert!(
        usage.contains("33"),
        "RFC 3339 resets_at must be accepted: {usage}"
    );
}

fn time_iso(epoch: i64) -> String {
    let out = Command::new("date")
        .args(["-u", "-d", &format!("@{epoch}"), "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .unwrap();
    text(&out.stdout).trim().to_string()
}

#[test]
fn wrapper_malformed_input_still_runs_inner_command() {
    let f = Fixture::new();
    write_settings(
        &f,
        "X",
        &serde_json::json!({"statusLine": {"type": "command", "command": "echo inner"}}),
    );
    f.ok(&["profile", "auto-switch", "install"]);
    f.switch_on();
    let out = f.tick_with(f.tick_cmd("X"), "not json");
    assert_eq!(text(&out.stdout), "inner\n");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn wrapper_for_a_different_config_dir_is_passthrough_only() {
    let f = Fixture::new();
    f.switch_on();
    let json = Fixture::statusline_json(99.0, 10.0);
    let mut c = f.tick_cmd("X");
    c.env("CLAUDE_CONFIG_DIR", f.dir("Y"));
    let out = f.tick_with(c, &json);
    assert!(out.status.success());
    let usage = fs::read_to_string(f.state().join("usage.json")).unwrap_or_default();
    assert!(
        !usage.contains("99"),
        "mismatched dir must not record: {usage}"
    );
    f.assert_one_place_each();
    assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));
}

// ---------------------------------------------------------------------------
// In-session swap (Linux)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod swap {
    use super::*;

    fn prime(f: &Fixture) {
        f.switch_on();
    }

    #[test]
    fn exhausted_session_swaps_by_move_not_copy() {
        let f = Fixture::new();
        prime(&f);
        let y_inode = Fixture::inode(&f.dir("Y").join(".credentials.json"));
        let x_inode = Fixture::inode(&f.dir("X").join(".credentials.json"));
        let _session = f.hold("X");

        let out = f.tick("X", 95.0);
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert_eq!(
            text(&out.stderr),
            "amplihack: profile \"X\" reached the auto-switch threshold; the session in profile \"X\" now uses the login of profile \"Y\"\n"
        );
        assert_eq!(text(&out.stdout), "", "nothing on the status line");

        let live = f.dir("X").join(".credentials.json");
        assert_eq!(
            f.where_is("Y"),
            live,
            "Y's login is now live in X's session"
        );
        assert_eq!(Fixture::inode(&live), y_inode, "moved, not copied");
        assert!(!f.dir("Y").join(".credentials.json").exists());
        assert_eq!(f.where_is("X"), f.parked("X"), "X's own login is parked");
        assert_eq!(Fixture::inode(&f.parked("X")), x_inode);
        assert_eq!(fs::metadata(f.parked("X")).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(f.parked("X").parent().unwrap())
                .unwrap()
                .mode()
                & 0o777,
            0o700
        );
        let mtime = fs::metadata(&live).unwrap().mtime();
        assert!(mtime >= now() - 30, "moved-in login must get a fresh mtime");
        assert_eq!(f.hosts(), BTreeMap::from([("X".into(), "Y".into())]));
        assert!(!f.state().join("journal.json").exists());
        f.assert_one_place_each();
        f.assert_no_leak();
    }

    #[test]
    fn swap_patches_only_oauth_account() {
        let f = Fixture::new();
        prime(&f);
        let _session = f.hold("X");
        f.tick("X", 95.0);
        assert_eq!(
            f.oauth_uuid("X"),
            "uuid-Y",
            "/status must name the live account"
        );
        let raw = fs::read_to_string(f.dir("X").join(".claude.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["primaryApiKey"], "SENTINEL-APIKEY-X", "other keys kept");
        assert_eq!(v["numStartups"], 3);
        let mode = fs::metadata(f.dir("X").join(".claude.json"))
            .unwrap()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        // The snapshot holds the oauthAccount block only.
        let snap = fs::read_to_string(f.state().join("accounts").join("Y.json")).unwrap();
        assert!(!snap.contains("SENTINEL"), "{snap}");
        f.assert_no_leak();
    }

    /// The in-session swap asks the overflow gate (issue #1606): a plan
    /// profile with capacity wins over the overflow profile; with none, and
    /// the plan reset an hour away (beyond the default 30-minute wait), the
    /// session moves onto the overflow profile. "Capacity" is the rotator's
    /// test, below 85% session and 90% week, so a plan profile at 87%
    /// session does not count. No rotator runs here, so the gate is the
    /// defaults.
    #[test]
    fn the_swap_uses_overflow_only_when_no_plan_profile_has_capacity() {
        for (y, to) in [(40.0, "Y"), (87.0, "Z"), (99.0, "Z")] {
            let f = Fixture::new();
            prime(&f);
            f.ok(&["profile", "overflow", "add", "Z"]);
            f.seed_usage(&[("Y", y), ("Z", 0.0)]);
            let _session = f.hold("X");
            let out = f.tick("X", 95.0);
            assert!(out.status.success(), "{}", text(&out.stderr));
            assert_eq!(
                f.hosts().get("X").map(String::as_str),
                Some(to),
                "Y at {y}%: {}",
                text(&out.stderr)
            );
            f.assert_one_place_each();
            f.assert_no_leak();
        }
    }

    /// The move back off overflow follows the rotator's capacity test, not
    /// the auto-switch threshold (issue #1606). With `threshold 80` and the
    /// default gate (85% session, 90% week), a plan profile whose session
    /// has reset but whose week reads 85% has capacity for the rotator, so
    /// the session leaves the overflow profile for it although 85% is over
    /// the auto-switch threshold. At 92% week it has none, and the session
    /// stays on the overflow profile.
    #[test]
    fn the_move_back_off_overflow_uses_the_gate_not_the_auto_switch_threshold() {
        for (y_week, to) in [(85.0, "Y"), (92.0, "Z")] {
            let f = Fixture::new();
            f.ok(&["profile", "auto-switch", "on", "--threshold", "80"]);
            f.ok(&["profile", "overflow", "add", "Z"]);
            f.seed_usage(&[("Y", 99.0), ("Z", 0.0)]);
            let _session = f.hold("X");
            let out = f.tick("X", 95.0);
            assert!(out.status.success(), "{}", text(&out.stderr));
            assert_eq!(
                f.hosts().get("X").map(String::as_str),
                Some("Z"),
                "onto overflow first: {}",
                text(&out.stderr)
            );
            // Y's session resets; its week stays at `y_week`. The swap
            // was over five minutes ago.
            f.age_swaps();
            f.seed_usage_weeks(&[("X", 95.0, 10.0), ("Y", 0.0, y_week), ("Z", 10.0, 10.0)]);
            let out = f.tick("X", 10.0);
            assert!(out.status.success(), "{}", text(&out.stderr));
            assert_eq!(
                f.hosts().get("X").map(String::as_str),
                Some(to),
                "Y at {y_week}% week: {}",
                text(&out.stderr)
            );
            if to == "Y" {
                assert!(
                    text(&out.stderr)
                        .contains("is an overflow profile and a plan profile has capacity again"),
                    "{}",
                    text(&out.stderr)
                );
            }
            f.assert_one_place_each();
            f.assert_no_leak();
        }
    }

    #[test]
    fn no_swap_below_threshold_or_when_off() {
        let f = Fixture::new();
        prime(&f);
        let _session = f.hold("X");
        f.tick("X", 50.0);
        assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));

        f.ok(&["profile", "auto-switch", "off"]);
        f.tick("X", 99.0);
        assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));
        assert!(f.hosts().is_empty());

        // Env off beats file on, on every tick (nothing cached).
        f.switch_on();
        let json = Fixture::statusline_json(99.0, 10.0);
        let mut c = f.tick_cmd("X");
        c.env(TOGGLE_ENV, "0");
        f.tick_with(c, &json);
        assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));
    }

    #[test]
    fn seven_day_window_alone_triggers_and_threshold_is_respected() {
        let f = Fixture::new();
        f.ok(&["profile", "auto-switch", "on", "--threshold", "70"]);
        f.ok(&["profile", "auto-switch", "status"]);
        let _session = f.hold("X");
        let json = Fixture::statusline_json(5.0, 70.0);
        f.tick_with(f.tick_cmd("X"), &json);
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Y"));
    }

    #[test]
    fn candidate_with_most_quota_left_is_chosen_and_exclusions_respected() {
        let f = Fixture::new();
        prime(&f);
        f.seed_usage(&[("Y", 60.0), ("Z", 20.0)]);
        let _session = f.hold("X");
        f.tick("X", 95.0);
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Z"));

        let g = Fixture::new();
        g.ok(&["profile", "auto-switch", "on", "--exclude", "Z"]);
        g.ok(&["profile", "auto-switch", "status"]);
        g.seed_usage(&[("Y", 60.0), ("Z", 20.0)]);
        let _session = g.hold("X");
        g.tick("X", 95.0);
        assert_eq!(g.hosts().get("X").map(String::as_str), Some("Y"));
    }

    #[test]
    fn expired_window_counts_as_full_quota() {
        let f = Fixture::new();
        prime(&f);
        // Y and Z both look exhausted, but Y's window has already reset.
        let state = f.state();
        fs::create_dir_all(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        private_file(
            &state.join("usage.json"),
            &serde_json::json!({"version": 1, "accounts": {
                "Y": {"five_hour": {"used_percentage": 100.0, "resets_at": now() - 60},
                      "seven_day": {"used_percentage": 5.0, "resets_at": now() + 86400},
                      "observed_at": now() - 3600, "source": "statusline"},
                "Z": {"five_hour": {"used_percentage": 100.0, "resets_at": now() + 3600},
                      "seven_day": {"used_percentage": 5.0, "resets_at": now() + 86400},
                      "observed_at": now(), "source": "statusline"},
            }})
            .to_string(),
        );
        let _session = f.hold("X");
        f.tick("X", 95.0);
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Y"));
    }

    #[test]
    fn no_eligible_candidate_means_no_swap() {
        let f = Fixture::new();
        prime(&f);
        f.seed_usage(&[("Y", 99.0), ("Z", 99.0)]);
        let _session = f.hold("X");
        f.tick("X", 95.0);
        assert!(f.hosts().is_empty());
        assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));
    }

    #[test]
    fn candidate_whose_own_directory_is_live_is_not_swapped_in() {
        let f = Fixture::new();
        prime(&f);
        let _x = f.hold("X");
        let _y = f.hold("Y");
        f.seed_usage(&[("Y", 1.0), ("Z", 50.0)]);
        f.tick("X", 95.0);
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Z"));
        assert_eq!(f.where_is("Y"), f.dir("Y").join(".credentials.json"));
    }

    #[test]
    fn busy_refresh_lock_skips_the_tick_and_is_never_broken() {
        let f = Fixture::new();
        prime(&f);
        let _session = f.hold("X");
        let lock = f.dir("X").join(".oauth_refresh.lock");
        fs::create_dir(&lock).unwrap();
        f.tick("X", 95.0);
        assert!(lock.is_dir(), "Claude's refresh lock must not be removed");
        assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));
        fs::remove_dir(&lock).unwrap();
        f.tick("X", 95.0);
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Y"));
        assert!(!lock.exists(), "our own refresh lock is released");
    }

    #[test]
    fn readings_right_after_a_swap_do_not_swap_again() {
        let f = Fixture::new();
        prime(&f);
        let _session = f.hold("X");
        f.tick("X", 95.0);
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Y"));
        // Same numbers as before the swap: still the previous account's.
        f.tick("X", 95.0);
        f.tick("X", 99.0);
        assert_eq!(
            f.hosts().get("X").map(String::as_str),
            Some("Y"),
            "cooldown"
        );
        f.assert_one_place_each();
        let usage = fs::read_to_string(f.state().join("usage.json")).unwrap();
        let usage: serde_json::Value = serde_json::from_str(&usage).unwrap();
        assert!(
            usage["accounts"]["Y"].is_null(),
            "readings after a swap must not be attributed to Y yet: {usage}"
        );
    }

    #[test]
    fn non_regular_or_hard_linked_credentials_are_never_moved() {
        let f = Fixture::new();
        prime(&f);
        let _session = f.hold("X");
        let y = f.dir("Y").join(".credentials.json");
        let link = f.home.join("extra-link");
        fs::hard_link(&y, &link).unwrap();
        f.seed_usage(&[("Y", 1.0), ("Z", 99.0)]);
        f.tick("X", 95.0);
        assert!(f.hosts().is_empty(), "nlink != 1 must refuse the move");
        assert!(y.exists());
        fs::remove_file(&link).unwrap();
    }

    // ---- leases ------------------------------------------------------------

    fn swapped() -> (Fixture, Holder) {
        let f = Fixture::new();
        prime(&f);
        let h = f.hold("X");
        f.tick("X", 95.0);
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Y"));
        (f, h)
    }

    #[test]
    fn leased_account_cannot_be_launched_or_logged_into() {
        let (f, _h) = swapped();
        for args in [
            vec![
                "claude",
                "--profile",
                "Y",
                "--subprocess-safe",
                "--",
                "-p",
                "x",
            ],
            vec!["profile", "run", "Y", "--", "-p", "x"],
            vec!["profile", "login", "Y"],
        ] {
            let out = f.run_args(&args);
            let stderr = text(&out.stderr);
            assert!(!out.status.success(), "{args:?} must be refused");
            assert!(
                stderr.contains("profile \"Y\": its login is in use by a session in profile \"X\""),
                "{args:?}: {stderr}"
            );
            assert!(
                stderr.contains("hint: wait for that session to end, or run `amplihack profile auto-switch status`"),
                "{stderr}"
            );
        }
        assert!(f.attempts().is_empty(), "claude must not be spawned");
        f.assert_one_place_each();
    }

    #[test]
    fn hosting_directory_cannot_be_launched_directly() {
        let (f, _h) = swapped();
        for args in [
            vec![
                "claude",
                "--profile",
                "X",
                "--subprocess-safe",
                "--",
                "-p",
                "x",
            ],
            vec!["profile", "login", "X"],
        ] {
            let out = f.run_args(&args);
            let stderr = text(&out.stderr);
            assert!(!out.status.success(), "{args:?} must be refused");
            assert!(
                stderr.contains(
                    "profile \"X\": its directory holds the login of profile \"Y\" for a running session"
                ),
                "{args:?}: {stderr}"
            );
        }
        // A fresh shell exporting only the profile name does not join.
        let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p", "x"]);
        c.env("AMPLIHACK_CLAUDE_PROFILE", "X");
        let out = f.run(&mut c);
        assert!(!out.status.success());
        assert!(text(&out.stderr).contains("its directory holds the login"));
        assert!(f.attempts().is_empty());
    }

    #[test]
    fn nested_launch_inside_the_hosting_session_joins_it() {
        let (f, _h) = swapped();
        let out = f.run(&mut f.nested("X"));
        assert!(out.status.success(), "{}", text(&out.stderr));
        let rec = f.record("X");
        assert!(rec.contains(&format!("CLAUDE_CONFIG_DIR={}\n", f.dir("X").display())));
        f.assert_one_place_each();
    }

    #[test]
    fn remove_is_refused_while_a_login_is_away() {
        let (f, _h) = swapped();
        for name in ["X", "Y"] {
            let out = f.run_args(&["profile", "remove", name, "--yes"]);
            assert!(!out.status.success(), "remove {name} must be refused");
            assert!(f.dir(name).is_dir());
        }
        f.ok(&["profile", "remove", "Z", "--yes"]);
    }

    #[test]
    fn lease_protection_stays_after_switch_off() {
        let (f, _h) = swapped();
        f.ok(&["profile", "auto-switch", "off"]);
        let out = f.run_args(&[
            "claude",
            "--profile",
            "Y",
            "--subprocess-safe",
            "--",
            "-p",
            "x",
        ]);
        assert!(!out.status.success());
        // `off` does not pull the login out of the running session.
        assert_eq!(f.where_is("Y"), f.dir("X").join(".credentials.json"));
    }

    // ---- session end and recovery ------------------------------------------

    fn assert_all_home(f: &Fixture) {
        for name in PROFILES {
            assert_eq!(
                f.where_is(name),
                f.dir(name).join(".credentials.json"),
                "{name} must be back home"
            );
        }
        assert!(f.hosts().is_empty(), "{:?}", f.hosts());
        assert!(!f.state().join("journal.json").exists());
        assert!(!f.parked("X").exists());
    }

    #[test]
    fn logins_return_when_the_session_ends() {
        let (f, h) = swapped();
        let y_inode = Fixture::inode(&f.dir("X").join(".credentials.json"));
        drop(h);
        f.ok(&["profile", "auto-switch", "repair"]);
        let status = f.stdout(&["profile", "auto-switch", "status"]);
        assert_all_home(&f);
        assert_eq!(
            Fixture::inode(&f.dir("Y").join(".credentials.json")),
            y_inode,
            "returned by move"
        );
        assert_eq!(f.oauth_uuid("X"), "uuid-X", "owner's oauthAccount restored");
        assert!(status.contains("invariant: ok"), "{status}");
        f.assert_no_leak();
    }

    #[test]
    fn return_handles_a_logged_out_live_login() {
        let (f, h) = swapped();
        // `/logout` inside the session deleted the borrowed login.
        fs::remove_file(f.dir("X").join(".credentials.json")).unwrap();
        drop(h);
        f.ok(&["profile", "auto-switch", "repair"]);
        assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));
        assert!(f.hosts().is_empty());
    }

    #[test]
    fn launcher_returns_logins_when_its_claude_exits() {
        let f = Fixture::new();
        f.ok(&["profile", "auto-switch", "install"]);
        prime(&f);
        let y_inode = Fixture::inode(&f.dir("Y").join(".credentials.json"));
        let wrapper = settings(&f, "X")["statusLine"]["command"]
            .as_str()
            .unwrap()
            .to_string();
        let mut c = f.cmd(&[
            "claude",
            "--profile",
            "X",
            "--subprocess-safe",
            "--",
            "-p",
            "go",
        ]);
        c.env("STATUSLINE_CMD", &wrapper)
            .env("STATUSLINE_JSON", Fixture::statusline_json(97.0, 10.0));
        let out = f.run(&mut c);
        assert!(out.status.success(), "{}", text(&out.stderr));
        let rec = f.record("X");
        assert!(
            rec.contains(&format!("CRED_INODE_AFTER={y_inode}\n")),
            "the swap happened inside the session: {rec}\nwrapper stderr: {}",
            fs::read_to_string(f.records.join("statusline.err")).unwrap_or_default()
        );
        // The launcher returned everything after claude exited, with no status call.
        assert_all_home(&f);
        f.assert_no_leak();
    }

    #[test]
    fn launcher_returns_logins_when_claude_fails() {
        let f = Fixture::new();
        f.ok(&["profile", "auto-switch", "install"]);
        prime(&f);
        let wrapper = settings(&f, "X")["statusLine"]["command"]
            .as_str()
            .unwrap()
            .to_string();
        let mut c = f.cmd(&[
            "claude",
            "--profile",
            "X",
            "--subprocess-safe",
            "--",
            "-p",
            "go",
        ]);
        c.env("STATUSLINE_CMD", &wrapper)
            .env("STATUSLINE_JSON", Fixture::statusline_json(97.0, 10.0))
            .env("EXIT_CODE", "5");
        let out = f.run(&mut c);
        assert_eq!(out.status.code(), Some(5), "exit code passes through");
        assert_all_home(&f);
    }

    fn crash_at(step: &str) -> (Fixture, Holder) {
        let f = Fixture::new();
        prime(&f);
        let h = f.hold("X");
        let json = Fixture::statusline_json(95.0, 10.0);
        let mut c = f.tick_cmd("X");
        c.env(ABORT_ENV, step);
        let out = f.tick_with(c, &json);
        assert!(
            !out.status.success(),
            "the debug abort hook must stop the wrapper at {step}"
        );
        f.assert_one_place_each();
        (f, h)
    }

    #[test]
    fn crash_after_journal_rolls_back() {
        let (f, h) = crash_at("J0");
        assert!(f.state().join("journal.json").exists());
        f.ok(&["profile", "auto-switch", "repair"]);
        assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));
        assert!(f.hosts().is_empty());
        drop(h);
        f.ok(&["profile", "auto-switch", "repair"]);
        assert_all_home(&f);
    }

    #[test]
    fn crash_after_first_move_rolls_back() {
        let (f, h) = crash_at("S1");
        assert!(!f.dir("X").join(".credentials.json").exists());
        f.ok(&["profile", "auto-switch", "repair"]);
        assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));
        assert_eq!(f.where_is("Y"), f.dir("Y").join(".credentials.json"));
        assert!(f.hosts().is_empty());
        drop(h);
    }

    #[test]
    fn crash_after_second_move_rolls_forward() {
        let (f, h) = crash_at("S2");
        f.ok(&["profile", "auto-switch", "repair"]);
        assert_eq!(f.where_is("Y"), f.dir("X").join(".credentials.json"));
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Y"));
        assert!(!f.state().join("journal.json").exists());
        drop(h);
        f.ok(&["profile", "auto-switch", "repair"]);
        assert_all_home(&f);
    }

    #[test]
    fn crash_after_ledger_update_rolls_forward() {
        let (f, h) = crash_at("S3");
        f.ok(&["profile", "auto-switch", "repair"]);
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Y"));
        assert!(!f.state().join("journal.json").exists());
        drop(h);
        f.ok(&["profile", "auto-switch", "repair"]);
        assert_all_home(&f);
    }

    #[test]
    fn recovery_runs_at_the_next_launch_too() {
        let (f, h) = crash_at("S2");
        drop(h);
        let out = f.run_args(&[
            "claude",
            "--profile",
            "Y",
            "--subprocess-safe",
            "--",
            "-p",
            "x",
        ]);
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert_all_home(&f);
        assert_eq!(f.attempts(), vec!["Y".to_string()]);
    }

    #[test]
    fn damaged_ledger_fails_closed_and_is_not_overwritten() {
        let (f, h) = swapped();
        drop(h);
        let ledger = f.state().join("ledger.json");
        private_file(&ledger, "{\"version\":2,\"hosts\":{}}");
        let out = f.run_args(&[
            "claude",
            "--profile",
            "X",
            "--subprocess-safe",
            "--",
            "-p",
            "x",
        ]);
        assert!(!out.status.success());
        assert!(
            text(&out.stderr).contains("Recovery"),
            "{}",
            text(&out.stderr)
        );
        assert_eq!(
            fs::read_to_string(&ledger).unwrap(),
            "{\"version\":2,\"hosts\":{}}"
        );
        assert!(f.attempts().is_empty());
        f.assert_one_place_each();
    }

    #[test]
    fn concurrent_wrappers_never_lose_or_duplicate_a_login() {
        let f = Fixture::new();
        prime(&f);
        let _x = f.hold("X");
        let json = Fixture::statusline_json(95.0, 10.0);
        let children: Vec<Child> = (0..8)
            .map(|_| {
                let mut c = f.tick_cmd("X");
                let mut child = c.spawn().unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(json.as_bytes())
                    .unwrap();
                child
            })
            .collect();
        for c in children {
            let out = c.wait_with_output().unwrap();
            f.printed.borrow_mut().extend_from_slice(&out.stderr);
        }
        f.assert_one_place_each();
        let hosts = f.hosts();
        assert_eq!(hosts.len(), 1, "{hosts:?}");
        let status = f.stdout(&["profile", "auto-switch", "status"]);
        assert!(status.contains("invariant: ok"), "{status}");
        f.assert_no_leak();
    }

    #[test]
    fn concurrent_sessions_never_lease_one_account_twice() {
        let f = Fixture::new();
        prime(&f);
        let _x = f.hold("X");
        let _z = f.hold("Z");
        // Only Y is eligible for both; exactly one session may get it.
        let jx = Fixture::statusline_json(95.0, 10.0);
        let mut a = f.tick_cmd("X").spawn().unwrap();
        let mut b = f.tick_cmd("Z").spawn().unwrap();
        a.stdin.take().unwrap().write_all(jx.as_bytes()).unwrap();
        b.stdin.take().unwrap().write_all(jx.as_bytes()).unwrap();
        a.wait().unwrap();
        b.wait().unwrap();
        f.assert_one_place_each();
        let hosts = f.hosts();
        assert_eq!(
            hosts.values().collect::<Vec<_>>(),
            vec!["Y"],
            "only one session may lease Y: {hosts:?}"
        );
    }

    // ---- read-only status, repair and notes ---------------------------------

    /// Bytes, mode and inode of every path under `dirs`, without following
    /// symlinks.
    fn tree(dirs: &[PathBuf]) -> BTreeMap<PathBuf, (u32, u64, Vec<u8>)> {
        fn walk(p: &Path, out: &mut BTreeMap<PathBuf, (u32, u64, Vec<u8>)>) {
            let Ok(m) = fs::symlink_metadata(p) else {
                return;
            };
            let bytes = if m.is_file() {
                fs::read(p).unwrap()
            } else {
                Vec::new()
            };
            out.insert(p.to_path_buf(), (m.mode(), m.ino(), bytes));
            if m.is_dir() {
                for e in fs::read_dir(p).unwrap().flatten() {
                    walk(&e.path(), out);
                }
            }
        }
        let mut out = BTreeMap::new();
        for d in dirs {
            walk(d, &mut out);
        }
        out
    }

    #[test]
    fn status_is_read_only() {
        // A swap stopped after the ledger update: the journal is pending, a
        // login is in another directory and the switch is on.
        let (f, h) = crash_at("S3");
        f.seed_usage(&[("Z", 40.0)]);
        let dot_claude = f.home.join(".claude");
        fs::create_dir_all(&dot_claude).unwrap();
        fs::write(dot_claude.join("settings.json"), "{}").unwrap();
        fs::write(f.home.join(".claude.json"), "{}").unwrap();
        let lock = f.state().join("lock");
        let _ = fs::remove_file(&lock);
        let dirs = vec![f.root.clone(), dot_claude, f.home.join(".claude.json")];
        let before = tree(&dirs);

        let status = f.stdout(&["profile", "auto-switch", "status"]);

        assert_eq!(before, tree(&dirs), "status changed something on disk");
        assert!(!lock.exists(), "status must not take the lock");
        assert!(status.contains("invariant: pending recovery\n"), "{status}");
        assert!(
            status.contains(
                "note: interrupted swap pending — run: amplihack profile auto-switch repair\n"
            ),
            "{status}"
        );
        assert!(f.state().join("journal.json").exists());
        drop(h);
        f.assert_no_leak();
    }

    #[test]
    fn repair_runs_recovery() {
        let (f, h) = crash_at("S1");
        let out = f.stdout(&["profile", "auto-switch", "repair"]);
        assert!(
            out.contains("recovery: rolled back an interrupted swap in X\n"),
            "{out}"
        );
        assert!(!f.state().join("journal.json").exists());
        assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));
        drop(h);
        let out = f.stdout(&["profile", "auto-switch", "repair"]);
        assert!(out.ends_with("recovery: nothing to do\n"), "{out}");

        let (g, h) = crash_at("S2");
        let out = g.stdout(&["profile", "auto-switch", "repair"]);
        assert!(
            out.contains("recovery: finished an interrupted swap in X\n"),
            "{out}"
        );
        drop(h);
        let out = g.stdout(&["profile", "auto-switch", "repair"]);
        assert!(
            out.contains("recovery: returned the login of Y hosted in X (session ended)\n"),
            "{out}"
        );
        assert_all_home(&g);
    }

    #[test]
    fn stale_lease_is_a_note_and_status_leaves_it() {
        let (f, h) = swapped();
        drop(h);
        let status = f.stdout(&["profile", "auto-switch", "status"]);
        assert!(
            status.contains(
                "note: stale lease: X (session ended) — run: amplihack profile auto-switch repair\n"
            ),
            "{status}"
        );
        assert!(status.contains("invariant: ok"), "{status}");
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Y"));
        f.ok(&["profile", "auto-switch", "repair"]);
        let status = f.stdout(&["profile", "auto-switch", "status"]);
        assert!(!status.contains("stale lease"), "{status}");
    }

    #[test]
    fn oauth_patch_keeps_the_claude_json_mode() {
        let f = Fixture::new();
        let cj = f.dir("X").join(".claude.json");
        fs::set_permissions(&cj, fs::Permissions::from_mode(0o644)).unwrap();
        prime(&f);
        let _h = f.hold("X");
        f.tick("X", 95.0);
        assert_eq!(f.hosts().get("X").map(String::as_str), Some("Y"));
        assert_eq!(f.oauth_uuid("X"), "uuid-Y", "patched");
        assert_eq!(fs::metadata(&cj).unwrap().mode() & 0o7777, 0o644);
    }
}

// ---------------------------------------------------------------------------
// install, repair and status notes
// ---------------------------------------------------------------------------

#[test]
fn install_twice_is_byte_identical() {
    let f = Fixture::new();
    write_settings(
        &f,
        "Y",
        &serde_json::json!({"statusLine": {"type": "command", "command": "echo mine"}}),
    );
    f.ok(&["profile", "auto-switch", "install"]);
    let files = |f: &Fixture| -> Vec<(Vec<u8>, u64, u32)> {
        PROFILES
            .iter()
            .map(|n| {
                let p = f.dir(n).join("settings.json");
                let m = fs::metadata(&p).unwrap();
                (fs::read(&p).unwrap(), m.ino(), m.mode())
            })
            .collect()
    };
    let before = files(&f);
    let out = f.stdout(&["profile", "auto-switch", "install"]);
    assert_eq!(before, files(&f));
    for n in PROFILES {
        assert!(out.contains(&format!("unchanged  {n}\n")), "{out}");
    }
}

#[test]
fn repair_repoints_stale_wrapper() {
    let f = Fixture::new();
    let old = "'/old/amplihack' profile statusline --root '/old' --owner 'Z'";
    write_settings(
        &f,
        "Z",
        &serde_json::json!({"statusLine": {"type": "command", "command": old, "padding": 1}}),
    );
    let out = f.stdout(&["profile", "auto-switch", "install"]);
    assert!(
        out.contains(
            "stale      Z (wrapper pointed at a different binary) — run: amplihack profile auto-switch repair\n"
        ),
        "{out}"
    );
    assert_eq!(
        settings(&f, "Z")["statusLine"]["command"],
        old,
        "install leaves it"
    );
    let status = f.stdout(&["profile", "auto-switch", "status"]);
    assert!(
        status.contains(
            "note: profiles without the wrapper: Z — run: amplihack profile auto-switch install\n"
        ),
        "{status}"
    );

    let out = f.stdout(&["profile", "auto-switch", "repair"]);
    assert!(
        out.contains("repaired   Z (wrapper pointed at a different binary)\n"),
        "{out}"
    );
    assert!(out.ends_with("recovery: nothing to do\n"), "{out}");
    let z = settings(&f, "Z");
    assert_eq!(z["statusLine"]["command"], canonical_wrapper(&f, "Z"));
    assert_eq!(z["statusLine"]["padding"], 1);
    assert!(
        !f.state().join("statusline").join("Z.json").exists(),
        "a wrapper is never saved as the original"
    );
}

#[test]
fn on_and_status_name_profiles_without_the_wrapper() {
    let f = Fixture::new();
    let out = f.ok(&["profile", "auto-switch", "on"]);
    let note = "note: profiles without the wrapper: X, Y, Z — run: amplihack profile auto-switch install\n";
    assert!(text(&out.stderr).contains(note), "{}", text(&out.stderr));
    assert!(
        f.stdout(&["profile", "auto-switch", "status"])
            .contains(note)
    );
    f.ok(&["profile", "auto-switch", "install"]);
    let out = f.ok(&["profile", "auto-switch", "on"]);
    assert!(!text(&out.stderr).contains("without the wrapper"));
    assert!(
        !f.stdout(&["profile", "auto-switch", "status"])
            .contains("without the wrapper")
    );
}

#[test]
fn excluded_but_missing_is_a_note() {
    let f = Fixture::new();
    f.ok(&["profile", "auto-switch", "exclude", "Y", "nope", "gone"]);
    let status = f.stdout(&["profile", "auto-switch", "status"]);
    assert!(
        status.contains("note: excluded but missing: nope, gone\n")
            || status.contains("note: excluded but missing: gone, nope\n"),
        "{status}"
    );
    assert!(!status.contains("missing: Y"), "{status}");
}

#[test]
fn shared_account_is_a_note_without_the_uuid() {
    let f = Fixture::new();
    private_file(
        &f.dir("Z").join(".claude.json"),
        "{\"oauthAccount\":{\"accountUuid\":\"uuid-X\",\"organizationUuid\":\"org-X\",\"emailAddress\":\"x@example.invalid\"}}",
    );
    let status = f.stdout(&["profile", "auto-switch", "status"]);
    assert!(
        status.contains(
            "note: profiles 'X' and 'Z' are signed in to the same account and organization \
             (duplicate login); fix: run 'amplihack profile login Z' with the browser signed \
             out of claude.ai or in a private window.\n"
        ),
        "{status}"
    );
    assert!(!status.contains("readings are not reliable"), "{status}");
    assert!(
        !status.contains("uuid-") && !status.contains("org-"),
        "{status}"
    );
    assert!(!status.contains("profiles 'X' and 'Y'"), "{status}");
    f.assert_no_leak();
}

#[test]
fn one_user_in_two_organizations_is_not_a_shared_account() {
    // Bug 1: the same accountUuid in another organization is another
    // subscription with its own limits.
    let f = Fixture::new();
    for (p, org) in [("X", "org-team"), ("Z", "org-personal")] {
        private_file(
            &f.dir(p).join(".claude.json"),
            &format!(
                "{{\"oauthAccount\":{{\"accountUuid\":\"uuid-X\",\"organizationUuid\":\"{org}\",\"emailAddress\":\"x@example.invalid\"}}}}"
            ),
        );
    }
    let status = f.stdout(&["profile", "auto-switch", "status"]);
    assert!(!status.contains("same account"), "{status}");
    assert!(!status.contains("org-"), "{status}");
    f.assert_no_leak();
}

#[test]
fn status_escapes_env_and_exclude() {
    let f = Fixture::new();
    f.ok(&["profile", "auto-switch", "exclude", "Y"]);
    let mut c = f.cmd(&["profile", "auto-switch", "status"]);
    c.env(TOGGLE_ENV, "\u{1b}[31mEVIL\u{7}");
    let out = f.run(&mut c);
    assert!(out.status.success());
    let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(
        all.contains(
            "note: ignored AMPLIHACK_CLAUDE_AUTO_SWITCH value (use 1 or 0); the file decides\n"
        ),
        "{all}"
    );
    assert!(!all.contains('\u{1b}') && !all.contains("EVIL"), "{all:?}");
    // A hand-edited exclusion that is not a valid name makes the file corrupt;
    // it is never printed.
    fs::write(
        f.root.join(".auto-switch"),
        "enabled=on\nexclude=\u{1b}[2Jx\n",
    )
    .unwrap();
    let status = f.stdout(&["profile", "auto-switch", "status"]);
    assert!(status.contains("switch: off (corrupt)"), "{status}");
    assert!(!status.contains('\u{1b}'), "{status:?}");
}

#[test]
fn tampered_saved_statusline_not_executed() {
    let f = Fixture::new();
    write_settings(
        &f,
        "Y",
        &serde_json::json!({"statusLine": {"type": "command", "command": "echo mine"}}),
    );
    f.ok(&["profile", "auto-switch", "install"]);
    let saved = f.state().join("statusline").join("Y.json");
    let pwned = f.records.join("pwned");
    let evil = format!(
        "{{\"type\":\"command\",\"command\":\"touch {}\"}}",
        pwned.display()
    );
    let decoy = f.records.join("decoy.json");
    // Group-readable, then a symlink to a file the user owns.
    for tamper in ["mode", "symlink"] {
        let _ = fs::remove_file(&saved);
        if tamper == "mode" {
            fs::write(&saved, &evil).unwrap();
            fs::set_permissions(&saved, fs::Permissions::from_mode(0o644)).unwrap();
        } else {
            private_file(&decoy, &evil);
            std::os::unix::fs::symlink(&decoy, &saved).unwrap();
        }
        let out = f.tick_with(f.tick_cmd("Y"), &Fixture::statusline_json(10.0, 10.0));
        assert!(!pwned.exists(), "{tamper}: tampered command ran");
        assert_eq!(text(&out.stdout), "", "{tamper}");
        assert!(text(&out.stderr).contains("auto-switch repair"), "{tamper}");
        assert_eq!(out.status.code(), Some(0));
    }
}

#[test]
fn statusline_no_network_no_notice() {
    let f = Fixture::new();
    write_settings(
        &f,
        "Y",
        &serde_json::json!({"statusLine": {"type": "command", "command": "echo mine"}}),
    );
    f.ok(&["profile", "auto-switch", "install"]);
    f.switch_on();
    let out = f.tick_with(f.tick_cmd("Y"), &Fixture::statusline_json(10.0, 10.0));
    assert_eq!(text(&out.stdout), "mine\n", "only the original's output");
    assert_eq!(text(&out.stderr), "", "no notice of its own");
    assert_eq!(out.status.code(), Some(0));
}

// ---------------------------------------------------------------------------
// Between processes: nested launches and usage-limit reruns
// ---------------------------------------------------------------------------

#[test]
fn nested_launch_moves_to_a_candidate_when_inherited_profile_is_exhausted() {
    let f = Fixture::new();
    f.switch_on();
    f.seed_usage(&[("X", 96.0), ("Y", 40.0), ("Z", 10.0)]);
    let inodes: Vec<u64> = PROFILES
        .iter()
        .map(|n| Fixture::inode(&f.dir(n).join(".credentials.json")))
        .collect();
    let out = f.run(&mut f.nested("X"));
    assert!(out.status.success(), "{}", text(&out.stderr));
    let rec = f.record("Z");
    assert!(
        rec.contains(&format!("CLAUDE_CONFIG_DIR={}\n", f.dir("Z").display())),
        "{rec}"
    );
    assert!(rec.contains("AMPLIHACK_CLAUDE_PROFILE=Z\n"), "{rec}");
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(
            "amplihack: profile \"X\" is at 96% of its five-hour limit; auto-switch is using profile \"Z\" for this launch\n"
        ),
        "the documented note, naming both profiles: {stderr}"
    );
    // Nested launches never move logins.
    let after: Vec<u64> = PROFILES
        .iter()
        .map(|n| Fixture::inode(&f.dir(n).join(".credentials.json")))
        .collect();
    assert_eq!(inodes, after);
    f.assert_no_leak();
}

#[test]
fn explicit_profile_flag_is_never_redirected() {
    let f = Fixture::new();
    f.switch_on();
    f.seed_usage(&[("X", 96.0)]);
    let mut c = f.cmd(&[
        "claude",
        "--profile",
        "X",
        "--subprocess-safe",
        "--",
        "-p",
        "x",
    ]);
    c.env("AMPLIHACK_CLAUDE_PROFILE", "Y");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(f.attempts(), vec!["X".to_string()]);
}

#[test]
fn default_pointer_is_never_redirected() {
    let f = Fixture::new();
    f.switch_on();
    set_default(&f.root, "X");
    f.seed_usage(&[("X", 96.0)]);
    let out = f.run_args(&["claude", "--subprocess-safe", "--", "-p", "x"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(f.attempts(), vec!["X".to_string()]);
}

#[test]
fn nested_launch_keeps_inherited_profile_when_off() {
    let f = Fixture::new();
    f.seed_usage(&[("X", 99.0)]);
    let out = f.run(&mut f.nested("X"));
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(f.attempts(), vec!["X".to_string()]);
}

#[test]
fn usage_limited_step_reruns_under_the_next_candidate() {
    let f = Fixture::new();
    f.switch_on();
    let mut c = f.nested("X");
    c.env("LIMITED", "X");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(f.attempts(), vec!["X".to_string(), "Y".to_string()]);
    assert_eq!(
        claude_stdout(&out),
        "done as Y\n",
        "only the final attempt's stdout is replayed"
    );
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("working..."),
        "stderr streams live: {stderr}"
    );
    let usage: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.state().join("usage.json")).unwrap()).unwrap();
    assert_eq!(usage["accounts"]["X"]["source"], "usage_limit_failure");
    assert_eq!(
        usage["accounts"]["X"]["five_hour"]["used_percentage"].as_f64(),
        Some(100.0)
    );
    let rec = f.record("Y");
    assert!(
        rec.contains(&format!("{SUPERVISED_ENV}=<unset>\n")),
        "{rec}"
    );
    f.assert_no_leak();
}

#[test]
fn each_candidate_is_tried_once_then_the_last_failure_is_passed_back() {
    let f = Fixture::new();
    f.switch_on();
    let mut c = f.nested("X");
    c.env("LIMITED", "X Y Z");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(f.attempts(), vec!["X", "Y", "Z"]);
    assert_eq!(claude_stdout(&out), "partial output from Z\n");
    let stderr = text(&out.stderr);
    // The wait, disabled here, writes one `skipped` line after the limit.
    let last = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty() && !l.starts_with("amplihack.recipe.usage_limit_wait "))
        .unwrap();
    assert!(
        last.starts_with(LIMIT_MARKER),
        "the runner's classifier must still see the limit: {stderr}"
    );
}

/// A nested `-p` step with the switch on whose fake `claude` prints `inject`
/// to stderr and exits `code`.
fn injected(inject: &str, code: i32) -> (Fixture, Output) {
    let f = Fixture::new();
    f.switch_on();
    let mut c = f.nested("X");
    c.env("INJECT", inject).env("EXIT_CODE", code.to_string());
    let out = f.run(&mut c);
    (f, out)
}

#[test]
fn marker_followed_by_another_stderr_line_reruns() {
    // The runner's classifier sees a usage limit here, so the supervisor must
    // too (D2): the marker need not be the last line. Every profile fails the
    // same way, so each is tried once.
    let (f, out) = injected(
        &format!("{LIMIT_MARKER}|{}\nError: request failed", now() + 600),
        1,
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(f.attempts(), vec!["X", "Y", "Z"]);
    assert!(
        text(&out.stderr).contains(
            "amplihack: profile \"X\" hit its usage limit; rerunning this step under profile \"Y\" (auto-switch)"
        ),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn marker_with_exit_zero_never_reruns() {
    let (f, out) = injected(&format!("model said: {LIMIT_MARKER}|1"), 0);
    assert!(out.status.success());
    assert_eq!(f.attempts(), vec!["X".to_string()]);
    assert!(!f.state().join("usage.json").exists(), "nothing recorded");
}

#[test]
fn marker_with_a_work_failure_never_reruns() {
    let (f, out) = injected(
        &format!("{LIMIT_MARKER}|1\ntest result: FAILED. 3 passed; 1 failed"),
        1,
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(f.attempts(), vec!["X".to_string()]);
}

#[test]
fn attempt_killed_by_a_signal_never_reruns() {
    let f = Fixture::new();
    f.switch_on();
    let mut c = f.nested("X");
    c.env("KILL_PARENT", "1");
    let out = f.run(&mut c);
    assert!(!out.status.success());
    assert_eq!(f.attempts(), vec!["X".to_string()]);
}

/// A stopped step must not be rerun under another account (review 17b #1):
/// the supervisor keeps its stop-signal handlers for the whole run and checks
/// them before starting another attempt.
#[test]
#[cfg(target_os = "linux")]
fn stopped_supervisor_never_reruns_a_usage_limited_step() {
    let f = Fixture::new();
    f.switch_on();
    let mut c = f.nested("X");
    c.env("INT_SUPERVISOR", "1");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(130), "{}", text(&out.stderr));
    assert_eq!(f.attempts(), vec!["X".to_string()]);
    assert!(
        !text(&out.stderr).contains("rerunning this step"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn usage_limit_with_switch_off_is_passed_straight_back() {
    let f = Fixture::new();
    let mut c = f.nested("X");
    c.env("LIMITED", "X");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(f.attempts(), vec!["X".to_string()]);
    assert!(
        !f.state().exists(),
        "off must not create the state directory"
    );
}

#[test]
fn top_level_launch_is_not_supervised() {
    let f = Fixture::new();
    f.switch_on();
    let mut c = f.cmd(&[
        "claude",
        "--profile",
        "X",
        "--subprocess-safe",
        "--",
        "-p",
        "x",
    ]);
    c.env("LIMITED", "X");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(f.attempts(), vec!["X".to_string()]);
}

// ---------------------------------------------------------------------------
// OFF means no change
// ---------------------------------------------------------------------------

#[test]
fn off_changes_nothing() {
    let f = Fixture::new();
    // A profile carrying the wrapper exactly as `install` writes it, with no
    // saved original and no state directory: the strict "off" case.
    write_settings(
        &f,
        "Y",
        &serde_json::json!({"statusLine": {"type": "command",
            "command": canonical_wrapper(&f, "Y")}}),
    );
    let wrapper = canonical_wrapper(&f, "Y");
    let before = f.snapshot();

    f.ok(&["profile", "list"]);
    f.ok(&["profile", "auto-switch", "status"]);
    let out = f.run(&mut f.nested("X"));
    assert!(out.status.success());
    assert_eq!(claude_stdout(&out), "done as X\n");
    let mut c = f.cmd(&[
        "claude",
        "--profile",
        "Y",
        "--subprocess-safe",
        "--",
        "-p",
        "x",
    ]);
    c.env("STATUSLINE_CMD", &wrapper)
        .env("STATUSLINE_JSON", Fixture::statusline_json(99.0, 99.0))
        .env("EXIT_CODE", "4");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(4), "exit code passes through");
    assert_eq!(fs::read(f.records.join("statusline.out")).unwrap(), b"");
    let wrapper_err = fs::read_to_string(f.records.join("statusline.err")).unwrap();
    assert!(wrapper_err.is_empty(), "{wrapper_err}");
    let mut c = f.nested("X");
    c.env("LIMITED", "X");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1));

    assert!(!f.state().exists(), "off must not create .auto-switch.d");
    assert_eq!(
        before,
        f.snapshot(),
        "off must not change anything under the root"
    );
    assert_eq!(
        f.attempts(),
        vec!["X", "Y", "X"],
        "no substitution, no rerun"
    );
    for p in ["X", "Y"] {
        let rec = f.record(p);
        assert!(
            rec.contains(&format!("{SUPERVISED_ENV}=<unset>\n")),
            "{rec}"
        );
        assert!(
            rec.contains(&format!("AMPLIHACK_CLAUDE_PROFILE={p}\n")),
            "{rec}"
        );
    }
}

#[test]
fn wrapper_while_off_runs_the_original_and_records_nothing() {
    let f = Fixture::new();
    write_settings(
        &f,
        "X",
        &serde_json::json!({"statusLine": {"type": "command", "command": "echo inner"}}),
    );
    f.ok(&["profile", "auto-switch", "install"]);
    let out = f.tick("X", 99.0);
    assert_eq!(text(&out.stdout), "inner\n");
    assert!(!f.state().join("usage.json").exists());
    assert!(!f.state().join("ledger.json").exists());
    assert_eq!(f.where_is("X"), f.dir("X").join(".credentials.json"));
}

// ---------------------------------------------------------------------------
// Source-level guards
// ---------------------------------------------------------------------------

fn auto_switch_sources() -> Vec<(PathBuf, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/auto_switch");
    assert!(dir.is_dir(), "{} must exist", dir.display());
    let mut out = Vec::new();
    for e in fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if name.ends_with(".rs") && !name.starts_with("tests_") {
            out.push((p.clone(), fs::read_to_string(&p).unwrap()));
        }
    }
    assert!(
        out.len() >= 5,
        "expected the auto_switch modules, found {}",
        out.len()
    );
    out
}

/// Names through which the auto-switch code reaches a login file.
const CRED_NAMES: [&str; 6] = [
    "credentials",
    "CREDS",
    "live_creds",
    "home_slot",
    "parked_dir",
    "slot(",
];

/// Operations that open, read, copy or write file contents.
const CONTENT_OPS: [&str; 10] = [
    "File::open",
    "OpenOptions",
    "fs::read",
    "fs::copy",
    "fs::write",
    "read_to_",
    "read_bounded",
    "write_private_atomic",
    "write_atomic",
    "read_sub_json",
];

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn mentions_word(code: &str, word: &str) -> bool {
    code.match_indices(word).any(|(i, _)| {
        let before = code[..i].chars().next_back();
        let after = code[i + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Lines of `src` that reach a login file and also open, read, copy or write
/// file contents. A variable bound (`let x = …`) to a login path counts as a
/// login name for the rest of the file.
fn credential_content_access(src: &str) -> Vec<(usize, String)> {
    let code: Vec<&str> = src.lines().map(|l| l.split("//").next().unwrap()).collect();
    let mut tainted: Vec<String> = Vec::new();
    for line in &code {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("let ")
            && CRED_NAMES.iter().any(|n| line.contains(n))
        {
            let name: String = rest
                .trim_start_matches("mut ")
                .chars()
                .take_while(|&c| is_ident(c))
                .collect();
            if !name.is_empty() {
                tainted.push(name);
            }
        }
    }
    code.iter()
        .enumerate()
        .filter(|(_, line)| {
            CRED_NAMES.iter().any(|n| line.contains(n))
                || tainted.iter().any(|t| mentions_word(line, t))
        })
        .filter(|(_, line)| CONTENT_OPS.iter().any(|op| line.contains(op)))
        .map(|(i, line)| (i + 1, line.to_string()))
        .collect()
}

#[test]
fn auto_switch_never_opens_reads_or_copies_credentials() {
    // Login files are only renamed (`move_login`) and looked at through
    // metadata (`fsx::exists`, `fsx::movable_login`, `touch_nofollow`).
    let mut seen = 0;
    for (path, src) in auto_switch_sources() {
        seen += src
            .lines()
            .filter(|l| CRED_NAMES.iter().any(|n| l.contains(n)))
            .count();
        let hits = credential_content_access(&src);
        assert!(
            hits.is_empty(),
            "{}: login files must only be renamed: {hits:?}",
            path.display()
        );
    }
    assert!(seen > 10, "the scan must see the login paths ({seen})");
}

#[test]
fn credential_scan_catches_reads_through_names_and_variables() {
    for bad in [
        "let raw = fs::read(state.live_creds(n))?;",
        "let f = File::open(state.parked_dir(n).join(CREDS))?;",
        "let live = state.live_creds(&j.dir);\nlet raw = fsx::read_bounded_nofollow(&live, 10);",
        "let mut home = state.home_slot(&l, n);\nfs::copy(&home, \"/tmp/x\")?;",
        "let s = slot(state, l, d, a);\nlet t = std::fs::read_to_string(&s);",
    ] {
        assert!(
            !credential_content_access(bad).is_empty(),
            "not caught: {bad}"
        );
    }
    for ok in [
        "let live = state.live_creds(&j.dir);\nif fsx::exists(&live) {}",
        "move_login(state, &live, &home)?;",
        "let livelier = fs::read(other)?;",
    ] {
        assert!(credential_content_access(ok).is_empty(), "false hit: {ok}");
    }
}

#[test]
fn auto_switch_makes_no_network_calls() {
    for (path, src) in auto_switch_sources() {
        for banned in [
            "reqwest",
            "ureq",
            "hyper",
            "TcpStream",
            "UdpSocket",
            "std::net",
        ] {
            assert!(!src.contains(banned), "{} uses {banned}", path.display());
        }
    }
}

#[test]
fn auto_switch_uses_only_bounded_reads() {
    for (path, src) in auto_switch_sources() {
        for banned in [
            "fs::read(",
            "fs::read_to_string(",
            "read_to_end(&mut",
            ".read_to_string(",
        ] {
            let allowed = path.ends_with("fsx.rs") && banned == "read_to_end(&mut";
            assert!(
                allowed || !src.contains(banned),
                "{}: use fsx::read_bounded_nofollow, not {banned}",
                path.display()
            );
        }
    }
}

#[test]
fn test_abort_hook_is_debug_only() {
    let mut found = false;
    for (path, src) in auto_switch_sources() {
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.contains("TEST_ABORT") && !line.trim_start().starts_with("//") {
                found = true;
                let window = lines[i.saturating_sub(3)..=i].join("\n");
                assert!(
                    window.contains("cfg(debug_assertions)"),
                    "{}:{}: the crash hook must be gated on debug_assertions",
                    path.display(),
                    i + 1
                );
            }
        }
    }
    assert!(found, "the debug crash hook must exist");
}

#[test]
fn every_sentinel_survives_a_full_flow_without_being_printed() {
    let f = Fixture::new();
    write_settings(
        &f,
        "X",
        &serde_json::json!({"env": {"TOKEN": "SENTINEL-SETTINGS-ENV"}}),
    );
    f.ok(&["profile", "auto-switch", "install"]);
    f.switch_on();
    f.ok(&["profile", "auto-switch", "status"]);
    f.tick("X", 95.0);
    f.ok(&["profile", "auto-switch", "status"]);
    let mut c = f.nested("X");
    c.env("LIMITED", "X Y");
    f.run(&mut c);
    f.ok(&["profile", "auto-switch", "status"]);
    f.assert_one_place_each();
    f.assert_no_leak();
}

/// Rotator autostart opt-out: with auto-switch on and
/// `AMPLIHACK_ROTATE_AUTOSTART=0` (set by every fixture command), a launch
/// behaves as before and starts no rotator.
///
/// The launch under test runs without `AMPLIHACK_TEST_HOME_GUARD`. With the
/// guard set, the binary returns before it reads the opt-out (#1690), so
/// this test would pass even if the opt-out were broken. Without it, the
/// opt-out is the only thing between this launch and a real `profile rotate
/// watch --spawned`. If it ever breaks, the test fails on `rotate.pid` and
/// `KillRotator` kills the daemon it started, so nothing outlives the test.
#[cfg(unix)]
#[test]
fn launch_with_autostart_off_starts_no_rotator() {
    /// Kills the rotator named by `rotate.pid`, if a broken opt-out started one.
    struct KillRotator(PathBuf);
    impl Drop for KillRotator {
        fn drop(&mut self) {
            if let Ok(s) = fs::read_to_string(&self.0)
                && let Ok(pid) = s.trim().parse::<i32>()
                && pid > 1
            {
                let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
                if String::from_utf8_lossy(&cmdline).contains("rotate") {
                    // SAFETY: plain kill(2) of a process this test started.
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
        }
    }

    let f = Fixture::new();
    let rotate = f.state().join("rotate");
    let _reap = KillRotator(rotate.join("rotate.pid"));
    // As root the launch skips autostart before the opt-out is read, so the
    // opt-out would not be what this test checks.
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skip: running as root, autostart stops before the opt-out");
        return;
    }
    let launch = || {
        let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p", "x"]);
        c.env_remove(crate::isolated_amplihack::HOME_GUARD_ENV);
        f.run(&mut c)
    };
    let off = launch();
    assert!(off.status.success(), "{}", text(&off.stderr));
    f.switch_on();
    let out = launch();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(out.status.code(), off.status.code());
    assert_eq!(claude_stdout(&out), claude_stdout(&off), "stdout unchanged");
    assert!(
        !text(&out.stderr).contains("started Claude login rotator"),
        "{}",
        text(&out.stderr)
    );
    assert!(!rotate.join("rotate.pid").exists(), "no rotator started");
    assert!(
        !rotate.join("autostart").exists(),
        "the setting is not written"
    );
}

/// `auto-switch status` shows the autostart state right after `switch:`, and
/// marks it inactive while auto-switch is off.
#[cfg(unix)]
#[test]
fn auto_switch_status_shows_autostart() {
    let f = Fixture::new();
    let lines = |s: String| s.lines().map(str::to_string).collect::<Vec<_>>();
    let off = lines(f.stdout(&["profile", "auto-switch", "status"]));
    let i = off.iter().position(|l| l.starts_with("switch:")).unwrap();
    assert_eq!(
        off.get(i + 1).map(String::as_str),
        Some("autostart: off (env) — inactive (auto-switch off)"),
        "{off:?}"
    );
    f.switch_on();
    let on = lines(f.stdout(&["profile", "auto-switch", "status"]));
    let i = on.iter().position(|l| l.starts_with("switch:")).unwrap();
    assert_eq!(
        on.get(i + 1).map(String::as_str),
        Some("autostart: off (env)"),
        "{on:?}"
    );
    let mut c = f.cmd(&["profile", "auto-switch", "status"]);
    c.env_remove("AMPLIHACK_ROTATE_AUTOSTART");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("\nautostart: on (default)\n"),
        "{}",
        text(&out.stdout)
    );
}
