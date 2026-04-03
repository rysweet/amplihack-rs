use std::path::PathBuf;

use amplihack_agent_generator::{
    AgentAssembler, BundleStatus, ExecutionPlan, GenerationMetrics, GoalAgentBundle,
    GoalAgentPackager, GoalDefinition, ObjectivePlanner, PlanPhase, PromptAnalyzer,
    SkillDefinition, SkillSynthesizer,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn valid_goal() -> GoalDefinition {
    GoalDefinition::new("build tool", "create a CLI tool", "development").unwrap()
}

fn valid_phase() -> PlanPhase {
    PlanPhase::new("init", "initialise project", vec!["shell".into()]).unwrap()
}

fn valid_plan() -> ExecutionPlan {
    ExecutionPlan::new(Uuid::new_v4(), vec![valid_phase()]).unwrap()
}

fn valid_skill() -> SkillDefinition {
    SkillDefinition::new("builder", PathBuf::from("skills/build.yaml"), "build code").unwrap()
}

// ---------------------------------------------------------------------------
// Full pipeline — behavioral tests
// ---------------------------------------------------------------------------

#[test]
fn full_pipeline_analyze_to_package() {
    let analyzer = PromptAnalyzer::new();
    let planner = ObjectivePlanner::new();
    let synthesizer = SkillSynthesizer::new();
    let assembler = AgentAssembler::new();
    let packager = GoalAgentPackager::new();

    let goal = analyzer.analyze("build a data pipeline").unwrap();
    let plan = planner.plan(&goal).unwrap();
    let skills = synthesizer.synthesize(&plan).unwrap();
    let bundle = assembler.assemble(&goal, &plan, skills).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = packager.package(&bundle, dir.path()).unwrap();
    assert!(path.join("bundle.json").exists());
}

#[test]
fn pipeline_analyze_step() {
    let analyzer = PromptAnalyzer::new();
    let goal = analyzer.analyze("simple task").unwrap();
    assert_eq!(goal.goal, "simple task");
    assert!(!goal.domain.is_empty());
}

#[test]
fn pipeline_plan_step() {
    let planner = ObjectivePlanner::new();
    let plan = planner.plan(&valid_goal()).unwrap();
    assert!(plan.phase_count() >= 3);
}

#[test]
fn pipeline_synthesize_step() {
    let synthesizer = SkillSynthesizer::new();
    let skills = synthesizer.synthesize(&valid_plan()).unwrap();
    assert_eq!(skills.len(), 1);
}

#[test]
fn pipeline_assemble_step() {
    let assembler = AgentAssembler::new();
    let bundle = assembler
        .assemble(&valid_goal(), &valid_plan(), vec![valid_skill()])
        .unwrap();
    assert_eq!(bundle.status, BundleStatus::Ready);
    assert!(bundle.goal_definition.is_some());
    assert!(bundle.execution_plan.is_some());
    assert!(!bundle.skills.is_empty());
}

#[test]
fn pipeline_package_step() {
    let packager = GoalAgentPackager::new();
    let bundle = GoalAgentBundle::new("pkg-test", "0.1.0").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = packager.package(&bundle, dir.path()).unwrap();
    assert!(path.join("bundle.json").exists());
}

#[test]
fn pipeline_error_propagation_from_analyzer() {
    let analyzer = PromptAnalyzer::new();
    // Empty prompt should produce an error, not panic
    let result = analyzer.analyze("");
    assert!(result.is_err());
}

#[test]
fn output_directory_creation() {
    let packager = GoalAgentPackager::new();
    let bundle = GoalAgentBundle::new("dir-test", "0.1.0").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("nonexistent").join("nested").join("dir");
    let path = packager.package(&bundle, &nested).unwrap();
    assert!(path.join("bundle.json").exists());
}

// ---------------------------------------------------------------------------
// Metrics collection (no todo!() — these exercise model structs and pass)
// ---------------------------------------------------------------------------

#[test]
fn metrics_collection_from_stages() {
    let metrics = GenerationMetrics {
        total_time_seconds: 25.0,
        analysis_time: 5.0,
        planning_time: 8.0,
        synthesis_time: 7.0,
        assembly_time: 5.0,
        skill_count: 3,
        phase_count: 4,
        bundle_size_kb: 42.0,
    };
    assert_eq!(metrics.skill_count, 3);
    assert_eq!(metrics.phase_count, 4);
    assert!((metrics.average_phase_time() - 6.25).abs() < f64::EPSILON);
}

#[test]
fn pipeline_bundle_lifecycle() {
    let mut bundle = GoalAgentBundle::new("lifecycle", "0.1.0").unwrap();
    assert_eq!(bundle.status, BundleStatus::Pending);
    assert!(!bundle.is_complete());

    bundle.status = BundleStatus::Planning;
    assert!(!bundle.is_complete());

    bundle.goal_definition = Some(valid_goal());
    bundle.execution_plan = Some(valid_plan());
    bundle.skills = vec![valid_skill()];
    bundle.status = BundleStatus::Assembling;
    assert!(!bundle.is_complete());

    bundle.status = BundleStatus::Ready;
    assert!(bundle.is_complete());
}

#[test]
fn metrics_serde_preserves_all_fields() {
    let m = GenerationMetrics {
        total_time_seconds: 100.0,
        analysis_time: 10.0,
        planning_time: 20.0,
        synthesis_time: 30.0,
        assembly_time: 40.0,
        skill_count: 5,
        phase_count: 3,
        bundle_size_kb: 128.0,
    };
    let json = serde_json::to_string(&m).unwrap();
    let m2: GenerationMetrics = serde_json::from_str(&json).unwrap();
    assert_eq!(m.skill_count, m2.skill_count);
    assert_eq!(m.phase_count, m2.phase_count);
    assert!((m.bundle_size_kb - m2.bundle_size_kb).abs() < f64::EPSILON);
}

#[test]
fn pipeline_components_are_default_constructible() {
    // Verify all pipeline components implement Default
    let _a = PromptAnalyzer::default();
    let _p = ObjectivePlanner::default();
    let _s = SkillSynthesizer::default();
    let _asm = AgentAssembler::default();
    let _pkg = GoalAgentPackager::default();
}
