//! The heartbeat rewrite (issue #2967): while a step waits for a usage limit,
//! the runner's `... working (` heartbeat for it reads
//! `... waiting for usage limit until <until> (...)`.
//!
//! Contract of `heartbeat.rs`, all pure:
//!
//! - `rewrite(line, lookup, tz) -> Option<String>`. `lookup(pid)` gives the
//!   `until` of the live, unexpired wait-state record of supervisor `pid`, or
//!   `None`; it owns freshness, so a record whose `until` has passed gives
//!   `None`. `tz` formats `until` as the wait's own `until=` field is formatted
//!   (`%Y-%m-%d %H:%M:%S %:z`). The result is `None` for every line that is
//!   not a heartbeat of a waiting step, and then the caller prints the line
//!   byte for byte. Only the word `working` is replaced.
//! - `is_wait_line(line) -> bool`: an anchored whole-line match of the
//!   rewritten form, built from the same wording constants as `rewrite`.
//! - `pump(line, lookup, tz) -> Pumped`, what the stderr pump does with one
//!   line: `print` is what goes to the terminal and the recipe log, and
//!   `buffer` is what goes into the bounded failure-evidence buffer, `None`
//!   for nothing. The buffer keeps the runner's own line, never the rewrite,
//!   and drops a wait line, which names a usage limit and would otherwise be
//!   read as one by the failure classifier. Any doubt buffers the line.
//!
//! The line is the pinned runner's (`HeartbeatReport::render` in
//! `amplihack-recipe-runner`): `  [HH:MM:SS] [amplihack:<agent>:<pid>] ...
//! working (<n>s elapsed, <n>s since last output)`. The pid is the one that
//! ends the label, which is the step's supervisor. A pid of 0, one above
//! `i32::MAX`, or one that is not plain decimal is never looked up.
//!
//! Wired from `recipe/run/mod.rs` with `#[cfg(test)] mod tests_heartbeat;`.

use super::heartbeat::{Pumped, is_wait_line, pump, rewrite};
use super::limit_patterns::all_markers;
use chrono_tz::Tz;
use std::cell::RefCell;

const LA: Tz = chrono_tz::America::Los_Angeles;
/// 2026-10-18 08:32:00 -07:00.
const UNTIL: i64 = 1_792_337_520;
const UNTIL_TEXT: &str = "2026-10-18 08:32:00 -07:00";
const PID: u32 = 41_822;

const WORKING: &str =
    "  [15:01:10] [amplihack:claude:41822] ... working (3590s elapsed, 3590s since last output)";
const WAITING: &str = "  [15:01:10] [amplihack:claude:41822] ... waiting for usage limit until 2026-10-18 08:32:00 -07:00 (3590s elapsed, 3590s since last output)";

fn live(pid: u32) -> Option<Option<i64>> {
    (pid == PID).then_some(Some(UNTIL))
}

fn none(_: u32) -> Option<Option<i64>> {
    None
}

/// A supervisor waiting with no deadline (`until=none`, issue #2911).
fn unbounded(pid: u32) -> Option<Option<i64>> {
    (pid == PID).then_some(None)
}

/// A lookup, as `rewrite` takes one: whether a supervisor is waiting, and
/// the `until` of its record, `None` inside for a wait with no deadline.
type Lookup<'a> = &'a dyn Fn(u32) -> Option<Option<i64>>;

#[test]
fn a_heartbeat_with_a_live_record_is_rewritten() {
    assert_eq!(rewrite(WORKING, live, &LA).as_deref(), Some(WAITING));
}

#[test]
fn until_is_formatted_like_the_waits_until_field_in_the_given_zone() {
    let utc = rewrite(WORKING, live, &chrono_tz::UTC).unwrap();
    assert!(utc.contains("until 2026-10-18 15:32:00 +00:00 ("), "{utc}");
    let la = rewrite(WORKING, live, &LA).unwrap();
    assert!(la.contains(&format!("until {UNTIL_TEXT} (")), "{la}");
}

#[test]
fn only_the_word_working_changes() {
    for line in [
        WORKING,
        "[15:01:10] [amplihack:claude:41822] ... working (1s elapsed, 0s since last output)",
        "    [00:00:00] [amplihack:copilot:41822] ... working (45s elapsed, 32s since last output)",
        "\t[23:59:59] [amplihack:codex:41822] ... working (9999s elapsed, 12s since last output)",
        // The label alone, with no time of day before it.
        "  [amplihack:claude:41822] ... working (7s elapsed, 7s since last output)",
    ] {
        let out = rewrite(line, live, &LA).unwrap_or_else(|| panic!("{line}"));
        let (head, tail) = line.split_once("working").unwrap();
        assert_eq!(
            out,
            format!("{head}waiting for usage limit until {UNTIL_TEXT}{tail}"),
            "{line}"
        );
        assert!(!out.contains("working"), "{out}");
    }
}

#[test]
fn the_pid_looked_up_is_the_one_that_ends_the_label() {
    let asked = RefCell::new(Vec::new());
    let lookup = |pid| {
        asked.borrow_mut().push(pid);
        None
    };
    assert_eq!(rewrite(WORKING, lookup, &LA), None);
    assert_eq!(*asked.borrow(), [PID]);
}

#[test]
fn every_line_that_is_not_a_heartbeat_of_a_waiting_step_is_left_alone() {
    let any: Lookup<'_> = &|_| Some(Some(UNTIL));
    let cases: &[(&str, Lookup<'_>)] = &[
        // No record for that pid: not waiting (or the record expired).
        (WORKING, &none),
        (
            "  [15:01:10] [amplihack:claude:7] ... working (3s elapsed, 3s since last output)",
            &live,
        ),
        // Not the heartbeat shape.
        ("", any),
        ("working", any),
        (
            "  [15:01:10] [amplihack:claude:41822] ... still working (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:41822] ... idle (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:41822] working (3s elapsed, 3s since last output)",
            any,
        ),
        // The runner's other lines for the same step.
        (
            "  [15:01:10] [amplihack:claude:41822] child process ended: exit 1 after 32s",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:41822] Reading the issue",
            any,
        ),
        // Not anchored: the shape inside other text.
        (
            "log: [15:01:10] [amplihack:claude:41822] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10] note [amplihack:claude:41822] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10]  [amplihack:claude:41822] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        ("model: \"] ... working (\" amplihack:claude:41822", any),
        // A bracket inside a group.
        (
            "  [15:01:10] [amplihack:[claude]:41822] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        // Text after the heartbeat's closing parenthesis.
        (
            "  [15:01:10] [amplihack:claude:41822] ... working (3s elapsed, 3s since last output) extra",
            any,
        ),
        // Counts that are not the runner's.
        (
            "  [15:01:10] [amplihack:claude:41822] ... working (3s elapsed)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:41822] ... working (s elapsed, 3s since last output)",
            any,
        ),
        // The form runners printed before they labelled the heartbeat.
        (
            "  [agent] ... working (3590s elapsed, 3590s since last output, pid 41822 alive)",
            any,
        ),
        // No pid in the label, or one that cannot be a process id.
        (
            "  [15:01:10] [agent] ... working (45s elapsed, 32s since last output)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:abc] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:-5] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude: 5] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:0] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:041822] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:2147483648] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        (
            "  [15:01:10] [amplihack:claude:99999999999999999999] ... working (3s elapsed, 3s since last output)",
            any,
        ),
        // Already rewritten: nothing left to replace.
        (WAITING, &live),
    ];
    for (line, lookup) in cases {
        assert_eq!(rewrite(line, lookup, &LA), None, "{line:?}");
    }
}

#[test]
fn a_pid_at_the_i32_limit_is_looked_up_and_one_above_it_is_not() {
    let line = |pid: u64| {
        format!(
            "  [15:01:10] [amplihack:claude:{pid}] ... working (3s elapsed, 3s since last output)"
        )
    };
    assert!(rewrite(&line(i32::MAX as u64), |_| Some(Some(UNTIL)), &LA).is_some());
    assert!(rewrite(&line(i32::MAX as u64 + 1), |_| Some(Some(UNTIL)), &LA).is_none());
    assert!(rewrite(&line(1), |_| Some(Some(UNTIL)), &LA).is_some());
}

#[test]
fn an_extreme_until_does_not_panic() {
    for until in [i64::MAX, i64::MIN, 0, -1] {
        let _ = rewrite(WORKING, |_| Some(Some(until)), &LA);
    }
}

// --- a wait with no deadline (issue #2911) -------------------------------------

const WAITING_NONE: &str = "  [15:01:10] [amplihack:claude:41822] ... waiting for usage limit until none (3590s elapsed, 3590s since last output)";

#[test]
fn a_heartbeat_of_a_wait_with_no_deadline_says_until_none() {
    assert_eq!(
        rewrite(WORKING, unbounded, &LA).as_deref(),
        Some(WAITING_NONE)
    );
    assert!(is_wait_line(WAITING_NONE));
}

#[test]
fn the_pump_treats_a_no_deadline_wait_line_as_a_wait_line() {
    let Pumped { print, buffer } = pump(WORKING, unbounded, &LA);
    assert_eq!(print, WAITING_NONE);
    assert_eq!(
        buffer.as_deref(),
        Some(WORKING),
        "the runner's line is kept"
    );
    let Pumped { print, buffer } = pump(WAITING_NONE, none, &LA);
    assert_eq!(print, WAITING_NONE);
    assert_eq!(buffer, None, "a nested run's wait line is not kept");
}

#[test]
fn none_must_be_followed_by_the_counts_alone() {
    for line in [
        "  [15:01:10] [amplihack:claude:41822] ... waiting for usage limit until nonesuch (3s elapsed, 3s since last output)",
        "  [15:01:10] [amplihack:claude:41822] ... waiting for usage limit until none",
        "  [15:01:10] [amplihack:claude:41822] ... waiting for usage limit until none (3s elapsed, 3s since last output) weekly limit",
    ] {
        assert!(!is_wait_line(line), "{line:?}");
    }
}

#[test]
fn a_rewritten_line_is_a_wait_line() {
    let out = rewrite(WORKING, live, &LA).unwrap();
    assert!(is_wait_line(&out));
    assert!(is_wait_line(WAITING));
}

#[test]
fn a_line_that_is_not_exactly_the_rewritten_form_is_not_a_wait_line() {
    let marked = format!("{WAITING} You've hit your weekly limit \u{b7} resets 9am");
    let named = format!("{WAITING} 5-hour limit");
    for line in [
        "",
        WORKING,
        // The wait phrase inside other text.
        "log: [15:01:10] [amplihack:claude:41822] ... waiting for usage limit until 2026-10-18 08:32:00 -07:00 (3590s elapsed, 3590s since last output)",
        // A limit marker beyond the wait phrase.
        marked.as_str(),
        named.as_str(),
        // A limit marker in the label.
        "  [15:01:10] [weekly limit:41822] ... waiting for usage limit until 2026-10-18 08:32:00 -07:00 (3590s elapsed, 3590s since last output)",
        // Not a time.
        "  [15:01:10] [amplihack:claude:41822] ... waiting for usage limit until soon (3590s elapsed, 3590s since last output)",
        "  [15:01:10] [amplihack:claude:41822] ... waiting for usage limit until  (3s elapsed, 3s since last output)",
        // A model's sentence.
        "The step is waiting for usage limit until tomorrow",
        "waiting for usage limit until 2026-10-18 08:32:00 -07:00",
    ] {
        assert!(!is_wait_line(line), "{line:?}");
    }
}

#[test]
fn the_rewritten_heartbeat_names_a_usage_limit_which_is_why_it_is_not_buffered() {
    let out = rewrite(WORKING, live, &LA).unwrap().to_ascii_lowercase();
    assert!(
        all_markers().iter().any(|m| out.contains(m)),
        "the wait phrase carries a marker, so it must never reach the classifier: {out}"
    );
}

#[test]
fn the_pump_prints_the_rewrite_and_buffers_the_runners_own_line() {
    let Pumped { print, buffer } = pump(WORKING, live, &LA);
    assert_eq!(print, WAITING);
    assert_eq!(buffer.as_deref(), Some(WORKING));
}

#[test]
fn the_pump_passes_a_heartbeat_with_no_record_unchanged_and_buffers_it() {
    let Pumped { print, buffer } = pump(WORKING, none, &LA);
    assert_eq!(print, WORKING);
    assert_eq!(buffer.as_deref(), Some(WORKING));
}

#[test]
fn the_pump_prints_a_wait_line_from_a_nested_run_and_does_not_buffer_it() {
    let Pumped { print, buffer } = pump(WAITING, none, &LA);
    assert_eq!(print, WAITING);
    assert_eq!(buffer, None);
}

#[test]
fn the_pump_buffers_a_line_that_carries_the_wait_phrase_and_a_limit_marker() {
    let mixed = format!("{WAITING} You've hit your weekly limit \u{b7} resets 9am");
    let Pumped { print, buffer } = pump(&mixed, none, &LA);
    assert_eq!(print, mixed);
    assert_eq!(buffer.as_deref(), Some(mixed.as_str()));
}

#[test]
fn the_pump_leaves_every_other_line_byte_for_byte_and_buffers_it() {
    for line in [
        "",
        "plain output",
        "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity harness=claude reset=none until=none bound=none",
        "You've hit your session limit · resets 8:30am (America/Los_Angeles)",
        "  \u{e9} non-ascii \u{fffd}",
        "  [15:01:10] [amplihack:claude:41822] ... working",
        "  [15:01:10] [amplihack:claude:41822] child process ended: exit 0 after 5s",
    ] {
        let Pumped { print, buffer } = pump(line, live, &LA);
        assert_eq!(print, line);
        assert_eq!(buffer.as_deref(), Some(line));
    }
}

#[test]
fn a_rewrite_never_reaches_the_buffer() {
    // The classifier reads the buffer; the rewrite names a usage limit.
    let Pumped { buffer, .. } = pump(WORKING, live, &LA);
    let kept = buffer.unwrap().to_ascii_lowercase();
    assert!(!kept.contains("waiting for usage limit"), "{kept}");
}

// --- no timer, no polling (issue #2967 review) ---------------------------------
//
// The heartbeat rewrite and the wait-state record must add no timer, no
// polling loop, no retry and no gate. These two tests are that rule's
// evidence: the code of both files names no sleep, thread, interval or
// duration, and the record is looked up only for a line that is already the
// runner's heartbeat, so the runner's own clock is the only one involved.

/// The code lines of `source`, with `//` comments dropped.
fn code(source: &str) -> Vec<&str> {
    source
        .lines()
        .map(str::trim_start)
        .filter(|l| !l.starts_with("//"))
        .collect()
}

#[test]
fn neither_the_rewrite_nor_the_record_has_a_timer_of_its_own() {
    const FORBIDDEN: &[&str] = &[
        "sleep",
        "thread::",
        "spawn(",
        "Instant",
        "Duration",
        "recv_timeout",
        "park_timeout",
        "interval",
    ];
    for (name, source) in [
        ("recipe/run/heartbeat.rs", include_str!("heartbeat.rs")),
        (
            "profile/limit_wait/wait_state.rs",
            include_str!("../../profile/limit_wait/wait_state.rs"),
        ),
    ] {
        for line in code(source) {
            for word in FORBIDDEN {
                assert!(!line.contains(word), "{name}: {word:?} in {line:?}");
            }
        }
    }
}

#[test]
fn the_record_is_looked_up_only_for_a_runner_heartbeat_line() {
    let asked = RefCell::new(0u32);
    let lookup = |_| {
        *asked.borrow_mut() += 1;
        Some(Some(UNTIL))
    };
    for line in [
        "",
        "plain output",
        "amplihack.recipe.usage_limit_wait progress remaining=60s",
        "  [15:01:10] [amplihack:claude:41822] child process ended: exit 0 after 5s",
        "  [15:01:10] [amplihack:claude:41822] ... working",
        WAITING,
    ] {
        let _ = pump(line, lookup, &LA);
    }
    assert_eq!(*asked.borrow(), 0, "no line but a heartbeat reads a record");
    let _ = pump(WORKING, lookup, &LA);
    assert_eq!(*asked.borrow(), 1, "one heartbeat, one lookup");
}
