//! Issue #1710 (D11): the logins section, read from the stable JSON of
//! `amplihack profile usage --json`.
//!
//! Contract (`logins`):
//! - `parse(json: &[u8]) -> Result<Vec<LoginRow>, String>`: one row per
//!   element, in the order `profile usage` printed them; `Err` is the fixed
//!   text `UNREADABLE`, never the output or a path.
//! - An API-key profile is the one `profile usage` reports with `kind:
//!   "api-key"` (#1700); this section has no detector of its own.
//! - `overflow: true` has no plan bars; `live_in` set is `(live)`.
//! - `timeout`, `unparsed`, or no session and no weekly window, is `no
//!   reading (<state>)`.

use super::bars::{LoginRow, Reading};
use super::logins::{UNREADABLE, parse};

/// A row with every field `profile usage --json` prints (main at #1727).
fn row(profile: &str, kind: &str, state: &str, session: f64, week: f64) -> String {
    format!(
        r#"{{
  "profile": "{profile}",
  "account": "{profile}@example.com",
  "default": false,
  "leased_to": null,
  "live_in": null,
  "overflow": false,
  "overflow_blocked_until": null,
  "kind": "{kind}",
  "api_key_blocked": null,
  "state": "{state}",
  "session": {{"label": "session", "used_percentage": {session}, "resets_text": "4:50pm (Europe/London)", "resets_at": null}},
  "week_all": {{"label": "all models", "used_percentage": {week}, "resets_text": "Oct 30, 6pm (Europe/London)", "resets_at": 1793300000}},
  "week_models": [],
  "error": null
}}"#
    )
}

fn parse_rows(rows: &[String]) -> Vec<LoginRow> {
    parse(format!("[{}]", rows.join(",")).as_bytes()).unwrap()
}

fn names(rows: &[LoginRow]) -> Vec<&str> {
    rows.iter().map(|r| r.profile.as_str()).collect()
}

#[test]
fn rows_keep_the_order_profile_usage_printed() {
    let rows = parse_rows(&[
        row("work", "subscription", "ok", 10.0, 20.0),
        row("spare", "subscription", "ok", 30.0, 40.0),
    ]);
    assert_eq!(names(&rows), ["work", "spare"]);
}

#[test]
fn a_good_reading_has_both_percentages_the_state_and_the_resets_without_zone() {
    let rows = parse_rows(&[row("p", "subscription", "ok", 31.0, 58.0)]);
    assert_eq!(
        rows[0].reading,
        Reading::Bars {
            session: Some(31.0),
            week: Some(58.0),
            state: "ok".into(),
            session_resets: Some("4:50pm".into()),
            week_resets: Some("Oct 30, 6pm".into()),
        }
    );
    assert!(!rows[0].live);
}

#[test]
fn the_api_key_kind_from_profile_usage_is_the_only_api_key_sign() {
    let rows = parse_rows(&[
        row("platform", "api-key", "api_key", 0.0, 0.0),
        // A name that looks like a key plays no part.
        row("api-key", "subscription", "ok", 5.0, 5.0),
    ]);
    assert_eq!(rows[0].reading, Reading::ApiKey);
    assert!(matches!(rows[1].reading, Reading::Bars { .. }));
}

#[test]
fn an_overflow_profile_is_never_shown_with_bars() {
    let json = row("spare", "subscription", "ok", 1.0, 2.0)
        .replace(r#""overflow": false"#, r#""overflow": true"#);
    assert_eq!(parse_rows(&[json])[0].reading, Reading::Overflow);
}

#[test]
fn the_live_login_and_the_home_row_are_marked_live() {
    let live = row("work", "subscription", "ok", 1.0, 2.0)
        .replace(r#""live_in": null"#, r#""live_in": "~/.claude""#);
    let home = row("~/.claude", "subscription", "ok", 1.0, 2.0)
        .replace(r#""live_in": null"#, r#""live_in": "~/.claude""#);
    let rows = parse_rows(&[live, home]);
    assert!(rows[0].live && rows[1].live);
    assert_eq!(names(&rows), ["work", "~/.claude"]);
}

#[test]
fn timeout_unparsed_and_no_numbers_are_no_reading() {
    let none =
        r#"{"profile": "gone", "state": "not_logged_in", "session": null, "week_all": null}"#;
    let rows = parse_rows(&[
        row("slow", "subscription", "timeout", 1.0, 1.0),
        row("odd", "subscription", "unparsed", 1.0, 1.0),
        none.to_string(),
    ]);
    let states: Vec<_> = rows
        .iter()
        .map(|r| match &r.reading {
            Reading::NoReading { state } => state.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(states, ["timeout", "unparsed", "not_logged_in"]);
}

#[test]
fn an_empty_list_has_no_rows() {
    assert!(parse(b"[]").unwrap().is_empty());
}

#[test]
fn output_that_is_not_a_row_list_gives_fixed_text() {
    for bad in [&b""[..], b"warning: /home/secret/path", b"{\"profile\": 1}"] {
        assert_eq!(parse(bad).unwrap_err(), UNREADABLE);
    }
}
