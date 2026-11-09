//! Issue #2018 (part 2): `amplihack recipe steps` over planted records:
//! states, filtering, both output formats and sanitising (D8, D10). Exit
//! codes and usage errors are in `issue_2018_steps_exit`.
//!
//! Records are planted by hand under the fixture's temporary `HOME` with
//! `support::step_2018_plant`. Linux only (`/proc` start times).

#![cfg(target_os = "linux")]

use crate::support::proc_watch::alive;
use crate::support::step_2018::{Fx, states, text};
use crate::support::step_2018_plant::{
    DEAD_A, DEAD_B, DEAD_C, me, plant, plant_every_state, planted,
};
use serde_json::json;

const HEADER: [&str; 10] = [
    "STATE", "RUN ID", "TOOL", "LAUNCHER", "AGENT", "PARENT", "CREATED", "CWD", "RECORD", "REASON",
];

/// Splits a table line into its cells: columns are separated by two or
/// more spaces, and no planted value here holds two spaces in a row.
fn cells(line: &str) -> Vec<&str> {
    line.split("  ")
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect()
}

// ---------------------------------------------------------------------
// States, filtering and output formats
// ---------------------------------------------------------------------

#[test]
fn json_output_reports_every_state_in_order_with_four_keys() {
    let fx = Fx::new();
    let planted = plant_every_state(&fx);
    let listed = fx.steps_json(&[]);
    assert_eq!(
        states(&listed),
        ["live", "orphan", "orphan", "stale", "unreadable"],
        "{listed:#?}"
    );
    for (entry, path) in listed.iter().zip(&planted.paths) {
        let mut keys: Vec<&str> = entry
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["path", "reason", "record", "state"], "{entry}");
        assert_eq!(entry["path"], json!(path.to_str().unwrap()));
    }
    for entry in &listed[..4] {
        assert!(entry["reason"].is_null(), "{entry}");
        assert_eq!(entry["record"]["format"], json!(1), "{entry}");
    }
    assert_eq!(listed[1]["record"]["tool"], json!("copilot"));
    assert_eq!(
        listed[2]["record"]["agent"]["pid"],
        json!(planted.sleeper.0.id())
    );
    assert!(listed[4]["record"].is_null());
    assert_eq!(listed[4]["reason"], json!("unsupported format 2"));
}

/// `--orphans` keeps orphans and every unreadable entry.
#[test]
fn orphans_keeps_orphans_and_unreadable_entries() {
    let fx = Fx::new();
    let _planted = plant_every_state(&fx);
    let listed = fx.steps_json(&["--orphans"]);
    assert_eq!(
        states(&listed),
        ["orphan", "orphan", "unreadable"],
        "{listed:#?}"
    );
    assert_eq!(listed[0]["record"]["run_id"], json!("run-b"));
    assert_eq!(listed[1]["record"]["run_id"], json!("run-c"));
}

#[test]
fn table_output_has_the_documented_columns_and_one_row_per_entry() {
    let fx = Fx::new();
    let planted = plant_every_state(&fx);
    let out = fx.steps(&[]);
    assert!(out.status.success(), "stderr:\n{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 6, "a header and five rows:\n{stdout}");
    assert_eq!(cells(lines[0]), HEADER, "header: {:?}", lines[0]);

    let runs = ["run-a", "run-b", "run-c", "run-d", "run-d"];
    let states = ["live", "orphan", "orphan", "stale", "unreadable"];
    for (i, row) in lines[1..].iter().enumerate() {
        let c = cells(row);
        assert_eq!(c.len(), HEADER.len(), "row {row:?}");
        assert_eq!((c[0], c[1]), (states[i], runs[i]), "row {row:?}");
        assert_eq!(
            c[8],
            planted.paths[i].to_str().unwrap(),
            "RECORD in {row:?}"
        );
    }
    let live = cells(lines[1]);
    let me = me().to_string();
    let me = me.as_str();
    let want = ["claude", me, me, me, "2026-11-07T10:15:03Z", "/tmp/project"];
    assert_eq!(&live[2..8], want, "row {:?}", lines[1]);
    assert_eq!(live[9], "-", "REASON is filled for unreadable entries only");
    let bad = cells(lines[5]);
    assert_eq!(&bad[2..8], ["-"; 6], "missing values are shown as -");
    assert_eq!(bad[9], "unsupported format 2");
}

#[test]
fn listing_neither_signals_nor_removes_anything() {
    let fx = Fx::new();
    let planted = plant_every_state(&fx);
    for args in [&[][..], &["--orphans"][..], &["--format", "json"][..]] {
        assert!(fx.steps(args).status.success(), "{args:?}");
    }
    for path in &planted.paths {
        assert!(path.is_file(), "{} was removed", path.display());
    }
    assert!(
        alive(planted.sleeper.0.id() as i32),
        "the orphan's agent was signalled"
    );
}

/// D10: a record whose `run_id` or `launcher.pid` disagrees with its path.
#[test]
fn a_record_that_does_not_match_its_path_is_unreadable() {
    let fx = Fx::new();
    plant(
        &fx,
        "run-x",
        DEAD_A,
        &planted("run-y", "claude", DEAD_A, DEAD_A, DEAD_A),
    );
    plant(
        &fx,
        "run-x",
        DEAD_B,
        &planted("run-x", "claude", DEAD_C, DEAD_C, DEAD_C),
    );
    let listed = fx.steps_json(&[]);
    assert_eq!(states(&listed), ["unreadable", "unreadable"], "{listed:#?}");
    for entry in &listed {
        assert_eq!(entry["reason"], json!("record does not match its path"));
    }
}

/// Table cells are sanitised: a planted `cwd` cannot set the terminal
/// title, forge a row or reorder the line. JSON keeps the value exactly.
#[test]
fn control_and_bidi_characters_in_a_record_are_neutralised_in_the_table() {
    let fx = Fx::new();
    let mut body = planted("run-esc", "clau\u{202E}de", me(), me(), me());
    body["cwd"] = json!("/tmp/\u{1b}]0;x\u{7}evil\nforged row");
    plant(&fx, "run-esc", me(), &body);

    let out = fx.steps(&[]);
    assert!(out.status.success(), "stderr:\n{}", text(&out.stderr));
    for byte in [0x1b_u8, 0x07] {
        assert!(
            !out.stdout.contains(&byte),
            "raw byte {byte:#04x} reached stdout"
        );
    }
    let stdout = text(&out.stdout);
    assert!(!stdout.contains('\u{202E}'), "bidi override reached stdout");
    assert_eq!(
        stdout.lines().count(),
        2,
        "a newline must not forge a row:\n{stdout}"
    );
    assert!(stdout.contains("/tmp/?]0;x?evil?forged row"), "{stdout}");
    assert!(stdout.contains("clau?de"), "{stdout}");

    let listed = fx.steps_json(&[]);
    assert_eq!(
        listed[0]["record"]["cwd"], body["cwd"],
        "JSON keeps the value"
    );
}
