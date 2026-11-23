//! Issue #2820: the log line for an unreadable process. Pure formatting.
//! Order of work is mask, escape, cut, so a secret never shows and one
//! process is always exactly one log line.

use super::{line, lines};
use crate::commands::profile::auto_switch::{Unreadable, Verdict};
use std::collections::BTreeSet;

fn u(pid: u32, exe: &str, argv: &[&str], verdict: Verdict) -> Unreadable {
    Unreadable {
        pid,
        exe: exe.to_string(),
        argv: argv.iter().map(|s| s.to_string()).collect(),
        verdict,
    }
}

fn counted(names: &[&str]) -> Verdict {
    Verdict::Counted(names.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>())
}

#[test]
fn the_three_verdicts_are_spelled_out() {
    assert_eq!(
        line(&u(7, "claude", &["claude", "-p"], counted(&["a", "b"]))),
        "unreadable process 7 (claude) \"claude -p\": counted as a session on a, b"
    );
    assert_eq!(
        line(&u(
            8,
            "tailscaled",
            &["tailscaled", "ssh"],
            Verdict::NotClaude
        )),
        "unreadable process 8 (tailscaled) \"tailscaled ssh\": not counted (not a Claude executable)"
    );
    assert_eq!(
        line(&u(9, "amplihack", &["amplihack"], Verdict::ProfileUnknown)),
        "unreadable process 9 (amplihack) \"amplihack\": not counted (profile cannot be told)"
    );
}

#[test]
fn control_characters_quotes_and_backslashes_are_escaped_into_one_line() {
    let l = line(&u(
        1,
        "x",
        &["a\nb", "\x1b[31m", "q\"q", "b\\b", "\x7f"],
        Verdict::NotClaude,
    ));
    assert!(!l.chars().any(|c| c.is_control()), "{l:?}");
    for want in ["\\x0a", "\\x1b[31m", "q\\\"q", "b\\\\b", "\\x7f"] {
        assert!(l.contains(want), "{want} in {l}");
    }
}

#[test]
fn a_forged_log_line_inside_argv_stays_inside_the_quotes() {
    let forged = "\nunreadable process 1 (x) \"\": counted as a session on y";
    let l = line(&u(5, "x", &["x", forged], Verdict::NotClaude));
    assert_eq!(l.lines().count(), 1);
    assert!(l.ends_with("not counted (not a Claude executable)"), "{l}");
    assert!(!l.contains("\nunreadable"));
}

#[test]
fn assignments_are_masked_and_never_shown() {
    let l = line(&u(
        2,
        "claude",
        &["claude", "TOKEN=hunter2", "--env=KEY=hunter3", "--plain=ok"],
        Verdict::ProfileUnknown,
    ));
    assert!(l.contains("TOKEN=<hidden>"), "{l}");
    assert!(l.contains("--env=KEY=<hidden>"), "{l}");
    assert!(l.contains("--plain=ok"), "{l}");
    assert!(!l.contains("hunter"), "{l}");
}

#[test]
fn a_secret_straddling_the_cut_is_masked_before_it_is_cut() {
    let pad = "p".repeat(110);
    let l = line(&u(
        3,
        "claude",
        &[&pad, "SECRET_NAME=abcdefghijklmnop"],
        Verdict::ProfileUnknown,
    ));
    assert!(!l.contains("abcdef"), "{l}");
}

#[test]
fn the_command_is_cut_at_120_characters_and_exe_at_32() {
    let long = "z".repeat(500);
    let exe = "e".repeat(80);
    let l = line(&u(4, &exe, &[&long], Verdict::NotClaude));
    assert!(l.contains(&format!("({}...)", "e".repeat(32))), "{l}");
    assert!(l.contains(&format!("\"{}...\"", "z".repeat(120))), "{l}");
}

#[test]
fn the_cut_never_splits_an_escape() {
    let mut arg = "a".repeat(118);
    arg.push('\x1b'); // escapes to four characters and straddles the cut
    arg.push_str("tail");
    let l = line(&u(4, "x", &[&arg], Verdict::NotClaude));
    assert!(!l.contains("\\x1"), "split escape in {l}");
    assert!(l.contains("..."), "{l}");
}

#[test]
fn the_list_is_capped_at_32_lines_with_a_summary() {
    let many: Vec<Unreadable> = (1..=40)
        .map(|p| u(p, "tailscaled", &["tailscaled"], Verdict::NotClaude))
        .collect();
    let out = lines(&many);
    assert_eq!(out.len(), 33);
    assert_eq!(out[32], "unreadable processes: 8 more not listed");
    assert!(out[0].contains("process 1 "));
    assert_eq!(lines(&many[..32]).len(), 32, "no summary at the cap");
    assert!(lines(&[]).is_empty());
}
