//! Issue #2001: the live Codex rate-limit reader.
//!
//! Wired from `mod.rs` as `#[cfg(all(test, unix))] mod tests;`.
//!
//! The contract these tests pin (see docs/reference/profile-command.md,
//! "Codex accounts"):
//!
//! - `parse::{num, reading, is_not_signed_in}` (`pub(super)`): pure, total
//!   parsing of an `account/rateLimits/read` result.
//! - `rpc::group_target(pid: u32) -> Option<i32>` (`pub(super)`): the
//!   negative process-group id to signal, never for pid 0 or 1.
//! - `timeout_secs(raw: Option<&OsStr>) -> (u64, Option<String>)`
//!   (`pub(crate)`): the deadline and the one stderr warning.
//! - `list_accounts`, `read_account`, `read_all`, `headroom`, `next_choice`,
//!   `codex_bin`, `profiles_root` and the public types and constants.
//!
//! Every Codex here is a fake `app-server` script written into a temporary
//! directory (mode 0700) under `$TMPDIR`. No real `codex`, no real
//! `~/.codex` or `~/.codex-profiles`, and no network.

#[path = "../../../amplihack-cli/tests/support/exec_stub.rs"]
#[allow(dead_code)]
mod exec_stub;

use super::parse;
use super::rpc::{STOP_RESERVE, group_target, stop_reserve};
use super::*;
use serde_json::json;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;
use std::time::{Duration, Instant};

const SECS_1: Duration = Duration::from_secs(1);
const SECS_10: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn lock() -> MutexGuard<'static, ()> {
    crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn scratch() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("codex-capacity-")
        .tempdir()
        .expect("temp dir under $TMPDIR");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// The handshake every fake answers before its own part: checks it was run
/// as `<bin> app-server`, answers `initialize` (id 1), reads `initialized`
/// and the `account/rateLimits/read` request. Each step that is not what
/// the docs promise exits non-zero with a reason on stderr, so a wrong
/// exchange shows up as an `Error` naming it.
const HANDSHAKE: &str = r#"
[ "$1" = "app-server" ] || { echo "bad args: $*" >&2; exit 64; }
[ "$#" = 1 ] || { echo "extra args: $*" >&2; exit 64; }
IFS= read -r init || { echo "no initialize" >&2; exit 65; }
case "$init" in *'"method":"initialize"'*) ;; *) echo "first line: $init" >&2; exit 65;; esac
case "$init" in *'"clientInfo"'*'"name":"amplihack"'*) ;; *) echo "no clientInfo: $init" >&2; exit 65;; esac
printf '%s\n' '{"id":1,"result":{"userAgent":"fake-codex/0.0"}}'
IFS= read -r note || { echo "no initialized" >&2; exit 66; }
case "$note" in *'"method":"initialized"'*) ;; *) echo "second line: $note" >&2; exit 66;; esac
IFS= read -r req || { echo "no request" >&2; exit 67; }
case "$req" in *'"method":"account/rateLimits/read"'*) ;; *) echo "third line: $req" >&2; exit 67;; esac
case "$req" in *'"id":2'*) ;; *) echo "request id: $req" >&2; exit 67;; esac
"#;

/// Keeps the fake alive after it has answered until its stdin ends, as a
/// real app server does (codex-cli 0.166.0 exits when stdin closes).
const LINGER: &str = "\nexec cat >/dev/null\n";

const TWO_WINDOWS: &str = r#"{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":12,"windowDurationMins":300,"resetsAt":1794001200},"secondary":{"usedPercent":41,"windowDurationMins":10080,"resetsAt":1794470400},"credits":null,"planType":"plus","rateLimitReachedType":null,"ordinaryUsageAllowed":true}}}"#;
const WEEKLY_ONLY: &str = r#"{"id":2,"result":{"rateLimits":{"primary":null,"secondary":{"usedPercent":34,"windowDurationMins":10080,"resetsAt":1794321000},"credits":null,"planType":"pro","rateLimitReachedType":null,"ordinaryUsageAllowed":true}}}"#;
const CREDITS_ONLY: &str = r#"{"id":2,"result":{"rateLimits":{"primary":null,"secondary":null,"credits":{"hasCredits":true,"unlimited":false,"balance":"4200"},"planType":"team"}}}"#;
const NOT_SIGNED_IN: &str = r#"{"error":{"code":-32600,"message":"codex account authentication required to read rate limits"},"id":2}"#;

/// Writes `<dir>/<name>` as `#!/bin/sh` + `body`, mode 0755, without this
/// process holding a write descriptor (ETXTBSY; see `exec_stub`).
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    exec_stub::write_executable(&path, format!("#!/bin/sh\n{body}"), 0o755);
    path
}

/// A fake that does the handshake, prints `lines` and lingers.
fn answering(dir: &Path, lines: &[&str]) -> PathBuf {
    let mut body = String::from(HANDSHAKE);
    for l in lines {
        body.push_str(&format!("printf '%s\\n' '{l}'\n"));
    }
    body.push_str(LINGER);
    script(dir, "codex", &body)
}

fn account(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    fs::create_dir_all(&p).unwrap();
    p
}

fn read(bin: &Path, home: &Path, timeout: Duration) -> Outcome {
    read_account(bin.as_os_str(), home, timeout)
}

fn ok(o: Outcome) -> Reading {
    match o {
        Outcome::Ok(r) => r,
        other => panic!("expected a reading, got {other:?}"),
    }
}

fn err(o: Outcome) -> String {
    match o {
        Outcome::Error(e) => e,
        other => panic!("expected an error, got {other:?}"),
    }
}

/// How long a test waits for something that should happen promptly before
/// it reports a hang. It is a hang detector, not a timing assertion: no
/// test passes or fails on how fast the host is.
const HANG: Duration = Duration::from_secs(60);

/// Proof that a fake and everything it started have exited, without a clock
/// and without `kill(pid, 0)` (which succeeds for a zombie).
///
/// The fake opens a FIFO for writing as its first command (`exec 9>`), so
/// it and every process it starts hold the write end. A thread here reads
/// the other end until EOF, which the kernel delivers exactly when the last
/// holder has exited, whether or not it has been reaped. Only the fake
/// opens the FIFO, so processes started by other tests never hold it.
struct Witness {
    path: PathBuf,
    eof: std::sync::mpsc::Receiver<()>,
}

impl Witness {
    fn new(dir: &Path) -> Self {
        let path = dir.join("witness");
        mkfifo(&path);
        let p = path.clone();
        let (tx, eof) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Read;
            // Blocks until the fake opens the write end.
            let mut f = fs::File::open(&p).expect("open the witness FIFO");
            let mut sink = Vec::new();
            let _ = f.read_to_end(&mut sink);
            let _ = tx.send(());
        });
        Witness { path, eof }
    }

    /// The line a fake runs first, so that it and its children hold the FIFO.
    fn hold(&self) -> String {
        format!("exec 9>'{}'\n", self.path.display())
    }

    /// Blocks until every holder has exited; fails only on a hang.
    fn assert_all_exit(&self, what: &str) {
        self.eof
            .recv_timeout(HANG)
            .unwrap_or_else(|_| panic!("{what} still running {HANG:?} after the read"));
    }
}

/// Runs `f` on a thread and fails the test if it takes longer than `limit`,
/// so a reader that opens a FIFO fails instead of hanging the suite.
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit)
        .unwrap_or_else(|_| panic!("did not finish within {limit:?}"))
}

fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path and a plain mode.
    let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(rc, 0, "mkfifo {}", path.display());
}

/// An account directory whose `auth.json` is a FIFO (opening it blocks)
/// and whose `config.toml` has mode 000, so any read of either fails the
/// test by hanging or erroring, also as root.
fn trapped_account(dir: &Path, name: &str) -> PathBuf {
    let p = account(dir, name);
    mkfifo(&p.join("auth.json"));
    let cfg = p.join("config.toml");
    fs::write(&cfg, "model = \"never-read\"\n").unwrap();
    fs::set_permissions(&cfg, fs::Permissions::from_mode(0o000)).unwrap();
    p
}

fn win(name: WindowName, used: Option<f64>, minutes: Option<u64>, at: Option<i64>) -> Window {
    Window {
        name,
        used_percent: used,
        window_minutes: minutes,
        resets_at: at,
    }
}

fn reading_with(windows: Vec<Window>, credits: Option<Credits>) -> Reading {
    Reading {
        windows,
        credits,
        ..Reading::default()
    }
}

fn used(pcts: &[f64]) -> Reading {
    let names = [WindowName::Primary, WindowName::Secondary];
    reading_with(
        pcts.iter()
            .zip(names)
            .map(|(p, n)| win(n, Some(*p), Some(300), None))
            .collect(),
        None,
    )
}

fn row(label: &str, path: &str, outcome: Outcome) -> AccountResult {
    AccountResult::new(label, PathBuf::from(path), outcome, 1_794_000_000)
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[test]
fn constants_are_the_documented_values() {
    assert_eq!(ROOT_ENV, "CODEX_PROFILES_ROOT");
    assert_eq!(BIN_ENV, "CODEX_BIN");
    assert_eq!(TIMEOUT_ENV, "AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS");
    assert_eq!(DEFAULT_TIMEOUT_SECS, 20);
    assert_eq!(MAX_TIMEOUT_SECS, 300);
    assert_eq!(DEFAULT_FLOOR, 3000.0);
    assert_eq!(HOME_LABEL, "~/.codex");
}

// ---------------------------------------------------------------------------
// parse::num (D4)
// ---------------------------------------------------------------------------

#[test]
fn num_accepts_non_negative_numbers_and_decimal_strings() {
    for (v, want) in [
        (json!(12), 12.0),
        (json!(0), 0.0),
        (json!(12.5), 12.5),
        (json!(150), 150.0),
        (json!("34"), 34.0),
        (json!("12.5"), 12.5),
        (json!("4200"), 4200.0),
        (json!("0"), 0.0),
    ] {
        assert_eq!(parse::num(&v), Some(want), "{v}");
    }
}

#[test]
fn num_rejects_everything_else() {
    let huge = "9".repeat(400);
    for v in [
        json!(-1),
        json!(-0.5),
        json!("-1"),
        json!(""),
        json!(" 5"),
        json!("5 "),
        json!("abc"),
        json!("1e3"),
        json!("inf"),
        json!("NaN"),
        json!("infinity"),
        json!(huge),
        json!(true),
        json!(null),
        json!([1]),
        json!({"v": 1}),
    ] {
        assert_eq!(parse::num(&v), None, "{v}");
    }
}

// ---------------------------------------------------------------------------
// parse::is_not_signed_in (D1)
// ---------------------------------------------------------------------------

#[test]
fn not_signed_in_matches_the_four_phrases_ignoring_case() {
    for msg in [
        "codex account authentication required to read rate limits",
        "Authentication Required",
        "you are NOT LOGGED IN",
        "Not signed in",
        "error: login required",
    ] {
        assert!(parse::is_not_signed_in(msg), "{msg}");
    }
}

#[test]
fn not_signed_in_does_not_match_bare_auth_or_login() {
    for msg in [
        "",
        "auth",
        "login failed: network unreachable",
        "invalid authentication token",
        "authorization header missing",
        "rate limit read failed",
        "internal error",
    ] {
        assert!(!parse::is_not_signed_in(msg), "{msg}");
    }
}

// ---------------------------------------------------------------------------
// parse::reading (D3)
// ---------------------------------------------------------------------------

#[test]
fn reading_takes_two_camel_case_windows_in_order() {
    let v: serde_json::Value = serde_json::from_str(TWO_WINDOWS).unwrap();
    let r = parse::reading(&v["result"]).expect("an object result");
    assert_eq!(
        r.windows,
        vec![
            win(
                WindowName::Primary,
                Some(12.0),
                Some(300),
                Some(1_794_001_200)
            ),
            win(
                WindowName::Secondary,
                Some(41.0),
                Some(10_080),
                Some(1_794_470_400)
            ),
        ]
    );
    assert_eq!(r.credits, None);
    assert_eq!(r.plan_type.as_deref(), Some("plus"));
    assert_eq!(r.rate_limit_reached_type, None);
    assert_eq!(r.ordinary_usage_allowed, Some(true));
}

#[test]
fn reading_takes_a_weekly_only_answer() {
    let v: serde_json::Value = serde_json::from_str(WEEKLY_ONLY).unwrap();
    let r = parse::reading(&v["result"]).unwrap();
    assert_eq!(
        r.windows,
        vec![win(
            WindowName::Secondary,
            Some(34.0),
            Some(10_080),
            Some(1_794_321_000)
        )]
    );
    assert_eq!(r.plan_type.as_deref(), Some("pro"));
}

#[test]
fn reading_takes_credits_only_with_a_string_balance() {
    let v: serde_json::Value = serde_json::from_str(CREDITS_ONLY).unwrap();
    let r = parse::reading(&v["result"]).unwrap();
    assert!(r.windows.is_empty());
    assert_eq!(
        r.credits,
        Some(Credits {
            has_credits: true,
            balance: Some(4200.0)
        })
    );
    assert_eq!(r.plan_type.as_deref(), Some("team"));
    assert_eq!(r.ordinary_usage_allowed, None);
}

#[test]
fn reading_accepts_snake_case_under_rate_limits() {
    let r = parse::reading(&json!({
        "rate_limits": {
            "primary": {"used_percent": "7.5", "window_duration_mins": 300, "resets_at": 1_794_001_200},
            "credits": {"has_credits": false, "balance": 10},
            "plan_type": "plus",
            "rate_limit_reached_type": "primary",
            "ordinary_usage_allowed": false
        }
    }))
    .unwrap();
    assert_eq!(
        r.windows,
        vec![win(
            WindowName::Primary,
            Some(7.5),
            Some(300),
            Some(1_794_001_200)
        )]
    );
    assert_eq!(
        r.credits,
        Some(Credits {
            has_credits: false,
            balance: Some(10.0)
        })
    );
    assert_eq!(r.rate_limit_reached_type.as_deref(), Some("primary"));
    assert_eq!(r.ordinary_usage_allowed, Some(false));
}

#[test]
fn reading_accepts_the_figures_in_the_result_itself() {
    let r = parse::reading(&json!({
        "secondary": {"usedPercent": 50, "windowDurationMins": 10080, "resetsAt": 1_794_470_400},
        "planType": "pro"
    }))
    .unwrap();
    assert_eq!(r.windows.len(), 1);
    assert_eq!(r.windows[0].name, WindowName::Secondary);
    assert_eq!(r.windows[0].used_percent, Some(50.0));
    assert_eq!(r.plan_type.as_deref(), Some("pro"));
}

#[test]
fn an_empty_object_is_a_reading_with_nothing_reported() {
    assert_eq!(parse::reading(&json!({})), Some(Reading::default()));
}

#[test]
fn a_result_that_is_not_an_object_is_no_reading() {
    for v in [
        json!(null),
        json!([1, 2]),
        json!("ok"),
        json!(42),
        json!(true),
    ] {
        assert_eq!(parse::reading(&v), None, "{v}");
    }
}

#[test]
fn bad_field_values_count_as_absent() {
    let r = parse::reading(&json!({
        "rateLimits": {
            "primary": {"usedPercent": -3, "windowDurationMins": -5, "resetsAt": "soon"},
            "secondary": {"usedPercent": "9".repeat(400), "windowDurationMins": 30.5},
            "credits": {"hasCredits": "true", "balance": "lots"},
            "planType": 7,
            "ordinaryUsageAllowed": "yes"
        }
    }))
    .unwrap();
    assert_eq!(
        r.windows,
        vec![
            win(WindowName::Primary, None, None, None),
            win(WindowName::Secondary, None, None, None),
        ]
    );
    assert_eq!(
        r.credits,
        Some(Credits {
            has_credits: false,
            balance: None
        }),
        "hasCredits counts only as JSON true"
    );
    assert_eq!(r.plan_type, None);
    assert_eq!(r.ordinary_usage_allowed, None);
}

#[test]
fn a_window_that_is_not_an_object_is_not_shown() {
    let r = parse::reading(&json!({
        "rateLimits": {"primary": 12, "secondary": "x"}
    }))
    .unwrap();
    assert!(r.windows.is_empty(), "{:?}", r.windows);
}

#[test]
fn resets_at_is_kept_as_given_and_an_overflow_is_none() {
    let r = parse::reading(&json!({
        "rateLimits": {
            "primary": {"resetsAt": -5},
            "secondary": {"resetsAt": 1e30}
        }
    }))
    .unwrap();
    assert_eq!(r.windows[0].resets_at, Some(-5), "as received");
    assert_eq!(r.windows[1].resets_at, None, "no i64 for 1e30");
}

#[test]
fn server_strings_are_cut_to_200_chars_on_a_char_boundary_and_cleaned() {
    let long = "é".repeat(300);
    let r = parse::reading(&json!({
        "planType": long,
        "rateLimitReachedType": "pro\u{1b}[31m\u{202e}x\u{7}"
    }))
    .unwrap();
    let plan = r.plan_type.unwrap();
    assert_eq!(plan.chars().count(), 200, "cut by chars, not bytes");
    assert!(plan.chars().all(|c| c == 'é'));
    let reached = r.rate_limit_reached_type.unwrap();
    assert!(reached.starts_with("pro"), "{reached:?}");
    for bad in ['\u{1b}', '\u{202e}', '\u{7}'] {
        assert!(!reached.contains(bad), "{reached:?} keeps {bad:?}");
    }
}

// ---------------------------------------------------------------------------
// timeout_secs (D7)
// ---------------------------------------------------------------------------

#[test]
fn timeout_defaults_to_20_and_takes_1_to_300() {
    assert_eq!(timeout_secs(None), (20, None));
    assert_eq!(timeout_secs(Some(OsStr::new(""))), (20, None));
    for (raw, want) in [("1", 1), ("5", 5), ("300", 300)] {
        assert_eq!(timeout_secs(Some(OsStr::new(raw))), (want, None), "{raw}");
    }
}

#[test]
fn a_bad_timeout_warns_once_and_uses_20() {
    for raw in ["0", "301", "abc", "-1", "1.5", " 5"] {
        let (secs, warning) = timeout_secs(Some(OsStr::new(raw)));
        assert_eq!(secs, 20, "{raw}");
        assert_eq!(
            warning.as_deref(),
            Some(
                format!(
                    "warning: AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS={raw:?} is not a whole number from 1 to 300; using 20"
                )
                .as_str()
            ),
            "{raw}"
        );
    }
}

#[test]
fn the_timeout_warning_escapes_control_characters() {
    let (_, warning) = timeout_secs(Some(OsStr::new("5\u{1b}[2J")));
    let w = warning.expect("a warning");
    assert!(!w.contains('\u{1b}'), "{w:?}");
    assert!(w.starts_with("warning: AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS="));
}

// ---------------------------------------------------------------------------
// Environment: codex_bin and profiles_root
// ---------------------------------------------------------------------------

/// Sets or removes `vars` for the guard's life. Callers hold `lock()`.
struct Vars(Vec<(&'static str, Option<OsString>)>);

impl Vars {
    fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
        let saved = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var_os(k)))
            .collect();
        for (k, v) in vars {
            // SAFETY: every environment change in this crate's tests holds
            // `env_lock()`.
            unsafe {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
        Vars(saved)
    }
}

impl Drop for Vars {
    fn drop(&mut self) {
        for (k, v) in self.0.drain(..) {
            // SAFETY: as in `set`.
            unsafe {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}

#[test]
fn codex_bin_is_codex_bin_when_set_and_not_empty() {
    let _l = lock();
    {
        let _v = Vars::set(&[("CODEX_BIN", None)]);
        assert_eq!(codex_bin(), OsString::from("codex"));
    }
    {
        let _v = Vars::set(&[("CODEX_BIN", Some(""))]);
        assert_eq!(codex_bin(), OsString::from("codex"));
    }
    {
        let _v = Vars::set(&[("CODEX_BIN", Some("/opt/fake/codex"))]);
        assert_eq!(codex_bin(), OsString::from("/opt/fake/codex"));
    }
}

#[test]
fn profiles_root_is_the_variable_or_home_codex_profiles() {
    let _l = lock();
    let home = Path::new("/fixture/home");
    {
        let _v = Vars::set(&[("CODEX_PROFILES_ROOT", None)]);
        assert_eq!(
            profiles_root(Some(home)),
            Some(home.join(".codex-profiles"))
        );
        assert_eq!(profiles_root(None), None);
    }
    {
        let _v = Vars::set(&[("CODEX_PROFILES_ROOT", Some(""))]);
        assert_eq!(
            profiles_root(Some(home)),
            Some(home.join(".codex-profiles")),
            "empty means unset"
        );
    }
    {
        let _v = Vars::set(&[("CODEX_PROFILES_ROOT", Some("/elsewhere/codex"))]);
        assert_eq!(
            profiles_root(Some(home)),
            Some(PathBuf::from("/elsewhere/codex"))
        );
        assert_eq!(profiles_root(None), Some(PathBuf::from("/elsewhere/codex")));
    }
}

// ---------------------------------------------------------------------------
// rpc::group_target (PROC-2)
// ---------------------------------------------------------------------------

#[test]
fn the_group_kill_never_targets_pid_0_or_1() {
    assert_eq!(group_target(0), None, "kill(0) is our own group");
    assert_eq!(group_target(1), None, "kill(-1) is every process");
    assert_eq!(group_target(2), Some(-2));
    assert_eq!(group_target(4242), Some(-4242));
    assert_eq!(group_target(u32::MAX), None, "does not fit a pid_t");
    assert_eq!(group_target(i32::MAX as u32), Some(-i32::MAX));
}

// ---------------------------------------------------------------------------
// headroom and next_choice (D5)
// ---------------------------------------------------------------------------

#[test]
fn headroom_is_100_minus_the_fullest_window() {
    assert_eq!(headroom(&used(&[12.0, 41.0]), DEFAULT_FLOOR), 59.0);
    assert_eq!(headroom(&used(&[34.0]), DEFAULT_FLOOR), 66.0);
    assert_eq!(headroom(&used(&[0.0]), DEFAULT_FLOOR), 100.0);
}

#[test]
fn a_full_or_empty_reading_without_credits_is_minus_one() {
    assert_eq!(headroom(&used(&[100.0, 3.0]), DEFAULT_FLOOR), -1.0);
    assert_eq!(headroom(&used(&[120.0]), DEFAULT_FLOOR), -1.0);
    assert_eq!(headroom(&Reading::default(), DEFAULT_FLOOR), -1.0);
    let no_pct = reading_with(vec![win(WindowName::Primary, None, Some(300), None)], None);
    assert_eq!(
        headroom(&no_pct, DEFAULT_FLOOR),
        -1.0,
        "no percentage is room 0"
    );
}

#[test]
fn credits_above_the_floor_give_half_a_point_only_when_there_is_no_room() {
    let credits = |has: bool, bal: Option<f64>| {
        Some(Credits {
            has_credits: has,
            balance: bal,
        })
    };
    let full = |c| {
        reading_with(
            vec![win(WindowName::Primary, Some(100.0), Some(300), None)],
            c,
        )
    };
    assert_eq!(
        headroom(&full(credits(true, Some(4200.0))), DEFAULT_FLOOR),
        0.5
    );
    assert_eq!(
        headroom(
            &reading_with(vec![], credits(true, Some(4200.0))),
            DEFAULT_FLOOR
        ),
        0.5,
        "credits only"
    );
    assert_eq!(
        headroom(&full(credits(true, Some(3000.0))), DEFAULT_FLOOR),
        -1.0,
        "the balance must be above the floor"
    );
    assert_eq!(headroom(&full(credits(true, None)), DEFAULT_FLOOR), -1.0);
    assert_eq!(
        headroom(&full(credits(false, Some(9000.0))), DEFAULT_FLOOR),
        -1.0
    );
    assert_eq!(
        headroom(&full(credits(true, Some(150.0))), 100.0),
        0.5,
        "the floor is a parameter"
    );
    let roomy = reading_with(
        vec![win(WindowName::Primary, Some(20.0), Some(300), None)],
        credits(true, Some(9000.0)),
    );
    assert_eq!(
        headroom(&roomy, DEFAULT_FLOOR),
        80.0,
        "credits never add room"
    );
}

#[test]
fn next_choice_is_the_row_with_the_most_window_room() {
    let rows = [
        row("ffarmer", "/r/ffarmer", Outcome::Ok(used(&[12.0, 41.0]))),
        row("ggrant", "/r/ggrant", Outcome::Ok(used(&[34.0]))),
        row("~/.codex", "/h/.codex", Outcome::Ok(used(&[88.0, 70.0]))),
    ];
    assert_eq!(next_choice(&rows, DEFAULT_FLOOR), Some(1));
}

#[test]
fn next_choice_takes_credits_above_the_floor_when_no_window_has_room() {
    let rows = [
        row("full", "/r/full", Outcome::Ok(used(&[100.0]))),
        row(
            "hhale",
            "/r/hhale",
            Outcome::Ok(reading_with(
                vec![],
                Some(Credits {
                    has_credits: true,
                    balance: Some(4200.0),
                }),
            )),
        ),
    ];
    assert_eq!(next_choice(&rows, DEFAULT_FLOOR), Some(1));
}

#[test]
fn next_choice_is_none_without_a_candidate_with_room() {
    let rows = [
        row("full", "/r/full", Outcome::Ok(used(&[100.0]))),
        row("bare", "/r/bare", Outcome::Ok(Reading::default())),
        row(
            "poor",
            "/r/poor",
            Outcome::Ok(reading_with(
                vec![],
                Some(Credits {
                    has_credits: true,
                    balance: Some(10.0),
                }),
            )),
        ),
        row("out", "/r/out", Outcome::NotSignedIn),
        row("slow", "/r/slow", Outcome::Timeout),
        row("bad", "/r/bad", Outcome::Error("boom".into())),
    ];
    assert_eq!(next_choice(&rows, DEFAULT_FLOOR), None);
    assert_eq!(next_choice(&[], DEFAULT_FLOOR), None);
}

#[test]
fn only_signed_in_rows_are_candidates() {
    let rows = [
        row("out", "/r/zzz", Outcome::NotSignedIn),
        row("slow", "/r/yyy", Outcome::Timeout),
        row("bad", "/r/xxx", Outcome::Error("e".into())),
        row("ok", "/r/aaa", Outcome::Ok(used(&[99.0]))),
    ];
    assert_eq!(next_choice(&rows, DEFAULT_FLOOR), Some(3));
}

#[test]
fn equal_room_is_broken_by_descending_path_bytes() {
    let rows = [
        row("ffarmer", "/r/ffarmer", Outcome::Ok(used(&[34.0]))),
        row("ggrant", "/r/ggrant", Outcome::Ok(used(&[34.0]))),
        row("aaron", "/r/aaron", Outcome::Ok(used(&[34.0]))),
    ];
    assert_eq!(
        next_choice(&rows, DEFAULT_FLOOR),
        Some(1),
        "ggrant > ffarmer"
    );
    // Bytes, not case-folded: 'a' (0x61) sorts after 'Z' (0x5a).
    let rows = [
        row("Zed", "/r/Zed", Outcome::Ok(used(&[10.0]))),
        row("amy", "/r/amy", Outcome::Ok(used(&[10.0]))),
    ];
    assert_eq!(next_choice(&rows, DEFAULT_FLOOR), Some(1));
    // The path as built, not the label: `/h/.codex` vs `/r/x`.
    let rows = [
        row("x", "/r/x", Outcome::Ok(used(&[10.0]))),
        row("~/.codex", "/h/.codex", Outcome::Ok(used(&[10.0]))),
    ];
    assert_eq!(next_choice(&rows, DEFAULT_FLOOR), Some(0));
}

// ---------------------------------------------------------------------------
// list_accounts (D6)
// ---------------------------------------------------------------------------

fn labels(accounts: &[Account]) -> Vec<&str> {
    accounts.iter().map(|a| a.label.as_str()).collect()
}

#[test]
fn accounts_are_the_root_subdirectories_in_byte_order() {
    let d = scratch();
    let root = d.path().join("profiles");
    for n in ["ggrant", "ffarmer", "Bob", ".hidden"] {
        account(&root, n);
    }
    fs::write(root.join("notes.txt"), "a file is not an account").unwrap();
    let target = account(d.path(), "elsewhere");
    std::os::unix::fs::symlink(&target, root.join("linked")).unwrap();
    std::os::unix::fs::symlink(d.path().join("missing"), root.join("dangling")).unwrap();
    let home = d.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let got = list_accounts(&root, Some(&home)).unwrap();
    assert_eq!(labels(&got), ["Bob", "ffarmer", "ggrant", "linked"]);
    for a in &got {
        assert_eq!(a.path, root.join(&a.label), "the path as built");
    }
}

#[test]
fn home_codex_is_listed_last_when_it_is_a_directory() {
    let d = scratch();
    let root = d.path().join("profiles");
    account(&root, "a");
    let home = d.path().join("home");
    account(&home, ".codex");
    let got = list_accounts(&root, Some(&home)).unwrap();
    assert_eq!(labels(&got), ["a", "~/.codex"]);
    assert_eq!(got[1].path, home.join(".codex"));
}

#[test]
fn home_codex_is_skipped_when_it_is_a_file_missing_or_there_is_no_home() {
    let d = scratch();
    let root = d.path().join("profiles");
    account(&root, "a");
    let home = d.path().join("home");
    fs::create_dir_all(&home).unwrap();
    assert_eq!(labels(&list_accounts(&root, Some(&home)).unwrap()), ["a"]);
    fs::write(home.join(".codex"), "").unwrap();
    assert_eq!(labels(&list_accounts(&root, Some(&home)).unwrap()), ["a"]);
    assert_eq!(labels(&list_accounts(&root, None).unwrap()), ["a"]);
}

#[test]
fn home_codex_is_skipped_when_it_resolves_to_a_profile_or_the_root() {
    let d = scratch();
    let root = d.path().join("profiles");
    let a = account(&root, "a");
    let home = d.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::os::unix::fs::symlink(&a, home.join(".codex")).unwrap();
    assert_eq!(
        labels(&list_accounts(&root, Some(&home)).unwrap()),
        ["a"],
        "the profile row stands"
    );

    let home2 = d.path().join("home2");
    fs::create_dir_all(&home2).unwrap();
    std::os::unix::fs::symlink(&root, home2.join(".codex")).unwrap();
    assert_eq!(labels(&list_accounts(&root, Some(&home2)).unwrap()), ["a"]);
}

#[test]
fn a_missing_or_empty_root_is_not_an_error() {
    let d = scratch();
    let home = d.path().join("home");
    fs::create_dir_all(&home).unwrap();
    assert!(
        list_accounts(&d.path().join("nope"), Some(&home))
            .unwrap()
            .is_empty()
    );
    let empty = account(d.path(), "empty");
    assert!(list_accounts(&empty, Some(&home)).unwrap().is_empty());
    let file = d.path().join("file");
    fs::write(&file, "").unwrap();
    assert!(list_accounts(&file, Some(&home)).unwrap().is_empty());
    account(&home, ".codex");
    assert_eq!(
        labels(&list_accounts(&d.path().join("nope"), Some(&home)).unwrap()),
        ["~/.codex"]
    );
}

#[test]
fn an_unreadable_root_is_an_error() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch();
    let root = d.path().join("profiles");
    account(&root, "a");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = fs::read_dir(&root).is_ok();
    let got = list_accounts(&root, None);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    if readable_anyway {
        // Running as root: the mode does not stop the listing.
        return;
    }
    assert!(
        got.is_err(),
        "an unreadable root is not an empty one: {got:?}"
    );
}

#[test]
fn listing_never_opens_a_file_inside_an_account() {
    let d = scratch();
    let root = d.path().join("profiles");
    trapped_account(&root, "a");
    let home = d.path().join("home");
    trapped_account(&home, ".codex");
    let got = within(Duration::from_secs(60), move || {
        labels(&list_accounts(&root, Some(&home)).unwrap())
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
    });
    assert_eq!(got, ["a", "~/.codex"]);
}

// ---------------------------------------------------------------------------
// read_account: the live exchange (D2, D9, D10)
// ---------------------------------------------------------------------------

#[test]
fn reads_two_windows() {
    let d = scratch();
    let bin = answering(d.path(), &[TWO_WINDOWS]);
    let r = ok(read(&bin, &account(d.path(), "ffarmer"), SECS_10));
    assert_eq!(r.windows.len(), 2);
    assert_eq!(r.windows[0].name, WindowName::Primary);
    assert_eq!(r.windows[0].used_percent, Some(12.0));
    assert_eq!(r.windows[0].window_minutes, Some(300));
    assert_eq!(r.windows[0].resets_at, Some(1_794_001_200));
    assert_eq!(r.windows[1].name, WindowName::Secondary);
    assert_eq!(r.windows[1].used_percent, Some(41.0));
    assert_eq!(r.windows[1].window_minutes, Some(10_080));
    assert_eq!(r.plan_type.as_deref(), Some("plus"));
    assert_eq!(r.ordinary_usage_allowed, Some(true));
}

#[test]
fn reads_a_weekly_only_account() {
    let d = scratch();
    let bin = answering(d.path(), &[WEEKLY_ONLY]);
    let r = ok(read(&bin, &account(d.path(), "ggrant"), SECS_10));
    assert_eq!(r.windows.len(), 1);
    assert_eq!(r.windows[0].name, WindowName::Secondary);
    assert_eq!(r.windows[0].used_percent, Some(34.0));
}

#[test]
fn reads_a_credits_only_account() {
    let d = scratch();
    let bin = answering(d.path(), &[CREDITS_ONLY]);
    let r = ok(read(&bin, &account(d.path(), "hhale"), SECS_10));
    assert!(r.windows.is_empty());
    assert_eq!(
        r.credits,
        Some(Credits {
            has_credits: true,
            balance: Some(4200.0)
        })
    );
}

#[test]
fn notifications_and_other_lines_without_an_id_are_skipped() {
    let d = scratch();
    let mut body = String::from(
        r#"
printf '%s\n' '{"method":"remoteControl/status/changed","params":{"status":"idle"}}'
"#,
    );
    body.push_str(HANDSHAKE);
    body.push_str(
        r#"
printf '%s\n' '{"method":"remoteControl/status/changed","params":{"status":"busy"}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"account/updated","params":{}}'
printf '%s\n' '{"id":2,"result":{"rateLimits":{"secondary":{"usedPercent":34,"windowDurationMins":10080,"resetsAt":1794321000}}}}'
"#,
    );
    body.push_str(LINGER);
    let bin = script(d.path(), "codex", &body);
    let r = ok(read(&bin, &account(d.path(), "ggrant"), SECS_10));
    assert_eq!(r.windows[0].used_percent, Some(34.0));
}

#[test]
fn the_real_not_signed_in_wording_is_not_signed_in() {
    let d = scratch();
    let bin = answering(d.path(), &[NOT_SIGNED_IN]);
    assert_eq!(
        read(&bin, &account(d.path(), "jjones"), SECS_10),
        Outcome::NotSignedIn
    );
}

#[test]
fn any_other_server_error_is_its_message() {
    let d = scratch();
    let bin = answering(
        d.path(),
        &[r#"{"id":2,"error":{"code":-32603,"message":"rate limit backend unavailable"}}"#],
    );
    assert_eq!(
        err(read(&bin, &account(d.path(), "x"), SECS_10)),
        "rate limit backend unavailable"
    );
}

#[test]
fn an_error_answer_to_initialize_counts_too() {
    let d = scratch();
    let bin = script(
        d.path(),
        "codex",
        r#"
IFS= read -r init
printf '%s\n' '{"id":1,"error":{"code":-32600,"message":"initialize refused"}}'
exec cat >/dev/null
"#,
    );
    assert_eq!(
        err(read(&bin, &account(d.path(), "x"), SECS_10)),
        "initialize refused"
    );
}

#[test]
fn a_long_server_message_is_cut_to_200_chars_and_cleaned() {
    let d = scratch();
    let msg = format!("\\u001b[31m{}", "m".repeat(500));
    let line = format!(r#"{{"id":2,"error":{{"message":"{msg}"}}}}"#);
    let bin = answering(d.path(), &[&line]);
    let e = err(read(&bin, &account(d.path(), "x"), SECS_10));
    assert!(e.chars().count() <= 200, "{} chars", e.chars().count());
    assert!(!e.contains('\u{1b}'), "{e:?}");
    assert!(e.ends_with("mmm"), "{e:?}");
}

#[test]
fn a_result_that_is_not_an_object_is_an_unexpected_response() {
    let d = scratch();
    let bin = answering(d.path(), &[r#"{"id":2,"result":[1,2,3]}"#]);
    assert_eq!(
        err(read(&bin, &account(d.path(), "x"), SECS_10)),
        "unexpected response from codex app-server"
    );
}

#[test]
fn an_exit_before_the_answer_names_the_status_and_last_stderr_line() {
    let d = scratch();
    let bin = script(
        d.path(),
        "codex",
        r#"
echo "warning: first line" >&2
echo "error: unrecognized subcommand 'app-server'" >&2
echo "" >&2
exit 1
"#,
    );
    assert_eq!(
        err(read(&bin, &account(d.path(), "mmoss"), SECS_10)),
        "codex app-server exited (exit status: 1): error: unrecognized subcommand 'app-server'"
    );
}

#[test]
fn the_stderr_line_in_an_exit_error_is_cut_to_200_chars() {
    let d = scratch();
    let bin = script(
        d.path(),
        "codex",
        &format!("echo '{}' >&2\nexit 3\n", "x".repeat(500)),
    );
    let e = err(read(&bin, &account(d.path(), "x"), SECS_10));
    let prefix = "codex app-server exited (exit status: 3): ";
    assert!(e.starts_with(prefix), "{e}");
    assert_eq!(e[prefix.len()..].chars().count(), 200, "{e}");
}

#[test]
fn a_missing_binary_is_named() {
    let d = scratch();
    let bin = d.path().join("no-such-codex");
    assert_eq!(
        err(read(&bin, &account(d.path(), "x"), SECS_10)),
        format!("codex binary not found: {}", bin.display())
    );
}

#[test]
fn a_binary_that_cannot_start_is_named_with_the_reason() {
    let d = scratch();
    let bin = d.path().join("codex");
    exec_stub::write_executable(&bin, "#!/bin/sh\nexit 0\n", 0o644);
    let e = err(read(&bin, &account(d.path(), "x"), SECS_10));
    assert!(
        e.starts_with(&format!("cannot start {}: ", bin.display())),
        "{e}"
    );
}

#[test]
fn a_stdout_line_over_1_mib_is_an_unexpected_response() {
    let d = scratch();
    let mut body = String::from(HANDSHAKE);
    body.push_str(
        r#"
printf '{"id":2,"result":{"planType":"'
head -c 2097152 /dev/zero | tr '\000' a
printf '"}}\n'
"#,
    );
    body.push_str(LINGER);
    let bin = script(d.path(), "codex", &body);
    // The fake lingers after the long line, so a reader that kept reading
    // would end in `Timeout`, not in this error.
    assert_eq!(
        err(read(&bin, &account(d.path(), "x"), SECS_10)),
        "unexpected response from codex app-server"
    );
}

#[test]
fn stderr_past_64_kib_does_not_stall_the_child() {
    let d = scratch();
    let mut body = String::from(HANDSHAKE);
    body.push_str("head -c 300000 /dev/zero | tr '\\000' e >&2\n");
    body.push_str(&format!("printf '%s\\n' '{TWO_WINDOWS}'\n"));
    body.push_str(LINGER);
    let bin = script(d.path(), "codex", &body);
    let r = ok(read(&bin, &account(d.path(), "x"), SECS_10));
    assert_eq!(r.windows.len(), 2);
}

#[test]
fn a_huge_digit_string_percentage_is_absent_not_infinite() {
    let d = scratch();
    let line = format!(
        r#"{{"id":2,"result":{{"rateLimits":{{"primary":{{"usedPercent":"{}","windowDurationMins":300}}}}}}}}"#,
        "9".repeat(400)
    );
    let bin = answering(d.path(), &[&line]);
    let r = ok(read(&bin, &account(d.path(), "x"), SECS_10));
    assert_eq!(r.windows[0].used_percent, None);
    assert_eq!(r.windows[0].window_minutes, Some(300));
}

#[test]
fn a_silent_server_times_out_and_its_whole_group_is_killed() {
    let d = scratch();
    let w = Witness::new(d.path());
    let mut body = w.hold();
    body.push_str(HANDSHAKE);
    body.push_str("sleep 600 >/dev/null 2>&1 &\nsleep 600\n");
    let bin = script(d.path(), "codex", &body);
    let start = Instant::now();
    assert_eq!(
        read(&bin, &account(d.path(), "kkim"), SECS_1),
        Outcome::Timeout
    );
    // A lower bound only: the answer is not given up before the ask
    // deadline (1 s less the 250 ms stop reserve). A slow host cannot make
    // this fail. That the stop stays inside the one deadline is shown by
    // the order of events: the read returned `Timeout` with both `sleep 600`
    // processes running, and both are then gone.
    let took = start.elapsed();
    assert!(took >= Duration::from_millis(750), "{took:?}");
    w.assert_all_exit("the app server or its child");
}

#[test]
fn the_stop_reserve_is_3_seconds_or_a_quarter_of_the_deadline() {
    assert_eq!(STOP_RESERVE, Duration::from_secs(3));
    assert_eq!(
        stop_reserve(Duration::from_secs(20)),
        Duration::from_secs(3)
    );
    assert_eq!(
        stop_reserve(Duration::from_secs(12)),
        Duration::from_secs(3)
    );
    assert_eq!(stop_reserve(Duration::from_secs(8)), Duration::from_secs(2));
    assert_eq!(stop_reserve(SECS_1), Duration::from_millis(250));
}

/// A fake that records a `SIGTERM` in `term` and then exits.
fn trap_term(term: &Path) -> String {
    format!("trap 'echo term > \"{}\"; exit 143' TERM\n", term.display())
}

#[test]
fn a_server_that_exits_when_stdin_ends_gets_no_signal_after_its_answer() {
    let d = scratch();
    let (term, done) = (d.path().join("term"), d.path().join("done"));
    let mut body = trap_term(&term);
    body.push_str(HANDSHAKE);
    body.push_str(&format!(
        "printf '%s\\n' '{TWO_WINDOWS}'\ncat >/dev/null\necho done > '{}'\n",
        done.display()
    ));
    let bin = script(d.path(), "codex", &body);
    let r = ok(read(&bin, &account(d.path(), "x"), Duration::from_secs(20)));
    assert_eq!(r.windows.len(), 2);
    assert!(done.exists(), "the server did not finish on its own");
    assert!(
        !term.exists(),
        "a server that exits on stdin EOF was signalled"
    );
}

#[test]
fn a_timed_out_server_that_exits_when_stdin_ends_finishes_its_write() {
    // The renewal case: the server is still working when the answer is
    // due. Its stdin is closed first, and the work it does before exiting
    // (here, a write into the account) completes before anything is
    // signalled.
    let d = scratch();
    let acct = account(d.path(), "renewing");
    let (term, wrote) = (d.path().join("term"), acct.join("auth-written"));
    let mut body = trap_term(&term);
    body.push_str(HANDSHAKE);
    body.push_str(&format!(
        "cat >/dev/null\necho new-token > '{}'\n",
        wrote.display()
    ));
    let bin = script(d.path(), "codex", &body);
    assert_eq!(read(&bin, &acct, Duration::from_secs(4)), Outcome::Timeout);
    assert!(wrote.exists(), "the server's write was cut short");
    assert!(
        !term.exists(),
        "a server that exits on stdin EOF was signalled"
    );
}

#[test]
fn a_server_that_ignores_stdin_end_gets_sigterm_before_sigkill() {
    let d = scratch();
    let term = d.path().join("term");
    let mut body = trap_term(&term);
    body.push_str(HANDSHAKE);
    // `sleep` does not read stdin, so its end is not seen.
    body.push_str("sleep 600\n");
    let bin = script(d.path(), "codex", &body);
    assert_eq!(
        read(&bin, &account(d.path(), "x"), Duration::from_secs(4)),
        Outcome::Timeout
    );
    assert!(
        term.exists(),
        "SIGTERM was not sent before the group was killed"
    );
}

#[test]
fn a_server_that_ignores_sigterm_is_killed_by_the_deadline() {
    let d = scratch();
    let w = Witness::new(d.path());
    let mut body = w.hold();
    body.push_str("trap '' TERM\n");
    body.push_str(HANDSHAKE);
    body.push_str("sleep 600\n");
    let bin = script(d.path(), "codex", &body);
    let start = Instant::now();
    assert_eq!(
        read(&bin, &account(d.path(), "x"), Duration::from_secs(2)),
        Outcome::Timeout
    );
    // A lower bound only: the server is given the whole reserve before
    // `SIGKILL`. The kill itself is shown by the server and its `sleep 600`
    // exiting, which nothing but the read's `SIGKILL` causes.
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(1900),
        "the stop ended early: {took:?}"
    );
    w.assert_all_exit("the app server that ignores SIGTERM");
}

#[test]
fn the_deadline_covers_the_handshake() {
    let d = scratch();
    let w = Witness::new(d.path());
    let bin = script(d.path(), "codex", &format!("{}sleep 600\n", w.hold()));
    // The fake never answers `initialize`; a read that waited for the
    // handshake outside the deadline would never return `Timeout`.
    assert_eq!(
        read(&bin, &account(d.path(), "x"), SECS_1),
        Outcome::Timeout
    );
    w.assert_all_exit("the app server stuck before the handshake");
}

#[test]
fn a_successful_read_kills_what_the_server_left_running() {
    let d = scratch();
    let w = Witness::new(d.path());
    let mut body = w.hold();
    body.push_str(HANDSHAKE);
    // The grandchild keeps stdout and stderr open: the reader must not wait
    // for their EOF once it has its answer.
    body.push_str(&format!("sleep 600 &\nprintf '%s\\n' '{TWO_WINDOWS}'\n"));
    body.push_str(LINGER);
    let bin = script(d.path(), "codex", &body);
    let acct = account(d.path(), "x");
    // The read's own deadline is far past the hang detector, so a reader
    // that waited for the grandchild's EOF (or for the deadline) would be
    // reported as a hang instead of passing late.
    let r = ok(within(HANG, move || read(&bin, &acct, HANG * 10)));
    assert_eq!(r.windows.len(), 2);
    w.assert_all_exit("the grandchild the server left running");
}

#[test]
fn the_child_gets_codex_home_and_no_api_keys_and_the_parent_env_is_unchanged() {
    let _l = lock();
    // The sentinels are made unique per run: a literal could already appear
    // in an inherited variable (a recipe runner exports its design notes),
    // which would make the "not in the child" check fail for the wrong reason.
    let tag = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let openai_key = format!("sk-sentinel-{tag}");
    let codex_key = format!("codex-sentinel-{tag}");
    let _v = Vars::set(&[
        ("OPENAI_API_KEY", Some(openai_key.as_str())),
        ("CODEX_API_KEY", Some(codex_key.as_str())),
        ("RUST_LOG", Some("trace")),
        ("CODEX_HOME", None),
    ]);
    let d = scratch();
    let seen = d.path().join("seen-env");
    let mut body = String::from(HANDSHAKE);
    body.push_str(&format!(
        "env -0 > '{}'\nprintf '%s\\n' '{WEEKLY_ONLY}'\n",
        seen.display()
    ));
    body.push_str(LINGER);
    let bin = script(d.path(), "codex", &body);
    let acct = account(d.path(), "ggrant");
    ok(read(&bin, &acct, SECS_10));

    // NUL-separated entries: an inherited multi-line value (a recipe
    // runner's review notes, say) can hold a line that starts with
    // `OPENAI_API_KEY=` without being that variable.
    let env = fs::read_to_string(&seen).unwrap();
    let lines: Vec<&str> = env.split('\0').collect();
    assert!(
        lines.contains(&format!("CODEX_HOME={}", acct.display()).as_str()),
        "{env}"
    );
    for gone in ["OPENAI_API_KEY=", "CODEX_API_KEY=", "RUST_LOG="] {
        assert!(
            !lines.iter().any(|l| l.starts_with(gone)),
            "{gone} in {env}"
        );
    }
    assert!(!env.contains(&openai_key));
    assert!(!env.contains(&codex_key));
    assert_eq!(
        std::env::var_os("CODEX_HOME"),
        None,
        "never set in amplihack"
    );
    assert_eq!(
        std::env::var_os("OPENAI_API_KEY").as_deref(),
        Some(OsStr::new(&openai_key)),
        "removed from the child only"
    );
}

#[test]
fn no_shell_is_used_and_the_path_reaches_the_child_only_through_codex_home() {
    let d = scratch();
    let args = d.path().join("args");
    let body = format!(
        "printf '%s\\n' \"$0\" \"$@\" > '{}'\n{HANDSHAKE}printf '%s\\n' '{WEEKLY_ONLY}'\n{LINGER}",
        args.display()
    );
    let bin = script(d.path(), "codex", &body);
    let acct = account(d.path(), "a'b; touch PWNED $(id)");
    ok(read(&bin, &acct, SECS_10));
    let got = fs::read_to_string(&args).unwrap();
    assert_eq!(got, format!("{}\napp-server\n", bin.display()));
    assert!(!d.path().join("PWNED").exists());
}

#[test]
fn reading_never_opens_a_file_in_the_account_and_writes_nothing_there() {
    let d = scratch();
    let bin = answering(d.path(), &[TWO_WINDOWS]);
    let acct = trapped_account(d.path(), "ffarmer");
    let before: Vec<_> = fs::read_dir(&acct)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let a = acct.clone();
    let out = within(Duration::from_secs(60), move || {
        read_account(bin.as_os_str(), &a, SECS_10)
    });
    assert_eq!(ok(out).windows.len(), 2);
    let mut after: Vec<_> = fs::read_dir(&acct)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let mut before = before;
    before.sort();
    after.sort();
    assert_eq!(before, after, "amplihack writes nothing into CODEX_HOME");
}

// ---------------------------------------------------------------------------
// read_all (D6, D8, D16)
// ---------------------------------------------------------------------------

/// One fake for every account, answering by the account's directory name.
/// `read_all` drops a `NotSignedIn` row only when its label is
/// [`HOME_LABEL`], so the home row can point at any directory here.
/// A `meet*` account reads the FIFO of the same name in `markers` to EOF
/// before it answers; see [`read_all_reads_the_accounts_at_once`].
fn by_name(dir: &Path, markers: &Path) -> PathBuf {
    fs::create_dir_all(markers).unwrap();
    let body = format!(
        r#"{HANDSHAKE}
name=$(basename "$CODEX_HOME")
case "$name" in
  two) printf '%s\n' '{TWO_WINDOWS}' ;;
  weekly|.codex) printf '%s\n' '{WEEKLY_ONLY}' ;;
  credits) printf '%s\n' '{CREDITS_ONLY}' ;;
  nosign) printf '%s\n' '{NOT_SIGNED_IN}' ;;
  slow) sleep 600 ;;
  crash) echo "boom" >&2; exit 2 ;;
  meet*)
    cat '{m}/'"$name" >/dev/null
    printf '%s\n' '{WEEKLY_ONLY}' ;;
  *) echo "unknown fixture $name" >&2; exit 9 ;;
esac
{LINGER}"#,
        m = markers.display()
    );
    script(dir, "codex", &body)
}

fn acct(root: &Path, name: &str) -> Account {
    Account {
        label: name.to_string(),
        path: account(root, name),
    }
}

fn home_acct(home: &Path) -> Account {
    Account {
        label: HOME_LABEL.to_string(),
        path: account(home, ".codex"),
    }
}

fn kind(o: &Outcome) -> &'static str {
    match o {
        Outcome::Ok(_) => "ok",
        Outcome::NotSignedIn => "not_signed_in",
        Outcome::Timeout => "timeout",
        Outcome::Error(_) => "error",
    }
}

#[test]
fn read_all_keeps_the_input_order_and_each_outcome() {
    let d = scratch();
    let bin = by_name(d.path(), &d.path().join("m"));
    let root = d.path().join("profiles");
    let accounts = [
        acct(&root, "slow"),
        acct(&root, "two"),
        acct(&root, "nosign"),
        acct(&root, "credits"),
        acct(&root, "crash"),
        acct(&root, "weekly"),
    ];
    let before = chrono::Utc::now().timestamp();
    let rows = read_all(bin.as_os_str(), &accounts, SECS_1);
    let after = chrono::Utc::now().timestamp();
    let got: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.label.as_str(), kind(&r.outcome)))
        .collect();
    assert_eq!(
        got,
        [
            ("slow", "timeout"),
            ("two", "ok"),
            ("nosign", "not_signed_in"),
            ("credits", "ok"),
            ("crash", "error"),
            ("weekly", "ok"),
        ]
    );
    for (r, a) in rows.iter().zip(&accounts) {
        assert_eq!(r.path, a.path);
        assert!(
            (before..=after).contains(&r.read_at),
            "{} read_at {} not in {before}..={after}",
            r.label,
            r.read_at
        );
    }
}

#[test]
fn read_all_reads_the_accounts_at_once() {
    // Each `meet*` fake blocks reading its own FIFO until EOF. The release
    // thread opens the three FIFOs for writing one after another, and each
    // open blocks until that fake has opened its FIFO for reading. Only once
    // all three are open does it close them, which gives every fake its EOF
    // together. A serial `read_all` starts `meet2` only after `meet1` has
    // finished, so `meet1` waits with its FIFO held open and no EOF until its
    // deadline, and reads as `timeout`. There is no give-up in the barrier:
    // the per-account deadline alone decides the outcome.
    let d = scratch();
    let m = d.path().join("m");
    let bin = by_name(d.path(), &m);
    let names = ["meet1", "meet2", "meet3"];
    let fifos: Vec<PathBuf> = names.iter().map(|n| m.join(n)).collect();
    for f in &fifos {
        mkfifo(f);
    }
    let to_open = fifos.clone();
    let release = std::thread::spawn(move || {
        let held: Vec<fs::File> = to_open
            .iter()
            .map(|f| fs::OpenOptions::new().write(true).open(f).unwrap())
            .collect();
        drop(held);
    });
    let root = d.path().join("profiles");
    let accounts = names.map(|n| acct(&root, n));
    let rows = read_all(bin.as_os_str(), &accounts, Duration::from_secs(30));
    // A fake that never opened its FIFO leaves the release thread blocked in
    // `open`. Readers of our own, held until the thread has ended, let every
    // open it has left return at once.
    let unblock: Vec<fs::File> = fifos
        .iter()
        .map(|f| {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(f)
                .unwrap()
        })
        .collect();
    release.join().unwrap();
    drop(unblock);
    for r in &rows {
        assert_eq!(kind(&r.outcome), "ok", "{}: {:?}", r.label, r.outcome);
    }
}

#[test]
fn read_all_drops_home_codex_when_it_is_not_signed_in() {
    let d = scratch();
    let root = d.path().join("profiles");
    let home = d.path().join("home");
    let bin = by_name(d.path(), &d.path().join("m"));
    // A profile that is not signed in stays; only `~/.codex` is dropped.
    let accounts = [acct(&root, "nosign"), {
        let mut h = home_acct(&home);
        // Point the home row at a directory the fake answers "not signed in".
        h.path = account(&home, "nosign");
        h
    }];
    let rows = read_all(bin.as_os_str(), &accounts, SECS_10);
    let got: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(got, ["nosign"]);
}

#[test]
fn read_all_keeps_home_codex_last_when_signed_in_or_failed() {
    let d = scratch();
    let root = d.path().join("profiles");
    let home = d.path().join("home");
    let bin = by_name(d.path(), &d.path().join("m"));
    let rows = read_all(
        bin.as_os_str(),
        &[acct(&root, "two"), home_acct(&home)],
        SECS_10,
    );
    let got: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.label.as_str(), kind(&r.outcome)))
        .collect();
    assert_eq!(got, [("two", "ok"), ("~/.codex", "ok")]);

    let mut slow_home = home_acct(&home);
    slow_home.path = account(&home, "slow");
    let rows = read_all(bin.as_os_str(), &[slow_home], SECS_1);
    assert_eq!(rows.len(), 1, "a timed-out ~/.codex is shown");
    assert_eq!(rows[0].outcome, Outcome::Timeout);
}

#[test]
fn read_all_of_nothing_is_nothing() {
    let d = scratch();
    let bin = d.path().join("never-run");
    assert!(read_all(bin.as_os_str(), &[], SECS_1).is_empty());
}
