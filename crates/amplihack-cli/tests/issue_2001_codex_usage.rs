//! Issue #2001: `amplihack profile usage` reads every Codex account live and
//! lists it after the Claude output.
//!
//! Every run uses the rotate `Fixture` (temporary HOME and Claude profile
//! root, fake `claude`) plus a Codex profiles root under the same temporary
//! directory and a fake `codex app-server` (`CODEX_BIN`) that answers by the
//! account's directory name. No real `codex`, `~/.codex` or
//! `~/.codex-profiles` is touched, and no account is contacted.
//!
//! Each account directory holds an `auth.json` that is a FIFO and a
//! `config.toml` with mode 000, so a read of either hangs or fails; every
//! command must still finish inside a 60-second deadline.

#![cfg(unix)]

use crate::support::exec_stub::write_executable;
use crate::support::profile_rotate::{Fixture, PROFILES, assert_no_secrets, text};
use std::fs;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// A hang detector, not a timing assertion: a command that has not finished
/// by then is reported as hung (a FIFO read, or a process left running).
const DEADLINE: Duration = Duration::from_secs(60);

/// The descriptor number the command under test inherits the witness on.
const WITNESS_FD: i32 = 9;
const OPENAI_SENTINEL: &str = "sk-test-should-be-removed";
const CODEX_KEY_SENTINEL: &str = "codex-api-key-SENTINEL-2001";
const TIMEOUT_WARNING: &str = "warning: AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS=\"0\" is not a whole number from 1 to 300; using 20";

/// The fake app server. It refuses to answer when an API key reached it,
/// logs each start (`$CODEX_RUN_LOG`) and its pid (`$CODEX_PID_DIR`), and
/// answers by `basename "$CODEX_HOME"`. It never opens a file in the
/// account directory.
const FAKE_CODEX: &str = r#"#!/bin/sh
[ "$1" = "app-server" ] || { echo "bad args: $*" >&2; exit 64; }
name=$(basename "$CODEX_HOME")
echo "$name" >> "$CODEX_RUN_LOG"
echo $$ > "$CODEX_PID_DIR/$$"
if [ -n "${OPENAI_API_KEY+x}" ] || [ -n "${CODEX_API_KEY+x}" ]; then
  echo "an API key reached the app server" >&2
  exit 70
fi
[ -z "${RUST_LOG+x}" ] || { echo "RUST_LOG reached the app server" >&2; exit 71; }
# Issue #2271: `gated` answers nothing until the test closes its end of
# the FIFO at $CODEX_GATE.
if [ "$name" = gated ]; then cat "$CODEX_GATE" >/dev/null; fi
IFS= read -r init || exit 65
case "$init" in *'"method":"initialize"'*) ;; *) echo "first line: $init" >&2; exit 65;; esac
printf '%s\n' '{"method":"remoteControl/status/changed","params":{"status":"idle"}}'
printf '%s\n' '{"id":1,"result":{"userAgent":"fake-codex/0.0"}}'
IFS= read -r note || exit 66
IFS= read -r req || exit 67
case "$req" in *'"method":"account/rateLimits/read"'*) ;; *) echo "third line: $req" >&2; exit 67;; esac
now=$(date +%s)
case "$name" in
  ffarmer)
    printf '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":12,"windowDurationMins":300,"resetsAt":%s},"secondary":{"usedPercent":41,"windowDurationMins":10080,"resetsAt":%s},"credits":null,"planType":"plus","rateLimitReachedType":null,"ordinaryUsageAllowed":true}}}\n' $((now + 9060)) $((now + 478260)) ;;
  ggrant)
    printf '{"id":2,"result":{"rateLimits":{"primary":null,"secondary":{"usedPercent":34,"windowDurationMins":10080,"resetsAt":%s},"planType":"pro","ordinaryUsageAllowed":true}}}\n' $((now + 328860)) ;;
  hhale)
    printf '%s\n' '{"id":2,"result":{"rateLimits":{"credits":{"hasCredits":true,"balance":"4200"},"planType":"team"}}}' ;;
  jjones)
    printf '%s\n' '{"error":{"code":-32600,"message":"codex account authentication required to read rate limits"},"id":2}' ;;
  kkim)
    sleep 600 >/dev/null 2>&1 &
    echo $! > "$CODEX_PID_DIR/$!"
    sleep 600 ;;
  gated)
    printf '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":7,"windowDurationMins":300,"resetsAt":%s}},"planType":"plus"}}\n' $((now + 3600)) ;;
  mmoss)
    echo "error: unrecognized subcommand 'app-server'" >&2
    exit 1 ;;
  .codex)
    if [ "$FAKE_HOME_STATE" = ok ]; then
      printf '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":88,"windowDurationMins":300,"resetsAt":%s}},"planType":"plus"}}\n' $((now + 3960))
    else
      printf '%s\n' '{"error":{"code":-32600,"message":"codex account authentication required to read rate limits"},"id":2}'
    fi ;;
  *)
    printf '%s\n' '{"id":2,"result":{"rateLimits":{"secondary":{"usedPercent":5,"windowDurationMins":10080}},"planType":"pro\u001b[2J\u202eevil"}}' ;;
esac
exec cat >/dev/null
"#;

const ACCOUNTS: [&str; 6] = ["ffarmer", "ggrant", "hhale", "jjones", "kkim", "mmoss"];

struct Codex {
    f: Fixture,
    root: PathBuf,
    bin: PathBuf,
    run_log: PathBuf,
    pid_dir: PathBuf,
}

fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path and a plain mode.
    let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(rc, 0, "mkfifo {}", path.display());
}

/// An account directory whose login files would hang or fail on any read.
fn trapped(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    mkfifo(&dir.join("auth.json"));
    let cfg = dir.join("config.toml");
    fs::write(&cfg, "model = \"never-read\"\n").unwrap();
    fs::set_permissions(&cfg, fs::Permissions::from_mode(0o000)).unwrap();
}

impl Codex {
    /// The fixture with `names` as Codex accounts under `<tmp>/codex-profiles`.
    fn with(names: &[&str]) -> Self {
        let f = Fixture::new();
        f.assert_isolated();
        let base = f.home.parent().unwrap().to_path_buf();
        let root = base.join("codex-profiles");
        fs::create_dir_all(&root).unwrap();
        for n in names {
            trapped(&root.join(n));
        }
        let bin = base.join("codex-bin").join("codex");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        write_executable(&bin, FAKE_CODEX, 0o755);
        let pid_dir = base.join("codex-pids");
        fs::create_dir_all(&pid_dir).unwrap();
        Codex {
            f,
            root,
            bin,
            run_log: base.join("codex-runs.log"),
            pid_dir,
        }
    }

    fn new() -> Self {
        Self::with(&ACCOUNTS)
    }

    fn home_codex(&self) -> PathBuf {
        self.f.home.join(".codex")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut full = vec!["profile", "usage"];
        full.extend_from_slice(args);
        let mut c = self.f.cmd(&full);
        c.env("CODEX_PROFILES_ROOT", &self.root)
            .env("CODEX_BIN", &self.bin)
            .env("AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS", "1")
            .env("CODEX_RUN_LOG", &self.run_log)
            .env("CODEX_PID_DIR", &self.pid_dir)
            .env("FAKE_HOME_STATE", "nosign");
        c
    }

    fn runs(&self) -> Vec<String> {
        fs::read_to_string(&self.run_log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    /// Every pid a fake wrote, and the background `sleep` of `kkim`.
    fn pids(&self) -> Vec<i32> {
        fs::read_dir(&self.pid_dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .collect()
    }
}

/// A pipe with `O_CLOEXEC` on both ends, so that a process started by
/// another test does not inherit it.
///
/// `std::io::pipe` is used because `libc::pipe2` does not exist on macOS,
/// so the previous spelling did not build there (#2210). How std sets the
/// flag differs by platform:
///
/// - On Linux (and the BSDs) std calls `pipe2(O_CLOEXEC)`, so the flag is
///   set atomically and no other thread can fork in between.
/// - On macOS std calls `pipe()` and then sets `FD_CLOEXEC` on each end.
///   A child forked by another test in that gap inherits the write end
///   across `exec`, and [`run`]'s wait for end-of-file then lasts until
///   that unrelated child exits. macOS has no atomic alternative, so the
///   window remains there. These tests run in Linux CI, where it does not
///   arise.
fn cloexec_pipe() -> (OwnedFd, OwnedFd) {
    let (r, w) = std::io::pipe().expect("pipe");
    (OwnedFd::from(r), OwnedFd::from(w))
}

/// Runs `c` until it and every process it started have exited, and fails
/// the test if that has not happened after [`DEADLINE`].
///
/// The command gets the write end of a pipe as descriptor 9 without
/// `FD_CLOEXEC`, so every Codex app server it starts, and anything those
/// start, holds it too. The read end reaches EOF exactly when the last of
/// them has exited, reaped or not, so no pid is probed with `kill(pid, 0)`
/// (which succeeds for a zombie) and nothing is polled. The write end in
/// this process is closed right after the spawn; only the child's copy
/// remains.
fn run(mut c: Command) -> Output {
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (witness_r, witness_w) = cloexec_pipe();
    let w = witness_w.as_raw_fd();
    // SAFETY: `dup2` is async-signal-safe; it installs the write end as
    // descriptor 9, which `dup2` leaves without `FD_CLOEXEC`.
    unsafe {
        c.pre_exec(move || {
            if libc::dup2(w, WITNESS_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = c.spawn().expect("start amplihack");
    drop(witness_w);
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let o = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let e = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    // The thread waits for the exit event without reaping (`WNOWAIT`), so
    // the pid stays this command's and `child` can still kill it below.
    let pid = child.id() as libc::id_t;
    let (tx, done) = mpsc::channel();
    std::thread::spawn(move || {
        // SAFETY: a zeroed `siginfo_t` is a valid out-parameter.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: waits for this test's own unreaped child; reaps nothing.
        let rc =
            unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) };
        assert_eq!(rc, 0, "waitid: {}", std::io::Error::last_os_error());
        let mut witness = fs::File::from(witness_r);
        let mut sink = Vec::new();
        let _ = witness.read_to_end(&mut sink);
        let _ = tx.send(());
    });
    if done.recv_timeout(DEADLINE).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        panic!(
            "profile usage or a process it started was still running after {DEADLINE:?} \
             (a FIFO read, or a Codex app server left behind)"
        );
    }
    let status = child.wait().expect("reap amplihack");
    Output {
        status,
        stdout: o.join().unwrap(),
        stderr: e.join().unwrap(),
    }
}

fn ok(c: Command) -> Output {
    let out = run(c);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        text(&out.stdout),
        text(&out.stderr)
    );
    assert_no_secrets(&out);
    out
}

fn json(out: &Output) -> serde_json::Value {
    let s = text(&out.stdout);
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("stdout must be only JSON ({e}): {s:?}"))
}

fn codex_rows(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v["codex"].as_array().expect("a `codex` array").clone()
}

fn field<'a>(rows: &'a [serde_json::Value], key: &str) -> Vec<&'a str> {
    rows.iter()
        .map(|r| r[key].as_str().unwrap_or("-"))
        .collect()
}

/// The Codex block of a text run: from its header to the end of stdout.
fn codex_block(stdout: &str) -> &str {
    let at = stdout
        .find("Codex accounts under ")
        .or_else(|| stdout.find("Codex: no accounts under "))
        .unwrap_or_else(|| panic!("no Codex block:\n{stdout}"));
    &stdout[at..]
}

fn line_of<'a>(block: &'a str, label: &str) -> &'a str {
    block
        .lines()
        .find(|l| l.starts_with(&format!("{label} ")))
        .unwrap_or_else(|| panic!("no line for {label}:\n{block}"))
}

// ---------------------------------------------------------------------------
// --json stays the bare array of Claude rows
// ---------------------------------------------------------------------------

/// Host scripts index `--json` with `.[]` and `.[0]` and send stderr to
/// /dev/null, so an object there would make them report no capacity without
/// a message. `--json` is the array of Claude rows it always was, and starts
/// no Codex; the Codex rows are in `--summary-json`.
#[test]
fn json_stays_an_array_of_claude_rows_and_reads_no_codex() {
    let c = Codex::new();
    let v = json(&ok(c.cmd(&["--json"])));
    let rows = v.as_array().unwrap_or_else(|| panic!("an array: {v}"));
    let mut got: Vec<&str> = rows
        .iter()
        .map(|r| r["profile"].as_str().unwrap())
        .collect();
    got.sort_unstable();
    let mut want = PROFILES.to_vec();
    want.sort_unstable();
    assert_eq!(got, want, "{v}");
    assert!(c.runs().is_empty(), "no Codex was started: {:?}", c.runs());
}

// ---------------------------------------------------------------------------
// --summary-json
// ---------------------------------------------------------------------------

#[test]
fn summary_json_lists_every_account_in_order_with_its_state() {
    let c = Codex::new();
    let out = ok(c.cmd(&["--summary-json"]));
    let v = json(&out);
    assert!(v["profiles"].is_array(), "{v}");
    assert!(!v["profiles"].as_array().unwrap().is_empty(), "{v}");
    let rows = codex_rows(&v);
    assert_eq!(field(&rows, "account"), ACCOUNTS);
    assert_eq!(
        field(&rows, "state"),
        ["ok", "ok", "ok", "not_signed_in", "timeout", "error"]
    );
    for (r, name) in rows.iter().zip(ACCOUNTS) {
        assert_eq!(
            r["path"].as_str().unwrap(),
            c.root.join(name).to_str().unwrap()
        );
        let read_at = r["read_at"].as_str().unwrap();
        assert!(
            chrono::DateTime::parse_from_rfc3339(read_at).is_ok() && read_at.ends_with('Z'),
            "{read_at}"
        );
    }
    assert!(rows[3]["error"].is_null());
    assert_eq!(rows[4]["error"], "timed out after 1s");
    assert_eq!(
        rows[5]["error"],
        "codex app-server exited (exit status: 1): error: unrecognized subcommand 'app-server'"
    );
    for r in &rows[3..] {
        assert_eq!(r["windows"], serde_json::json!([]), "{r}");
        assert!(r["credits"].is_null(), "{r}");
    }
}

#[test]
fn json_rows_carry_the_live_figures() {
    let c = Codex::new();
    let rows = codex_rows(&json(&ok(c.cmd(&["--summary-json"]))));
    let ff = &rows[0];
    assert_eq!(ff["windows"].as_array().unwrap().len(), 2);
    assert_eq!(ff["windows"][0]["name"], "primary");
    assert_eq!(ff["windows"][0]["used_percent"], 12.0);
    assert_eq!(ff["windows"][0]["window_minutes"], 300);
    assert!(ff["windows"][0]["resets_at"].is_i64());
    assert!(
        ff["windows"][0]["resets_at_rfc3339"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );
    assert_eq!(ff["windows"][1]["window_minutes"], 10_080);
    assert_eq!(ff["plan_type"], "plus");
    assert_eq!(ff["ordinary_usage_allowed"], true);
    let gg = &rows[1];
    assert_eq!(gg["windows"].as_array().unwrap().len(), 1, "weekly only");
    assert_eq!(gg["windows"][0]["name"], "secondary");
    assert_eq!(gg["windows"][0]["used_percent"], 34.0);
    let hh = &rows[2];
    assert_eq!(hh["windows"], serde_json::json!([]), "credits only");
    assert_eq!(
        hh["credits"],
        serde_json::json!({"has_credits": true, "balance": 4200.0})
    );
    assert_eq!(hh["plan_type"], "team");
}

#[test]
fn the_next_choice_is_marked_on_exactly_one_row() {
    let c = Codex::new();
    let rows = codex_rows(&json(&ok(c.cmd(&["--summary-json"]))));
    let marked: Vec<&str> = rows
        .iter()
        .filter(|r| r["next_choice"] == true)
        .map(|r| r["account"].as_str().unwrap())
        .collect();
    assert_eq!(marked, ["ggrant"], "room 66 beats 59");
}

#[test]
fn summary_json_adds_the_same_codex_rows_after_the_summary() {
    let c = Codex::new();
    let out = ok(c.cmd(&["--summary-json"]));
    let s = text(&out.stdout);
    let v = json(&out);
    assert!(v["profiles"].is_array(), "the Claude rows keep their key");
    let rows = codex_rows(&v);
    assert_eq!(field(&rows, "account"), ACCOUNTS);
    assert!(s.find("\"summary\"").unwrap() < s.find("\"codex\"").unwrap());
}

// ---------------------------------------------------------------------------
// Text views
// ---------------------------------------------------------------------------

#[test]
fn the_bar_view_prints_the_codex_block_after_the_claude_output() {
    let c = Codex::new();
    let out = ok(c.cmd(&[]));
    let s = text(&out.stdout);
    let header = format!("Codex accounts under {}\n", c.root.display());
    let at = s.find(&header).unwrap_or_else(|| panic!("{s}"));
    assert!(s[..at].ends_with("\n\n"), "one empty line before it:\n{s}");
    for p in PROFILES {
        assert!(s[..at].contains(p), "Claude rows first: {s}");
    }
    let block = codex_block(&s);
    assert!(line_of(block, "ggrant").ends_with("<- next"), "{block}");
    assert!(line_of(block, "ffarmer").contains("5h █"), "{block}");
    assert!(line_of(block, "ffarmer").contains("7d ████"), "{block}");
    assert!(line_of(block, "hhale").contains("credits 4200"), "{block}");
    assert!(
        line_of(block, "jjones").contains("not signed in"),
        "{block}"
    );
    assert!(
        line_of(block, "kkim").contains("timed out      after 1s"),
        "{block}"
    );
    assert!(
        line_of(block, "mmoss").contains("error: codex app-server exited (exit status: 1)"),
        "{block}"
    );
    assert_eq!(block.matches("<- next").count(), 1, "{block}");
    for l in block.lines().skip(1) {
        assert!(l.contains("read "), "every row has its read time: {l}");
    }
}

#[test]
fn the_table_view_prints_the_block_without_bars() {
    let c = Codex::new();
    let s = text(&ok(c.cmd(&["--table"])).stdout);
    let block = codex_block(&s);
    assert!(!block.contains('█') && !block.contains('░'), "{block}");
    assert!(line_of(block, "ffarmer").contains("5h 12% "), "{block}");
    assert!(line_of(block, "ggrant").ends_with("<- next"), "{block}");
}

#[test]
fn utc_applies_to_the_codex_times() {
    let c = Codex::new();
    let s = text(&ok(c.cmd(&["--utc"])).stdout);
    let block = codex_block(&s);
    for l in block.lines().skip(1) {
        let read = l.split("read ").nth(1).unwrap_or_else(|| panic!("{l}"));
        assert!(
            read.split_whitespace().next().unwrap().ends_with('Z'),
            "{l}"
        );
    }
}

// ---------------------------------------------------------------------------
// Credentials and processes
// ---------------------------------------------------------------------------

#[test]
fn no_login_file_is_opened_and_no_api_key_reaches_codex() {
    let c = Codex::new();
    let mut cmd = c.cmd(&["--summary-json"]);
    cmd.env("OPENAI_API_KEY", OPENAI_SENTINEL)
        .env("CODEX_API_KEY", CODEX_KEY_SENTINEL)
        .env("RUST_LOG", "trace");
    let out = run(cmd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    for s in [text(&out.stdout), text(&out.stderr)] {
        assert!(!s.contains(OPENAI_SENTINEL), "{s}");
        assert!(!s.contains(CODEX_KEY_SENTINEL), "{s}");
    }
    let rows = codex_rows(&json(&out));
    assert_eq!(rows[0]["state"], "ok", "the key check refused: {}", rows[0]);
    assert_eq!(rows[1]["state"], "ok", "{}", rows[1]);
    // The login files are where they were, untouched.
    for n in ACCOUNTS {
        let meta = fs::symlink_metadata(c.root.join(n).join("auth.json")).unwrap();
        assert!(std::os::unix::fs::FileTypeExt::is_fifo(&meta.file_type()));
    }
}

#[test]
fn no_codex_process_outlives_the_command() {
    let c = Codex::new();
    // `run` returns only once the command and every process it started,
    // including `kkim`'s app server and its background `sleep 600`, have
    // exited (see its witness pipe). The pid files show the fakes ran.
    ok(c.cmd(&["--summary-json"]));
    let pids = c.pids();
    assert!(pids.len() > ACCOUNTS.len(), "{pids:?}");
}

#[test]
fn every_account_is_started_once() {
    let c = Codex::new();
    ok(c.cmd(&["--summary-json"]));
    let mut runs = c.runs();
    runs.sort();
    let mut want: Vec<String> = ACCOUNTS.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(runs, want);
}

#[test]
fn codex_failures_never_change_the_exit_status() {
    let c = Codex::with(&["jjones", "kkim", "mmoss"]);
    let out = run(c.cmd(&[]));
    assert!(out.status.success(), "{}", text(&out.stderr));
    let missing = Codex::new();
    let mut cmd = missing.cmd(&["--summary-json"]);
    cmd.env("CODEX_BIN", missing.root.join("no-such-codex"));
    let out = run(cmd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let rows = codex_rows(&json(&out));
    for r in &rows {
        assert_eq!(r["state"], "error", "{r}");
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .starts_with("codex binary not found: "),
            "{r}"
        );
    }
}

// ---------------------------------------------------------------------------
// Which accounts are read
// ---------------------------------------------------------------------------

#[test]
fn home_codex_is_listed_last_when_signed_in() {
    let c = Codex::with(&["ggrant"]);
    trapped(&c.home_codex());
    let mut cmd = c.cmd(&["--summary-json"]);
    cmd.env("FAKE_HOME_STATE", "ok");
    let rows = codex_rows(&json(&ok(cmd)));
    assert_eq!(field(&rows, "account"), ["ggrant", "~/.codex"]);
    assert_eq!(
        rows[1]["path"].as_str().unwrap(),
        c.home_codex().to_str().unwrap()
    );
    assert_eq!(rows[1]["state"], "ok");
}

#[test]
fn home_codex_is_dropped_when_not_signed_in() {
    let c = Codex::with(&["ggrant"]);
    trapped(&c.home_codex());
    let rows = codex_rows(&json(&ok(c.cmd(&["--summary-json"]))));
    assert_eq!(field(&rows, "account"), ["ggrant"]);
    assert!(c.runs().contains(&".codex".to_string()), "it was read");
    let s = text(&ok(c.cmd(&[])).stdout);
    assert!(!codex_block(&s).contains("~/.codex"), "{s}");
}

#[test]
fn home_codex_linked_to_a_profile_is_not_read_twice() {
    let c = Codex::with(&["ggrant"]);
    std::os::unix::fs::symlink(c.root.join("ggrant"), c.home_codex()).unwrap();
    let mut cmd = c.cmd(&["--summary-json"]);
    cmd.env("FAKE_HOME_STATE", "ok");
    let rows = codex_rows(&json(&ok(cmd)));
    assert_eq!(field(&rows, "account"), ["ggrant"]);
    assert_eq!(c.runs(), ["ggrant"]);
}

#[test]
fn hidden_directories_and_files_in_the_root_are_not_accounts() {
    let c = Codex::with(&["ggrant", ".hidden"]);
    fs::write(c.root.join("notes.txt"), "not an account").unwrap();
    let rows = codex_rows(&json(&ok(c.cmd(&["--summary-json"]))));
    assert_eq!(field(&rows, "account"), ["ggrant"]);
}

#[test]
fn the_default_root_is_home_codex_profiles() {
    let c = Codex::with(&[]);
    let default_root = c.f.home.join(".codex-profiles");
    trapped(&default_root.join("ggrant"));
    let mut cmd = c.cmd(&["--summary-json"]);
    cmd.env_remove("CODEX_PROFILES_ROOT");
    let rows = codex_rows(&json(&ok(cmd)));
    assert_eq!(field(&rows, "account"), ["ggrant"]);
    assert_eq!(
        rows[0]["path"].as_str().unwrap(),
        default_root.join("ggrant").to_str().unwrap()
    );
}

#[test]
fn names_given_select_claude_profiles_only() {
    let c = Codex::new();
    let v = json(&ok(c.cmd(&["--json", PROFILES[0]])));
    assert_eq!(v.as_array().unwrap().len(), 1, "{v}");
    let v = json(&ok(c.cmd(&["--summary-json", PROFILES[0]])));
    assert_eq!(v["codex"], serde_json::json!([]));
    let s = text(&ok(c.cmd(&[PROFILES[0]])).stdout);
    assert!(!s.contains("Codex"), "{s}");
    assert!(c.runs().is_empty(), "no Codex was started: {:?}", c.runs());
}

#[test]
fn an_empty_or_missing_root_is_one_line_and_an_empty_array() {
    let c = Codex::with(&[]);
    let s = text(&ok(c.cmd(&[])).stdout);
    assert!(
        s.contains(&format!(
            "\n\nCodex: no accounts under {}\n",
            c.root.display()
        )),
        "{s}"
    );
    assert_eq!(
        json(&ok(c.cmd(&["--summary-json"])))["codex"],
        serde_json::json!([])
    );

    let missing = c.root.join("missing");
    let mut cmd = c.cmd(&[]);
    cmd.env("CODEX_PROFILES_ROOT", &missing);
    let out = ok(cmd);
    assert!(
        text(&out.stdout).contains(&format!("Codex: no accounts under {}", missing.display())),
        "{}",
        text(&out.stdout)
    );
    assert!(c.runs().is_empty());
}

#[test]
fn an_unreadable_root_warns_and_still_prints_the_claude_output() {
    let c = Codex::with(&["ggrant"]);
    fs::set_permissions(&c.root, fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = fs::read_dir(&c.root).is_ok();
    let text_out = run(c.cmd(&[]));
    let json_out = run(c.cmd(&["--summary-json"]));
    fs::set_permissions(&c.root, fs::Permissions::from_mode(0o700)).unwrap();
    if readable_anyway {
        // Running as root: the mode does not stop the listing.
        return;
    }
    let warning = format!(
        "warning: cannot list Codex accounts under {}: ",
        c.root.display()
    );
    for out in [&text_out, &json_out] {
        assert!(out.status.success(), "exit {:?}", out.status.code());
        assert_no_secrets(out);
        let err = text(&out.stderr);
        assert_eq!(err.matches(&warning).count(), 1, "{err}");
    }
    let s = text(&text_out.stdout);
    for p in PROFILES {
        assert!(s.contains(p), "the Claude rows are still printed: {s}");
    }
    assert!(
        s.contains(&format!("Codex: no accounts under {}\n", c.root.display())),
        "{s}"
    );
    assert_eq!(json(&json_out)["codex"], serde_json::json!([]));
    assert!(c.runs().is_empty(), "no Codex was started: {:?}", c.runs());
}

// ---------------------------------------------------------------------------
// Settings and cleaning
// ---------------------------------------------------------------------------

#[test]
fn a_bad_timeout_warns_once_and_uses_20_seconds() {
    let c = Codex::with(&["ggrant"]);
    let mut cmd = c.cmd(&["--summary-json"]);
    cmd.env("AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS", "0");
    let out = ok(cmd);
    let err = text(&out.stderr);
    assert_eq!(err.matches(TIMEOUT_WARNING).count(), 1, "{err}");
    assert_eq!(codex_rows(&json(&out))[0]["state"], "ok");
}

#[test]
fn labels_and_server_strings_cannot_send_terminal_escapes() {
    let c = Codex::with(&[]);
    let evil = "ev\u{1b}[31mil\u{202e}x";
    trapped(&c.root.join(evil));
    let s = text(&ok(c.cmd(&[])).stdout);
    let block = codex_block(&s);
    for bad in ['\u{1b}', '\u{202e}'] {
        assert!(!block.contains(bad), "{bad:?} in {block:?}");
    }
    assert!(block.contains("plan pro"), "{block}");

    let out = ok(c.cmd(&["--summary-json"]));
    assert!(!text(&out.stdout).contains('\u{1b}'), "JSON escapes it");
    let rows = codex_rows(&json(&out));
    assert_eq!(rows[0]["account"], evil, "the name as it is");
    let plan = rows[0]["plan_type"].as_str().unwrap();
    assert!(plan.starts_with("pro"), "{plan:?}");
    assert!(
        !plan.contains('\u{1b}') && !plan.contains('\u{202e}'),
        "cleaned before it is stored: {plan:?}"
    );
}

// ---------------------------------------------------------------------------
// Issue #2271: the Claude output never waits for Codex
//
// The `gated` account's app server answers only once the test closes its
// end of a FIFO. In the bar and table views the whole Claude block, up to
// the last line of the capacity summary, must reach stdout while it is
// still held; only then is the gate opened and the Codex block read.
// `--summary-json` stays one document that includes the Codex rows, and
// `--json` starts no Codex at all (`json_stays_an_array_of_claude_rows_and_
// reads_no_codex`). Order is shown by events (lines read while the gate is
// closed), never by time; DEADLINE only reports a hang.
// ---------------------------------------------------------------------------

/// The FIFO the `gated` fake reads. Opening it for writing returns once the
/// fake has opened it for reading; closing it lets the fake answer.
struct Gate {
    path: PathBuf,
    writer: Option<fs::File>,
}

impl Gate {
    fn new(dir: &Path) -> Self {
        let path = dir.join("codex-gate");
        mkfifo(&path);
        Gate { path, writer: None }
    }

    /// Takes the write end, which opens once the fake is waiting on the
    /// FIFO. Not opening within DEADLINE is a hang.
    fn hold(&mut self) {
        if self.writer.is_some() {
            return;
        }
        let path = self.path.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fs::OpenOptions::new().write(true).open(&path));
        });
        match rx.recv_timeout(DEADLINE) {
            Ok(w) => self.writer = Some(w.expect("open the gate")),
            Err(_) => panic!("the gated Codex account never waited on its gate"),
        }
    }

    /// Opens the gate: holds the write end and closes it, so the fake
    /// reads end-of-file and answers.
    fn open(&mut self) {
        self.hold();
        self.writer = None;
    }

    /// Never blocks: lets a waiting fake go, if there is one.
    fn release_if_waiting(&mut self) {
        use std::os::unix::fs::OpenOptionsExt;
        self.writer = None;
        if let Ok(w) = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.path)
        {
            drop(w);
        }
    }
}

/// A started `profile usage` whose stdout is read line by line on its own
/// thread. On drop it lets a held fake go and kills and reaps the command,
/// so a failed test leaves nothing running.
struct Running {
    child: std::process::Child,
    lines: mpsc::Receiver<String>,
    gate: Gate,
    stderr: Option<std::thread::JoinHandle<Vec<u8>>>,
}

impl Running {
    fn spawn(mut cmd: Command, gate: Gate) -> Self {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("start amplihack");
        let out = child.stdout.take().unwrap();
        let mut err = child.stderr.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for l in std::io::BufReader::new(out).lines() {
                let Ok(l) = l else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let stderr = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = err.read_to_end(&mut b);
            b
        });
        Running {
            child,
            lines,
            gate,
            stderr: Some(stderr),
        }
    }

    /// The next stdout line; `None` at the end of stdout. A line that does
    /// not come within DEADLINE is a hang and fails the test.
    fn next(&self, waiting_for: &str) -> Option<String> {
        match self.lines.recv_timeout(DEADLINE) {
            Ok(l) => Some(l),
            Err(mpsc::RecvTimeoutError::Disconnected) => None,
            Err(mpsc::RecvTimeoutError::Timeout) => panic!(
                "no stdout line within {DEADLINE:?} while waiting for {waiting_for}: \
                 the Claude output is held back until Codex answers"
            ),
        }
    }

    /// Reads stdout up to and including the first line `last` accepts.
    fn read_until(&self, what: &str, last: impl Fn(&str) -> bool) -> Vec<String> {
        let mut seen = Vec::new();
        loop {
            let l = self
                .next(what)
                .unwrap_or_else(|| panic!("stdout ended before {what}:\n{}", seen.join("\n")));
            let done = last(&l);
            seen.push(l);
            if done {
                return seen;
            }
        }
    }

    /// Opens the gate, reads the rest of stdout and waits for the exit.
    fn finish(mut self) -> (Vec<String>, std::process::ExitStatus, String) {
        self.gate.open();
        let mut rest = Vec::new();
        while let Some(l) = self.next("the end of stdout") {
            rest.push(l);
        }
        let status = self.child.wait().expect("reap amplihack");
        let err = self.stderr.take().unwrap().join().unwrap();
        (rest, status, text(&err))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.gate.release_if_waiting();
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// `gated` as the only Codex account, with a Codex deadline long enough
/// that only the gate decides when it answers.
fn gated(args: &[&str]) -> (Codex, Running) {
    let c = Codex::with(&["gated"]);
    let gate = Gate::new(c.f.home.parent().unwrap());
    let mut cmd = c.cmd(args);
    cmd.env("CODEX_GATE", &gate.path)
        .env("AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS", "300");
    let r = Running::spawn(cmd, gate);
    (c, r)
}

fn is_codex_header(l: &str) -> bool {
    l.starts_with("Codex accounts under ") || l.starts_with("Codex: ")
}

/// The capacity summary's last line (issue #1797).
fn is_summary_end(l: &str) -> bool {
    l.starts_with("unknown ")
}

fn assert_claude_block_before_codex(args: &[&str]) {
    let (c, r) = gated(args);
    let before = r.read_until("the end of the capacity summary", is_summary_end);
    assert!(
        !before.iter().any(|l| is_codex_header(l)),
        "Codex printed before it answered:\n{}",
        before.join("\n")
    );
    let shown = before.join("\n");
    for p in PROFILES {
        assert!(shown.contains(p), "every Claude row first: {shown}");
    }
    assert!(
        before.iter().any(|l| l.starts_with("order: ")),
        "the Claude footer first: {shown}"
    );
    let (rest, status, stderr) = r.finish();
    assert!(status.success(), "exit {:?}: {stderr}", status.code());
    let header = rest
        .iter()
        .position(|l| is_codex_header(l))
        .unwrap_or_else(|| panic!("no Codex block after the gate:\n{}", rest.join("\n")));
    assert!(
        rest[..header].iter().all(|l| l.is_empty()),
        "only the empty line before the Codex block: {rest:?}"
    );
    let block = rest[header..].join("\n");
    assert!(block.contains("gated"), "{block}");
    assert!(!block.contains("timed out"), "it answered: {block}");
    assert_eq!(c.runs(), ["gated"]);
}

#[test]
fn the_bar_view_prints_the_claude_block_before_codex_answers() {
    assert_claude_block_before_codex(&[]);
}

#[test]
fn the_table_prints_the_claude_block_before_codex_answers() {
    assert_claude_block_before_codex(&["--table"]);
}

#[test]
fn summary_json_is_one_document_with_codex_after_the_gate() {
    let (c, mut r) = gated(&["--summary-json"]);
    // The write end opens only once the fake is waiting on the FIFO: Codex
    // has not answered yet, so nothing may be on stdout.
    r.gate.hold();
    assert!(
        r.lines.try_recv().is_err(),
        "--summary-json printed before Codex answered"
    );
    let (rest, status, stderr) = r.finish();
    assert!(status.success(), "exit {:?}: {stderr}", status.code());
    let s = rest.join("\n");
    let v: serde_json::Value =
        serde_json::from_str(&s).unwrap_or_else(|e| panic!("one JSON document ({e}): {s}"));
    assert!(v["profiles"].is_array(), "{v}");
    let rows = codex_rows(&v);
    assert_eq!(field(&rows, "account"), ["gated"]);
    assert_eq!(field(&rows, "state"), ["ok"]);
    assert_eq!(c.runs(), ["gated"]);
}
