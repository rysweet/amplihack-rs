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
        let (workflow, reason, confidence) = if requires_repository_edit(request) {
            (
                WorkflowType::Default,
                "requested repository edit".into(),
                0.9,
            )
        } else if administrative_action(request) {
            (
                WorkflowType::Ops,
                "administrative action or answer destination".into(),
                0.9,
            )
        } else {
            self.classify_by_keywords(&keywords)
        };

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
}

/// True when the request explicitly asks for a tracked repository file to
/// change (issue #2553).
///
/// Conservative by construction: merely mentioning a source path, a coverage
/// figure or an existing pull request is not enough. A clause must carry an
/// edit verb as a whole word *and* name an edit destination. The request is
/// split on `;` and newline so that one administrative clause cannot vouch
/// for an editing clause, or the reverse.
///
/// A clause whose object is administrative — creating or assigning an issue,
/// reporting coverage, coordinating a review — stays administrative even when
/// its subject is code, unless the same clause also conjoins an explicit edit
/// ("and update", "and commit"). "Create an issue about `src/router.rs`" is
/// therefore not a repository edit, while "... and update it" is.
///
/// Shared with `commands::orch::reclassify_task_type`, so the hook-level and
/// decomposition-level answers cannot drift apart.
pub fn requires_repository_edit(request: &str) -> bool {
    let lower = request.to_ascii_lowercase();
    let edit = [
        "rename",
        "change",
        "fix",
        "correct",
        "remove",
        "replace",
        "edit",
        "update",
        "write",
        "commit",
        "implement",
        "refactor",
        "extend",
        "add",
        "build",
        "create",
        "port",
    ];
    // `documentation` on its own is deliberately absent: writing an answer
    // *about* documentation into chat edits no tracked file. The destinations
    // here all name a file, a path or an explicit tracked-file scope.
    let destination = [
        "tracked",
        "docs/",
        "readme.md",
        "documentation file",
        "documentation files",
        "across the docs",
        "src/",
        "crates/",
        "tests/",
        "source files",
        "code and docs",
        ".rs",
        ".md",
        ".yaml",
        ".toml",
    ];
    // Administrative objects remain administrative even when their subject is code.
    let administration = [
        "create an issue",
        "create a triage issue",
        "open an issue",
        "open a cross-check issue",
        "assign issue",
        "report unit test",
        "report test coverage",
        "coordinate review",
    ];
    let clauses = lower.replace(", then ", ";").replace(" then ", ";");
    clauses.split([';', '\n']).any(|clause| {
        let report_destination = [
            "in chat",
            "in the status issue",
            "in the coordination issue",
            "in an issue",
            "issue comment",
        ]
        .iter()
        .any(|destination| clause.contains(destination));
        let report_object = [
            "answers",
            "findings",
            "status report",
            "write up",
            "write-up",
            "report about",
        ]
        .iter()
        .any(|object| clause.contains(object));
        let admin = administration.iter().any(|s| clause.contains(s))
            || (report_destination && report_object);
        let explicit_edit = [
            "and edit",
            "and update",
            "and commit",
            "and remove",
            "and rename",
            "and replace",
            "and correct",
            "and refactor",
            "and implement",
        ]
        .iter()
        .any(|s| clause.contains(s));
        (!admin || explicit_edit)
            && edit.iter().any(|verb| {
                clause
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|word| word == *verb)
            })
            && destination.iter().any(|path| clause.contains(path))
    }) || [
        "write tests",
        "add tests",
        "add a test",
        "open a pr",
        "open pr",
    ]
    .iter()
    .any(|s| lower.contains(s))
}

/// Administrative objects: triage, issue bookkeeping, coordination, and an
/// answer written for a person rather than into a tracked file.
///
/// Fix #269 narrowed the OPS keyword list because a generic word was stealing
/// development work, and that narrowing is not reopened here: every trigger
/// below names an administrative object rather than a bare verb or noun. A
/// bare `assign` would claim "refactor the assignment helper", which assigns a
/// value to a local, and a bare `status` would claim "fix the status bar
/// rendering", which is a bug report.
pub fn administrative_action(request: &str) -> bool {
    let lower = request.to_ascii_lowercase();
    [
        "triage",
        "open an issue",
        "create an issue",
        "assign the issue",
        "assign it to",
        "status issue",
        "report status",
        "status report",
        "coordina",
        "answers",
        "in chat",
        "question set",
    ]
    .iter()
    .any(|s| lower.contains(s))
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
    // OPS keywords must be specific multi-word phrases to avoid stealing
    // development tasks that incidentally mention "cleanup" or "manage"
    // (fix #269). Single generic words like "cleanup" and "manage" matched
    // as substrings inside code paths (e.g. "cmd_cleanup.rs") and task
    // descriptions ("Add ... disk-cleanup loop"), causing misclassification.
    m.insert(
        WorkflowType::Ops,
        vec![
            "run command",
            "disk cleanup",
            "repo management",
            "git operations",
            "delete files",
            "organize files",
            "clean up temp",
            "manage repos",
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
    fn multi_req_add_task_not_misclassified_as_ops() {
        // Regression test for #269: a complex "Add" task with code references
        // like "cmd_cleanup.rs" must not be stolen by OPS keywords.
        let c = WorkflowClassifier::default();
        let r = c.classify(
            "Add an agentic disk-cleanup loop. Extend src/cmd_cleanup.rs with a new function.",
        );
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

    /// Issue #2553 — the positive calibration set. An administrative request
    /// that carries no repository-edit signal is OPS, and OPS has no recipe,
    /// so it starts no workflow, takes no worktree and opens no PR.
    ///
    /// The third row is the 13 November request that motivated the issue; it
    /// exercises the UNDERSTAND path, which rule 2 or rule 5 of the routing
    /// prompt would otherwise claim.
    #[test]
    fn issue_2553_administrative_requests_classify_as_ops() {
        let classifier = WorkflowClassifier::default();
        for request in [
            "Triage these issues",
            "Open an issue for X and assign Y",
            "Research these issues and write up the answers",
            "Write up the answers to these questions in chat",
            "Create an issue about src/router.rs and assign its owner",
            "Research this question set and report the answers in an issue",
            "Update the project status in the coordination issue",
            "Coordinate review of the open pull requests",
            "Report unit test coverage in the status issue",
        ] {
            let result = classifier.classify(request);
            assert_eq!(result.workflow, WorkflowType::Ops, "{request}");
            assert_eq!(
                result.workflow.recipe_name(),
                None,
                "an OPS classification starts no workflow: {request}"
            );
        }
    }

    /// The negative calibration set. Fix #269 narrowed the OPS keywords
    /// because a generic word stole development work; the new route must not
    /// reopen that. Each row is a near neighbour of a row above.
    #[test]
    fn issue_2553_near_neighbour_edit_requests_stay_on_the_default_workflow() {
        let classifier = WorkflowClassifier::default();
        for request in [
            "Rename term X across the tracked docs of repos A and B",
            "Correct the wording in tracked docs/routing.md",
            "Commit the research answers to README.md",
            "Remove the stale entries from docs/",
            "Fix the typo in README",
            "Rename the config key across crates/amplihack-workflows/",
            "Add tests for the issue parser",
            "Open a PR for the routing fix",
            // `assign` as a bare substring used to steal this one.
            "Refactor the assignment helper",
            // `status` as a bare substring used to steal this one.
            "Fix the status bar rendering",
        ] {
            assert_eq!(
                classifier.classify(request).workflow,
                WorkflowType::Default,
                "{request}"
            );
        }
    }

    /// `documentation` as a bare destination used to steal this one: writing
    /// an answer about documentation into chat edits no tracked file.
    #[test]
    fn issue_2553_answering_about_documentation_is_not_a_repository_edit() {
        let classifier = WorkflowClassifier::default();
        for request in [
            "Write up the documentation gaps in chat",
            "Summarise the documentation gaps in the triage issue",
        ] {
            assert_eq!(
                classifier.classify(request).workflow,
                WorkflowType::Ops,
                "{request}"
            );
            assert!(!requires_repository_edit(request), "{request}");
        }
    }

    #[test]
    fn issue_2553_report_destination_controls_routing_even_when_the_subject_names_a_file() {
        let classifier = WorkflowClassifier::default();
        for request in [
            "Write the research findings about docs/routing.md in the status issue.",
            "Write up the answers about src/router.rs in chat.",
            "Create a status report about README.md in the coordination issue.",
        ] {
            assert_eq!(
                classifier.classify(request).workflow,
                WorkflowType::Ops,
                "{request}"
            );
            assert!(!requires_repository_edit(request), "{request}");
        }
    }

    #[test]
    fn issue_2553_administrative_prefixes_do_not_hide_repository_edit_workstreams() {
        let classifier = WorkflowClassifier::default();
        for request in [
            "Open an issue about src/router.rs, then correct docs/routing.md.",
            "Create an issue about the wording and correct docs/routing.md.",
            "Create a triage issue for the wording sweep, then replace the name across tracked documentation files.",
        ] {
            assert_eq!(
                classifier.classify(request).workflow,
                WorkflowType::Default,
                "{request}"
            );
            assert!(requires_repository_edit(request), "{request}");
        }
    }

    /// Blank input keeps its pre-existing answer: DEFAULT at zero confidence,
    /// decided before either gate runs.
    #[test]
    fn issue_2553_blank_input_is_unchanged_by_the_new_gates() {
        let classifier = WorkflowClassifier::default();
        for request in ["", "   ", "\n\t"] {
            let result = classifier.classify(request);
            assert_eq!(result.workflow, WorkflowType::Default, "{request:?}");
            assert_eq!(result.confidence, 0.0, "{request:?}");
        }
    }
}
