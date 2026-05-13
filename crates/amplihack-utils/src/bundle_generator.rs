//! Agent Bundle Generator.
//!
//! Ported from `amplihack/bundle_generator/`.
//!
//! Provides types, error handling, and the core API for generating, testing,
//! and packaging AI agent bundles from natural language descriptions.
//!
//! ## Architecture
//!
//! The pipeline stages mirror the Python implementation:
//!
//! 1. **Parsing** — analyse natural language prompts ([`PromptParser`])
//! 2. **Extraction** — extract intent and requirements ([`IntentExtractor`])
//! 3. **Generation** — create agent content ([`AgentGenerator`])
//! 4. **Building** — assemble bundles ([`BundleBuilder`])
//! 5. **Packaging** — produce distributable packages ([`FilesystemPackager`])
//! 6. **Distribution** — publish to GitHub ([`GitHubDistributor`])

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error hierarchy
// ---------------------------------------------------------------------------

/// Errors from bundle generator operations.
#[derive(Debug, Error)]
pub enum BundleGeneratorError {
    /// Prompt parsing failed.
    #[error("[PARSING_FAILED] {message}")]
    Parsing {
        /// Human-readable description.
        message: String,
        /// Fragment of the prompt that caused the issue.
        prompt_fragment: Option<String>,
        /// Character position.
        position: Option<usize>,
    },

    /// Intent extraction failed.
    #[error("[EXTRACTION_FAILED] {message}")]
    Extraction {
        /// Human-readable description.
        message: String,
        /// Terms that could not be interpreted.
        ambiguous_terms: Vec<String>,
        /// Extraction confidence (0.0–1.0).
        confidence: Option<f64>,
    },

    /// Agent content generation failed.
    #[error("[GENERATION_FAILED] {message}")]
    Generation {
        /// Human-readable description.
        message: String,
        /// Name of the agent being generated.
        agent_name: Option<String>,
        /// Pipeline stage that failed.
        stage: Option<String>,
    },

    /// Bundle validation failed.
    #[error("[VALIDATION_FAILED] {message}")]
    Validation {
        /// Human-readable description.
        message: String,
        /// Validation category.
        validation_type: String,
        /// Individual failures.
        failures: Vec<String>,
    },

    /// Bundle packaging failed.
    #[error("[PACKAGING_FAILED] {message}")]
    Packaging {
        /// Human-readable description.
        message: String,
        /// Target format.
        format: Option<String>,
        /// File path involved.
        path: Option<String>,
    },

    /// Distribution failed.
    #[error("[DISTRIBUTION_FAILED] {message}")]
    Distribution {
        /// Human-readable description.
        message: String,
        /// Target platform.
        platform: Option<String>,
        /// HTTP status code, if applicable.
        http_status: Option<u16>,
    },

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl BundleGeneratorError {
    /// Suggested recovery action for this error.
    pub fn recovery_suggestion(&self) -> &str {
        match self {
            Self::Parsing { .. } => {
                "Check prompt syntax and structure. Ensure clear agent descriptions."
            }
            Self::Extraction { .. } => {
                "Provide clearer agent requirements. Use specific action verbs and clear role definitions."
            }
            Self::Generation { .. } => {
                "Try simplifying agent requirements or generating agents individually."
            }
            Self::Validation { .. } => {
                "Review validation failures and correct the identified issues."
            }
            Self::Packaging { .. } => "Check file permissions and available disk space.",
            Self::Distribution { .. } => {
                "Check network connectivity and authentication. Verify repository permissions."
            }
            Self::Io(_) | Self::Json(_) => "Check file system state and retry.",
        }
    }
}

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

/// Result of parsing a natural language prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedPrompt {
    /// Original prompt text.
    pub raw_prompt: String,
    /// Tokenised words.
    pub tokens: Vec<String>,
    /// Sentence segments.
    pub sentences: Vec<String>,
    /// Key phrases extracted from the prompt.
    pub key_phrases: Vec<String>,
    /// Named entities grouped by type.
    pub entities: HashMap<String, Vec<String>>,
    /// Parsing confidence (0.0–1.0).
    pub confidence: f64,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl ParsedPrompt {
    /// Validate the parsed prompt.
    pub fn validate(&self) -> Result<(), BundleGeneratorError> {
        if !(0.0..=1.0).contains(&self.confidence) {
            return Err(BundleGeneratorError::Parsing {
                message: format!(
                    "Confidence must be between 0 and 1, got {}",
                    self.confidence
                ),
                prompt_fragment: None,
                position: None,
            });
        }
        if self.raw_prompt.trim().is_empty() {
            return Err(BundleGeneratorError::Parsing {
                message: "Raw prompt cannot be empty".into(),
                prompt_fragment: None,
                position: None,
            });
        }
        Ok(())
    }
}

/// Action to perform with the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BundleAction {
    /// Create new agents.
    Create,
    /// Modify existing agents.
    Modify,
    /// Combine multiple agents.
    Combine,
    /// Specialise an agent for a domain.
    Specialize,
}

/// Complexity tier for a bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Complexity {
    /// Minimal configuration.
    Simple,
    /// Standard multi-agent bundle.
    Standard,
    /// Complex multi-agent bundle with dependencies.
    Advanced,
}

/// Agent type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentType {
    /// Core infrastructure agent.
    Core,
    /// Domain-specific agent.
    Specialized,
    /// Workflow orchestration agent.
    Workflow,
}

/// Requirements for a single agent within a bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequirement {
    /// Agent name (alphanumeric + hyphens/underscores).
    pub name: String,
    /// Role description.
    pub role: String,
    /// Purpose statement.
    pub purpose: String,
    /// List of capabilities.
    pub capabilities: Vec<String>,
    /// Constraints on the agent.
    #[serde(default)]
    pub constraints: Vec<String>,
    /// Suggested type.
    #[serde(default = "default_agent_type")]
    pub suggested_type: AgentType,
    /// Dependencies on other agents.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Priority (0 = highest).
    #[serde(default)]
    pub priority: u32,
}

fn default_agent_type() -> AgentType {
    AgentType::Specialized
}

impl AgentRequirement {
    /// Validate the agent requirement.
    pub fn validate(&self) -> Result<(), BundleGeneratorError> {
        let clean = self.name.replace(['-', '_'], "");
        if !clean.chars().all(|c| c.is_alphanumeric()) {
            return Err(BundleGeneratorError::Validation {
                message: format!(
                    "Agent name must be alphanumeric with hyphens/underscores: {}",
                    self.name
                ),
                validation_type: "agent_name".into(),
                failures: vec![self.name.clone()],
            });
        }
        if self.capabilities.is_empty() {
            return Err(BundleGeneratorError::Validation {
                message: format!("Agent {} must have at least one capability", self.name),
                validation_type: "capabilities".into(),
                failures: vec![],
            });
        }
        Ok(())
    }
}

/// Extracted intent from a parsed prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedIntent {
    /// Action to perform.
    pub action: BundleAction,
    /// Domain (e.g. "security", "data-processing").
    pub domain: String,
    /// Number of agents to generate.
    pub agent_count: usize,
    /// Per-agent requirements.
    pub agent_requirements: Vec<AgentRequirement>,
    /// Complexity tier.
    pub complexity: Complexity,
    /// Global constraints.
    pub constraints: Vec<String>,
    /// Global dependencies.
    pub dependencies: Vec<String>,
    /// Extraction confidence (0.0–1.0).
    pub confidence: f64,
}

impl ExtractedIntent {
    /// Validate the extracted intent.
    pub fn validate(&self) -> Result<(), BundleGeneratorError> {
        if self.agent_count == 0 {
            return Err(BundleGeneratorError::Extraction {
                message: "Must have at least one agent".into(),
                ambiguous_terms: vec![],
                confidence: Some(self.confidence),
            });
        }
        if self.agent_count > 10 {
            return Err(BundleGeneratorError::Extraction {
                message: "Maximum 10 agents per bundle".into(),
                ambiguous_terms: vec![],
                confidence: Some(self.confidence),
            });
        }
        if self.agent_requirements.is_empty() {
            return Err(BundleGeneratorError::Extraction {
                message: "Must have at least one agent requirement".into(),
                ambiguous_terms: vec![],
                confidence: Some(self.confidence),
            });
        }
        Ok(())
    }
}

/// A generated agent with content and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedAgent {
    /// Unique identifier.
    pub id: String,
    /// Agent name.
    pub name: String,
    /// Agent type.
    #[serde(rename = "type")]
    pub agent_type: AgentType,
    /// Role description.
    pub role: String,
    /// Short description.
    pub description: String,
    /// Markdown content (agent definition).
    pub content: String,
    /// LLM model to use ("inherit" = use parent).
    #[serde(default = "default_model")]
    pub model: String,
    /// Capabilities list.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Dependencies on other agents.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Test file contents.
    #[serde(default)]
    pub tests: Vec<String>,
    /// Additional documentation.
    #[serde(default)]
    pub documentation: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Time spent generating this agent.
    #[serde(default)]
    pub generation_time_seconds: f64,
}

fn default_model() -> String {
    "inherit".into()
}

impl GeneratedAgent {
    /// Estimated file size in KiB.
    pub fn file_size_kb(&self) -> f64 {
        self.content.len() as f64 / 1024.0
    }
}

/// A complete bundle of agents ready for packaging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBundle {
    /// Unique identifier.
    pub id: String,
    /// Bundle name (3–50 characters).
    pub name: String,
    /// Semantic version.
    #[serde(default = "default_version")]
    pub version: String,
    /// Short description.
    #[serde(default)]
    pub description: String,
    /// Agents in this bundle.
    pub agents: Vec<GeneratedAgent>,
    /// Arbitrary manifest data.
    #[serde(default)]
    pub manifest: HashMap<String, serde_json::Value>,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Bundle status.
    #[serde(default = "default_status")]
    pub status: BundleStatus,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

fn default_version() -> String {
    "1.0.0".into()
}

/// Status of a bundle in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BundleStatus {
    /// Awaiting processing.
    Pending,
    /// Currently being generated.
    Processing,
    /// Ready for packaging/distribution.
    Ready,
    /// Generation failed.
    Failed,
}

fn default_status() -> BundleStatus {
    BundleStatus::Pending
}

impl AgentBundle {
    /// Validate the bundle.
    pub fn validate(&self) -> Result<(), BundleGeneratorError> {
        if self.name.is_empty() {
            return Err(BundleGeneratorError::Validation {
                message: "Bundle must have a name".into(),
                validation_type: "bundle_name".into(),
                failures: vec![],
            });
        }
        if self.name.len() < 3 || self.name.len() > 50 {
            return Err(BundleGeneratorError::Validation {
                message: "Bundle name must be 3-50 characters".into(),
                validation_type: "bundle_name".into(),
                failures: vec![self.name.clone()],
            });
        }
        if self.agents.is_empty() {
            return Err(BundleGeneratorError::Validation {
                message: "Bundle must contain at least one agent".into(),
                validation_type: "agent_count".into(),
                failures: vec![],
            });
        }
        Ok(())
    }

    /// Number of agents in the bundle.
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Total estimated size in KiB.
    pub fn total_size_kb(&self) -> f64 {
        self.agents.iter().map(|a| a.file_size_kb()).sum()
    }
}

/// Package format for distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageFormat {
    /// Gzipped tar archive.
    #[serde(rename = "tar.gz")]
    TarGz,
    /// Zip archive.
    Zip,
    /// Plain directory.
    Directory,
    /// UVX package.
    Uvx,
}

/// A packaged bundle ready for distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagedBundle {
    /// The underlying bundle.
    pub bundle: AgentBundle,
    /// Path to the package on disk.
    pub package_path: PathBuf,
    /// Package format.
    pub format: PackageFormat,
    /// Checksum of the package file.
    #[serde(default)]
    pub checksum: String,
    /// Package size in bytes.
    #[serde(default)]
    pub size_bytes: u64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Distribution platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DistributionPlatform {
    /// GitHub repository.
    Github,
    /// PyPI registry.
    Pypi,
    /// Local filesystem.
    Local,
}

/// Result of distributing a bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionResult {
    /// Whether distribution succeeded.
    pub success: bool,
    /// Target platform.
    pub platform: DistributionPlatform,
    /// URL of the published package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Repository identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Branch name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Commit SHA.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    /// Release tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_tag: Option<String>,
    /// Errors encountered.
    #[serde(default)]
    pub errors: Vec<String>,
    /// Warnings encountered.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Time spent distributing (seconds).
    #[serde(default)]
    pub distribution_time_seconds: f64,
}

impl DistributionResult {
    /// Whether the result contains errors.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Whether the result contains warnings.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

/// Result of testing an agent or bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    /// What was tested.
    pub test_type: TestType,
    /// Name of the test target.
    pub target_name: String,
    /// Whether all tests passed.
    pub passed: bool,
    /// Total test count.
    #[serde(default)]
    pub test_count: usize,
    /// Passed tests.
    #[serde(default)]
    pub passed_count: usize,
    /// Failed tests.
    #[serde(default)]
    pub failed_count: usize,
    /// Skipped tests.
    #[serde(default)]
    pub skipped_count: usize,
    /// Execution duration (seconds).
    #[serde(default)]
    pub duration_seconds: f64,
    /// Test coverage percentage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage_percent: Option<f64>,
}

/// Type of test being run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestType {
    /// Single agent test.
    Agent,
    /// Full bundle test.
    Bundle,
    /// Integration test.
    Integration,
}

impl TestResult {
    /// Test success rate (0.0–1.0).
    pub fn success_rate(&self) -> f64 {
        if self.test_count == 0 {
            return 0.0;
        }
        self.passed_count as f64 / self.test_count as f64
    }
}

/// Metrics for a bundle generation run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationMetrics {
    /// Total elapsed time (seconds).
    pub total_time_seconds: f64,
    /// Time spent parsing (seconds).
    pub parsing_time: f64,
    /// Time spent on extraction (seconds).
    pub extraction_time: f64,
    /// Time spent generating content (seconds).
    pub generation_time: f64,
    /// Time spent on validation (seconds).
    pub validation_time: f64,
    /// Time spent packaging (seconds).
    pub packaging_time: f64,
    /// Number of agents generated.
    pub agent_count: usize,
    /// Total content size (KiB).
    pub total_size_kb: f64,
    /// Peak memory usage (MiB).
    pub memory_peak_mb: f64,
}

impl GenerationMetrics {
    /// Average generation time per agent.
    pub fn average_agent_time(&self) -> f64 {
        if self.agent_count == 0 {
            return 0.0;
        }
        self.generation_time / self.agent_count as f64
    }
}

// ---------------------------------------------------------------------------
// Pipeline traits
// ---------------------------------------------------------------------------

/// Parses natural language prompts into structured representations.
pub trait PromptParser: Send + Sync {
    /// Parse a raw prompt string.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Parsing`] on invalid input.
    fn parse(&self, prompt: &str) -> Result<ParsedPrompt, BundleGeneratorError>;
}

/// Extracts structured intent from a parsed prompt.
pub trait IntentExtractor: Send + Sync {
    /// Extract intent from a parsed prompt.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Extraction`] on ambiguous input.
    fn extract(&self, parsed: &ParsedPrompt) -> Result<ExtractedIntent, BundleGeneratorError>;
}

/// Generates agent content from requirements.
pub trait AgentGenerator: Send + Sync {
    /// Generate an agent from a requirement specification.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Generation`] if content creation fails.
    fn generate(
        &self,
        requirement: &AgentRequirement,
        context: &ExtractedIntent,
    ) -> Result<GeneratedAgent, BundleGeneratorError>;
}

/// Assembles generated agents into a bundle.
pub trait BundleBuilder: Send + Sync {
    /// Build a bundle from generated agents.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Validation`] if the bundle is invalid.
    fn build(
        &self,
        name: &str,
        agents: Vec<GeneratedAgent>,
        intent: &ExtractedIntent,
    ) -> Result<AgentBundle, BundleGeneratorError>;
}

// ---------------------------------------------------------------------------
// FilesystemPackager
// ---------------------------------------------------------------------------

/// Unsafe system directories that must not be used as output targets.
const UNSAFE_PATHS: &[&str] = &[
    "/", "/etc", "/usr", "/bin", "/sbin", "/sys", "/proc", "/dev",
];

/// Creates complete filesystem packages for agent bundles.
///
/// Orchestrates writing agents, documentation, configuration, and scripts
/// to a target directory.
pub struct FilesystemPackager {
    output_dir: PathBuf,
}

impl FilesystemPackager {
    /// Create a new packager targeting `output_dir`.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Packaging`] if the path points to a
    /// system directory.
    pub fn new(output_dir: impl Into<PathBuf>) -> Result<Self, BundleGeneratorError> {
        let output_dir = output_dir.into();
        validate_output_dir(&output_dir)?;
        Ok(Self { output_dir })
    }

    /// Create a complete filesystem package for a bundle.
    ///
    /// Creates:
    /// - `agents/` — agent markdown files
    /// - `tests/` — test files
    /// - `docs/` — documentation
    /// - `config/` — configuration
    /// - `manifest.json` — bundle metadata
    /// - `README.md`
    ///
    /// Returns the path to the created package directory.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Packaging`] on I/O failures.
    pub fn create_package(
        &self,
        bundle: &AgentBundle,
        _options: Option<&HashMap<String, serde_json::Value>>,
    ) -> Result<PathBuf, BundleGeneratorError> {
        let package_name = format!("{}-{}", bundle.name, bundle.version);
        let package_path = self.output_dir.join(&package_name);

        // Create directory structure.
        for subdir in &["agents", "tests", "docs", "config"] {
            std::fs::create_dir_all(package_path.join(subdir))?;
        }

        // Write agent files.
        for agent in &bundle.agents {
            let agent_file = package_path
                .join("agents")
                .join(format!("{}.md", agent.name));
            std::fs::write(&agent_file, &agent.content)?;
        }

        // Write manifest.
        let manifest_path = package_path.join("manifest.json");
        let manifest_json = serde_json::to_string_pretty(bundle)?;
        std::fs::write(&manifest_path, manifest_json)?;

        // Write README.
        let readme = format!(
            "# {}\n\n{}\n\n## Agents\n\n{}\n",
            bundle.name,
            bundle.description,
            bundle
                .agents
                .iter()
                .map(|a| format!("- **{}**: {}", a.name, a.description))
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::fs::write(package_path.join("README.md"), readme)?;

        Ok(package_path)
    }
}

/// Validate that `output_dir` is not a system directory.
fn validate_output_dir(path: &Path) -> Result<(), BundleGeneratorError> {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let resolved_str = resolved.to_string_lossy();

    for &unsafe_path in UNSAFE_PATHS {
        if resolved_str == unsafe_path {
            return Err(BundleGeneratorError::Packaging {
                message: format!(
                    "Cannot write to system directory: {resolved_str}. \
                     Choose a user directory for output."
                ),
                format: None,
                path: Some(resolved_str.into_owned()),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GitHubDistributor — distributes bundles via `gh` CLI
// ---------------------------------------------------------------------------

/// Truncate a string to at most `max_bytes` bytes without splitting a
/// multi-byte UTF-8 character.
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Distributes agent bundles to GitHub repositories using the `gh` CLI.
pub struct GitHubDistributor {
    /// GitHub personal access token (passed to `gh` via env).
    token: String,
}

impl GitHubDistributor {
    /// Create a new distributor with a GitHub token.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    /// Create a GitHub repository.
    ///
    /// When `public` is `true` the repo is created with `--public`,
    /// otherwise `--private`.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Distribution`] on failure.
    pub fn create_repository(
        &self,
        repo_name: &str,
        description: &str,
        public: bool,
    ) -> Result<String, BundleGeneratorError> {
        let visibility = if public { "--public" } else { "--private" };
        let desc_truncated = truncate_to_char_boundary(description, 100);

        let output = std::process::Command::new("gh")
            .args(["repo", "create", repo_name, visibility])
            .arg("--description")
            .arg(desc_truncated)
            .env("GH_TOKEN", &self.token)
            .output()
            .map_err(|e| BundleGeneratorError::Distribution {
                message: format!("failed to run gh: {e}"),
                platform: Some("github".into()),
                http_status: None,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BundleGeneratorError::Distribution {
                message: format!("gh repo create failed: {stderr}"),
                platform: Some("github".into()),
                http_status: None,
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Push a bundle file to a GitHub repository using the Contents API.
    ///
    /// Writes the JSON body to a temp file and uses `gh api --input` to
    /// avoid CLI argument length limits. Fetches the existing file SHA
    /// first so updates are idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Distribution`] on failure.
    pub fn push_bundle(
        &self,
        repo: &str,
        path: &str,
        content: &[u8],
        message: &str,
    ) -> Result<(), BundleGeneratorError> {
        use base64::{Engine, engine::general_purpose::STANDARD};

        // GET existing file SHA for idempotent update (contents API)
        let existing_sha = self.get_file_sha(repo, path);

        let encoded = STANDARD.encode(content);
        let mut body = serde_json::json!({
            "message": message,
            "content": encoded,
        });
        if let Some(sha) = existing_sha {
            body["sha"] = serde_json::Value::String(sha);
        }

        // Write JSON body to a temp file to avoid E2BIG on large bundles
        let tmp =
            tempfile::NamedTempFile::new().map_err(|e| BundleGeneratorError::Distribution {
                message: format!("failed to create temp file: {e}"),
                platform: Some("github".into()),
                http_status: None,
            })?;
        std::fs::write(tmp.path(), serde_json::to_vec(&body).unwrap_or_default()).map_err(|e| {
            BundleGeneratorError::Distribution {
                message: format!("failed to write temp file: {e}"),
                platform: Some("github".into()),
                http_status: None,
            }
        })?;

        let api_path = format!("repos/{repo}/contents/{path}");
        let output = std::process::Command::new("gh")
            .args(["api", "-X", "PUT", &api_path, "--input"])
            .arg(tmp.path())
            .env("GH_TOKEN", &self.token)
            .output()
            .map_err(|e| BundleGeneratorError::Distribution {
                message: format!("failed to run gh api: {e}"),
                platform: Some("github".into()),
                http_status: None,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BundleGeneratorError::Distribution {
                message: format!("gh api PUT failed: {stderr}"),
                platform: Some("github".into()),
                http_status: None,
            });
        }

        Ok(())
    }

    /// Fetch the SHA of an existing file, or `None` if it does not exist.
    fn get_file_sha(&self, repo: &str, path: &str) -> Option<String> {
        let api_path = format!("repos/{repo}/contents/{path}");
        let output = std::process::Command::new("gh")
            .args(["api", &api_path, "--jq", ".sha"])
            .env("GH_TOKEN", &self.token)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if sha.is_empty() { None } else { Some(sha) }
    }

    /// Distribute a packaged bundle to GitHub.
    ///
    /// Creates the repository (if needed), pushes the bundle content, and
    /// returns a [`DistributionResult`].
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Distribution`] on failure.
    pub fn distribute(
        &self,
        bundle: &PackagedBundle,
        repo_name: &str,
    ) -> Result<DistributionResult, BundleGeneratorError> {
        self.distribute_with_options(bundle, repo_name, true)
    }

    /// Distribute with explicit visibility control.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Distribution`] on failure.
    pub fn distribute_with_options(
        &self,
        bundle: &PackagedBundle,
        repo_name: &str,
        public: bool,
    ) -> Result<DistributionResult, BundleGeneratorError> {
        let desc = truncate_to_char_boundary(&bundle.bundle.description, 100);
        let _repo_url = self.create_repository(repo_name, desc, public)?;

        let bundle_bytes = std::fs::read(&bundle.package_path).map_err(|e| {
            BundleGeneratorError::Distribution {
                message: format!("failed to read bundle: {e}"),
                platform: Some("github".into()),
                http_status: None,
            }
        })?;

        let file_name = bundle
            .package_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("bundle.tar.gz");

        self.push_bundle(
            repo_name,
            file_name,
            &bundle_bytes,
            &format!("Upload bundle {}", bundle.bundle.name),
        )?;

        Ok(DistributionResult {
            success: true,
            platform: DistributionPlatform::Github,
            url: Some(format!("https://github.com/{repo_name}")),
            repository: Some(repo_name.to_string()),
            branch: Some("main".into()),
            commit_sha: None,
            release_tag: None,
            errors: vec![],
            warnings: vec![],
            distribution_time_seconds: 0.0,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_prompt_validation_empty() {
        let p = ParsedPrompt {
            raw_prompt: "   ".into(),
            tokens: vec![],
            sentences: vec![],
            key_phrases: vec![],
            entities: HashMap::new(),
            confidence: 0.5,
            metadata: HashMap::new(),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn parsed_prompt_validation_bad_confidence() {
        let p = ParsedPrompt {
            raw_prompt: "create an agent".into(),
            tokens: vec!["create".into()],
            sentences: vec!["create an agent".into()],
            key_phrases: vec![],
            entities: HashMap::new(),
            confidence: 1.5,
            metadata: HashMap::new(),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn parsed_prompt_validation_ok() {
        let p = ParsedPrompt {
            raw_prompt: "create an agent".into(),
            tokens: vec!["create".into(), "an".into(), "agent".into()],
            sentences: vec!["create an agent".into()],
            key_phrases: vec!["agent".into()],
            entities: HashMap::new(),
            confidence: 0.9,
            metadata: HashMap::new(),
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn agent_requirement_validation() {
        let req = AgentRequirement {
            name: "my-agent".into(),
            role: "tester".into(),
            purpose: "testing".into(),
            capabilities: vec!["test".into()],
            constraints: vec![],
            suggested_type: AgentType::Specialized,
            dependencies: vec![],
            priority: 0,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn agent_requirement_empty_capabilities() {
        let req = AgentRequirement {
            name: "my-agent".into(),
            role: "tester".into(),
            purpose: "testing".into(),
            capabilities: vec![],
            constraints: vec![],
            suggested_type: AgentType::Specialized,
            dependencies: vec![],
            priority: 0,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn extracted_intent_zero_agents() {
        let intent = ExtractedIntent {
            action: BundleAction::Create,
            domain: "security".into(),
            agent_count: 0,
            agent_requirements: vec![],
            complexity: Complexity::Simple,
            constraints: vec![],
            dependencies: vec![],
            confidence: 0.8,
        };
        assert!(intent.validate().is_err());
    }

    #[test]
    fn extracted_intent_too_many_agents() {
        let intent = ExtractedIntent {
            action: BundleAction::Create,
            domain: "security".into(),
            agent_count: 11,
            agent_requirements: vec![AgentRequirement {
                name: "a".into(),
                role: "r".into(),
                purpose: "p".into(),
                capabilities: vec!["c".into()],
                constraints: vec![],
                suggested_type: AgentType::Core,
                dependencies: vec![],
                priority: 0,
            }],
            complexity: Complexity::Advanced,
            constraints: vec![],
            dependencies: vec![],
            confidence: 0.8,
        };
        assert!(intent.validate().is_err());
    }

    #[test]
    fn bundle_validation_empty_name() {
        let bundle = AgentBundle {
            id: "test-id".into(),
            name: String::new(),
            version: "1.0.0".into(),
            description: String::new(),
            agents: vec![],
            manifest: HashMap::new(),
            metadata: HashMap::new(),
            status: BundleStatus::Pending,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        assert!(bundle.validate().is_err());
    }

    #[test]
    fn bundle_validation_no_agents() {
        let bundle = AgentBundle {
            id: "test-id".into(),
            name: "my-bundle".into(),
            version: "1.0.0".into(),
            description: String::new(),
            agents: vec![],
            manifest: HashMap::new(),
            metadata: HashMap::new(),
            status: BundleStatus::Pending,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        assert!(bundle.validate().is_err());
    }

    #[test]
    fn generation_metrics_average() {
        let m = GenerationMetrics {
            generation_time: 10.0,
            agent_count: 5,
            ..Default::default()
        };
        assert!((m.average_agent_time() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn generation_metrics_zero_agents() {
        let m = GenerationMetrics::default();
        assert!((m.average_agent_time()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_result_success_rate() {
        let r = TestResult {
            test_type: TestType::Bundle,
            target_name: "my-bundle".into(),
            passed: true,
            test_count: 10,
            passed_count: 8,
            failed_count: 2,
            skipped_count: 0,
            duration_seconds: 1.0,
            coverage_percent: None,
        };
        assert!((r.success_rate() - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn distribution_result_flags() {
        let r = DistributionResult {
            success: true,
            platform: DistributionPlatform::Github,
            url: Some("https://github.com/test/repo".into()),
            repository: Some("test/repo".into()),
            branch: None,
            commit_sha: None,
            release_tag: None,
            errors: vec![],
            warnings: vec!["check license".into()],
            distribution_time_seconds: 0.0,
        };
        assert!(!r.has_errors());
        assert!(r.has_warnings());
    }

    #[test]
    fn validate_output_dir_rejects_root() {
        let result = validate_output_dir(Path::new("/"));
        assert!(result.is_err());
    }

    #[test]
    fn generated_agent_file_size() {
        let agent = GeneratedAgent {
            id: "id".into(),
            name: "test".into(),
            agent_type: AgentType::Specialized,
            role: "tester".into(),
            description: "test agent".into(),
            content: "x".repeat(1024),
            model: "inherit".into(),
            capabilities: vec![],
            dependencies: vec![],
            tests: vec![],
            documentation: String::new(),
            created_at: Utc::now(),
            generation_time_seconds: 0.0,
        };
        assert!((agent.file_size_kb() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn error_recovery_suggestions() {
        let err = BundleGeneratorError::Parsing {
            message: "bad".into(),
            prompt_fragment: None,
            position: None,
        };
        assert!(!err.recovery_suggestion().is_empty());
    }

    #[test]
    fn bundle_serde_roundtrip() {
        let bundle = AgentBundle {
            id: "test-id".into(),
            name: "test-bundle".into(),
            version: "1.0.0".into(),
            description: "A test bundle".into(),
            agents: vec![GeneratedAgent {
                id: "agent-1".into(),
                name: "test-agent".into(),
                agent_type: AgentType::Core,
                role: "tester".into(),
                description: "tests things".into(),
                content: "# Test Agent\n\nContent here.".repeat(10),
                model: "inherit".into(),
                capabilities: vec!["testing".into()],
                dependencies: vec![],
                tests: vec![],
                documentation: String::new(),
                created_at: Utc::now(),
                generation_time_seconds: 1.5,
            }],
            manifest: HashMap::new(),
            metadata: HashMap::new(),
            status: BundleStatus::Ready,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&bundle).unwrap();
        let deserialized: AgentBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, bundle.name);
        assert_eq!(deserialized.agents.len(), 1);
    }

    // -----------------------------------------------------------------------
    // GitHubDistributor unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn github_distributor_new_stores_token() {
        let d = GitHubDistributor::new("ghp_test123");
        assert_eq!(d.token, "ghp_test123");
    }

    #[test]
    fn distribute_fails_without_gh() {
        let d = GitHubDistributor::new("fake-token");
        let bundle = PackagedBundle {
            bundle: AgentBundle {
                id: "test-id".into(),
                name: "test-bundle".into(),
                version: "1.0.0".into(),
                description: "a test bundle".into(),
                agents: vec![],
                manifest: HashMap::new(),
                metadata: HashMap::new(),
                status: BundleStatus::Pending,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            package_path: PathBuf::from("/nonexistent/bundle.tar.gz"),
            format: PackageFormat::TarGz,
            size_bytes: 0,
            checksum: String::new(),
            created_at: Utc::now(),
        };
        let result = d.distribute(&bundle, "test/repo");
        assert!(result.is_err());
    }

    #[test]
    fn truncate_char_boundary_ascii() {
        assert_eq!(truncate_to_char_boundary("hello world", 5), "hello");
    }

    #[test]
    fn truncate_char_boundary_multibyte() {
        // 'café' — 'é' is 2 bytes. Byte 4 would split 'é'.
        let s = "café";
        let t = truncate_to_char_boundary(s, 4);
        assert!(t.len() <= 4);
        assert!(t.is_char_boundary(t.len()));
    }

    #[test]
    fn truncate_char_boundary_emoji() {
        // '🦀' = 4 bytes
        let t = truncate_to_char_boundary("🦀rust", 2);
        assert!(t.is_empty() || t.len() <= 2);
    }

    #[test]
    fn truncate_char_boundary_beyond_len() {
        assert_eq!(truncate_to_char_boundary("hi", 100), "hi");
    }

    #[test]
    fn truncate_char_boundary_empty() {
        assert!(truncate_to_char_boundary("", 10).is_empty());
    }

    #[test]
    fn truncate_char_boundary_zero() {
        assert!(truncate_to_char_boundary("hello", 0).is_empty());
    }

    #[test]
    fn push_bundle_json_structure() {
        use base64::{Engine, engine::general_purpose::STANDARD};

        let content = b"test bundle content";
        let encoded = STANDARD.encode(content);
        let mut body = serde_json::json!({
            "message": "Upload bundle",
            "content": encoded,
        });
        // Simulate idempotent update
        body["sha"] = serde_json::Value::String("abc123".into());

        let json_str = serde_json::to_string(&body).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["message"], "Upload bundle");
        assert_eq!(parsed["sha"], "abc123");
        let decoded = STANDARD
            .decode(parsed["content"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, content);
    }

    #[test]
    fn base64_crate_roundtrip() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let data = b"Hello GitHub distributor!";
        let encoded = STANDARD.encode(data);
        let decoded = STANDARD.decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }
}
