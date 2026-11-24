//! Issue #2085 (T9): `limit_resume::keys`, the only keys the hook may send
//! and the only pane it may send them to.
//!
//! Contract (design C4, D6, D8, D9, S3, S10, S13):
//! - `Key::{Escape, ClearLine, LiteralContinue, Enter}`; `RESUME_KEYS` is
//!   exactly those four, in that order. No other key can be expressed.
//! - `Pane::parse(tmux, tmux_pane)`: `$TMUX` set and non-empty, `$TMUX_PANE`
//!   exactly `^%[0-9]{1,9}$`. `id()`, `log_tag()` (12 hex digits of a
//!   SHA-256); `Debug` shows only the tag; no `Display`.
//! - `argv(&Pane, Key)`: one fixed `tmux send-keys` argv per key;
//!   `pane_tty_argv(&Pane)`: `display-message -p -t %N #{pane_tty}`.
//! - `send_with(program, &Pane, Key)`: runs `program` with `argv`, no shell,
//!   null stdio; a non-zero exit is an error. `tmux_send` is
//!   `send_with("tmux", …)`.
//! - `parse_stat(text) -> Option<ProcStat { ppid, pgrp, tpgid }>` splits
//!   after the last `)`; `ProcStat::foreground()` is `pgrp == tpgid`. The
//!   ancestry walk the pane check uses (#2695) reads `ppid` from the same
//!   line, so it is asserted here too.
//! - `foreground_now(pid)`: one read of `/proc/<pid>/stat`; false when the
//!   process is gone, unreadable or not in the foreground (S4, Ctrl-Z).
//! - `same_tty(pane_tty_output, stdin_target)`: the pane's tty (at most 256
//!   bytes, absolute, under `/dev/`, one trailing newline allowed) is the
//!   file Claude Code's stdin points at.
//! - `capture_argv(&Pane)`: `capture-pane -p -t %N`, with no `-S`, so only
//!   the visible screen is read. `recognise(capture)` (#2695) is `Held` for
//!   a screen holding one of `SCREEN_MARKERS`, `Unrecognised` otherwise:
//!   `screen-unreadable` for nothing to read, `screen-unknown` for a screen
//!   the keys do not answer. Nothing read is returned.
//!
//! No test talks to a tmux server.

use amplihack_hooks::limit_resume::keys::{
    Key, Pane, ProcStat, RESUME_KEYS, SCREEN_MARKERS, SCREEN_UNKNOWN, SCREEN_UNREADABLE,
    ScreenCheck, UNRECOGNISED_REASONS, argv, capture_argv, foreground_now, list_panes_argv,
    pane_on_tty, pane_tty_argv, parse_stat, recognise, same_tty, send_with,
};
use std::ffi::OsStr;
use std::path::Path;

const TMUX: Option<&str> = Some("/tmp/tmux-1000/default,4242,0");

fn pane(id: &str) -> Pane {
    Pane::parse(TMUX, Some(id)).unwrap_or_else(|| panic!("{id} is a pane id"))
}

// ── Keys ────────────────────────────────────────────────────────────────────

#[test]
fn the_resume_keys_are_escape_clear_continue_enter() {
    assert_eq!(
        RESUME_KEYS,
        [
            Key::Escape,
            Key::ClearLine,
            Key::LiteralContinue,
            Key::Enter
        ]
    );
}

#[test]
fn each_key_has_one_fixed_argv() {
    let p = pane("%17");
    let want: [&[&str]; 4] = [
        &["send-keys", "-t", "%17", "Escape"],
        &["send-keys", "-t", "%17", "C-u"],
        &["send-keys", "-t", "%17", "-l", "continue"],
        &["send-keys", "-t", "%17", "Enter"],
    ];
    for (key, want) in RESUME_KEYS.iter().zip(want) {
        assert_eq!(argv(&p, *key), want, "{key:?}");
    }
}

#[test]
fn the_pane_tty_query_is_fixed() {
    assert_eq!(
        pane_tty_argv(&pane("%3")),
        ["display-message", "-p", "-t", "%3", "#{pane_tty}"]
    );
}

#[test]
fn no_argv_can_send_a_digit_a_slash_command_or_other_text() {
    // D9: the limit menu's digits and the paid-usage commands are never sent.
    let p = pane("%905");
    for key in RESUME_KEYS {
        let a = argv(&p, key);
        assert_eq!(&a[..3], ["send-keys", "-t", "%905"], "{key:?}");
        let rest = &a[3..];
        for arg in rest {
            assert!(!arg.chars().any(|c| c.is_ascii_digit()), "{key:?}: {arg}");
            assert!(!arg.contains('/'), "{key:?}: {arg}");
            assert!(!arg.contains("usage-credits"), "{key:?}: {arg}");
            assert!(!arg.contains("rate-limit-options"), "{key:?}: {arg}");
            assert!(!arg.contains(' '), "{key:?}: {arg}");
        }
        // `-l` (literal) only ever carries `continue`, and `continue` is
        // only ever sent literally.
        let literal = rest.iter().any(|a| a == "-l");
        let cont = rest.iter().any(|a| a == "continue");
        assert_eq!(literal, key == Key::LiteralContinue, "{key:?}");
        assert_eq!(cont, key == Key::LiteralContinue, "{key:?}");
        assert!(
            rest.len() <= 2,
            "one key name or `-l continue` per run: {a:?}"
        );
    }
}

// ── Pane ────────────────────────────────────────────────────────────────────

#[test]
fn a_pane_needs_tmux_and_a_well_formed_id() {
    for ok in ["%0", "%7", "%123456789"] {
        let p = Pane::parse(TMUX, Some(ok)).unwrap_or_else(|| panic!("{ok}"));
        assert_eq!(p.id(), ok);
    }
    for bad in [
        "",
        "%",
        "%a",
        "1",
        "%1;x",
        "%1 ",
        " %1",
        "%-1",
        "%1\n",
        "%%1",
        "%1234567890",
        "%١",
        "$1",
        "@1",
    ] {
        assert!(Pane::parse(TMUX, Some(bad)).is_none(), "{bad:?}");
    }
    assert!(Pane::parse(TMUX, None).is_none());
    // D6: an inherited $TMUX_PANE without $TMUX is not a pane.
    assert!(Pane::parse(None, Some("%1")).is_none());
    assert!(Pane::parse(Some(""), Some("%1")).is_none());
}

#[test]
fn the_log_tag_is_a_hash_prefix_and_hides_the_id() {
    let p = pane("%42");
    let tag = p.log_tag();
    assert_eq!(tag.len(), 12);
    assert!(
        tag.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "{tag}"
    );
    assert!(!tag.contains('%'), "{tag}");
    assert_eq!(tag, pane("%42").log_tag(), "stable");
    assert_ne!(tag, pane("%43").log_tag());
}

#[test]
fn debug_shows_only_the_tag() {
    let p = pane("%42");
    let shown = format!("{p:?}");
    assert!(!shown.contains('%'), "{shown}");
    assert!(shown.contains(&p.log_tag()), "{shown}");
}

// ── Sending ─────────────────────────────────────────────────────────────────

#[test]
fn a_zero_exit_is_sent_and_a_non_zero_exit_is_an_error() {
    let p = pane("%1");
    assert!(send_with(OsStr::new("true"), &p, Key::Enter).is_ok());
    assert!(send_with(OsStr::new("false"), &p, Key::Enter).is_err());
}

#[test]
fn a_missing_program_is_an_error() {
    let p = pane("%1");
    let missing = OsStr::new("/nonexistent/amplihack-test/tmux");
    assert!(send_with(missing, &p, Key::Escape).is_err());
}

// ── Pane verification (S3) ──────────────────────────────────────────────────

fn stat_line(comm: &str, pgrp: i32, tpgid: i32) -> String {
    format!("4321 ({comm}) S 4000 {pgrp} 4321 34817 {tpgid} 4194560 100 0 0 0 1 2 0 0 20 0 9 0")
}

#[test]
fn parse_stat_reads_pgrp_and_tpgid() {
    let s = parse_stat(&stat_line("claude", 4321, 4321)).unwrap();
    assert_eq!(
        s,
        ProcStat {
            ppid: 4000,
            pgrp: 4321,
            tpgid: 4321
        }
    );
    assert!(s.foreground());
}

#[test]
fn parse_stat_splits_after_the_last_paren() {
    // A command name may hold `) (` and digits; only the last `)` ends it.
    let s = parse_stat(&stat_line("x) (S 1 2 3 4 5", 77, 78)).unwrap();
    assert_eq!(s.ppid, 4000);
    assert_eq!(s.pgrp, 77);
    assert_eq!(s.tpgid, 78);
    assert!(!s.foreground(), "a background process group gets no keys");
}

#[test]
fn parse_stat_refuses_malformed_text() {
    for bad in [
        "",
        "4321 claude S 1 2 3 4 5",
        "4321 (claude) S 1",
        "4321 (claude) S 1 x 3 4 y",
        "4321 (claude",
    ] {
        assert!(parse_stat(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn no_controlling_terminal_is_not_foreground() {
    // tpgid is -1 without a controlling terminal.
    let s = parse_stat(&stat_line("claude", 4321, -1)).unwrap();
    assert!(!s.foreground());
}

#[test]
fn the_pane_tty_must_be_claude_codes_stdin() {
    let stdin = Path::new("/dev/pts/7");
    assert!(same_tty(b"/dev/pts/7\n", stdin));
    assert!(same_tty(b"/dev/pts/7", stdin));
    for bad in [
        &b"/dev/pts/8\n"[..],
        b"",
        b"\n",
        b"pts/7\n",
        b"/tmp/pts/7\n",
        b"/dev/pts/7\n\n",
        b"/dev/pts/7 \n",
        b"/dev/../tmp/x\n",
    ] {
        assert!(!same_tty(bad, stdin), "{:?}", String::from_utf8_lossy(bad));
    }
    // An stdin that is not a tty under /dev is never a match.
    assert!(!same_tty(b"/tmp/x\n", Path::new("/tmp/x")));
    assert!(!same_tty(b"pipe:[123]\n", Path::new("pipe:[123]")));
}

#[test]
fn an_oversized_pane_tty_is_refused() {
    let long = format!("/dev/{}", "p".repeat(300));
    assert!(!same_tty(long.as_bytes(), Path::new(&long)));
}

#[test]
fn foreground_now_is_false_for_a_pid_that_does_not_exist() {
    // Above the kernel's pid_max (2^22), so no process can hold it.
    assert!(!foreground_now(u32::MAX));
}

// ── What the pane is showing (issue #2695) ──────────────────────────────────

#[test]
fn capture_argv_reads_the_visible_screen_of_one_pane() {
    assert_eq!(
        capture_argv(&pane("%5")),
        ["capture-pane", "-p", "-t", "%5"]
    );
    // No `-S`: the scrollback is never read, so an old limit notice above
    // the screen cannot stand in for the one showing now.
    assert!(!capture_argv(&pane("%5")).iter().any(|a| a == "-S"));
}

#[test]
fn the_menu_and_the_automatic_continue_screens_are_recognised() {
    for screen in [
        "Claude usage limit reached - resets at 3:00pm\n\nWhat do you want to do?\n",
        "WHAT DO YOU WANT TO DO?",
        "Usage limit reached - Continuing automatically at 12:30am",
        "You've hit your 5-hour limit - resets 3pm",
        "You've hit your weekly limit - resets Monday",
        "You've hit your session limit",
    ] {
        assert_eq!(
            recognise(screen.as_bytes()),
            ScreenCheck::Held,
            "a held screen: {screen}"
        );
    }
}

#[test]
fn a_screen_the_keys_do_not_answer_is_refused_by_name() {
    assert_eq!(
        recognise(b"user@host:~/src$ git status\nOn branch main\n"),
        ScreenCheck::Unrecognised(SCREEN_UNKNOWN)
    );
    // A permission prompt is not a usage limit, and Enter would answer it.
    assert_eq!(
        recognise(b"Do you want to allow this edit?\n  1. Yes\n  2. No\n"),
        ScreenCheck::Unrecognised(SCREEN_UNKNOWN)
    );
}

#[test]
fn nothing_to_read_is_refused_as_unreadable() {
    for empty in [&b""[..], b"\n", b"   \n\t\n"] {
        assert_eq!(
            recognise(empty),
            ScreenCheck::Unrecognised(SCREEN_UNREADABLE),
            "{:?}",
            String::from_utf8_lossy(empty)
        );
    }
}

#[test]
fn a_capture_that_is_not_utf8_is_refused_and_never_returned() {
    // Invalid UTF-8 must neither panic nor pass: lossy decoding leaves
    // replacement characters, which match no marker.
    assert_eq!(
        recognise(&[0xff, 0xfe, 0x80, 0x0a]),
        ScreenCheck::Unrecognised(SCREEN_UNKNOWN)
    );
}

#[test]
fn every_refusal_reason_is_a_fixed_word_from_the_closed_set() {
    for reason in UNRECOGNISED_REASONS {
        assert!(
            reason.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
            "{reason} must be a fixed word"
        );
    }
    assert_eq!(UNRECOGNISED_REASONS, [SCREEN_UNREADABLE, SCREEN_UNKNOWN]);
    // Every marker is lowercase, because `recognise` lowercases the capture
    // before it matches; an uppercase marker could never match.
    for m in SCREEN_MARKERS {
        assert_eq!(*m, m.to_ascii_lowercase(), "{m} must be lowercase");
    }
}

// ── Pane discovery by terminal (#3104) ──────────────────────────────────────

#[test]
fn list_panes_argv_is_one_fixed_read_only_listing() {
    assert_eq!(
        list_panes_argv(),
        ["list-panes", "-a", "-F", "#{pane_id} #{pane_tty}"]
    );
    let a = list_panes_argv();
    assert!(!a.iter().any(|x| x == "-t"), "no target");
    assert!(!a.iter().any(|x| x == "send-keys"), "never types");
}

fn found(listing: &str, tty: &str) -> Option<Pane> {
    pane_on_tty(listing.as_bytes(), Path::new(tty))
}

#[test]
fn exactly_one_pane_on_the_holders_terminal_is_found() {
    let got = found(
        "%1 /dev/pts/3\n%7 /dev/pts/7\n%9 /dev/pts/9\n",
        "/dev/pts/7",
    );
    assert_eq!(got.expect("one match").id(), "%7");
}

#[test]
fn the_trailing_newline_is_optional_but_only_one_is_stripped() {
    assert!(found("%7 /dev/pts/7", "/dev/pts/7").is_some());
    assert!(found("%7 /dev/pts/7\n", "/dev/pts/7").is_some());
    assert!(
        found("%7 /dev/pts/7\n\n", "/dev/pts/7").is_none(),
        "a second newline is an empty line, so the output is malformed"
    );
}

#[test]
fn no_match_or_empty_output_is_no_pane() {
    assert!(found("%1 /dev/pts/3\n", "/dev/pts/7").is_none());
    assert!(found("", "/dev/pts/7").is_none());
}

#[test]
fn two_different_panes_on_one_terminal_are_ambiguous() {
    assert!(found("%7 /dev/pts/7\n%8 /dev/pts/7\n", "/dev/pts/7").is_none());
}

#[test]
fn repeated_identical_lines_are_one_pane() {
    let got = found("%7 /dev/pts/7\n%7 /dev/pts/7\n", "/dev/pts/7");
    assert_eq!(got.expect("one pane listed twice").id(), "%7");
}

#[test]
fn one_id_on_two_ttys_makes_the_whole_listing_malformed() {
    assert!(
        found(
            "%7 /dev/pts/7\n%7 /dev/pts/8\n%1 /dev/pts/3\n",
            "/dev/pts/3"
        )
        .is_none()
    );
}

#[test]
fn any_malformed_line_disables_the_whole_listing() {
    let bad = [
        "%7  /dev/pts/7\n",         // two spaces
        "%7\t/dev/pts/7\n",         // tab
        "%7 /dev/pts/7 \n",         // trailing space
        " %7 /dev/pts/7\n",         // leading space
        "%7 /dev/pts/7\r\n",        // carriage return
        "%7\n",                     // no tty
        "%7 \n",                    // empty tty
        "7 /dev/pts/7\n",           // no percent
        "%x /dev/pts/7\n",          // not digits
        "%1234567890 /dev/pts/7\n", // too many digits
        "-t /dev/pts/7\n",          // option shaped
        "%7 /dev/pts/7 x\n",        // whitespace inside the tty
    ];
    for listing in bad {
        // A good line for the holder precedes the bad one, so a lenient
        // parser would find a pane.
        let all = format!("%1 /dev/pts/1\n{listing}");
        assert!(found(&all, "/dev/pts/1").is_none(), "{listing:?}");
    }
    assert!(found("%1 /dev/pts/1\n\n%2 /dev/pts/2\n", "/dev/pts/1").is_none());
}

#[test]
fn a_tty_longer_than_the_limit_is_malformed() {
    let long = format!("/dev/{}", "p".repeat(300));
    assert!(found(&format!("%7 {long}\n"), &long).is_none());
}

#[test]
fn a_listing_over_64_kib_is_refused_whole() {
    let mut l = String::from("%7 /dev/pts/7\n");
    while l.len() <= 64 * 1024 {
        l.push_str("%1 /dev/pts/1\n");
    }
    assert!(found(&l, "/dev/pts/7").is_none());
}

#[test]
fn a_holder_terminal_that_is_not_a_terminal_never_matches() {
    for tty in [
        "/dev/null",
        "pipe:[123]",
        "/dev/pts/7 (deleted)",
        "socket:[9]",
    ] {
        let listing = format!("%7 {}\n", tty.replace(' ', "_"));
        assert!(found(&listing, tty).is_none(), "{tty}");
    }
}

#[test]
fn a_tty_with_a_parent_dir_component_never_matches() {
    assert!(found("%7 /dev/../etc/x\n", "/dev/../etc/x").is_none());
}
