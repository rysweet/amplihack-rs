//! Issue #1818: logins whose plan has no session limit.
//!
//! `profile usage` reads a weekly-only `/usage` output (or a session line
//! with no reset clause) as `ok` with no session window, labels it
//! `no session limit`, and writes `"session_limit": false` in `--json`.
//! `rotate status` lists such profiles from auto-switch's `usage.json` on a
//! `no session limit:` line, left out when the list is empty.
//!
//! Every command runs through the shared fixture: `isolated(..)` with a
//! temporary HOME and profile root, a fake `claude`, no tmux and no real
//! rotator. The profile root is set explicitly to a path inside the
//! temporary directory, never inherited (#1699).

#![cfg(unix)]

use crate::support::profile_rotate::{Fixture, text};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn state_dir(f: &Fixture) -> PathBuf {
    f.root.join(".auto-switch.d")
}

/// Writes auto-switch's `usage.json` (mode 0600, in a 0700 directory) with
/// one reading per `(account, five_hour, seven_day)`.
fn seed_usage(f: &Fixture, readings: &[(&str, bool, bool)], mode: u32) {
    f.assert_isolated();
    let dir = state_dir(f);
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let now = chrono::Utc::now().timestamp();
    let mut accounts = serde_json::Map::new();
    for (name, five, seven) in readings {
        let mut r = serde_json::Map::new();
        if *five {
            r.insert(
                "five_hour".into(),
                serde_json::json!({"used_percentage": 20.0, "resets_at": now + 3600}),
            );
        }
        if *seven {
            r.insert(
                "seven_day".into(),
                serde_json::json!({"used_percentage": 30.0, "resets_at": now + 86_400}),
            );
        }
        r.insert("observed_at".into(), serde_json::json!(now - 10));
        r.insert("source".into(), serde_json::json!("statusline"));
        accounts.insert(name.to_string(), serde_json::Value::Object(r));
    }
    let path = dir.join("usage.json");
    fs::write(
        &path,
        serde_json::json!({"version": 1, "accounts": accounts}).to_string(),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
}

fn rotate_status(f: &Fixture) -> (String, String) {
    let out = f.ok(&["profile", "rotate", "status"]);
    (text(&out.stdout), text(&out.stderr))
}

fn no_limit_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|l| l.starts_with("no session limit:"))
        .collect()
}

// ---------------------------------------------------------------------------
// rotate status
// ---------------------------------------------------------------------------

#[test]
fn rotate_status_lists_weekly_only_profiles_sorted() {
    let f = Fixture::new();
    seed_usage(
        &f,
        &[
            ("work", true, true),
            ("spare", false, true),
            ("extra", false, true),
        ],
        0o600,
    );
    let (stdout, stderr) = rotate_status(&f);
    assert_eq!(
        no_limit_lines(&stdout),
        ["no session limit: extra, spare"],
        "{stdout}"
    );
    assert!(!stderr.contains("usage"), "{stderr}");
}

#[test]
fn rotate_status_puts_the_line_after_on_overflow_and_before_default_and_hold() {
    let f = Fixture::new();
    seed_usage(&f, &[("spare", false, true)], 0o600);
    let (stdout, _) = rotate_status(&f);
    let lines: Vec<&str> = stdout.lines().collect();
    let at = |prefix: &str| lines.iter().position(|l| l.starts_with(prefix));
    let ours = at("no session limit:").unwrap_or_else(|| panic!("{stdout}"));
    assert_eq!(lines[ours], "no session limit: spare");
    let on_overflow = at("on overflow:").expect("on overflow line");
    assert!(on_overflow < ours, "{stdout}");
    for later in ["default:", "hold:", "alert:", "warning:", "journal:"] {
        if let Some(i) = at(later) {
            assert!(ours < i, "{later} before ours: {stdout}");
        }
    }
}

#[test]
fn rotate_status_lists_only_profiles_that_exist() {
    let f = Fixture::new();
    seed_usage(&f, &[("spare", false, true), ("ghost", false, true)], 0o600);
    let (stdout, _) = rotate_status(&f);
    assert_eq!(
        no_limit_lines(&stdout),
        ["no session limit: spare"],
        "{stdout}"
    );
}

#[test]
fn rotate_status_leaves_the_line_out_when_no_profile_qualifies() {
    let f = Fixture::new();
    // Session windows everywhere, a reading with neither window, and one
    // with only a session window.
    seed_usage(
        &f,
        &[
            ("work", true, true),
            ("spare", false, false),
            ("extra", true, false),
        ],
        0o600,
    );
    let (stdout, _) = rotate_status(&f);
    assert!(no_limit_lines(&stdout).is_empty(), "{stdout}");
    assert!(!stdout.contains("no session limit"), "{stdout}");
}

#[test]
fn rotate_status_leaves_the_line_out_with_no_usage_file() {
    let f = Fixture::new();
    let (stdout, stderr) = rotate_status(&f);
    assert!(!stdout.contains("no session limit"), "{stdout}");
    assert!(!stderr.contains("usage"), "{stderr}");
}

#[test]
fn rotate_status_ignores_an_unsafe_or_damaged_usage_file_silently() {
    let f = Fixture::new();
    seed_usage(&f, &[("spare", false, true)], 0o644);
    let (stdout, stderr) = rotate_status(&f);
    assert!(!stdout.contains("no session limit"), "{stdout}");
    assert!(
        !stderr.contains("usage.json"),
        "nothing on stderr: {stderr}"
    );

    let path = state_dir(&f).join("usage.json");
    fs::write(&path, "{not json").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let (stdout, stderr) = rotate_status(&f);
    assert!(!stdout.contains("no session limit"), "{stdout}");
    assert!(
        !stderr.contains("usage.json"),
        "nothing on stderr: {stderr}"
    );

    // T6: a symlink to a valid 0600 file is refused by the nofollow open.
    fs::remove_file(&path).unwrap();
    seed_usage(&f, &[("spare", false, true)], 0o600);
    let real = state_dir(&f).join("usage-real.json");
    fs::rename(&path, &real).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    let (stdout, stderr) = rotate_status(&f);
    assert!(!stdout.contains("no session limit"), "{stdout}");
    assert!(
        !stderr.contains("usage.json"),
        "nothing on stderr: {stderr}"
    );
}

#[test]
fn rotate_status_writes_nothing_to_usage_json() {
    let f = Fixture::new();
    seed_usage(&f, &[("spare", false, true)], 0o600);
    let path = state_dir(&f).join("usage.json");
    let before = fs::read(&path).unwrap();
    rotate_status(&f);
    assert_eq!(fs::read(&path).unwrap(), before, "read-only");
}

// ---------------------------------------------------------------------------
// profile usage
// ---------------------------------------------------------------------------

fn usage_rows(f: &Fixture, profiles: &[&str]) -> Vec<serde_json::Value> {
    let mut args = vec!["profile", "usage", "--json"];
    args.extend_from_slice(profiles);
    let out = f.ok(&args);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", text(&out.stdout)));
    v.as_array().unwrap().clone()
}

fn by_profile<'a>(rows: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    rows.iter()
        .find(|r| r["profile"] == name)
        .unwrap_or_else(|| panic!("no row for {name}: {rows:?}"))
}

#[test]
fn profile_usage_json_marks_weekly_only_and_reset_less_logins() {
    let f = Fixture::new();
    f.usage_weekly_only("spare", 30);
    f.usage_no_reset("extra", 100, 40);
    let rows = usage_rows(&f, &["work", "spare", "extra"]);

    let work = by_profile(&rows, "work");
    assert_eq!(work["session_limit"], true, "a printed reset: {work}");
    assert_eq!(work["session"]["used_percentage"], 90.0);

    for (name, week) in [("spare", 30.0), ("extra", 40.0)] {
        let r = by_profile(&rows, name);
        assert_eq!(r["state"], "ok", "{r}");
        assert_eq!(r["session_limit"], false, "{r}");
        assert_eq!(r["session"], serde_json::Value::Null, "{r}");
        assert_eq!(r["week_all"]["used_percentage"], week, "{r}");
    }
}

#[test]
fn profile_usage_table_shows_no_session_limit_for_the_weekly_only_row() {
    let f = Fixture::new();
    f.usage_weekly_only("spare", 30);
    let out = f.ok(&["profile", "usage", "work", "spare"]);
    let stdout = text(&out.stdout);
    let spare = stdout
        .lines()
        .find(|l| l.starts_with("spare"))
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(spare.contains("no session limit"), "{stdout}");
    assert!(spare.contains("30%"), "{stdout}");
    let work = stdout.lines().find(|l| l.starts_with("work")).unwrap();
    assert!(!work.contains("no session limit"), "{stdout}");
    assert_eq!(stdout.matches("no session limit").count(), 1, "{stdout}");
}

#[test]
fn a_reset_less_session_with_no_week_line_keeps_the_session_limit() {
    let f = Fixture::new();
    fs::write(
        f.canned.join(format!(
            "{}.out",
            crate::support::profile_rotate::email("spare")
        )),
        "Current session: 100% used\n",
    )
    .unwrap();
    let rows = usage_rows(&f, &["spare"]);
    let r = by_profile(&rows, "spare");
    assert_eq!(r["state"], "session_exhausted", "{r}");
    assert_eq!(r["session_limit"], true, "{r}");
}
