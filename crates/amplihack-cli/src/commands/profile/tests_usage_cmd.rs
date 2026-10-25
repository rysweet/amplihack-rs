//! `profile usage` command (issue #1578): row ordering, marks, the table and
//! JSON contracts, display sanitising, the timeout setting, the probe's
//! deadline, and what is handed to auto-switch. Probe tests that need `sh`
//! are Unix-only; everything else runs on every platform.

use super::auto_switch::{LeaseView, RecordOutcome};
use super::usage_cmd::{
    ErrorReason, Raw, Row, env_credential_var, error_warnings, format_pct, marks, observations,
    order, persist_messages, render_json, render_table, sanitize_display, swap_warning,
    timeout_secs,
};
use super::usage_parse::{UsageReport, UsageState, UsageWindow};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

const NOW: i64 = 1_792_152_000;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn win(label: &str, pct: f64, at: Option<i64>) -> UsageWindow {
    UsageWindow {
        label: label.to_string(),
        used_percentage: pct,
        resets_text: Some(format!("text-{label}-{pct}")),
        resets_at: at,
    }
}

fn report(
    state: UsageState,
    s: Option<(f64, Option<i64>)>,
    w: Option<(f64, Option<i64>)>,
) -> UsageReport {
    UsageReport {
        state,
        session: s.map(|(p, at)| win("session", p, at)),
        week_all: w.map(|(p, at)| win("all models", p, at)),
        week_models: Vec::new(),
    }
}

fn row(name: &str, rep: UsageReport) -> Row {
    Row {
        profile: name.to_string(),
        account: name.to_string(),
        default: false,
        leased_to: None,
        live_in: false,
        report: rep,
        error: None,
        raw: None,
    }
}

fn ok(name: &str, s: Option<(f64, Option<i64>)>, w: Option<(f64, Option<i64>)>) -> Row {
    row(name, report(UsageState::Ok, s, w))
}

fn names(rows: &[Row]) -> Vec<&str> {
    rows.iter().map(|r| r.profile.as_str()).collect()
}

fn view(hosts: &[(&str, &str)]) -> LeaseView {
    LeaseView {
        hosts: hosts
            .iter()
            .map(|(d, l)| (d.to_string(), l.to_string()))
            .collect::<BTreeMap<_, _>>(),
    }
}

// ---------------------------------------------------------------------------
// Ordering (D10)
// ---------------------------------------------------------------------------

#[test]
fn usable_rows_sort_by_the_higher_window_then_name() {
    let mut rows = vec![
        ok(
            "a",
            Some((20.0, Some(NOW + HOUR))),
            Some((30.0, Some(NOW + DAY))),
        ),
        ok("z", None, Some((10.0, Some(NOW + DAY)))),
        ok(
            "c",
            Some((5.0, Some(NOW + HOUR))),
            Some((10.0, Some(NOW + DAY))),
        ),
        // The session window has already reset: it counts as 0%.
        ok(
            "m",
            Some((90.0, Some(NOW - 10))),
            Some((5.0, Some(NOW + DAY))),
        ),
        // An unresolved reset is not a passed one.
        ok("n", Some((50.0, None)), None),
    ];
    order(&mut rows, NOW);
    assert_eq!(names(&rows), ["m", "c", "z", "a", "n"]);
}

#[test]
fn exhausted_rows_sort_by_back_at_with_unknown_last() {
    let ex = |name: &str, state, s, w| row(name, report(state, s, w));
    let mut rows = vec![
        ex(
            "w",
            UsageState::WeeklyExhausted,
            Some((10.0, Some(NOW + HOUR))),
            Some((100.0, Some(NOW + DAY))),
        ),
        ex(
            "u",
            UsageState::SessionExhausted,
            Some((100.0, None)),
            Some((10.0, Some(NOW + DAY))),
        ),
        // Back only when both blocking windows have reset: the later one.
        ex(
            "x",
            UsageState::SessionExhausted,
            Some((100.0, Some(NOW + 100))),
            Some((100.0, Some(NOW + 5000))),
        ),
        ex(
            "b",
            UsageState::SessionExhausted,
            Some((100.0, Some(NOW + HOUR))),
            Some((68.0, Some(NOW + DAY))),
        ),
        ex("s", UsageState::SpendLimit, None, None),
        ex(
            "y",
            UsageState::SessionExhausted,
            Some((100.0, Some(NOW + 3000))),
            None,
        ),
        // One blocking window without a reset makes back-at unknown.
        ex(
            "t",
            UsageState::SessionExhausted,
            Some((100.0, Some(NOW + 100))),
            Some((100.0, None)),
        ),
    ];
    order(&mut rows, NOW);
    assert_eq!(names(&rows), ["y", "b", "x", "w", "s", "t", "u"]);
}

#[test]
fn unavailable_rows_sort_by_state_then_name() {
    let mut rows = vec![
        row("aaa", report(UsageState::Error, None, None)),
        row("bbb", report(UsageState::Unparsed, None, None)),
        row("ccc", report(UsageState::Timeout, None, None)),
        row("zzz", report(UsageState::NotLoggedIn, None, None)),
        row("yyy", report(UsageState::NotLoggedIn, None, None)),
    ];
    order(&mut rows, NOW);
    assert_eq!(names(&rows), ["yyy", "zzz", "ccc", "bbb", "aaa"]);
}

#[test]
fn groups_are_usable_then_exhausted_then_unavailable() {
    let mut rows = vec![
        row("d", report(UsageState::NotLoggedIn, None, None)),
        row(
            "b",
            report(
                UsageState::SessionExhausted,
                Some((100.0, Some(NOW + HOUR))),
                None,
            ),
        ),
        ok("a", Some((99.0, Some(NOW + HOUR))), None),
        ok(
            "c",
            Some((5.0, Some(NOW + HOUR))),
            Some((10.0, Some(NOW + DAY))),
        ),
    ];
    order(&mut rows, NOW);
    assert_eq!(names(&rows), ["c", "a", "b", "d"]);
}

// ---------------------------------------------------------------------------
// Marks and attribution (D11)
// ---------------------------------------------------------------------------

#[test]
fn lease_view_gives_account_and_leased_to() {
    let v = view(&[("work", "personal")]);
    assert_eq!(v.account_in("work"), "personal");
    assert_eq!(v.account_in("personal"), "personal");
    assert_eq!(v.leased_to("personal"), Some("work"));
    assert_eq!(v.leased_to("work"), None);
    // A directory recorded as hosting itself is not a lease.
    assert_eq!(view(&[("a", "a")]).leased_to("a"), None);
}

#[test]
fn marks_render_default_lease_and_hosts() {
    let mut r = ok("work", Some((1.0, None)), None);
    assert_eq!(marks(&r), "-");
    r.default = true;
    assert_eq!(marks(&r), "*");
    r.account = "personal".into();
    assert_eq!(marks(&r), "* hosts:personal");
    let mut p = row("personal", report(UsageState::NotLoggedIn, None, None));
    p.leased_to = Some("work".into());
    assert_eq!(marks(&p), "leased→work");
    p.default = true;
    assert_eq!(marks(&p), "* leased→work");
}

// ---------------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------------

#[test]
fn percentages_print_whole_or_to_one_decimal() {
    assert_eq!(format_pct(20.0), "20%");
    assert_eq!(format_pct(0.0), "0%");
    assert_eq!(format_pct(100.0), "100%");
    assert_eq!(format_pct(12.5), "12.5%");
    assert_eq!(format_pct(33.33), "33.3%");
}

fn table_rows() -> Vec<Row> {
    let mut rep = report(
        UsageState::Ok,
        Some((20.0, Some(NOW + HOUR))),
        Some((30.5, Some(NOW + DAY))),
    );
    rep.session.as_mut().unwrap().resets_text = Some("Oct 16, 7pm (America/Los_Angeles)".into());
    rep.week_models.push(UsageWindow {
        label: "Fable".into(),
        used_percentage: 0.0,
        resets_text: Some("Oct 22, 2am (America/Los_Angeles)".into()),
        resets_at: None,
    });
    let mut work = row("work", rep);
    work.default = true;
    work.account = "personal".into();
    let mut personal = row("personal", report(UsageState::NotLoggedIn, None, None));
    personal.leased_to = Some("work".into());
    vec![work, personal]
}

#[test]
fn table_has_the_documented_columns_and_dashes_for_missing_values() {
    let out = render_table(&table_rows());
    let lines: Vec<&str> = out.lines().collect();
    let header: Vec<&str> = lines[0]
        .split("  ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(
        header,
        [
            "PROFILE",
            "MARKS",
            "STATE",
            "SESSION",
            "SESSION RESETS",
            "WEEK",
            "WEEK RESETS"
        ]
    );
    assert_eq!(lines.len(), 3, "{out}");
    let work: Vec<&str> = lines[1].split_whitespace().collect();
    assert_eq!(work[0], "work");
    assert!(lines[1].contains("* hosts:personal"), "{out}");
    assert!(
        lines[1].contains("20%") && lines[1].contains("30.5%"),
        "{out}"
    );
    assert!(
        lines[1].contains("Oct 16, 7pm (America/Los_Angeles)"),
        "{out}"
    );
    let personal: Vec<&str> = lines[2].split_whitespace().collect();
    assert_eq!(
        personal,
        [
            "personal",
            "leased→work",
            "not_logged_in",
            "-",
            "-",
            "-",
            "-"
        ]
    );
    assert!(!out.contains("Fable"), "model lines are JSON-only: {out}");
}

#[test]
fn table_columns_line_up_by_character() {
    let out = render_table(&table_rows());
    let lines: Vec<Vec<char>> = out.lines().map(|l| l.chars().collect()).collect();
    let col = |line: &[char], word: &str| {
        let s: String = line.iter().collect();
        s.find(word).map(|b| s[..b].chars().count())
    };
    let state_at = col(&lines[0], "STATE").unwrap();
    for (line, want) in [(&lines[1], "ok"), (&lines[2], "not_logged_in")] {
        let s: String = line[state_at..].iter().collect();
        assert!(
            s.starts_with(want),
            "STATE column misaligned: {:?}",
            String::from_iter(line.iter())
        );
    }
}

#[test]
fn table_strips_control_characters_and_cuts_long_text() {
    let mut r = ok("evil", Some((1.0, None)), Some((2.0, None)));
    r.report.session.as_mut().unwrap().resets_text =
        Some("4pm\u{1b}]0;pwned\u{7}\u{1b}[2J (UTC)\u{9b}".into());
    r.report.week_all.as_mut().unwrap().resets_text = Some("x".repeat(500));
    let out = render_table(&[r]);
    assert!(
        !out.chars().any(|c| c.is_control() && c != '\n'),
        "control character reached the terminal: {out:?}"
    );
    assert!(
        !out.contains(&"x".repeat(65)),
        "reset text must be cut to 64 characters"
    );
}

#[test]
fn raw_blocks_follow_their_row_only_when_present() {
    let mut rows = table_rows();
    assert!(!render_table(&rows).contains("raw stdout:"));
    rows[0].raw = Some(Raw {
        stdout: "line one\n\u{1b}[31mline two\n".into(),
        stderr: String::new(),
    });
    let out = render_table(&rows);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[1].starts_with("work"));
    assert_eq!(lines[2], "  raw stdout:");
    assert_eq!(lines[3], "    line one");
    assert!(
        lines[4].starts_with("    ") && lines[4].ends_with("line two"),
        "{out:?}"
    );
    assert_eq!(lines[5], "  raw stderr:");
    assert_eq!(lines[6], "    (empty)");
    assert!(lines[7].starts_with("personal"));
    assert!(!out.contains('\u{1b}'));
}

#[test]
fn sanitize_replaces_controls_and_keeps_newlines_only_when_asked() {
    let s = "a\u{1b}[0mb\u{7}c\u{0}d\u{7f}e\u{85}f\tg\rh\ni";
    for keep in [false, true] {
        let out = sanitize_display(s, keep);
        assert!(!out.chars().any(|c| c.is_control() && c != '\n'), "{out:?}");
        assert_eq!(out.contains('\n'), keep, "{out:?}");
        for c in ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i'] {
            assert!(out.contains(c), "printable text kept: {out:?}");
        }
    }
    let clean = "Oct 16, 4:50pm (America/Los_Angeles) leased→a é漢 · •";
    assert_eq!(sanitize_display(clean, false), clean);
}

#[test]
fn error_rows_print_one_fixed_warning_each() {
    let mut r = row("review", report(UsageState::Error, None, None));
    r.error = Some(ErrorReason::StartFailed);
    let fine = ok("work", Some((1.0, None)), None);
    assert_eq!(
        error_warnings(&[fine, r]),
        ["warning: profile \"review\": could not start claude"]
    );
}

#[test]
fn error_reasons_are_the_four_fixed_texts() {
    let all = [
        (ErrorReason::ProfileRefused, "profile directory refused"),
        (ErrorReason::StartFailed, "could not start claude"),
        (ErrorReason::ReadFailed, "could not read claude output"),
        (ErrorReason::WaitFailed, "could not wait for claude"),
    ];
    for (reason, text) in all {
        assert_eq!(reason.as_str(), text);
        assert!(
            !text.contains('/') && !text.contains(':'),
            "no paths or I/O detail"
        );
    }
}

// ---------------------------------------------------------------------------
// JSON (D14)
// ---------------------------------------------------------------------------

fn json(rows: &[Row]) -> serde_json::Value {
    let text = render_json(rows);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

fn keys(v: &serde_json::Value) -> Vec<&str> {
    let mut k: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    k.sort_unstable();
    k
}

#[test]
fn json_is_an_array_of_rows_with_the_exact_keys() {
    let v = json(&table_rows());
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    for r in arr {
        assert_eq!(
            keys(r),
            [
                "account",
                "default",
                "error",
                "leased_to",
                "live_in",
                "profile",
                "session",
                "state",
                "week_all",
                "week_models"
            ],
            "no `raw` key without --raw"
        );
    }
    let work = &arr[0];
    assert_eq!(work["profile"], "work");
    assert_eq!(work["account"], "personal");
    assert_eq!(work["default"], true);
    assert_eq!(work["leased_to"], serde_json::Value::Null);
    assert_eq!(work["state"], "ok");
    assert_eq!(work["error"], serde_json::Value::Null);
    assert_eq!(
        keys(&work["session"]),
        ["label", "resets_at", "resets_text", "used_percentage"]
    );
    assert_eq!(work["session"]["label"], "session");
    assert_eq!(work["session"]["used_percentage"], 20.0);
    assert_eq!(
        work["session"]["resets_text"],
        "Oct 16, 7pm (America/Los_Angeles)"
    );
    assert_eq!(work["session"]["resets_at"], NOW + HOUR);
    assert_eq!(work["week_all"]["label"], "all models");
    assert_eq!(work["week_models"][0]["label"], "Fable");
    assert_eq!(work["week_models"][0]["resets_at"], serde_json::Value::Null);

    let personal = &arr[1];
    assert_eq!(personal["leased_to"], "work");
    assert_eq!(personal["state"], "not_logged_in");
    assert_eq!(personal["session"], serde_json::Value::Null);
    assert_eq!(personal["week_all"], serde_json::Value::Null);
    assert_eq!(personal["week_models"], serde_json::json!([]));
}

#[test]
fn json_state_names_are_snake_case() {
    for (state, want) in [
        (UsageState::Ok, "ok"),
        (UsageState::SessionExhausted, "session_exhausted"),
        (UsageState::WeeklyExhausted, "weekly_exhausted"),
        (UsageState::SpendLimit, "spend_limit"),
        (UsageState::NotLoggedIn, "not_logged_in"),
        (UsageState::Timeout, "timeout"),
        (UsageState::Unparsed, "unparsed"),
        (UsageState::Error, "error"),
    ] {
        assert_eq!(
            json(&[row("p", report(state, None, None))])[0]["state"],
            want
        );
    }
}

#[test]
fn json_error_row_carries_the_reason() {
    let mut r = row("p", report(UsageState::Error, None, None));
    r.error = Some(ErrorReason::ProfileRefused);
    assert_eq!(json(&[r])[0]["error"], "profile directory refused");
}

#[test]
fn json_raw_only_when_kept_and_strings_are_escaped_not_cleaned() {
    let mut r = ok("p", Some((1.0, None)), None);
    r.report.session.as_mut().unwrap().resets_text = Some("a\u{1b}b".into());
    r.raw = Some(Raw {
        stdout: "out\u{1b}[31m".into(),
        stderr: "err".into(),
    });
    let text = render_json(&[r]);
    assert!(
        !text.contains('\u{1b}'),
        "JSON escaping, never a raw ESC: {text:?}"
    );
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(keys(&v[0]["raw"]), ["stderr", "stdout"]);
    assert_eq!(v[0]["raw"]["stdout"], "out\u{1b}[31m");
    assert_eq!(v[0]["session"]["resets_text"], "a\u{1b}b");
}

#[test]
fn json_for_no_rows_is_an_empty_array() {
    assert_eq!(render_json(&[]).trim(), "[]");
}

// ---------------------------------------------------------------------------
// Timeout setting (D12, S3) and environment credentials (S2)
// ---------------------------------------------------------------------------

#[test]
fn timeout_flag_wins_then_env_then_thirty() {
    assert_eq!(timeout_secs(None, None), (30, None));
    assert_eq!(timeout_secs(Some(2), Some(OsStr::new("45"))), (2, None));
    assert_eq!(timeout_secs(None, Some(OsStr::new("45"))), (45, None));
    assert_eq!(timeout_secs(None, Some(OsStr::new("600"))), (600, None));
    assert_eq!(
        timeout_secs(None, Some(OsStr::new(""))),
        (30, None),
        "empty means unset"
    );
}

#[test]
fn bad_timeout_env_warns_with_the_value_escaped_and_cut() {
    let (secs, warning) = timeout_secs(None, Some(OsStr::new("soon")));
    assert_eq!(secs, 30);
    assert_eq!(
        warning.as_deref(),
        Some(
            "warning: ignored AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS \"soon\" (use 1 to 600); using 30"
        )
    );
    for bad in ["0", "601", "-1", "1.5", "99999999999999999999999"] {
        let (secs, warning) = timeout_secs(None, Some(OsStr::new(bad)));
        assert_eq!(secs, 30, "{bad}");
        assert!(warning.is_some(), "{bad}");
    }
    let long = format!("\u{1b}[2J{}", "y".repeat(200));
    let (_, warning) = timeout_secs(None, Some(OsStr::new(&long)));
    let warning = warning.unwrap();
    assert!(!warning.contains('\u{1b}'), "{warning:?}");
    assert!(
        !warning.contains(&"y".repeat(33)),
        "value cut to 32 characters: {warning}"
    );
}

#[test]
fn env_credentials_are_found_by_name_only_when_non_empty() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    };
    assert_eq!(env_credential_var(env(&[])), None);
    assert_eq!(env_credential_var(env(&[("ANTHROPIC_API_KEY", "")])), None);
    assert_eq!(
        env_credential_var(env(&[("ANTHROPIC_API_KEY", "sk-x")])),
        Some("ANTHROPIC_API_KEY")
    );
    assert_eq!(
        env_credential_var(env(&[("ANTHROPIC_AUTH_TOKEN", "t")])),
        Some("ANTHROPIC_AUTH_TOKEN")
    );
    assert_eq!(
        env_credential_var(env(&[("CLAUDE_CODE_OAUTH_TOKEN", "t")])),
        Some("CLAUDE_CODE_OAUTH_TOKEN")
    );
}

// ---------------------------------------------------------------------------
// Hand-off to auto-switch
// ---------------------------------------------------------------------------

#[test]
fn observations_cover_only_recordable_states_and_use_the_live_account() {
    let mut hosted = ok("work", Some((20.0, Some(NOW + HOUR))), Some((30.0, None)));
    hosted.account = "personal".into();
    hosted
        .report
        .week_models
        .push(win("Opus", 100.0, Some(NOW + DAY)));
    let rows = vec![
        hosted,
        row(
            "b",
            report(
                UsageState::SessionExhausted,
                Some((100.0, Some(NOW + HOUR))),
                None,
            ),
        ),
        row(
            "w",
            report(
                UsageState::WeeklyExhausted,
                None,
                Some((100.0, Some(NOW + DAY))),
            ),
        ),
        row(
            "s",
            report(UsageState::SpendLimit, Some((10.0, Some(NOW + HOUR))), None),
        ),
        row("n", report(UsageState::NotLoggedIn, None, None)),
        row("t", report(UsageState::Timeout, None, None)),
        row("u", report(UsageState::Unparsed, None, None)),
        row("e", report(UsageState::Error, None, None)),
    ];
    let obs = observations(&rows);
    let got: Vec<(&str, &str)> = obs
        .iter()
        .map(|o| (o.profile.as_str(), o.account.as_str()))
        .collect();
    assert_eq!(got, [("work", "personal"), ("b", "b"), ("w", "w")]);
    let work = &obs[0];
    assert_eq!(work.session.unwrap().used_percentage, 20.0);
    assert_eq!(work.session.unwrap().resets_at, Some(NOW + HOUR));
    assert_eq!(work.week.unwrap().used_percentage, 30.0);
    assert_eq!(work.week.unwrap().resets_at, None);
    assert!(obs[1].week.is_none());
}

#[test]
fn persist_outcomes_print_the_documented_lines() {
    assert_eq!(
        persist_messages(&RecordOutcome::Busy, false),
        ["auto-switch: state is busy; usage record not updated"]
    );
    assert_eq!(
        persist_messages(&RecordOutcome::SwapInProgress, true),
        ["auto-switch: swap in progress; usage record not updated"]
    );
    assert_eq!(
        persist_messages(
            &RecordOutcome::Recorded {
                written: vec!["personal".into()],
                moved: vec!["work".into()],
            },
            false
        ),
        [
            "auto-switch: profile \"work\" changed accounts during the probe; its usage was not recorded"
        ]
    );
    assert!(
        persist_messages(&RecordOutcome::NotSetUp, true).is_empty(),
        "no note with --json"
    );
    let note = persist_messages(&RecordOutcome::NotSetUp, false);
    if cfg!(unix) {
        assert_eq!(
            note,
            ["note: auto-switch not set up; usage record not updated"]
        );
    } else {
        assert!(
            note.is_empty(),
            "nothing is noted where auto-switch does not exist"
        );
    }
}

// ---------------------------------------------------------------------------
// Probe (D12): deadline, caps, stdin, raw text. Uses `sh`, so Unix only.
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod probe {
    use super::super::usage_cmd::{ErrorReason, probe};
    use super::super::usage_parse::UsageState;
    use super::NOW;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn sh(script: &str) -> Command {
        // Other tests can change the process-wide PATH while running in
        // parallel; keep this shell fixture independent of those changes.
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(script).env("PATH", "/usr/bin:/bin");
        c
    }

    const LINE: &str = "printf 'Current session: 5%% used · resets 4pm (UTC)\\n'";

    #[test]
    fn reads_and_parses_a_quick_child() {
        let p = probe(sh(LINE), Duration::from_secs(10), NOW, false);
        assert_eq!(p.report.state, UsageState::Ok);
        assert_eq!(p.report.session.unwrap().used_percentage, 5.0);
        assert!(p.error.is_none());
        assert!(p.raw.is_none(), "raw text is dropped without --raw");
    }

    #[test]
    fn keeps_raw_text_when_asked() {
        let p = probe(
            sh(&format!("{LINE}; echo oops >&2; exit 3")),
            Duration::from_secs(10),
            NOW,
            true,
        );
        assert_eq!(
            p.report.state,
            UsageState::Ok,
            "exit status does not decide"
        );
        let raw = p.raw.unwrap();
        assert_eq!(raw.stdout, "Current session: 5% used · resets 4pm (UTC)\n");
        assert_eq!(raw.stderr, "oops\n");
    }

    #[test]
    fn stderr_phrases_count() {
        let p = probe(
            sh("echo 'Not logged in · Please run /login' >&2; exit 1"),
            Duration::from_secs(10),
            NOW,
            false,
        );
        assert_eq!(p.report.state, UsageState::NotLoggedIn);
    }

    #[test]
    fn stdin_is_null() {
        // With an inherited terminal or pipe, `cat` would wait for input.
        let p = probe(
            sh(&format!("cat >/dev/null; {LINE}")),
            Duration::from_secs(5),
            NOW,
            false,
        );
        assert_eq!(p.report.state, UsageState::Ok);
    }

    #[test]
    fn deadline_kills_the_child_and_reports_timeout() {
        let start = Instant::now();
        let p = probe(sh("sleep 30"), Duration::from_secs(1), NOW, false);
        assert_eq!(p.report.state, UsageState::Timeout);
        assert!(p.error.is_none());
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_grandchild_holding_the_pipes_does_not_extend_the_deadline() {
        // `sh` cannot exec the last command here, so `sleep` is a grandchild
        // that keeps stdout and stderr open after `sh` is killed.
        let start = Instant::now();
        let p = probe(
            sh("sleep 30; echo done"),
            Duration::from_secs(1),
            NOW,
            false,
        );
        assert_eq!(p.report.state, UsageState::Timeout);
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_grandchild_holding_the_pipes_after_a_normal_exit_is_not_waited_for() {
        let start = Instant::now();
        let _ = probe(
            sh(&format!("{LINE}; (sleep 30 &); exit 0")),
            Duration::from_secs(20),
            NOW,
            false,
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    /// True while a process with this pid exists (signal 0 checks only).
    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 performs only the permission and existence check.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn wait_gone(pid: i32) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn read_pid(path: &std::path::Path) -> i32 {
        let start = Instant::now();
        loop {
            if let Ok(s) = std::fs::read_to_string(path)
                && let Ok(pid) = s.trim().parse()
            {
                return pid;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "no pid written");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_timeout_kills_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        // The background `sleep` stands in for an MCP server `claude` started.
        let script = format!(
            "sleep 60 >/dev/null 2>&1 & echo $! > '{}'; sleep 60",
            pidfile.display()
        );
        let p = probe(sh(&script), Duration::from_secs(1), NOW, false);
        assert_eq!(p.report.state, UsageState::Timeout);
        let pid = read_pid(&pidfile);
        assert!(wait_gone(pid), "grandchild {pid} outlived the probe");
    }

    #[test]
    fn a_normal_exit_kills_what_the_child_left_running() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let script = format!(
            "sleep 60 >/dev/null 2>&1 & echo $! > '{}'; {LINE}",
            pidfile.display()
        );
        let p = probe(sh(&script), Duration::from_secs(10), NOW, false);
        assert_eq!(p.report.state, UsageState::Ok);
        let pid = read_pid(&pidfile);
        assert!(wait_gone(pid), "grandchild {pid} outlived the probe");
    }

    #[test]
    fn output_past_64_kib_is_dropped_without_stalling_the_child() {
        let start = Instant::now();
        let script = format!("head -c 200000 /dev/zero | tr '\\000' a; echo; {LINE}");
        let p = probe(sh(&script), Duration::from_secs(10), NOW, true);
        assert_eq!(
            p.report.state,
            UsageState::Unparsed,
            "the line after the cap is dropped"
        );
        assert!(p.raw.unwrap().stdout.len() <= 64 * 1024);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn spawn_failure_is_a_fixed_error() {
        let p = probe(
            Command::new("/nonexistent/amplihack-1578/claude"),
            Duration::from_secs(5),
            NOW,
            true,
        );
        assert_eq!(p.report.state, UsageState::Error);
        assert_eq!(p.error, Some(ErrorReason::StartFailed));
    }
}

// ---------------------------------------------------------------------------
// Source guards (privacy, no environment mutation, no shell)
// ---------------------------------------------------------------------------

#[test]
fn usage_sources_never_touch_credentials_the_environment_or_a_shell() {
    for (name, src) in [
        ("usage_cmd.rs", include_str!("usage_cmd.rs")),
        ("usage_parse.rs", include_str!("usage_parse.rs")),
    ] {
        for needle in [
            "set_var",
            "remove_var",
            "Command::new(\"sh\"",
            ".credentials.json",
            "CREDS",
            "dangerously-skip-permissions",
            "allowedTools",
        ] {
            assert!(!src.contains(needle), "{name} must not contain {needle:?}");
        }
    }
}

#[test]
fn which_live_warnings_apply_to_the_selected_profiles() {
    use super::name::ProfileName;
    use super::store::ProfileStore;
    use super::usage_cmd::live_warning_applies;
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(tmp.path().join("root"));
    let (a, b) = (
        ProfileName::parse("a").unwrap(),
        ProfileName::parse("b").unwrap(),
    );
    store.add(&a).unwrap();
    store.add(&b).unwrap();
    std::fs::write(store.root().join("a/.credentials.json"), "{}").unwrap();
    let ledger = super::default_live::LEDGER_WARNING;
    assert!(!live_warning_applies(
        &store,
        std::slice::from_ref(&a),
        ledger
    ));
    assert!(live_warning_applies(
        &store,
        std::slice::from_ref(&b),
        ledger
    ));
    assert!(live_warning_applies(&store, &[a.clone(), b], ledger));
    // The profile the own-login warning is about has a login, so it is
    // shown even when every selected profile has one.
    let own = super::default_live::OWN_LOGIN_WARNING;
    assert!(live_warning_applies(&store, std::slice::from_ref(&a), own));
}

#[test]
fn swap_warning_names_the_profile_in_fixed_text() {
    let name = super::name::ProfileName::parse("work").unwrap();
    assert_eq!(
        swap_warning(&name),
        "warning: the login in ~/.claude changed while profile \"work\" was probed; its usage was not recorded"
    );
}
