//! `profile usage` command (issue #1578): row ordering, marks, the table and
//! JSON contracts, display sanitising, the timeout setting, the probe's
//! deadline, and what is handed to auto-switch. Probe tests that need `sh`
//! are Unix-only; everything else runs on every platform.

use super::auto_switch::{LeaseView, RecordOutcome};
use super::usage_cmd::{
    ErrorReason, Raw, Row, env_credential_var, error_warnings, format_pct, marks, observations,
    order, persist_messages, render_json, render_table, sanitize_display, swap_warning,
};
use super::usage_parse::{UsageReport, UsageState, UsageWindow};
use std::collections::BTreeMap;
use std::ffi::OsString;

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
        session_limit: true,
    }
}

fn row(name: &str, rep: UsageReport) -> Row {
    Row {
        profile: name.to_string(),
        account: name.to_string(),
        default: false,
        leased_to: None,
        live_in: false,
        overflow: false,
        overflow_blocked_until: None,
        live_in_home: false,
        api_key: false,
        api_key_blocked: None,
        report: rep,
        error: None,
        raw: None,
        source: None,
        direct_failure: None,
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
                "api_key_blocked",
                "default",
                "error",
                "kind",
                "leased_to",
                "live_in",
                "overflow",
                "overflow_blocked_until",
                "profile",
                "session",
                "session_limit",
                "source",
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
// Environment credentials (S2)
// ---------------------------------------------------------------------------

// The deadline resolver moved to `probe_limits` (issue #1698); its tests
// are in `tests_probe_limits.rs`.

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
        // Absolute path and fixed PATH: other tests briefly point the
        // process-wide PATH at a temporary directory.
        let mut c = Command::new("/bin/sh");
        c.arg("-c")
            .arg(script)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin");
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
        ("usage_request.rs", include_str!("usage_request.rs")),
        ("probe_limits.rs", include_str!("probe_limits.rs")),
    ] {
        for needle in [
            "set_var",
            "remove_var",
            "Command::new(\"sh\"",
            ".credentials.json",
            ".claude.json",
            "CREDS",
            "oauthAccount",
            "accessToken",
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

// ---------------------------------------------------------------------------
// Overflow marks (issue #1606)
// ---------------------------------------------------------------------------

#[test]
fn overflow_rows_are_marked_with_any_block() {
    let mut r = ok("cred", Some((10.0, None)), Some((20.0, None)));
    r.overflow = true;
    assert_eq!(marks(&r), "overflow");
    r.overflow_blocked_until = Some(1_793_491_200);
    assert_eq!(marks(&r), "overflow, blocked until 2026-11-01T00:00:00Z");
    r.default = true;
    assert_eq!(marks(&r), "* overflow, blocked until 2026-11-01T00:00:00Z");
}

// "running on overflow" needs the live row to be marked and its own probe in
// this run to show a plan limit reached: an exhausted state, or the session
// or all-models weekly window at 100%. Either limit is enough; both are not
// needed.

fn live_cred(state: UsageState, s: f64, w: f64) -> Row {
    let mut r = row("cred", report(state, Some((s, None)), Some((w, None))));
    r.overflow = true;
    r.live_in = true;
    r
}

#[test]
fn the_table_says_when_the_live_login_runs_on_overflow() {
    let mut cred = live_cred(UsageState::SessionExhausted, 100.0, 40.0);
    cred.live_in = false;
    let mut work = ok("work", Some((10.0, None)), Some((20.0, None)));
    work.live_in = true;
    let out = render_table(&[work.clone(), cred.clone()]);
    assert!(!out.contains("running on overflow"), "{out}");
    work.live_in = false;
    cred.live_in = true;
    let out = render_table(&[work, cred]);
    assert!(out.ends_with("running on overflow: cred\n"), "{out}");
}

#[test]
fn a_marked_live_login_with_plan_capacity_is_not_on_overflow() {
    let out = render_table(&[live_cred(UsageState::Ok, 10.0, 20.0)]);
    assert!(!out.contains("running on overflow"), "{out}");
}

#[test]
fn either_plan_limit_on_the_live_marked_login_means_overflow() {
    for (state, s, w) in [
        (UsageState::SessionExhausted, 100.0, 10.0),
        (UsageState::WeeklyExhausted, 10.0, 100.0),
        (UsageState::Ok, 100.0, 10.0),
        (UsageState::Ok, 10.0, 100.0),
    ] {
        let out = render_table(&[live_cred(state, s, w)]);
        assert!(
            out.ends_with("running on overflow: cred\n"),
            "{state:?} {s}/{w}: {out}"
        );
    }
}

#[test]
fn a_full_per_model_weekly_window_alone_is_not_overflow() {
    // The rotator's capacity test reads the session and all-models weekly
    // windows only, so a full per-model window leaves plan capacity.
    let mut r = live_cred(UsageState::Ok, 10.0, 20.0);
    r.report.week_models.push(UsageWindow {
        label: "Opus".into(),
        used_percentage: 100.0,
        resets_text: None,
        resets_at: None,
    });
    let out = render_table(&[r]);
    assert!(!out.contains("running on overflow"), "{out}");
}

#[test]
fn a_failed_probe_on_the_live_marked_login_is_not_shown_as_overflow() {
    for state in [
        UsageState::Timeout,
        UsageState::Error,
        UsageState::SpendLimit,
    ] {
        let out = render_table(&[row("cred", report(state, None, None))].map(|mut r| {
            r.overflow = true;
            r.live_in = true;
            r
        }));
        assert!(!out.contains("running on overflow"), "{state:?}: {out}");
    }
}

#[test]
fn json_rows_carry_the_overflow_fields() {
    let mut cred = ok("cred", Some((10.0, None)), Some((20.0, None)));
    cred.overflow = true;
    cred.overflow_blocked_until = Some(1_793_491_200);
    let v = json(&[cred, ok("work", None, Some((5.0, None)))]);
    let arr = v.as_array().expect("still a top-level array");
    assert_eq!(arr[0]["overflow"], true);
    assert_eq!(arr[0]["overflow_blocked_until"], 1_793_491_200);
    assert_eq!(arr[1]["overflow"], false);
    assert!(arr[1]["overflow_blocked_until"].is_null());
}

// ---------------------------------------------------------------------------
// Issue #1693: the `~/.claude` row for an account no profile has
// ---------------------------------------------------------------------------

fn home_row() -> Row {
    let mut r = ok(
        "~/.claude",
        Some((10.0, Some(NOW + HOUR))),
        Some((95.0, Some(NOW + DAY))),
    );
    r.live_in = true;
    r.live_in_home = true;
    r
}

#[test]
fn the_home_row_is_shown_with_the_literal_label() {
    let v = json(&[home_row()]);
    assert_eq!(v[0]["profile"], "~/.claude");
    assert_eq!(v[0]["account"], "~/.claude");
    assert_eq!(v[0]["live_in"], "~/.claude");
    assert_eq!(v[0]["default"], false);
    assert!(v[0]["leased_to"].is_null());
}

#[test]
fn the_home_row_is_never_recorded() {
    let rows = vec![
        home_row(),
        ok("work", Some((20.0, None)), Some((30.0, None))),
    ];
    let obs = observations(&rows);
    let got: Vec<&str> = obs.iter().map(|o| o.profile.as_str()).collect();
    assert_eq!(got, ["work"]);
}

#[test]
fn no_profile_can_be_named_like_the_home_row() {
    assert!(super::name::ProfileName::parse("~/.claude").is_err());
}

// ---------------------------------------------------------------------------
// Issue #1698: probes under host load. A slow probe inside its deadline is
// read; one past it is `timeout` (never 100%); the pool never runs more than
// the bound it is given. Fake `claude` scripts only; never a real one.
// ---------------------------------------------------------------------------

/// Every profile probed by `mode` with `claude`, at most `parallel` at once,
/// through the pool `profile usage` uses; in the order of `names`.
#[cfg(unix)]
fn probe_each(
    store: &super::store::ProfileStore,
    bin: &crate::binary_finder::BinaryInfo,
    names: &[super::name::ProfileName],
    mode: super::probe_limits::ProbeMode,
    timeout: std::time::Duration,
    parallel: usize,
) -> Vec<super::usage_cmd::Probe> {
    super::usage_cmd::pooled(names.len(), parallel, |i| {
        super::usage_cmd::probe_profile(store, bin, &names[i], false, mode, timeout, NOW, false)
    })
}

#[cfg(unix)]
mod under_load {
    use super::super::name::ProfileName;
    use super::super::probe_limits::ProbeMode;
    use super::super::store::ProfileStore;
    use super::super::usage_cmd::{Row, observations, probe, render_json, render_table};
    use super::super::usage_parse::UsageState;
    use super::probe_each;
    use super::{NOW, row};
    use crate::binary_finder::BinaryInfo;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    const USAGE: &str = "printf 'Current session: 37%% used · resets 4pm (UTC)\\nCurrent week (all models): 61%% used · resets Oct 22, 2am (UTC)\\n'";

    fn sh(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.arg("-c")
            .arg(script)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN");
        c
    }

    #[test]
    fn a_slow_probe_inside_its_deadline_is_read() {
        let start = Instant::now();
        let p = probe(
            sh(&format!("sleep 2; {USAGE}")),
            Duration::from_secs(20),
            NOW,
            false,
        );
        assert!(start.elapsed() >= Duration::from_secs(2));
        assert_eq!(p.report.state, UsageState::Ok);
        assert!(p.error.is_none());
        assert_eq!(p.report.session.unwrap().used_percentage, 37.0);
        assert_eq!(p.report.week_all.unwrap().used_percentage, 61.0);
    }

    fn timed_out_row(name: &str) -> Row {
        // Prints a full screen after the deadline; it must never be read.
        let p = probe(
            sh(&format!("sleep 30; {USAGE}")),
            Duration::from_secs(1),
            NOW,
            false,
        );
        assert_eq!(p.report.state, UsageState::Timeout);
        assert!(p.error.is_none());
        let mut r = row(name, p.report);
        r.error = p.error;
        r
    }

    #[test]
    fn a_timed_out_probe_shows_timeout_and_dashes_in_the_table() {
        let out = render_table(&[timed_out_row("slow")]);
        let line = out
            .lines()
            .find(|l| l.starts_with("slow"))
            .unwrap_or_else(|| panic!("no row for slow: {out}"));
        let cols: Vec<&str> = line.split_whitespace().collect();
        assert!(cols.contains(&"timeout"), "{line}");
        assert!(cols.contains(&"-"), "{line}");
        assert!(!line.contains('%'), "unknown, never a percentage: {line}");
    }

    #[test]
    fn a_timed_out_probe_is_timeout_with_null_windows_in_json() {
        let text = render_json(&[timed_out_row("slow")]);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v[0]["profile"], "slow");
        assert_eq!(v[0]["state"], "timeout");
        assert!(v[0]["session"].is_null(), "{text}");
        assert!(v[0]["week_all"].is_null(), "{text}");
        assert!(v[0]["error"].is_null(), "{text}");
        assert!(!text.contains("100"), "never shown as 100%: {text}");
    }

    #[test]
    fn a_timed_out_probe_is_never_recorded() {
        assert!(observations(&[timed_out_row("slow")]).is_empty());
    }

    // --- The probe pool -----------------------------------------------------

    struct Pool {
        _dir: tempfile::TempDir,
        store: ProfileStore,
        bin: BinaryInfo,
        marks: PathBuf,
        names: Vec<ProfileName>,
    }

    /// `n` profiles and a fake `claude` that appends `S <profile>` on start
    /// and `E <profile>` on exit to a marks file, sleeping `secs` between.
    fn pool(n: usize, secs: f64) -> Pool {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path().join("root"));
        let names: Vec<ProfileName> = (0..n)
            .map(|i| ProfileName::parse(&format!("p{i}")).unwrap())
            .collect();
        for name in &names {
            store.add(name).unwrap();
        }
        let marks = dir.path().join("marks");
        let path = dir.path().join("claude");
        // Lines shorter than PIPE_BUF are appended atomically.
        let body = format!(
            "#!/bin/sh\n\
             PATH=/usr/local/bin:/usr/bin:/bin; export PATH\n\
             [ \"$1\" = -p ] || exit 0\n\
             who=${{CLAUDE_CONFIG_DIR##*/}}\n\
             echo \"S $who\" >> '{marks}'\n\
             sleep {secs}\n\
             echo \"E $who\" >> '{marks}'\n\
             {USAGE}\n",
            marks = marks.display()
        );
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        settle(&path);
        Pool {
            _dir: dir,
            store,
            bin: BinaryInfo {
                name: "claude".into(),
                path,
                version: None,
            },
            marks,
            names,
        }
    }

    /// Another test thread's fork can hold the fake's write descriptor until
    /// its exec, and an exec in that window fails with ETXTBSY. Run the fake
    /// with no arguments (it exits at once) until it starts.
    fn settle(path: &Path) {
        let start = Instant::now();
        while Command::new(path).status().is_err() {
            assert!(start.elapsed() < Duration::from_secs(5), "fake never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The most probes running at once, from the start and end marks.
    fn peak(marks: &Path) -> usize {
        let text = fs::read_to_string(marks).unwrap();
        let (mut running, mut peak) = (0usize, 0usize);
        for line in text.lines() {
            match line.split_whitespace().next() {
                Some("S") => {
                    running += 1;
                    peak = peak.max(running);
                }
                Some("E") => running = running.saturating_sub(1),
                _ => panic!("bad mark {line:?}"),
            }
        }
        assert_eq!(running, 0, "every probe ended: {text}");
        peak
    }

    fn run_pool(p: &Pool, parallel: usize) -> Vec<super::super::usage_cmd::Probe> {
        probe_each(
            &p.store,
            &p.bin,
            &p.names,
            ProbeMode::Text,
            Duration::from_secs(60),
            parallel,
        )
    }

    #[test]
    fn the_pool_never_runs_more_probes_than_its_bound() {
        let p = pool(6, 0.5);
        let probes = run_pool(&p, 2);
        assert_eq!(probes.len(), 6);
        for (i, probe) in probes.iter().enumerate() {
            assert_eq!(probe.report.state, UsageState::Ok, "p{i}: {probe:?}");
            assert_eq!(probe.report.session.as_ref().unwrap().used_percentage, 37.0);
        }
        let peak = peak(&p.marks);
        assert!(peak <= 2, "peak {peak} over the bound of 2");
        assert!(peak >= 2, "the bound of 2 was not used (peak {peak})");
    }

    #[test]
    fn a_bound_of_one_runs_the_probes_one_at_a_time() {
        let p = pool(3, 0.2);
        let probes = run_pool(&p, 1);
        assert!(probes.iter().all(|x| x.report.state == UsageState::Ok));
        assert_eq!(peak(&p.marks), 1);
    }

    #[test]
    fn a_bound_of_zero_still_probes_every_profile_one_at_a_time() {
        let p = pool(2, 0.1);
        let probes = run_pool(&p, 0);
        assert_eq!(probes.len(), 2);
        assert!(probes.iter().all(|x| x.report.state == UsageState::Ok));
        assert_eq!(peak(&p.marks), 1);
    }

    #[test]
    fn a_bound_above_the_number_of_profiles_runs_them_all() {
        let p = pool(3, 0.5);
        let probes = run_pool(&p, 16);
        assert!(probes.iter().all(|x| x.report.state == UsageState::Ok));
        assert!(peak(&p.marks) <= 3);
    }

    #[test]
    fn results_keep_the_order_of_the_profiles() {
        // Each fake prints its own name as the session percentage.
        let p = pool(4, 0.0);
        let body = "#!/bin/sh\n\
             PATH=/usr/local/bin:/usr/bin:/bin; export PATH\n\
             [ \"$1\" = -p ] || exit 0\n\
             who=${CLAUDE_CONFIG_DIR##*/}\n\
             n=${who#p}\n\
             sleep 0.$((3 - n))\n\
             printf 'Current session: %s%% used · resets 4pm (UTC)\\nCurrent week (all models): 1%% used · resets Oct 22, 2am (UTC)\\n' \"$n\"\n";
        let path = p.bin.path.with_file_name("claude-order");
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        settle(&path);
        let bin = BinaryInfo {
            name: "claude".into(),
            path,
            version: None,
        };
        let probes = probe_each(
            &p.store,
            &bin,
            &p.names,
            ProbeMode::Text,
            Duration::from_secs(60),
            2,
        );
        let got: Vec<f64> = probes
            .iter()
            .map(|x| x.report.session.as_ref().unwrap().used_percentage)
            .collect();
        assert_eq!(got, [0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_slow_profile_in_the_pool_times_out_alone() {
        let p = pool(3, 0.1);
        // p1 sleeps past the deadline; the others finish.
        let body = format!(
            "#!/bin/sh\n\
             PATH=/usr/local/bin:/usr/bin:/bin; export PATH\n\
             [ \"$1\" = -p ] || exit 0\n\
             who=${{CLAUDE_CONFIG_DIR##*/}}\n\
             [ \"$who\" = p1 ] && exec sleep 30\n\
             {USAGE}\n"
        );
        let path = p.bin.path.with_file_name("claude-slow");
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        settle(&path);
        let bin = BinaryInfo {
            name: "claude".into(),
            path,
            version: None,
        };
        let start = Instant::now();
        let probes = probe_each(
            &p.store,
            &bin,
            &p.names,
            ProbeMode::Text,
            Duration::from_secs(1),
            2,
        );
        assert!(start.elapsed() < Duration::from_secs(15));
        let states: Vec<UsageState> = probes.iter().map(|x| x.report.state).collect();
        assert_eq!(
            states,
            [UsageState::Ok, UsageState::Timeout, UsageState::Ok]
        );
        assert!(probes[1].report.session.is_none());
        assert!(probes[1].report.week_all.is_none());
    }
}

// ---------------------------------------------------------------------------
// Issue #1698: the `get_usage` request and its `/usage` fallback within one
// deadline. A script fake switches on `$2 = --input-format`; it records each
// start and what it read on stdin in its own temporary directory.
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod request {
    use super::super::ProfileError;
    use super::super::name::ProfileName;
    use super::super::probe_limits::ProbeMode;
    use super::super::rotate::decide::{Probed, Reading};
    use super::super::store::ProfileStore;
    use super::super::usage_cmd::{Probe, probe_with};
    use super::super::usage_parse::UsageState;
    use super::super::usage_request::{ARGS, REQUEST_ID, REQUEST_LINE};
    use super::NOW;
    use super::probe_each;
    use crate::binary_finder::BinaryInfo;
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    const USAGE: &str = "printf 'Current session: 37%% used · resets 4pm (UTC)\\nCurrent week (all models): 61%% used · resets Oct 22, 2am (UTC)\\n'";

    struct Fake {
        dir: tempfile::TempDir,
        path: PathBuf,
    }

    impl Fake {
        /// A fake that runs `request` for the usage request and `usage` for
        /// `-p /usage`, after recording its arguments and its stdin.
        fn new(request: &str, usage: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let d = dir.path().display().to_string();
            let path = dir.path().join("claude");
            let body = format!(
                "#!/bin/sh\n\
                 PATH=/usr/local/bin:/usr/bin:/bin; export PATH\n\
                 d='{d}'\n\
                 [ \"$1\" = -p ] || exit 0\n\
                 echo \"$*\" >> \"$d/starts\"\n\
                 if [ \"$2\" = --input-format ]; then\n\
                 {request}\n\
                 else\n\
                 cat > \"$d/stdin-usage\"\n\
                 {usage}\n\
                 fi\n"
            );
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            settle(&path);
            Self { dir, path }
        }

        /// The answer the request branch prints with `cat "$d/answer"`.
        fn answer(self, line: &str) -> Self {
            fs::write(self.dir.path().join("answer"), format!("{line}\n")).unwrap();
            self
        }

        fn command(&self, args: &[String]) -> Command {
            let mut c = Command::new(&self.path);
            c.args(args).env("PATH", "/usr/local/bin:/usr/bin:/bin");
            c
        }

        fn probe(&self, mode: ProbeMode, secs: u64, keep_raw: bool) -> Probe {
            let build = |a: &[String]| Ok(self.command(a));
            probe_with(build, mode, Duration::from_secs(secs), NOW, keep_raw).unwrap()
        }

        fn starts(&self) -> Vec<String> {
            fs::read_to_string(self.dir.path().join("starts"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        fn read(&self, file: &str) -> Option<String> {
            fs::read_to_string(self.dir.path().join(file)).ok()
        }
    }

    /// Another test thread's fork can hold the fake's write descriptor until
    /// its exec (ETXTBSY); run the fake with no arguments until it starts.
    fn settle(path: &Path) {
        let start = Instant::now();
        while Command::new(path).status().is_err() {
            assert!(start.elapsed() < Duration::from_secs(5), "fake never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// An accepted answer: session 20%, week 30%.
    fn accepted() -> String {
        serde_json::json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": REQUEST_ID,
                "response": {
                    "rate_limits_available": true,
                    "rate_limits": {
                        "five_hour": {"utilization": 20, "resets_at": "2026-10-16T17:00:00Z"},
                        "seven_day": {"utilization": 30, "resets_at": "2026-10-22T09:00:00Z"}
                    }
                }
            }
        })
        .to_string()
    }

    fn error_answer() -> String {
        serde_json::json!({
            "type": "control_response",
            "response": {"subtype": "error", "request_id": REQUEST_ID, "error": "unknown"}
        })
        .to_string()
    }

    fn session(p: &Probe) -> f64 {
        p.report.session.as_ref().unwrap().used_percentage
    }

    const ANSWER: &str = "cat > \"$d/stdin\"; cat \"$d/answer\"";

    #[test]
    fn an_accepted_answer_is_used_with_one_start() {
        let f = Fake::new(ANSWER, USAGE).answer(&accepted());
        let start = Instant::now();
        let p = f.probe(ProbeMode::Auto, 20, false);
        // stdin was closed after the line, so the fake did not wait for it.
        assert!(start.elapsed() < Duration::from_secs(10), "stdin left open");
        assert_eq!(p.report.state, UsageState::Ok);
        assert_eq!(session(&p), 20.0);
        assert_eq!(p.error, None);
        assert_eq!(p.raw, None);
        assert_eq!(f.starts(), [ARGS.join(" ")]);
        assert_eq!(f.read("stdin"), Some(format!("{REQUEST_LINE}\n")));
    }

    /// What Claude 2.1.316 printed for `-p /usage` under a key in `$HOME`.
    const COST_TEXT: &str =
        "printf 'Total cost:            $0.0000\\nTotal duration (API):  0s\\n'";

    /// The answer of Claude 2.1.316 under each `$HOME` setting, with the
    /// fields amplihack reads. See "API-key login check".
    fn home_key_answer(subscription: Option<&str>, available: bool, limits: bool) -> String {
        let rate_limits = limits.then(|| {
            serde_json::json!({
                "five_hour": {"utilization": 20, "resets_at": "2026-10-16T17:00:00Z"},
                "seven_day": {"utilization": 30, "resets_at": "2026-10-22T09:00:00Z"}
            })
        });
        serde_json::json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": REQUEST_ID,
                "response": {
                    "subscription_type": subscription,
                    "rate_limits_available": available,
                    "rate_limits": rate_limits
                }
            }
        })
        .to_string()
    }

    fn reading(p: &Probe) -> Reading {
        Probed::from_report(&p.report).reading
    }

    /// `env.ANTHROPIC_API_KEY` and `apiKeyHelper`: plan limits do not apply,
    /// so the fallback runs, prints cost text, and the live reading fails.
    #[test]
    fn a_home_key_in_settings_reads_as_failed_through_the_fallback() {
        let answer = home_key_answer(None, false, false);
        let f = Fake::new(ANSWER, COST_TEXT).answer(&answer);
        let p = f.probe(ProbeMode::Auto, 20, false);
        assert_eq!(reading(&p), Reading::Failed);
        assert_eq!(f.starts(), [ARGS.join(" "), "-p /usage".to_string()]);
    }

    /// `primaryApiKey` answers like a plain plan login, so figures in that
    /// answer give a normal reading and `/usage` never runs.
    #[test]
    fn a_home_primary_key_with_figures_reads_as_measured() {
        let answer = home_key_answer(Some("max"), true, true);
        let f = Fake::new(ANSWER, COST_TEXT).answer(&answer);
        let p = f.probe(ProbeMode::Auto, 20, false);
        assert!(matches!(reading(&p), Reading::Measured { .. }));
        assert_eq!(f.starts(), [ARGS.join(" ")]);
    }

    /// With no figures, as when the usage read failed, `primaryApiKey` ends
    /// on the same fallback, which fails.
    #[test]
    fn a_home_primary_key_without_figures_reads_as_failed_through_the_fallback() {
        let answer = home_key_answer(Some("max"), true, false);
        let f = Fake::new(ANSWER, COST_TEXT).answer(&answer);
        let p = f.probe(ProbeMode::Auto, 20, false);
        assert_eq!(reading(&p), Reading::Failed);
        assert_eq!(f.starts(), [ARGS.join(" "), "-p /usage".to_string()]);
    }

    #[test]
    fn an_error_answer_falls_back_to_usage() {
        let f = Fake::new(ANSWER, USAGE).answer(&error_answer());
        let p = f.probe(ProbeMode::Auto, 20, true);
        assert_eq!(p.report.state, UsageState::Ok);
        assert_eq!(session(&p), 37.0);
        assert_eq!(f.starts(), [ARGS.join(" "), "-p /usage".to_string()]);
        // `--raw` shows the attempt that gave the row.
        let raw = p.raw.unwrap();
        assert!(raw.stdout.contains("Current session"), "{raw:?}");
        assert!(!raw.stdout.contains("control_response"), "{raw:?}");
        // The fallback reads /dev/null.
        assert_eq!(f.read("stdin-usage").as_deref(), Some(""));
    }

    #[test]
    fn an_answer_then_a_hang_is_still_used() {
        let f = Fake::new(&format!("{ANSWER}; exec sleep 30"), USAGE).answer(&accepted());
        let start = Instant::now();
        let p = f.probe(ProbeMode::Auto, 3, false);
        assert!(start.elapsed() < Duration::from_secs(8));
        assert_eq!(p.report.state, UsageState::Ok);
        assert_eq!(session(&p), 20.0);
        assert_eq!(f.starts().len(), 1, "no /usage after the deadline");
    }

    #[test]
    fn a_silent_hang_times_out_and_never_starts_usage() {
        let f = Fake::new("exec sleep 30", USAGE);
        let start = Instant::now();
        let p = f.probe(ProbeMode::Auto, 1, true);
        assert!(start.elapsed() < Duration::from_secs(6));
        assert_eq!(p.report.state, UsageState::Timeout);
        assert_eq!(p.error, None);
        assert!(p.report.session.is_none() && p.report.week_all.is_none());
        assert_eq!(f.starts(), [ARGS.join(" ")]);
        assert!(p.raw.is_some(), "the request attempt's output");
    }

    #[test]
    fn the_fallback_gets_only_the_time_left() {
        // The request gives up after 3 s of a 4 s deadline; /usage then
        // hangs. With a fresh deadline the probe would take 7 s.
        let f = Fake::new("cat > \"$d/stdin\"; sleep 3; exit 1", "exec sleep 30");
        let start = Instant::now();
        let p = f.probe(ProbeMode::Auto, 4, false);
        let took = start.elapsed();
        assert_eq!(p.report.state, UsageState::Timeout);
        assert!(took < Duration::from_secs(6), "took {took:?}");
        assert!(f.starts().len() <= 2);
    }

    #[test]
    fn text_mode_runs_usage_alone_with_no_stdin() {
        let f = Fake::new(ANSWER, USAGE).answer(&accepted());
        let p = f.probe(ProbeMode::Text, 20, false);
        assert_eq!(p.report.state, UsageState::Ok);
        assert_eq!(session(&p), 37.0);
        assert_eq!(f.starts(), ["-p /usage"]);
        assert_eq!(f.read("stdin"), None);
        assert_eq!(f.read("stdin-usage").as_deref(), Some(""));
    }

    #[test]
    fn a_request_that_ignores_stdin_falls_back() {
        // Whether the write fails depends on timing; only the outcome counts.
        let f = Fake::new("exit 1", USAGE);
        let p = f.probe(ProbeMode::Auto, 20, false);
        assert_eq!(p.report.state, UsageState::Ok);
        assert_eq!(session(&p), 37.0);
    }

    #[test]
    fn a_refused_first_build_starts_nothing() {
        let f = Fake::new(ANSWER, USAGE).answer(&accepted());
        let calls = Cell::new(0);
        let build = |_: &[String]| -> Result<Command, ProfileError> {
            calls.set(calls.get() + 1);
            Err(ProfileError::NotFound("p".into()))
        };
        let r = probe_with(build, ProbeMode::Auto, Duration::from_secs(5), NOW, false);
        assert!(r.is_err());
        assert_eq!(calls.get(), 1);
        assert!(f.starts().is_empty());
    }

    #[test]
    fn the_directory_is_checked_again_before_usage() {
        let f = Fake::new("exit 1", USAGE);
        let calls = Cell::new(0);
        let build = |a: &[String]| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Ok(f.command(a))
            } else {
                Err(ProfileError::NotFound("p".into()))
            }
        };
        let r = probe_with(build, ProbeMode::Auto, Duration::from_secs(20), NOW, false);
        assert!(r.is_err());
        assert_eq!(calls.get(), 2);
        assert_eq!(f.starts(), [ARGS.join(" ")]);
    }

    #[test]
    fn an_overflow_profile_is_probed_with_usage_only() {
        let f = Fake::new(ANSWER, USAGE).answer(&accepted());
        let store = ProfileStore::new(f.dir.path().join("root"));
        let names: Vec<ProfileName> = ["cred", "plan"]
            .iter()
            .map(|n| ProfileName::parse(n).unwrap())
            .collect();
        for n in &names {
            store.add(n).unwrap();
        }
        let toggle = store.root().join(".auto-switch");
        fs::write(&toggle, "enabled=off\noverflow=cred\n").unwrap();
        fs::set_permissions(&toggle, fs::Permissions::from_mode(0o600)).unwrap();
        let bin = BinaryInfo {
            name: "claude".into(),
            path: f.path.clone(),
            version: None,
        };
        let probes = probe_each(
            &store,
            &bin,
            &names,
            ProbeMode::Auto,
            Duration::from_secs(20),
            1,
        );
        assert_eq!(session(&probes[0]), 37.0, "cred read with /usage");
        assert_eq!(session(&probes[1]), 20.0, "plan read with the request");
        assert_eq!(f.starts(), ["-p /usage".to_string(), ARGS.join(" ")]);
    }
}

// ---------------------------------------------------------------------------
// Issue #1818: no session limit
// ---------------------------------------------------------------------------

/// A plan with no session limit: no session window, the week read.
fn weekly_only(name: &str, state: UsageState, week: f64) -> Row {
    let mut rep = report(state, None, Some((week, Some(NOW + DAY))));
    rep.session_limit = false;
    row(name, rep)
}

fn cells(out: &str, name: &str) -> Vec<String> {
    let header = out.lines().next().unwrap();
    let line = out
        .lines()
        .find(|l| l.starts_with(name))
        .unwrap_or_else(|| panic!("no row for {name}: {out}"));
    // Cut at the header's column starts, so a cell with spaces stays whole.
    let starts: Vec<usize> = [
        "PROFILE",
        "MARKS",
        "STATE",
        "SESSION",
        "SESSION RESETS",
        "WEEK",
        "WEEK RESETS",
    ]
    .iter()
    .scan(0, |from, h| {
        let at = *from + header[*from..].find(h).unwrap();
        *from = at + h.len();
        Some(header[..at].chars().count())
    })
    .collect();
    let chars: Vec<char> = line.chars().collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            let b = starts.get(i + 1).copied().unwrap_or(chars.len());
            chars[a.min(chars.len())..b.min(chars.len())]
                .iter()
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect()
}

#[test]
fn the_table_says_no_session_limit_in_full_with_a_dash_for_the_reset() {
    for (state, word) in [
        (UsageState::Ok, "ok"),
        (UsageState::WeeklyExhausted, "weekly_exhausted"),
    ] {
        let week = if state == UsageState::Ok { 40.0 } else { 100.0 };
        let out = render_table(&[weekly_only("free", state, week)]);
        let c = cells(&out, "free");
        assert_eq!(c[2], word, "{out}");
        assert_eq!(c[3], "no session limit", "never clipped: {out}");
        assert_eq!(c[4], "-", "{out}");
        assert!(c[5].ends_with('%'), "{out}");
    }
}

#[test]
fn the_label_appears_only_on_rows_with_no_session_limit() {
    let normal = ok(
        "normal",
        Some((20.0, Some(NOW + HOUR))),
        Some((30.0, Some(NOW + DAY))),
    );
    // A failed reading keeps its cells even with no windows.
    let mut timeout = row("slow", report(UsageState::Timeout, None, None));
    timeout.report.session_limit = true;
    let mut key = weekly_only("key", UsageState::Ok, 10.0);
    key.api_key = true;
    let out = render_table(&[
        normal,
        timeout,
        row("gone", report(UsageState::NotLoggedIn, None, None)),
        row("bad", report(UsageState::Unparsed, None, None)),
        row("err", report(UsageState::Error, None, None)),
        row(
            "spent",
            report(UsageState::SpendLimit, None, Some((10.0, None))),
        ),
        key,
        weekly_only("free", UsageState::Ok, 10.0),
    ]);
    assert_eq!(
        out.matches("no session limit").count(),
        1,
        "only the weekly-only row: {out}"
    );
    assert_eq!(cells(&out, "normal")[3], "20%");
    for name in ["slow", "gone", "bad", "err", "spent"] {
        assert_eq!(cells(&out, name)[3], "-", "{name}: {out}");
    }
    assert!(
        cells(&out, "key")[3].starts_with("api-key"),
        "API-key rows keep their cell: {out}"
    );
}

#[test]
fn json_rows_carry_session_limit_right_after_session() {
    let rows = [
        ok(
            "normal",
            Some((20.0, Some(NOW + HOUR))),
            Some((30.0, Some(NOW + DAY))),
        ),
        weekly_only("free", UsageState::Ok, 40.0),
        row("slow", report(UsageState::Timeout, None, None)),
    ];
    let text = render_json(&rows);
    let v = json(&rows);
    assert_eq!(v[0]["session_limit"], true);
    assert_eq!(v[1]["session_limit"], false);
    assert_eq!(v[1]["session"], serde_json::Value::Null);
    assert_eq!(v[1]["state"], "ok");
    assert_eq!(v[1]["week_all"]["used_percentage"], 40.0);
    assert_eq!(v[2]["session_limit"], true, "a failed row is never false");
    assert_eq!(v[2]["state"], "timeout");
    // Field order: `session_limit` is written straight after `session`.
    for obj in text.split("\"profile\"").skip(1) {
        let s = obj.find("\"session\":").expect("session key");
        let l = obj.find("\"session_limit\":").expect("session_limit key");
        let w = obj.find("\"week_all\":").expect("week_all key");
        assert!(s < l && l < w, "{text}");
    }
}

#[test]
fn json_session_limit_is_a_bool_on_every_row() {
    let v = json(&table_rows());
    for r in v.as_array().unwrap() {
        assert!(r["session_limit"].is_boolean(), "{r}");
    }
}

/// A row with no session limit reaches `record_usage` as a week with no
/// session window, so a stored session window is kept until its reset
/// passes (crusty round 3 on PR #1829).
#[test]
fn observations_of_no_session_limit_rows_have_no_session_window() {
    let rows = vec![
        ok(
            "normal",
            Some((20.0, Some(NOW + HOUR))),
            Some((30.0, Some(NOW + DAY))),
        ),
        weekly_only("free", UsageState::Ok, 40.0),
        weekly_only("full", UsageState::WeeklyExhausted, 100.0),
    ];
    let obs = observations(&rows);
    let got: Vec<(&str, bool)> = obs
        .iter()
        .map(|o| (o.profile.as_str(), o.session.is_some()))
        .collect();
    assert_eq!(got, [("normal", true), ("free", false), ("full", false)]);
    assert_eq!(obs[1].week.unwrap().used_percentage, 40.0);
}

// ---------------------------------------------------------------------------
// Issue #1844: the reading source, the fallback mark and the weekly figure
//
// Contract:
// - `Row::source: Option<Source>` and `Row::direct_failure:
//   Option<&'static str>`.
// - The table shows `(fallback)` in the marks column of a fallback row,
//   after the other marks; passive and direct rows get no mark.
// - JSON has `source` (`passive`, `direct`, `fallback` or `null` for a row
//   never probed) right after `state`.
// - With `--raw`, a passive or direct row is followed by
//   `  (<source>: no claude output)` and its JSON `raw` is empty strings.
// - `fallback_notes(rows)` gives one stderr line per fallback row with a
//   direct failure: `note: profile "<name>": direct read gave <reason>;
//   using claude`. It is printed only with `--raw`.
// - `observations()` leaves out passive rows.
// - An exhausted session keeps the weekly figure as reported in both the
//   table and the JSON.
// ---------------------------------------------------------------------------

use super::usage_cmd::fallback_notes;
use super::usage_source::Source;

fn sourced(mut r: Row, source: Option<Source>) -> Row {
    r.source = source;
    r
}

fn exhausted_100_55(name: &str) -> Row {
    row(
        name,
        report(
            UsageState::SessionExhausted,
            Some((100.0, Some(NOW + HOUR))),
            Some((55.0, Some(NOW + DAY))),
        ),
    )
}

fn line_of<'a>(out: &'a str, name: &str) -> &'a str {
    out.lines()
        .find(|l| l.split_whitespace().next() == Some(name))
        .unwrap_or_else(|| panic!("no row for {name}: {out}"))
}

#[test]
fn a_fallback_row_is_marked_and_others_are_not() {
    let rows = vec![
        sourced(
            ok("aa", Some((1.0, None)), Some((2.0, None))),
            Some(Source::Passive),
        ),
        sourced(
            ok("bb", Some((1.0, None)), Some((2.0, None))),
            Some(Source::Direct),
        ),
        sourced(
            ok("cc", Some((1.0, None)), Some((2.0, None))),
            Some(Source::Fallback),
        ),
    ];
    assert_eq!(marks(&rows[0]), "-");
    assert_eq!(marks(&rows[1]), "-");
    assert_eq!(marks(&rows[2]), "(fallback)");
    let mut def = rows[2].clone();
    def.default = true;
    assert_eq!(marks(&def), "* (fallback)");
    let out = render_table(&rows);
    assert!(line_of(&out, "cc").contains("(fallback)"), "{out}");
    assert!(!line_of(&out, "aa").contains("fallback"), "{out}");
    assert!(!line_of(&out, "bb").contains("fallback"), "{out}");
}

#[test]
fn json_gives_the_source_or_null() {
    let rows = vec![
        sourced(
            ok("aa", Some((1.0, None)), Some((2.0, None))),
            Some(Source::Passive),
        ),
        sourced(
            ok("bb", Some((1.0, None)), Some((2.0, None))),
            Some(Source::Direct),
        ),
        sourced(
            ok("cc", Some((1.0, None)), Some((2.0, None))),
            Some(Source::Fallback),
        ),
        ok("dd", None, None),
    ];
    let v = json(&rows);
    let got: Vec<&serde_json::Value> = v.as_array().unwrap().iter().map(|r| &r["source"]).collect();
    assert_eq!(
        got,
        [
            &serde_json::json!("passive"),
            &serde_json::json!("direct"),
            &serde_json::json!("fallback"),
            &serde_json::Value::Null,
        ]
    );
    let text = render_json(&rows);
    let first = text.find("\"state\"").unwrap();
    let source = text.find("\"source\"").unwrap();
    let after = text[first..].find('\n').map(|i| first + i).unwrap();
    assert!(
        source > first && source <= after + 20,
        "source follows state: {text}"
    );
}

#[test]
fn an_exhausted_session_keeps_the_weekly_figure_in_table_and_json() {
    let rows = vec![sourced(exhausted_100_55("team"), Some(Source::Direct))];
    let out = render_table(&rows);
    let l = line_of(&out, "team");
    assert!(l.contains("100%") && l.contains("55%"), "{l}");
    let v = json(&rows);
    assert_eq!(v[0]["state"], "session_exhausted");
    assert_eq!(v[0]["session"]["used_percentage"], 100.0);
    assert_eq!(v[0]["week_all"]["used_percentage"], 55.0);
}

#[test]
fn raw_shows_a_fixed_line_for_rows_read_without_claude() {
    let empty = Raw {
        stdout: String::new(),
        stderr: String::new(),
    };
    let mut p = sourced(
        ok("aa", Some((1.0, None)), Some((2.0, None))),
        Some(Source::Passive),
    );
    p.raw = Some(empty.clone());
    let mut d = sourced(
        ok("bb", Some((1.0, None)), Some((2.0, None))),
        Some(Source::Direct),
    );
    d.raw = Some(empty);
    let out = render_table(&[p.clone(), d.clone()]);
    assert!(out.contains("\n  (passive: no claude output)\n"), "{out}");
    assert!(out.contains("\n  (direct: no claude output)\n"), "{out}");
    assert!(!out.contains("raw stdout"), "{out}");
    let v = json(&[p, d]);
    assert_eq!(v[0]["raw"], serde_json::json!({"stdout": "", "stderr": ""}));
}

#[test]
fn fallback_notes_name_the_direct_failure() {
    let mut a = sourced(
        ok("aa", Some((1.0, None)), Some((2.0, None))),
        Some(Source::Fallback),
    );
    a.direct_failure = Some("http 5xx");
    let b = sourced(
        ok("bb", Some((1.0, None)), Some((2.0, None))),
        Some(Source::Fallback),
    );
    let c = sourced(
        ok("cc", Some((1.0, None)), Some((2.0, None))),
        Some(Source::Direct),
    );
    assert_eq!(
        fallback_notes(&[a, b, c]),
        ["note: profile \"aa\": direct read gave http 5xx; using claude"]
    );
}

#[test]
fn passive_rows_are_not_recorded_again() {
    let rows = vec![
        sourced(
            ok("aa", Some((1.0, None)), Some((2.0, None))),
            Some(Source::Passive),
        ),
        sourced(
            ok("bb", Some((1.0, None)), Some((2.0, None))),
            Some(Source::Direct),
        ),
        sourced(
            ok("cc", Some((1.0, None)), Some((2.0, None))),
            Some(Source::Fallback),
        ),
    ];
    let names: Vec<String> = observations(&rows).into_iter().map(|o| o.profile).collect();
    assert_eq!(names, ["bb", "cc"]);
}

#[test]
fn a_direct_error_row_shows_its_reason() {
    let mut r = sourced(
        row("aa", report(UsageState::Error, None, None)),
        Some(Source::Direct),
    );
    r.error = Some(ErrorReason::Direct("http 5xx"));
    assert_eq!(
        error_warnings(&[r.clone()]),
        ["warning: profile \"aa\": direct read: http 5xx"]
    );
    assert_eq!(json(&[r])[0]["error"], "direct read: http 5xx");
}

#[test]
fn a_missing_claude_is_an_error_row_with_its_reason() {
    let mut r = sourced(
        row("aa", report(UsageState::Error, None, None)),
        Some(Source::Fallback),
    );
    r.error = Some(ErrorReason::NoClaude);
    assert_eq!(json(&[r])[0]["error"], "claude not found");
}
