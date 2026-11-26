//! Issue #1983: a run stays on the CLI it started with. The only native fleet
//! reasoner starts `claude`, so under `auto` a run observed on another CLI gets
//! the heuristic reasoner instead. Naming `claude` is the explicit request.

use super::*;
use std::cell::Cell;

const OTHER_CLIS: [&str; 3] = ["codex", "copilot", "amplifier"];

#[test]
fn auto_in_a_run_on_another_cli_uses_the_heuristic_reasoner() {
    for cli in OTHER_CLIS {
        let backend = NativeReasonerBackend::detect_for("auto", Some(cli)).unwrap();
        assert!(
            matches!(&backend, NativeReasonerBackend::StaysOn(c) if c == cli),
            "{cli}: {backend:?}"
        );
        assert_eq!(backend.label(), "heuristic");
        let notice = backend.root_sandbox_notice().expect("a notice line");
        assert!(
            notice.contains(cli) && notice.contains("--backend claude"),
            "{notice}"
        );
    }
}

#[test]
fn a_backend_chosen_for_another_cli_never_builds_a_claude_command() {
    for cli in OTHER_CLIS {
        let backend = NativeReasonerBackend::StaysOn(cli.to_string());
        let configured = Cell::new(false);
        let result = backend.complete_with("prompt", &|_| configured.set(true));
        assert!(result.is_err(), "{cli}: {result:?}");
        assert!(!configured.get(), "{cli}: a child command was built");
    }
}

#[test]
fn auto_in_a_claude_run_or_with_no_observed_cli_behaves_as_before() {
    for observed in [Some("claude"), None] {
        let backend = NativeReasonerBackend::detect_for("auto", observed).unwrap();
        assert!(
            matches!(
                backend,
                NativeReasonerBackend::Claude(..) | NativeReasonerBackend::None
            ),
            "{observed:?}: {backend:?}"
        );
    }
}

#[test]
fn naming_claude_or_anthropic_still_asks_for_claude() {
    for requested in ["claude", "anthropic"] {
        for observed in OTHER_CLIS {
            let backend = NativeReasonerBackend::detect_for(requested, Some(observed)).unwrap();
            assert!(
                matches!(
                    backend,
                    NativeReasonerBackend::Claude(..) | NativeReasonerBackend::None
                ),
                "{requested} on {observed}: {backend:?}"
            );
        }
    }
}

#[test]
fn backends_that_are_not_implemented_are_still_an_error() {
    for requested in ["copilot", "litellm", "nonsense"] {
        assert!(NativeReasonerBackend::detect_for(requested, Some("codex")).is_err());
    }
}

#[test]
fn a_heuristic_reasoner_chosen_for_the_run_still_reasons_and_says_why() {
    let mut ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
    ctx.agent_status = AgentStatus::Running;
    ctx.tmux_capture = "working on it".to_string();
    let expected = heuristic_decision(&ctx);
    let reasoner = FleetSessionReasoner::new(
        PathBuf::from("/nonexistent/azlin"),
        NativeReasonerBackend::StaysOn("codex".to_string()),
    );
    let configured = Cell::new(false);
    let (decision, diagnostic) = reasoner.reason_with(&ctx, &|_| configured.set(true));
    assert!(!configured.get(), "a child command was built");
    assert_eq!(decision.action, expected.action);
    assert_eq!(decision.input_text, expected.input_text);
    let diagnostic = diagnostic.expect("the result says why claude was not started");
    assert!(
        diagnostic.contains("codex") && diagnostic.contains("--backend claude"),
        "{diagnostic}"
    );
}
