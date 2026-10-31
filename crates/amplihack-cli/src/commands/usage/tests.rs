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
    assert_eq!(r.total.unwrap().total(), 210);
    assert_eq!(r.sessions[0].direct.unwrap().total(), 195);
    assert_eq!(r.sessions[0].inclusive.unwrap().total(), 210);
    assert_eq!(r.sessions[1].inclusive.unwrap().total(), 15);
    assert_eq!(r.models[0].observed_attempts, 2);
    assert_eq!(r.total.unwrap().cache_write, 30);
    assert_eq!(
        (
            r.total.unwrap().cache_write_5m,
            r.total.unwrap().cache_write_1h
        ),
        (20, 10)
    );
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
    assert!(!r.complete);
    assert!(has(&r, "cache_write_counter_unreported"));
    assert_eq!(
        r.total,
        Some(Tokens {
            input: 130,
            cache_read: 50,
            output: 35,
            reasoning: 15,
            ..Tokens::default()
        })
    );
    assert_eq!(r.total.unwrap().total(), 230);
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
    assert_eq!(r.total.unwrap().total(), 20);
    assert_eq!(r.unattributed.unwrap().total(), 20);
    assert_eq!(r.sessions[0].inclusive.unwrap().total(), 20);
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
    r.total.as_mut().unwrap().output += 10;
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
            "input_tokens":input,"cached_input_tokens":read,"cache_write_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":reasoning}}}})
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
    assert_eq!(r.total.unwrap().total(), 230);
    assert!(!r.complete);
    assert!(has(&r, "cumulative_counter_regressed"));
    assert!(has(&r, "unidentified_cumulative_remainder"));
    assert!(r.api_price_estimate.is_none());
}

#[test]
fn codex_last_only_requires_stable_identity_and_deduplicates_copies() {
    let dir = tempfile::tempdir().unwrap();
    let meta = json!({"type":"session_meta","payload":{"id":"s"}});
    let mut row = json!({"type":"event_msg","timestamp":"2026-01-01T00:00:01Z","payload":{"type":"token_count","model":"model","request_id":"call-1","info":{"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1}}}});
    std::fs::write(dir.path().join("log"), format!("{meta}\n{row}\n{row}\n")).unwrap();
    let m = manifest(json!([session("s", "codex", json!(["log", "log"]))]));
    let r = collect(&m, dir.path()).unwrap();
    assert!(r.complete);
    assert_eq!(r.total.unwrap().total(), 15);
    row["payload"].as_object_mut().unwrap().remove("request_id");
    std::fs::write(dir.path().join("log"), format!("{meta}\n{row}\n")).unwrap();
    let r = collect(&m, dir.path()).unwrap();
    assert!(r.total.is_none());
    assert_eq!(r.unresolved.len(), 2);
    assert_eq!(r.unresolved[0].tokens.total(), 15);
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
    assert_eq!(r.total.unwrap().total(), 15);
    assert_eq!(r.unattributed.unwrap().total(), 15);
    assert!(r.sessions[0].direct.is_none());
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
    assert_eq!(r.sessions[0].direct.unwrap().total(), 15);
    assert!(r.sessions[1].direct.is_none());
    assert_eq!(r.models[0].model, "copied-model");
    assert!(!r.complete); // The manifest still declares an empty original file list.
}

#[test]
fn native_response_ids_override_snapshots_and_preserve_cache_writes() {
    let m = manifest(json!([session(
        "native",
        "codex",
        json!(["codex-native.jsonl", "codex-native.jsonl"])
    )]));
    let r = collect(&m, &fixture_dir()).unwrap();
    assert_eq!(
        r.total,
        Some(Tokens {
            input: 125,
            cache_read: 50,
            cache_write: 5,
            output: 35,
            reasoning: 15,
            ..Tokens::default()
        })
    );
    assert_eq!(r.models.len(), 2);
    assert_eq!(r.models[0].tokens.total(), 130);
    assert_eq!(r.models[1].tokens.total(), 100);
    assert_eq!(r.models.iter().map(|m| m.observed_attempts).sum::<u64>(), 2);
    assert!(has(&r, "unknown_cache_write_duration"));
    assert!(!has(&r, "unidentified_cumulative_remainder"));
    assert!(!has(&r, "cumulative_counter_regressed"));
    assert!(r.models.iter().all(|m| !m.reasoning_in_output));
}

#[test]
fn identified_response_plus_missing_prefix_preserves_unknown_cumulative_remainder() {
    let dir = tempfile::tempdir().unwrap();
    let meta = json!({"type":"session_meta","payload":{"id":"s"}});
    let response = json!({"type":"token_usage_record","timestamp":"2026-01-01T00:00:03Z","payload":{"thread_id":"s","response_id":"late-response","model":"model-b","usage":{"input_tokens":80,"cached_input_tokens":30,"cache_write_input_tokens":0,"output_tokens":20,"reasoning_output_tokens":5}}});
    std::fs::write(
        dir.path().join("log"),
        format!("{meta}\n{}\n{response}\n", codex_record(180, 50, 50, 15, 2)),
    )
    .unwrap();
    let r = collect(
        &manifest(json!([session("s", "codex", json!(["log"]))])),
        dir.path(),
    )
    .unwrap();
    assert_eq!(r.total.unwrap().total(), 230);
    assert!(has(&r, "unidentified_cumulative_remainder"));
    let unknown = r.models.iter().find(|m| m.model == "unknown").unwrap();
    assert_eq!(unknown.tokens.total(), 130);
    assert_eq!(unknown.observed_attempts, 0);
    assert!(!r.complete);
}

#[test]
fn no_usage_is_null_and_missing_identity_evidence_is_non_additive() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("empty"), "{}").unwrap();
    let m = manifest(json!([session("s", "codex", json!(["empty"]))]));
    let r = collect(&m, dir.path()).unwrap();
    assert!(r.total.is_none());
    assert!(r.sessions[0].direct.is_none());
    assert_eq!(serde_json::to_value(&r).unwrap()["total"], Value::Null);
    assert!(summary(&r).contains("Total unknown"));
    let row = json!({"type":"assistant","sessionId":"s","timestamp":"2026-01-01T00:00:00Z","message":{"model":"model","usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5}}});
    std::fs::write(dir.path().join("empty"), format!("{row}\n{row}\n")).unwrap();
    let mut m = m;
    m.sessions[0].provider = Provider::Claude;
    let r = collect(&m, dir.path()).unwrap();
    assert!(r.total.is_none());
    assert_eq!(r.unresolved.len(), 2);
    assert!(r.unresolved.iter().all(|u| u.tokens.total() == 15));
    assert!(summary(&r).contains("non-additive evidence"));
}

#[test]
fn unmapped_native_responses_are_globally_deduplicated_and_unattributed() {
    let dir = tempfile::tempdir().unwrap();
    let row = json!({"type":"token_usage_record","timestamp":"2026-01-01T00:00:00Z","payload":{"thread_id":"unmapped","response_id":"global-response","model":"model","usage":{"input_tokens":10,"cached_input_tokens":2,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1}}});
    std::fs::write(dir.path().join("log"), format!("{row}\n{row}\n")).unwrap();
    let m = manifest(json!([session("s", "codex", json!(["log", "log"]))]));
    let r = collect(&m, dir.path()).unwrap();
    assert_eq!(r.total.unwrap().total(), 15);
    assert_eq!(r.unattributed.unwrap().total(), 15);
    assert!(r.sessions[0].direct.is_none());
    assert_eq!(r.models[0].observed_attempts, 1);
    assert!(!r.complete);
}

#[test]
fn global_response_ids_prevent_duplicate_history_across_session_counters() {
    let dir = tempfile::tempdir().unwrap();
    for id in ["a", "b"] {
        let row = json!({"type":"token_usage_record","timestamp":"2026-01-01T00:00:00Z","payload":{"thread_id":id,"response_id":"copied-response","model":"model","usage":{"input_tokens":10,"cached_input_tokens":2,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1},"thread_token_usage":{"input_tokens":10,"cached_input_tokens":2,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1}}});
        std::fs::write(dir.path().join(id), row.to_string()).unwrap();
    }
    // A third copy must not restore an owner after the conflict. Both manifest
    // orders must conserve tokens and leave all ownership ambiguous.
    for ids in [["a", "b", "a"], ["b", "a", "b"]] {
        let m = manifest(json!([
            session(ids[0], "codex", json!([ids[0]])),
            session(ids[1], "codex", json!([ids[1], ids[2]]))
        ]));
        let r = collect(&m, dir.path()).unwrap();
        assert_eq!(r.total.unwrap().total(), 15);
        assert_eq!(r.unattributed, r.total);
        assert!(
            r.sessions
                .iter()
                .all(|s| s.direct.is_none() && s.inclusive.is_none())
        );
        assert_eq!(r.models[0].observed_attempts, 1);
        assert!(has(&r, "conflicting_message_identity"));
        assert!(!r.complete);
    }
}

#[test]
fn delayed_cumulative_subsets_across_models_preserve_conservation() {
    let dir = tempfile::tempdir().unwrap();
    let meta = json!({"type":"session_meta","payload":{"id":"s"}});
    let context = |model: &str, second: u32| json!({"type":"turn_context","timestamp":format!("2026-01-01T00:00:{second:02}Z"),"payload":{"model":model}});
    std::fs::write(
        dir.path().join("log"),
        format!(
            "{meta}\n{}\n{}\n{}\n{}\n",
            context("a", 0),
            codex_record(100, 0, 30, 0, 1),
            context("b", 2),
            codex_record(100, 20, 30, 10, 3)
        ),
    )
    .unwrap();
    let r = collect(
        &manifest(json!([session("s", "codex", json!(["log"]))])),
        dir.path(),
    )
    .unwrap();
    assert_eq!(r.total.unwrap().total(), 130);
    assert_eq!(
        r.total.unwrap(),
        Tokens {
            input: 80,
            cache_read: 20,
            output: 20,
            reasoning: 10,
            ..Tokens::default()
        }
    );
    assert!(has(&r, "ambiguous_cumulative_model_attribution"));
    assert_eq!(r.models[0].model, "unknown");
    assert!(!r.complete);
}

fn native_usage(provider: &str, native: &str, response: &str, usage: Value) -> Value {
    if provider == "claude" {
        json!({"type":"assistant","sessionId":native,"timestamp":"2026-01-01T00:00:00Z",
            "message":{"id":response,"model":"model","usage":usage}})
    } else {
        json!({"type":"token_usage_record","timestamp":"2026-01-01T00:00:00Z",
            "payload":{"thread_id":native,"response_id":response,"model":"model","usage":usage}})
    }
}

#[test]
fn codex_model_context_cannot_cross_files_or_native_sessions() {
    for boundary in ["file", "explicit-native", "session-meta", "missing-native"] {
        let dir = tempfile::tempdir().unwrap();
        let usage = json!({"input_tokens":10,"cached_input_tokens":0,
            "cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0});
        let meta_a = json!({"type":"session_meta","payload":{"id":"a"}});
        let meta_b = json!({"type":"session_meta","payload":{"id":"b"}});
        let context = json!({"type":"turn_context","timestamp":"2026-01-01T00:00:00Z",
            "payload":{"model":"model-for-a"}});
        let mut own = native_usage("codex", "a", "own-a", usage.clone());
        own["timestamp"] = json!("2026-01-01T00:00:01Z");
        own["payload"].as_object_mut().unwrap().remove("model");
        let mut copy = native_usage("codex", "b", "copied-b", usage.clone());
        copy["timestamp"] = json!("2026-01-01T00:00:02Z");
        copy["payload"].as_object_mut().unwrap().remove("model");
        let mut valid = native_usage("codex", "b", "own-b", usage);
        valid["timestamp"] = json!("2026-01-01T00:00:03Z");
        valid["payload"]["model"] = json!("model-for-b");
        std::fs::write(dir.path().join("b"), valid.to_string()).unwrap();
        let files = match boundary {
            "file" => {
                std::fs::write(dir.path().join("a"), format!("{meta_a}\n{context}\n{own}"))
                    .unwrap();
                std::fs::write(dir.path().join("copy"), copy.to_string()).unwrap();
                json!(["a", "copy"])
            }
            "explicit-native" => {
                std::fs::write(
                    dir.path().join("a"),
                    format!("{meta_a}\n{context}\n{own}\n{copy}"),
                )
                .unwrap();
                json!(["a"])
            }
            "session-meta" => {
                copy["payload"].as_object_mut().unwrap().remove("thread_id");
                std::fs::write(
                    dir.path().join("a"),
                    format!("{meta_a}\n{context}\n{own}\n{meta_b}\n{copy}"),
                )
                .unwrap();
                json!(["a"])
            }
            "missing-native" => {
                std::fs::write(dir.path().join("a"), format!("{meta_a}\n{context}\n{own}"))
                    .unwrap();
                std::fs::write(dir.path().join("copy"), format!("{context}\n{copy}")).unwrap();
                json!(["a", "copy"])
            }
            _ => unreachable!(),
        };
        let r = collect(
            &manifest(json!([
                session("a", "codex", files),
                session("b", "codex", json!(["b"]))
            ])),
            dir.path(),
        )
        .unwrap();
        assert_eq!(r.total.unwrap().total(), 45, "{boundary}");
        assert_eq!(r.sessions[0].direct.unwrap().total(), 15, "{boundary}");
        assert_eq!(r.sessions[1].direct.unwrap().total(), 30, "{boundary}");
        assert!(!r.sessions[0].complete, "{boundary}");
        assert!(!r.sessions[1].complete, "{boundary}");
        assert!(!r.complete, "{boundary}");
        assert!(has(&r, "unknown_model"), "{boundary}");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.session == Some(1) && d.code == "incomplete_usage_observation"),
            "{boundary}"
        );
        assert_eq!(r.models.len(), 3, "{boundary}");
        for model in ["model-for-a", "model-for-b", "unknown"] {
            let bucket = r.models.iter().find(|m| m.model == model).unwrap();
            assert_eq!(bucket.tokens.total(), 15, "{boundary}: {model}");
            assert_eq!(bucket.observed_attempts, 1, "{boundary}: {model}");
        }
    }
}

#[test]
fn invalid_codex_session_metadata_clears_owner_and_model_until_recovery() {
    for payload in [
        json!({}),
        json!({"id":null}),
        json!({"id":42}),
        json!({"id":false}),
        json!({"id":[]}),
        json!({"id":{}}),
        json!({"id":""}),
        json!({"id":"PRIVATE SESSION CONTENT"}),
        json!({"id":"a".repeat(161)}),
        Value::Null,
    ] {
        for kind in ["cumulative", "response", "last-only"] {
            let dir = tempfile::tempdir().unwrap();
            let usage = |input: u64, second: u32| {
                let tokens = json!({"input_tokens":input,"cached_input_tokens":0,
                    "cache_write_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0});
                let (record_type, payload) = match kind {
                    "cumulative" => (
                        "event_msg",
                        json!({"type":"token_count",
                        "info":{"total_token_usage":tokens}}),
                    ),
                    "last-only" => (
                        "event_msg",
                        json!({"type":"token_count",
                        "request_id":format!("request-{second}"),"info":{"last_token_usage":tokens}}),
                    ),
                    _ => (
                        "token_usage_record",
                        json!({"response_id":format!("response-{second}"),
                        "usage":tokens}),
                    ),
                };
                json!({"type":record_type,"timestamp":format!("2026-01-01T00:00:{second:02}Z"),
                    "payload":payload})
            };
            let mut records = vec![
                json!({"type":"session_meta","payload":{"id":"s"}}),
                json!({"type":"turn_context","payload":{"model":"model-a"}}),
                usage(10, 1),
                json!({"type":"session_meta","payload":payload}),
                usage(50, 2),
                usage(60, 3),
            ];
            let m = manifest(json!([session("s", "codex", json!(["log"]))]));
            // Inspect the invalid boundary before any valid metadata returns.
            // Then recover the same owner without restoring the previous model.
            for recovered in [false, true] {
                if recovered {
                    records.extend([
                        json!({"type":"session_meta","payload":{"id":"s"}}),
                        usage(20, 4),
                        json!({"type":"turn_context","payload":{"model":"model-b"}}),
                        usage(30, 5),
                    ]);
                }
                std::fs::write(
                    dir.path().join("log"),
                    records
                        .iter()
                        .map(Value::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
                .unwrap();
                let r = collect(&m, dir.path()).unwrap();
                assert!(!r.complete, "{kind}: {payload}, recovered={recovered}");
                assert!(!r.sessions[0].complete);
                let owned = if !recovered {
                    10
                } else if kind == "cumulative" {
                    30
                } else {
                    60
                };
                assert_eq!(
                    r.sessions[0].direct.unwrap().input,
                    owned,
                    "{kind}: {payload}"
                );
                assert_eq!(r.sessions[0].inclusive, r.sessions[0].direct);
                assert_eq!(
                    r.sessions[0]
                        .models
                        .iter()
                        .find(|m| m.model == "model-a")
                        .unwrap()
                        .tokens
                        .input,
                    10
                );
                assert!(r.diagnostics.iter().any(|d| {
                    d.code == "missing_or_invalid_session_identity"
                        && d.session == Some(0)
                        && d.file == Some(0)
                        && d.line == Some(4)
                }));
                assert_eq!(
                    r.diagnostics
                        .iter()
                        .filter(|d| d.code == "missing_native_session_identity")
                        .count(),
                    2
                );
                assert!(has(&r, "unknown_model"));
                if kind == "cumulative" {
                    assert_eq!(r.total.unwrap().input, owned);
                    assert_eq!(r.unattributed, None);
                    assert_eq!(r.unresolved.len(), 2);
                    for (evidence, input) in r.unresolved.iter().zip([50, 60]) {
                        assert_eq!(evidence.reason, "missing_native_session_non_additive");
                        assert_eq!(evidence.model, "unknown");
                        assert_eq!(evidence.tokens.input, input);
                    }
                } else {
                    assert_eq!(r.total.unwrap().input, owned + 110);
                    assert_eq!(r.unattributed.unwrap().input, 110);
                    assert!(r.unresolved.is_empty());
                    assert_eq!(
                        r.models
                            .iter()
                            .find(|m| m.model == "unknown")
                            .unwrap()
                            .tokens
                            .input,
                        110 + if recovered { 20 } else { 0 }
                    );
                }
                if recovered {
                    for (model, input) in if kind == "cumulative" {
                        [("unknown", 10), ("model-b", 10)]
                    } else {
                        [("unknown", 20), ("model-b", 30)]
                    } {
                        assert_eq!(
                            r.sessions[0]
                                .models
                                .iter()
                                .find(|m| m.model == model)
                                .unwrap()
                                .tokens
                                .input,
                            input
                        );
                    }
                }
                let output = format!("{}{}", serde_json::to_string(&r).unwrap(), summary(&r));
                assert!(!output.contains("PRIVATE SESSION CONTENT"));
            }
        }
    }
}

#[test]
fn explicit_codex_usage_identity_survives_invalid_metadata_and_new_session_recovery() {
    for identity_field in ["thread_id", "session_id"] {
        let dir = tempfile::tempdir().unwrap();
        let tokens = |input: u64| {
            json!({"input_tokens":input,"cached_input_tokens":0,
            "cache_write_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0})
        };
        let response = |input: u64, second: u32| {
            json!({"type":"token_usage_record",
            "timestamp":format!("2026-01-01T00:00:{second:02}Z"),
            "payload":{"response_id":format!("response-{second}"),"usage":tokens(input)}})
        };
        let mut explicit = response(50, 2);
        explicit["payload"][identity_field] = json!("s");
        explicit["payload"]["thread_token_usage"] = tokens(60);
        let records = [
            json!({"type":"session_meta","payload":{"id":"s"}}),
            json!({"type":"turn_context","payload":{"model":"model-a"}}),
            response(10, 1),
            json!({"type":"session_meta","payload":{"id":null}}),
            explicit,
            json!({"type":"session_meta","payload":{"id":"b"}}),
            json!({"type":"turn_context","payload":{"model":"model-b"}}),
            response(20, 3),
        ];
        std::fs::write(
            dir.path().join("log"),
            records.map(|r| r.to_string()).join("\n"),
        )
        .unwrap();
        std::fs::write(dir.path().join("b"), "").unwrap();
        let r = collect(
            &manifest(json!([
                session("s", "codex", json!(["log"])),
                session("b", "codex", json!(["b"])),
            ])),
            dir.path(),
        )
        .unwrap();
        assert!(!r.complete);
        assert_eq!(r.total.unwrap().input, 80);
        assert_eq!(r.sessions[0].direct.unwrap().input, 60);
        assert_eq!(r.sessions[1].direct.unwrap().input, 20);
        assert!(r.sessions[1].complete);
        assert_eq!(r.unattributed, None);
        assert!(r.unresolved.is_empty());
        assert!(!has(&r, "missing_native_session_identity"));
        for (model, input) in [("model-a", 10), ("unknown", 50), ("model-b", 20)] {
            assert_eq!(
                r.models
                    .iter()
                    .find(|m| m.model == model)
                    .unwrap()
                    .tokens
                    .input,
                input,
                "{identity_field}: {model}"
            );
        }
    }
}

#[test]
fn invalid_codex_turn_context_clears_model_until_valid_context_recovers() {
    for payload in [
        json!({}),
        json!({"model":null}),
        json!({"model":42}),
        json!({"model":false}),
        json!({"model":[]}),
        json!({"model":{}}),
        json!({"model":""}),
        json!({"model":"PRIVATE MODEL CONTENT"}),
        json!({"model":"a".repeat(161)}),
    ] {
        for record_type in ["token_usage_record", "event_msg"] {
            let dir = tempfile::tempdir().unwrap();
            let usage = json!({"input_tokens":10,"cached_input_tokens":0,
                "cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0});
            let response = |id: &str, second: u32| {
                let payload = if record_type == "token_usage_record" {
                    json!({"response_id":id,"usage":usage})
                } else {
                    json!({"type":"token_count","response_id":id,
                        "info":{"last_token_usage":usage}})
                };
                json!({"type":record_type,"timestamp":format!("2026-01-01T00:00:{second:02}Z"),
                    "payload":payload})
            };
            let records = [
                json!({"type":"session_meta","payload":{"id":"s"}}),
                json!({"type":"turn_context","payload":{"model":"model-a"}}),
                response("before", 1),
                json!({"type":"turn_context","payload":payload}),
                response("after-invalid", 2),
                response("still-unknown", 3),
                json!({"type":"turn_context","payload":{"model":"model-b"}}),
                response("recovered", 4),
            ];
            std::fs::write(
                dir.path().join("log"),
                records.map(|r| r.to_string()).join("\n"),
            )
            .unwrap();
            let r = collect(
                &manifest(json!([session("s", "codex", json!(["log"]))])),
                dir.path(),
            )
            .unwrap();
            assert!(!r.complete, "{record_type}: {payload}");
            assert!(!r.sessions[0].complete);
            assert_eq!(r.total.unwrap().total(), 60);
            assert_eq!(r.sessions[0].direct, r.total);
            assert_eq!(r.models.len(), 3);
            for (model, attempts) in [("model-a", 1), ("unknown", 2), ("model-b", 1)] {
                let bucket = r.models.iter().find(|m| m.model == model).unwrap();
                assert_eq!(bucket.tokens.total(), 15 * attempts, "{model}");
                assert_eq!(bucket.observed_attempts, attempts, "{model}");
            }
            assert!(has(&r, "unknown_model"));
            assert!(r.diagnostics.iter().any(|d| {
                d.code == "missing_or_invalid_model"
                    && d.session == Some(0)
                    && d.file == Some(0)
                    && d.line == Some(4)
            }));
            let output = format!("{}{}", serde_json::to_string(&r).unwrap(), summary(&r));
            assert!(!output.contains("PRIVATE MODEL CONTENT"));
        }
    }
}

#[test]
fn conflicting_response_models_remain_unknown_for_every_copy_order() {
    for provider in ["claude", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        let usage = if provider == "claude" {
            json!({"input_tokens":10,"cache_read_input_tokens":0,
                "cache_creation_input_tokens":0,"output_tokens":5})
        } else {
            json!({"input_tokens":10,"cached_input_tokens":0,
                "cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0})
        };
        for model in ["a", "b", "missing"] {
            let mut row = native_usage(provider, "s", "one-response", usage.clone());
            let object = if provider == "claude" {
                row["message"].as_object_mut().unwrap()
            } else {
                row["payload"].as_object_mut().unwrap()
            };
            if model == "missing" {
                object.remove("model");
            } else {
                object.insert("model".into(), json!(model));
            }
            std::fs::write(dir.path().join(model), row.to_string()).unwrap();
        }
        for order in [
            ["a", "b", "missing"],
            ["a", "missing", "b"],
            ["b", "a", "missing"],
            ["b", "missing", "a"],
            ["missing", "a", "b"],
            ["missing", "b", "a"],
        ] {
            // Repeat every copy after conflict to exercise permanent ambiguity.
            let files: Vec<_> = order.into_iter().cycle().take(6).collect();
            let r = collect(
                &manifest(json!([session("s", provider, json!(files))])),
                dir.path(),
            )
            .unwrap();
            assert!(!r.complete, "{provider}: {order:?}");
            assert!(!r.sessions[0].complete);
            assert!(has(&r, "conflicting_message_identity"));
            assert_eq!(r.total.unwrap().total(), 15);
            assert_eq!(r.sessions[0].direct.unwrap().total(), 15);
            assert_eq!(r.models.len(), 1);
            assert_eq!(r.models[0].model, "unknown", "{provider}: {order:?}");
            assert_eq!(r.models[0].tokens, r.total.unwrap());
            assert_eq!(r.models[0].observed_attempts, 1);
        }
        // Missing metadata alone must still allow a reliable copy to fill it.
        for order in [["a", "missing"], ["missing", "a"]] {
            let r = collect(
                &manifest(json!([session("s", provider, json!(order))])),
                dir.path(),
            )
            .unwrap();
            assert!(!has(&r, "conflicting_message_identity"));
            assert_eq!(r.models.len(), 1);
            assert_eq!(r.models[0].model, "a");
            assert_eq!(r.models[0].tokens.total(), 15);
            assert_eq!(r.models[0].observed_attempts, 1);
        }
    }
}

#[test]
fn copied_partial_usage_marks_accounting_owner_and_ancestors_incomplete() {
    for provider in ["claude", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        let usage = if provider == "claude" {
            json!({"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0})
        } else {
            json!({"input_tokens":10,"output_tokens":5,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0})
        };
        for id in ["root", "owner", "holder"] {
            std::fs::write(
                dir.path().join(id),
                native_usage(provider, id, id, usage.clone()).to_string(),
            )
            .unwrap();
        }
        let mut partial = usage;
        partial.as_object_mut().unwrap().remove("output_tokens");
        let copy = native_usage(provider, "owner", "partial-copy", partial);
        std::fs::write(dir.path().join("copy"), copy.to_string()).unwrap();
        let mut owner = session("owner", provider, json!(["owner"]));
        owner["parent"] = json!("root");
        let holder = session("holder", provider, json!(["holder", "copy"]));
        for entries in [
            json!([
                session("root", provider, json!(["root"])),
                owner.clone(),
                holder.clone()
            ]),
            json!([holder, owner, session("root", provider, json!(["root"]))]),
        ] {
            let m = manifest(entries);
            let r = collect(&m, dir.path()).unwrap();
            let owner = r.sessions.iter().find(|s| s.id == "owner").unwrap();
            assert_eq!(owner.direct.unwrap().total(), 25);
            assert!(!owner.complete, "{provider}");
            let root = r.sessions.iter().find(|s| s.id == "root").unwrap();
            assert_eq!(root.inclusive.unwrap().total(), 40);
            assert!(!root.complete, "{provider}");
            assert!(!r.complete);
            let source = m.sessions.iter().position(|s| s.id == "holder").unwrap();
            assert!(
                r.diagnostics
                    .iter()
                    .any(|d| d.code == "missing_or_invalid_token_field"
                        && d.session == Some(source)
                        && d.file == Some(1)
                        && d.line == Some(1))
            );
            let owner = m.sessions.iter().position(|s| s.id == "owner").unwrap();
            assert!(
                r.diagnostics
                    .iter()
                    .any(|d| d.code == "incomplete_usage_observation"
                        && d.session == Some(owner)
                        && d.file.is_none()
                        && d.line.is_none())
            );
        }
    }
}

#[test]
fn copied_missing_usage_marks_native_owner_incomplete_without_inventing_totals() {
    for provider in ["claude", "codex"] {
        for absent in [false, true] {
            for observed in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let usage = if provider == "claude" {
                    json!({"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0})
                } else {
                    json!({"input_tokens":10,"output_tokens":5,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0})
                };
                for id in ["root", "owner", "holder"] {
                    let contents = if observed {
                        native_usage(provider, id, id, usage.clone()).to_string()
                    } else {
                        String::new()
                    };
                    std::fs::write(dir.path().join(id), contents).unwrap();
                }
                let mut copy = native_usage(provider, "owner", "missing-copy", Value::Null);
                if absent {
                    let payload = if provider == "claude" {
                        &mut copy["message"]
                    } else {
                        &mut copy["payload"]
                    };
                    payload.as_object_mut().unwrap().remove("usage");
                }
                std::fs::write(dir.path().join("copy"), copy.to_string()).unwrap();
                let mut owner = session("owner", provider, json!(["owner"]));
                owner["parent"] = json!("root");
                let entries = vec![
                    session("root", provider, json!(["root"])),
                    owner,
                    session("holder", provider, json!(["holder", "copy"])),
                ];
                for entries in [entries.clone(), entries.into_iter().rev().collect()] {
                    let m = manifest(json!(entries));
                    let r = collect(&m, dir.path()).unwrap();
                    assert!(
                        !r.complete,
                        "{provider}, absent={absent}, observed={observed}"
                    );
                    assert_eq!(r.total.map(Tokens::total), observed.then_some(45));
                    for s in &r.sessions {
                        assert!(!s.complete, "{provider}: {} with absent={absent}", s.id);
                        assert_eq!(s.direct.map(Tokens::total), observed.then_some(15));
                        let inclusive = if s.id == "root" { 30 } else { 15 };
                        assert_eq!(
                            s.inclusive.map(Tokens::total),
                            observed.then_some(inclusive)
                        );
                    }
                    let holder = m.sessions.iter().position(|s| s.id == "holder").unwrap();
                    assert!(r.diagnostics.iter().any(|d| d.code == "missing_usage"
                        && d.session == Some(holder)
                        && d.file == Some(1)
                        && d.line == Some(1)));
                    let owner = m.sessions.iter().position(|s| s.id == "owner").unwrap();
                    assert!(
                        r.diagnostics
                            .iter()
                            .any(|d| d.code == "incomplete_usage_observation"
                                && d.session == Some(owner)
                                && d.file.is_none()
                                && d.line.is_none())
                    );
                    if !observed {
                        assert!(r.models.is_empty());
                        let json = serde_json::to_value(&r).unwrap();
                        assert!(json["total"].is_null());
                        for s in json["sessions"].as_array().unwrap() {
                            assert!(s["direct"].is_null());
                            assert!(s["inclusive"].is_null());
                        }
                        assert!(summary(&r).contains("Total unknown"));
                    }
                }
            }
        }
    }
}

#[test]
fn empty_or_invalid_counters_are_unknown_but_observed_zero_is_numeric() {
    for provider in ["claude", "codex"] {
        for (usage, observed) in [
            (json!({}), false),
            (
                json!({"input_tokens":"invalid","output_tokens":null}),
                false,
            ),
            (json!({"unrecognized_tokens":0}), false),
            (json!({"input_tokens":0}), true),
            (json!({"output_tokens":0}), true),
            (
                json!({"cache_creation_input_tokens":0,"cached_input_tokens":0}),
                true,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let row = native_usage(provider, "s", "response", usage);
            std::fs::write(dir.path().join("log"), row.to_string()).unwrap();
            let r = collect(
                &manifest(json!([session("s", provider, json!(["log"]))])),
                dir.path(),
            )
            .unwrap();
            let expected = observed.then_some(Tokens::default());
            assert_eq!(r.total, expected, "{provider}: {row}");
            assert_eq!(r.sessions[0].direct, expected);
            assert_eq!(r.sessions[0].inclusive, expected);
            assert_eq!(r.models.len(), usize::from(observed));
            assert!(!r.complete);
            if !observed {
                let json = serde_json::to_value(&r).unwrap();
                assert!(json["total"].is_null());
                assert!(json["sessions"][0]["direct"].is_null());
                assert!(summary(&r).contains("Total unknown"));
            }
        }
    }
}

#[test]
fn empty_codex_cumulative_snapshots_do_not_establish_zero_totals() {
    for usage in [json!({}), json!({"input_tokens":0})] {
        for native in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let row = if native {
                json!({"type":"token_usage_record","timestamp":"2026-01-01T00:00:01Z",
                    "payload":{"thread_id":"s","model":"model","thread_token_usage":usage}})
            } else {
                json!({"type":"event_msg","timestamp":"2026-01-01T00:00:01Z",
                    "payload":{"type":"token_count","model":"model","info":{"total_token_usage":usage}}})
            };
            let meta = json!({"type":"session_meta","payload":{"id":"s"}});
            std::fs::write(dir.path().join("log"), format!("{meta}\n{row}\n")).unwrap();
            let r = collect(
                &manifest(json!([session("s", "codex", json!(["log"]))])),
                dir.path(),
            )
            .unwrap();
            let expected = usage.get("input_tokens").map(|_| Tokens::default());
            assert_eq!(r.total, expected);
            assert_eq!(r.sessions[0].direct, expected);
            assert!(!r.complete);
        }
    }
}

#[test]
fn partial_copy_without_native_identity_follows_later_identified_owner() {
    let dir = tempfile::tempdir().unwrap();
    let mut partial = native_usage("claude", "owner", "response", json!({"input_tokens":10}));
    partial.as_object_mut().unwrap().remove("sessionId");
    let full = native_usage(
        "claude",
        "owner",
        "response",
        json!({"input_tokens":10,
        "output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}),
    );
    std::fs::write(dir.path().join("partial"), partial.to_string()).unwrap();
    std::fs::write(dir.path().join("full"), full.to_string()).unwrap();
    let r = collect(
        &manifest(json!([
            session("holder", "claude", json!(["partial"])),
            session("owner", "claude", json!(["full"]))
        ])),
        dir.path(),
    )
    .unwrap();
    assert_eq!(r.sessions[1].direct.unwrap().total(), 15);
    assert!(!r.sessions[1].complete);
    assert_eq!(r.unattributed, None);
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "incomplete_usage_observation" && d.session == Some(1))
    );
}

#[test]
fn conflicting_known_and_unmapped_native_owners_remain_unattributed() {
    let dir = tempfile::tempdir().unwrap();
    for native in ["owner", "outside"] {
        let row = native_usage(
            "claude",
            native,
            "response",
            json!({"input_tokens":10,
            "output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}),
        );
        std::fs::write(dir.path().join(native), row.to_string()).unwrap();
    }
    for files in [
        json!(["owner", "outside", "owner"]),
        json!(["outside", "owner", "outside"]),
    ] {
        let r = collect(
            &manifest(json!([session("owner", "claude", files)])),
            dir.path(),
        )
        .unwrap();
        assert_eq!(r.total.unwrap().total(), 15);
        assert_eq!(r.unattributed, r.total);
        assert_eq!(r.sessions[0].direct, None);
        assert_eq!(r.models[0].observed_attempts, 1);
        assert!(has(&r, "conflicting_message_identity"));
    }
}

#[test]
fn empty_copies_still_contribute_identity_evidence_without_zero_totals() {
    for provider in ["claude", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        let full = native_usage(
            provider,
            "a",
            "response",
            json!({"input_tokens":10,
            "output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,
            "cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}),
        );
        let mut empty = native_usage(provider, "b", "response", json!({}));
        if provider == "codex" {
            empty["payload"]["thread_token_usage"] = full["payload"]["usage"].clone();
        }
        std::fs::write(dir.path().join("full"), full.to_string()).unwrap();
        std::fs::write(dir.path().join("empty"), empty.to_string()).unwrap();
        for entries in [
            json!([
                session("a", provider, json!(["full"])),
                session("b", provider, json!(["empty"]))
            ]),
            json!([
                session("b", provider, json!(["empty"])),
                session("a", provider, json!(["full"]))
            ]),
        ] {
            let r = collect(&manifest(entries), dir.path()).unwrap();
            assert_eq!(r.total.unwrap().total(), 15);
            assert_eq!(r.unattributed, r.total);
            assert!(r.sessions.iter().all(|s| s.direct.is_none()));
            assert_eq!(r.models[0].observed_attempts, 1);
            assert!(has(&r, "conflicting_message_identity"));
        }
    }
}

#[test]
fn aggregation_diagnostics_follow_copied_usage_to_its_owner() {
    for provider in ["claude", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        let usage = json!({"input_tokens":5,"output_tokens":0,
            "cache_creation_input_tokens":0,"cache_read_input_tokens":0,
            "cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0});
        for id in ["owner", "holder"] {
            std::fs::write(
                dir.path().join(id),
                native_usage(provider, id, id, usage.clone()).to_string(),
            )
            .unwrap();
        }
        let (copies, code, total) = if provider == "claude" {
            let first = native_usage(
                provider,
                "owner",
                "copied",
                json!({"input_tokens":0,"output_tokens":0,
                "cache_creation_input_tokens":5,"cache_read_input_tokens":0,
                "cache_creation":{"ephemeral_5m_input_tokens":5,"ephemeral_1h_input_tokens":0}}),
            );
            let second = native_usage(
                provider,
                "owner",
                "copied",
                json!({"input_tokens":0,"output_tokens":0,
                "cache_creation_input_tokens":5,"cache_read_input_tokens":0,
                "cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":5}}),
            );
            (
                format!("{first}\n{second}\n"),
                "conflicting_cache_partitions",
                10,
            )
        } else {
            let meta = json!({"type":"session_meta","payload":{"id":"owner"}});
            let snapshot = |input, second| {
                json!({"type":"event_msg",
                "timestamp":format!("2026-01-01T00:00:{second:02}Z"),"payload":{"type":"token_count","model":"model",
                "info":{"total_token_usage":{"input_tokens":input,"output_tokens":0,
                    "cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}}})
            };
            (
                format!("{meta}\n{}\n{}\n", snapshot(10, 1), snapshot(9, 2)),
                "cumulative_counter_regressed",
                10,
            )
        };
        std::fs::write(dir.path().join("copies"), copies).unwrap();
        let r = collect(
            &manifest(json!([
                session("owner", provider, json!(["owner"])),
                session("holder", provider, json!(["holder", "copies"]))
            ])),
            dir.path(),
        )
        .unwrap();
        assert!(has(&r, code), "{provider}");
        assert_eq!(r.sessions[0].direct.unwrap().total(), total);
        assert!(!r.sessions[0].complete, "{provider}");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == code && d.session == Some(1))
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "incomplete_usage_observation" && d.session == Some(0))
        );
    }
}
