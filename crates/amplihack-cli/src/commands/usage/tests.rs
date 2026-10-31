use super::models::*;
use super::*;
use crate::commands::pr::{GhOutput, GhRunner};
use serde_json::{Value, json};
use std::{cell::RefCell, path::PathBuf};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/commands/usage/fixtures")
}
fn manifest(sessions: Value) -> Manifest {
    serde_json::from_value(
        json!({"version":1,"workstream":"issue-1810","repository":"example/repo",
        "branch":"feat/usage","prs":[42],"completed":true,"sessions":sessions}),
    )
    .unwrap()
}
fn session(id: &str, provider: &str, files: Value) -> Value {
    json!({"id":id,"provider":provider,"run_id":"recipe-run-1","complete":true,"files":files})
}
fn has(r: &Report, code: &str) -> bool {
    r.diagnostics.iter().any(|d| d.code == code)
}

#[test]
fn streamed_copied_parent_child_history_and_resumes_count_once() {
    let mut child = session("child", "claude", json!(["claude-child.jsonl"]));
    child["parent"] = json!("parent");
    let m = manifest(json!([
        session(
            "parent",
            "claude",
            json!(["claude-parent.jsonl", "claude-parent.jsonl"])
        ),
        child
    ]));
    let r = collect(&m, &fixture_dir()).unwrap();
    assert!(r.complete);
    assert_eq!(r.total.total(), 210);
    assert_eq!(r.sessions[0].direct.total(), 195);
    assert_eq!(r.sessions[0].inclusive.total(), 210);
    assert_eq!(r.sessions[1].inclusive.total(), 15);
    assert_eq!(r.models[0].observed_attempts, 2);
    assert_eq!(r.total.cache_write, 30);
    assert_eq!((r.total.cache_write_5m, r.total.cache_write_1h), (20, 10));
    assert_eq!(r.api_price_estimate, None);
    assert_eq!(r.price_reference, None);
    assert_eq!(
        r.first_observed.as_deref(),
        Some("2026-01-01T00:00:01+00:00")
    );
}

#[test]
fn codex_cumulative_last_repetition_resume_and_model_changes() {
    let m = manifest(json!([session(
        "codex-session",
        "codex",
        json!([
            "codex-resume.jsonl",
            "codex-start.jsonl",
            "codex-start.jsonl"
        ])
    )]));
    let r = collect(&m, &fixture_dir()).unwrap();
    assert!(r.complete, "{:?}", r.diagnostics);
    assert_eq!(
        r.total,
        Tokens {
            input: 130,
            cache_read: 50,
            output: 35,
            reasoning: 15,
            ..Tokens::default()
        }
    );
    assert_eq!(r.total.total(), 230);
    assert_eq!(r.models.len(), 2);
    assert_eq!(r.models[0].tokens.total(), 130);
    assert_eq!(r.models[1].tokens.total(), 100);
    assert_eq!(r.models.iter().map(|m| m.observed_attempts).sum::<u64>(), 2);
}

#[test]
fn partial_corrupt_missing_logs_are_private_read_only_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("private-host-path.jsonl");
    let content = "{\"type\":\"assistant\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"id\":\"partial\",\"content\":\"SECRET_TOOL_ARGS\",\"usage\":{\"input_tokens\":7}}}\n{PRIVATE_CORRUPT_SENTINEL";
    std::fs::write(&log, content).unwrap();
    let m = manifest(json!([session(
        "partial",
        "claude",
        json!(["private-host-path.jsonl", "missing-private-host-path"])
    )]));
    let r = collect(&m, dir.path()).unwrap();
    assert!(!r.complete);
    for c in [
        "invalid_json_record",
        "unreadable_file",
        "missing_or_invalid_token_field",
        "unknown_model",
    ] {
        assert!(has(&r, c), "{c}");
    }
    let output = format!("{}{}", serde_json::to_string(&r).unwrap(), summary(&r));
    for private in [
        "private-host-path",
        "SECRET_TOOL_ARGS",
        "PRIVATE_CORRUPT_SENTINEL",
        "content",
        "usage\":",
    ] {
        assert!(!output.contains(private), "{private}");
    }
    assert_eq!(std::fs::read_to_string(log).unwrap(), content);
}

#[test]
fn retries_are_separate_and_unattributed_work_remains_visible() {
    let dir = tempfile::tempdir().unwrap();
    for (id, message) in [("first", "attempt-a"), ("retry", "attempt-b")] {
        std::fs::write(dir.path().join(id), json!({"type":"assistant","sessionId":id,"timestamp":"2026-01-01T00:00:00Z",
            "message":{"id":message,"model":"new-unpriced-model","usage":{"input_tokens":7,"output_tokens":3,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}).to_string()).unwrap();
    }
    let mut retry = session("retry", "claude", json!(["retry"]));
    retry["parent"] = json!("first");
    retry["run_id"] = json!("recipe-retry");
    let mut m = manifest(json!([session("first", "claude", json!(["first"])), retry]));
    m.workstream = None;
    let r = collect(&m, dir.path()).unwrap();
    assert_eq!(r.total.total(), 20);
    assert_eq!(r.unattributed.total(), 20);
    assert_eq!(r.sessions[0].inclusive.total(), 20);
    assert!(has(&r, "unattributed_workstream_identity"));
}

#[test]
fn ancestry_cycles_duplicate_identities_and_unsafe_labels_rejected() {
    let mut s = session("s", "claude", json!([]));
    s["parent"] = json!("s");
    assert!(collect(&manifest(json!([s])), &fixture_dir()).is_err());
    assert!(
        collect(
            &manifest(json!([
                session("s", "claude", json!([])),
                session("s", "codex", json!([]))
            ])),
            &fixture_dir()
        )
        .is_err()
    );
    let mut m = manifest(json!([session("s", "codex", json!([]))]));
    m.workstream = Some("secret\n<!-- injected -->".into());
    assert!(collect(&m, &fixture_dir()).is_err());
}

#[derive(Default)]
struct Hosting {
    comments: RefCell<Vec<Value>>,
    methods: RefCell<Vec<String>>,
    draft: bool,
}
impl GhRunner for Hosting {
    fn run_gh(&self, args: &[&str]) -> anyhow::Result<GhOutput> {
        let value = if args[0] == "pr" {
            json!({"headRefName":"feat/usage","isDraft":self.draft,"state":"OPEN"})
        } else if args[1] == "user" {
            json!({"id":7})
        } else if let Some(index) = args.iter().position(|s| *s == "--method") {
            let method = args[index + 1];
            self.methods.borrow_mut().push(method.to_owned());
            let field = args.iter().find(|s| s.starts_with("body=@")).unwrap();
            let body = std::fs::read_to_string(field.trim_start_matches("body=@")).unwrap();
            let comment = json!({"id":101,"user":{"id":7},"body":body});
            if method == "POST" {
                self.comments.borrow_mut().push(comment);
            } else {
                self.comments.borrow_mut()[0] = comment;
            }
            json!({})
        } else {
            json!([self.comments.borrow().clone()])
        };
        Ok(GhOutput {
            stdout: value.to_string(),
            stderr: String::new(),
            success: true,
        })
    }
}

#[test]
fn stable_comment_create_noop_late_update_and_shared_pr_total() {
    let mut m = manifest(json!([session(
        "parent",
        "claude",
        json!(["claude-parent.jsonl"])
    )]));
    m.prs.push(43);
    let mut r = collect(&m, &fixture_dir()).unwrap();
    assert_eq!(r.allocation, "shared_non_additive");
    assert!(summary(&r).contains("Shared non-additive"));
    let host = Hosting::default();
    assert_eq!(publish::publish(&m, &r, 42, &host).unwrap(), "created");
    assert_eq!(publish::publish(&m, &r, 42, &host).unwrap(), "unchanged");
    r.total.output += 10;
    assert_eq!(publish::publish(&m, &r, 42, &host).unwrap(), "updated");
    assert_eq!(host.comments.borrow().len(), 1);
    assert_eq!(*host.methods.borrow(), vec!["POST", "PATCH"]);
    assert_eq!(publish::publish(&m, &r, 43, &host).unwrap(), "created");
    assert_eq!(host.comments.borrow().len(), 2);
}

#[test]
fn publication_requires_explicit_completion_ready_pr_and_owned_marker() {
    let mut m = manifest(json!([session(
        "parent",
        "claude",
        json!(["claude-parent.jsonl"])
    )]));
    let r = collect(&m, &fixture_dir()).unwrap();
    m.completed = false;
    let host = Hosting::default();
    assert!(publish::publish(&m, &r, 42, &host).is_err());
    assert!(host.methods.borrow().is_empty());
    m.completed = true;
    assert!(
        publish::publish(
            &m,
            &r,
            42,
            &Hosting {
                draft: true,
                ..Hosting::default()
            }
        )
        .is_err()
    );
    publish::publish(&m, &r, 42, &host).unwrap();
    host.comments.borrow_mut()[0]["user"]["id"] = json!(8);
    assert!(publish::publish(&m, &r, 42, &host).is_err());
    assert_eq!(host.methods.borrow().len(), 1);
}

#[test]
fn clap_registers_read_only_and_publish_commands() {
    use clap::Parser;
    assert!(
        crate::Cli::try_parse_from([
            "amplihack",
            "usage",
            "report",
            "--manifest",
            "usage.json",
            "--format",
            "json"
        ])
        .is_ok()
    );
    assert!(
        crate::Cli::try_parse_from([
            "amplihack",
            "usage",
            "publish",
            "--manifest",
            "usage.json",
            "--pr",
            "42"
        ])
        .is_ok()
    );
}

fn codex_record(input: u64, read: u64, output: u64, reasoning: u64, second: u32) -> Value {
    json!({"type":"event_msg","timestamp":format!("2026-01-01T00:00:{second:02}Z"),
        "payload":{"type":"token_count","info":{"total_token_usage":{
            "input_tokens":input,"cached_input_tokens":read,"output_tokens":output,"reasoning_output_tokens":reasoning}}}})
}

#[test]
fn codex_regressions_and_last_only_mixture_never_double_count() {
    let dir = tempfile::tempdir().unwrap();
    let rows = [
        json!({"type":"session_meta","payload":{"id":"s"}}),
        json!({"type":"turn_context","timestamp":"2026-01-01T00:00:00Z","payload":{"model":"unsupported-price-model"}}),
        codex_record(100, 20, 30, 10, 1),
        codex_record(20, 5, 10, 3, 2),
        codex_record(180, 50, 50, 15, 3),
        json!({"type":"event_msg","timestamp":"2026-01-01T00:00:04Z","payload":{"type":"token_count","request_id":"repeat-last","info":{"last_token_usage":{"input_tokens":80,"cached_input_tokens":30,"output_tokens":20,"reasoning_output_tokens":5}}}}),
    ];
    std::fs::write(
        dir.path().join("log"),
        rows.iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let r = collect(
        &manifest(json!([session("s", "codex", json!(["log"]))])),
        dir.path(),
    )
    .unwrap();
    assert_eq!(r.total.total(), 230);
    assert!(!r.complete);
    assert!(has(&r, "cumulative_counter_regressed"));
    assert!(has(&r, "mixed_cumulative_and_last_only_usage"));
    assert!(r.api_price_estimate.is_none());
}

#[test]
fn codex_last_only_requires_stable_identity_and_deduplicates_copies() {
    let dir = tempfile::tempdir().unwrap();
    let meta = json!({"type":"session_meta","payload":{"id":"s"}});
    let mut row = json!({"type":"event_msg","timestamp":"2026-01-01T00:00:01Z","payload":{"type":"token_count","model":"model","request_id":"call-1","info":{"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5,"reasoning_output_tokens":1}}}});
    std::fs::write(dir.path().join("log"), format!("{meta}\n{row}\n{row}\n")).unwrap();
    let m = manifest(json!([session("s", "codex", json!(["log", "log"]))]));
    let r = collect(&m, dir.path()).unwrap();
    assert!(r.complete);
    assert_eq!(r.total.total(), 15);
    row["payload"].as_object_mut().unwrap().remove("request_id");
    std::fs::write(dir.path().join("log"), format!("{meta}\n{row}\n")).unwrap();
    let r = collect(&m, dir.path()).unwrap();
    assert_eq!(r.total.total(), 0);
    assert!(!r.complete);
    assert!(has(&r, "usage_without_stable_identity"));
}

#[test]
fn manifest_coverage_missing_parent_and_native_mapping_stay_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let log = json!({"type":"assistant","sessionId":"not-in-manifest","timestamp":"2026-01-01T00:00:01Z",
        "message":{"id":"message","model":"model","usage":{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":5}}});
    std::fs::write(dir.path().join("log"), log.to_string()).unwrap();
    let mut s = session("s", "claude", json!(["log"]));
    s["complete"] = json!(false);
    s["parent"] = json!("missing");
    let r = collect(&manifest(json!([s])), dir.path()).unwrap();
    assert_eq!(r.total.total(), 0);
    for code in [
        "session_coverage_unconfirmed",
        "missing_parent",
        "unmapped_native_session",
        "no_usage_observed",
    ] {
        assert!(has(&r, code));
    }
    assert!(!r.complete);
}

#[test]
fn publication_refuses_ambiguous_duplicate_markers() {
    let m = manifest(json!([session(
        "parent",
        "claude",
        json!(["claude-parent.jsonl"])
    )]));
    let r = collect(&m, &fixture_dir()).unwrap();
    let host = Hosting::default();
    publish::publish(&m, &r, 42, &host).unwrap();
    let duplicate = host.comments.borrow()[0].clone();
    host.comments.borrow_mut().push(duplicate);
    assert!(publish::publish(&m, &r, 42, &host).is_err());
    assert_eq!(host.methods.borrow().len(), 1);
}

#[test]
fn foreign_codex_metadata_keeps_model_and_usage_with_native_owner() {
    let dir = tempfile::tempdir().unwrap();
    let meta = json!({"type":"session_meta","payload":{"id":"original"}});
    let context = json!({"type":"turn_context","timestamp":"2026-01-01T00:00:00Z","payload":{"model":"copied-model"}});
    std::fs::write(
        dir.path().join("copy"),
        format!("{meta}\n{context}\n{}\n", codex_record(10, 2, 5, 1, 1)),
    )
    .unwrap();
    let m = manifest(json!([
        session("original", "codex", json!([])),
        session("copy-holder", "codex", json!(["copy"]))
    ]));
    let r = collect(&m, dir.path()).unwrap();
    assert_eq!(r.sessions[0].direct.total(), 15);
    assert_eq!(r.sessions[1].direct.total(), 0);
    assert_eq!(r.models[0].model, "copied-model");
    assert!(!r.complete); // The manifest still declares an empty original file list.
}
