//! Issue #1710 (D2): the caller identity that `recipe run --detach` records
//! as a run's `parent` and that `workstreams status` filters on.
//!
//! Contract: `run_identity::caller_identity(get)` takes a getter for one
//! environment variable and returns `<source>:<value>` for the first usable
//! variable in the order `AMPLIHACK_RUN_ID`, `AMPLIHACK_SESSION_ID`,
//! `AMPLIHACK_TREE_ID`, `CLAUDE_CODE_SESSION_ID`. A value is usable when it is
//! non-empty, at most 128 bytes and printable ASCII. Pure: never reads the
//! process environment itself.

use super::run_identity::{RUN_ID_ENV, caller_identity};
use std::collections::BTreeMap;

fn identity(vars: &[(&str, &str)]) -> Option<String> {
    let map: BTreeMap<String, String> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    caller_identity(|k| map.get(k).cloned())
}

#[test]
fn the_run_id_variable_is_named_amplihack_run_id() {
    assert_eq!(RUN_ID_ENV, "AMPLIHACK_RUN_ID");
}

#[test]
fn no_variable_gives_no_identity() {
    assert_eq!(identity(&[]), None);
}

#[test]
fn each_source_is_prefixed_with_its_name() {
    assert_eq!(
        identity(&[("AMPLIHACK_RUN_ID", "amp-r-1")]).as_deref(),
        Some("run:amp-r-1")
    );
    assert_eq!(
        identity(&[("AMPLIHACK_SESSION_ID", "s-1")]).as_deref(),
        Some("session:s-1")
    );
    assert_eq!(
        identity(&[("AMPLIHACK_TREE_ID", "t-1")]).as_deref(),
        Some("tree:t-1")
    );
    assert_eq!(
        identity(&[("CLAUDE_CODE_SESSION_ID", "c-1")]).as_deref(),
        Some("claude:c-1")
    );
}

#[test]
fn run_id_wins_over_every_other_source() {
    let all = [
        ("CLAUDE_CODE_SESSION_ID", "c"),
        ("AMPLIHACK_TREE_ID", "t"),
        ("AMPLIHACK_SESSION_ID", "s"),
        ("AMPLIHACK_RUN_ID", "r"),
    ];
    assert_eq!(identity(&all).as_deref(), Some("run:r"));
}

#[test]
fn the_order_is_run_then_session_then_tree_then_claude() {
    assert_eq!(
        identity(&[
            ("CLAUDE_CODE_SESSION_ID", "c"),
            ("AMPLIHACK_TREE_ID", "t"),
            ("AMPLIHACK_SESSION_ID", "s"),
        ])
        .as_deref(),
        Some("session:s")
    );
    assert_eq!(
        identity(&[("CLAUDE_CODE_SESSION_ID", "c"), ("AMPLIHACK_TREE_ID", "t")]).as_deref(),
        Some("tree:t")
    );
}

#[test]
fn an_empty_value_is_skipped() {
    assert_eq!(
        identity(&[("AMPLIHACK_RUN_ID", ""), ("AMPLIHACK_SESSION_ID", "s")]).as_deref(),
        Some("session:s")
    );
}

#[test]
fn a_value_longer_than_128_bytes_is_skipped() {
    let long = "x".repeat(129);
    let max = "y".repeat(128);
    assert_eq!(
        identity(&[("AMPLIHACK_RUN_ID", &long), ("AMPLIHACK_TREE_ID", &max)]),
        Some(format!("tree:{max}"))
    );
}

#[test]
fn a_value_that_is_not_printable_ascii_is_skipped() {
    for bad in ["a\nb", "a\x1b[31mb", "caf\u{e9}", "tab\there", "\x7f"] {
        assert_eq!(
            identity(&[("AMPLIHACK_RUN_ID", bad), ("CLAUDE_CODE_SESSION_ID", "c")]).as_deref(),
            Some("claude:c"),
            "{bad:?} must be skipped"
        );
    }
}

#[test]
fn only_unusable_values_give_no_identity() {
    assert_eq!(
        identity(&[
            ("AMPLIHACK_RUN_ID", ""),
            ("AMPLIHACK_SESSION_ID", "\u{1}"),
            ("AMPLIHACK_TREE_ID", &"z".repeat(500)),
        ]),
        None
    );
}
