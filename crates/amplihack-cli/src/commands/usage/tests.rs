use super::models::*;
use super::*;
use crate::commands::pr::{GhOutput, GhRunner};
use serde_json::{Value, json};
use std::{cell::RefCell, path::PathBuf};

// Response counts and context per request. The tests read report fields
// through JSON, so a renamed or missing field fails an assertion that names
// it, rather than failing the build.
mod context_and_response_counts {
    use super::*;

    fn usage(input: Value) -> Value {
        json!({"input_tokens":input,"output_tokens":9,
            "cache_read_input_tokens":20,"cache_creation_input_tokens":10,
            "cache_creation":{"ephemeral_5m_input_tokens":4,"ephemeral_1h_input_tokens":6}})
    }

    fn codex_usage(input: u64, read: u64, write: u64) -> Value {
        json!({"input_tokens":input,"cached_input_tokens":read,
            "cache_write_input_tokens":write,"output_tokens":50,"reasoning_output_tokens":8})
    }

    fn write(dir: &std::path::Path, name: &str, rows: &[Value]) {
        std::fs::write(
            dir.join(name),
            rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
    }

    fn report(provider: &str, rows: &[Value]) -> Report {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "log", rows);
        collect(
            &manifest(json!([session("s", provider, json!(["log"]))])),
            dir.path(),
        )
        .unwrap()
    }

    fn public(r: &Report) -> Value {
        serde_json::to_value(r).unwrap()
    }

    fn context(r: &Value, index: usize) -> &Value {
        let c = &r["sessions"][index]["context_per_request"];
        for field in [
            "input_tokens",
            "observed_attempts",
            "tokens_per_observed_attempt",
            "unit",
            "attempt_basis",
            "unknown_reasons",
        ] {
            assert!(
                c.get(field).is_some(),
                "context_per_request.{field} must be present: {c}"
            );
        }
        assert_eq!(c["unit"], "tokens per observed attempt");
        let reasons = c["unknown_reasons"].as_array().unwrap();
        let mut sorted = reasons.clone();
        sorted.sort_by_key(Value::to_string);
        sorted.dedup();
        assert_eq!(*reasons, sorted, "reason codes must be sorted and unique");
        c
    }

    fn unknown(c: &Value, reason: &str) {
        assert!(c["tokens_per_observed_attempt"].is_null(), "{c}");
        assert!(
            c["unknown_reasons"]
                .as_array()
                .unwrap()
                .contains(&json!(reason)),
            "missing {reason}: {c}"
        );
    }

    #[test]
    fn direct_context_deduplicates_streams_copies_and_excludes_children_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let a = native_usage("claude", "s", "a", usage(json!(100)));
        let mut streamed = a.clone();
        streamed["message"]["usage"]["output_tokens"] = json!(999);
        let mut b = native_usage("claude", "s", "b", usage(json!(50)));
        b["message"]["usage"]["cache_read_input_tokens"] = json!(0);
        b["message"]["usage"]["cache_creation_input_tokens"] = json!(0);
        b["message"]["usage"]["cache_creation"] =
            json!({"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0});
        let child = native_usage("claude", "child", "c", usage(json!(1000)));
        write(dir.path(), "start", &[a.clone(), streamed]);
        write(dir.path(), "resume", &[a, b]);
        write(dir.path(), "child", &[child]);
        let mut child_session = session("child", "claude", json!(["child"]));
        child_session["parent"] = json!("s");
        let r = collect(
            &manifest(json!([
                session("s", "claude", json!(["start", "resume"])),
                child_session
            ])),
            dir.path(),
        )
        .unwrap();
        let v = public(&r);
        let c = context(&v, 0);
        assert_eq!(c["input_tokens"], 180);
        assert_eq!(c["observed_attempts"], 2);
        assert_eq!(c["tokens_per_observed_attempt"], 90.0);
        assert_eq!(c["attempt_basis"], "identifier_backed_responses");
        assert_eq!(c["unknown_reasons"], json!([]));
        assert_eq!(v["identifier_backed_response_count"], 3);
        assert_eq!(v["sessions"][0]["identifier_backed_response_count"], 2);
        assert!(r.sessions[0].inclusive.unwrap().total() > r.sessions[0].direct.unwrap().total());
    }

    #[test]
    fn explicit_zero_is_numeric_while_missing_and_invalid_input_remain_unknown() {
        for (value, reason) in [
            (json!(0), None),
            (Value::Null, Some("missing_input_fields")),
            (json!(-1), Some("invalid_input_fields")),
            (json!("0"), Some("invalid_input_fields")),
            (json!(1.5), Some("invalid_input_fields")),
            (json!(true), Some("invalid_input_fields")),
        ] {
            let mut u = usage(value.clone());
            u["cache_read_input_tokens"] = json!(0);
            u["cache_creation_input_tokens"] = json!(0);
            u["cache_creation"] =
                json!({"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0});
            if value.is_null() {
                u.as_object_mut().unwrap().remove("input_tokens");
            }
            let r = report("claude", &[native_usage("claude", "s", "a", u)]);
            let v = public(&r);
            let c = context(&v, 0);
            assert_eq!(c["observed_attempts"], 1, "{value}");
            assert_eq!(r.total.unwrap().output, 9, "retain known totals: {value}");
            if let Some(reason) = reason {
                assert!(c["input_tokens"].is_null(), "{value}: {c}");
                unknown(c, reason);
            } else {
                assert_eq!(c["input_tokens"], 0);
                assert_eq!(c["tokens_per_observed_attempt"], 0.0);
                assert_eq!(c["unknown_reasons"], json!([]));
            }
        }
    }

    #[test]
    fn each_missing_input_bucket_taints_context_even_with_other_known_counters() {
        for field in [
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ] {
            let mut u = usage(json!(100));
            u.as_object_mut().unwrap().remove(field);
            let r = public(&report("claude", &[native_usage("claude", "s", "a", u)]));
            let c = context(&r, 0);
            assert!(c["input_tokens"].is_null(), "missing {field}");
            unknown(c, "missing_input_fields");
        }
    }

    #[test]
    fn one_unknown_contributor_taints_context_without_discarding_known_totals() {
        let rows = [
            native_usage("claude", "s", "known", usage(json!(100))),
            native_usage("claude", "s", "unknown", json!({"output_tokens":5})),
        ];
        let r = report("claude", &rows);
        let v = public(&r);
        let c = context(&v, 0);
        assert_eq!(v["identifier_backed_response_count"], 2);
        assert_eq!(c["observed_attempts"], 2);
        assert!(c["input_tokens"].is_null());
        unknown(c, "missing_input_fields");
        assert_eq!(r.total.unwrap().input, 100);
        assert_eq!(r.total.unwrap().output, 14);
    }

    /// The record Claude Code writes itself for an API error, as found in
    /// local transcripts: a client UUID as `message.id`, model `<synthetic>`,
    /// `isApiErrorMessage: true`, every counter an explicit zero and a
    /// `cache_creation` sub-object. Interrupt records have the same message
    /// but no `isApiErrorMessage`.
    fn client_record(id: &str, api_error: bool) -> Value {
        let mut v = json!({"type":"assistant","sessionId":"s",
            "timestamp":"2026-01-01T00:00:01Z","uuid":id,
            "message":{"id":id,"type":"message","role":"assistant","model":"<synthetic>",
                "stop_reason":"stop_sequence","stop_sequence":"","content":[],
                "usage":{"input_tokens":0,"output_tokens":0,
                    "cache_creation_input_tokens":0,"cache_read_input_tokens":0,
                    "cache_creation":{"ephemeral_5m_input_tokens":0,
                        "ephemeral_1h_input_tokens":0},
                    "server_tool_use":{"web_search_requests":0,"web_fetch_requests":0},
                    "service_tier":null,"output_tokens_details":null}}});
        if api_error {
            v["isApiErrorMessage"] = json!(true);
            v["apiErrorStatus"] = json!(429);
            v["error"] = json!("rate_limit");
        }
        v
    }

    #[test]
    fn client_generated_records_are_neither_responses_nor_attempts() {
        let real = native_usage("claude", "s", "msg_real", usage(json!(100)));
        let baseline = public(&report("claude", std::slice::from_ref(&real)));
        // An API error flagged only by `isApiErrorMessage` is dropped too.
        let mut flagged = client_record("4b6f0c1e-0d7a-4d55-9a43-1f0f6b8f1a02", true);
        flagged["message"]["model"] = json!("model");
        let with_client = public(&report(
            "claude",
            &[
                real,
                client_record("9cbe036f-5d2b-4a1e-8f7c-3a6e2d9b0c11", true),
                client_record("33a38de9-7e41-4c0b-b5d2-6f1a9c8e2d44", false),
                flagged,
            ],
        ));
        let c = context(&with_client, 0);
        assert_eq!(c["input_tokens"], 130);
        assert_eq!(c["observed_attempts"], 1);
        assert_eq!(c["tokens_per_observed_attempt"], 130.0);
        assert_eq!(c["unknown_reasons"], json!([]));
        assert_eq!(with_client["identifier_backed_response_count"], 1);
        assert_eq!(
            with_client["sessions"][0]["identifier_backed_response_count"],
            1
        );
        assert_eq!(with_client["models"][0]["observed_attempts"], 1);
        // Nothing about the report changes: counts, context, totals, models,
        // diagnostics, completeness and observation bounds.
        assert_eq!(with_client, baseline);
    }

    #[test]
    fn a_session_of_only_client_records_reports_no_observation_not_zero() {
        let only = public(&report(
            "claude",
            &[
                client_record("9cbe036f-5d2b-4a1e-8f7c-3a6e2d9b0c11", true),
                client_record("33a38de9-7e41-4c0b-b5d2-6f1a9c8e2d44", false),
            ],
        ));
        let c = context(&only, 0);
        assert!(c["tokens_per_observed_attempt"].is_null(), "{c}");
        assert!(c["input_tokens"].is_null(), "{c}");
        assert!(c["observed_attempts"].is_null(), "{c}");
        // Nothing was observed, so no input field was absent either.
        assert_eq!(c["unknown_reasons"], json!(["no_usage_observed"]));
        assert_eq!(c["attempt_basis"], "unknown");
        assert_eq!(only["identifier_backed_response_count"], 0);
        assert!(only["total"].is_null());
        // The same as a session whose log holds no records at all.
        assert_eq!(only, public(&report("claude", &[])));
    }

    #[test]
    fn counters_on_a_client_record_are_diagnosed_not_silently_dropped() {
        for (field, value) in [
            ("input_tokens", json!(5)),
            ("output_tokens", json!(5)),
            ("cache_read_input_tokens", json!("5")),
        ] {
            let mut record = client_record("9cbe036f-5d2b-4a1e-8f7c-3a6e2d9b0c11", true);
            record["message"]["usage"][field] = value.clone();
            let r = report(
                "claude",
                &[
                    native_usage("claude", "s", "msg_real", usage(json!(100))),
                    record,
                ],
            );
            assert!(has(&r, "usage_on_client_generated_record"), "{field}");
            assert!(!r.complete && !r.sessions[0].complete, "{field}");
            assert_eq!(r.identifier_backed_response_count, 1, "{field}");
            assert_eq!(r.total.unwrap().input, 100, "{field}: {value}");
        }
    }

    #[test]
    fn zero_cumulative_attempts_never_substitute_one_or_divide_by_zero() {
        let mut zero = codex_record(0, 0, 0, 0, 1);
        zero["payload"]["model"] = json!("model");
        let r = public(&report(
            "codex",
            &[json!({"type":"session_meta","payload":{"id":"s"}}), zero],
        ));
        let c = context(&r, 0);
        assert_eq!(c["observed_attempts"], 0);
        unknown(c, "zero_observed_attempts");
    }

    #[test]
    fn codex_inclusive_input_is_not_added_to_its_cache_subsets() {
        let r = public(&report(
            "codex",
            &[native_usage("codex", "s", "a", codex_usage(120, 20, 10))],
        ));
        let c = context(&r, 0);
        assert_eq!(c["input_tokens"], 120);
        assert_eq!(c["observed_attempts"], 1);
        assert_eq!(c["tokens_per_observed_attempt"], 120.0);
    }

    #[test]
    fn real_shaped_codex_fixtures_without_cache_write_report_numeric_context() {
        // codex-start.jsonl / codex-resume.jsonl carry the Codex CLI
        // `token_count` shape, which has no `cache_write_input_tokens`.
        let r = collect(
            &manifest(json!([session(
                "codex-session",
                "codex",
                json!([
                    "codex-resume.jsonl",
                    "codex-start.jsonl",
                    "codex-start.jsonl"
                ])
            )])),
            &fixture_dir(),
        )
        .unwrap();
        assert!(has(&r, "cache_write_counter_unreported"));
        // Each transition equals its record's `last_token_usage`; the
        // repeated start file is a replay and is not checked at all.
        assert!(!has(&r, "unconfirmed_cumulative_transition"));
        let v = public(&r);
        let c = context(&v, 0);
        assert_eq!(c["input_tokens"], 180);
        assert_eq!(c["observed_attempts"], 2);
        assert_eq!(c["tokens_per_observed_attempt"], 90.0);
        assert_eq!(c["unknown_reasons"], json!([]));
    }

    #[test]
    fn a_missing_cumulative_prefix_leaves_cumulative_only_attempts_unknown() {
        // The resume file alone: its total of 180 covers the start file's
        // request too, while its `last_token_usage` reports 80.
        let r = collect(
            &manifest(json!([session(
                "codex-session",
                "codex",
                json!(["codex-resume.jsonl"])
            )])),
            &fixture_dir(),
        )
        .unwrap();
        assert!(has(&r, "unconfirmed_cumulative_transition"));
        assert!(!r.sessions[0].complete);
        let v = public(&r);
        let c = context(&v, 0);
        assert_eq!(c["input_tokens"], 180, "the input is retained");
        assert!(c["observed_attempts"].is_null(), "{c}");
        assert!(c["tokens_per_observed_attempt"].is_null(), "{c}");
        assert_eq!(c["attempt_basis"], "unknown");
        assert_eq!(c["unknown_reasons"], json!(["unknown_attempts_for_input"]));
        // Token totals keep the whole cumulative high-water mark.
        assert_eq!(r.total.unwrap().input + r.total.unwrap().cache_read, 180);
    }

    /// A Codex CLI `token_count` record: `[input, cached, output, reasoning]`.
    fn token_count(total: [u64; 4], last: Option<[u64; 4]>, second: u32) -> Value {
        let usage = |[input, read, output, reasoning]: [u64; 4]| {
            json!({"input_tokens":input,"cached_input_tokens":read,
                "cache_write_input_tokens":0,"output_tokens":output,
                "reasoning_output_tokens":reasoning})
        };
        let mut info = json!({"total_token_usage": usage(total)});
        if let Some(last) = last {
            info["last_token_usage"] = usage(last);
        }
        json!({"type":"event_msg","timestamp":format!("2026-01-01T00:00:{second:02}Z"),
            "payload":{"type":"token_count","info":info}})
    }

    #[test]
    fn a_gap_between_resume_files_leaves_cumulative_only_attempts_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, record: Value| {
            write(
                dir.path(),
                name,
                &[
                    json!({"type":"session_meta","payload":{"id":"s"}}),
                    json!({"type":"turn_context","payload":{"model":"m"}}),
                    record,
                ],
            )
        };
        file(
            "start",
            token_count([100, 20, 30, 10], Some([100, 20, 30, 10]), 1),
        );
        file(
            "middle",
            token_count([180, 50, 50, 15], Some([80, 30, 20, 5]), 2),
        );
        file(
            "end",
            token_count([250, 60, 60, 20], Some([70, 10, 10, 5]), 3),
        );
        let run = |files: Value| {
            collect(&manifest(json!([session("s", "codex", files)])), dir.path()).unwrap()
        };

        let all = run(json!(["start", "middle", "end"]));
        assert!(all.complete, "{:?}", all.diagnostics);
        let v = public(&all);
        let c = context(&v, 0);
        assert_eq!(c["input_tokens"], 250);
        assert_eq!(c["observed_attempts"], 3);
        assert_eq!(c["tokens_per_observed_attempt"], 250.0 / 3.0);
        assert_eq!(c["attempt_basis"], "cumulative_only_transitions");

        // Without the middle file the last transition covers two requests.
        let gap = run(json!(["start", "end"]));
        assert!(!gap.complete);
        assert!(has(&gap, "unconfirmed_cumulative_transition"));
        assert_eq!(gap.total, all.total, "token totals are unaffected");
        let v = public(&gap);
        let c = context(&v, 0);
        assert_eq!(c["input_tokens"], 250);
        assert!(c["observed_attempts"].is_null(), "{c}");
        unknown(c, "unknown_attempts_for_input");
        assert_eq!(c["attempt_basis"], "unknown");
    }

    #[test]
    fn unusable_last_usage_cannot_confirm_a_cumulative_transition() {
        for last in [json!({}), json!("80")] {
            let mut record = token_count([180, 50, 50, 15], None, 1);
            record["payload"]["info"]["last_token_usage"] = last.clone();
            let r = report(
                "codex",
                &[json!({"type":"session_meta","payload":{"id":"s"}}), record],
            );
            assert!(has(&r, "missing_or_invalid_token_field"), "{last}");
            assert!(has(&r, "unconfirmed_cumulative_transition"), "{last}");
            let v = public(&r);
            let c = context(&v, 0);
            assert!(c["observed_attempts"].is_null(), "{last}: {c}");
            unknown(c, "unknown_attempts_for_input");
        }
    }

    #[test]
    fn without_last_usage_a_cumulative_transition_counts_as_one_attempt() {
        // The documented limit: with no `last_token_usage` there is nothing
        // to check a transition against, so a missing prefix is not detected.
        let r = report(
            "codex",
            &[
                json!({"type":"session_meta","payload":{"id":"s"}}),
                json!({"type":"turn_context","payload":{"model":"m"}}),
                token_count([180, 50, 50, 15], None, 1),
            ],
        );
        assert!(!has(&r, "unconfirmed_cumulative_transition"));
        let v = public(&r);
        let c = context(&v, 0);
        assert_eq!(c["observed_attempts"], 1);
        assert_eq!(c["tokens_per_observed_attempt"], 180.0);
        assert_eq!(c["unknown_reasons"], json!([]));
    }

    #[test]
    fn a_session_without_attributed_usage_reports_no_usage_observed() {
        // An empty log, and a log whose only response belongs to a native
        // session that is not in the manifest.
        for rows in [
            vec![],
            vec![native_usage("claude", "elsewhere", "m", usage(json!(100)))],
        ] {
            let r = report("claude", &rows);
            assert!(has(&r, "no_usage_observed"));
            let v = public(&r);
            let c = context(&v, 0);
            assert!(c["input_tokens"].is_null(), "{c}");
            assert!(c["observed_attempts"].is_null(), "{c}");
            assert_eq!(c["unknown_reasons"], json!(["no_usage_observed"]));
        }
    }

    #[test]
    fn claude_subagent_records_count_in_the_parent_direct_context() {
        // Claude Code subagent transcripts carry the parent's `sessionId`
        // with `isSidechain: true`. Ownership follows `sessionId`, so their
        // requests are the parent's direct requests wherever they are listed.
        let dir = tempfile::tempdir().unwrap();
        let mut sub = native_usage("claude", "s", "msg_sub", usage(json!(10)));
        sub["isSidechain"] = json!(true);
        sub["agentId"] = json!("a1");
        write(
            dir.path(),
            "main",
            &[native_usage("claude", "s", "msg_main", usage(json!(100)))],
        );
        write(dir.path(), "agent", &[sub]);
        let mut child = session("agent-entry", "claude", json!(["agent"]));
        child["parent"] = json!("s");
        for sessions in [
            json!([session("s", "claude", json!(["main", "agent"]))]),
            json!([session("s", "claude", json!(["main"])), child]),
        ] {
            let v = public(&collect(&manifest(sessions), dir.path()).unwrap());
            let c = context(&v, 0);
            assert_eq!(c["input_tokens"], 130 + 40);
            assert_eq!(c["observed_attempts"], 2);
            assert_eq!(c["tokens_per_observed_attempt"], 85.0);
            assert_eq!(v["sessions"][0]["identifier_backed_response_count"], 2);
            if let Some(entry) = v["sessions"].get(1) {
                assert!(entry["direct"].is_null(), "{entry}");
                assert_eq!(
                    entry["context_per_request"]["unknown_reasons"],
                    json!(["no_usage_observed"])
                );
            }
        }
    }

    #[test]
    fn codex_cache_write_is_optional_but_required_fields_and_present_values_are_checked() {
        let mut absent = codex_usage(120, 20, 0);
        absent
            .as_object_mut()
            .unwrap()
            .remove("cache_write_input_tokens");
        let c = public(&report("codex", &[native_usage("codex", "s", "a", absent)]));
        let ctx = context(&c, 0);
        assert_eq!(ctx["tokens_per_observed_attempt"], 120.0);
        assert_eq!(ctx["unknown_reasons"], json!([]));

        for field in ["input_tokens", "cached_input_tokens"] {
            let mut u = codex_usage(120, 20, 0);
            u.as_object_mut().unwrap().remove(field);
            u.as_object_mut()
                .unwrap()
                .remove("cache_write_input_tokens");
            let r = public(&report("codex", &[native_usage("codex", "s", "a", u)]));
            unknown(context(&r, 0), "missing_input_fields");
        }

        let mut invalid = codex_usage(120, 20, 0);
        invalid["cache_write_input_tokens"] = json!("10");
        let r = public(&report(
            "codex",
            &[native_usage("codex", "s", "a", invalid)],
        ));
        unknown(context(&r, 0), "invalid_input_fields");

        let r = public(&report(
            "codex",
            &[native_usage("codex", "s", "a", codex_usage(100, 60, 50))],
        ));
        unknown(context(&r, 0), "overlapping_input_subsets");
    }

    #[test]
    fn impossible_codex_cache_subsets_keep_context_unknown() {
        let r = report(
            "codex",
            &[native_usage("codex", "s", "a", codex_usage(10, 8, 8))],
        );
        let v = public(&r);
        unknown(context(&v, 0), "overlapping_input_subsets");
        assert!(!r.complete);
        assert_eq!(
            r.total.unwrap().output,
            42,
            "known output survives invalid input"
        );
    }

    #[test]
    fn cumulative_only_transitions_deduplicate_replays_across_models_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let meta = json!({"type":"session_meta","payload":{"id":"s"}});
        let turn = |m: &str| json!({"type":"turn_context","payload":{"model":m}});
        let a = codex_record(100, 20, 30, 10, 1);
        let b = codex_record(180, 50, 50, 15, 2);
        write(
            dir.path(),
            "start",
            &[meta.clone(), turn("first"), a.clone()],
        );
        write(
            dir.path(),
            "resume",
            &[meta, turn("second"), a, b.clone(), b],
        );
        let r = public(
            &collect(
                &manifest(json!([session("s", "codex", json!(["start", "resume"]))])),
                dir.path(),
            )
            .unwrap(),
        );
        let c = context(&r, 0);
        assert_eq!(r["identifier_backed_response_count"], 0);
        assert_eq!(c["input_tokens"], 180);
        assert_eq!(c["observed_attempts"], 2);
        assert_eq!(c["tokens_per_observed_attempt"], 90.0);
        assert_eq!(c["attempt_basis"], "cumulative_only_transitions");
    }

    #[test]
    fn identified_response_with_missing_cumulative_prefix_retains_input_but_unknown_attempts() {
        let mut row = native_usage("codex", "s", "a", codex_usage(40, 5, 0));
        row["payload"]["thread_token_usage"] = codex_usage(100, 20, 0);
        let r = report("codex", &[row]);
        let v = public(&r);
        let c = context(&v, 0);
        assert_eq!(c["input_tokens"], 100);
        assert!(c["observed_attempts"].is_null());
        unknown(c, "unknown_attempts_for_input");
        assert_eq!(c["attempt_basis"], "unknown");
        assert_eq!(v["identifier_backed_response_count"], 1);
        assert_eq!(r.total.unwrap().input + r.total.unwrap().cache_read, 100);
    }

    #[test]
    fn contradictory_cumulative_and_response_input_cannot_supply_a_compatible_ratio() {
        let mut row = native_usage("codex", "s", "a", codex_usage(100, 0, 0));
        row["payload"]["thread_token_usage"] = codex_usage(40, 0, 0);
        let r = public(&report("codex", &[row]));
        let c = context(&r, 0);
        unknown(c, "incompatible_attempt_basis");
        assert_eq!(c["attempt_basis"], "incompatible");
        assert_eq!(r["complete"], false);
    }

    #[test]
    fn provider_scoped_response_count_includes_missing_usage_but_excludes_unidentified() {
        let dir = tempfile::tempdir().unwrap();
        let a = native_usage("claude", "s", "same-id", usage(json!(10)));
        let mut no_usage = native_usage("claude", "s", "without-usage", Value::Null);
        no_usage["message"].as_object_mut().unwrap().remove("usage");
        let mut unidentified = native_usage("claude", "s", "ignored", usage(json!(999)));
        unidentified["message"]
            .as_object_mut()
            .unwrap()
            .remove("id");
        write(
            dir.path(),
            "claude",
            &[a.clone(), a, no_usage.clone(), no_usage, unidentified],
        );
        write(
            dir.path(),
            "codex",
            &[native_usage("codex", "c", "same-id", codex_usage(10, 0, 0))],
        );
        let r = collect(
            &manifest(json!([
                session("s", "claude", json!(["claude"])),
                session("c", "codex", json!(["codex"]))
            ])),
            dir.path(),
        )
        .unwrap();
        let v = public(&r);
        assert_eq!(v["identifier_backed_response_count"], 3);
        assert_eq!(v["sessions"][0]["identifier_backed_response_count"], 2);
        assert_eq!(v["sessions"][1]["identifier_backed_response_count"], 1);
        unknown(context(&v, 0), "missing_input_fields");
        assert!(!r.unresolved.is_empty());
    }

    #[test]
    fn unidentified_response_with_valid_usage_makes_context_input_unknown() {
        let identified = native_usage("claude", "s", "a", usage(json!(100)));
        let mut unidentified = native_usage("claude", "s", "ignored", usage(json!(999)));
        unidentified["message"]
            .as_object_mut()
            .unwrap()
            .remove("id");
        let v = public(&report("claude", &[identified, unidentified]));
        let c = context(&v, 0);
        assert!(c["input_tokens"].is_null(), "{c}");
        assert!(c["observed_attempts"].is_null(), "{c}");
        assert!(c["tokens_per_observed_attempt"].is_null(), "{c}");
        assert_eq!(c["unknown_reasons"], json!(["unidentified_response_input"]));
        assert_eq!(v["identifier_backed_response_count"], 1);
    }

    #[test]
    fn conflicting_native_owners_count_once_globally_and_neither_owns_context() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["a", "b"] {
            write(
                dir.path(),
                id,
                &[native_usage("claude", id, "shared", usage(json!(100)))],
            );
        }
        let r = collect(
            &manifest(json!([
                session("a", "claude", json!(["a"])),
                session("b", "claude", json!(["b"]))
            ])),
            dir.path(),
        )
        .unwrap();
        let v = public(&r);
        assert_eq!(v["identifier_backed_response_count"], 1);
        assert_eq!(r.unattributed, r.total);
        for i in 0..2 {
            assert_eq!(v["sessions"][i]["identifier_backed_response_count"], 0);
            assert!(r.sessions[i].direct.is_none());
            unknown(context(&v, i), "ambiguous_session_ownership");
        }
    }

    #[test]
    fn unsupported_provider_preserves_spelling_and_never_selects_another_adapter() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "log",
            &[native_usage("claude", "s", "a", usage(json!(100)))],
        );
        let value = json!({"version":1,"workstream":"issue-1810","repository":"example/repo",
            "branch":"feat/usage","prs":[42],"completed":true,
            "sessions":[session("s","OtherProvider-v2",json!(["log"]))]});
        let parsed = serde_json::from_value::<Manifest>(value);
        assert!(
            parsed.is_ok(),
            "safe unsupported providers must be representable"
        );
        let r = collect(&parsed.unwrap(), dir.path()).unwrap();
        let v = public(&r);
        assert_eq!(v["sessions"][0]["provider"], "OtherProvider-v2");
        assert_eq!(v["identifier_backed_response_count"], 0);
        assert!(r.total.is_none());
        assert!(!r.complete);
        unknown(context(&v, 0), "unsupported_provider");
    }

    #[test]
    fn unsupported_accounting_schema_cannot_turn_unknown_usage_into_zero() {
        let row = json!({"type":"token_usage_record","timestamp":"2026-01-01T00:00:00Z",
            "payload":{"thread_id":"s","response_id":"a","model":"model",
            "usage":{"future_input_tokens":100}}});
        let r = report("codex", &[row]);
        let v = public(&r);
        assert_eq!(v["identifier_backed_response_count"], 1);
        assert!(r.total.is_none());
        unknown(context(&v, 0), "unsupported_schema");
    }

    #[test]
    fn checked_context_accumulation_refuses_overflow_within_or_between_responses() {
        for rows in [
            vec![native_usage("claude", "s", "a", usage(json!(u64::MAX)))],
            vec![
                native_usage("claude", "s", "a", usage(json!(u64::MAX - 40))),
                native_usage("claude", "s", "b", usage(json!(50))),
            ],
        ] {
            let r = report("claude", &rows);
            let v = public(&r);
            let c = context(&v, 0);
            assert!(c["input_tokens"].is_null());
            unknown(c, "arithmetic_overflow");
            assert!(!r.complete);
        }
    }

    #[test]
    fn merged_response_copies_cannot_establish_overlapping_codex_input() {
        let rows = [
            native_usage("codex", "s", "a", codex_usage(10, 8, 0)),
            native_usage("codex", "s", "a", codex_usage(10, 0, 8)),
        ];
        let r = public(&report("codex", &rows));
        assert_eq!(r["identifier_backed_response_count"], 1);
        unknown(context(&r, 0), "overlapping_input_subsets");
    }

    #[test]
    fn merged_claude_buckets_are_checked_after_component_maxima() {
        let mut first = usage(json!(u64::MAX - 10));
        first["cache_creation_input_tokens"] = json!(0);
        first["cache_read_input_tokens"] = json!(0);
        first["cache_creation"] =
            json!({"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0});
        let second = usage(json!(0));
        let rows = [
            native_usage("claude", "s", "a", first),
            native_usage("claude", "s", "a", second),
        ];
        let r = public(&report("claude", &rows));
        unknown(context(&r, 0), "arithmetic_overflow");
        assert!(context(&r, 0)["input_tokens"].is_null());
    }

    #[test]
    fn unsupported_cumulative_contributor_taints_known_response_context() {
        let rows = [
            native_usage("codex", "s", "a", codex_usage(10, 0, 0)),
            json!({"type":"token_usage_record","timestamp":"2026-01-01T00:00:01Z",
                "payload":{"thread_id":"s","model":"model",
                    "thread_token_usage":{"future_input_tokens":100}}}),
        ];
        let r = public(&report("codex", &rows));
        unknown(context(&r, 0), "unsupported_schema");
        assert!(context(&r, 0)["input_tokens"].is_null());
    }

    #[test]
    fn contradictory_cache_partitions_cannot_inflate_context() {
        let mut u = usage(json!(100));
        u["cache_creation"] =
            json!({"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":10});
        let r = public(&report("claude", &[native_usage("claude", "s", "a", u)]));
        unknown(context(&r, 0), "overlapping_input_subsets");
        assert_eq!(r["total"]["output"], 9);
    }

    #[test]
    fn native_cumulative_only_thread_snapshots_supply_compatible_attempts() {
        let row = json!({"type":"token_usage_record","timestamp":"2026-01-01T00:00:00Z",
            "payload":{"thread_id":"s","model":"model","thread_token_usage":codex_usage(120,20,0)}});
        let r = public(&report("codex", &[row.clone(), row]));
        let c = context(&r, 0);
        assert_eq!(r["identifier_backed_response_count"], 0);
        assert_eq!(c["input_tokens"], 120);
        assert_eq!(c["observed_attempts"], 1);
        assert_eq!(c["tokens_per_observed_attempt"], 120.0);
        assert_eq!(c["attempt_basis"], "cumulative_only_transitions");
    }

    #[test]
    fn context_lines_have_their_own_heading_with_one_line_per_session() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "a",
            &[native_usage("claude", "s", "a", usage(json!(100)))],
        );
        write(
            dir.path(),
            "b",
            &[native_usage("claude", "review", "b", usage(json!(50)))],
        );
        let r = collect(
            &manifest(json!([
                session("s", "claude", json!(["a"])),
                session("review", "claude", json!(["b"]))
            ])),
            dir.path(),
        )
        .unwrap();
        let rendered = summary(&r);
        let section = |heading: &str| -> Vec<String> {
            let start = rendered
                .find(heading)
                .unwrap_or_else(|| panic!("{heading}"));
            rendered[start + heading.len()..]
                .lines()
                .skip_while(|l| l.trim().is_empty())
                .take_while(|l| l.starts_with("- "))
                .map(str::to_owned)
                .collect()
        };
        let context = section("Session context per request:");
        let totals = section("Session direct / inclusive token totals (inclusive rows overlap):");
        for lines in [&context, &totals] {
            assert_eq!(lines.len(), 2, "{rendered}");
            assert_eq!(lines.iter().filter(|l| l.starts_with("- `s`")).count(), 1);
            assert_eq!(
                lines.iter().filter(|l| l.starts_with("- `review`")).count(),
                1
            );
        }
        assert!(
            context
                .iter()
                .all(|l| l.contains("tokens per observed attempt"))
        );
        assert!(
            totals
                .iter()
                .all(|l| !l.contains("tokens per observed attempt"))
        );
    }

    #[test]
    fn canonical_renderer_and_publication_include_every_context_and_preserve_one_marker() {
        let dir = tempfile::tempdir().unwrap();
        let known = native_usage("claude", "s", "a", usage(json!(100)));
        let missing = native_usage("claude", "review", "b", json!({"output_tokens":5}));
        write(dir.path(), "first", &[known]);
        write(dir.path(), "review", &[missing]);
        let mut m = manifest(json!([
            session("s", "claude", json!(["first"])),
            session("review", "claude", json!(["review"]))
        ]));
        m.prs.push(43);
        let r = collect(&m, dir.path()).unwrap();
        let rendered = summary(&r);
        let lower = rendered.to_lowercase();
        for phrase in [
            "tokens per observed attempt",
            "observational",
            "context-window",
            "missing_input_fields",
            "unknown",
            "identifier",
            "shared non-additive",
            "inclusive",
            "informational",
        ] {
            assert!(lower.contains(phrase), "renderer must explain {phrase}");
        }
        assert!(rendered.contains("130"));
        assert_eq!(summary(&r), rendered, "rendering is deterministic");
        let host = Hosting::default();
        assert_eq!(publish::publish(&m, &r, 42, &host).unwrap(), "created");
        let original = host.comments.borrow()[0].clone();
        let (marker, body) = original["body"].as_str().unwrap().split_once('\n').unwrap();
        assert_eq!(body, rendered, "publication uses the canonical renderer");
        assert!(marker.starts_with("<!-- amplihack-usage:v1:"));
        assert_eq!(publish::publish(&m, &r, 42, &host).unwrap(), "unchanged");
        assert_eq!(*host.methods.borrow(), vec!["POST"]);
        // A successor input snapshot supplies late review usage; the old bytes stay untouched.
        write(
            dir.path(),
            "late",
            &[native_usage("claude", "review", "late", usage(json!(25)))],
        );
        m.sessions[1].files.push("late".into());
        let late = collect(&m, dir.path()).unwrap();
        assert_eq!(publish::publish(&m, &late, 42, &host).unwrap(), "updated");
        let comments = host.comments.borrow();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["id"], original["id"]);
        assert!(comments[0]["body"].as_str().unwrap().starts_with(marker));
        assert_eq!(*host.methods.borrow(), vec!["POST", "PATCH"]);
    }
}

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
    m.workstream = Some("safe".into());
    m.sessions[0].provider = Provider::Unsupported("secret\n<!-- injected -->".into());
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
