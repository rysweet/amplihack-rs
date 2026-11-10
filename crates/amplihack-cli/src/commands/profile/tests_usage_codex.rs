//! Issue #2001: the Codex block of `profile usage` (`usage_codex.rs`).
//!
//! Formatting only: rows are built from fixed `AccountResult`s, so nothing
//! here starts a process. The layout pinned below is the one in
//! docs/reference/profile-command.md, "Codex row layout", and the JSON is
//! "Codex JSON rows"; the golden file is `usage_json_codex_golden.json`.
//!
//! The seam these tests use:
//!
//! - `view(root, rows, timeout_secs, floor: Result<u64, String>) -> CodexView`,
//!   which also works out `next` at that floor (issue #2007:
//!   `rotator.codex_floor`); with `Err(reason)` no row is chosen and
//!   `next_withheld` holds the reason;
//! - `bars_block(&CodexView, &TimeView, colour) -> String` and
//!   `table_block(&CodexView, &TimeView) -> String`, each the block from its
//!   header line to its last row, every line ending in `\n`;
//! - `json_rows(&CodexView) -> Vec<CodexJsonRow>` (`Serialize`);
//! - `window_label(WindowName, Option<u64>) -> String`;
//! - `usage_cmd::render_json(&[Row])`, which has no Codex rows, and
//!   `usage_cmd::render_summary_json(at, &[Row], Option<&Summary>,
//!   &[CodexJsonRow], thresholds: &serde_json::Value)`.

use super::usage_codex::{CodexView, bars_block, json_rows, table_block, view, window_label};
use super::usage_view::TimeView;
use amplihack_utils::codex_capacity::{
    AccountResult, Credits, Outcome, Reading, Window, WindowName,
};
use chrono::FixedOffset;
use std::path::PathBuf;

/// 2026-11-06T19:09:00Z, the read time of the documented example.
const READ: i64 = 1_793_992_140;
const ROOT: &str = "/home/ryan/.codex-profiles";
const HOME_CODEX: &str = "/home/ryan/.codex";
const MIN: i64 = 60;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
/// The default `rotator.codex_floor`, `codex-pick`'s own default.
const FLOOR: u64 = 3000;

/// The `thresholds` object of `--summary-json` with nothing set.
fn default_thresholds() -> serde_json::Value {
    use super::rotate::settings::no_config;
    super::usage_thresholds::json(&super::usage_thresholds::resolve(
        None,
        &|_| None,
        &no_config,
    ))
}

fn utc0() -> TimeView {
    TimeView::Fixed(FixedOffset::east_opt(0).unwrap(), "UTC".to_string())
}

fn w(name: WindowName, pct: Option<f64>, minutes: Option<u64>, at: Option<i64>) -> Window {
    Window {
        name,
        used_percent: pct,
        window_minutes: minutes,
        resets_at: at,
    }
}

fn five(pct: f64, at: i64) -> Window {
    w(WindowName::Primary, Some(pct), Some(300), Some(at))
}

fn week(pct: f64, at: i64) -> Window {
    w(WindowName::Secondary, Some(pct), Some(10_080), Some(at))
}

fn reading(windows: Vec<Window>, credits: Option<Credits>, plan: Option<&str>) -> Reading {
    let mut r = Reading::default();
    r.windows = windows;
    r.credits = credits;
    r.plan_type = plan.map(String::from);
    r
}

fn at(label: &str, outcome: Outcome) -> AccountResult {
    let path = if label == "~/.codex" {
        PathBuf::from(HOME_CODEX)
    } else {
        PathBuf::from(ROOT).join(label)
    };
    AccountResult::new(label, path, outcome, READ)
}

fn signed_in(label: &str, r: Reading) -> AccountResult {
    at(label, Outcome::Ok(r))
}

const CRASH: &str =
    "codex app-server exited (exit status: 1): error: unrecognized subcommand 'app-server'";

/// The seven accounts of the documented example, in its order.
fn example_rows() -> Vec<AccountResult> {
    let mut ggrant = reading(vec![week(34.0, 1_794_321_000)], None, Some("pro"));
    ggrant.ordinary_usage_allowed = Some(true);
    let mut ffarmer = reading(
        vec![five(12.0, 1_794_001_200), week(41.0, 1_794_470_400)],
        None,
        Some("plus"),
    );
    ffarmer.ordinary_usage_allowed = Some(true);
    vec![
        signed_in("ffarmer", ffarmer),
        signed_in("ggrant", ggrant),
        signed_in(
            "hhale",
            reading(
                vec![],
                Some(Credits {
                    has_credits: true,
                    balance: Some(4200.0),
                }),
                Some("team"),
            ),
        ),
        at("jjones", Outcome::NotSignedIn),
        at("kkim", Outcome::Timeout),
        at("mmoss", Outcome::Error(CRASH.to_string())),
        signed_in(
            "~/.codex",
            reading(
                vec![five(88.0, 1_793_996_100), week(70.0, 1_794_193_200)],
                None,
                Some("plus"),
            ),
        ),
    ]
}

fn example() -> CodexView {
    view(PathBuf::from(ROOT), example_rows(), 20, Ok(FLOOR))
}

fn one(r: AccountResult) -> CodexView {
    view(PathBuf::from(ROOT), vec![r], 20, Ok(FLOOR))
}

/// The fields of a signed-in row, after its state (which itself contains
/// ` in `).
fn after_state(line: &str) -> &str {
    line.split_once("signed in").map_or(line, |(_, rest)| rest)
}

/// The line of `block` for `label`.
fn line_of<'a>(block: &'a str, label: &str) -> &'a str {
    block
        .lines()
        .find(|l| l.starts_with(label))
        .unwrap_or_else(|| panic!("no line for {label}:\n{block}"))
}

fn json(v: &CodexView) -> serde_json::Value {
    serde_json::to_value(json_rows(v)).expect("Codex rows serialise")
}

// ---------------------------------------------------------------------------
// Window labels (D14)
// ---------------------------------------------------------------------------

#[test]
fn window_lengths_are_labelled_by_days_hours_or_minutes() {
    let p = WindowName::Primary;
    for (minutes, want) in [
        (300, "5h"),
        (10_080, "7d"),
        (1440, "1d"),
        (2880, "2d"),
        (60, "1h"),
        (120, "2h"),
        (90, "90m"),
        (1, "1m"),
        (1500, "25h"),
    ] {
        assert_eq!(window_label(p, Some(minutes)), want, "{minutes}");
    }
}

#[test]
fn a_window_without_a_length_is_named_by_its_field() {
    assert_eq!(window_label(WindowName::Primary, None), "primary");
    assert_eq!(window_label(WindowName::Secondary, None), "secondary");
}

// ---------------------------------------------------------------------------
// The next choice
// ---------------------------------------------------------------------------

#[test]
fn the_view_marks_the_account_codex_pick_would_choose() {
    assert_eq!(example().next, Some(1), "ggrant, room 66");
}

#[test]
fn no_account_with_room_means_no_marker() {
    let v = view(
        PathBuf::from(ROOT),
        vec![
            signed_in("full", reading(vec![five(100.0, READ + HOUR)], None, None)),
            at("out", Outcome::NotSignedIn),
        ],
        20,
        Ok(FLOOR),
    );
    assert_eq!(v.next, None);
    let block = bars_block(&v, &utc0(), false);
    assert!(!block.contains("<- next"), "{block}");
    assert!(
        json(&v)
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["next_choice"] == false)
    );
}

// ---------------------------------------------------------------------------
// Bars (the default view)
// ---------------------------------------------------------------------------

#[test]
fn the_bar_block_is_the_documented_example() {
    let got = bars_block(&example(), &utc0(), false);
    let want = "\
Codex accounts under /home/ryan/.codex-profiles
ffarmer   signed in      5h █░░░░░░░░░  12% 11-06 21:40 in 2h 31m  7d ████░░░░░░  41% 11-12 08:00 in 132h 51m  plan plus  read 19:09:00
ggrant    signed in      7d ███░░░░░░░  34% 11-10 14:30 in 91h 21m  plan pro  read 19:09:00  <- next
hhale     signed in      credits 4200  plan team  read 19:09:00
jjones    not signed in  read 19:09:00
kkim      timed out      after 20s  read 19:09:00
mmoss     error: codex app-server exited (exit status: 1): error: unrecognized subcommand 'app-server'  read 19:09:00
~/.codex  signed in      5h ████████░░  88% 11-06 20:15 in 1h 6m  7d ███████░░░  70% 11-09 03:00 in 55h 51m  plan plus  read 19:09:00
";
    assert_eq!(got.trim_start_matches('\n'), want, "\n{got}");
}

#[test]
fn utc_puts_z_on_resets_and_the_read_time() {
    let block = bars_block(&example(), &TimeView::Utc, false);
    let l = line_of(&block, "ffarmer");
    assert!(
        l.contains("5h █░░░░░░░░░  12% 11-06 21:40Z in 2h 31m"),
        "{l}"
    );
    assert!(l.ends_with("read 19:09:00Z"), "{l}");
}

#[test]
fn times_follow_the_zone_of_the_view() {
    let plus1 = TimeView::Fixed(FixedOffset::east_opt(3600).unwrap(), "+01:00".to_string());
    let block = bars_block(&example(), &plus1, false);
    let l = line_of(&block, "ffarmer");
    assert!(l.contains("12% 11-06 22:40 in 2h 31m"), "{l}");
    assert!(l.ends_with("read 20:09:00"), "{l}");
}

#[test]
fn a_reset_at_or_before_the_read_is_due() {
    for reset in [READ, READ - 10 * MIN] {
        let v = one(signed_in("a", reading(vec![five(50.0, reset)], None, None)));
        let block = bars_block(&v, &utc0(), false);
        let l = line_of(&block, "a ");
        assert!(l.contains(" due"), "{l}");
        assert!(!after_state(l).contains(" in "), "{l}");
    }
}

#[test]
fn the_time_to_reset_is_rounded_down_to_the_minute() {
    let v = one(signed_in(
        "a",
        reading(vec![five(50.0, READ + 2 * HOUR + 59)], None, None),
    ));
    let l = bars_block(&v, &utc0(), false);
    assert!(line_of(&l, "a ").contains("in 2h 0m"), "{l}");
    let v = one(signed_in(
        "a",
        reading(vec![five(50.0, READ + 59)], None, None),
    ));
    let l = bars_block(&v, &utc0(), false);
    assert!(line_of(&l, "a ").contains("in 0h 0m"), "{l}");
}

#[test]
fn an_out_of_range_reset_is_shown_as_given_with_no_time_left() {
    for raw in [0, -5, READ + 401 * DAY] {
        let v = one(signed_in("a", reading(vec![five(50.0, raw)], None, None)));
        let block = bars_block(&v, &utc0(), false);
        let l = line_of(&block, "a ");
        assert!(l.contains(&format!("reset {raw}")), "{raw}: {l}");
        assert!(!after_state(l).contains(" in "), "{raw}: {l}");
        let row = &json(&v)[0];
        assert_eq!(row["windows"][0]["resets_at"], raw, "as received");
        assert!(row["windows"][0]["resets_at_rfc3339"].is_null(), "{row}");
    }
    let edge = READ + 400 * DAY;
    let v = one(signed_in("a", reading(vec![five(50.0, edge)], None, None)));
    let l = bars_block(&v, &utc0(), false);
    assert!(
        line_of(&l, "a ").contains(" in 9600h 0m"),
        "400 days is in range: {l}"
    );
}

#[test]
fn a_window_without_a_reset_or_a_percentage_shows_dashes() {
    let v = one(signed_in(
        "a",
        reading(
            vec![
                w(WindowName::Primary, Some(12.0), Some(300), None),
                w(WindowName::Secondary, None, None, Some(READ + HOUR)),
            ],
            None,
            None,
        ),
    ));
    let block = bars_block(&v, &utc0(), false);
    let l = line_of(&block, "a ");
    assert!(l.contains("5h █░░░░░░░░░  12% -"), "{l}");
    assert!(l.contains("secondary - 11-06 20:09 in 1h 0m"), "{l}");
}

#[test]
fn credits_show_the_balance_or_whether_there_are_any() {
    let cases = [
        (true, Some(4200.0), "credits 4200"),
        (true, Some(12.5), "credits 12.5"),
        (false, Some(0.0), "credits 0"),
        (true, None, "credits yes"),
        (false, None, "credits no"),
    ];
    for (has, balance, want) in cases {
        let v = one(signed_in(
            "a",
            reading(
                vec![],
                Some(Credits {
                    has_credits: has,
                    balance,
                }),
                None,
            ),
        ));
        let block = bars_block(&v, &utc0(), false);
        let l = line_of(&block, "a ");
        assert!(l.contains(&format!("  {want}  ")), "{want}: {l}");
        assert!(!l.contains("no windows reported"), "{l}");
    }
}

#[test]
fn a_signed_in_account_with_no_figures_says_so() {
    let v = one(signed_in("a", Reading::default()));
    let block = bars_block(&v, &utc0(), false);
    assert_eq!(
        line_of(&block, "a "),
        "a  signed in      no windows reported  read 19:09:00"
    );
}

#[test]
fn the_timeout_detail_is_the_deadline_that_applied() {
    let v = view(
        PathBuf::from(ROOT),
        vec![at("kkim", Outcome::Timeout)],
        7,
        Ok(FLOOR),
    );
    let block = bars_block(&v, &utc0(), false);
    assert_eq!(
        line_of(&block, "kkim"),
        "kkim  timed out      after 7s  read 19:09:00"
    );
    assert_eq!(json(&v)[0]["error"], "timed out after 7s");
}

#[test]
fn labels_pad_to_the_longest_and_cut_past_24_characters() {
    let long = "a".repeat(30);
    let v = view(
        PathBuf::from(ROOT),
        vec![
            at(&long, Outcome::NotSignedIn),
            at("b", Outcome::NotSignedIn),
        ],
        20,
        Ok(FLOOR),
    );
    let block = bars_block(&v, &utc0(), false);
    let cut = format!("{}…", "a".repeat(23));
    assert_eq!(
        line_of(&block, &cut),
        format!("{cut}  not signed in  read 19:09:00")
    );
    assert_eq!(
        line_of(&block, "b "),
        format!("b{}  not signed in  read 19:09:00", " ".repeat(23))
    );
    let row = &json(&v)[0];
    assert_eq!(row["account"], long, "JSON keeps the whole name");
}

#[test]
fn the_rows_keep_their_order_and_are_not_sorted_by_room() {
    let block = bars_block(&example(), &utc0(), false);
    let labels: Vec<&str> = block
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert_eq!(
        labels,
        [
            "ffarmer", "ggrant", "hhale", "jjones", "kkim", "mmoss", "~/.codex"
        ]
    );
}

#[test]
fn colour_follows_the_claude_bars() {
    let block = bars_block(&example(), &utc0(), true);
    // The Claude rule: green below 70, yellow below 90, red at 90 and above.
    let l = line_of(&block, "~/.codex");
    assert!(l.contains("\x1b[33m"), "88% and 70% are yellow: {l:?}");
    assert!(
        !l.contains("\x1b[31m"),
        "nothing here is 90% or more: {l:?}"
    );
    let l = line_of(&block, "ggrant");
    assert!(l.contains("\x1b[32m"), "34% is green: {l:?}");
    let plain = bars_block(&example(), &utc0(), false);
    assert!(!plain.contains('\x1b'), "{plain:?}");
}

#[test]
fn no_accounts_is_one_line_naming_the_root() {
    let v = view(PathBuf::from(ROOT), vec![], 20, Ok(FLOOR));
    for block in [bars_block(&v, &utc0(), false), table_block(&v, &utc0())] {
        assert_eq!(
            block.trim_start_matches('\n'),
            "Codex: no accounts under /home/ryan/.codex-profiles\n"
        );
    }
    assert_eq!(json(&v), serde_json::json!([]));
}

#[test]
fn labels_root_plan_and_reasons_are_cleaned_before_printing() {
    let mut r = reading(vec![], None, Some("pro"));
    r.plan_type = Some("pro\u{1b}[2J".to_string());
    let v = view(
        PathBuf::from("/tmp/ro\u{1b}]0;x\u{7}ot"),
        vec![
            signed_in("ev\u{1b}[31mil\u{202e}", r),
            at("bad", Outcome::Error("boom\u{1b}[0m\u{9b}31m".to_string())),
        ],
        20,
        Ok(FLOOR),
    );
    for block in [bars_block(&v, &utc0(), false), table_block(&v, &utc0())] {
        for c in ['\u{1b}', '\u{7}', '\u{202e}', '\u{9b}'] {
            assert!(!block.contains(c), "{c:?} in {block:?}");
        }
        assert!(block.contains("error: boom"), "{block}");
    }
}

// ---------------------------------------------------------------------------
// --table
// ---------------------------------------------------------------------------

#[test]
fn the_table_block_leaves_out_the_bar_and_keeps_one_decimal() {
    let mut rows = example_rows();
    if let Outcome::Ok(r) = &mut rows[0].outcome {
        r.windows[0].used_percent = Some(12.5);
    }
    let v = view(PathBuf::from(ROOT), rows, 20, Ok(FLOOR));
    let block = table_block(&v, &utc0());
    assert!(
        block
            .trim_start_matches('\n')
            .starts_with("Codex accounts under /home/ryan/.codex-profiles\n"),
        "{block}"
    );
    assert!(!block.contains('█') && !block.contains('░'), "{block}");
    assert_eq!(
        line_of(&block, "ffarmer"),
        "ffarmer   signed in      5h 12.5% 11-06 21:40 in 2h 31m  7d 41% 11-12 08:00 in 132h 51m  plan plus  read 19:09:00"
    );
    assert!(line_of(&block, "ggrant").ends_with("read 19:09:00  <- next"));
    assert_eq!(
        line_of(&block, "jjones"),
        "jjones    not signed in  read 19:09:00"
    );
}

#[test]
fn the_table_shows_a_dash_for_a_missing_percentage() {
    let v = one(signed_in(
        "a",
        reading(
            vec![w(WindowName::Primary, None, Some(300), Some(READ + HOUR))],
            None,
            None,
        ),
    ));
    let block = table_block(&v, &utc0());
    assert!(
        line_of(&block, "a ").contains("5h - 11-06 20:09 in 1h 0m"),
        "{block}"
    );
}

// ---------------------------------------------------------------------------
// JSON rows
// ---------------------------------------------------------------------------

fn keys(v: &serde_json::Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

#[test]
fn codex_json_matches_the_golden_file_byte_for_byte() {
    let want = include_str!("testdata/usage_json_codex_golden.json");
    let got = serde_json::to_string_pretty(&json_rows(&example())).unwrap();
    assert_eq!(got, want.strip_suffix('\n').unwrap_or(want));
}

#[test]
fn every_row_has_every_key_in_order() {
    let text = serde_json::to_string(&json_rows(&example())).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    for row in v.as_array().unwrap() {
        let mut k = keys(row);
        k.sort_unstable();
        assert_eq!(
            k,
            [
                "account",
                "credits",
                "error",
                "next_choice",
                "ordinary_usage_allowed",
                "path",
                "plan_type",
                "rate_limit_reached_type",
                "read_at",
                "state",
                "windows"
            ],
            "{row}"
        );
    }
    // The written order is the documented one.
    let order = [
        "\"account\"",
        "\"path\"",
        "\"state\"",
        "\"error\"",
        "\"windows\"",
        "\"credits\"",
        "\"plan_type\"",
        "\"rate_limit_reached_type\"",
        "\"ordinary_usage_allowed\"",
        "\"read_at\"",
        "\"next_choice\"",
    ];
    let first = &text[..text.find("},{\"account\"").unwrap()];
    let at: Vec<usize> = order.iter().map(|k| first.find(k).unwrap()).collect();
    assert!(at.windows(2).all(|p| p[0] < p[1]), "{first}");
    let wkeys = [
        "\"name\"",
        "\"used_percent\"",
        "\"window_minutes\"",
        "\"resets_at\"",
        "\"resets_at_rfc3339\"",
    ];
    let at: Vec<usize> = wkeys.iter().map(|k| first.find(k).unwrap()).collect();
    assert!(at.windows(2).all(|p| p[0] < p[1]), "{first}");
}

#[test]
fn json_states_errors_and_empty_figures() {
    let v = json(&example());
    let by = |name: &str| {
        v.as_array()
            .unwrap()
            .iter()
            .find(|r| r["account"] == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .clone()
    };
    let states: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["state"].as_str().unwrap())
        .collect();
    assert_eq!(
        states,
        ["ok", "ok", "ok", "not_signed_in", "timeout", "error", "ok"]
    );
    assert!(by("ffarmer")["error"].is_null());
    assert!(
        by("jjones")["error"].is_null(),
        "not_signed_in has no reason"
    );
    assert_eq!(by("kkim")["error"], "timed out after 20s");
    assert_eq!(by("mmoss")["error"], CRASH);
    for name in ["jjones", "kkim", "mmoss"] {
        let r = by(name);
        assert_eq!(r["windows"], serde_json::json!([]), "{name}");
        assert!(r["credits"].is_null(), "{name}");
        assert!(r["plan_type"].is_null(), "{name}");
        assert!(r["ordinary_usage_allowed"].is_null(), "{name}");
        assert_eq!(r["read_at"], "2026-11-06T19:09:00Z", "{name}");
    }
    assert_eq!(
        by("hhale")["credits"],
        serde_json::json!({"has_credits": true, "balance": 4200.0})
    );
    assert_eq!(by("~/.codex")["path"], HOME_CODEX);
    assert_eq!(by("ggrant")["path"], "/home/ryan/.codex-profiles/ggrant");
}

#[test]
fn json_windows_carry_the_reset_as_epoch_and_rfc3339() {
    let v = json(&example());
    let g = &v[1]["windows"][0];
    assert_eq!(g["name"], "secondary");
    assert_eq!(g["used_percent"], 34.0);
    assert!(g["used_percent"].is_f64(), "a float: {g}");
    assert_eq!(g["window_minutes"], 10_080);
    assert_eq!(g["resets_at"], 1_794_321_000);
    assert_eq!(g["resets_at_rfc3339"], "2026-11-10T14:30:00Z");
    assert_eq!(v[0]["windows"][0]["name"], "primary");
    assert_eq!(v[0]["windows"][0]["window_minutes"], 300);
}

#[test]
fn next_choice_is_true_on_exactly_the_marked_row() {
    let v = json(&example());
    let marked: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["next_choice"] == true)
        .map(|r| r["account"].as_str().unwrap())
        .collect();
    assert_eq!(marked, ["ggrant"]);
    for r in v.as_array().unwrap() {
        assert!(r["next_choice"].is_boolean(), "{r}");
    }
}

#[test]
fn json_names_and_paths_are_escaped_not_cleaned() {
    let v = view(
        PathBuf::from("/r"),
        vec![AccountResult::new(
            "q\"b\\s\u{1b}",
            PathBuf::from("/r/q\"b\\s\u{1b}"),
            Outcome::NotSignedIn,
            READ,
        )],
        20,
        Ok(FLOOR),
    );
    let text = serde_json::to_string(&json_rows(&v)).unwrap();
    assert!(!text.contains('\u{1b}'), "JSON escaping: {text:?}");
    let back: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(back[0]["account"], "q\"b\\s\u{1b}");
    assert_eq!(back[0]["path"], "/r/q\"b\\s\u{1b}");
}

// ---------------------------------------------------------------------------
// The whole `--json` array and `--summary-json` object
// ---------------------------------------------------------------------------

#[test]
fn json_output_stays_a_bare_array_that_old_jq_filters_read() {
    // Host scripts run `.[] | select(.profile == $p)` and `.[0]` on `--json`
    // with stderr sent to /dev/null. An object would make jq fail and the
    // scripts report no capacity, so `--json` stays an array of Claude rows.
    use super::usage_cmd::render_json;
    let v: serde_json::Value = serde_json::from_str(&render_json(&[])).unwrap();
    assert_eq!(v, serde_json::json!([]));
}

#[test]
fn summary_json_has_codex_after_summary_and_keeps_profiles() {
    use super::usage_cmd::render_summary_json;
    let codex = json_rows(&example());
    let text = render_summary_json(READ, &[], None, &codex, &default_thresholds());
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut k = keys(&v);
    k.sort_unstable();
    assert_eq!(k, ["at", "codex", "profiles", "summary", "thresholds"]);
    assert_eq!(v["profiles"], serde_json::json!([]));
    assert!(v["summary"].is_null());
    assert_eq!(v["codex"], serde_json::to_value(&codex).unwrap());
    let i = |k: &str| text.find(k).unwrap();
    assert!(
        i("\"at\"") < i("\"profiles\"")
            && i("\"profiles\"") < i("\"summary\"")
            && i("\"summary\"") < i("\"codex\"")
            && i("\"codex\"") < i("\"thresholds\""),
        "written in the documented order: {text}"
    );
}

// ---------------------------------------------------------------------------
// The floor comes from `rotator.codex_floor` (issue #2007)
// ---------------------------------------------------------------------------

/// One account with its rate limits used up and 4000 credits, and one
/// that is not signed in: only the credits can make the first the choice.
fn credits_only(balance: f64) -> Vec<AccountResult> {
    vec![
        signed_in(
            "creditor",
            reading(
                vec![five(100.0, READ + HOUR), week(100.0, READ + DAY)],
                Some(Credits {
                    has_credits: true,
                    balance: Some(balance),
                }),
                Some("plus"),
            ),
        ),
        at("out", Outcome::NotSignedIn),
    ]
}

#[test]
fn an_account_between_the_default_and_a_higher_floor_is_chosen_only_below_it() {
    let at_default = view(PathBuf::from(ROOT), credits_only(4000.0), 20, Ok(3000));
    assert_eq!(at_default.next, Some(0));
    assert_eq!(at_default.next_withheld, None);
    let raised = view(PathBuf::from(ROOT), credits_only(4000.0), 20, Ok(5000));
    assert_eq!(raised.next, None);
    assert_eq!(raised.next_withheld, None);
    let block = bars_block(&raised, &utc0(), false);
    assert!(!block.contains("<- next"), "{block}");
    assert!(!block.contains("not chosen"), "{block}");
}

#[test]
fn a_floor_of_zero_lets_any_positive_balance_be_chosen() {
    let v = view(PathBuf::from(ROOT), credits_only(1.0), 20, Ok(0));
    assert_eq!(v.next, Some(0));
    let v = view(PathBuf::from(ROOT), credits_only(1.0), 20, Ok(FLOOR));
    assert_eq!(v.next, None);
}

#[test]
fn an_unreadable_floor_chooses_nothing_and_says_why() {
    let why = "must be an integer from 0 to 1000000";
    let v = view(
        PathBuf::from(ROOT),
        example_rows(),
        20,
        Err(why.to_string()),
    );
    assert_eq!(
        v.next, None,
        "never chosen with a floor the operator did not set"
    );
    assert_eq!(v.next_withheld.as_deref(), Some(why));
    let want = "next: not chosen (rotator.codex_floor: must be an integer from 0 to 1000000)\n";
    for block in [bars_block(&v, &utc0(), false), table_block(&v, &utc0())] {
        assert!(!block.contains("<- next"), "{block}");
        assert!(block.ends_with(want), "the line follows the rows: {block}");
        assert_eq!(block.matches("not chosen").count(), 1, "{block}");
    }
    for row in json(&v).as_array().unwrap() {
        assert_eq!(row["next_choice"], false, "{row}");
    }
}

#[test]
fn the_not_chosen_line_is_cleaned() {
    let v = view(
        PathBuf::from(ROOT),
        example_rows(),
        20,
        Err("bad\u{1b}[2J\u{202e}value".to_string()),
    );
    for block in [bars_block(&v, &utc0(), false), table_block(&v, &utc0())] {
        let line = block
            .lines()
            .find(|l| l.starts_with("next: not chosen (rotator.codex_floor: "))
            .unwrap_or_else(|| panic!("{block}"));
        assert!(
            !line.contains('\u{1b}') && !line.contains('\u{202e}'),
            "{line:?}"
        );
        assert!(line.is_ascii(), "{line:?}");
    }
}

#[test]
fn with_no_accounts_a_bad_floor_adds_no_line() {
    // Nothing could have been chosen, so there is nothing to explain.
    let v = view(PathBuf::from(ROOT), vec![], 20, Err("x".to_string()));
    for block in [bars_block(&v, &utc0(), false), table_block(&v, &utc0())] {
        assert_eq!(
            block.trim_start_matches('\n'),
            "Codex: no accounts under /home/ryan/.codex-profiles\n"
        );
    }
}
