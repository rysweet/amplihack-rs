//! Issue #2539: `step-recovery init`, `record` and `report`, and the refusal
//! of unsafe recovery directories and record files.
//!
//! `report` is where a handed-off run ends: any handoff record anywhere in the
//! directory gives exit 81 (`STEP_RECOVERY_HANDOFF_EXIT`). An invalid or
//! unsafe record file gives exit 2, never a clean report.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use serde_json::{Value, json};

use crate::support::step_recovery::{Fixture, HANDOFF_EXIT, show, text};

fn record(f: &Fixture, step: &str, reason: &str) -> std::process::Output {
    f.amplihack()
        .args(["orch", "helper", "step-recovery", "record", "--dir"])
        .arg(&f.dir)
        .args([
            "--step-id",
            step,
            "--reason",
            reason,
            "--owner",
            "run-requester",
            "--needs",
            "rerun finalization",
        ])
        .output()
        .unwrap()
}

fn handoffs(o: &std::process::Output) -> Vec<Value> {
    let v: Value = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("report stdout is JSON ({e}):\n{}", show(o)));
    v["step_recovery_handoffs"]
        .as_array()
        .expect("handoff list")
        .clone()
}

#[test]
fn an_empty_directory_reports_no_handoffs_and_exits_0() {
    let f = Fixture::new();
    let o = f.report();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(handoffs(&o).is_empty());
}

#[test]
fn record_writes_a_validator_handoff_that_report_exits_81_on() {
    let f = Fixture::new();
    let o = record(&f, "autodrive-report", "no finalization evidence was found");
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let r = &f.records("autodrive-report")[0];
    assert_eq!(
        (r["choice"].as_str(), r["source"].as_str()),
        (Some("handoff"), Some("validator"))
    );
    assert!(
        r["failure"]["exit_code"].is_null(),
        "no step failed, so no exit code: {r}"
    );

    let o = f.report();
    assert_eq!(o.status.code(), Some(HANDOFF_EXIT), "{}", show(&o));
    let list = handoffs(&o);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["step_id"], "autodrive-report");
    assert_eq!(list[0]["owner"], "run-requester");
    assert_eq!(list[0]["needs"], "rerun finalization");
    assert_eq!(list[0]["source"], "validator");
    let err = text(&o.stderr);
    for field in [
        "run-requester",
        "rerun finalization",
        "no finalization evidence was found",
        "autodrive-report.jsonl",
    ] {
        assert!(err.contains(field), "stderr names {field}:\n{err}");
    }
}

#[test]
fn a_later_continue_for_the_same_step_does_not_hide_an_earlier_handoff() {
    let f = Fixture::new();
    let tool = f.tool(
        "fails",
        "#!/bin/sh\necho 'SECRET-EXCERPT-TEXT failure' >&2\nexit 1\n",
    );
    f.choose(
        1,
        json!({"choice": "handoff", "reason": "round one needs a person",
        "owner": "run-requester", "needs": "check the merge queue"}),
    );
    assert_eq!(
        f.run("step-03-finalize", &[], &[tool.to_str().unwrap()])
            .status
            .code(),
        Some(0)
    );
    // A later loop round recorded `continue_recorded` for the same step
    // (written directly here): a "last record wins" report would go clean.
    let file = f.dir.join("step-03-finalize.jsonl");
    let mut later = f.records("step-03-finalize")[0].clone();
    later["choice"] = json!("continue_recorded");
    later["reason"] = json!("later round skipped it");
    later["source"] = json!("agent");
    let obj = later.as_object_mut().unwrap();
    obj.remove("owner");
    obj.remove("needs");
    let mut body = fs::read_to_string(&file).unwrap();
    body.push_str(&format!("{later}\n"));
    fs::write(&file, body).unwrap();

    let o = f.report();
    assert_eq!(o.status.code(), Some(HANDOFF_EXIT), "{}", show(&o));
    assert_eq!(handoffs(&o)[0]["needs"], "check the merge queue");
    let all = format!("{}{}", text(&o.stdout), text(&o.stderr));
    assert!(
        !all.contains("SECRET-EXCERPT-TEXT"),
        "report never prints output_excerpt:\n{all}"
    );
}

#[test]
fn handoffs_are_listed_in_file_order_then_record_order() {
    let f = Fixture::new();
    for step in ["b-step", "a-step"] {
        assert_eq!(
            record(&f, step, &format!("{step} one")).status.code(),
            Some(0)
        );
    }
    assert_eq!(record(&f, "a-step", "a-step two").status.code(), Some(0));
    let o = f.report();
    let reasons: Vec<String> = handoffs(&o)
        .iter()
        .map(|h| h["reason"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(reasons, ["a-step one", "a-step two", "b-step one"]);
}

/// A named way to spoil the recovery directory.
type Case = (&'static str, fn(&Fixture));

#[test]
fn invalid_or_unsafe_record_files_make_report_exit_2() {
    let cases: [Case; 5] = [
        ("not JSON", |f| {
            fs::write(f.dir.join("s.jsonl"), "{not json\n").unwrap()
        }),
        ("unknown field", |f| {
            fs::write(
                f.dir.join("s.jsonl"),
                "{\"schema_version\":1,\"extra\":true}\n",
            )
            .unwrap()
        }),
        ("mode 0644", |f| {
            assert_eq!(record(f, "s", "x").status.code(), Some(0));
            fs::set_permissions(f.dir.join("s.jsonl"), fs::Permissions::from_mode(0o644)).unwrap();
        }),
        ("symlink", |f| {
            assert_eq!(record(f, "real", "x").status.code(), Some(0));
            symlink(f.dir.join("real.jsonl"), f.dir.join("s.jsonl")).unwrap();
        }),
        ("over 4 MiB", |f| {
            let line = format!("{}\n", " ".repeat(1024));
            fs::write(f.dir.join("s.jsonl"), line.repeat(4 * 1024 + 1)).unwrap();
        }),
    ];
    for (name, setup) in cases {
        let f = Fixture::new();
        setup(&f);
        if name != "mode 0644" && name != "symlink" {
            fs::set_permissions(f.dir.join("s.jsonl"), fs::Permissions::from_mode(0o600)).unwrap();
        }
        let o = f.report();
        assert_eq!(o.status.code(), Some(2), "{name}:\n{}", show(&o));
    }
}

/// Review S1: a directory with no record is removed only by the recipe whose
/// `init --created-by` made it; a caller's directory is the caller's.
#[test]
fn remove_if_created_by_removes_only_the_creators_clean_directory() {
    let f = Fixture::new();
    let sr = |args: &[&str]| {
        f.amplihack()
            .args(["orch", "helper", "step-recovery"])
            .args(args)
            .output()
            .unwrap()
    };
    let made = sr(&["init", "--created-by", "auto-drive-to-merge"]);
    assert_eq!(made.status.code(), Some(0), "{}", show(&made));
    let dir = text(&made.stdout).trim().to_string();
    assert!(std::path::Path::new(&dir).join(".created-by").is_file());
    // A nested workflow-finalize given this directory keeps it.
    let o = sr(&[
        "report",
        "--dir",
        &dir,
        "--remove-if-created-by",
        "workflow-finalize",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(std::path::Path::new(&dir).is_dir(), "{}", show(&o));
    // The creator removes it.
    let o = sr(&[
        "report",
        "--dir",
        &dir,
        "--remove-if-created-by",
        "auto-drive-to-merge",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(!std::path::Path::new(&dir).exists(), "{}", show(&o));
    // A directory init did not create has no creator note and is kept.
    let o = sr(&[
        "report",
        "--dir",
        f.dir.to_str().unwrap(),
        "--remove-if-created-by",
        "x",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(f.dir.is_dir());
}
