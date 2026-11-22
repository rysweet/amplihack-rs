//! Issue #2916 (TDD): the notice for an effort withheld by the version rule.
//!
//! The version text comes from a binary amplihack does not control, so the
//! notice prints the parsed triple when the text parses and otherwise a
//! sanitised, capped copy. Neither rendering carries a control character.

use super::notice::Notice;
use super::table::{Effort, Tier};

fn unsupported(found: Option<&str>) -> String {
    Notice::EffortUnsupported {
        harness: "claude".to_string(),
        tier: Tier::Strong,
        effort: Effort::parse("xhigh").unwrap(),
        found: found.map(str::to_owned),
    }
    .render()
}

#[test]
fn a_version_found_is_named_with_the_minimum() {
    let line = unsupported(Some("2.1.320 (Claude Code)"));
    assert!(line.contains("2.1.320"), "{line}");
    assert!(line.contains("2.1.329"), "{line}");
    assert!(line.contains("runs without `--effort xhigh`"), "{line}");
    assert!(!line.contains("--help"), "{line}");
}

#[test]
fn an_unreadable_version_is_said_to_be_unreadable() {
    let line = unsupported(None);
    assert!(line.contains("could not be read"), "{line}");
    assert!(line.contains("2.1.329"), "{line}");
    assert!(line.contains("runs without `--effort xhigh`"), "{line}");
}

#[test]
fn hostile_version_text_carries_no_control_character() {
    let line = unsupported(Some("\x1b[2J2.1.320\r\nfake line"));
    assert!(!line.chars().any(char::is_control), "{line:?}");
}

#[test]
fn long_version_text_is_capped() {
    let line = unsupported(Some(&"x".repeat(500)));
    assert!(line.len() < 500, "{} bytes", line.len());
}

#[test]
fn a_bad_effort_value_and_column_are_sanitised() {
    let line = Notice::BadEffort {
        column: "claude".to_string(),
        tier: "strong".to_string(),
        value: "\x1b[2Jturbo\r\n".to_string(),
    }
    .render();
    assert!(!line.chars().any(char::is_control), "{line:?}");
    assert!(line.contains("turbo"), "{line}");
}
