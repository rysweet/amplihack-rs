//! `governor status` `token_share` from the workstream usage manifest
//! (`AMPLIHACK_GOVERNOR_MANIFEST`, `manifest::token_shares`).
//!
//! Kept apart from `tests_status.rs` so each test file stays within the
//! 300-line module budget (`tests/cycle2_module_budget.rs`).

use serde_json::{Value, json};

#[test]
fn token_share_is_empty_when_a_manifest_session_has_no_usage_adapter() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("log"),
        r#"{"type":"assistant","sessionId":"s","timestamp":"2026-01-01T00:00:00Z","message":{"id":"msg_1","model":"model","usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}
"#,
    )
    .unwrap();
    let path = tmp.path().join("usage.json");
    let share = |sessions: Value| {
        let m = json!({"version":1,"workstream":"ws-1","repository":"o/r",
            "branch":"b","prs":[1],"sessions":sessions});
        std::fs::write(&path, m.to_string()).unwrap();
        super::manifest::token_shares(&path)
    };
    let claude = json!({"id":"s","provider":"claude","run_id":"r",
        "complete":true,"files":["log"]});
    // The control: a manifest of supported providers yields a share.
    assert_eq!(share(json!([claude])).get("ws-1").copied(), Some(1.0));
    // The same manifest plus a session with no adapter yields none, rather
    // than a share computed without that session's unknown tokens.
    let other = json!({"id":"o","provider":"other-agent","run_id":"r",
        "complete":true,"files":["log"]});
    assert!(share(json!([claude, other])).is_empty());
}
