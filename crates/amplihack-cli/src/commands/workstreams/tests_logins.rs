//! Issue #1710 (D11): the logins section, built from `profile usage` rows.
//!
//! Contract (`logins`):
//! - `is_api_key_profile(dir) -> bool`: `settings.json` has `apiKeyHelper`
//!   or `env.ANTHROPIC_API_KEY`, or `.claude.json` has `primaryApiKey`.
//!   Files are read only when regular, owned by this user, mode
//!   `& 0o077 == 0`, and at most 4 MiB. Never based on the profile name.
//! - `login_rows(profiles: &[(String, bool)], rows: Option<&[Row]>) ->
//!   Vec<LoginRow>`: one row per profile in `profiles` (the bool marks an
//!   API-key profile), in that order, then the `~/.claude` row when `rows`
//!   holds one with `live_in_home`. `rows` is `None` when the probe did not
//!   finish by the deadline.

use super::logins::{is_api_key_profile, login_rows};
use super::render::{LoginRow, Reading};
use crate::commands::profile::usage_cmd::Row;
use crate::commands::profile::usage_parse::{UsageReport, UsageState, UsageWindow};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn window(label: &str, pct: f64, resets: &str) -> UsageWindow {
    UsageWindow {
        label: label.to_string(),
        used_percentage: pct,
        resets_text: Some(resets.to_string()),
        resets_at: None,
    }
}

fn usage_row(name: &str, state: UsageState) -> Row {
    Row {
        profile: name.to_string(),
        account: name.to_string(),
        default: false,
        leased_to: None,
        live_in: false,
        overflow: false,
        overflow_blocked_until: None,
        live_in_home: false,
        report: UsageReport {
            state,
            session: Some(window("session", 31.0, "4:50pm (Europe/London)")),
            week_all: Some(window("all models", 58.0, "Oct 30, 6pm (Europe/London)")),
            week_models: Vec::new(),
        },
        error: None,
        raw: None,
    }
}

fn profiles(names: &[(&str, bool)]) -> Vec<(String, bool)> {
    names.iter().map(|(n, k)| (n.to_string(), *k)).collect()
}

fn names(rows: &[LoginRow]) -> Vec<&str> {
    rows.iter().map(|r| r.profile.as_str()).collect()
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[test]
fn every_profile_gets_a_row_even_without_a_result() {
    let rows = login_rows(
        &profiles(&[("work", false), ("spare", false)]),
        Some(&[usage_row("work", UsageState::Ok)]),
    );
    assert_eq!(names(&rows), ["work", "spare"]);
    assert!(matches!(rows[0].reading, Reading::Bars { .. }));
    assert!(matches!(rows[1].reading, Reading::NoReading { .. }));
}

#[test]
fn a_probe_that_missed_the_deadline_leaves_every_profile_without_a_reading() {
    let rows = login_rows(&profiles(&[("a", false), ("b", false)]), None);
    assert_eq!(names(&rows), ["a", "b"]);
    assert!(
        rows.iter()
            .all(|r| matches!(r.reading, Reading::NoReading { .. })),
        "{rows:?}"
    );
}

#[test]
fn timeout_and_unparsed_states_are_no_reading() {
    for state in [UsageState::Timeout, UsageState::Unparsed] {
        let rows = login_rows(&profiles(&[("p", false)]), Some(&[usage_row("p", state)]));
        match &rows[0].reading {
            Reading::NoReading { state: shown } => assert_eq!(shown, state.as_str()),
            other => panic!("{state:?} gave {other:?}"),
        }
    }
}

#[test]
fn a_good_reading_has_both_percentages_the_state_and_the_reset() {
    let rows = login_rows(
        &profiles(&[("work", false)]),
        Some(&[usage_row("work", UsageState::Ok)]),
    );
    match &rows[0].reading {
        Reading::Bars {
            session,
            week,
            state,
            resets,
        } => {
            assert_eq!(*session, Some(31.0));
            assert_eq!(*week, Some(58.0));
            assert_eq!(state, "ok");
            let resets = resets.as_deref().unwrap();
            assert!(resets.contains("4:50pm"), "{resets}");
            assert!(!resets.contains("Europe"), "the zone is dropped: {resets}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_api_key_profile_is_never_shown_with_bars() {
    let rows = login_rows(
        &profiles(&[("platform", true)]),
        Some(&[usage_row("platform", UsageState::Ok)]),
    );
    assert!(matches!(rows[0].reading, Reading::ApiKey), "{rows:?}");
}

#[test]
fn the_live_login_is_marked() {
    let mut live = usage_row("work", UsageState::Ok);
    live.live_in = true;
    let rows = login_rows(
        &profiles(&[("work", false), ("spare", false)]),
        Some(&[live, usage_row("spare", UsageState::Ok)]),
    );
    assert!(rows[0].live);
    assert!(!rows[1].live);
}

#[test]
fn a_login_only_in_home_is_added_last_as_the_home_row() {
    let mut home = usage_row("~", UsageState::Ok);
    home.live_in_home = true;
    let rows = login_rows(
        &profiles(&[("work", false)]),
        Some(&[home, usage_row("work", UsageState::Ok)]),
    );
    assert_eq!(names(&rows), ["work", "~/.claude"]);
    assert!(rows[1].live);
}

#[test]
fn rows_for_unknown_profiles_are_ignored() {
    let rows = login_rows(
        &profiles(&[("work", false)]),
        Some(&[usage_row("intruder", UsageState::Ok)]),
    );
    assert_eq!(names(&rows), ["work"]);
}

// ---------------------------------------------------------------------------
// API-key detection
// ---------------------------------------------------------------------------

fn private(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn profile_dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}

#[test]
fn each_api_key_sign_is_recognised() {
    let d = profile_dir();
    private(
        &d.path().join("settings.json"),
        r#"{"apiKeyHelper":"/bin/key"}"#,
    );
    assert!(is_api_key_profile(d.path()));

    let d = profile_dir();
    private(
        &d.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"sk-ant-xyz"}}"#,
    );
    assert!(is_api_key_profile(d.path()));

    let d = profile_dir();
    private(
        &d.path().join(".claude.json"),
        r#"{"primaryApiKey":"sk-ant-xyz"}"#,
    );
    assert!(is_api_key_profile(d.path()));
}

#[test]
fn a_plan_login_is_not_an_api_key_profile() {
    let d = profile_dir();
    private(&d.path().join("settings.json"), r#"{"env":{"OTHER":"1"}}"#);
    private(
        &d.path().join(".claude.json"),
        r#"{"oauthAccount":{"x":1}}"#,
    );
    assert!(!is_api_key_profile(d.path()));
    assert!(!is_api_key_profile(profile_dir().path()));
}

#[test]
fn the_profile_name_plays_no_part() {
    let root = profile_dir();
    let dir = root.path().join("api-key-platform");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!is_api_key_profile(&dir));
}

#[test]
fn files_that_fail_the_ownership_and_mode_check_are_not_read() {
    let d = profile_dir();
    let settings = d.path().join("settings.json");
    private(&settings, r#"{"apiKeyHelper":"/bin/key"}"#);
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!is_api_key_profile(d.path()));

    let d = profile_dir();
    let elsewhere = profile_dir();
    let target = elsewhere.path().join("s.json");
    private(&target, r#"{"apiKeyHelper":"/bin/key"}"#);
    std::os::unix::fs::symlink(&target, d.path().join("settings.json")).unwrap();
    assert!(!is_api_key_profile(d.path()));
}

#[test]
fn a_claude_json_over_4_mib_is_not_inspected() {
    let d = profile_dir();
    let mut body = String::from(r#"{"primaryApiKey":"sk","pad":""#);
    body.push_str(&"x".repeat(4 * 1024 * 1024));
    body.push_str("\"}");
    private(&d.path().join(".claude.json"), &body);
    assert!(!is_api_key_profile(d.path()));
}

#[test]
fn a_claude_json_between_64_kib_and_4_mib_is_inspected() {
    let d = profile_dir();
    let mut body = String::from(r#"{"primaryApiKey":"sk","pad":""#);
    body.push_str(&"x".repeat(512 * 1024));
    body.push_str("\"}");
    private(&d.path().join(".claude.json"), &body);
    assert!(is_api_key_profile(d.path()));
}

#[test]
fn malformed_json_is_not_an_api_key_profile() {
    let d = profile_dir();
    private(&d.path().join("settings.json"), "{apiKeyHelper");
    private(&d.path().join(".claude.json"), "primaryApiKey");
    assert!(!is_api_key_profile(d.path()));
}
