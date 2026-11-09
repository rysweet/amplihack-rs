//! Issue #2018: the record's location, run-id rule and format-1 schema
//! (D6, D7 and data minimisation). See `tests_support` for the contract.

#![cfg(unix)]

use super::tests_support::*;
use crate::step_record::{StepRecord, record_path, root, valid_run_id};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------
// Location and run id
// ---------------------------------------------------------------------

#[test]
fn the_root_is_under_home_dot_amplihack_runtime() {
    assert_eq!(
        root(Path::new("/home/alice")),
        PathBuf::from("/home/alice/.amplihack/runtime/recipe-steps")
    );
}

#[test]
fn the_record_path_is_keyed_by_run_id_and_launcher_pid() {
    let root = Path::new("/h/.amplihack/runtime/recipe-steps");
    assert_eq!(
        record_path(root, RUN, 41907),
        root.join(RUN).join("41907.json")
    );
}

#[test]
fn run_ids_that_qualify() {
    for id in [RUN, "a", "A-Z_a-z_0-9", "ws-17", &"x".repeat(128)] {
        assert!(valid_run_id(id), "{id:?} must be accepted");
    }
}

#[test]
fn run_ids_that_do_not_qualify() {
    let too_long = "x".repeat(129);
    for id in [
        "",
        ".",
        "..",
        "a/b",
        "../x",
        "a\0b",
        "a b",
        "a\nb",
        "run.1",
        "\u{FF21}",
        "caf\u{e9}",
        "\u{e9}",
        too_long.as_str(),
    ] {
        assert!(!valid_run_id(id), "{id:?} must be rejected");
    }
}

// ---------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------

/// Data minimisation: the serialised key set is exactly the format-1 list,
/// so no argv, prompt, token or other environment variable can slip in.
#[test]
fn the_serialised_keys_are_exactly_the_format_1_list() {
    let value = record_json(RUN, 100);
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "agent",
            "created_at",
            "cwd",
            "format",
            "launcher",
            "parent",
            "run_dir",
            "run_id",
            "tool",
            "workspace_run_id",
        ]
    );
    for who in ["launcher", "agent", "parent"] {
        let mut inner: Vec<&str> = value[who]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        inner.sort_unstable();
        assert_eq!(inner, vec!["pid", "start_ticks"], "{who}");
    }
    assert_eq!(value["format"], 1);
}

#[test]
fn absent_values_serialise_as_null_and_are_never_left_out() {
    let mut rec = record(RUN, 100);
    rec.workspace_run_id = None;
    rec.run_dir = None;
    rec.agent.start_ticks = None;
    rec.parent = None;
    let value = serde_json::to_value(&rec).unwrap();
    for key in ["workspace_run_id", "run_dir", "parent"] {
        assert!(
            value.as_object().unwrap().contains_key(key),
            "{key} missing: {value}"
        );
        assert!(value[key].is_null(), "{key}: {value}");
    }
    assert!(
        value["agent"]
            .as_object()
            .unwrap()
            .contains_key("start_ticks")
    );
    assert!(value["agent"]["start_ticks"].is_null());
}

#[test]
fn a_record_round_trips_with_and_without_a_parent() {
    let mut rec = record(RUN, 100);
    for _ in 0..2 {
        let text = serde_json::to_string(&rec).unwrap();
        let back: StepRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(back, rec);
        rec.parent = None;
    }
}
