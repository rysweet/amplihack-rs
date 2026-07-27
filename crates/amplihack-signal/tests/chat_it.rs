//! TDD contract tests for the Phase A `/signal` topic chat (amplihack-signal
//! reusable core). These are written **first** and are expected to FAIL to
//! compile until the `chat` module tree and the `transport::parse_group_members`
//! helper are implemented.
//!
//! Run: `cargo test -p amplihack-signal --features signal --test chat_it`.
//!
//! The whole file is gated on the `signal` feature so a default (feature-off)
//! build compiles it away to nothing (matching the crate's empty-shim policy).
#![cfg(feature = "signal")]

// =============================================================================
// Group naming — chat::naming  (host + tmux slug, deterministic, 40-char cap)
// =============================================================================
mod naming {
    use amplihack_signal::chat::naming::{group_name, slug};

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
// Control phrases — chat::control  (parsed BEFORE a body becomes a prompt)
// =============================================================================
mod control {
    use amplihack_signal::chat::control::{Control, parse_control};

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
// Tool allowlist — chat::allowlist  (default read-only, NOT allow-all)
// =============================================================================
mod allowlist {
    use amplihack_signal::chat::allowlist::ToolAllowlist;

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
// Outbound chunking — chat::chunk  (Signal per-message limit, char-safe)
// =============================================================================
mod chunk {
    use amplihack_signal::chat::chunk::{SIGNAL_MAX_BYTES, chunk};

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
// Outbound membership verification — chat::membership  (FAIL CLOSED)
// =============================================================================
mod membership {
    use amplihack_signal::chat::membership::{Membership, classify};

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
// Copilot turn driver — chat::turn  (pinned argv + serialized single-turn)
// =============================================================================
mod turn {
    use amplihack_signal::chat::allowlist::ToolAllowlist;
    use amplihack_signal::chat::turn::{
        CopilotTurnRunner, PreemptSlot, SerialTurnDriver, TurnRunner, build_turn_argv,
    };
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

    // F2 pre-emption contract: a control `stop` fires the child-bound trigger
    // published into the shared `PreemptSlot`, which kills the OWNED child via
    // `Child::start_kill()` (immune to PID reuse). The pre-empted turn resolves
    // to an `io::ErrorKind::Interrupted` error — never a `SIGKILL` of a recycled
    // PID.
    #[tokio::test]
    async fn preempt_kills_in_flight_child_with_interrupted_error() {
        let slot: PreemptSlot = Arc::new(Mutex::new(None));
        let runner = CopilotTurnRunner::new("sleep", slot.clone());
        // A long-blocking child so the pre-empt lands mid-turn.
        let handle = tokio::spawn(runner.run_argv(vec!["30".to_string()]));

        // Wait until the runner has published its pre-empt trigger.
        for _ in 0..200 {
            if slot.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            slot.lock().unwrap().is_some(),
            "runner must publish a pre-empt trigger while a turn is in flight"
        );

        // Fire the pre-empt exactly like the chat's `Control::Stop` path.
        if let Some(tx) = slot.lock().unwrap().take() {
            let _ = tx.send(());
        }

        let res = handle.await.expect("turn task should not panic");
        let err = res.expect_err("a pre-empted turn must resolve to an error");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::Interrupted,
            "pre-emption must surface as Interrupted, got: {err:?}"
        );
        // Slot is cleared on completion, so a later pre-empt is a no-op.
        assert!(slot.lock().unwrap().is_none());
    }

    // A normal (un-pre-empted) turn returns its captured stdout unchanged.
    #[tokio::test]
    async fn normal_turn_returns_stdout() {
        let slot: PreemptSlot = Arc::new(Mutex::new(None));
        let runner = CopilotTurnRunner::new("printf", slot.clone());
        let out = runner
            .run_argv(vec!["hello-turn".to_string()])
            .await
            .expect("a normal turn should succeed");
        assert_eq!(out, "hello-turn");
        assert!(
            slot.lock().unwrap().is_none(),
            "slot must be cleared after a normal turn completes"
        );
    }

    // A non-zero child exit is NOT a pre-emption: it must surface as a plain
    // error (so the chat can relay the failure and resume), never as
    // `Interrupted`, and the slot must still be cleared.
    #[tokio::test]
    async fn nonzero_exit_surfaces_as_error_not_interrupted() {
        let slot: PreemptSlot = Arc::new(Mutex::new(None));
        let runner = CopilotTurnRunner::new("false", slot.clone());
        let err = runner
            .run_argv(vec![])
            .await
            .expect_err("a non-zero exit must resolve to an error");
        assert_ne!(
            err.kind(),
            std::io::ErrorKind::Interrupted,
            "an ordinary failed turn must not masquerade as a pre-emption"
        );
        assert!(
            slot.lock().unwrap().is_none(),
            "slot must be cleared even when the turn fails"
        );
    }

    // Pre-emption must stay deadlock-free even when the child is actively
    // flooding stdout: the concurrent pipe drain keeps `wait()` (and the
    // post-kill reap) from blocking on a full OS pipe buffer. A child that
    // writes an unbounded stream via `yes` is pre-empted and reaped, resolving
    // to `Interrupted` well within the timeout.
    #[tokio::test]
    async fn preempt_is_deadlock_free_while_child_floods_stdout() {
        let slot: PreemptSlot = Arc::new(Mutex::new(None));
        let runner = CopilotTurnRunner::new("yes", slot.clone());
        let handle = tokio::spawn(runner.run_argv(vec!["flood".to_string()]));

        for _ in 0..200 {
            if slot.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if let Some(tx) = slot.lock().unwrap().take() {
            let _ = tx.send(());
        }

        let res = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("pre-empting a stdout-flooding child must not deadlock")
            .expect("turn task should not panic");
        let err = res.expect_err("a pre-empted turn must resolve to an error");
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
        assert!(slot.lock().unwrap().is_none());
    }

    // Firing the trigger after the turn already finished is a harmless no-op:
    // the runner clears the slot on completion, so the operator's late `stop`
    // cannot kill an unrelated (recycled) process.
    #[tokio::test]
    async fn preempt_after_completion_is_a_noop() {
        let slot: PreemptSlot = Arc::new(Mutex::new(None));
        let runner = CopilotTurnRunner::new("printf", slot.clone());
        let out = runner
            .run_argv(vec!["done".to_string()])
            .await
            .expect("turn should succeed");
        assert_eq!(out, "done");
        // Slot is already None; a late take()+send() has nothing to fire.
        assert!(
            slot.lock().unwrap().take().is_none(),
            "no stale trigger may linger after the turn completes"
        );
    }

    // A runner that only records the argv of every turn and returns immediately,
    // so a sequence of turns can be replayed and inspected deterministically.
    struct RecordingRunner {
        seen: Arc<Mutex<Vec<Vec<String>>>>,
    }

    impl TurnRunner for RecordingRunner {
        fn run_argv(
            &self,
            argv: Vec<String>,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<String>> + Send>> {
            let seen = self.seen.clone();
            Box::pin(async move {
                seen.lock().unwrap().push(argv);
                Ok(String::from("ok"))
            })
        }
    }

    // INV-5 (resume continuity + injection safety). Successive turns on one
    // `SerialTurnDriver` all resume the SAME `--session-id`, and every prompt —
    // including adversarial shell metacharacters and a leading dash — is passed
    // as EXACTLY ONE `-p` argv element, verbatim, never split or concatenated
    // into a shell string. A weak assertion here would silently permit a future
    // command-injection regression, so the prompts are explicitly adversarial.
    #[tokio::test]
    async fn characterization_inv5_successive_turns_reuse_same_session_id() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let runner = RecordingRunner { seen: seen.clone() };
        let driver = SerialTurnDriver::new(runner, SID, ToolAllowlist::read_only_default());

        let prompts = [
            "first turn",
            "; rm -rf / #",
            "$(whoami)",
            "`id`",
            "a && b || c",
            "quote\"and'quote",
            "line1\nline2",
            "-p --session-id 99999999-0000-0000-0000-000000000000",
        ];
        for p in prompts {
            driver
                .run_turn(p)
                .await
                .expect("recording runner turn succeeds");
        }

        let calls = seen.lock().unwrap();
        assert_eq!(
            calls.len(),
            prompts.len(),
            "every turn must reach the runner"
        );

        for (argv, expected_prompt) in calls.iter().zip(prompts.iter()) {
            // Resume continuity: the pinned session id is reused every turn.
            let sid_pos = argv
                .iter()
                .position(|a| a == "--session-id")
                .expect("every turn pins --session-id");
            assert_eq!(
                argv.get(sid_pos + 1).map(String::as_str),
                Some(SID),
                "successive turns must resume the SAME session id"
            );

            // Injection safety: exactly one prompt flag, and the prompt is one
            // argv element, byte-for-byte, even with shell metacharacters.
            let prompt_flags = argv
                .iter()
                .filter(|a| a.as_str() == "-p" || a.as_str() == "--prompt")
                .count();
            assert_eq!(
                prompt_flags, 1,
                "argv must carry exactly one prompt flag: {argv:?}"
            );
            let p_pos = argv
                .iter()
                .position(|a| a == "-p" || a == "--prompt")
                .expect("argv must carry a prompt flag");
            assert_eq!(
                argv.get(p_pos + 1).map(String::as_str),
                Some(*expected_prompt),
                "prompt must be exactly one argv element, unmodified: {argv:?}"
            );
        }

        assert_eq!(
            driver.session_id(),
            SID,
            "the driver stays bound to its pinned session id across turns"
        );
    }

    // A runner whose completion is gated on an out-of-band channel, so a "long"
    // turn is simulated deterministically with no real sleep. Used to prove the
    // driver imposes no per-turn wall-clock deadline.
    struct GatedRunner {
        rx: Mutex<Option<tokio::sync::oneshot::Receiver<String>>>,
    }

    impl TurnRunner for GatedRunner {
        fn run_argv(
            &self,
            _argv: Vec<String>,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<String>> + Send>> {
            let rx = self
                .rx
                .lock()
                .unwrap()
                .take()
                .expect("GatedRunner runs exactly one turn");
            Box::pin(async move {
                rx.await
                    .map_err(|_| std::io::Error::other("runner completion channel dropped"))
            })
        }
    }

    // INV-6 (no per-turn wall-clock timeout). `SerialTurnDriver::run_turn` wraps
    // the turn in NO `tokio::time::timeout` / elapsed-time cap: while the
    // runner's work is still outstanding the turn stays alive across many
    // runtime polls (it is never aborted), and it resolves with the runner's
    // captured output only on natural completion.
    #[tokio::test]
    async fn characterization_inv6_turn_runs_to_completion_no_wallclock_cap() {
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        let runner = GatedRunner {
            rx: Mutex::new(Some(rx)),
        };
        let driver = Arc::new(SerialTurnDriver::new(
            runner,
            SID,
            ToolAllowlist::read_only_default(),
        ));

        let d = driver.clone();
        let handle =
            tokio::spawn(async move { d.run_turn("a legitimately long-running turn").await });

        // With the turn's work still outstanding, spin the runtime repeatedly:
        // a per-turn deadline (if one existed) would abort the turn here. It
        // does not — the turn is still in flight.
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        assert!(
            !handle.is_finished(),
            "a turn must not be aborted while its work is outstanding (no per-turn wall-clock cap)"
        );

        // Natural completion resolves the turn with the runner's captured output.
        tx.send("captured turn output".to_string())
            .expect("gated runner still awaiting completion");
        let out = handle
            .await
            .expect("turn task must not panic")
            .expect("turn resolves Ok on natural completion");
        assert_eq!(out, "captured turn output");
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
// Outbound redaction reuse — chat::outbound  (redact BEFORE chunk)
// =============================================================================
mod outbound {
    use amplihack_signal::chat::chunk::SIGNAL_MAX_BYTES;
    use amplihack_signal::chat::outbound::{redact_and_chunk, redact_for_relay};

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
// Chat error taxonomy — chat::ChatError  (6-code exit contract) +
// real daemon-down and loopback failure modes
// =============================================================================
mod failure_modes {
    use amplihack_signal::chat::{ChatError, validate_endpoint};

    #[test]
    fn exit_code_contract_is_stable() {
        assert_eq!(ChatError::NotLinked.exit_code(), 1);
        assert_eq!(ChatError::RemoteEndpointRejected.exit_code(), 2);
        assert_eq!(ChatError::GroupCreateFailed.exit_code(), 3);
        assert_eq!(ChatError::DaemonUnavailable.exit_code(), 4);
        assert_eq!(ChatError::ResumeProbeFailed.exit_code(), 5);
    }

    #[test]
    fn non_loopback_endpoint_fails_closed_with_exit_2() {
        let err = validate_endpoint("10.0.0.5:7583", false).expect_err("routable host rejected");
        assert!(matches!(err, ChatError::RemoteEndpointRejected));
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
        // chat must exhaust its bounded retry budget and surface a clean
        // DaemonUnavailable (exit 4), never hang or silently disable itself.
        let err = amplihack_signal::chat::connect_daemon("127.0.0.1:9", 2)
            .await
            .expect_err("connecting to a closed port must fail");
        assert!(matches!(err, ChatError::DaemonUnavailable));
        assert_eq!(err.exit_code(), 4);
    }
}

// =============================================================================
// Inbound gating fail-closed anchor — chat::gating  (INV-3, consolidated)
// =============================================================================
mod gating {
    use amplihack_signal::config::{
        ENV_ACCOUNT, ENV_ALLOWLIST, ENV_ENDPOINT, ENV_OWN_DEVICE_ID, SignalConfig,
    };
    use amplihack_signal::gating::Gate;
    use amplihack_signal::transport::Envelope;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    const GID: &str = "grp-abc123==";
    const ACCOUNT: &str = "+15551230000";
    const OPERATOR: &str = "+15551230001";

    fn cfg(allowlist: &[&str], own_device: Option<u32>) -> SignalConfig {
        let mut env = HashMap::new();
        env.insert(ENV_ENDPOINT.to_string(), "127.0.0.1:7583".to_string());
        env.insert(ENV_ACCOUNT.to_string(), ACCOUNT.to_string());
        env.insert(ENV_ALLOWLIST.to_string(), allowlist.join(","));
        if let Some(d) = own_device {
            env.insert(ENV_OWN_DEVICE_ID.to_string(), d.to_string());
        }
        SignalConfig::from_sources(&env, None).expect("valid test config")
    }

    fn data_msg(sender: &str, device: u32, body: &str) -> Envelope {
        Envelope {
            source: Some(sender.to_string()),
            source_device: Some(device),
            group_id: Some(GID.to_string()),
            body: Some(body.to_string()),
            is_sync: false,
        }
    }

    fn sync_msg(sender: &str, device: u32, body: &str) -> Envelope {
        Envelope {
            source: Some(sender.to_string()),
            source_device: Some(device),
            group_id: Some(GID.to_string()),
            body: Some(body.to_string()),
            is_sync: true,
        }
    }

    // INV-3 (inbound gating fail-closed). Consolidated anchor pinning the
    // deny-by-default posture: an inbound envelope becomes an instruction ONLY
    // on the single well-formed happy path, and every non-happy path DENIES.
    // Time is supplied through the deterministic `evaluate_at` / `record_outbound_at`
    // seams (fixed `Instant`s), so no wall-clock time enters the assertions. Any
    // future change that flips one of these denials into an accept — i.e. fails
    // OPEN — must break this test.
    #[test]
    fn characterization_inv3_inbound_gate_failclosed() {
        let now = Instant::now();

        // The ONLY accept: a well-formed, allowlisted operator dataMessage.
        let mut gate = Gate::new(&cfg(&[OPERATOR], None), GID);
        assert_eq!(
            gate.evaluate_at(&data_msg(OPERATOR, 1, "run the tests"), now),
            Some("run the tests".to_string()),
            "a well-formed allowlisted dataMessage is the canonical accept"
        );

        // Wrong group → deny.
        let mut gate = Gate::new(&cfg(&[OPERATOR], None), GID);
        let mut env = data_msg(OPERATOR, 1, "hi");
        env.group_id = Some("grp-OTHER==".to_string());
        assert_eq!(gate.evaluate_at(&env, now), None, "wrong group must deny");

        // Non-group frame → deny.
        let mut gate = Gate::new(&cfg(&[OPERATOR], None), GID);
        let mut env = data_msg(OPERATOR, 1, "hi");
        env.group_id = None;
        assert_eq!(
            gate.evaluate_at(&env, now),
            None,
            "non-group frame must deny"
        );

        // Empty body → deny.
        let mut gate = Gate::new(&cfg(&[OPERATOR], None), GID);
        assert_eq!(
            gate.evaluate_at(&data_msg(OPERATOR, 1, ""), now),
            None,
            "empty body must deny"
        );

        // Sender not on the allowlist → deny.
        let mut gate = Gate::new(&cfg(&[OPERATOR], None), GID);
        assert_eq!(
            gate.evaluate_at(&data_msg("+15559999999", 1, "hi"), now),
            None,
            "non-allowlisted sender must deny"
        );

        // Empty allowlist denies everything — even a would-be operator.
        let mut gate = Gate::new(&cfg(&[], None), GID);
        assert_eq!(
            gate.evaluate_at(&data_msg(OPERATOR, 1, "hi"), now),
            None,
            "fail-closed: an empty allowlist denies everything"
        );

        // Sync from a non-primary device (>= 2) → deny (device-1-only operator).
        let mut gate = Gate::new(&cfg(&[ACCOUNT], None), GID);
        assert_eq!(
            gate.evaluate_at(&sync_msg(ACCOUNT, 2, "session started"), now),
            None,
            "a sync from a non-primary device must deny"
        );

        // Sync from the bot's own configured linked device → deny (echo guard).
        let mut gate = Gate::new(&cfg(&[ACCOUNT], Some(3)), GID);
        assert_eq!(
            gate.evaluate_at(&sync_msg(ACCOUNT, 3, "session started"), now),
            None,
            "the bot's own linked-device sync echo must deny"
        );

        // Sync claiming device 1 but authored by a non-account number → deny.
        let mut gate = Gate::new(&cfg(&[ACCOUNT, "+15551230009"], None), GID);
        assert_eq!(
            gate.evaluate_at(&sync_msg("+15551230009", 1, "spoofed"), now),
            None,
            "a sync not authored by the account must deny"
        );

        // A body matching a recently-sent outbound, inside the echo TTL → deny.
        let mut gate = Gate::new(&cfg(&[OPERATOR], None), GID);
        gate.record_outbound_at("session started", now);
        assert_eq!(
            gate.evaluate_at(
                &data_msg(OPERATOR, 1, "session started"),
                now + Duration::from_secs(1),
            ),
            None,
            "a recently-sent outbound echo inside the TTL must deny"
        );
    }
}
