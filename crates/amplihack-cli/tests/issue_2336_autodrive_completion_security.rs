//! Decision/writer/dispatch integrity tests, nested under the #2336 module.
//! Each rejection case first prepares and writes a valid control decision;
//! accepting a missing helper is never treated as proof of fail-closed behavior.

use super::fixture::{Drive, assert_complete, decision_file};
use crate::support::auto_drive_light::{private_file, show};
use serde_json::{Value, json};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

fn prepared() -> (Drive, Value) {
    let d = Drive::new();
    d.record(1);
    let p = d.prepare(1, "");
    d.decide(&p, "complete");
    (d, p)
}

fn rejected(d: &Drive, p: &Value, why: &str) {
    let o = d.dispatch(p, 0);
    assert!(!o.status.success(), "{why}: {}", show(&o));
    assert!(d.evaluators().is_empty(), "{why}: no evaluator fallback");
    assert!(!d.marked(), "{why}: no phase authority");
}

#[test]
fn writer_binds_private_atomic_decision_to_prepared_run_and_actual_blob_hash() {
    let (d, p) = prepared();
    let path = decision_file(&p);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        v.as_object().unwrap().len(),
        6,
        "v1 has exactly six typed fields"
    );
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["run_id"], p["run_id"]);
    assert_eq!(v["round_label"], "round-1");
    assert_eq!(v["action"], "complete");
    let hash = d.git(
        &d.state,
        &[
            "hash-object",
            "--no-filters",
            d.p("crusty-round-1.json").to_str().unwrap(),
        ],
    );
    assert_eq!(v["round_record_hash"], hash);
    for file in [&path, Path::new(p["private_state"].as_str().unwrap())] {
        assert_private(file);
        assert_private(file.parent().unwrap());
    }
    assert_ne!(
        path.parent(),
        Path::new(p["private_state"].as_str().unwrap()).parent(),
        "trusted state must be outside agent output directory"
    );
    let original = std::fs::read(&path).unwrap();
    let o = d.write(&p, "evaluate", "Cannot replace an already written decision");
    assert!(
        !o.status.success(),
        "atomic create must refuse overwrite: {}",
        show(&o)
    );
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert_complete(&d, &d.dispatch(&p, 0));
}

#[test]
fn dispatch_rejects_malformed_duplicate_unknown_trailing_and_oversized_decisions() {
    for case in [
        "missing",
        "bad-json",
        "duplicate",
        "missing-field",
        "wrong-type",
        "unknown",
        "trailing",
        "oversized",
        "version",
        "action",
        "reason",
        "control",
        "run",
        "round",
        "hash",
    ] {
        let (d, p) = prepared();
        let path = decision_file(&p);
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let text = match case {
            "missing" => {
                std::fs::remove_file(path).unwrap();
                rejected(&d, &p, case);
                continue;
            }
            "bad-json" => "{".to_string(),
            "duplicate" => format!(
                "{{\"action\":\"evaluate\",{}",
                v.to_string().trim_start_matches('{')
            ),
            "missing-field" => {
                v.as_object_mut().unwrap().remove("round_record_hash");
                v.to_string()
            }
            "wrong-type" => {
                v["action"] = json!(true);
                v.to_string()
            }
            "unknown" => {
                v["executable"] = json!("/bin/true");
                v.to_string()
            }
            "trailing" => format!("{v}\n{{}}"),
            "oversized" => format!("{}{}", " ".repeat(16 * 1024), v),
            "version" => {
                v["schema_version"] = json!(2);
                v.to_string()
            }
            "action" => {
                v["action"] = json!("COMPLETE");
                v.to_string()
            }
            "reason" => {
                v["reason"] = json!("x".repeat(1025));
                v.to_string()
            }
            "control" => {
                v["reason"] = json!("reason\u{1b}[2J");
                v.to_string()
            }
            "run" => {
                v["run_id"] = json!("another-run");
                v.to_string()
            }
            "round" => {
                v["round_label"] = json!("round-2");
                v.to_string()
            }
            "hash" => {
                v["round_record_hash"] = json!("0".repeat(40));
                v.to_string()
            }
            _ => unreachable!(),
        };
        private_file(&path, &text);
        rejected(&d, &p, case);
    }
}

#[test]
fn writer_rejects_invalid_actions_and_reason_boundaries_without_creating_output() {
    for (action, reason) in [
        ("DONE", "a".into()),
        ("complete", "a".repeat(1025)),
        ("evaluate", "escape\u{1b}in reason".into()),
        ("evaluate", "line\nbreak".into()),
    ] {
        let d = Drive::new();
        d.record(1);
        let p = d.prepare(1, "");
        let o = d.write(&p, action, &reason);
        assert!(!o.status.success(), "invalid writer input: {}", show(&o));
        assert!(
            !decision_file(&p).exists(),
            "invalid input must not create a partial decision"
        );
    }
    let d = Drive::new();
    d.record(1);
    let p = d.prepare(1, "");
    let o = d.write(&p, "complete", &"x".repeat(1024));
    assert!(
        o.status.success(),
        "exact reason boundary is allowed: {}",
        show(&o)
    );
    assert_complete(&d, &d.dispatch(&p, 0));
}

#[test]
fn cross_run_replay_is_rejected_even_when_round_and_record_hash_match() {
    let (d, first) = prepared();
    let next = d.prepare(1, "");
    assert_ne!(
        first["run_id"], next["run_id"],
        "each preparation needs a fresh unpredictable identity"
    );
    assert_ne!(
        first["decision_file"], next["decision_file"],
        "per-run output cannot alias"
    );
    let replay = std::fs::read_to_string(decision_file(&first)).unwrap();
    private_file(&decision_file(&next), &replay);
    rejected(&d, &next, "cross-run replay");
}

#[test]
fn record_copy_manifest_sidecars_and_head_are_independently_validated_on_both_actions() {
    for action in ["complete", "evaluate"] {
        for change in [
            "record", "latest", "manifest", "minor", "severity", "findings", "head",
        ] {
            let d = Drive::new();
            d.record(1);
            let p = d.prepare(1, "");
            d.decide(&p, action);
            match change {
                "record" => private_file(&d.p("crusty-round-1.json"), "{}\n"),
                "latest" => private_file(&d.p("crusty-latest.json"), "{}\n"),
                "manifest" => private_file(
                    &d.p("crusty-records.tsv"),
                    "round-1\tcrusty-round-1.json\t0000000000000000000000000000000000000000\n",
                ),
                "minor" => private_file(&d.p("crusty-round-1.json.minor"), "[]\n"),
                "severity" => {
                    private_file(&d.p("crusty-round-1.json.severity"), "docs-typo\tmajor\n")
                }
                "findings" => private_file(&d.p("crusty-round-1.json.findings"), "hidden-major\n"),
                "head" => {
                    d.git(&d.repo, &["commit", "--allow-empty", "-qm", "changed head"]);
                }
                _ => unreachable!(),
            }
            rejected(&d, &p, &format!("{action}: altered {change}"));
        }
    }
}

#[test]
fn dispatch_and_writer_refuse_unsafe_file_types_and_permissions_without_following_links() {
    for case in [
        "decision-symlink",
        "decision-directory",
        "loose-file",
        "loose-parent",
        "parent-symlink",
        "state-symlink",
    ] {
        let (d, p) = prepared();
        let path = decision_file(&p);
        let target = d.tmp.path().join("untouched");
        private_file(&target, "do not overwrite\n");
        match case {
            "decision-symlink" => {
                std::fs::remove_file(&path).unwrap();
                symlink(&target, &path).unwrap();
            }
            "decision-directory" => {
                std::fs::remove_file(&path).unwrap();
                std::fs::create_dir(&path).unwrap();
            }
            "loose-file" => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap()
            }
            "loose-parent" => std::fs::set_permissions(
                path.parent().unwrap(),
                std::fs::Permissions::from_mode(0o777),
            )
            .unwrap(),
            "parent-symlink" => {
                let parent = path.parent().unwrap();
                let moved = parent.with_extension("moved");
                std::fs::rename(parent, &moved).unwrap();
                symlink(moved, parent).unwrap();
            }
            "state-symlink" => {
                let state = Path::new(p["private_state"].as_str().unwrap());
                let moved = state.with_extension("moved");
                std::fs::rename(state, &moved).unwrap();
                symlink(moved, state).unwrap();
            }
            _ => unreachable!(),
        }
        rejected(&d, &p, case);
        let o = d.write(&p, "complete", "must refuse unsafe existing destination");
        assert!(
            !o.status.success(),
            "writer must refuse {case}: {}",
            show(&o)
        );
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            "do not overwrite\n"
        );
    }
}

#[test]
fn executable_arguments_environment_and_depth_are_not_evaluator_context_fields() {
    for field in [
        "executable",
        "args",
        "env",
        "max_depth",
        "AMPLIHACK_MAX_DEPTH",
    ] {
        let d = Drive::new();
        d.record(1);
        let p = d.prepare(1, "");
        d.decide(&p, "evaluate");
        // Mutation of an input context after prepare must not mutate the
        // trusted captured context. Untrusted fields never become commands.
        let context = d.p("evaluator-context.json");
        let mut v: Value =
            serde_json::from_str(&std::fs::read_to_string(&context).unwrap()).unwrap();
        v[field] = json!("touch SHOULD_NOT_EXIST");
        private_file(&context, &v.to_string());
        let o = d.dispatch(&p, 0);
        // Either immutable captured context is used, or tampering is rejected.
        // A STUCK evaluator is expected; arbitrary launch data is never used.
        assert!(
            !o.status.success(),
            "unresolved record does not complete: {}",
            show(&o)
        );
        assert!(!d.review.join("SHOULD_NOT_EXIST").exists());
        for call in d.evaluators() {
            assert_eq!(call["ceiling"], "6");
            assert!(
                !call.to_string().contains("SHOULD_NOT_EXIST"),
                "untrusted launch fields must not be forwarded"
            );
        }
        // Prepare itself must reject injected launch fields as well.
        let o = d.helper(
            "prepare",
            &[
                "--state-dir".as_ref(),
                d.state.as_os_str(),
                "--round-label".as_ref(),
                "round-1".as_ref(),
                "--record".as_ref(),
                d.p("crusty-round-1.json").as_os_str(),
                "--repo".as_ref(),
                d.repo.as_os_str(),
                "--review-dir".as_ref(),
                d.review.as_os_str(),
                "--launch-tools".as_ref(),
                crate::support::auto_drive_light::tools_dir().as_os_str(),
                "--evaluator-context-file".as_ref(),
                context.as_os_str(),
            ],
        );
        assert!(
            !o.status.success(),
            "prepare must deny unknown executable context {field}: {}",
            show(&o)
        );
    }
}

pub fn assert_private(p: &Path) {
    let m = std::fs::symlink_metadata(p).unwrap();
    assert!(!m.file_type().is_symlink());
    assert_eq!(
        m.permissions().mode() & 0o077,
        0,
        "{} must be private",
        p.display()
    );
}

#[test]
fn a_decision_cannot_be_dispatched_twice() {
    let (d, p) = prepared();
    assert_complete(&d, &d.dispatch(&p, 0));
    rejected(&d, &p, "same-run replay");
}
