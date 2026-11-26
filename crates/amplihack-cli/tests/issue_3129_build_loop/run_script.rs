//! A2, A3 and A4: `autodrive_build_run.sh loop` and `report`.
//!
//! `loop` runs from a copy of the bundle's tools folder in which the shared
//! driver, `autodrive_goal_loop.sh`, is a stand-in; the stand-in's exit code
//! and what it leaves in the state folder come from `STUB_*` variables.
//! `report` runs against the stand-in gh. Assertions are on exit codes, stdout
//! JSON fields and files, never on message wording.

#![cfg(target_os = "linux")]

use super::tools::{World, text, tools_dir};
use crate::support::exec_stub::write_executable;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::Output;

/// The shared driver's stand-in. It records its arguments and its task file,
/// then acts as `STUB_*` says: leaves a turn record (`STUB_TURN=1`) or a
/// `loop-end.json` (`STUB_END`) in its `--state-dir`, and exits `STUB_RC`.
const DRIVER: &str = r##"#!/usr/bin/env bash
printf '%s\n' "$@" > "$STUB_ARGS_LOG"
state=""; prev=""
for a in "$@"; do
  case "$prev" in
    --state-dir) state="$a" ;;
    --task-file) printf '%s\n' "$a" > "$STUB_SCRATCH_LOG" ;;
  esac
  prev="$a"
done
if [ "${STUB_TURN:-0}" = 1 ]; then
  mkdir -p "$state/turns/0001"; echo '{}' > "$state/turns/0001/reading.json"
fi
if [ -n "${STUB_END:-}" ]; then printf '%s\n' "$STUB_END" > "$state/loop-end.json"; fi
exit "${STUB_RC:-0}"
"##;

struct Setup {
    w: World,
    state: PathBuf,
    tools: PathBuf,
}

fn setup() -> Setup {
    let w = World::new();
    let tools = w.base.join("bundle/tools");
    fs::create_dir_all(&tools).unwrap();
    for e in fs::read_dir(tools_dir()).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            fs::copy(e.path(), tools.join(e.file_name())).unwrap();
        }
    }
    write_executable(&tools.join("autodrive_goal_loop.sh"), DRIVER, 0o755);
    let state = w.base.join("state");
    fs::create_dir_all(&state).unwrap();
    Setup { w, state, tools }
}

fn run_loop(s: &Setup, envs: &[(&str, &str)]) -> Output {
    let mut c =
        s.w.tool_at(&s.tools.join("autodrive_build_run.sh"), &s.w.repo);
    c.args(["loop", s.state.to_str().unwrap()])
        .env("RECIPE_VAR_repo_path", &s.w.repo)
        .env("RECIPE_VAR_task_description", "Build it.")
        .env("STUB_ARGS_LOG", s.w.base.join("args.log"))
        .env("STUB_SCRATCH_LOG", s.w.base.join("scratch.log"));
    for (k, v) in envs {
        c.env(k, v);
    }
    c.output().unwrap()
}

/// The loop calls the shared driver with the build goal and its own state
/// folder, from the launch checkout.
#[test]
fn the_loop_runs_the_shared_driver_with_the_build_goal() {
    let s = setup();
    let out = run_loop(&s, &[("STUB_END", r#"{"end":"ready"}"#)]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let args: Vec<String> = fs::read_to_string(s.w.base.join("args.log"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let after = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(after("--goal-tool"), "autodrive_build_goal.sh", "{args:?}");
    assert_eq!(
        after("--goal-file"),
        "skills/default-workflow/goal-build-pr.md",
        "{args:?}"
    );
    assert_eq!(after("--state-dir"), s.state.to_str().unwrap(), "{args:?}");
    assert_eq!(after("--repo"), s.w.repo.to_str().unwrap(), "{args:?}");
}

fn stdout_json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON line ({e}): {}", text(out)))
}

#[test]
fn a_loop_run_that_ends_normally_prints_its_end_line_and_exits_zero() {
    let s = setup();
    let out = run_loop(
        &s,
        &[
            ("STUB_TURN", "1"),
            ("STUB_END", r#"{"end":"ready","turn":1}"#),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(stdout_json(&out)["end"], "ready");
}

#[test]
fn a_loop_run_that_fails_after_a_turn_exits_zero_and_names_the_failure() {
    let s = setup();
    let out = run_loop(&s, &[("STUB_TURN", "1"), ("STUB_RC", "1")]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let v = stdout_json(&out);
    assert!(v["end"].is_null(), "{v}");
    assert_eq!(v["exit"], 1, "{v}");
    assert_eq!(v["turn"], 1, "{v}");
    assert!(v["error"].as_str().is_some_and(|e| !e.is_empty()), "{v}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("WARNING"),
        "the failure is visible on stderr: {}",
        text(&out)
    );
}

#[test]
fn a_loop_run_that_fails_before_any_turn_passes_its_exit_up() {
    let s = setup();
    let out = run_loop(&s, &[("STUB_RC", "3")]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    assert!(out.stdout.is_empty(), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("ERROR"),
        "{}",
        text(&out)
    );
}

#[test]
fn a_usage_limit_exit_is_passed_up_whether_or_not_a_turn_ran() {
    for code in ["75", "79"] {
        for turn in ["0", "1"] {
            let s = setup();
            let out = run_loop(&s, &[("STUB_RC", code), ("STUB_TURN", turn)]);
            assert_eq!(
                out.status.code(),
                Some(code.parse().unwrap()),
                "rc {code} turn {turn}: {}",
                text(&out)
            );
        }
    }
}

#[test]
fn a_loop_run_that_ends_before_turn_one_with_loop_end_counts_as_started() {
    let s = setup();
    let out = run_loop(
        &s,
        &[
            ("STUB_RC", "1"),
            ("STUB_END", r#"{"end":"handoff","turn":0}"#),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(stdout_json(&out)["exit"], 1, "{}", text(&out));
}

#[test]
fn a_symlinked_turn_record_does_not_count_as_a_started_loop() {
    let s = setup();
    let elsewhere = s.w.base.join("elsewhere.json");
    fs::write(&elsewhere, "{}").unwrap();
    fs::create_dir_all(s.state.join("turns/0001")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, s.state.join("turns/0001/reading.json")).unwrap();
    let out = run_loop(&s, &[("STUB_RC", "1")]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(out.stdout.is_empty(), "{}", text(&out));
}

#[test]
fn the_fallback_error_is_one_capped_line() {
    let s = setup();
    let out = run_loop(&s, &[("STUB_TURN", "1"), ("STUB_RC", "1")]);
    let v = stdout_json(&out);
    let e = v["error"].as_str().unwrap();
    assert!(e.len() <= 300 && !e.contains('\n'), "{e}");
}

#[test]
fn the_loop_leaves_no_scratch_folder_inside_or_outside_the_state_folder() {
    let s = setup();
    let out = run_loop(
        &s,
        &[("STUB_TURN", "1"), ("STUB_END", r#"{"end":"ready"}"#)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let scratch = fs::read_to_string(s.w.base.join("scratch.log")).unwrap();
    let scratch = PathBuf::from(scratch.trim());
    assert!(
        scratch.starts_with(&s.state),
        "the scratch folder is inside the state folder: {scratch:?}"
    );
    assert!(!scratch.exists(), "and it is removed: {scratch:?}");
    let strays: Vec<_> = fs::read_dir(&s.w.base)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("tmp."))
        .collect();
    assert!(strays.is_empty(), "outside the state folder: {strays:?}");
    let inside: Vec<_> = fs::read_dir(&s.state)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".build-loop."))
        .collect();
    assert!(inside.is_empty(), "inside it: {inside:?}");
}

/// A `mktemp` in front of the real one, logging each path it makes.
const MKTEMP: &str = r##"#!/usr/bin/env bash
for real in /usr/bin/mktemp /bin/mktemp; do [ -x "$real" ] && break; done
p="$("$real" "$@")" || exit $?
printf '%s\n' "$p" >> "$MKTEMP_LOG"
printf '%s\n' "$p"
"##;

/// S-D3: the loop makes every temporary file, the task text's included,
/// inside the state folder. The files are removed on return wherever they are
/// made, so the log of what `mktemp` made is the evidence.
#[test]
fn the_loop_makes_its_temporary_files_inside_the_state_folder() {
    let s = setup();
    write_executable(&s.w.bin.join("mktemp"), MKTEMP, 0o755);
    let log = s.w.base.join("mktemp.log");
    let out = run_loop(
        &s,
        &[
            ("STUB_END", r#"{"end":"ready"}"#),
            ("MKTEMP_LOG", log.to_str().unwrap()),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let made = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !made.trim().is_empty(),
        "mktemp was never called: {}",
        text(&out)
    );
    for p in made.lines() {
        assert!(
            PathBuf::from(p).starts_with(&s.state),
            "made outside the state folder: {p}\n{made}"
        );
    }
}

// ---- report: outcomes ---------------------------------------------------

fn report(s: &Setup) -> Value {
    let out =
        s.w.tool("autodrive_build_run.sh", &s.w.repo)
            .args(["report", s.state.to_str().unwrap()])
            .output()
            .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    stdout_json(&out)
}

/// Writes one kind of unreadable ledger into a fresh state folder.
type MakeLedger = Box<dyn Fn(&Setup)>;

fn ledger(s: &Setup, v: &Value) {
    fs::write(s.state.join("outcomes.json"), v.to_string()).unwrap();
}

#[test]
fn the_report_lists_each_outcome_by_name_and_status() {
    let s = setup();
    let none = report(&s);
    assert_eq!(none["outcomes"], json!([]), "no ledger is an empty list");
    ledger(
        &s,
        &json!({"outcomes": [
            {"name": "C1a", "status": "met", "evidence": "pr_ready.rs:38"},
            {"name": "G1", "status": "rejected", "reason": "out of scope"},
            {"name": "C2", "status": "unmet"}
        ]}),
    );
    let v = report(&s);
    assert_eq!(
        v["outcomes"],
        json!([
            {"name": "C1a", "status": "met"},
            {"name": "G1", "status": "rejected"},
            {"name": "C2", "status": "unmet"}
        ]),
        "{v}"
    );
    let dump = v.to_string();
    assert!(
        !dump.contains("out of scope") && !dump.contains("pr_ready.rs"),
        "reason and evidence stay out of the report: {dump}"
    );
}

#[test]
fn an_unreadable_outcome_ledger_reports_null_and_one_error() {
    let cases: Vec<(&str, MakeLedger)> = vec![
        (
            "not json",
            Box::new(|s| fs::write(s.state.join("outcomes.json"), "{nope").unwrap()),
        ),
        (
            "no outcomes array",
            Box::new(|s| ledger(s, &json!({"other": []}))),
        ),
        (
            "empty name",
            Box::new(|s| ledger(s, &json!({"outcomes": [{"name": "", "status": "met"}]}))),
        ),
        (
            "bad status",
            Box::new(|s| ledger(s, &json!({"outcomes": [{"name": "C1", "status": "done"}]}))),
        ),
        (
            "oversized",
            Box::new(|s| ledger(s, &json!({"outcomes": [], "pad": "x".repeat(600_000)}))),
        ),
        (
            "symlink",
            Box::new(|s| {
                let real = s.w.base.join("real.json");
                fs::write(&real, r#"{"outcomes":[]}"#).unwrap();
                std::os::unix::fs::symlink(&real, s.state.join("outcomes.json")).unwrap();
            }),
        ),
    ];
    for (label, make) in cases {
        let s = setup();
        let before = report(&s);
        make(&s);
        let v = report(&s);
        assert!(v["outcomes"].is_null(), "{label}: {v}");
        let before_errs = before["errors"].as_array().unwrap().len();
        assert_eq!(
            v["errors"].as_array().unwrap().len(),
            before_errs + 1,
            "{label}: one more error: {v}"
        );
        assert_eq!(
            v["terminal_state"], before["terminal_state"],
            "{label}: outcomes never change the terminal state"
        );
        assert_eq!(v["terminal_success"], before["terminal_success"], "{label}");
    }
}

#[test]
fn control_characters_in_an_outcome_name_never_reach_the_report() {
    let s = setup();
    let long = "n".repeat(300);
    ledger(
        &s,
        &json!({"outcomes": [
            {"name": "a\u{1b}[31mb\nc\u{2028}d\u{2029}e", "status": "met"},
            {"name": long, "status": "unmet"}
        ]}),
    );
    let v = report(&s);
    let first = v["outcomes"][0]["name"].as_str().unwrap();
    assert!(
        first
            .chars()
            .all(|c| !c.is_control() && c != '\u{2028}' && c != '\u{2029}'),
        "{first:?}"
    );
    assert_eq!(
        v["outcomes"][1]["name"].as_str().unwrap().chars().count(),
        200
    );
}

// ---- report: terminal state from the live pull request ---------------------

/// A loop that ended `ready` reports the pull request as GitHub reads it: open
/// and ready for review is IMPLEMENTED_VERIFIED, open as a draft is
/// EXISTING_OPEN_PR. `isDraft: false` is a value, not a missing one.
#[test]
fn a_ready_build_reports_the_draft_flag_it_read() {
    for (draft, state) in [(false, "IMPLEMENTED_VERIFIED"), (true, "EXISTING_OPEN_PR")] {
        let s = setup();
        fs::write(
            s.state.join("loop-end.json"),
            json!({"end": "ready", "turn": 4}).to_string(),
        )
        .unwrap();
        fs::write(
            s.state.join("pr.json"),
            json!({"schema_version": 1, "number": 42, "url": "https://github.com/o/r/pull/42"})
                .to_string(),
        )
        .unwrap();
        s.w.pr_view(&json!({
            "number": 42, "url": "https://github.com/o/r/pull/42", "state": "OPEN",
            "isDraft": draft, "isCrossRepository": false
        }));
        let v = report(&s);
        assert_eq!(v["pr_draft"], json!(draft), "{v}");
        assert_eq!(v["terminal_state"], json!(state), "{v}");
        assert_eq!(v["terminal_success"], json!("true"), "{v}");
    }
}
