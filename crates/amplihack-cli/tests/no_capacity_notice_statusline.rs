//! The no-capacity notice on the status line: `amplihack profile statusline`
//! passes the original status line through byte for byte, and only when every
//! account is over the threshold (or at its limit) appends one warning line.
//!
//! Runs the real `amplihack` binary with an isolated `HOME` and profile root
//! (selected with `AMPLIHACK_CLAUDE_PROFILE_ROOT`) and a fake `sh` original
//! status-line command. Nothing touches the real `~/.claude`,
//! `~/.claude-profiles` or `~/.amplihack`.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use amplihack_utils::claude_capacity::{self as cap, CapacityFile, State};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const HOUR: i64 = 3600;
const PREFIX_LOW: &str = "⚠ all Claude accounts ≥90% · next reset ";
const PREFIX_EXHAUSTED: &str = "⚠ all Claude accounts at limit · next reset ";

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    work: PathBuf,
}

impl Fixture {
    /// Profiles `names` under a private root, each with `original` as its
    /// status-line command, and the wrapper installed.
    fn new(names: &[&str], original: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let root = home.join("profiles");
        let work = tmp.path().join("work");
        for d in [&home, &work] {
            fs::create_dir_all(d).unwrap();
        }
        let mut f = Self {
            _tmp: tmp,
            home,
            root,
            work,
        };
        for n in names {
            f.ok(&["profile", "add", n], "0");
        }
        f.root = f.root.canonicalize().unwrap();
        for n in names {
            let settings = serde_json::json!({
                "statusLine": {"type": "command", "command": original}
            });
            fs::write(
                f.root.join(n).join("settings.json"),
                serde_json::to_vec_pretty(&settings).unwrap(),
            )
            .unwrap();
        }
        f.ok(&["profile", "auto-switch", "install"], "0");
        f
    }

    fn cmd(&self, args: &[&str], toggle: &str) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("TZ", "UTC")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", toggle)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root);
        c
    }

    fn ok(&self, args: &[&str], toggle: &str) -> Output {
        let out = self.cmd(args, toggle).output().unwrap();
        assert!(
            out.status.success(),
            "`amplihack {}` failed: {}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// One status-line tick for a session in `owner`. With `session` false,
    /// `CLAUDE_CONFIG_DIR` is unset, so no reading is recorded and only the
    /// planted capacity file decides.
    fn tick(&self, owner: &str, toggle: &str, json: &str, session: bool) -> Output {
        let root = self.root.to_str().unwrap();
        let mut c = self.cmd(
            &["profile", "statusline", "--root", root, "--owner", owner],
            toggle,
        );
        if session {
            c.env("CLAUDE_CONFIG_DIR", self.root.join(owner));
        }
        let mut child = c
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(json.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn plant(&self, state: State, reset: Option<i64>) -> CapacityFile {
        let f = CapacityFile {
            version: 1,
            state,
            threshold: 90,
            over: vec!["X".into()],
            earliest_reset: reset.and_then(|r| chrono::DateTime::from_timestamp(r, 0)),
            written_at: chrono::DateTime::from_timestamp(now(), 0).unwrap(),
            writer: "statusline".into(),
        };
        let dir = self.root.join(cap::STATE_DIR);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let p = cap::path(&self.root);
        fs::write(&p, serde_json::to_vec(&f).unwrap()).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        f
    }

    fn capacity(&self) -> PathBuf {
        cap::path(&self.root)
    }
}

/// Status-line input like Claude Code's, with the given usage.
fn input(five_hour: f64, seven_day: f64) -> String {
    format!(
        "{{\"session_id\":\"s\",\"model\":{{\"id\":\"m\"}},\"rate_limits\":{{\
         \"five_hour\":{{\"used_percentage\":{five_hour},\"resets_at\":{}}},\
         \"seven_day\":{{\"used_percentage\":{seven_day},\"resets_at\":{}}}}}}}",
        now() + HOUR,
        now() + 3 * 86_400
    )
}

/// A reset on the hour, a few hours out, and how the warning shows it in UTC.
fn round_reset() -> (i64, String) {
    let r = (now() / HOUR + 3) * HOUR;
    let shown = chrono::DateTime::from_timestamp(r, 0)
        .unwrap()
        .format("%b %-d %-I%P")
        .to_string();
    (r, shown)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Splits `stdout` into the original output and the appended warning line.
fn split_warning(stdout: &[u8]) -> (&[u8], &str) {
    let body = stdout
        .strip_suffix(b"\n")
        .unwrap_or_else(|| panic!("the warning ends with a newline: {:?}", text(stdout)));
    let at = body.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    (
        &stdout[..at],
        std::str::from_utf8(&body[at..]).expect("the warning is UTF-8"),
    )
}

// ---------------------------------------------------------------------------
// Off or ok: byte-identical
// ---------------------------------------------------------------------------

#[test]
fn off_is_byte_identical_even_with_an_exhausted_file() {
    for original in ["printf 'abc'", "printf 'abc\\n'"] {
        let f = Fixture::new(&["X"], original);
        f.plant(State::Exhausted, Some(now() + HOUR));
        let want = if original.ends_with("\\n'") {
            &b"abc\n"[..]
        } else {
            b"abc"
        };
        let out = f.tick("X", "0", &input(100.0, 100.0), true);
        assert_eq!(out.stdout, want, "{original}: {}", text(&out.stdout));
        let out = f.tick("X", "0", &input(100.0, 100.0), false);
        assert_eq!(out.stdout, want, "{original} (no session)");
    }
}

#[test]
fn ok_is_byte_identical() {
    for (original, want) in [("printf 'abc'", &b"abc"[..]), ("printf 'abc\\n'", b"abc\n")] {
        let f = Fixture::new(&["X", "Y"], original);
        let out = f.tick("X", "1", &input(20.0, 10.0), true);
        assert_eq!(out.stdout, want, "{original}: {}", text(&out.stdout));
        assert_eq!(out.status.code(), Some(0));
        f.plant(State::Ok, None);
        let out = f.tick("X", "1", &input(20.0, 10.0), false);
        assert_eq!(out.stdout, want, "{original} with an ok file");
    }
}

#[test]
fn a_corrupt_or_past_reset_file_is_byte_identical() {
    let f = Fixture::new(&["X"], "printf 'abc'");
    fs::write(f.capacity(), b"{corrupt").unwrap();
    fs::set_permissions(f.capacity(), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(f.tick("X", "1", &input(5.0, 5.0), false).stdout, b"abc");
    f.plant(State::Exhausted, Some(now() - 60));
    assert_eq!(
        f.tick("X", "1", &input(5.0, 5.0), false).stdout,
        b"abc",
        "the reset has passed"
    );
}

// ---------------------------------------------------------------------------
// Low or exhausted: original output, then one warning line
// ---------------------------------------------------------------------------

#[test]
fn low_appends_the_warning_with_or_without_a_trailing_newline() {
    let (reset, shown) = round_reset();
    let line = format!("{PREFIX_LOW}{shown}\n");
    for (original, body) in [("printf 'abc'", "abc\n"), ("printf 'abc\\n'", "abc\n")] {
        let f = Fixture::new(&["X"], original);
        f.plant(State::Low, Some(reset));
        let out = f.tick("X", "1", &input(5.0, 5.0), false);
        assert_eq!(text(&out.stdout), format!("{body}{line}"), "{original}");
        assert_eq!(out.status.code(), Some(0));
    }
}

#[test]
fn exhausted_appends_the_at_limit_warning() {
    let (reset, shown) = round_reset();
    let f = Fixture::new(&["X"], "printf 'abc'");
    f.plant(State::Exhausted, Some(reset));
    let out = f.tick("X", "1", &input(5.0, 5.0), false);
    assert_eq!(
        text(&out.stdout),
        format!("abc\n{PREFIX_EXHAUSTED}{shown}\n")
    );
}

#[test]
fn an_unknown_reset_says_so() {
    let f = Fixture::new(&["X"], "printf 'abc'");
    f.plant(State::Exhausted, None);
    let out = f.tick("X", "1", &input(5.0, 5.0), false);
    assert_eq!(
        text(&out.stdout),
        "abc\n⚠ all Claude accounts at limit · next reset unknown\n"
    );
}

#[test]
fn empty_original_output_gets_just_the_warning() {
    let (reset, shown) = round_reset();
    let f = Fixture::new(&["X"], "true");
    f.plant(State::Low, Some(reset));
    let out = f.tick("X", "1", &input(5.0, 5.0), false);
    assert_eq!(text(&out.stdout), format!("{PREFIX_LOW}{shown}\n"));
}

#[test]
fn the_exit_status_passes_through_with_the_warning() {
    let (reset, _) = round_reset();
    let f = Fixture::new(&["X"], "printf 'abc'; exit 3");
    f.plant(State::Low, Some(reset));
    let out = f.tick("X", "1", &input(5.0, 5.0), false);
    assert_eq!(out.status.code(), Some(3));
    let (original, warning) = split_warning(&out.stdout);
    assert_eq!(original, b"abc\n");
    assert!(warning.starts_with(PREFIX_LOW), "{warning}");
}

#[test]
fn binary_and_large_output_pass_through_unchanged_before_the_warning() {
    let (reset, _) = round_reset();
    let f = Fixture::new(
        &["X"],
        "head -c 1048576 /dev/zero | tr '\\000' 'x'; printf '\\001\\377 tail'",
    );
    f.plant(State::Exhausted, Some(reset));
    let out = f.tick("X", "1", &input(5.0, 5.0), false);
    let mut want = vec![b'x'; 1 << 20];
    want.extend_from_slice(b"\x01\xff tail");
    assert!(out.stdout.len() > want.len(), "{} bytes", out.stdout.len());
    assert_eq!(
        &out.stdout[..want.len()],
        &want[..],
        "byte for byte, no cap"
    );
    let (_, warning) = split_warning(&out.stdout);
    assert!(warning.starts_with(PREFIX_EXHAUSTED), "{warning}");
}

#[test]
fn the_original_stderr_is_not_mixed_into_stdout() {
    let (reset, _) = round_reset();
    let f = Fixture::new(&["X"], "printf 'abc'; printf 'oops' >&2");
    f.plant(State::Low, Some(reset));
    let out = f.tick("X", "1", &input(5.0, 5.0), false);
    assert!(text(&out.stderr).contains("oops"));
    assert!(!text(&out.stdout).contains("oops"));
}

// ---------------------------------------------------------------------------
// End to end: the wrapper's own reading drives the notice
// ---------------------------------------------------------------------------

#[test]
fn a_reading_that_puts_every_account_over_shows_the_warning_on_that_tick() {
    let f = Fixture::new(&["X"], "printf 'abc'");
    let out = f.tick("X", "1", &input(95.0, 10.0), true);
    let (original, warning) = split_warning(&out.stdout);
    assert_eq!(original, b"abc\n", "{}", text(&out.stdout));
    assert!(warning.starts_with(PREFIX_LOW), "{warning}");
    assert!(
        !warning.ends_with("unknown"),
        "the reading has a reset: {warning}"
    );
    let written = cap::read(&f.root).expect("the recorder wrote capacity.json");
    assert_eq!(written.state, State::Low);
    assert_eq!(written.writer, "statusline");
    assert_eq!(
        fs::metadata(f.capacity()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn a_reading_at_the_limit_shows_the_exhausted_warning() {
    let f = Fixture::new(&["X"], "printf 'abc'");
    let out = f.tick("X", "1", &input(100.0, 10.0), true);
    let (_, warning) = split_warning(&out.stdout);
    assert!(warning.starts_with(PREFIX_EXHAUSTED), "{warning}");
}

#[test]
fn recovery_on_a_later_tick_clears_the_warning() {
    let f = Fixture::new(&["X"], "printf 'abc'");
    let first = f.tick("X", "1", &input(95.0, 10.0), true);
    assert_ne!(first.stdout, b"abc");
    // A newer, lower reading (the window reset).
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let later = f.tick("X", "1", &input(3.0, 10.0), true);
    assert_eq!(later.stdout, b"abc", "{}", text(&later.stdout));
}

#[test]
fn one_account_with_room_means_no_warning() {
    let f = Fixture::new(&["X", "Y"], "printf 'abc'");
    // Y has no reading at all: missing data never raises an alarm.
    let out = f.tick("X", "1", &input(99.0, 10.0), true);
    assert_eq!(out.stdout, b"abc", "{}", text(&out.stdout));
    assert!(!f.capacity().exists(), "an ok state never creates the file");
}
