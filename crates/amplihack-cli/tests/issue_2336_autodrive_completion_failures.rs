//! Terminal evaluator stops retain their classifications after dispatch moves to Rust.

use super::fixture::{Drive, json_output};
use crate::support::auto_drive_light::show;

#[test]
fn evaluator_refusal_orphaning_and_usage_failure_keep_their_stop_classifications() {
    for (rc, token, message) in [
        (
            79,
            "TERMINAL_POLICY_REFUSAL",
            "BLOCKED_TERMINAL orchestration_unavailable",
        ),
        (
            75,
            "ORPHANED",
            "orphaned: parent recipe run exited; stopping",
        ),
        (
            1,
            "USAGE_LIMIT",
            "amplihack.recipe.failure_class {\"class\":\"usage_limit\"}",
        ),
    ] {
        let d = Drive::new();
        d.set("review.json", r#"{"crusty_verdict":"CONCERNS","concerns":[{"id":"wrong-value","severity":"major","summary":"Wrong value","evidence":"lib.rs:2"}]}"#);
        d.record(1);
        let p = d.prepare(1, "");
        d.decide(&p, "evaluate");
        d.set("eval-rc", &rc.to_string());
        d.set("eval-message", message);
        let o = d.dispatch(&p, 0);
        assert!(!o.status.success(), "{token}: {}", show(&o));
        assert_eq!(
            json_output(&o)["loop_result"],
            token,
            "failure is not evaluator STUCK: {}",
            show(&o)
        );
        if rc != 1 {
            assert_eq!(o.status.code(), Some(rc));
        }
        assert_eq!(
            d.evaluators().len(),
            1,
            "terminal evaluator failure is not retried"
        );
        assert!(!d.marked());
    }
}
