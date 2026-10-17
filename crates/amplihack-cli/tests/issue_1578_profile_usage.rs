//! Issue #1578: `amplihack profile usage` shows each profile's session and
//! weekly usage, orders rows by capacity, marks the default and leases, and
//! refreshes auto-switch's `usage.json`.
//!
//! Runs the real `amplihack` binary with an isolated `HOME` and profile root.
//! A fake `claude` (via `AMPLIHACK_CLAUDE_BINARY_PATH`) answers `--version`,
//! records its arguments and `CLAUDE_CONFIG_DIR`, and prints canned `/usage`
//! output chosen by the profile directory's name (`.claude` when the
//! variable is unset, as for a login live in `~/.claude`). Reset texts are generated
//! here, relative to the real clock, in zone `UTC`, so nothing depends on the
//! host's zone, the year, or GNU `date`.

#![cfg(unix)]

use chrono::{Datelike, Timelike};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// See `issue_1567_nested_profile.rs`: the binary lives in `bins/amplihack`.
fn amplihack_bin() -> PathBuf {
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

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn private_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// `Oct 16, 4:50pm (UTC)`, the way Claude Code prints a reset.
fn reset_text(t: i64) -> String {
    let d = chrono::DateTime::from_timestamp(t, 0).unwrap();
    let (pm, h) = d.hour12();
    let ampm = if pm { "pm" } else { "am" };
    let time = match d.minute() {
        0 => format!("{h}{ampm}"),
        m => format!("{h}:{m:02}{ampm}"),
    };
    format!("{} {}, {time} (UTC)", d.format("%b"), d.day())
}

/// Records its arguments and directory; `<p>.slow` makes it sleep in a child
/// process (not `exec`), so a grandchild keeps the pipes open after a kill.
/// An executable `<p>.hook` runs during the probe (test-only).
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
p=$(basename "${CLAUDE_CONFIG_DIR:-$HOME/.claude}")
printf '%s\n' "$#" "$@" > "$RECORD_DIR/args.$p"
printf 'CLAUDE_CONFIG_DIR=%s\n' "$CLAUDE_CONFIG_DIR" > "$RECORD_DIR/env.$p"
if [ -f "$CANNED/$p.slow" ]; then
  sleep 30
fi
[ -x "$CANNED/$p.hook" ] && "$CANNED/$p.hook"
[ -f "$CANNED/$p.err" ] && cat "$CANNED/$p.err" >&2
[ -f "$CANNED/$p.out" ] && cat "$CANNED/$p.out"
exit 0
"#;

const BANNER: &str = "You are currently using your subscription to power your Claude Code usage";
const STDERR_MARKER: &str = "RAW-STDERR-MARKER-1578";

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    records: PathBuf,
    canned: PathBuf,
    work: PathBuf,
    tools: PathBuf,
    /// Minute-aligned "now" the canned reset texts are relative to.
    base: i64,
}

impl Fixture {
    /// An empty profile root.
    fn empty() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let root = home.join("profiles");
        let records = tmp.path().join("records");
        let canned = tmp.path().join("canned");
        let work = tmp.path().join("work");
        let tools = tmp.path().join("tools");
        for d in [&home, &records, &canned, &work, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        executable(&tools.join("claude"), FAKE_CLAUDE);
        let t = now();
        Self {
            _tmp: tmp,
            home,
            root,
            records,
            canned,
            work,
            tools,
            base: t - t % 60,
        }
    }

    /// Profiles a (20/30, plus a model line), b (100% session), c (5/10, the
    /// default) and d (not logged in). Without `lease`, auto-switch is not
    /// set up; with it, the ledger says directory `a` holds `d`'s login.
    fn new(lease: bool) -> Self {
        let mut f = Self::empty();
        for p in ["a", "b", "c", "d"] {
            f.ok(&["profile", "add", p]);
        }
        f.root = f.root.canonicalize().unwrap();
        f.ok(&["profile", "switch", "c"]);
        let (h, w) = (f.base + 2 * 3600, f.base + 3 * 86_400);
        f.canned(
            "a",
            &format!(
                "{BANNER}\n\nCurrent session: 20% used · resets {}\n\
                 Current week (all models): 30% used · resets {}\n\
                 Current week (Fable): 0% used · resets {}\n\n\
                 What's contributing to your limits usage?\n",
                reset_text(h),
                reset_text(w),
                reset_text(w)
            ),
        );
        fs::write(f.canned.join("a.err"), format!("{STDERR_MARKER}\n")).unwrap();
        f.canned(
            "b",
            &format!(
                "{BANNER}\n\nCurrent session: 100% used · resets {}\n\
                 Current week (all models): 68% used · resets {}\n",
                reset_text(f.base + 3600),
                reset_text(w)
            ),
        );
        f.canned(
            "c",
            &format!(
                "{BANNER}\n\nCurrent session: 5% used · resets {}\n\
                 Current week (all models): 10% used · resets {}\n",
                reset_text(f.base + 4 * 3600),
                reset_text(w)
            ),
        );
        f.canned("d", "Not logged in · Please run /login\n");
        for p in ["a", "b", "c", "d"] {
            private_file(
                &f.root.join(p).join(".credentials.json"),
                &format!("{{\"claudeAiOauth\":{{\"accessToken\":\"SENTINEL-TOKEN-{p}\"}}}}"),
            );
        }
        if lease {
            let state = f.state();
            fs::create_dir_all(&state).unwrap();
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
            private_file(
                &state.join("ledger.json"),
                &serde_json::json!({
                    "version": 1,
                    "hosts": {"a": {"live": "d", "swapped_at": now() - 1000, "pre_swap_fingerprint": null}}
                })
                .to_string(),
            );
        }
        f
    }

    fn canned(&self, p: &str, stdout: &str) {
        fs::write(self.canned.join(format!("{p}.out")), stdout).unwrap();
    }

    fn state(&self) -> PathBuf {
        self.root.join(".auto-switch.d")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
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

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
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

    fn json(&self, args: &[&str]) -> (Vec<serde_json::Value>, String) {
        let out = self.ok(args);
        assert_no_secrets(&out);
        let stdout = text(&out.stdout);
        let v: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("stdout must be only JSON ({e}): {stdout:?}"));
        (
            v.as_array().expect("a JSON array").clone(),
            text(&out.stderr),
        )
    }

    fn usage_json(&self) -> Option<serde_json::Value> {
        let path = self.state().join("usage.json");
        let raw = fs::read_to_string(&path).ok()?;
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "usage.json must stay private");
        Some(serde_json::from_str(&raw).unwrap())
    }

    fn probed(&self) -> BTreeSet<String> {
        fs::read_dir(&self.records)
            .unwrap()
            .filter_map(|e| {
                let name = e.unwrap().file_name().to_string_lossy().to_string();
                name.strip_prefix("args.").map(str::to_string)
            })
            .collect()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

/// Data rows of the table: lines after the header that start a row (not
/// indented raw text).
fn table_rows(stdout: &str) -> Vec<&str> {
    let mut lines = stdout.lines();
    let header = lines.next().unwrap_or_default();
    assert!(header.starts_with("PROFILE"), "header first: {stdout}");
    lines
        .filter(|l| !l.starts_with(' ') && !l.is_empty())
        .collect()
}

fn first_word(l: &str) -> &str {
    l.split_whitespace().next().unwrap_or_default()
}

fn names(rows: &[serde_json::Value]) -> Vec<&str> {
    rows.iter()
        .map(|r| r["profile"].as_str().unwrap())
        .collect()
}

fn assert_no_secrets(out: &Output) {
    for s in [text(&out.stdout), text(&out.stderr)] {
        assert!(
            !s.contains("SENTINEL"),
            "credential or env secret printed: {s}"
        );
    }
}

// ---------------------------------------------------------------------------

#[test]
fn table_orders_by_capacity_marks_default_and_leases_and_hides_raw_text() {
    let f = Fixture::new(true);
    let out = f.ok(&["profile", "usage"]);
    let stdout = text(&out.stdout);
    let rows = table_rows(&stdout);
    assert_eq!(
        rows.iter().map(|l| first_word(l)).collect::<Vec<_>>(),
        ["c", "a", "b", "d"],
        "{stdout}"
    );
    assert!(
        rows[0].split_whitespace().nth(1) == Some("*"),
        "default marked: {stdout}"
    );
    assert!(rows[1].contains("hosts:d"), "a holds d's login: {stdout}");
    assert!(rows[3].contains("leased→a"), "d is lent to a: {stdout}");
    assert!(rows[2].contains("session_exhausted") && rows[2].contains("100%"));
    assert!(
        rows[2].contains(&reset_text(f.base + 3600)),
        "b shows when it is back: {stdout}"
    );
    assert!(rows[3].contains("not_logged_in"));
    assert!(!stdout.contains("Fable"), "model lines are JSON-only");

    let all = format!("{stdout}{}", text(&out.stderr));
    assert!(
        !all.contains(BANNER) && !all.contains(STDERR_MARKER),
        "raw text without --raw: {all}"
    );
    assert!(
        !all.contains("Not logged in"),
        "raw text without --raw: {all}"
    );
    assert_no_secrets(&out);

    for p in ["a", "b", "c", "d"] {
        let args = fs::read_to_string(f.records.join(format!("args.{p}"))).unwrap();
        assert_eq!(
            args, "2\n-p\n/usage\n",
            "exactly `claude -p /usage` for {p}"
        );
        let env = fs::read_to_string(f.records.join(format!("env.{p}"))).unwrap();
        assert_eq!(
            env,
            format!("CLAUDE_CONFIG_DIR={}\n", f.root.join(p).display())
        );
    }
}

#[test]
fn raw_shows_what_claude_printed() {
    let f = Fixture::new(true);
    let out = f.ok(&["profile", "usage", "--raw"]);
    let stdout = text(&out.stdout);
    assert!(stdout.contains("  raw stdout:"), "{stdout}");
    assert!(stdout.contains("  raw stderr:"), "{stdout}");
    assert!(stdout.contains(BANNER), "{stdout}");
    assert!(stdout.contains(STDERR_MARKER), "{stdout}");
    assert_eq!(
        table_rows(&stdout)
            .iter()
            .map(|l| first_word(l))
            .collect::<Vec<_>>(),
        ["c", "a", "b", "d"]
    );
    assert_no_secrets(&out);
}

#[test]
fn json_has_the_documented_shape_and_order() {
    let f = Fixture::new(true);
    let (rows, stderr) = f.json(&["profile", "usage", "--json"]);
    assert_eq!(names(&rows), ["c", "a", "b", "d"]);
    let states: Vec<&str> = rows.iter().map(|r| r["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["ok", "ok", "session_exhausted", "not_logged_in"]);

    let (c, a, b, d) = (&rows[0], &rows[1], &rows[2], &rows[3]);
    assert_eq!(c["default"], true);
    assert_eq!(a["default"], false);
    assert_eq!(a["account"], "d");
    assert_eq!(a["leased_to"], serde_json::Value::Null);
    for r in &rows {
        assert_eq!(r["live_in"], serde_json::Value::Null, "{r}");
    }
    assert_eq!(d["account"], "d");
    assert_eq!(d["leased_to"], "a");
    assert_eq!(a["session"]["used_percentage"], 20.0);
    assert_eq!(a["session"]["resets_text"], reset_text(f.base + 2 * 3600));
    assert_eq!(a["session"]["resets_at"], f.base + 2 * 3600);
    assert_eq!(a["week_all"]["label"], "all models");
    assert_eq!(a["week_all"]["resets_at"], f.base + 3 * 86_400);
    assert_eq!(a["week_models"][0]["label"], "Fable");
    assert_eq!(b["session"]["resets_at"], f.base + 3600);
    assert_eq!(d["session"], serde_json::Value::Null);
    assert_eq!(d["week_models"], serde_json::json!([]));
    for r in &rows {
        assert!(r.get("raw").is_none(), "no raw key without --raw: {r}");
        assert_eq!(r["error"], serde_json::Value::Null);
    }
    assert!(!stderr.contains("note:"), "--json prints no note: {stderr}");

    let (rows, _) = f.json(&["profile", "usage", "--json", "--raw"]);
    assert!(rows[1]["raw"]["stdout"].as_str().unwrap().contains(BANNER));
    assert!(
        rows[1]["raw"]["stderr"]
            .as_str()
            .unwrap()
            .contains(STDERR_MARKER)
    );
}

#[test]
fn named_profiles_keep_the_usual_order_and_are_probed_once() {
    let f = Fixture::new(false);
    let (rows, _) = f.json(&["profile", "usage", "--json", "b", "c", "b"]);
    assert_eq!(names(&rows), ["c", "b"]);
    assert_eq!(
        f.probed(),
        BTreeSet::from(["b".to_string(), "c".to_string()])
    );
}

#[test]
fn usage_record_is_refreshed_under_the_live_account() {
    let f = Fixture::new(true);
    assert!(f.usage_json().is_none());
    let t0 = now();
    f.ok(&["profile", "usage"]);
    let u = f.usage_json().expect("usage.json written");
    assert_eq!(u["version"], 1);
    let accounts = u["accounts"].as_object().unwrap();
    let keys: BTreeSet<&str> = accounts.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        BTreeSet::from(["b", "c", "d"]),
        "a's numbers belong to d; d's own row is not logged in"
    );
    for (name, reading) in accounts {
        let fields: BTreeSet<&str> = reading
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert!(
            fields.is_subset(&BTreeSet::from([
                "five_hour",
                "seven_day",
                "observed_at",
                "source"
            ])),
            "{name}: {fields:?}"
        );
        assert_eq!(reading["source"], "statusline", "{name}");
        assert!(reading["observed_at"].as_i64().unwrap() >= t0, "{name}");
    }
    let d = &accounts["d"];
    assert_eq!(d["five_hour"]["used_percentage"], 20.0);
    assert_eq!(d["five_hour"]["resets_at"], f.base + 2 * 3600);
    assert_eq!(d["seven_day"]["used_percentage"], 30.0);
    assert_eq!(d["seven_day"]["resets_at"], f.base + 3 * 86_400);
    assert_eq!(accounts["b"]["five_hour"]["used_percentage"], 100.0);
    assert_eq!(accounts["c"]["five_hour"]["used_percentage"], 5.0);
    assert_eq!(accounts["c"]["seven_day"]["used_percentage"], 10.0);
}

#[test]
fn not_set_up_records_nothing_and_says_so_only_in_table_mode() {
    let f = Fixture::new(false);
    let out = f.ok(&["profile", "usage"]);
    assert!(
        text(&out.stderr).contains("note: auto-switch not set up; usage record not updated"),
        "{}",
        text(&out.stderr)
    );
    assert!(!f.state().exists(), "no auto-switch state created");
    let (_, stderr) = f.json(&["profile", "usage", "--json"]);
    assert!(!stderr.contains("note:"), "{stderr}");
    assert!(!f.state().exists());
}

#[test]
fn an_env_credential_skips_recording_and_is_named_not_echoed() {
    let f = Fixture::new(true);
    let out = f
        .cmd(&["profile", "usage"])
        .env("ANTHROPIC_API_KEY", ENV_KEY_VALUE)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(table_rows(&text(&out.stdout)).len(), 4, "rows still shown");
    assert!(
        text(&out.stderr)
            .contains("auto-switch: ANTHROPIC_API_KEY is set; usage record not updated"),
        "{}",
        text(&out.stderr)
    );
    assert!(f.usage_json().is_none(), "nothing recorded");
    assert_no_secrets(&out);
    for stream in [&out.stdout, &out.stderr] {
        assert!(!text(stream).contains(ENV_KEY_VALUE), "value echoed");
    }
}

#[test]
fn an_env_credential_is_warned_about_even_without_auto_switch() {
    let f = Fixture::new(false);
    for args in [&["profile", "usage"][..], &["profile", "usage", "--json"]] {
        let out = f
            .cmd(args)
            .env("ANTHROPIC_API_KEY", ENV_KEY_VALUE)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert!(
            text(&out.stderr).contains(
                "warning: ANTHROPIC_API_KEY is set; claude may report that credential's usage, not each profile's"
            ),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert!(!f.state().exists(), "no auto-switch state created");
        for stream in [&out.stdout, &out.stderr] {
            assert!(!text(stream).contains(ENV_KEY_VALUE), "value echoed");
        }
    }
}

/// Plain words, so secret scanners do not mistake the test value for a key.
const ENV_KEY_VALUE: &str = "value-of-the-env-key";

#[test]
fn a_slow_profile_times_out_without_holding_up_the_others() {
    let f = Fixture::new(false);
    f.ok(&["profile", "add", "e"]);
    fs::write(f.canned.join("e.slow"), "").unwrap();
    f.canned("e", "Current session: 1% used · resets 4pm (UTC)\n");
    let start = Instant::now();
    let (rows, _) = f.json(&[
        "profile",
        "usage",
        "--json",
        "--timeout-secs",
        "2",
        "a",
        "e",
    ]);
    let took = start.elapsed();
    assert!(took < Duration::from_secs(8), "took {took:?}");
    assert_eq!(names(&rows), ["a", "e"]);
    assert_eq!(rows[0]["state"], "ok");
    assert_eq!(rows[1]["state"], "timeout");
    assert_eq!(rows[1]["session"], serde_json::Value::Null);
}

#[test]
fn timeout_env_is_honoured_and_a_bad_value_warns() {
    let f = Fixture::new(false);
    f.ok(&["profile", "add", "e"]);
    fs::write(f.canned.join("e.slow"), "").unwrap();
    let start = Instant::now();
    let out = f
        .cmd(&["profile", "usage", "--json", "e"])
        .env("AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        start.elapsed() < Duration::from_secs(6),
        "{:?}",
        start.elapsed()
    );

    let out = f
        .cmd(&["profile", "usage", "--json", "c"])
        .env("AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS", "soon")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        text(&out.stderr).contains(
            "warning: ignored AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS \"soon\" (use 1 to 600); using 30"
        ),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn a_refused_profile_directory_is_an_error_row_not_a_failure() {
    let f = Fixture::new(false);
    fs::set_permissions(f.root.join("b"), fs::Permissions::from_mode(0o755)).unwrap();
    let out = f.ok(&["profile", "usage", "--json"]);
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(names(&rows), ["c", "a", "d", "b"], "error rows sort last");
    assert_eq!(rows[3]["state"], "error");
    assert_eq!(rows[3]["error"], "profile directory refused");
    assert!(
        text(&out.stderr).contains("warning: profile \"b\": profile directory refused"),
        "{}",
        text(&out.stderr)
    );
    assert!(
        !f.probed().contains("b"),
        "a refused directory is never spawned"
    );
}

#[test]
fn setup_failures_exit_non_zero_before_probing() {
    let f = Fixture::new(false);
    for (args, needle) in [
        (vec!["profile", "usage", "../x"], "invalid profile name"),
        (vec!["profile", "usage", "a", "nosuch"], "not found"),
    ] {
        let out = f.run(&args);
        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.contains(needle) && stderr.contains("hint:"),
            "{args:?}: {stderr}"
        );
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(f.probed().is_empty(), "{args:?}: nothing probed");
    }
    for bad in ["0", "601", "soon"] {
        let out = f.run(&["profile", "usage", "--timeout-secs", bad]);
        assert_eq!(out.status.code(), Some(2), "--timeout-secs {bad}");
    }
    assert!(f.probed().is_empty());

    let empty = tempfile::tempdir().unwrap();
    let out = f
        .cmd(&["profile", "usage"])
        .env("AMPLIHACK_CLAUDE_BINARY_PATH", empty.path().join("claude"))
        .env("PATH", empty.path())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "claude not found: {}",
        text(&out.stderr)
    );
    assert!(f.probed().is_empty());
}

#[test]
fn no_profiles_prints_the_list_message_or_an_empty_array() {
    let f = Fixture::empty();
    let out = f.ok(&["profile", "usage"]);
    assert_eq!(
        text(&out.stdout).trim(),
        "no profiles; create one with `amplihack profile add <name>`"
    );
    let out = f.ok(&["profile", "usage", "--json"]);
    assert_eq!(text(&out.stdout).trim(), "[]");
}

impl Fixture {
    /// Moves `c`'s login into `~/.claude`, where `claude` reports 40/55.
    fn login_in_default_dir(&self) {
        fs::remove_file(self.root.join("c/.credentials.json")).unwrap();
        private_file(&self.home.join(".claude/.credentials.json"), "{}");
        self.canned(
            ".claude",
            &format!(
                "{BANNER}\n\nCurrent session: 40% used · resets {}\n\
                 Current week (all models): 55% used · resets {}\n",
                reset_text(self.base + 3600),
                reset_text(self.base + 2 * 86_400)
            ),
        );
    }

    fn assert_c_is_live_in_default_dir(&self) {
        let (rows, _) = self.json(&["profile", "usage", "--json"]);
        let c = rows.iter().find(|r| r["profile"] == "c").unwrap();
        assert_eq!(c["live_in"], "~/.claude");
        assert_eq!(c["state"], "ok");
        assert_eq!(c["session"]["used_percentage"], 40.0);
        assert_eq!(c["week_all"]["used_percentage"], 55.0);
        let live: Vec<_> = rows.iter().filter(|r| !r["live_in"].is_null()).collect();
        assert_eq!(live.len(), 1);
        assert_eq!(
            fs::read_to_string(self.records.join("env..claude")).unwrap(),
            "CLAUDE_CONFIG_DIR=\n",
            "the variable is removed, never set to ~/.claude"
        );
        let u = self.usage_json().expect("usage.json written");
        assert_eq!(u["accounts"]["c"]["five_hour"]["used_percentage"], 40.0);
        assert_eq!(u["accounts"]["c"]["seven_day"]["used_percentage"], 55.0);

        let out = self.ok(&["profile", "usage"]);
        let stdout = text(&out.stdout);
        let row = table_rows(&stdout)
            .into_iter()
            .find(|l| first_word(l) == "c")
            .unwrap();
        assert!(row.contains("* live in ~/.claude"), "{row}");
        assert!(!stdout.contains(&*self.home.to_string_lossy()), "{stdout}");
    }
}

#[test]
fn rotator_ledger_marks_the_login_live_in_default_dir() {
    let f = Fixture::new(true);
    f.login_in_default_dir();
    let rotator = f.root.join(".rotator");
    private_file(
        &rotator.join("ledger.json"),
        r#"{"duplicate":null,"live":"c","live_hash":"SECRET-HASH","live_week_reset":0,"since":5}"#,
    );
    fs::set_permissions(&rotator, fs::Permissions::from_mode(0o700)).unwrap();
    let before = fs::read(rotator.join("ledger.json")).unwrap();
    f.assert_c_is_live_in_default_dir();
    assert_eq!(fs::read(rotator.join("ledger.json")).unwrap(), before);
    assert_eq!(
        fs::read_dir(&rotator).unwrap().count(),
        1,
        "nothing written under .rotator"
    );
}

#[test]
fn without_a_ledger_the_matching_account_is_live_in_default_dir() {
    let f = Fixture::new(true);
    f.login_in_default_dir();
    let acct = r#"{"oauthAccount":{"accountUuid":"uuid-c","emailAddress":"c@example.com"}}"#;
    private_file(&f.home.join(".claude.json"), acct);
    private_file(&f.root.join("c/.claude.json"), acct);
    f.assert_c_is_live_in_default_dir();
}

#[test]
fn no_ledger_is_quiet_and_marks_nothing_live() {
    let f = Fixture::new(true);
    f.login_in_default_dir();
    let rotator = f.root.join(".rotator");
    fs::create_dir_all(&rotator).unwrap();
    fs::set_permissions(&rotator, fs::Permissions::from_mode(0o700)).unwrap();
    // ~/.claude has an account; no profile's .claude.json matches it.
    private_file(
        &f.home.join(".claude.json"),
        r#"{"oauthAccount":{"accountUuid":"uuid-x","emailAddress":"x@example.com"}}"#,
    );
    let (rows, stderr) = f.json(&["profile", "usage", "--json"]);
    assert!(rows.iter().all(|r| r["live_in"].is_null()));
    assert!(!stderr.contains("~/.claude"), "{stderr}");
    // c is probed through its own directory, not ~/.claude. The fake
    // `claude` does not check for a login, so its canned 5% comes back.
    let c = rows.iter().find(|r| r["profile"] == "c").unwrap();
    assert_eq!(c["session"]["used_percentage"], 5.0);
    assert!(f.records.join("env.c").exists());
    assert!(!f.records.join("env..claude").exists());
}

#[test]
fn a_swap_during_the_probe_is_warned_and_not_recorded() {
    let f = Fixture::new(true);
    f.login_in_default_dir();
    let rotator = f.root.join(".rotator");
    private_file(&rotator.join("ledger.json"), r#"{"live":"c","since":1}"#);
    fs::set_permissions(&rotator, fs::Permissions::from_mode(0o700)).unwrap();
    let r = rotator.display();
    executable(
        &f.canned.join(".claude.hook"),
        &format!(
            "#!/bin/sh\numask 077\n\
             printf '%s' '{{\"live\":\"c\",\"since\":2}}' > '{r}/ledger.tmp'\n\
             mv '{r}/ledger.tmp' '{r}/ledger.json'\n"
        ),
    );
    let out = f.ok(&["profile", "usage", "--json"]);
    assert_no_secrets(&out);
    let stderr = text(&out.stderr);
    let warning = "warning: the login in ~/.claude changed while profile \"c\" was probed; its usage was not recorded";
    assert_eq!(
        stderr.lines().filter(|l| *l == warning).count(),
        1,
        "{stderr}"
    );
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    let c = rows.iter().find(|r| r["profile"] == "c").unwrap();
    assert_eq!(c["state"], "ok");
    assert_eq!(c["session"]["used_percentage"], 40.0);
    assert_eq!(c["week_all"]["used_percentage"], 55.0);
    let u = f.usage_json().expect("usage.json written");
    let keys: BTreeSet<&str> = u["accounts"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["b", "d"]),
        "c's swapped numbers are dropped; a's belong to d"
    );
}

#[test]
fn malformed_ledger_warns_once_and_marks_nothing_live() {
    let f = Fixture::new(true);
    f.login_in_default_dir();
    private_file(&f.root.join(".rotator/ledger.json"), "not json");
    fs::set_permissions(f.root.join(".rotator"), fs::Permissions::from_mode(0o700)).unwrap();
    let (rows, stderr) = f.json(&["profile", "usage", "--json"]);
    assert!(rows.iter().all(|r| r["live_in"].is_null()));
    let warning = "warning: could not read the login rotator's ledger; no profile is marked live in ~/.claude";
    assert_eq!(
        stderr.lines().filter(|l| *l == warning).count(),
        1,
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(f.root.join(".rotator/ledger.json")).unwrap(),
        "not json"
    );
}
