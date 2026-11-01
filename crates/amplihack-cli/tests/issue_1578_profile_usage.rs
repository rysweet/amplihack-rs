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

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::profile_default::set_default;
use chrono::{Datelike, Timelike};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
///
/// Issue #1698: the usage request (`--input-format` in argv) is answered
/// only when `<p>.json` exists: the fake records its arguments and stdin,
/// sleeps for the seconds in `<p>.wait` if present, then prints a 16 KiB
/// `system` line and the canned answer. Without `<p>.json` it exits 1 at
/// once, so the probe falls back to `/usage`. With `$CANNED/marks`, the
/// `/usage` path appends `S <p>` and `E <p>` around a 0.5 s sleep.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
p=$(basename "${CLAUDE_CONFIG_DIR:-$HOME/.claude}")
case " $* " in
  *" --input-format "*)
    [ -f "$CANNED/$p.json" ] || exit 1
    printf '%s\n' "$#" "$@" > "$RECORD_DIR/args-request.$p"
    cat > "$RECORD_DIR/stdin.$p"
    if [ -f "$CANNED/$p.wait" ]; then sleep "$(cat "$CANNED/$p.wait")"; fi
    printf '{"type":"system","subtype":"init","note":"SYSTEM-LINE-MARKER-1698","pad":"%s"}\n' "$(head -c 16384 /dev/zero | tr '\0' x)"
    cat "$CANNED/$p.json"
    exit 0 ;;
esac
printf '%s\n' "$#" "$@" > "$RECORD_DIR/args.$p"
printf 'CLAUDE_CONFIG_DIR=%s\n' "$CLAUDE_CONFIG_DIR" > "$RECORD_DIR/env.$p"
if [ -f "$CANNED/marks" ]; then
  echo "S $p" >> "$RECORD_DIR/marks"
  sleep 0.5
  echo "E $p" >> "$RECORD_DIR/marks"
fi
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
        set_default(&f.root, "c");
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
        set_default(&f.root, "c");
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

    /// Makes the fake answer `p`'s usage request with `line` (issue #1698).
    fn answers(&self, p: &str, line: &str) {
        fs::write(self.canned.join(format!("{p}.json")), format!("{line}\n")).unwrap();
    }

    fn recorded(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.records.join(file)).ok()
    }

    fn state(&self) -> PathBuf {
        self.root.join(".auto-switch.d")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
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
            .env("RECORD_DIR", &self.records)
            .env("CANNED", &self.canned)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            // Launches never start a real background rotator here (autostart).
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            // Issue #1844: these tests read through `claude`; a reading an
            // earlier command recorded must not stand in as a passive one.
            .env("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "0")
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

/// No login token and nothing from the rotator's ledger beyond the name.
fn assert_no_secrets(out: &Output) {
    for s in [text(&out.stdout), text(&out.stderr)] {
        for secret in ["SENTINEL", "SECRET-HASH"] {
            assert!(!s.contains(secret), "secret {secret:?} printed: {s}");
        }
    }
}

fn warning_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.starts_with("warning:"))
        .collect()
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
    assert!(took < Duration::from_secs(20), "took {took:?}");
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
        start.elapsed() < Duration::from_secs(20),
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
            "warning: ignored AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS \"soon\" (use 1 to 600); using 120"
        ),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn a_bad_parallel_value_warns_and_every_profile_is_still_probed() {
    // Issue #1698: the variable is set on the child only.
    let f = Fixture::new(false);
    let out = f
        .cmd(&["profile", "usage", "--json"])
        .env("AMPLIHACK_PROFILE_USAGE_PARALLEL", "lots")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stderr = text(&out.stderr);
    let warning = warning_lines(&stderr)
        .into_iter()
        .find(|l| l.contains("AMPLIHACK_PROFILE_USAGE_PARALLEL"))
        .unwrap_or_else(|| panic!("no parallel warning: {stderr}"));
    assert!(
        warning.starts_with(
            "warning: ignored AMPLIHACK_PROFILE_USAGE_PARALLEL \"lots\" (use 1 to 16); using "
        ),
        "{warning}"
    );
    let n: usize = warning.rsplit(' ').next().unwrap().parse().unwrap();
    assert!((2..=4).contains(&n), "derived bound {n}");
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!rows.is_empty());
}

#[test]
fn a_valid_or_empty_parallel_value_prints_no_warning() {
    let f = Fixture::new(false);
    for v in ["1", "16", ""] {
        let out = f
            .cmd(&["profile", "usage", "--json"])
            .env("AMPLIHACK_PROFILE_USAGE_PARALLEL", v)
            .output()
            .unwrap();
        assert!(out.status.success(), "{v:?}: {}", text(&out.stderr));
        assert!(
            !text(&out.stderr).contains("AMPLIHACK_PROFILE_USAGE_PARALLEL"),
            "{v:?}: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn a_timed_out_profile_is_timeout_in_the_table_never_a_percentage() {
    let f = Fixture::new(false);
    f.ok(&["profile", "add", "e"]);
    fs::write(f.canned.join("e.slow"), "").unwrap();
    f.canned("e", "Current session: 100% used · resets 4pm (UTC)\n");
    let out = f
        .cmd(&["profile", "usage", "e"])
        .env("AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    let row = table_rows(&stdout)
        .into_iter()
        .find(|l| first_word(l) == "e")
        .unwrap_or_else(|| panic!("no row for e: {stdout}"));
    assert!(row.split_whitespace().any(|w| w == "timeout"), "{row}");
    assert!(!row.contains('%'), "{row}");
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
    // Issue #1844: `claude` is looked up only when a row needs it, and a
    // missing one fails only those rows, not the command.
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains(": claude not found"),
        "{}",
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
        private_file(
            &self.home.join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"SENTINEL-TOKEN-default"}}"#,
        );
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
        assert_no_secrets(&out);
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
    assert!(warning_lines(&stderr).is_empty(), "{stderr}");
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
fn an_auto_switch_swap_during_the_probe_is_not_recorded() {
    let f = Fixture::new(true);
    // While `b` is probed, auto-switch moves `c`'s login into `b`'s
    // directory. `b`'s numbers were read before that, so they must not be
    // recorded under `b` or `c`.
    let state = f.state().display().to_string();
    let ledger = serde_json::json!({
        "version": 1,
        "hosts": {
            "a": {"live": "d", "swapped_at": now() - 1000, "pre_swap_fingerprint": null},
            "b": {"live": "c", "swapped_at": now(), "pre_swap_fingerprint": null}
        }
    });
    executable(
        &f.canned.join("b.hook"),
        &format!(
            "#!/bin/sh\numask 077\n\
             printf '%s' '{ledger}' > '{state}/ledger.tmp'\n\
             mv '{state}/ledger.tmp' '{state}/ledger.json'\n"
        ),
    );
    let out = f.ok(&["profile", "usage", "--json"]);
    assert_no_secrets(&out);
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(
            "auto-switch: profile \"b\" changed accounts during the probe; its usage was not recorded"
        ),
        "{stderr}"
    );
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    let b = rows.iter().find(|r| r["profile"] == "b").unwrap();
    assert_eq!(b["account"], "b", "the account live when the probe started");
    let u = f.usage_json().expect("usage.json written");
    assert!(u["accounts"].get("b").is_none(), "{u}");
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
    assert_eq!(warning_lines(&stderr), [warning], "{stderr}");
    assert_eq!(
        fs::read_to_string(f.root.join(".rotator/ledger.json")).unwrap(),
        "not json"
    );
}

// ---------------------------------------------------------------------------
// Issue #1698: the usage request, its `/usage` fallback and the method
// variable. `<p>.json` makes the fake answer `p`'s request; `a.out` reads
// 20/30, so a row at 42/17 came from the request.
// ---------------------------------------------------------------------------

const REQUEST_LINE: &str = r#"{"type":"control_request","request_id":"amplihack-usage","request":{"subtype":"get_usage","skip_behaviors":true}}"#;
const REQUEST_ARGS: &str =
    "6\n-p\n--input-format\nstream-json\n--output-format\nstream-json\n--verbose\n";
const SYSTEM_MARKER: &str = "SYSTEM-LINE-MARKER-1698";
const PROBE_ENV: &str = "AMPLIHACK_PROFILE_USAGE_PROBE";

/// An answer shaped as Claude Code 2.1.314 sends it, with invented figures.
fn answer(session: f64, week: f64, credits: bool) -> String {
    let at = |t: i64| {
        chrono::DateTime::from_timestamp(t, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    };
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": "amplihack-usage",
            "response": {
                "rate_limits_available": true,
                "rate_limits": {
                    "five_hour": {"utilization": session, "resets_at": at(now() + 7200)},
                    "seven_day": {"utilization": week, "resets_at": at(now() + 3 * 86_400)},
                    "model_scoped": [
                        {"display_name": "Fable", "utilization": 3, "resets_at": at(now() + 3 * 86_400)}
                    ],
                    "extra_usage": {"is_enabled": credits}
                }
            }
        }
    })
    .to_string()
}

fn session(row: &serde_json::Value) -> &serde_json::Value {
    &row["session"]["used_percentage"]
}

#[test]
fn an_answered_request_gives_the_row_without_usage() {
    let f = Fixture::new(false);
    f.answers("a", &answer(42.0, 17.0, false));
    let (rows, _) = f.json(&["profile", "usage", "--json", "a"]);
    assert_eq!(rows[0]["state"], "ok");
    assert_eq!(session(&rows[0]), 42.0);
    assert_eq!(rows[0]["week_all"]["used_percentage"], 17.0);
    assert_eq!(rows[0]["week_models"][0]["label"], "Fable");
    assert!(rows[0]["session"]["resets_at"].is_i64(), "{}", rows[0]);
    assert_eq!(f.recorded("args-request.a").as_deref(), Some(REQUEST_ARGS));
    assert_eq!(f.recorded("stdin.a"), Some(format!("{REQUEST_LINE}\n")));
    assert_eq!(f.recorded("args.a"), None, "/usage was not run");

    let out = f.ok(&["profile", "usage", "a"]);
    let stdout = text(&out.stdout);
    let row = table_rows(&stdout)[0];
    assert!(row.contains("42%") && row.contains("17%"), "{stdout}");
}

#[test]
fn an_error_answer_falls_back_to_usage() {
    let f = Fixture::new(false);
    f.answers(
        "a",
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"amplihack-usage","error":"Unknown control request"}}"#,
    );
    let (rows, stderr) = f.json(&["profile", "usage", "--json", "a"]);
    assert_eq!(session(&rows[0]), 20.0, "read from /usage");
    assert!(f.recorded("args-request.a").is_some());
    assert_eq!(f.recorded("args.a").as_deref(), Some("2\n-p\n/usage\n"));
    assert!(!stderr.contains("Unknown control request"), "{stderr}");
}

#[test]
fn an_answer_with_usage_credits_on_falls_back_to_usage() {
    let f = Fixture::new(false);
    f.answers("a", &answer(42.0, 17.0, true));
    let (rows, _) = f.json(&["profile", "usage", "--json", "a"]);
    assert_eq!(session(&rows[0]), 20.0, "read from /usage");
    assert!(f.recorded("args.a").is_some());
}

#[test]
fn text_mode_runs_only_usage() {
    let f = Fixture::new(false);
    f.answers("a", &answer(42.0, 17.0, false));
    let out = f
        .cmd(&["profile", "usage", "--json", "a"])
        .env(PROBE_ENV, "text")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(session(&rows[0]), 20.0);
    assert_eq!(f.recorded("stdin.a"), None);
    assert_eq!(f.recorded("args-request.a"), None);
    assert!(
        !text(&out.stderr).contains(PROBE_ENV),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn a_bad_method_warns_once_and_still_uses_the_request() {
    let f = Fixture::new(false);
    f.answers("a", &answer(42.0, 17.0, false));
    let out = f
        .cmd(&["profile", "usage", "--json", "a"])
        .env(PROBE_ENV, "Text")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stderr = text(&out.stderr);
    let warnings: Vec<&str> = warning_lines(&stderr)
        .into_iter()
        .filter(|l| l.contains(PROBE_ENV))
        .collect();
    assert_eq!(
        warnings,
        [
            "warning: ignored AMPLIHACK_PROFILE_USAGE_PROBE \"Text\" (use direct, auto or text); using the default order"
        ]
    );
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(session(&rows[0]), 42.0);
}

#[test]
fn a_slow_request_inside_the_deadline_is_used() {
    let f = Fixture::new(false);
    f.answers("a", &answer(42.0, 17.0, false));
    fs::write(f.canned.join("a.wait"), "2").unwrap();
    let start = Instant::now();
    let (rows, _) = f.json(&["profile", "usage", "--json", "--timeout-secs", "10", "a"]);
    assert!(start.elapsed() >= Duration::from_secs(2));
    assert_eq!(rows[0]["state"], "ok");
    assert_eq!(session(&rows[0]), 42.0);
    assert_eq!(f.recorded("args.a"), None);
}

#[test]
fn a_request_past_the_deadline_is_timeout_and_usage_never_runs() {
    let f = Fixture::new(false);
    f.answers("a", &answer(42.0, 17.0, false));
    fs::write(f.canned.join("a.wait"), "30").unwrap();
    let start = Instant::now();
    let out = f.ok(&["profile", "usage", "--timeout-secs", "1", "a"]);
    assert!(start.elapsed() < Duration::from_secs(15));
    let stdout = text(&out.stdout);
    let row = table_rows(&stdout)[0];
    let cells: Vec<&str> = row.split_whitespace().collect();
    assert_eq!(cells[0], "a");
    assert!(cells.contains(&"timeout"), "{row}");
    assert!(!row.contains('%'), "{row}");
    // SESSION, SESSION RESETS, WEEK and WEEK RESETS.
    assert_eq!(cells[cells.len() - 4..], ["-", "-", "-", "-"], "{row}");
    assert_eq!(f.recorded("args.a"), None, "no /usage after the deadline");

    let (rows, _) = f.json(&["profile", "usage", "--json", "--timeout-secs", "1", "a"]);
    assert_eq!(rows[0]["state"], "timeout");
    assert_eq!(rows[0]["session"], serde_json::Value::Null);
    assert_eq!(rows[0]["week_all"], serde_json::Value::Null);
    assert_eq!(rows[0]["week_models"], serde_json::json!([]));
    assert_eq!(rows[0]["error"], serde_json::Value::Null);
}

#[test]
fn the_bound_holds_with_the_request_and_its_fallback() {
    let f = Fixture::new(false);
    for p in ["e", "g"] {
        f.ok(&["profile", "add", p]);
        f.canned(
            p,
            "Current session: 1% used\nCurrent week (all models): 1% used\n",
        );
    }
    fs::write(f.canned.join("marks"), "").unwrap();
    let out = f
        .cmd(&["profile", "usage", "--json"])
        .env("AMPLIHACK_PROFILE_USAGE_PARALLEL", "2")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let marks = fs::read_to_string(f.records.join("marks")).unwrap();
    let (mut running, mut peak) = (0usize, 0usize);
    for line in marks.lines() {
        if line.starts_with("S ") {
            running += 1;
            peak = peak.max(running);
        } else {
            running -= 1;
        }
    }
    assert_eq!(marks.lines().filter(|l| l.starts_with("S ")).count(), 6);
    assert_eq!(peak, 2, "six profiles at a bound of 2: {marks}");
}

#[test]
fn the_system_line_is_shown_only_with_raw() {
    let f = Fixture::new(false);
    f.answers("a", &answer(42.0, 17.0, false));
    let out = f.ok(&["profile", "usage", "a"]);
    for s in [text(&out.stdout), text(&out.stderr)] {
        assert!(!s.contains(SYSTEM_MARKER), "{s}");
    }
    let out = f.ok(&["profile", "usage", "--raw", "a"]);
    assert!(text(&out.stdout).contains(SYSTEM_MARKER));
    assert!(!text(&out.stderr).contains(SYSTEM_MARKER));
    assert_no_secrets(&out);
}

#[test]
fn an_overflow_profile_is_probed_with_usage_only() {
    let f = Fixture::new(false);
    f.answers("a", &answer(42.0, 17.0, false));
    f.ok(&["profile", "overflow", "add", "a"]);
    let (rows, _) = f.json(&["profile", "usage", "--json", "a"]);
    assert_eq!(rows[0]["overflow"], true);
    assert_eq!(session(&rows[0]), 20.0, "read from /usage");
    assert_eq!(f.recorded("stdin.a"), None);
    assert_eq!(f.recorded("args-request.a"), None);
}
