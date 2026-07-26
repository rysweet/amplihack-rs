//! TDD contract tests for the Phase A `/signal` topic bridge (amplihack-signal
//! reusable core). These are written **first** and are expected to FAIL to
//! compile until the `bridge` module tree and the `transport::parse_group_members`
//! helper are implemented.
//!
//! Run: `cargo test -p amplihack-signal --features signal --test bridge_it`.
//!
//! The whole file is gated on the `signal` feature so a default (feature-off)
//! build compiles it away to nothing (matching the crate's empty-shim policy).
#![cfg(feature = "signal")]

// =============================================================================
// Group naming — bridge::naming  (host + tmux slug, deterministic, 40-char cap)
// =============================================================================
mod naming {
    use amplihack_signal::bridge::naming::{group_name, slug};

    #[test]
    fn slug_lowercases_collapses_and_trims() {
        // Non-alphanumeric runs collapse to a single '-'; leading/trailing '-'
        // trimmed; result lowercased.
        assert_eq!(slug("review PR 3967!"), "review-pr-3967");
        assert_eq!(slug("  Hello___World  "), "hello-world");
        assert_eq!(slug("!!!edges!!!"), "edges");
    }

    #[test]
    fn slug_is_length_capped_to_40() {
        let long = "a very long topic ".repeat(20); // >> 40 chars
        let s = slug(&long);
        assert!(
            s.len() <= 40,
            "slug must be capped at ~40 chars, got {} ({s:?})",
            s.len()
        );
        // Cap must not leave a dangling trailing '-'.
        assert!(
            !s.ends_with('-'),
            "capped slug must not end with '-': {s:?}"
        );
    }

    #[test]
    fn group_name_without_tmux() {
        assert_eq!(
            group_name("azlin-07", None, "review PR 3967!"),
            "amplihack-azlin-07-review-pr-3967"
        );
    }

    #[test]
    fn group_name_with_tmux_includes_session() {
        // Documented example: host azlin-07, tmux session "ops".
        assert_eq!(
            group_name("azlin-07", Some("ops"), "review PR 3967!"),
            "amplihack-azlin-07-ops-review-pr-3967"
        );
    }

    #[test]
    fn group_name_is_deterministic() {
        let a = group_name("h", Some("t"), "same topic");
        let b = group_name("h", Some("t"), "same topic");
        assert_eq!(a, b, "naming must be a pure deterministic function");
    }
}

// =============================================================================
// Control phrases — bridge::control  (parsed BEFORE a body becomes a prompt)
// =============================================================================
mod control {
    use amplihack_signal::bridge::control::{Control, parse_control};

    #[test]
    fn status_is_a_control_command() {
        assert!(matches!(parse_control("status"), Control::Status));
        assert!(matches!(parse_control("  STATUS  "), Control::Status));
    }

    #[test]
    fn stop_and_kill_map_to_stop() {
        assert!(matches!(parse_control("stop"), Control::Stop));
        assert!(matches!(parse_control("kill"), Control::Stop));
        assert!(matches!(parse_control("  Stop\n"), Control::Stop));
        assert!(matches!(parse_control("KILL"), Control::Stop));
    }

    #[test]
    fn control_takes_precedence_but_only_on_exact_word() {
        // A sentence containing "stop" is a normal prompt, NOT a control phrase.
        match parse_control("please stop the review") {
            Control::Prompt(p) => assert_eq!(p, "please stop the review"),
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    #[test]
    fn ordinary_text_is_a_prompt() {
        match parse_control("run the tests again") {
            Control::Prompt(p) => assert_eq!(p, "run the tests again"),
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    #[test]
    fn parse_control_never_errors_unknown_is_prompt() {
        // Total function: any non-control body is preserved verbatim as a Prompt.
        assert!(matches!(parse_control("stopwatch"), Control::Prompt(_)));
    }
}

// =============================================================================
// Tool allowlist — bridge::allowlist  (default read-only, NOT allow-all)
// =============================================================================
mod allowlist {
    use amplihack_signal::bridge::allowlist::ToolAllowlist;

    #[test]
    fn default_is_read_only_view_grep_glob() {
        let al = ToolAllowlist::read_only_default();
        assert!(!al.is_dangerous(), "default must never be dangerous");
        assert_eq!(
            al.to_copilot_args(),
            vec![
                "--allow-tool",
                "view",
                "--allow-tool",
                "grep",
                "--allow-tool",
                "glob",
            ]
        );
    }

    #[test]
    fn default_never_emits_allow_all() {
        let args = ToolAllowlist::read_only_default().to_copilot_args();
        assert!(
            !args
                .iter()
                .any(|a| a == "--allow-all" || a == "--allow-all-tools"),
            "read-only default must not grant blanket tool access: {args:?}"
        );
    }

    #[test]
    fn scoped_flags_render_repeated_allow_tool_in_order() {
        let al = ToolAllowlist::from_flags(
            &["edit".to_string(), "shell(git commit)".to_string()],
            false,
        );
        assert!(!al.is_dangerous());
        assert_eq!(
            al.to_copilot_args(),
            vec!["--allow-tool", "edit", "--allow-tool", "shell(git commit)",]
        );
    }

    #[test]
    fn empty_flags_without_dangerous_falls_back_to_read_only_default() {
        let al = ToolAllowlist::from_flags(&[], false);
        assert!(!al.is_dangerous());
        assert_eq!(
            al.to_copilot_args(),
            ToolAllowlist::read_only_default().to_copilot_args(),
            "no --allow-tool + no dangerous ⇒ least-privilege read-only default"
        );
    }

    #[test]
    fn dangerous_maps_to_allow_all_tools_not_allow_all() {
        let al = ToolAllowlist::from_flags(&[], true);
        assert!(al.is_dangerous());
        let args = al.to_copilot_args();
        assert!(
            args.iter().any(|a| a == "--allow-all-tools"),
            "dangerous mode must emit --allow-all-tools: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "--allow-all"),
            "must use tools-only escape hatch, never the wider --allow-all: {args:?}"
        );
    }

    #[test]
    fn describe_lists_blast_radius_for_announcement() {
        // The first group message prints the effective allowlist verbatim.
        let d = ToolAllowlist::read_only_default().describe();
        for tool in ["view", "grep", "glob"] {
            assert!(d.contains(tool), "describe() must name {tool}: {d:?}");
        }
    }
}

// =============================================================================
// Outbound chunking — bridge::chunk  (Signal per-message limit, char-safe)
// =============================================================================
mod chunk {
    use amplihack_signal::bridge::chunk::{SIGNAL_MAX_BYTES, chunk};

    #[test]
    fn max_bytes_is_signal_sized_not_frame_sized() {
        // Distinct from the inbound JSON-RPC MAX_FRAME_BYTES (256 KiB).
        assert_eq!(SIGNAL_MAX_BYTES, 2000);
    }

    #[test]
    fn short_body_is_a_single_chunk() {
        assert_eq!(chunk("hello"), vec!["hello".to_string()]);
    }

    #[test]
    fn large_ascii_body_splits_under_the_cap_and_reassembles() {
        let body = "x".repeat(5000);
        let chunks = chunk(&body);
        assert!(chunks.len() >= 3, "5000 bytes must split into >=3 chunks");
        for c in &chunks {
            assert!(
                c.len() <= SIGNAL_MAX_BYTES,
                "chunk of {} bytes exceeds cap {SIGNAL_MAX_BYTES}",
                c.len()
            );
        }
        assert_eq!(chunks.concat(), body, "chunks must reassemble losslessly");
    }

    #[test]
    fn never_splits_a_multibyte_codepoint() {
        // '€' is 3 bytes; a run whose byte-length straddles the cap must still
        // never cut a codepoint (every chunk is valid UTF-8 by construction and
        // reassembles exactly).
        let body = "€".repeat(2000); // 6000 bytes
        let chunks = chunk(&body);
        for c in &chunks {
            assert!(c.len() <= SIGNAL_MAX_BYTES);
        }
        assert_eq!(chunks.concat(), body);
    }
}

// =============================================================================
// Outbound membership verification — bridge::membership  (FAIL CLOSED)
// =============================================================================
mod membership {
    use amplihack_signal::bridge::membership::{Membership, classify};

    fn expected() -> Vec<String> {
        vec!["+15551230000".to_string(), "+15551230001".to_string()]
    }

    #[test]
    fn exact_expected_set_is_verified_and_may_relay() {
        let actual = expected();
        let m = classify(&expected(), Some(&actual));
        assert!(matches!(m, Membership::Verified));
        assert!(m.may_relay(), "verified membership permits outbound relay");
    }

    #[test]
    fn query_error_is_unverified_and_refuses_relay() {
        // `None` models an RPC error / timeout / ambiguous response.
        let m = classify(&expected(), None);
        assert!(matches!(m, Membership::Unverified(_)));
        assert!(!m.may_relay(), "unverified membership must FAIL CLOSED");
    }

    #[test]
    fn unexpected_extra_member_refuses_relay() {
        let actual = vec![
            "+15551230000".to_string(),
            "+15551230001".to_string(),
            "+15559999999".to_string(), // intruder
        ];
        let m = classify(&expected(), Some(&actual));
        assert!(matches!(m, Membership::Unverified(_)));
        assert!(!m.may_relay());
    }

    #[test]
    fn missing_expected_member_refuses_relay() {
        let actual = vec!["+15551230000".to_string()];
        let m = classify(&expected(), Some(&actual));
        assert!(matches!(m, Membership::Unverified(_)));
        assert!(!m.may_relay());
    }
}

// =============================================================================
// Copilot turn driver — bridge::turn  (pinned argv + serialized single-turn)
// =============================================================================
mod turn {
    use amplihack_signal::bridge::allowlist::ToolAllowlist;
    use amplihack_signal::bridge::turn::{SerialTurnDriver, TurnRunner, build_turn_argv};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const SID: &str = "11111111-2222-3333-4444-555555555555";

    #[test]
    fn argv_pins_session_id_silent_and_no_color() {
        let argv = build_turn_argv(SID, "hello world", &ToolAllowlist::read_only_default());

        // --session-id is immediately followed by the pinned uuid.
        let sid_pos = argv
            .iter()
            .position(|a| a == "--session-id")
            .expect("argv must pin --session-id");
        assert_eq!(argv.get(sid_pos + 1).map(String::as_str), Some(SID));

        // Clean, ANSI-free, response-only capture.
        assert!(
            argv.iter().any(|a| a == "-s" || a == "--silent"),
            "argv must request silent (response-only) output: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a == "--no-color"),
            "argv must disable color for clean stdout: {argv:?}"
        );

        // Allowlist rendered into the same argv.
        assert!(argv.windows(2).any(|w| w == ["--allow-tool", "view"]));
    }

    #[test]
    fn prompt_is_a_single_argv_element_never_a_shell_string() {
        // IV-2 injection contract: the prompt (attacker-influenced) is passed as
        // ONE argv element, verbatim, and is never concatenated into a shell
        // command line. Metacharacters must survive unchanged and unsplit.
        let evil = "; rm -rf / # $(whoami) `id`";
        let argv = build_turn_argv(SID, evil, &ToolAllowlist::read_only_default());

        let p_pos = argv
            .iter()
            .position(|a| a == "-p" || a == "--prompt")
            .expect("argv must carry a prompt flag");
        assert_eq!(
            argv.get(p_pos + 1).map(String::as_str),
            Some(evil),
            "prompt must be exactly one argv element, unmodified"
        );
    }

    #[test]
    fn read_only_argv_never_grants_allow_all() {
        let argv = build_turn_argv(SID, "topic", &ToolAllowlist::read_only_default());
        assert!(
            !argv
                .iter()
                .any(|a| a == "--allow-all" || a == "--allow-all-tools")
        );
    }

    // A mock runner that records the maximum number of turns running at once and
    // the argv it received, to prove the driver serializes execution.
    struct MockRunner {
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<Vec<String>>>>,
    }

    impl TurnRunner for MockRunner {
        fn run_argv(
            &self,
            argv: Vec<String>,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<String>> + Send>> {
            let active = self.active.clone();
            let max_active = self.max_active.clone();
            let seen = self.seen.clone();
            Box::pin(async move {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(now, Ordering::SeqCst);
                seen.lock().unwrap().push(argv);
                tokio::time::sleep(Duration::from_millis(40)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(String::from("ok"))
            })
        }
    }

    #[tokio::test]
    async fn turns_execute_one_at_a_time_per_session() {
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let runner = MockRunner {
            active: active.clone(),
            max_active: max_active.clone(),
            seen: seen.clone(),
        };

        let driver = Arc::new(SerialTurnDriver::new(
            runner,
            SID,
            ToolAllowlist::read_only_default(),
        ));

        let d1 = driver.clone();
        let d2 = driver.clone();
        let t1 = tokio::spawn(async move { d1.run_turn("first").await });
        let t2 = tokio::spawn(async move { d2.run_turn("second").await });
        t1.await.unwrap().unwrap();
        t2.await.unwrap().unwrap();

        assert_eq!(
            max_active.load(Ordering::SeqCst),
            1,
            "the driver MUST serialize turns (one concurrent turn per session)"
        );
        assert_eq!(seen.lock().unwrap().len(), 2, "both turns must run");
    }
}

// =============================================================================
// Transport — parse_group_members  (fail-closed JSON-RPC parse for membership)
// =============================================================================
mod transport_members {
    use amplihack_signal::transport::parse_group_members;
    use serde_json::json;

    #[test]
    fn parses_expected_member_numbers() {
        // Assumed signal-cli `listGroups` shape (adjust in impl if OI-2 resolves
        // to `getGroup`): an array of groups, each with an `id` and `members`.
        let v = json!([{
            "id": "grp-abc==",
            "members": [
                {"number": "+15551230000"},
                {"number": "+15551230001"},
            ],
        }]);
        let members = parse_group_members(&v, "grp-abc==").expect("well-formed members parse");
        assert_eq!(members, vec!["+15551230000", "+15551230001"]);
    }

    #[test]
    fn missing_members_field_is_err_fail_closed() {
        let v = json!([{ "id": "grp-abc==" }]);
        assert!(
            parse_group_members(&v, "grp-abc==").is_err(),
            "absent member set must fail closed, never Ok(empty)"
        );
    }

    #[test]
    fn empty_members_is_err_fail_closed() {
        let v = json!([{ "id": "grp-abc==", "members": [] }]);
        assert!(parse_group_members(&v, "grp-abc==").is_err());
    }

    #[test]
    fn group_not_present_is_err_fail_closed() {
        let v = json!([{ "id": "some-other-group==", "members": [{"number": "+1"}] }]);
        assert!(parse_group_members(&v, "grp-abc==").is_err());
    }

    #[test]
    fn null_or_malformed_result_is_err() {
        assert!(parse_group_members(&serde_json::Value::Null, "grp-abc==").is_err());
        assert!(parse_group_members(&json!("nonsense"), "grp-abc==").is_err());
    }
}

// =============================================================================
// Outbound redaction reuse — bridge::outbound  (redact BEFORE chunk)
// =============================================================================
mod outbound {
    use amplihack_signal::bridge::chunk::SIGNAL_MAX_BYTES;
    use amplihack_signal::bridge::outbound::{redact_and_chunk, redact_for_relay};

    const SECRET: &str = "sk-supersecrettoken1234567890";

    #[test]
    fn redact_for_relay_removes_bearer_secret() {
        let red = redact_for_relay(&format!("Authorization: Bearer {SECRET}"));
        assert!(
            red.contains("[REDACTED]"),
            "expected redaction marker: {red:?}"
        );
        assert!(!red.contains(SECRET), "secret must not survive redaction");
    }

    #[test]
    fn redaction_happens_before_chunking_so_no_chunk_leaks_a_secret() {
        // DP-1: redaction is applied to the whole body BEFORE it is chunked, so a
        // secret can never leak in any individual outbound Signal message.
        let body = format!("{}\nAuthorization: Bearer {SECRET}", "x".repeat(2500));
        let chunks = redact_and_chunk(&body);
        assert!(chunks.len() >= 2, "body larger than the cap must chunk");
        for c in &chunks {
            assert!(c.len() <= SIGNAL_MAX_BYTES, "each chunk respects the cap");
            assert!(!c.contains(SECRET), "no chunk may contain the raw secret");
        }
    }
}

// =============================================================================
// Bridge error taxonomy — bridge::BridgeError  (6-code exit contract) +
// real daemon-down and loopback failure modes
// =============================================================================
mod failure_modes {
    use amplihack_signal::bridge::{BridgeError, validate_endpoint};

    #[test]
    fn exit_code_contract_is_stable() {
        assert_eq!(BridgeError::NotLinked.exit_code(), 1);
        assert_eq!(BridgeError::RemoteEndpointRejected.exit_code(), 2);
        assert_eq!(BridgeError::GroupCreateFailed.exit_code(), 3);
        assert_eq!(BridgeError::DaemonUnavailable.exit_code(), 4);
        assert_eq!(BridgeError::ResumeProbeFailed.exit_code(), 5);
    }

    #[test]
    fn non_loopback_endpoint_fails_closed_with_exit_2() {
        let err = validate_endpoint("10.0.0.5:7583", false).expect_err("routable host rejected");
        assert!(matches!(err, BridgeError::RemoteEndpointRejected));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn loopback_endpoint_is_accepted() {
        assert!(validate_endpoint("127.0.0.1:7583", false).is_ok());
    }

    #[test]
    fn remote_endpoint_allowed_only_with_explicit_unsafe_opt_in() {
        assert!(
            validate_endpoint("10.0.0.5:7583", true).is_ok(),
            "explicit --unsafe-remote-endpoint opt-in permits a non-loopback endpoint"
        );
    }

    #[tokio::test]
    async fn daemon_down_shuts_down_cleanly_with_exit_4_after_retry_budget() {
        // Port 9 (discard) is closed on a normal host ⇒ connection refused. The
        // bridge must exhaust its bounded retry budget and surface a clean
        // DaemonUnavailable (exit 4), never hang or silently disable itself.
        let err = amplihack_signal::bridge::connect_daemon("127.0.0.1:9", 2)
            .await
            .expect_err("connecting to a closed port must fail");
        assert!(matches!(err, BridgeError::DaemonUnavailable));
        assert_eq!(err.exit_code(), 4);
    }
}
