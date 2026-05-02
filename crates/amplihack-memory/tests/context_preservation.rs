// crates/amplihack-memory/tests/context_preservation.rs
//
// TDD: failing tests for ContextPreserver (port of
// amplifier-bundle/tools/amplihack/memory/context_preservation.py).

use amplihack_memory::context_preservation::{
    AgentDecision, ContextPreserver, ConversationContext, WorkflowState,
};
use tempfile::TempDir;

fn make_preserver(session: &str) -> (ContextPreserver, TempDir) {
    let dir = TempDir::new().unwrap();
    let p = ContextPreserver::with_db(session, dir.path().join("m.db")).unwrap();
    (p, dir)
}

#[test]
fn preserve_then_restore_conversation_context() {
    let (cp, _d) = make_preserver("sess-1");
    let ctx = ConversationContext {
        agent_id: "architect".into(),
        topic: "wave 3a".into(),
        messages: vec!["hi".into(), "ack".into()],
        metadata: serde_json::json!({"k": "v"}),
    };
    cp.preserve_conversation_context(&ctx).unwrap();
    let restored = cp
        .restore_conversation_context(Some("architect"))
        .unwrap()
        .expect("present");
    assert_eq!(restored.topic, "wave 3a");
    assert_eq!(restored.messages.len(), 2);
}

#[test]
fn restore_conversation_context_missing_returns_none() {
    let (cp, _d) = make_preserver("sess-2");
    assert!(
        cp.restore_conversation_context(Some("nobody"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn workflow_state_roundtrip_per_workflow() {
    let (cp, _d) = make_preserver("sess-3");
    cp.preserve_workflow_state(&WorkflowState {
        workflow_name: "default".into(),
        step: 7,
        data: serde_json::json!({"phase": "tdd"}),
    })
    .unwrap();
    cp.preserve_workflow_state(&WorkflowState {
        workflow_name: "consensus".into(),
        step: 2,
        data: serde_json::json!({"phase": "vote"}),
    })
    .unwrap();
    let d = cp.restore_workflow_state("default").unwrap().unwrap();
    assert_eq!(d.step, 7);
    let c = cp.restore_workflow_state("consensus").unwrap().unwrap();
    assert_eq!(c.step, 2);
}

#[test]
fn decision_history_returns_in_chronological_order() {
    let (cp, _d) = make_preserver("sess-4");
    for i in 0..3 {
        cp.preserve_agent_decision(&AgentDecision {
            agent_id: "reviewer".into(),
            decision: format!("d-{i}"),
            rationale: "because".into(),
            metadata: serde_json::Value::Null,
        })
        .unwrap();
    }
    let hist = cp.get_decision_history(Some("reviewer"), None).unwrap();
    assert_eq!(hist.len(), 3);
    assert_eq!(hist[0].decision, "d-0");
    assert_eq!(hist[2].decision, "d-2");
}

#[test]
fn decision_history_filter_by_agent() {
    let (cp, _d) = make_preserver("sess-5");
    cp.preserve_agent_decision(&AgentDecision {
        agent_id: "a".into(),
        decision: "x".into(),
        rationale: "".into(),
        metadata: serde_json::Value::Null,
    })
    .unwrap();
    cp.preserve_agent_decision(&AgentDecision {
        agent_id: "b".into(),
        decision: "y".into(),
        rationale: "".into(),
        metadata: serde_json::Value::Null,
    })
    .unwrap();
    let only_a = cp.get_decision_history(Some("a"), None).unwrap();
    assert_eq!(only_a.len(), 1);
    assert_eq!(only_a[0].decision, "x");
}

#[test]
fn cleanup_old_context_returns_deleted_count_zero_for_fresh_data() {
    let (cp, _d) = make_preserver("sess-6");
    cp.preserve_conversation_context(&ConversationContext {
        agent_id: "a".into(),
        topic: "t".into(),
        messages: vec![],
        metadata: serde_json::Value::Null,
    })
    .unwrap();
    let removed = cp.cleanup_old_context(7).unwrap();
    assert_eq!(removed, 0);
}
