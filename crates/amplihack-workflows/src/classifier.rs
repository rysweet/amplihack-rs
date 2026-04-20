//! Keyword-based workflow classifier.
//!
//! Routes user requests into 4 workflow types based on keyword matching.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::trace;

use crate::provenance::{self, ProvenanceEntry};

/// The four workflow types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkflowType {
    #[serde(rename = "DEFAULT_WORKFLOW")]
    Default,
    #[serde(rename = "INVESTIGATION_WORKFLOW")]
    Investigation,
    #[serde(rename = "OPS_WORKFLOW")]
    Ops,
    #[serde(rename = "Q&A_WORKFLOW")]
    QAndA,
}

impl WorkflowType {
    /// The workflow name string (matches Python).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "DEFAULT_WORKFLOW",
            Self::Investigation => "INVESTIGATION_WORKFLOW",
            Self::Ops => "OPS_WORKFLOW",
            Self::QAndA => "Q&A_WORKFLOW",
        }
    }

    /// Short display name (without _WORKFLOW suffix).
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Default => "DEFAULT",
            Self::Investigation => "INVESTIGATION",
            Self::Ops => "OPS",
            Self::QAndA => "Q&A",
        }
    }

    /// Corresponding recipe name, if any.
    pub fn recipe_name(&self) -> Option<&'static str> {
        match self {
            Self::Default => Some("default-workflow"),
            Self::Investigation => Some("investigation-workflow"),
            Self::Ops | Self::QAndA => None,
        }
    }
}

/// Result of classifying a user request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationResult {
    pub workflow: WorkflowType,
    pub reason: String,
    pub confidence: f64,
    pub keywords: Vec<String>,
}

/// Classifies user requests into appropriate workflows.
pub struct WorkflowClassifier {
    keyword_map: HashMap<WorkflowType, Vec<String>>,
    /// Base directory for provenance logs (typically the project root).
    /// When `None`, provenance logging is disabled.
    log_base_dir: Option<PathBuf>,
}

impl Default for WorkflowClassifier {
    fn default() -> Self {
        Self::new(None)
    }
}

impl WorkflowClassifier {
    /// Create a new classifier with optional custom keywords.
    pub fn new(custom_keywords: Option<HashMap<WorkflowType, Vec<String>>>) -> Self {
        let mut keyword_map = default_keyword_map();
        if let Some(custom) = custom_keywords {
            for (wf, kws) in custom {
                keyword_map.entry(wf).or_default().extend(kws);
            }
        }
        Self {
            keyword_map,
            log_base_dir: None,
        }
    }

    /// Enable provenance logging to the given base directory.
    pub fn with_log_dir(mut self, base_dir: impl Into<PathBuf>) -> Self {
        self.log_base_dir = Some(base_dir.into());
        self
    }

    /// Classify a user request. Returns a low-confidence default result for
    /// empty or whitespace-only input rather than panicking.
    pub fn classify(&self, request: &str) -> ClassificationResult {
        if request.trim().is_empty() {
            return ClassificationResult {
                workflow: WorkflowType::Default,
                reason: "empty request".into(),
                confidence: 0.0,
                keywords: vec![],
            };
        }

        let keywords = self.extract_keywords(request);
        let (mut workflow, mut reason, mut confidence) = self.classify_by_keywords(&keywords);

        // #269: Constructive-verb override — when a request matches OPS keywords
        // but also contains a constructive verb (add, create, build, etc.), the
        // user is building something, not doing ops. Override to DEFAULT.
        if workflow == WorkflowType::Ops && Self::has_constructive_verb(request) {
            trace!(
                original_workflow = "OPS",
                override_to = "DEFAULT",
                "constructive verb override: request contains ops keyword but is constructive"
            );
            workflow = WorkflowType::Default;
            reason = format!("{reason} (overridden: constructive verb detected)");
            confidence = 0.75;
        }
        let (workflow, reason, confidence) = (workflow, reason, confidence);

        trace!(
            workflow = workflow.as_str(),
            confidence, "classified request"
        );

        let result = ClassificationResult {
            workflow,
            reason,
            confidence,
            keywords,
        };

        if let Some(base) = &self.log_base_dir {
            let entry = ProvenanceEntry::new(
                "classification",
                result.workflow.as_str(),
                &result.reason,
                result.confidence,
                result.keywords.clone(),
                request,
            );
            provenance::log_classification(base, &entry);
        }

        result
    }

    /// Format a user-facing announcement of the classification.
    pub fn format_announcement(
        &self,
        result: &ClassificationResult,
        recipe_runner_available: bool,
    ) -> String {
        let display = result.workflow.display_name();
        let mut ann = format!(
            "WORKFLOW: {display}\nReason: {}\nFollowing: .claude/workflow/{}.md",
            result.reason,
            result.workflow.as_str()
        );

        if recipe_runner_available && let Some(recipe) = result.workflow.recipe_name() {
            ann.push_str(&format!("\nExecution: Recipe Runner (tier 1) - {recipe}"));
        }
        ann
    }

    fn extract_keywords(&self, request: &str) -> Vec<String> {
        let lower = request.to_lowercase();
        let mut matched = Vec::new();
        for keywords in self.keyword_map.values() {
            for kw in keywords {
                if lower.contains(kw.as_str()) {
                    matched.push(kw.clone());
                }
            }
        }
        matched
    }

    /// Constructive verbs that indicate the user wants to build/change something,
    /// not perform an ops task. When the request contains one of these AND an OPS
    /// keyword, DEFAULT wins. See issue #269.
    const CONSTRUCTIVE_VERBS: &[&str] = &[
        "add",
        "create",
        "build",
        "implement",
        "write",
        "design",
        "develop",
        "make",
        "introduce",
        "extend",
        "enhance",
        "refactor",
    ];

    fn classify_by_keywords(&self, keywords: &[String]) -> (WorkflowType, String, f64) {
        // Priority: DEFAULT > INVESTIGATION > OPS > Q&A
        let priority = [
            WorkflowType::Default,
            WorkflowType::Investigation,
            WorkflowType::Ops,
            WorkflowType::QAndA,
        ];

        for wf in &priority {
            if let Some(wf_keywords) = self.keyword_map.get(wf) {
                let matched: Vec<&String> = keywords
                    .iter()
                    .filter(|kw| wf_keywords.contains(kw))
                    .collect();
                if !matched.is_empty() {
                    let confidence = if matched.len() > 1 { 0.9 } else { 0.7 };
                    let reason = format!("keyword '{}'", matched[0]);
                    return (*wf, reason, confidence);
                }
            }
        }

        (
            WorkflowType::Default,
            "ambiguous request, defaulting to default workflow".into(),
            0.5,
        )
    }

    /// Returns `true` when the request text contains a constructive verb,
    /// indicating the user wants to build/modify something (not an ops task).
    fn has_constructive_verb(request: &str) -> bool {
        let lower = request.to_lowercase();
        Self::CONSTRUCTIVE_VERBS
            .iter()
            .any(|&verb| lower.contains(verb))
    }
}

fn default_keyword_map() -> HashMap<WorkflowType, Vec<String>> {
    let mut m = HashMap::new();
    m.insert(
        WorkflowType::QAndA,
        vec![
            "what is",
            "explain briefly",
            "quick question",
            "how do i run",
            "what does",
            "can you explain",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    m.insert(
        WorkflowType::Ops,
        vec![
            "run command",
            "disk cleanup",
            "repo management",
            "git operations",
            "delete files",
            "cleanup",
            "organize files",
            "clean up",
            "manage infrastructure",
            "manage deployment",
            "manage servers",
            "manage resources",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    m.insert(
        WorkflowType::Investigation,
        vec![
            "investigate",
            "understand",
            "analyze",
            "research",
            "explore",
            "how does",
            "how it works",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    m.insert(
        WorkflowType::Default,
        vec![
            "implement",
            "add",
            "fix",
            "create",
            "refactor",
            "update",
            "build",
            "develop",
            "remove",
            "delete",
            "modify",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_development_request() {
        let c = WorkflowClassifier::default();
        let r = c.classify("implement a new feature for logging");
        assert_eq!(r.workflow, WorkflowType::Default);
        assert!(r.confidence >= 0.7);
    }

    #[test]
    fn classifies_investigation_request() {
        let c = WorkflowClassifier::default();
        let r = c.classify("investigate why the tests are failing");
        assert_eq!(r.workflow, WorkflowType::Investigation);
    }

    #[test]
    fn classifies_ops_request() {
        let c = WorkflowClassifier::default();
        let r = c.classify("disk cleanup of temp files");
        assert_eq!(r.workflow, WorkflowType::Ops);
    }

    // ── #269: Misclassification regression tests ──

    #[test]
    fn issue_269_add_manage_users_is_default_not_ops() {
        let c = WorkflowClassifier::default();
        let r = c.classify("Add a feature to manage users in the admin panel");
        assert_eq!(
            r.workflow,
            WorkflowType::Default,
            "constructive 'add' + 'manage' should classify as DEFAULT, not OPS"
        );
    }

    #[test]
    fn issue_269_create_management_dashboard_is_default() {
        let c = WorkflowClassifier::default();
        let r = c.classify("Create a management dashboard for monitoring");
        assert_eq!(r.workflow, WorkflowType::Default);
    }

    #[test]
    fn issue_269_implement_cleanup_policy_is_default() {
        let c = WorkflowClassifier::default();
        // "cleanup" is OPS, but "implement" is constructive → DEFAULT wins
        let r = c.classify("Implement a cleanup policy for expired sessions");
        assert_eq!(r.workflow, WorkflowType::Default);
    }

    #[test]
    fn issue_269_legitimate_ops_manage_infrastructure() {
        let c = WorkflowClassifier::default();
        let r = c.classify("manage infrastructure for the production cluster");
        assert_eq!(
            r.workflow,
            WorkflowType::Ops,
            "pure ops task should stay as OPS"
        );
    }

    #[test]
    fn issue_269_legitimate_ops_disk_cleanup() {
        let c = WorkflowClassifier::default();
        let r = c.classify("disk cleanup of /var/log files");
        assert_eq!(r.workflow, WorkflowType::Ops);
    }

    #[test]
    fn issue_269_build_cleanup_tool_is_default() {
        let c = WorkflowClassifier::default();
        let r = c.classify("Build a cleanup tool for database records");
        assert_eq!(r.workflow, WorkflowType::Default);
    }

    #[test]
    fn issue_269_constructive_verb_override_has_reasonable_confidence() {
        let c = WorkflowClassifier::default();
        let r = c.classify("Add a cleanup feature to the admin panel");
        assert_eq!(r.workflow, WorkflowType::Default);
        assert!(
            r.confidence >= 0.5,
            "overridden classification should have reasonable confidence"
        );
    }

    #[test]
    fn classifies_qa_request() {
        let c = WorkflowClassifier::default();
        let r = c.classify("what is the purpose of this module");
        assert_eq!(r.workflow, WorkflowType::QAndA);
    }

    #[test]
    fn defaults_to_default_workflow() {
        let c = WorkflowClassifier::default();
        let r = c.classify("do something with the system");
        assert_eq!(r.workflow, WorkflowType::Default);
        assert!(r.confidence < 0.7);
    }

    #[test]
    fn default_takes_priority_over_ops() {
        let c = WorkflowClassifier::default();
        // "delete files" matches OPS but "delete" also matches DEFAULT
        let r = c.classify("delete files and fix the build");
        assert_eq!(r.workflow, WorkflowType::Default);
    }

    #[test]
    fn custom_keywords_extend() {
        let mut custom = HashMap::new();
        custom.insert(WorkflowType::QAndA, vec!["tell me about".to_string()]);
        let c = WorkflowClassifier::new(Some(custom));
        let r = c.classify("tell me about the architecture");
        assert_eq!(r.workflow, WorkflowType::QAndA);
    }

    #[test]
    fn format_announcement_basic() {
        let c = WorkflowClassifier::default();
        let r = c.classify("fix the broken tests");
        let ann = c.format_announcement(&r, false);
        assert!(ann.contains("DEFAULT"));
        assert!(ann.contains("Reason:"));
    }

    #[test]
    fn format_announcement_with_recipe() {
        let c = WorkflowClassifier::default();
        let r = c.classify("fix the tests");
        let ann = c.format_announcement(&r, true);
        assert!(ann.contains("Recipe Runner"));
        assert!(ann.contains("default-workflow"));
    }

    #[test]
    fn recipe_name_mapping() {
        assert_eq!(
            WorkflowType::Default.recipe_name(),
            Some("default-workflow")
        );
        assert_eq!(
            WorkflowType::Investigation.recipe_name(),
            Some("investigation-workflow")
        );
        assert_eq!(WorkflowType::Ops.recipe_name(), None);
        assert_eq!(WorkflowType::QAndA.recipe_name(), None);
    }
}
