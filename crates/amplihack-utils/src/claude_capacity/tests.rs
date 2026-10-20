//! Reader-side contract for the no-capacity notice: the effective state and
//! its expiry, the hardened reader, root resolution and the auto-switch gate,
//! the warning wording, and once-per-state-change dedupe.
//!
//! Every test uses a temporary root (mode 0700) and, where `HOME` matters, a
//! temporary `HOME`. Nothing reads or writes the real `~/.claude`,
//! `~/.claude-profiles` or `~/.amplihack`. Tests that touch the environment
//! hold the crate-wide `env_lock()`.

use super::dedupe;
use super::*;
use chrono::{DateTime, TimeZone, Utc};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;
use std::time::Duration;

const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn lock() -> MutexGuard<'static, ()> {
    crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Sets or removes variables until dropped. Hold [`lock`] while it lives.
struct Env(Vec<(&'static str, Option<OsString>)>);

impl Env {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn set(mut self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        self.0.push((key, std::env::var_os(key)));
        unsafe { std::env::set_var(key, value) };
        self
    }
    fn unset(mut self, key: &'static str) -> Self {
        self.0.push((key, std::env::var_os(key)));
        unsafe { std::env::remove_var(key) };
        self
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..).rev() {
            match value {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

fn ts(epoch: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(epoch, 0).unwrap()
}

fn now() -> i64 {
    Utc::now().timestamp()
}

struct Root {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Root {
    fn state_dir(&self) -> PathBuf {
        self.root.join(STATE_DIR)
    }
    fn capacity(&self) -> PathBuf {
        path(&self.root)
    }
}

fn private_dir(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

fn private_file(p: &Path, bytes: &[u8]) {
    fs::write(p, bytes).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

/// A private profile root with a private, empty state directory.
fn root() -> Root {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profiles");
    private_dir(&root);
    let root = root.canonicalize().unwrap();
    private_dir(&root.join(STATE_DIR));
    Root { _tmp: tmp, root }
}

fn file(state: State, reset: Option<i64>, written: i64) -> CapacityFile {
    CapacityFile {
        version: VERSION,
        state,
        threshold: 90,
        over: vec!["alpha".into(), "beta".into()],
        earliest_reset: reset.map(ts),
        written_at: ts(written),
        writer: "statusline".into(),
    }
}

fn plant(r: &Root, f: &CapacityFile) {
    private_file(&r.capacity(), &serde_json::to_vec(f).unwrap());
}

fn plant_raw(r: &Root, bytes: &[u8]) {
    private_file(&r.capacity(), bytes);
}

fn set_toggle_file(r: &Root, body: &str) {
    private_file(&r.root.join(TOGGLE_FILE), body.as_bytes());
}

// ---------------------------------------------------------------------------
// State ordering and wire format
// ---------------------------------------------------------------------------

#[test]
fn states_are_ordered_ok_low_exhausted() {
    assert!(State::Ok < State::Low);
    assert!(State::Low < State::Exhausted);
    assert_eq!(State::Ok.as_str(), "ok");
    assert_eq!(State::Low.as_str(), "low");
    assert_eq!(State::Exhausted.as_str(), "exhausted");
}

#[test]
fn file_serializes_to_the_documented_schema() {
    let f = CapacityFile {
        version: 1,
        state: State::Low,
        threshold: 90,
        over: vec!["profA".into(), "profB".into()],
        earliest_reset: Some(Utc.with_ymd_and_hms(2026, 10, 22, 9, 0, 0).unwrap()),
        written_at: Utc.with_ymd_and_hms(2026, 10, 20, 14, 3, 11).unwrap(),
        writer: "statusline".into(),
    };
    let v: serde_json::Value = serde_json::to_value(&f).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "version": 1,
            "state": "low",
            "threshold": 90,
            "over": ["profA", "profB"],
            "earliest_reset": "2026-10-22T09:00:00Z",
            "written_at": "2026-10-20T14:03:11Z",
            "writer": "statusline"
        })
    );
    let back: CapacityFile = serde_json::from_value(v).unwrap();
    assert_eq!(back, f);
}

#[test]
fn a_null_reset_round_trips() {
    let f = file(State::Exhausted, None, 1_000_000);
    let v = serde_json::to_value(&f).unwrap();
    assert!(v["earliest_reset"].is_null(), "{v}");
    assert_eq!(serde_json::from_value::<CapacityFile>(v).unwrap(), f);
}

// ---------------------------------------------------------------------------
// effective_state
// ---------------------------------------------------------------------------

#[test]
fn ok_is_always_ok() {
    let t = 2_000_000_000;
    assert_eq!(
        effective_state(&file(State::Ok, Some(t + HOUR), t), ts(t)),
        State::Ok
    );
    assert_eq!(effective_state(&file(State::Ok, None, t), ts(t)), State::Ok);
}

#[test]
fn low_and_exhausted_hold_until_the_earliest_reset() {
    let t = 2_000_000_000;
    for state in [State::Low, State::Exhausted] {
        let f = file(state, Some(t + HOUR), t);
        assert_eq!(effective_state(&f, ts(t)), state);
        assert_eq!(effective_state(&f, ts(t + HOUR - 1)), state);
    }
}

#[test]
fn one_account_past_its_reset_means_ok_with_no_rewrite() {
    let t = 2_000_000_000;
    let r = root();
    for state in [State::Low, State::Exhausted] {
        let f = file(state, Some(t + HOUR), t);
        assert_eq!(effective_state(&f, ts(t + HOUR)), State::Ok, "at the reset");
        assert_eq!(effective_state(&f, ts(t + DAY)), State::Ok, "after it");
    }
    // Reading the effective state is pure: the file on disk is untouched.
    let live = now();
    let f = file(State::Low, Some(live - 1), live - HOUR);
    plant(&r, &f);
    let before = fs::read(r.capacity()).unwrap();
    let read_back = read(&r.root).expect("valid file");
    assert_eq!(effective_state(&read_back, Utc::now()), State::Ok);
    assert_eq!(fs::read(r.capacity()).unwrap(), before, "no rewrite");
}

#[test]
fn an_unknown_reset_is_trusted_for_five_hours_only() {
    let t = 2_000_000_000;
    assert_eq!(UNKNOWN_RESET_TTL_SECS, 5 * HOUR);
    for state in [State::Low, State::Exhausted] {
        let f = file(state, None, t);
        assert_eq!(effective_state(&f, ts(t)), state);
        assert_eq!(effective_state(&f, ts(t + 5 * HOUR - 1)), state);
        assert_eq!(
            effective_state(&f, ts(t + 5 * HOUR + 1)),
            State::Ok,
            "a null reset older than 5h is ok"
        );
    }
}

#[test]
fn a_known_reset_is_not_cut_short_by_the_five_hour_bound() {
    // The 5-hour bound applies only to an unknown reset.
    let t = 2_000_000_000;
    let f = file(State::Low, Some(t + 3 * DAY), t);
    assert_eq!(effective_state(&f, ts(t + 6 * HOUR)), State::Low);
}

#[test]
fn extreme_timestamps_never_panic_and_overflow_means_ok() {
    let mut f = file(State::Low, None, 0);
    f.written_at = DateTime::<Utc>::MAX_UTC;
    assert_eq!(effective_state(&f, Utc::now()), State::Ok, "overflow is ok");
    f.earliest_reset = Some(DateTime::<Utc>::MAX_UTC);
    let _ = effective_state(&f, DateTime::<Utc>::MIN_UTC);
    let _ = effective_state(&f, DateTime::<Utc>::MAX_UTC);
}

// ---------------------------------------------------------------------------
// read: every bad file is None
// ---------------------------------------------------------------------------

#[test]
fn reads_a_valid_file() {
    let r = root();
    let t = now();
    let f = file(State::Low, Some(t + HOUR), t);
    plant(&r, &f);
    assert_eq!(read(&r.root), Some(f));
}

#[test]
fn missing_file_or_state_dir_is_none() {
    let r = root();
    assert_eq!(read(&r.root), None, "no capacity.json");
    fs::remove_dir(r.state_dir()).unwrap();
    assert_eq!(read(&r.root), None, "no state directory");
    assert_eq!(read(&r.root.join("does-not-exist")), None, "no root");
}

#[test]
fn empty_or_non_json_is_none() {
    let r = root();
    for bytes in [
        &b""[..],
        b"not json",
        b"{",
        b"[]",
        b"null",
        b"{\"version\":1}",
        b"\xff\xfe\x00",
    ] {
        plant_raw(&r, bytes);
        assert_eq!(read(&r.root), None, "{:?}", String::from_utf8_lossy(bytes));
    }
}

#[test]
fn oversized_file_is_none_even_when_it_is_valid_json() {
    let r = root();
    let t = now();
    let mut v = serde_json::to_value(file(State::Low, Some(t + HOUR), t)).unwrap();
    v["padding"] = serde_json::Value::String("x".repeat(MAX_BYTES as usize));
    let bytes = serde_json::to_vec(&v).unwrap();
    assert!(bytes.len() as u64 > MAX_BYTES);
    plant_raw(&r, &bytes);
    assert_eq!(read(&r.root), None);
}

#[test]
fn a_file_just_under_the_cap_is_read() {
    let r = root();
    let t = now();
    let mut v = serde_json::to_value(file(State::Low, Some(t + HOUR), t)).unwrap();
    v["padding"] = serde_json::Value::String(String::new());
    let base = serde_json::to_vec(&v).unwrap().len();
    v["padding"] = serde_json::Value::String("x".repeat(MAX_BYTES as usize - base));
    let bytes = serde_json::to_vec(&v).unwrap();
    assert_eq!(bytes.len() as u64, MAX_BYTES);
    plant_raw(&r, &bytes);
    assert!(read(&r.root).is_some(), "exactly 16 KiB is allowed");
}

#[test]
fn unknown_extra_fields_still_parse() {
    let r = root();
    let t = now();
    let mut v = serde_json::to_value(file(State::Exhausted, Some(t + HOUR), t)).unwrap();
    v["future_field"] = serde_json::json!({"nested": [1, 2, 3]});
    plant_raw(&r, &serde_json::to_vec(&v).unwrap());
    assert_eq!(
        read(&r.root).map(|f| f.state),
        Some(State::Exhausted),
        "a newer writer must not silence this reader"
    );
}

#[test]
fn a_symlinked_file_is_none() {
    let r = root();
    let t = now();
    let elsewhere = r.root.join("elsewhere.json");
    private_file(
        &elsewhere,
        &serde_json::to_vec(&file(State::Low, Some(t + HOUR), t)).unwrap(),
    );
    std::os::unix::fs::symlink(&elsewhere, r.capacity()).unwrap();
    assert_eq!(read(&r.root), None, "never follow a symlink");
}

#[test]
fn a_directory_in_place_of_the_file_is_none() {
    let r = root();
    fs::create_dir(r.capacity()).unwrap();
    assert_eq!(read(&r.root), None);
}

#[test]
fn a_fifo_is_none_and_does_not_hang() {
    let r = root();
    let c = std::ffi::CString::new(r.capacity().as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let root = r.root.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(read(&root));
    });
    let got = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("read must not block on a FIFO");
    assert_eq!(got, None);
}

#[test]
fn wrong_version_is_none() {
    let r = root();
    let t = now();
    for version in [0, 2, 99] {
        let mut f = file(State::Low, Some(t + HOUR), t);
        f.version = version;
        plant(&r, &f);
        assert_eq!(read(&r.root), None, "version {version}");
    }
}

#[test]
fn threshold_outside_1_to_100_is_none() {
    let r = root();
    let t = now();
    for threshold in [0u8, 101, 200, 255] {
        let mut f = file(State::Low, Some(t + HOUR), t);
        f.threshold = threshold;
        plant(&r, &f);
        assert_eq!(read(&r.root), None, "threshold {threshold}");
    }
    for threshold in [1u8, 100] {
        let mut f = file(State::Low, Some(t + HOUR), t);
        f.threshold = threshold;
        plant(&r, &f);
        assert!(read(&r.root).is_some(), "threshold {threshold}");
    }
}

#[test]
fn unknown_state_word_is_none() {
    let r = root();
    let t = now();
    let mut v = serde_json::to_value(file(State::Low, Some(t + HOUR), t)).unwrap();
    v["state"] = "critical".into();
    plant_raw(&r, &serde_json::to_vec(&v).unwrap());
    assert_eq!(read(&r.root), None);
}

#[test]
fn written_at_far_in_the_future_is_none() {
    let r = root();
    let t = now();
    plant(&r, &file(State::Low, Some(t + 2 * HOUR), t + HOUR));
    assert_eq!(read(&r.root), None, "written 1h in the future");
    plant(&r, &file(State::Low, Some(t + 2 * HOUR), t + 60));
    assert!(read(&r.root).is_some(), "1 minute of clock skew is allowed");
}

#[test]
fn a_reset_more_than_eight_days_out_counts_as_unknown() {
    let r = root();
    let t = now();
    plant(&r, &file(State::Low, Some(t + 9 * DAY), t));
    let f = read(&r.root).expect("the file itself is fine");
    assert_eq!(f.earliest_reset, None);
    assert_eq!(f.state, State::Low);

    plant_raw(
        &r,
        serde_json::to_string(&serde_json::json!({
            "version": 1, "state": "exhausted", "threshold": 90, "over": ["a"],
            "earliest_reset": "9999-12-31T23:59:59Z",
            "written_at": ts(t).to_rfc3339(), "writer": "rotate"
        }))
        .unwrap()
        .as_bytes(),
    );
    assert_eq!(read(&r.root).and_then(|f| f.earliest_reset), None);
}

#[test]
fn a_maximum_date_reset_never_panics() {
    let r = root();
    let t = now();
    let mut f = file(State::Low, None, t);
    f.earliest_reset = Some(DateTime::<Utc>::MAX_UTC);
    plant(&r, &f);
    // Either the date does not parse (None) or it counts as unknown.
    if let Some(f) = read(&r.root) {
        assert_eq!(f.earliest_reset, None);
    }
}

// ---------------------------------------------------------------------------
// resolve_root_with: matches root_config::resolve_with, but fails closed
// ---------------------------------------------------------------------------

fn no_env(_: &str) -> Option<OsString> {
    None
}

fn env_root(value: impl Into<OsString>) -> impl Fn(&str) -> Option<OsString> {
    let value = value.into();
    move |k| (k == ROOT_ENV).then(|| value.clone())
}

fn saved(home: &Path, bytes: &[u8], mode: u32) -> PathBuf {
    let dir = home.join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("profile-root");
    fs::write(&f, bytes).unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(mode)).unwrap();
    f
}

#[test]
fn root_from_the_variable() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        resolve_root_with(env_root("/srv/profiles"), Some(home.path())),
        Some(PathBuf::from("/srv/profiles"))
    );
}

#[test]
fn a_bad_variable_is_none_never_the_default() {
    use std::os::unix::ffi::OsStringExt;
    let home = tempfile::tempdir().unwrap();
    for bad in [
        OsString::from("relative/profiles"),
        OsString::from("/srv/../etc"),
        OsString::from("/srv/pro\u{7}files"),
        OsString::from("/srv/pro\nfiles"),
        OsString::from_vec(b"/srv/\xffprofiles".to_vec()),
    ] {
        assert_eq!(
            resolve_root_with(env_root(bad.clone()), Some(home.path())),
            None,
            "{bad:?}"
        );
    }
}

#[test]
fn an_empty_variable_falls_through_to_the_default() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        resolve_root_with(env_root(""), Some(home.path())),
        Some(home.path().join(".amplihack").join("claude-profiles"))
    );
}

#[test]
fn root_from_the_saved_file_then_the_default() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        resolve_root_with(no_env, Some(home.path())),
        Some(home.path().join(".amplihack").join("claude-profiles")),
        "default"
    );
    saved(home.path(), b"/srv/saved-profiles\n", 0o600);
    assert_eq!(
        resolve_root_with(no_env, Some(home.path())),
        Some(PathBuf::from("/srv/saved-profiles")),
        "saved"
    );
}

#[test]
fn a_bad_saved_file_is_none_never_the_default() {
    type Plant = Box<dyn Fn(&Path)>;
    let cases: Vec<(&str, Plant)> = vec![
        (
            "group-writable",
            Box::new(|h: &Path| {
                saved(h, b"/srv/p\n", 0o620);
            }),
        ),
        (
            "other-writable",
            Box::new(|h: &Path| {
                saved(h, b"/srv/p\n", 0o602);
            }),
        ),
        (
            "relative",
            Box::new(|h: &Path| {
                saved(h, b"srv/p\n", 0o600);
            }),
        ),
        (
            "dot-dot",
            Box::new(|h: &Path| {
                saved(h, b"/srv/../p\n", 0o600);
            }),
        ),
        (
            "control character",
            Box::new(|h: &Path| {
                saved(h, b"/srv/\x1bp\n", 0o600);
            }),
        ),
        (
            "non-UTF-8",
            Box::new(|h: &Path| {
                saved(h, b"/srv/\xffp\n", 0o600);
            }),
        ),
        (
            "too large",
            Box::new(|h: &Path| {
                let mut big = b"/srv/".to_vec();
                big.extend(std::iter::repeat_n(b'p', 5000));
                saved(h, &big, 0o600);
            }),
        ),
        (
            "symlink",
            Box::new(|h: &Path| {
                let real = h.join("real-root-file");
                fs::write(&real, b"/srv/p\n").unwrap();
                fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
                fs::create_dir_all(h.join(".amplihack")).unwrap();
                std::os::unix::fs::symlink(&real, h.join(".amplihack/profile-root")).unwrap();
            }),
        ),
    ];
    for (why, plant) in cases {
        let home = tempfile::tempdir().unwrap();
        plant(home.path());
        assert_eq!(resolve_root_with(no_env, Some(home.path())), None, "{why}");
    }
}

#[test]
fn no_home_and_no_variable_is_none() {
    assert_eq!(resolve_root_with(no_env, None), None);
}

#[test]
fn resolve_root_reads_the_process_environment() {
    let _l = lock();
    let home = tempfile::tempdir().unwrap();
    let _e = Env::new()
        .set("HOME", home.path())
        .set(ROOT_ENV, "/srv/from-env");
    assert_eq!(resolve_root(), Some(PathBuf::from("/srv/from-env")));
    let _e2 = Env::new().set(ROOT_ENV, "not-absolute");
    assert_eq!(resolve_root(), None, "fails closed");
}

// ---------------------------------------------------------------------------
// auto_switch_on
// ---------------------------------------------------------------------------

#[test]
fn gate_env_wins_then_the_file() {
    let _l = lock();
    let r = root();
    let cases: &[(Option<&str>, Option<&str>, bool)] = &[
        (Some("1"), None, true),
        (Some("1"), Some("enabled=off\n"), true),
        (Some("0"), Some("enabled=on\n"), false),
        (Some("0"), None, false),
        (None, Some("enabled=on\n"), true),
        (None, Some("enabled=on\nthreshold=80\nexclude=a,b\n"), true),
        (None, Some("enabled=off\n"), false),
        (None, None, false),
        (None, Some("garbage"), false),
        (Some("yes"), Some("enabled=on\n"), true),
        (Some("yes"), Some("enabled=off\n"), false),
        (Some("true"), None, false),
        (Some(""), Some("enabled=on\n"), true),
    ];
    for (env, body, want) in cases {
        let _ = fs::remove_file(r.root.join(TOGGLE_FILE));
        if let Some(body) = body {
            set_toggle_file(&r, body);
        }
        let _e = match env {
            Some(v) => Env::new().set(TOGGLE_ENV, v),
            None => Env::new().unset(TOGGLE_ENV),
        };
        assert_eq!(auto_switch_on(&r.root), *want, "env {env:?}, file {body:?}");
    }
}

#[test]
fn gate_refuses_a_symlinked_toggle_file() {
    let _l = lock();
    let r = root();
    let real = r.root.join("real-toggle");
    private_file(&real, b"enabled=on\n");
    std::os::unix::fs::symlink(&real, r.root.join(TOGGLE_FILE)).unwrap();
    let _e = Env::new().unset(TOGGLE_ENV);
    assert!(!auto_switch_on(&r.root));
}

#[test]
fn gate_refuses_an_oversized_toggle_file() {
    let _l = lock();
    let r = root();
    let mut body = "enabled=on\n".to_string();
    body.push_str(&"#".repeat(5000));
    set_toggle_file(&r, &body);
    let _e = Env::new().unset(TOGGLE_ENV);
    assert!(!auto_switch_on(&r.root));
}

// ---------------------------------------------------------------------------
// current
// ---------------------------------------------------------------------------

#[test]
fn current_is_none_while_auto_switch_is_off() {
    let _l = lock();
    let r = root();
    let t = now();
    plant(&r, &file(State::Low, Some(t + HOUR), t));
    let _e = Env::new().set(TOGGLE_ENV, "0");
    assert_eq!(current(&r.root, Utc::now()), None);
}

#[test]
fn current_gives_the_effective_state_while_on() {
    let _l = lock();
    let r = root();
    let t = now();
    let _e = Env::new().set(TOGGLE_ENV, "1");
    plant(&r, &file(State::Exhausted, Some(t + HOUR), t));
    assert_eq!(
        current(&r.root, ts(t)).map(|(s, _)| s),
        Some(State::Exhausted)
    );
    assert_eq!(
        current(&r.root, ts(t + HOUR)).map(|(s, _)| s),
        Some(State::Ok),
        "past the reset"
    );
}

#[test]
fn current_is_none_for_a_corrupt_or_missing_file() {
    let _l = lock();
    let r = root();
    let _e = Env::new().set(TOGGLE_ENV, "1");
    assert_eq!(current(&r.root, Utc::now()), None);
    plant_raw(&r, b"{corrupt");
    assert_eq!(current(&r.root, Utc::now()), None);
}

// ---------------------------------------------------------------------------
// Wording
// ---------------------------------------------------------------------------

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

#[test]
fn reset_times_in_local_time() {
    let _l = lock();
    let _e = Env::new().set("TZ", "UTC");
    assert_eq!(format_reset(Some(utc(2026, 10, 22, 2, 0))), "Oct 22 2am");
    assert_eq!(
        format_reset(Some(utc(2026, 10, 22, 2, 30))),
        "Oct 22 2:30am"
    );
    assert_eq!(
        format_reset(Some(utc(2026, 10, 22, 14, 5))),
        "Oct 22 2:05pm"
    );
    assert_eq!(format_reset(Some(utc(2026, 11, 3, 0, 0))), "Nov 3 12am");
    assert_eq!(format_reset(Some(utc(2026, 11, 3, 12, 0))), "Nov 3 12pm");
    assert_eq!(format_reset(None), "unknown");
}

#[test]
fn reset_times_follow_the_local_zone() {
    let _l = lock();
    let _e = Env::new().set("TZ", "America/New_York");
    // 09:00Z on 22 Oct 2026 is 05:00 EDT.
    assert_eq!(format_reset(Some(utc(2026, 10, 22, 9, 0))), "Oct 22 5am");
}

#[test]
fn low_line_names_the_actual_threshold() {
    let _l = lock();
    let _e = Env::new().set("TZ", "UTC");
    let reset = Some(utc(2026, 10, 22, 2, 0));
    assert_eq!(
        format_line(State::Low, 90, reset).as_deref(),
        Some("⚠ all Claude accounts ≥90% · next reset Oct 22 2am")
    );
    assert_eq!(
        format_line(State::Low, 75, reset).as_deref(),
        Some("⚠ all Claude accounts ≥75% · next reset Oct 22 2am")
    );
}

#[test]
fn exhausted_line_says_at_limit() {
    let _l = lock();
    let _e = Env::new().set("TZ", "UTC");
    assert_eq!(
        format_line(State::Exhausted, 90, Some(utc(2026, 10, 22, 2, 30))).as_deref(),
        Some("⚠ all Claude accounts at limit · next reset Oct 22 2:30am")
    );
}

#[test]
fn unknown_reset_wording() {
    assert_eq!(
        format_line(State::Low, 90, None).as_deref(),
        Some("⚠ all Claude accounts ≥90% · next reset unknown")
    );
    assert_eq!(
        format_line(State::Exhausted, 90, None).as_deref(),
        Some("⚠ all Claude accounts at limit · next reset unknown")
    );
}

#[test]
fn ok_has_no_line() {
    assert_eq!(format_line(State::Ok, 90, None), None);
    assert_eq!(
        format_line(State::Ok, 90, Some(utc(2026, 1, 1, 0, 0))),
        None
    );
}

#[test]
fn inspect_command_is_profile_usage() {
    assert_eq!(INSPECT, "amplihack profile usage");
}

// ---------------------------------------------------------------------------
// nested_session
// ---------------------------------------------------------------------------

#[test]
fn nested_session_is_a_positive_depth() {
    let _l = lock();
    for (value, want) in [
        (Some("1"), true),
        (Some("7"), true),
        (Some("0"), false),
        (Some(""), false),
        (Some("abc"), false),
        (Some("-1"), false),
        (None, false),
    ] {
        let _e = match value {
            Some(v) => Env::new().set("AMPLIHACK_SESSION_DEPTH", v),
            None => Env::new().unset("AMPLIHACK_SESSION_DEPTH"),
        };
        assert_eq!(nested_session(), want, "{value:?}");
    }
}

// ---------------------------------------------------------------------------
// dedupe
// ---------------------------------------------------------------------------

fn record(r: &Root, session: &str) -> PathBuf {
    dedupe::dir(&r.root).join(dedupe::record_name(session).unwrap())
}

#[test]
fn record_names_are_sanitized_and_bounded() {
    assert_eq!(dedupe::record_name(""), None);
    assert_eq!(dedupe::record_name("   "), None);
    assert_eq!(dedupe::record_name("\t\n"), None);
    assert_eq!(
        dedupe::record_name("abc-123_X").as_deref(),
        Some("abc-123_X")
    );
    let hostile = dedupe::record_name("../../etc/passwd").unwrap();
    assert!(!hostile.contains('/'), "{hostile}");
    assert!(!hostile.contains(".."), "{hostile}");
    let long = dedupe::record_name(&"s".repeat(1000)).unwrap();
    assert!(long.len() <= dedupe::MAX_NAME, "{}", long.len());
    assert!(!long.is_empty());
}

#[test]
fn notice_once_quiet_on_repeat_again_when_worse_and_again_after_ok() {
    let r = root();
    let s = Some("session-1");
    assert!(
        dedupe::should_show(&r.root, s, State::Low),
        "first low shows"
    );
    assert!(
        !dedupe::should_show(&r.root, s, State::Low),
        "repeat is quiet"
    );
    assert!(
        dedupe::should_show(&r.root, s, State::Exhausted),
        "worse shows again"
    );
    assert!(!dedupe::should_show(&r.root, s, State::Exhausted));
    assert!(
        !dedupe::should_show(&r.root, s, State::Low),
        "better but still bad is quiet"
    );
    assert!(
        !dedupe::should_show(&r.root, s, State::Ok),
        "ok never shows"
    );
    assert!(!record(&r, "session-1").exists(), "ok clears the record");
    assert!(
        dedupe::should_show(&r.root, s, State::Low),
        "a later drop shows again"
    );
}

#[test]
fn records_are_private_and_hold_one_word() {
    let r = root();
    assert!(dedupe::should_show(&r.root, Some("s1"), State::Low));
    let rec = record(&r, "s1");
    assert_eq!(fs::read_to_string(&rec).unwrap().trim(), "low");
    assert_eq!(fs::metadata(&rec).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        fs::metadata(dedupe::dir(&r.root)).unwrap().mode() & 0o777,
        0o700
    );
    assert!(dedupe::should_show(&r.root, Some("s1"), State::Exhausted));
    assert_eq!(fs::read_to_string(&rec).unwrap().trim(), "exhausted");
}

#[test]
fn sessions_are_independent() {
    let r = root();
    assert!(dedupe::should_show(&r.root, Some("a"), State::Low));
    assert!(dedupe::should_show(&r.root, Some("b"), State::Low));
    assert!(!dedupe::should_show(&r.root, Some("a"), State::Low));
}

#[test]
fn no_session_id_always_shows_and_writes_nothing() {
    let r = root();
    for _ in 0..3 {
        assert!(dedupe::should_show(&r.root, None, State::Low));
        assert!(dedupe::should_show(&r.root, None, State::Exhausted));
    }
    assert!(!dedupe::should_show(&r.root, None, State::Ok));
    let entries = fs::read_dir(dedupe::dir(&r.root))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(entries, 0);
}

#[test]
fn an_empty_session_id_does_not_panic_and_shows() {
    let r = root();
    for id in ["", "   "] {
        assert!(dedupe::should_show(&r.root, Some(id), State::Low));
        assert!(
            dedupe::should_show(&r.root, Some(id), State::Low),
            "no dedupe"
        );
        assert!(!dedupe::should_show(&r.root, Some(id), State::Ok));
    }
}

#[test]
fn a_corrupt_or_oversized_record_counts_as_none() {
    let r = root();
    assert!(dedupe::should_show(&r.root, Some("s"), State::Low));
    let rec = record(&r, "s");
    for bytes in [b"garbage".to_vec(), b"ok".to_vec(), b"".to_vec(), {
        let mut v = b"low".to_vec();
        v.extend(std::iter::repeat_n(b' ', 100));
        v
    }] {
        private_file(&rec, &bytes);
        assert!(
            dedupe::should_show(&r.root, Some("s"), State::Low),
            "{:?}",
            String::from_utf8_lossy(&bytes)
        );
    }
}

#[test]
fn creates_the_state_and_notice_directories_privately() {
    let r = root();
    fs::remove_dir(r.state_dir()).unwrap();
    assert!(dedupe::should_show(&r.root, Some("s"), State::Low));
    assert_eq!(fs::metadata(r.state_dir()).unwrap().mode() & 0o777, 0o700);
    assert!(!dedupe::should_show(&r.root, Some("s"), State::Low));
}

#[test]
fn a_symlinked_notice_dir_is_never_written_through() {
    let r = root();
    let outside = r._tmp.path().join("outside");
    private_dir(&outside);
    std::os::unix::fs::symlink(&outside, dedupe::dir(&r.root)).unwrap();
    assert!(
        dedupe::should_show(&r.root, Some("s"), State::Low),
        "still shown"
    );
    assert!(
        dedupe::should_show(&r.root, Some("s"), State::Low),
        "not recorded"
    );
    assert_eq!(
        fs::read_dir(&outside).unwrap().count(),
        0,
        "nothing outside"
    );
}

#[test]
fn ok_does_not_remove_through_a_symlinked_notice_dir() {
    let r = root();
    let outside = r._tmp.path().join("outside");
    private_dir(&outside);
    let name = dedupe::record_name("s").unwrap();
    private_file(&outside.join(&name), b"low");
    std::os::unix::fs::symlink(&outside, dedupe::dir(&r.root)).unwrap();
    assert!(!dedupe::should_show(&r.root, Some("s"), State::Ok));
    assert!(
        outside.join(&name).exists(),
        "file outside the root is kept"
    );
}

#[test]
fn a_shared_notice_dir_is_not_written() {
    let r = root();
    let d = dedupe::dir(&r.root);
    fs::create_dir(&d).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(dedupe::should_show(&r.root, Some("s"), State::Low));
    assert!(
        dedupe::should_show(&r.root, Some("s"), State::Low),
        "no record in a group/other-readable dir"
    );
    assert_eq!(fs::read_dir(&d).unwrap().count(), 0);
}

#[test]
fn a_record_that_is_a_symlink_is_not_followed() {
    let r = root();
    private_dir(&dedupe::dir(&r.root));
    let target = r._tmp.path().join("target");
    private_file(&target, b"exhausted");
    std::os::unix::fs::symlink(&target, record(&r, "s")).unwrap();
    // The symlink's "exhausted" must not count as a record, and writing the
    // new record must not write through it.
    assert!(dedupe::should_show(&r.root, Some("s"), State::Low));
    assert_eq!(fs::read(&target).unwrap(), b"exhausted");
}

/// Sets the modification time of `p` (not following symlinks) to `secs_ago`.
fn age(p: &Path, secs_ago: i64) {
    let t = libc::timespec {
        tv_sec: (now() - secs_ago) as libc::time_t,
        tv_nsec: 0,
    };
    let times = [t, t];
    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
    let rc = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    assert_eq!(rc, 0, "utimensat {}", p.display());
}

#[test]
fn writing_a_record_prunes_stale_ones_and_keeps_fresh_ones() {
    let r = root();
    let d = dedupe::dir(&r.root);
    private_dir(&d);
    for i in 0..3 {
        let p = d.join(format!("stale-{i}"));
        private_file(&p, b"low");
        age(&p, 8 * DAY);
    }
    let fresh = d.join("fresh");
    private_file(&fresh, b"low");
    age(&fresh, DAY);
    assert!(dedupe::should_show(&r.root, Some("writer"), State::Low));
    for i in 0..3 {
        assert!(!d.join(format!("stale-{i}")).exists(), "stale-{i} pruned");
    }
    assert!(fresh.exists(), "under 7 days is kept");
    assert!(record(&r, "writer").exists());
}

#[test]
fn prune_never_follows_or_deletes_symlinks_or_directories() {
    let r = root();
    let d = dedupe::dir(&r.root);
    private_dir(&d);
    let target = r._tmp.path().join("precious");
    private_file(&target, b"keep me");
    age(&target, 30 * DAY);
    let link = d.join("old-link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    age(&link, 30 * DAY);
    let sub = d.join("old-subdir");
    private_dir(&sub);
    let inner = sub.join("inner");
    private_file(&inner, b"low");
    age(&inner, 30 * DAY);
    age(&sub, 30 * DAY);

    assert!(dedupe::should_show(&r.root, Some("writer"), State::Low));
    assert_eq!(fs::read(&target).unwrap(), b"keep me");
    assert!(
        fs::symlink_metadata(&link).is_ok(),
        "the link is left alone"
    );
    assert!(inner.exists(), "no recursion");
}

#[test]
fn prune_examines_at_most_256_entries() {
    let r = root();
    let d = dedupe::dir(&r.root);
    private_dir(&d);
    let n = 1000;
    for i in 0..n {
        let p = d.join(format!("stale-{i:04}"));
        private_file(&p, b"low");
        age(&p, 8 * DAY);
    }
    assert!(dedupe::should_show(&r.root, Some("writer"), State::Low));
    let left = fs::read_dir(&d)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("stale-")
        })
        .count();
    let removed = n - left;
    assert!(removed >= 1, "some stale records go");
    assert!(
        removed <= dedupe::PRUNE_SCAN,
        "removed {removed}, more than the scan cap"
    );
}

#[test]
fn ok_with_no_record_is_quiet_and_harmless() {
    let r = root();
    assert!(!dedupe::should_show(
        &r.root,
        Some("never-shown"),
        State::Ok
    ));
    fs::remove_dir(r.state_dir()).unwrap();
    assert!(!dedupe::should_show(&r.root, Some("x"), State::Ok));
    assert!(!r.state_dir().exists(), "ok creates nothing");
}
