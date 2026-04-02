//! Session start classifier skill — orchestrates classification and execution.

use crate::cascade::ExecutionTierCascade;
use crate::classifier::{WorkflowClassifier, WorkflowType};
use crate::session::SessionStartDetector;
use serde_json::Value;
use std::time::Instant;
use tracing::{info, warn};

/// Orchestrates session start workflow classification and execution.
pub struct SessionStartClassifierSkill {
    classifier: WorkflowClassifier,
    cascade: ExecutionTierCascade,
    detector: SessionStartDetector,
}

impl Default for SessionStartClassifierSkill {
    fn default() -> Self {
        Self {
            classifier: WorkflowClassifier::default(),
            cascade: ExecutionTierCascade::default(),
            detector: SessionStartDetector::new(),
        }
    }
}

impl SessionStartClassifierSkill {
    pub fn new(
        classifier: WorkflowClassifier,
        cascade: ExecutionTierCascade,
        detector: SessionStartDetector,
    ) -> Self {
        Self {
            classifier,
            cascade,
            detector,
        }
    }

    /// Process session start: detect → classify → execute → announce.
    pub fn process(&self, context: &Value) -> Value {
        let start = Instant::now();

        // Check bypass
        if self.detector.should_bypass(context) {
            let reason = self.detector.bypass_reason(context).unwrap_or("unknown");
            return serde_json::json!({
                "activated": false,
                "should_classify": false,
                "bypassed": true,
                "reason": reason,
            });
        }

        // Check session start
        if !self.detector.is_session_start(context) {
            return serde_json::json!({
                "activated": false,
                "should_classify": false,
                "bypassed": false,
            });
        }

        // Extract user request
        let user_request = context
            .get("prompt")
            .or_else(|| context.get("user_request"))
            .and_then(Value::as_str)
            .unwrap_or("");

        if user_request.is_empty() {
            warn!("No user request in context");
            return serde_json::json!({
                "activated": false,
                "should_classify": false,
                "bypassed": false,
            });
        }

        // Classify
        let classification = self.classifier.classify(user_request);
        let recipe_available = self.cascade.is_recipe_runner_available();
        let announcement = self
            .classifier
            .format_announcement(&classification, recipe_available);

        info!(
            workflow = classification.workflow.as_str(),
            confidence = classification.confidence,
            "Classified session start"
        );

        // Execute via cascade for recipe-backed workflows
        let workflow = classification.workflow;
        let (tier, method, status) = if matches!(
            workflow,
            WorkflowType::Default | WorkflowType::Investigation
        ) {
            let exec = self.cascade.execute(workflow, context);
            (Some(exec.tier), exec.method, exec.status)
        } else {
            (None, "direct".to_string(), "success".to_string())
        };

        let classification_time = start.elapsed().as_secs_f64();

        serde_json::json!({
            "activated": true,
            "should_classify": true,
            "bypassed": false,
            "workflow": classification.workflow.as_str(),
            "reason": classification.reason,
            "confidence": classification.confidence,
            "keywords": classification.keywords,
            "tier": tier,
            "method": method,
            "status": status,
            "announcement": announcement,
            "classification_time": classification_time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn skill() -> SessionStartClassifierSkill {
        SessionStartClassifierSkill::default()
    }

    #[test]
    fn bypasses_slash_commands() {
        let s = skill();
        let r = s.process(&json!({
            "is_first_message": true,
            "prompt": "/dev fix it"
        }));
        assert_eq!(r["bypassed"], true);
        assert_eq!(r["activated"], false);
    }

    #[test]
    fn bypasses_follow_up() {
        let s = skill();
        let r = s.process(&json!({
            "is_first_message": false,
            "prompt": "also fix this"
        }));
        assert_eq!(r["bypassed"], true);
    }

    #[test]
    fn classifies_first_message() {
        let s = skill();
        let r = s.process(&json!({
            "is_first_message": true,
            "prompt": "implement a new logging system"
        }));
        assert_eq!(r["activated"], true);
        assert_eq!(r["workflow"], "DEFAULT_WORKFLOW");
        assert!(r["announcement"].as_str().unwrap().contains("DEFAULT"));
    }

    #[test]
    fn classifies_investigation() {
        let s = skill();
        let r = s.process(&json!({
            "is_first_message": true,
            "prompt": "investigate why CI is failing"
        }));
        assert_eq!(r["workflow"], "INVESTIGATION_WORKFLOW");
    }

    #[test]
    fn empty_prompt_not_activated() {
        let s = skill();
        let r = s.process(&json!({"is_first_message": true}));
        assert_eq!(r["activated"], false);
    }

    #[test]
    fn result_includes_classification_time() {
        let s = skill();
        let r = s.process(&json!({
            "is_first_message": true,
            "prompt": "fix the build"
        }));
        assert!(r["classification_time"].as_f64().unwrap() >= 0.0);
    }
}
