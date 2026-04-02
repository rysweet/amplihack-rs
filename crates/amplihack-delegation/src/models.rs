use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Delegation status
// ---------------------------------------------------------------------------

/// Overall outcome of a delegation run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum DelegationStatus {
    /// Score ≥ 80.
    Success,
    /// Score ≥ 50 and < 80.
    Partial,
    /// Score < 50.
    Failure,
}

impl std::fmt::Display for DelegationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "SUCCESS"),
            Self::Partial => write!(f, "PARTIAL"),
            Self::Failure => write!(f, "FAILURE"),
        }
    }
}

// ---------------------------------------------------------------------------
// Evidence types
// ---------------------------------------------------------------------------

/// Categories of evidence that can be collected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    /// Source code files.
    CodeFile,
    /// Test source files.
    TestFile,
    /// General documentation (READMEs, guides).
    Documentation,
    /// Architecture decision records, design docs.
    ArchitectureDoc,
    /// OpenAPI / Swagger specifications.
    ApiSpec,
    /// Test result artifacts (XML, JSON).
    TestResults,
    /// Execution / run logs.
    ExecutionLog,
    /// QA or validation reports.
    ValidationReport,
    /// Diagrams (Mermaid, PlantUML, Graphviz).
    Diagram,
    /// Configuration files (YAML, TOML, JSON, INI).
    Configuration,
}

impl std::fmt::Display for EvidenceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::CodeFile => "code_file",
            Self::TestFile => "test_file",
            Self::Documentation => "documentation",
            Self::ArchitectureDoc => "architecture_doc",
            Self::ApiSpec => "api_spec",
            Self::TestResults => "test_results",
            Self::ExecutionLog => "execution_log",
            Self::ValidationReport => "validation_report",
            Self::Diagram => "diagram",
            Self::Configuration => "configuration",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// Evidence item
// ---------------------------------------------------------------------------

/// A single piece of collected evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    /// The kind of evidence.
    pub evidence_type: EvidenceType,
    /// Filesystem path (relative to working dir).
    pub path: String,
    /// Full file content (may be large).
    pub content: String,
    /// Short excerpt for display.
    pub excerpt: String,
    /// Size of the original file in bytes.
    pub size_bytes: u64,
    /// When the evidence was collected.
    pub timestamp: DateTime<Utc>,
    /// Arbitrary key-value metadata (language, line count, …).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Scenario types
// ---------------------------------------------------------------------------

/// Category of a test scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioCategory {
    /// Normal expected-success paths.
    HappyPath,
    /// Failure and error-recovery paths.
    ErrorHandling,
    /// Edge cases and limits.
    BoundaryConditions,
    /// Authentication, authorization, injection.
    Security,
    /// Throughput, latency, concurrency.
    Performance,
    /// End-to-end cross-component tests.
    Integration,
}

impl std::fmt::Display for ScenarioCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::HappyPath => "happy_path",
            Self::ErrorHandling => "error_handling",
            Self::BoundaryConditions => "boundary_conditions",
            Self::Security => "security",
            Self::Performance => "performance",
            Self::Integration => "integration",
        };
        write!(f, "{s}")
    }
}

/// A generated test scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestScenario {
    /// Human-readable scenario name.
    pub name: String,
    /// Which category this scenario belongs to.
    pub category: ScenarioCategory,
    /// Prose description of what the scenario tests.
    pub description: String,
    /// Conditions that must hold before execution.
    pub preconditions: Vec<String>,
    /// Ordered execution steps.
    pub steps: Vec<String>,
    /// What should happen when the steps complete.
    pub expected_outcome: String,
    /// Priority level (`"high"`, `"medium"`, `"low"`).
    pub priority: String,
    /// Free-form tags for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// Subprocess result
// ---------------------------------------------------------------------------

/// Outcome of a spawned subprocess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubprocessResult {
    /// Process exit code.
    pub exit_code: i32,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// Wall-clock duration in seconds.
    pub duration_secs: f64,
    /// OS-level process ID.
    pub subprocess_pid: u32,
    /// Whether the process was killed due to timeout.
    #[serde(default)]
    pub timed_out: bool,
    /// Number of orphan child processes cleaned up.
    #[serde(default)]
    pub orphans_cleaned: u32,
}

impl SubprocessResult {
    /// Returns `true` when the process exited successfully without timeout.
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }

    /// Returns `true` when the process was killed by a signal (negative exit code).
    pub fn crashed(&self) -> bool {
        self.exit_code < 0
    }
}

// ---------------------------------------------------------------------------
// Evaluation result
// ---------------------------------------------------------------------------

/// Outcome of evaluating success criteria against collected evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    /// Aggregate score 0–100.
    pub score: u32,
    /// Human-readable evaluation notes.
    pub notes: String,
    /// Requirements that were satisfied.
    #[serde(default)]
    pub requirements_met: Vec<String>,
    /// Requirements that were *not* satisfied.
    #[serde(default)]
    pub requirements_missing: Vec<String>,
    /// Extra credit points (tests, docs, etc.).
    #[serde(default)]
    pub bonus_points: u32,
}

impl EvaluationResult {
    /// Build a new result, clamping the score to 0–100.
    pub fn new(
        score: u32,
        notes: String,
        requirements_met: Vec<String>,
        requirements_missing: Vec<String>,
        bonus_points: u32,
    ) -> Self {
        Self {
            score: score.min(100),
            notes,
            requirements_met,
            requirements_missing,
            bonus_points,
        }
    }

    /// Determine the [`DelegationStatus`] from the score.
    pub fn status(&self) -> DelegationStatus {
        match self.score {
            80..=100 => DelegationStatus::Success,
            50..=79 => DelegationStatus::Partial,
            _ => DelegationStatus::Failure,
        }
    }
}

// ---------------------------------------------------------------------------
// Meta-delegation result
// ---------------------------------------------------------------------------

/// Top-level result of a meta-delegation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaDelegationResult {
    /// Overall status.
    pub status: DelegationStatus,
    /// Aggregate success score 0–100.
    pub success_score: u32,
    /// Collected evidence items.
    pub evidence: Vec<EvidenceItem>,
    /// Combined execution log text.
    pub execution_log: String,
    /// Wall-clock duration of the entire delegation.
    pub duration_secs: f64,
    /// Which persona was used.
    pub persona_used: String,
    /// Which platform CLI was used.
    pub platform_used: String,
    /// Reason for failure, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// Notes when partially completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_completion_notes: Option<String>,
    /// PID of the spawned subprocess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subprocess_pid: Option<u32>,
    /// Generated test scenarios.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_scenarios: Option<Vec<TestScenario>>,
}

impl MetaDelegationResult {
    /// Return evidence items matching `evidence_type`.
    pub fn get_evidence_by_type(&self, evidence_type: &EvidenceType) -> Vec<&EvidenceItem> {
        self.evidence
            .iter()
            .filter(|e| &e.evidence_type == evidence_type)
            .collect()
    }

    /// Serialize to a JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialize from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

// Tests moved to tests/models_test.rs to stay under 400 lines.
