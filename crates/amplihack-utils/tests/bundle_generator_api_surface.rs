//! TDD (Step 7) — public API surface + behavior-preservation safety net for
//! the `bundle_generator` decomposition refactor (issue #846).
//!
//! Unlike `bundle_generator_module_structure.rs` (red-now / green-after),
//! this suite is a **characterization / regression net**: it passes against
//! the current monolith and MUST keep passing after the file is split into
//! the `bundle_generator/` directory module. It fails only if the refactor
//! changes the observable public API or behavior.
//!
//! It exercises every one of the 23 public items exclusively through their
//! `amplihack_utils::bundle_generator::*` paths, guaranteeing the re-export
//! surface stays byte-compatible.

use std::collections::HashMap;

use chrono::Utc;

use amplihack_utils::bundle_generator::{
    AgentBundle, AgentGenerator, AgentRequirement, AgentType, BundleAction, BundleBuilder,
    BundleGeneratorError, BundleStatus, Complexity, DistributionPlatform, DistributionResult,
    ExtractedIntent, FilesystemPackager, GeneratedAgent, GenerationMetrics, GitHubDistributor,
    IntentExtractor, PackageFormat, PackagedBundle, ParsedPrompt, PromptParser, TestResult,
    TestType,
};

// ---------------------------------------------------------------------------
// Public-path fixtures (all struct fields are part of the public API)
// ---------------------------------------------------------------------------

fn valid_agent() -> GeneratedAgent {
    GeneratedAgent {
        id: "agent-1".into(),
        name: "test-agent".into(),
        agent_type: AgentType::Core,
        role: "tester".into(),
        description: "tests things".into(),
        content: "x".repeat(2048),
        model: "inherit".into(),
        capabilities: vec!["testing".into()],
        dependencies: vec![],
        tests: vec![],
        documentation: String::new(),
        created_at: Utc::now(),
        generation_time_seconds: 1.5,
    }
}

fn valid_bundle() -> AgentBundle {
    AgentBundle {
        id: "test-id".into(),
        name: "test-bundle".into(),
        version: "1.0.0".into(),
        description: "A test bundle".into(),
        agents: vec![valid_agent()],
        manifest: HashMap::new(),
        metadata: HashMap::new(),
        status: BundleStatus::Ready,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// Trait paths must remain reachable (compile-time contract)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn _pipeline_traits_are_public() {
    fn assert_parser<T: PromptParser>() {}
    fn assert_extractor<T: IntentExtractor>() {}
    fn assert_generator<T: AgentGenerator>() {}
    fn assert_builder<T: BundleBuilder>() {}
    // Referencing the bounds is enough to lock the public paths.
    let _ = assert_parser::<DummyStage>;
    let _ = assert_extractor::<DummyStage>;
    let _ = assert_generator::<DummyStage>;
    let _ = assert_builder::<DummyStage>;
}

struct DummyStage;
impl PromptParser for DummyStage {
    fn parse(&self, _prompt: &str) -> Result<ParsedPrompt, BundleGeneratorError> {
        unimplemented!()
    }
}
impl IntentExtractor for DummyStage {
    fn extract(&self, _parsed: &ParsedPrompt) -> Result<ExtractedIntent, BundleGeneratorError> {
        unimplemented!()
    }
}
impl AgentGenerator for DummyStage {
    fn generate(
        &self,
        _requirement: &AgentRequirement,
        _context: &ExtractedIntent,
    ) -> Result<GeneratedAgent, BundleGeneratorError> {
        unimplemented!()
    }
}
impl BundleBuilder for DummyStage {
    fn build(
        &self,
        _name: &str,
        _agents: Vec<GeneratedAgent>,
        _intent: &ExtractedIntent,
    ) -> Result<AgentBundle, BundleGeneratorError> {
        unimplemented!()
    }
}

// ---------------------------------------------------------------------------
// Validation behavior (public methods)
// ---------------------------------------------------------------------------

#[test]
fn parsed_prompt_validation_via_public_api() {
    let bad = ParsedPrompt {
        raw_prompt: "   ".into(),
        tokens: vec![],
        sentences: vec![],
        key_phrases: vec![],
        entities: HashMap::new(),
        confidence: 0.5,
        metadata: HashMap::new(),
    };
    assert!(bad.validate().is_err());

    let good = ParsedPrompt {
        raw_prompt: "create an agent".into(),
        tokens: vec!["create".into(), "an".into(), "agent".into()],
        sentences: vec!["create an agent".into()],
        key_phrases: vec!["agent".into()],
        entities: HashMap::new(),
        confidence: 0.9,
        metadata: HashMap::new(),
    };
    assert!(good.validate().is_ok());
}

#[test]
fn agent_requirement_requires_capabilities() {
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
fn extracted_intent_agent_count_bounds() {
    let zero = ExtractedIntent {
        action: BundleAction::Create,
        domain: "security".into(),
        agent_count: 0,
        agent_requirements: vec![],
        complexity: Complexity::Simple,
        constraints: vec![],
        dependencies: vec![],
        confidence: 0.8,
    };
    assert!(zero.validate().is_err());
}

#[test]
fn bundle_validation_via_public_api() {
    let mut bundle = valid_bundle();
    assert!(bundle.validate().is_ok());
    bundle.name = String::new();
    assert!(bundle.validate().is_err());
}

// ---------------------------------------------------------------------------
// Derived accessors / math (public methods)
// ---------------------------------------------------------------------------

#[test]
fn bundle_accessors() {
    let bundle = valid_bundle();
    assert_eq!(bundle.agent_count(), 1);
    assert!(bundle.total_size_kb() > 0.0);
}

#[test]
fn generated_agent_file_size_kb() {
    let mut agent = valid_agent();
    agent.content = "x".repeat(1024);
    assert!((agent.file_size_kb() - 1.0).abs() < f64::EPSILON);
}

#[test]
fn generation_metrics_average() {
    let m = GenerationMetrics {
        generation_time: 10.0,
        agent_count: 5,
        ..Default::default()
    };
    assert!((m.average_agent_time() - 2.0).abs() < f64::EPSILON);
    assert!((GenerationMetrics::default().average_agent_time()).abs() < f64::EPSILON);
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
fn error_recovery_suggestion_non_empty() {
    let err = BundleGeneratorError::Parsing {
        message: "bad".into(),
        prompt_fragment: None,
        position: None,
    };
    assert!(!err.recovery_suggestion().is_empty());
}

// ---------------------------------------------------------------------------
// serde round-trip (public serialization contract)
// ---------------------------------------------------------------------------

#[test]
fn bundle_serde_roundtrip() {
    let bundle = valid_bundle();
    let json = serde_json::to_string(&bundle).unwrap();
    let restored: AgentBundle = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.name, bundle.name);
    assert_eq!(restored.agents.len(), 1);
}

// ---------------------------------------------------------------------------
// FilesystemPackager — path-safety guard + packaging (public behavior)
// ---------------------------------------------------------------------------

#[test]
fn packager_rejects_system_directory() {
    let result = FilesystemPackager::new("/");
    assert!(
        result.is_err(),
        "FilesystemPackager must reject system directories"
    );
}

#[test]
fn packager_creates_package_in_user_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let packager = FilesystemPackager::new(tmp.path()).expect("user dir must be accepted");
    let bundle = valid_bundle();
    let package_path = packager.create_package(&bundle, None).unwrap();
    assert!(package_path.join("manifest.json").is_file());
    assert!(package_path.join("README.md").is_file());
    assert!(package_path.join("agents").is_dir());
}

// ---------------------------------------------------------------------------
// GitHubDistributor — construction + failure path (no network / no gh)
// ---------------------------------------------------------------------------

#[test]
fn distributor_constructs_via_public_api() {
    // Token is private: we can only observe construction succeeds. Privacy
    // itself is enforced in bundle_generator_module_structure.rs.
    let _d = GitHubDistributor::new("ghp_public_api_test");
}

#[test]
fn distribute_fails_when_bundle_missing() {
    let d = GitHubDistributor::new("fake-token");
    let bundle = PackagedBundle {
        bundle: valid_bundle(),
        package_path: std::path::PathBuf::from("/nonexistent/bundle.tar.gz"),
        format: PackageFormat::TarGz,
        size_bytes: 0,
        checksum: String::new(),
        created_at: Utc::now(),
    };
    let result = d.distribute(&bundle, "test/repo");
    assert!(
        result.is_err(),
        "distribute must fail for a missing/unreadable package"
    );
}

#[test]
fn distribution_error_never_contains_token() {
    // Security: the auth token must not surface in any error message.
    let secret = "ghp_super_secret_value_1234567890";
    let d = GitHubDistributor::new(secret);
    let bundle = PackagedBundle {
        bundle: valid_bundle(),
        package_path: std::path::PathBuf::from("/nonexistent/bundle.tar.gz"),
        format: PackageFormat::TarGz,
        size_bytes: 0,
        checksum: String::new(),
        created_at: Utc::now(),
    };
    if let Err(e) = d.distribute(&bundle, "test/repo") {
        assert!(
            !format!("{e}").contains(secret),
            "error message must never leak the GitHub token"
        );
    }
}
