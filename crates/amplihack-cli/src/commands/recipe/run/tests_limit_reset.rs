//! `limit_reset::parse` (issue #1618): the text grammar after `resets `, with
//! and without a date, the today-or-tomorrow (or this-year-or-next) rule with
//! its 10-minute grace, and daylight-saving changes. `parse` never clamps;
//! the wait's plan refuses a reset more than five hours away.
//!
//! Wired from `recipe/run/mod.rs` with `#[cfg(test)] mod tests_limit_reset;`.

use super::limit_reset::{parse, parse_in};
use chrono::{TimeZone, Timelike};
use chrono_tz::Tz;

const LA: Tz = chrono_tz::America::Los_Angeles;
/// 2026-10-18 08:00:00 -07:00 (15:00 UTC).
const NOW: i64 = 1_792_335_600;
/// 2026-10-18 08:30:00 -07:00.
const LA_0830: i64 = 1_792_337_400;
const DAY: i64 = 86_400;

fn at(tz: Tz, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .expect("unambiguous local time")
        .timestamp()
}

#[test]
fn twelve_hour_time_with_zone_is_placed_today_when_in_the_future() {
    assert_eq!(parse("8:30am (America/Los_Angeles)", NOW), Some(LA_0830));
}

#[test]
fn every_documented_accepted_form_reads_its_instant() {
    let cases: [(&str, i64); 6] = [
        ("8:30am (America/Los_Angeles)", LA_0830),
        ("08:30 (America/Los_Angeles)", LA_0830),
        (
            "9pm (Europe/London)",
            at(chrono_tz::Europe::London, 2026, 10, 18, 21, 0),
        ),
        // 00:00 and 12:00 UTC today are 15 h and 3 h past: tomorrow.
        ("12am (UTC)", at(chrono_tz::UTC, 2026, 10, 19, 0, 0)),
        ("12pm (UTC)", at(chrono_tz::UTC, 2026, 10, 19, 12, 0)),
        // Tokyo is already on 2026-10-19 (00:00 JST); 17:00 is later that day.
        (
            "17:00 (Asia/Tokyo)",
            at(chrono_tz::Asia::Tokyo, 2026, 10, 19, 17, 0),
        ),
    ];
    for (fragment, want) in cases {
        assert_eq!(parse(fragment, NOW), Some(want), "fragment {fragment:?}");
    }
}

#[test]
fn twelve_am_is_midnight_and_twelve_pm_is_noon() {
    let midnight = parse("12am (UTC)", NOW).expect("12am parses");
    let noon = parse("12pm (UTC)", NOW).expect("12pm parses");
    let local = |t: i64| chrono_tz::UTC.timestamp_opt(t, 0).single().unwrap();
    assert_eq!((local(midnight).hour(), local(midnight).minute()), (0, 0));
    assert_eq!((local(noon).hour(), local(noon).minute()), (12, 0));
    let quarter = parse("12:15am (UTC)", NOW).expect("12:15am parses");
    assert_eq!((local(quarter).hour(), local(quarter).minute()), (0, 15));
}

#[test]
fn am_and_pm_are_read_in_any_case() {
    for f in [
        "8:30AM (America/Los_Angeles)",
        "8:30Am (America/Los_Angeles)",
        "8:30aM (America/Los_Angeles)",
    ] {
        assert_eq!(parse(f, NOW), Some(LA_0830), "fragment {f:?}");
    }
}

#[test]
fn text_after_the_closing_parenthesis_is_ignored() {
    assert_eq!(
        parse(
            "8:30am (America/Los_Angeles)\n/upgrade to increase your usage limit.",
            NOW
        ),
        Some(LA_0830)
    );
    assert_eq!(
        parse("8:30am (America/Los_Angeles) · more", NOW),
        Some(LA_0830)
    );
}

#[test]
fn documented_rejections_give_no_reset() {
    for f in [
        "8 (America/Los_Angeles)",
        "8:30am",
        "8:30am (Mars/Olympus)",
        "Oct 20 at 9am (UTC)",
        "Oct 20 9am (UTC)",
        "October 20, 9am (UTC)",
        "Oct 32, 9am (UTC)",
        "Oct 20, (UTC)",
        "99:99pm (UTC)",
    ] {
        assert_eq!(parse(f, NOW), None, "fragment {f:?}");
    }
}

#[test]
fn malformed_clock_times_and_zones_give_no_reset() {
    for f in [
        "",
        " 8:30am (UTC)",
        "13pm (UTC)",
        "0am (UTC)",
        "8:60am (UTC)",
        "8:5am (UTC)",
        "24:00 (UTC)",
        "23:60 (UTC)",
        "8:30am  (UTC)",
        "8:30am(UTC)",
        "8:30am (UTC",
        "8:30am ()",
        "8:30am UTC",
        "8:30am (utc/../../etc/passwd)",
        "8am (../../etc/passwd)",
        "8am (/usr/share/zoneinfo/UTC)",
        "-8:30am (UTC)",
        "+8:30am (UTC)",
        "８:30am (UTC)",
        "Monday 9am (UTC)",
        "2026-10-20 09:00 (UTC)",
        "tomorrow at 9am (UTC)",
    ] {
        assert_eq!(parse(f, NOW), None, "fragment {f:?}");
    }
}

#[test]
fn only_the_first_128_bytes_are_read() {
    // A zone whose closing parenthesis falls past byte 128 is never read.
    let long_zone = format!("8:30am ({})", "A".repeat(200));
    assert_eq!(parse(&long_zone, NOW), None);
    // A valid fragment followed by a long tail still parses: the fragment
    // fits inside the first 128 bytes.
    let tail = format!("8:30am (America/Los_Angeles){}", "x".repeat(10_000));
    assert_eq!(parse(&tail, NOW), Some(LA_0830));
}

#[test]
fn the_128_byte_cut_lands_on_a_character_boundary() {
    // 'é' is two bytes; with a 5-byte prefix the 128th byte splits one.
    for prefix_len in 0..4 {
        let f = format!("{}8am ({})", "x".repeat(prefix_len), "é".repeat(100));
        assert_eq!(parse(&f, NOW), None, "prefix {prefix_len}");
        let g = format!("8am (UTC{})", "€".repeat(60 + prefix_len));
        assert_eq!(parse(&g, NOW), None, "prefix {prefix_len}");
    }
}

#[test]
fn a_time_up_to_ten_minutes_past_is_today() {
    assert_eq!(
        parse("8:30am (America/Los_Angeles)", LA_0830),
        Some(LA_0830)
    );
    assert_eq!(
        parse("8:30am (America/Los_Angeles)", LA_0830 + 1),
        Some(LA_0830)
    );
    assert_eq!(
        parse("8:30am (America/Los_Angeles)", LA_0830 + 600),
        Some(LA_0830),
        "exactly 600 s past is still today"
    );
}

#[test]
fn a_time_more_than_ten_minutes_past_is_tomorrow() {
    assert_eq!(
        parse("8:30am (America/Los_Angeles)", LA_0830 + 601),
        Some(LA_0830 + DAY)
    );
    // 07:45 is 15 minutes before NOW (08:00).
    assert_eq!(
        parse("7:45am (America/Los_Angeles)", NOW),
        Some(at(LA, 2026, 10, 19, 7, 45))
    );
}

#[test]
fn parse_never_clamps() {
    // Tomorrow 07:45 is 23 h 45 m away; refusing it is the plan's job.
    let got = parse("7:45am (America/Los_Angeles)", NOW).unwrap();
    assert!(got > NOW + 5 * 3600, "{got} was clamped");
    // A time inside the grace stays in the past; it is not raised to now.
    let past = parse("7:55am (America/Los_Angeles)", NOW).unwrap();
    assert_eq!(past, at(LA, 2026, 10, 18, 7, 55));
    assert!(past < NOW);
}

#[test]
fn a_time_in_a_spring_forward_gap_becomes_the_first_instant_after_it() {
    // 2026-03-08 00:00 PST. 02:30 does not exist; it becomes 03:00 PDT.
    let now = 1_772_956_800;
    assert_eq!(
        parse("2:30am (America/Los_Angeles)", now),
        Some(1_772_964_000)
    );
    assert_eq!(
        parse("2:00am (America/Los_Angeles)", now),
        Some(1_772_964_000)
    );
}

#[test]
fn a_time_in_a_fall_back_overlap_becomes_the_earlier_instant() {
    // 2026-11-01 00:00 PDT. 01:30 occurs twice; the PDT one comes first.
    let now = 1_793_516_400;
    assert_eq!(
        parse("1:30am (America/Los_Angeles)", now),
        Some(1_793_521_800)
    );
}

#[test]
fn hostile_input_never_panics() {
    let mut seed: u64 = 0x1618_1618;
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) as u8
    };
    for len in [0usize, 1, 7, 127, 128, 129, 300, 10 * 1024] {
        for _ in 0..20 {
            let bytes: Vec<u8> = (0..len).map(|_| next()).collect();
            let text = String::from_utf8_lossy(&bytes);
            let _ = parse(&text, NOW);
            let _ = parse(&format!("8:30am ({text})"), NOW);
        }
    }
    for now in [i64::MIN, -1, 0, i64::MAX - 1, i64::MAX] {
        let _ = parse("8:30am (America/Los_Angeles)", now);
        let _ = parse("23:59 (Pacific/Kiritimati)", now);
    }
    let _ = parse(&"9".repeat(10_000), NOW);
    let _ = parse(&format!("{}:30am (UTC)", "9".repeat(40)), NOW);
}

#[test]
fn a_dated_reset_is_read_in_this_year_or_next() {
    // The real weekly-limit form. Oct 8 has passed, so next year's.
    assert_eq!(
        parse("Oct 8, 12am (America/Los_Angeles)", NOW),
        Some(at(LA, 2027, 10, 8, 0, 0))
    );
    assert_eq!(
        parse("oct 20, 9:15PM (America/Los_Angeles)", NOW),
        Some(at(LA, 2026, 10, 20, 21, 15))
    );
    // Today, within the grace, stays today.
    assert_eq!(
        parse("Oct 18, 7:55am (America/Los_Angeles)", NOW),
        Some(at(LA, 2026, 10, 18, 7, 55))
    );
    assert_eq!(
        parse("Jan 2, 13:00 (UTC)", NOW),
        Some(at(chrono_tz::UTC, 2027, 1, 2, 13, 0))
    );
}

// Issue #2230: the reset in "You've hit your 5-hour limit · resets 8:20pm
// (America/Los_Angeles)". Both fixed times are in PDT, so no daylight-saving
// change can move the expected instants.

/// 2026-10-18 20:20:00 -07:00.
const LA_2020: i64 = 1_792_380_000;
/// 2026-10-18 21:00:00 -07:00, past the 20:20 reset and its 10-minute grace.
const LA_2100: i64 = 1_792_382_400;

#[test]
fn the_five_hour_limit_reset_is_today_before_it_arrives() {
    assert_eq!(LA_2020, at(LA, 2026, 10, 18, 20, 20));
    assert_eq!(parse("8:20pm (America/Los_Angeles)", NOW), Some(LA_2020));
}

#[test]
fn the_five_hour_limit_reset_rolls_to_tomorrow_once_it_has_passed() {
    assert_eq!(LA_2100, at(LA, 2026, 10, 18, 21, 0));
    assert_eq!(
        parse("8:20pm (America/Los_Angeles)", LA_2100),
        Some(LA_2020 + DAY)
    );
}

// --- issue #2967: a reset with no zone, read in the host's zone ---------------
//
// `parse_in(fragment, now, local)`: a zone the text states follows the
// existing rule and never falls back to `local`; text that is empty or only
// ASCII whitespace after the clock time takes `local`; anything else is
// `None`. `parse` is `parse_in(.., None)`.

/// 2026-10-18 23:50:00 -07:00.
const LA_2350: i64 = 1_792_392_600;

#[test]
fn a_zoneless_clock_time_takes_the_local_zone() {
    // The observed wording ends `... your 5-hour limit resets 11:50pm`.
    assert_eq!(parse_in("11:50pm", NOW, Some(LA)), Some(LA_2350));
    assert_eq!(parse_in("8:30am", NOW, Some(LA)), Some(LA_0830));
    assert_eq!(parse_in("08:30", NOW, Some(LA)), Some(LA_0830));
    assert_eq!(
        parse_in("9pm", NOW, Some(chrono_tz::Europe::London)),
        Some(at(chrono_tz::Europe::London, 2026, 10, 18, 21, 0))
    );
}

#[test]
fn a_zoneless_dated_reset_takes_the_local_zone() {
    assert_eq!(
        parse_in("Oct 24, 2pm", NOW, Some(LA)),
        Some(at(LA, 2026, 10, 24, 14, 0))
    );
    // A date already past is next year's, as for a stated zone.
    assert_eq!(
        parse_in("Oct 8, 12am", NOW, Some(LA)),
        Some(at(LA, 2027, 10, 8, 0, 0))
    );
}

#[test]
fn a_zoneless_clock_time_follows_the_ten_minute_grace_rule() {
    // 07:55 is five minutes ago: still today's. 07:00 is an hour ago: tomorrow's.
    assert_eq!(
        parse_in("7:55am", NOW, Some(LA)),
        Some(at(LA, 2026, 10, 18, 7, 55))
    );
    assert_eq!(
        parse_in("7:00am", NOW, Some(LA)),
        Some(at(LA, 2026, 10, 19, 7, 0))
    );
}

#[test]
fn a_zoneless_clock_time_with_no_local_zone_is_none() {
    assert_eq!(parse_in("11:50pm", NOW, None), None);
    // `parse` keeps its behaviour: the zone is required.
    assert_eq!(parse("11:50pm", NOW), None);
    assert_eq!(parse("8:30am", NOW), None);
}

#[test]
fn only_whitespace_may_follow_a_zoneless_clock_time() {
    for tail in ["", " ", "  ", "\t", " \t "] {
        let f = format!("8:30am{tail}");
        assert_eq!(parse_in(&f, NOW, Some(LA)), Some(LA_0830), "{f:?}");
    }
    for tail in [
        ".",
        " foo",
        "\u{a0}",                // non-breaking space is not ASCII whitespace
        "\u{7}",                 // a control character
        " (",                    // an unclosed bracket opens a stated zone
        " (America/Los_Angeles", // a stated zone that never closes
        "\n/upgrade to increase your usage limit.",
    ] {
        let f = format!("8:30am{tail}");
        assert_eq!(parse_in(&f, NOW, Some(LA)), None, "{f:?}");
    }
}

#[test]
fn a_stated_zone_is_used_and_never_falls_back_to_the_local_one() {
    // The text's zone wins over `local`.
    assert_eq!(
        parse_in("8:30am (America/Los_Angeles)", NOW, Some(chrono_tz::UTC)),
        Some(LA_0830)
    );
    // A zone that does not exist is not replaced by `local`.
    assert_eq!(parse_in("8:30am (Nowhere/Zone)", NOW, Some(LA)), None);
    assert_eq!(parse_in("8:30am ()", NOW, Some(LA)), None);
    // Nor is a malformed one: exactly one space before the bracket.
    assert_eq!(
        parse_in("8:30am  (America/Los_Angeles)", NOW, Some(LA)),
        None
    );
}

#[test]
fn parse_is_parse_in_with_no_local_zone() {
    for f in [
        "8:30am (America/Los_Angeles)",
        "Oct 8, 12am (America/Los_Angeles)",
        "17:00 (Asia/Tokyo)",
        "8:30am",
        "8:30am (Nowhere/Zone)",
        "",
        "tomorrow",
    ] {
        assert_eq!(parse(f, NOW), parse_in(f, NOW, None), "{f:?}");
    }
}

#[test]
fn a_zoneless_reset_in_a_daylight_saving_gap_is_the_first_instant_after_it() {
    // 2026-03-08 02:30 does not exist in Los Angeles; the clocks go 02:00 -> 03:00.
    let now = at(LA, 2026, 3, 8, 1, 0);
    assert_eq!(
        parse_in("2:30am", now, Some(LA)),
        Some(at(LA, 2026, 3, 8, 3, 0))
    );
}

#[test]
fn extreme_instants_never_panic_and_are_not_clamped() {
    for now in [i64::MAX, i64::MIN, 0, i64::MAX - 1, i64::MIN + 1] {
        // The value does not matter; reaching the end does.
        let _ = parse_in("11:50pm", now, Some(LA));
        let _ = parse_in("Oct 8, 12am", now, Some(LA));
        let _ = parse_in("8:30am (America/Los_Angeles)", now, None);
    }
}

#[test]
fn a_zoneless_fragment_is_still_cut_at_128_bytes_on_a_character_boundary() {
    let long = format!("8:30am{}\u{e9}", " ".repeat(200));
    // Nothing but whitespace within the first 128 bytes, then text beyond
    // the cut: the cut text is not read.
    assert_eq!(parse_in(&long, NOW, Some(LA)), Some(LA_0830));
}
