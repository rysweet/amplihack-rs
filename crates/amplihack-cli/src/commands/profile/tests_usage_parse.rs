//! `profile usage` parser (issue #1578): recorded `/usage` outputs, states and
//! their precedence, and reset-time resolution. Platform-neutral: every time
//! is computed from a fixed `now` and an explicit IANA zone, never the host's.

use super::usage_parse::{
    NOT_LOGGED_IN, SPEND_LIMIT, UsageReport, UsageState, UsageWindow, parse_usage, resolve_reset,
};

/// 2026-10-16T12:00:00Z (05:00 in Los Angeles, PDT).
const NOW: i64 = 1_792_152_000;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
/// Oct 16 2026, 4:50pm PDT.
const LA_OCT16_450PM: i64 = 1_792_194_600;
/// Oct 22 2026, 2am PDT.
const LA_OCT22_2AM: i64 = 1_792_659_600;

const BANNER: &str =
    "You are currently using your subscription to power your Claude Code usage\n\n";
const TRAILER: &str = "\nWhat's contributing to your limits usage?\n...\n";

/// The sample from the issue, with the session and week percentages swapped in.
fn sample(session: &str, week: &str, extra: &str) -> String {
    format!(
        "{BANNER}Current session: {session}% used · resets Oct 16, 4:50pm (America/Los_Angeles)\n\
         Current week (all models): {week}% used · resets Oct 22, 2am (America/Los_Angeles)\n\
         {extra}{TRAILER}"
    )
}

fn parse(stdout: &str) -> UsageReport {
    parse_usage(stdout, "", NOW)
}

fn window(label: &str, pct: f64, text: &str, at: Option<i64>) -> UsageWindow {
    UsageWindow {
        label: label.to_string(),
        used_percentage: pct,
        resets_text: Some(text.to_string()),
        resets_at: at,
    }
}

// ---------------------------------------------------------------------------
// Recorded outputs
// ---------------------------------------------------------------------------

#[test]
fn issue_sample_is_session_exhausted_with_every_window_read() {
    let r = parse(&sample(
        "100",
        "68",
        "Current week (Fable): 0% used · resets Oct 22, 2am (America/Los_Angeles)\n",
    ));
    assert_eq!(r.state, UsageState::SessionExhausted);
    assert_eq!(
        r.session,
        Some(window(
            "session",
            100.0,
            "Oct 16, 4:50pm (America/Los_Angeles)",
            Some(LA_OCT16_450PM)
        ))
    );
    assert_eq!(
        r.week_all,
        Some(window(
            "all models",
            68.0,
            "Oct 22, 2am (America/Los_Angeles)",
            Some(LA_OCT22_2AM)
        ))
    );
    assert_eq!(
        r.week_models,
        vec![window(
            "Fable",
            0.0,
            "Oct 22, 2am (America/Los_Angeles)",
            Some(LA_OCT22_2AM)
        )]
    );
}

#[test]
fn normal_output_is_ok() {
    let r = parse(&sample("20", "30", ""));
    assert_eq!(r.state, UsageState::Ok);
    assert_eq!(r.session.as_ref().unwrap().used_percentage, 20.0);
    assert_eq!(r.week_all.as_ref().unwrap().used_percentage, 30.0);
    assert!(r.week_models.is_empty());
}

#[test]
fn weekly_at_100_is_weekly_exhausted() {
    let r = parse(&sample("40", "100", ""));
    assert_eq!(r.state, UsageState::WeeklyExhausted);
    assert_eq!(r.session.unwrap().used_percentage, 40.0);
}

#[test]
fn session_wins_over_weekly_when_both_are_exhausted() {
    assert_eq!(
        parse(&sample("100", "100", "")).state,
        UsageState::SessionExhausted
    );
}

#[test]
fn percentages_over_100_are_exhausted_and_kept_unclamped() {
    let r = parse(&sample("101", "5", ""));
    assert_eq!(r.state, UsageState::SessionExhausted);
    assert_eq!(
        r.session.unwrap().used_percentage,
        101.0,
        "JSON shows as read"
    );
}

#[test]
fn model_week_lines_are_kept_in_order_and_never_exhaust_the_profile() {
    let r = parse(&sample(
        "10",
        "20",
        "Current week (Fable): 0% used · resets Oct 22, 2am (America/Los_Angeles)\n\
         Current week (Opus): 100% used · resets Oct 22, 2am (America/Los_Angeles)\n",
    ));
    assert_eq!(r.state, UsageState::Ok, "a model limit does not block use");
    let labels: Vec<&str> = r.week_models.iter().map(|w| w.label.as_str()).collect();
    assert_eq!(labels, ["Fable", "Opus"]);
    assert_eq!(r.week_models[1].used_percentage, 100.0);
}

#[test]
fn only_one_of_session_or_all_models_is_enough_for_ok() {
    let session_only = "Current session: 7% used · resets Oct 16, 4:50pm (America/Los_Angeles)\n";
    let r = parse(session_only);
    assert_eq!(r.state, UsageState::Ok);
    assert!(r.week_all.is_none());

    let week_only =
        "Current week (all models): 7% used · resets Oct 22, 2am (America/Los_Angeles)\n";
    let r = parse(week_only);
    assert_eq!(r.state, UsageState::Ok);
    assert!(r.session.is_none());
}

#[test]
fn model_line_alone_is_unparsed_not_ok() {
    let r = parse("Current week (Opus): 3% used · resets Oct 22, 2am (America/Los_Angeles)\n");
    assert_eq!(r.state, UsageState::Unparsed);
    assert_eq!(r.week_models.len(), 1, "what was read is still reported");
}

#[test]
fn not_logged_in_output() {
    for (stdout, stderr) in [
        ("Not logged in · Please run /login\n", ""),
        ("", "Error: Invalid API key · Please run /login\n"),
        (
            "OAuth token has expired. Run /login to sign in again.\n",
            "",
        ),
        (
            "",
            "API Error: 401 {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\"}}",
        ),
    ] {
        let r = parse_usage(stdout, stderr, NOW);
        assert_eq!(r.state, UsageState::NotLoggedIn, "{stdout:?} / {stderr:?}");
        assert!(r.session.is_none() && r.week_all.is_none() && r.week_models.is_empty());
    }
}

#[test]
fn every_documented_not_logged_in_phrase_is_recognised_in_any_case() {
    for phrase in NOT_LOGGED_IN {
        for text in [phrase.to_string(), phrase.to_uppercase()] {
            let r = parse_usage(&format!("x {text} y\n"), "", NOW);
            assert_eq!(r.state, UsageState::NotLoggedIn, "{text:?}");
        }
    }
}

#[test]
fn not_logged_in_phrase_is_ignored_once_any_usage_line_was_read() {
    let with_session = format!("Not logged in\n{}", sample("10", "20", ""));
    assert_eq!(parse(&with_session).state, UsageState::Ok);

    let with_model_only =
        "Not logged in\nCurrent week (Opus): 3% used · resets Oct 22, 2am (America/Los_Angeles)\n";
    assert_eq!(
        parse(with_model_only).state,
        UsageState::Unparsed,
        "a model week line alone rules the phrases out"
    );
}

#[test]
fn spend_limit_output() {
    let r = parse_usage(
        "",
        "Your organization has reached its monthly spend limit.\n",
        NOW,
    );
    assert_eq!(r.state, UsageState::SpendLimit);
    assert!(r.session.is_none());

    for phrase in SPEND_LIMIT {
        let r = parse_usage(&format!("{}\n", phrase.to_uppercase()), "", NOW);
        assert_eq!(r.state, UsageState::SpendLimit, "{phrase:?}");
    }
}

#[test]
fn spend_limit_beats_every_parsed_state_and_keeps_the_percentages() {
    let out = format!(
        "{}Spend limit reached for your organization\n",
        sample("100", "100", "")
    );
    let r = parse(&out);
    assert_eq!(r.state, UsageState::SpendLimit);
    assert_eq!(r.session.unwrap().used_percentage, 100.0);
    assert_eq!(r.week_all.unwrap().used_percentage, 100.0);
}

/// Full `claude -p /usage` output captured on 2026-10-17, including the
/// "What's contributing" section. Nothing in a normal reading says
/// "spend limit", so the spend-limit precedence does not hide it.
const CAPTURED_2026_10_17: &str = "\
You are currently using your subscription to power your Claude Code usage

Current session: 29% used · resets Oct 17, 2:09pm (America/Los_Angeles)
Current week (all models): 6% used · resets Oct 24, 12:59am (America/Los_Angeles)
Current week (Fable): 0% used · resets Oct 24, 1am (America/Los_Angeles)

What's contributing to your limits usage?
Approximate, based on local sessions on this machine — does not include other devices or claude.ai. Behaviors are independent characteristics, not a breakdown.

Last 24h · 9972 requests · 321 sessions
  83% of your usage was at >150k context
  83% of your usage came from subagent-heavy sessions
  79% of your usage was while 4+ sessions ran in parallel
  Top skills: /qa-team 1%, /crusty-old-engineer 1%
  Top subagents: general-purpose 71%, amplihack:core:reviewer 3%, amplihack:core:builder 1%
  Top plugins: amplihack 5%

Last 7d · 87001 requests · 4193 sessions
  95% of your usage was while 4+ sessions ran in parallel
  81% of your usage came from subagent-heavy sessions
  Top plugins: amplihack 3%
";

#[test]
fn captured_full_output_is_ok_and_no_phrase_matches() {
    let r = parse(CAPTURED_2026_10_17);
    assert_eq!(r.state, UsageState::Ok);
    assert_eq!(r.session.unwrap().used_percentage, 29.0);
    assert_eq!(r.week_all.unwrap().used_percentage, 6.0);
    assert_eq!(r.week_models.len(), 1);
    let lower = CAPTURED_2026_10_17.to_lowercase();
    for phrase in SPEND_LIMIT.iter().chain(NOT_LOGGED_IN) {
        assert!(!lower.contains(phrase), "{phrase:?}");
    }
}

#[test]
fn spend_limit_beats_not_logged_in() {
    let r = parse_usage("Not logged in\n", "spending limit reached\n", NOW);
    assert_eq!(r.state, UsageState::SpendLimit);
}

#[test]
fn garbage_and_empty_output_is_unparsed() {
    for (stdout, stderr) in [
        ("", ""),
        ("\n\n   \n", ""),
        ("Unknown slash command: /usage\n", ""),
        ("Current session: lots used\n", "segfault\n"),
        ("Current sessions: 12% used · resets 4pm (UTC)\n", ""),
        ("current session: 12% used · resets 4pm (UTC)\n", ""),
        ("Current session: -5% used · resets 4pm (UTC)\n", ""),
        ("Current session: NaN% used · resets 4pm (UTC)\n", ""),
        ("Current session: 1e3% used · resets 4pm (UTC)\n", ""),
        ("\u{0}\u{1}\u{7f}\u{fffd}\u{1b}[31m", ""),
    ] {
        let r = parse_usage(stdout, stderr, NOW);
        assert_eq!(r.state, UsageState::Unparsed, "{stdout:?}");
        assert!(r.session.is_none() && r.week_all.is_none(), "{stdout:?}");
    }
}

#[test]
fn percentage_over_a_million_or_not_finite_skips_the_line() {
    let huge = format!(
        "Current session: 1{}% used · resets 4pm (UTC)\n",
        "0".repeat(400)
    );
    for line in [
        "Current session: 1000001% used · resets 4pm (UTC)\n".to_string(),
        huge,
    ] {
        let r = parse(&line);
        assert!(r.session.is_none(), "{line:.60}");
        assert_eq!(r.state, UsageState::Unparsed);
    }
    let r = parse("Current session: 1000000% used · resets 4pm (UTC)\n");
    assert_eq!(r.session.unwrap().used_percentage, 1_000_000.0);
}

// ---------------------------------------------------------------------------
// Line format tolerance
// ---------------------------------------------------------------------------

#[test]
fn separators_spacing_decimals_and_crlf_are_tolerated() {
    for line in [
        "Current session: 12.5% used · resets 4pm (UTC)",
        "Current session: 12.5% used • resets 4pm (UTC)",
        "Current session: 12.5% used - resets 4pm (UTC)",
        "Current session: 12.5% used resets 4pm (UTC)",
        "   Current session:   12.5%   used   ·   resets   4pm (UTC)   ",
        "Current session:12.5%used·resets 4pm (UTC)",
        "Current session: 12.5% used · resets 4pm (UTC)\r",
    ] {
        let r = parse(&format!("{line}\n"));
        let s = r.session.unwrap_or_else(|| panic!("not read: {line:?}"));
        assert_eq!(s.used_percentage, 12.5, "{line:?}");
        assert_eq!(
            s.resets_text.as_deref(),
            Some("4pm (UTC)"),
            "{line:?}: text is trimmed"
        );
        assert_eq!(s.resets_at, Some(NOW + 4 * HOUR), "{line:?}");
        assert_eq!(r.state, UsageState::Ok);
    }
}

#[test]
fn reset_text_is_kept_as_printed_even_when_it_cannot_be_resolved() {
    let r = parse("Current session: 3% used · resets in a little while\n");
    let s = r.session.unwrap();
    assert_eq!(s.resets_text.as_deref(), Some("in a little while"));
    assert_eq!(s.resets_at, None);
    assert_eq!(r.state, UsageState::Ok, "the percentage was read");
}

#[test]
fn a_line_without_a_reset_clause_is_read_with_no_reset() {
    // Claude 2.x prints this when no session is open.
    let out = "Current session: 0% used\n\
               Current week (all models): 84% used · resets 4pm (UTC)\n\
               Current week (Fable): 0% used\n";
    let r = parse(out);
    let s = r.session.expect("session line without a reset is read");
    assert_eq!(s.used_percentage, 0.0);
    assert_eq!(s.resets_text, None);
    assert_eq!(s.resets_at, None);
    assert_eq!(
        r.week_all.unwrap().resets_text.as_deref(),
        Some("4pm (UTC)")
    );
    assert_eq!(r.week_models.len(), 1);
    assert_eq!(r.week_models[0].resets_text, None);
    assert_eq!(r.state, UsageState::Ok);

    // Only reset-less lines still gives a usable reading, not `unparsed`.
    let r = parse("Current session: 12% used\n");
    assert_eq!(r.session.unwrap().used_percentage, 12.0);
    assert_eq!(r.state, UsageState::Ok);

    // Trailing text that is not a reset clause is still not a usage line.
    assert!(
        parse("Current session: 12% used by robots\n")
            .session
            .is_none()
    );
}

#[test]
fn parser_takes_no_exit_status_and_reads_stdout_only_for_lines() {
    // Usage lines on stderr are not usage lines.
    let r = parse_usage("", "Current session: 3% used · resets 4pm (UTC)\n", NOW);
    assert!(r.session.is_none());
    assert_eq!(r.state, UsageState::Unparsed);
}

// ---------------------------------------------------------------------------
// Reset-time resolution
// ---------------------------------------------------------------------------

#[test]
fn month_day_time_in_utc_and_los_angeles() {
    assert_eq!(
        resolve_reset("Oct 16, 4:50pm (America/Los_Angeles)", NOW),
        Some(LA_OCT16_450PM)
    );
    assert_eq!(
        resolve_reset("Oct 22, 2am (America/Los_Angeles)", NOW),
        Some(LA_OCT22_2AM)
    );
    assert_eq!(
        resolve_reset("Oct 17, 9am (UTC)", NOW),
        Some(NOW + 21 * HOUR)
    );
}

#[test]
fn am_pm_any_case_optional_minutes_and_loose_spacing() {
    let want = Some(NOW + DAY - 3 * HOUR); // Oct 17, 09:00Z
    for text in [
        "Oct 17, 9am (UTC)",
        "Oct 17, 9AM (UTC)",
        "Oct 17, 9:00am (UTC)",
        "Oct 17, 9 am (UTC)",
        "Oct 17,  9am  (UTC)",
        "Oct 17 , 9Am (UTC)",
    ] {
        assert_eq!(resolve_reset(text, NOW), want, "{text:?}");
    }
}

#[test]
fn midnight_and_noon() {
    assert_eq!(
        resolve_reset("Oct 17, 12am (UTC)", NOW),
        Some(1_792_195_200)
    );
    assert_eq!(resolve_reset("Oct 16, 12pm (UTC)", NOW), Some(NOW));
    assert_eq!(
        resolve_reset("Oct 16, 12:30pm (UTC)", NOW),
        Some(NOW + 1800)
    );
}

#[test]
fn explicit_year() {
    assert_eq!(
        resolve_reset("Jan 2, 2027, 9:30pm (UTC)", NOW),
        Some(1_798_925_400)
    );
}

#[test]
fn year_rolls_over_from_late_december() {
    let dec30 = 1_798_632_000; // 2026-12-30T12:00:00Z
    assert_eq!(
        resolve_reset("Jan 2, 9am (UTC)", dec30),
        Some(1_798_880_400),
        "2027-01-02T09:00Z"
    );
}

#[test]
fn a_reset_up_to_one_day_in_the_past_stays_this_year() {
    assert_eq!(resolve_reset("Oct 16, 11am (UTC)", NOW), Some(NOW - HOUR));
    assert_eq!(
        resolve_reset("Oct 15, 1pm (UTC)", NOW),
        Some(NOW - 23 * HOUR)
    );
    // 25 hours ago is outside the tolerance: the next one is a year away.
    assert_eq!(
        resolve_reset("Oct 15, 11am (UTC)", NOW),
        Some(1_823_598_000),
        "2027-10-15T11:00Z"
    );
}

#[test]
fn time_only_is_today_or_tomorrow_in_that_zone() {
    assert_eq!(resolve_reset("4pm (UTC)", NOW), Some(NOW + 4 * HOUR));
    assert_eq!(
        resolve_reset("9am (UTC)", NOW),
        Some(NOW + 21 * HOUR),
        "passed: tomorrow"
    );
    // 05:00 in Los Angeles: 4:50pm is today, 4am is tomorrow.
    assert_eq!(
        resolve_reset("4:50pm (America/Los_Angeles)", NOW),
        Some(LA_OCT16_450PM)
    );
    assert_eq!(
        resolve_reset("4am (America/Los_Angeles)", NOW),
        Some(NOW - HOUR + DAY)
    );
}

#[test]
fn time_only_reset_in_the_current_minute_is_today() {
    // Claude prints whole minutes: at 12:00:40 a reset due at 12:00:59
    // prints as 12pm, 40 seconds ago. That is not tomorrow.
    let noon = NOW; // 12:00 UTC
    assert_eq!(resolve_reset("12pm (UTC)", noon + 40), Some(noon));
    assert_eq!(resolve_reset("12pm (UTC)", noon + 600), Some(noon));
    assert_eq!(
        resolve_reset("12pm (UTC)", noon + 601),
        Some(noon + DAY),
        "more than ten minutes ago: tomorrow"
    );
    // At 00:04, a reset printed as 11:59pm was five minutes ago.
    let midnight = NOW + 12 * HOUR;
    assert_eq!(
        resolve_reset("11:59pm (UTC)", midnight + 4 * 60),
        Some(midnight - 60),
        "yesterday, not tonight"
    );
}

#[test]
fn repeated_hour_at_dst_end_resolves_to_the_earlier_time() {
    // 2026-11-01 01:30 happens twice in Los Angeles: PDT (08:30Z) then PST (09:30Z).
    assert_eq!(
        resolve_reset("Nov 1, 1:30am (America/Los_Angeles)", NOW),
        Some(1_793_521_800)
    );
}

#[test]
fn nonexistent_time_in_a_dst_gap_is_unresolved() {
    // 2027-03-14 02:30 does not exist in Los Angeles.
    assert_eq!(
        resolve_reset("Mar 14, 2027, 2:30am (America/Los_Angeles)", NOW),
        None
    );
    assert_eq!(
        resolve_reset("Mar 14, 2:30am (America/Los_Angeles)", NOW),
        None
    );
    assert_eq!(
        resolve_reset("Mar 14, 2027, 3:30am (America/Los_Angeles)", NOW),
        Some(1_805_020_200),
        "just after the gap is fine"
    );
}

#[test]
fn unknown_zone_missing_zone_and_other_forms_are_unresolved() {
    for text in [
        "Oct 22, 2am (Mars/Olympus)",
        "Oct 22, 2am",
        "Oct 22, 2am (Local)",
        "Oct 22, 2am (PDT)",
        "Feb 30, 1am (UTC)",
        "Oct 22, 13pm (UTC)",
        "Oct 22, 0am (UTC)",
        "Oct 22, 2:60am (UTC)",
        "Foo 22, 2am (UTC)",
        "in 3 hours",
        "2026-10-22T02:00:00Z",
        "",
        "(UTC)",
    ] {
        assert_eq!(resolve_reset(text, NOW), None, "{text:?}");
    }
}

#[test]
fn resolution_never_depends_on_the_host_zone() {
    // Same answer whatever TZ says; the test does not change the environment,
    // it only checks that a UTC reset is exact to the second.
    assert_eq!(
        resolve_reset("Oct 16, 4:50pm (UTC)", NOW),
        Some(1_792_169_400)
    );
}

// ---------------------------------------------------------------------------
// Totality (S4)
// ---------------------------------------------------------------------------

/// Small deterministic xorshift, so the fuzz cases are the same every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn assert_sane(r: &UsageReport) {
    for w in r
        .session
        .iter()
        .chain(r.week_all.iter())
        .chain(r.week_models.iter())
    {
        assert!(w.used_percentage.is_finite());
        assert!((0.0..=1_000_000.0).contains(&w.used_percentage));
    }
    assert!(!matches!(r.state, UsageState::Timeout | UsageState::Error));
}

#[test]
fn parser_never_panics_on_random_bytes() {
    let mut rng = Rng(0x1578_1578_dead_beef);
    let alphabet: Vec<char> = "Current session week (all models): 0123456789.% used·•- resets \
                               amp,Oct Jan (UTC/America_Los_Angeles)\n\r\t\u{0}\u{1b}é漢"
        .chars()
        .collect();
    for _ in 0..1000 {
        let len = (rng.next() % 300) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        let lossy = String::from_utf8_lossy(&bytes);
        assert_sane(&parse_usage(&lossy, &lossy, NOW));

        let text: String = (0..len)
            .map(|_| alphabet[(rng.next() as usize) % alphabet.len()])
            .collect();
        assert_sane(&parse_usage(&text, "", NOW));
        let _ = resolve_reset(&text, NOW);
    }
}

#[test]
fn parser_survives_long_lines_and_percent_floods() {
    let long = format!(
        "Current session: 5% used · resets {}\n",
        "x".repeat(200_000)
    );
    assert_sane(&parse(&long));
    assert_sane(&parse(&"%".repeat(100_000)));
    assert_sane(&parse(&"Current session: ".repeat(10_000)));
    assert_sane(&parse(&"9".repeat(100_000)));
    for now in [i64::MIN, -1, 0, i64::MAX] {
        assert_sane(&parse_usage(&sample("10", "20", ""), "", now));
        let _ = resolve_reset("Dec 31, 11:59pm (UTC)", now);
        let _ = resolve_reset("11:59pm (Pacific/Kiritimati)", now);
    }
}

/// Both wordings Claude uses for an exhausted or disabled credit balance
/// (issue #1606, D3), on either stream, are a spend limit.
#[test]
fn both_spend_limit_wordings_are_a_spend_limit() {
    for msg in [
        "You've hit your org's monthly spend limit · resets Nov 1, 12am (UTC)",
        "You\u{2019}ve hit your org\u{2019}s monthly spend limit",
        "Your organization has reached its monthly spend limit",
    ] {
        let out = parse_usage(&format!("{msg}\n"), "", NOW);
        assert_eq!(out.state, UsageState::SpendLimit, "stdout: {msg:?}");
        let err = parse_usage("", &format!("{msg}\n"), NOW);
        assert_eq!(err.state, UsageState::SpendLimit, "stderr: {msg:?}");
    }
}
