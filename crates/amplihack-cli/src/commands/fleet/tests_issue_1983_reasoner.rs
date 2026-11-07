//! Issue #1983: a run stays on the CLI it started with. The only native fleet
//! reasoner starts `claude`, so under `auto` a run observed on another CLI gets
//! the heuristic reasoner instead. Naming `claude` is the explicit request.

use super::*;

#[test]
fn auto_in_a_run_on_another_cli_does_not_start_claude() {
    for cli in ["codex", "copilot", "amplifier"] {
        let backend = NativeReasonerBackend::detect_for("auto", Some(cli)).unwrap();
        assert!(
            matches!(&backend, NativeReasonerBackend::StaysOn(c) if c == cli),
            "{cli}: {backend:?}"
        );
        assert_eq!(backend.label(), "heuristic");
        let notice = backend.root_sandbox_notice().expect("a notice");
        assert!(
            notice.contains(cli) && notice.contains("--backend claude"),
            "{notice}"
        );
        let err = backend.complete("prompt").unwrap_err().to_string();
        assert!(
            err.contains(cli) && err.contains("claude is not started"),
            "{err}"
        );
    }
}

#[test]
fn auto_in_a_claude_run_or_with_no_observed_cli_keeps_the_claude_backend() {
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
fn naming_claude_is_the_explicit_request_that_starts_claude() {
    for requested in ["claude", "anthropic"] {
        let backend = NativeReasonerBackend::detect_for(requested, Some("codex")).unwrap();
        assert!(
            !matches!(backend, NativeReasonerBackend::StaysOn(_)),
            "{requested}: {backend:?}"
        );
    }
}

#[test]
fn a_heuristic_reasoner_chosen_for_the_run_still_reasons() {
    let mut ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
    ctx.agent_status = AgentStatus::Running;
    ctx.tmux_capture = "working on it".to_string();
    let reasoner = FleetSessionReasoner::new(
        PathBuf::from("/nonexistent/azlin"),
        NativeReasonerBackend::StaysOn("codex".to_string()),
    );
    let (_decision, diagnostic) = reasoner.reason_with(&ctx, &|_| {});
    let diagnostic = diagnostic.expect("the diagnostic names why claude was not started");
    assert!(diagnostic.contains("codex"), "{diagnostic}");
}
