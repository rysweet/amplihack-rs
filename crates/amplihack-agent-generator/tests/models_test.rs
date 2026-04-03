use std::path::PathBuf;

use amplihack_agent_generator::{
    BundleStatus, Complexity, ExecutionPlan, GenerationMetrics, GoalAgentBundle, GoalDefinition,
    PlanPhase, SDKToolConfig, SkillDefinition, SubAgentConfig,
};
use uuid::Uuid;

fn valid_goal() -> GoalDefinition {
    GoalDefinition::new("analyse logs", "find anomalies", "data-processing").unwrap()
}
fn valid_phase() -> PlanPhase {
    PlanPhase::new("setup", "set up environment", vec!["shell".into()]).unwrap()
}
fn valid_skill() -> SkillDefinition {
    SkillDefinition::new("log-parser", PathBuf::from("skills/log.yaml"), "parse logs").unwrap()
}
fn valid_bundle() -> GoalAgentBundle {
    GoalAgentBundle::new("test-bundle", "0.1.0").unwrap()
}

// GoalDefinition validation

#[test]
fn goal_definition_valid() {
    assert!(GoalDefinition::new("prompt", "goal", "domain").is_ok());
}
#[test]
fn goal_definition_empty_prompt_rejected() {
    assert!(GoalDefinition::new("", "goal", "domain").is_err());
}
#[test]
fn goal_definition_whitespace_prompt_rejected() {
    assert!(GoalDefinition::new("   ", "goal", "domain").is_err());
}
#[test]
fn goal_definition_empty_goal_rejected() {
    assert!(GoalDefinition::new("prompt", "", "domain").is_err());
}
#[test]
fn goal_definition_empty_domain_rejected() {
    assert!(GoalDefinition::new("prompt", "goal", "").is_err());
}
#[test]
fn goal_definition_default_complexity() {
    assert_eq!(valid_goal().complexity, Complexity::Simple);
}
#[test]
fn goal_definition_serde_roundtrip() {
    let g = valid_goal();
    let g2: GoalDefinition = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
    assert_eq!(g.raw_prompt, g2.raw_prompt);
    assert_eq!(g.goal, g2.goal);
    assert_eq!(g.domain, g2.domain);
}

// PlanPhase validation

#[test]
fn plan_phase_valid() {
    assert!(PlanPhase::new("init", "initialize", vec!["cap1".into()]).is_ok());
}
#[test]
fn plan_phase_empty_name_rejected() {
    assert!(PlanPhase::new("", "desc", vec!["cap".into()]).is_err());
}
#[test]
fn plan_phase_empty_capabilities_rejected() {
    assert!(PlanPhase::new("phase", "desc", vec![]).is_err());
}
#[test]
fn plan_phase_default_parallel_safe() {
    assert!(valid_phase().parallel_safe);
}
#[test]
fn plan_phase_serde_roundtrip() {
    let p = valid_phase();
    let p2: PlanPhase = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(p.name, p2.name);
    assert_eq!(p.parallel_safe, p2.parallel_safe);
}

// ExecutionPlan validation

#[test]
fn execution_plan_valid() {
    assert!(ExecutionPlan::new(Uuid::new_v4(), vec![valid_phase()]).is_ok());
}
#[test]
fn execution_plan_empty_phases_rejected() {
    assert!(ExecutionPlan::new(Uuid::new_v4(), vec![]).is_err());
}
#[test]
fn execution_plan_too_many_phases_rejected() {
    let phases: Vec<PlanPhase> = (0..11)
        .map(|i| PlanPhase::new(format!("p{i}"), "desc", vec!["cap".into()]).unwrap())
        .collect();
    assert!(ExecutionPlan::new(Uuid::new_v4(), phases).is_err());
}
#[test]
fn execution_plan_max_phases_accepted() {
    let phases: Vec<PlanPhase> = (0..10)
        .map(|i| PlanPhase::new(format!("p{i}"), "desc", vec!["cap".into()]).unwrap())
        .collect();
    assert!(ExecutionPlan::new(Uuid::new_v4(), phases).is_ok());
}
#[test]
fn execution_plan_phase_count() {
    let plan = ExecutionPlan::new(Uuid::new_v4(), vec![valid_phase(), valid_phase(), valid_phase()]).unwrap();
    assert_eq!(plan.phase_count(), 3);
}
#[test]
fn execution_plan_serde_roundtrip() {
    let plan = ExecutionPlan::new(Uuid::new_v4(), vec![valid_phase()]).unwrap();
    let plan2: ExecutionPlan = serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
    assert_eq!(plan.goal_id, plan2.goal_id);
    assert_eq!(plan.phase_count(), plan2.phase_count());
}

// SkillDefinition validation

#[test]
fn skill_definition_valid() {
    assert!(SkillDefinition::new("skill", PathBuf::from("s.yaml"), "body").is_ok());
}
#[test]
fn skill_definition_empty_name_rejected() {
    assert!(SkillDefinition::new("", PathBuf::from("s.yaml"), "body").is_err());
}
#[test]
fn skill_definition_empty_content_rejected() {
    assert!(SkillDefinition::new("skill", PathBuf::from("s.yaml"), "").is_err());
}
#[test]
fn skill_definition_score_too_high() {
    assert!(valid_skill().with_match_score(1.1).is_err());
}
#[test]
fn skill_definition_score_too_low() {
    assert!(valid_skill().with_match_score(-0.1).is_err());
}
#[test]
fn skill_definition_valid_score_boundaries() {
    assert!(valid_skill().with_match_score(0.0).is_ok());
    assert!(valid_skill().with_match_score(1.0).is_ok());
    assert!(valid_skill().with_match_score(0.5).is_ok());
}
#[test]
fn skill_definition_serde_roundtrip() {
    let s = valid_skill().with_match_score(0.75).unwrap();
    let s2: SkillDefinition = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(s.name, s2.name);
    assert!((s.match_score - s2.match_score).abs() < f64::EPSILON);
}

// SDKToolConfig

#[test]
fn sdk_tool_config_to_dict() {
    let d = SDKToolConfig { name: "tool".into(), description: "does stuff".into(), category: "util".into() }.to_dict();
    assert_eq!(d.get("name").unwrap(), "tool");
    assert_eq!(d.get("description").unwrap(), "does stuff");
    assert_eq!(d.get("category").unwrap(), "util");
    assert_eq!(d.len(), 3);
}
#[test]
fn sdk_tool_config_serde_roundtrip() {
    let cfg = SDKToolConfig { name: "t".into(), description: "d".into(), category: "c".into() };
    let cfg2: SDKToolConfig = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(cfg.name, cfg2.name);
}

// SubAgentConfig

#[test]
fn sub_agent_config_default_filename() {
    let sa = SubAgentConfig::new("planner");
    assert_eq!(sa.filename, "planner.yaml");
    assert_eq!(sa.role, "planner");
}
#[test]
fn sub_agent_config_custom_filename() {
    let mut sa = SubAgentConfig::new("reviewer");
    sa.filename = "custom.yaml".into();
    assert_eq!(sa.filename, "custom.yaml");
}
#[test]
fn sub_agent_config_empty_config() {
    assert!(SubAgentConfig::new("worker").config.is_empty());
}

// GoalAgentBundle validation

#[test]
fn bundle_valid() {
    assert!(GoalAgentBundle::new("my-bundle", "1.0.0").is_ok());
}
#[test]
fn bundle_name_too_short() {
    assert!(GoalAgentBundle::new("ab", "1.0.0").is_err());
}
#[test]
fn bundle_name_too_long() {
    assert!(GoalAgentBundle::new(&"x".repeat(51), "1.0.0").is_err());
}
#[test]
fn bundle_name_boundary_3_chars() {
    assert!(GoalAgentBundle::new("abc", "1.0.0").is_ok());
}
#[test]
fn bundle_name_boundary_50_chars() {
    assert!(GoalAgentBundle::new(&"x".repeat(50), "1.0.0").is_ok());
}
#[test]
fn bundle_is_complete_when_ready() {
    let mut b = valid_bundle();
    b.goal_definition = Some(valid_goal());
    b.execution_plan = Some(ExecutionPlan::new(Uuid::new_v4(), vec![valid_phase()]).unwrap());
    b.skills = vec![valid_skill()];
    b.status = BundleStatus::Ready;
    assert!(b.is_complete());
}
#[test]
fn bundle_is_not_complete_without_goal() {
    let mut b = valid_bundle();
    b.execution_plan = Some(ExecutionPlan::new(Uuid::new_v4(), vec![valid_phase()]).unwrap());
    b.skills = vec![valid_skill()];
    b.status = BundleStatus::Ready;
    assert!(!b.is_complete());
}
#[test]
fn bundle_is_not_complete_without_plan() {
    let mut b = valid_bundle();
    b.goal_definition = Some(valid_goal());
    b.skills = vec![valid_skill()];
    b.status = BundleStatus::Ready;
    assert!(!b.is_complete());
}
#[test]
fn bundle_is_not_complete_without_skills() {
    let mut b = valid_bundle();
    b.goal_definition = Some(valid_goal());
    b.execution_plan = Some(ExecutionPlan::new(Uuid::new_v4(), vec![valid_phase()]).unwrap());
    b.status = BundleStatus::Ready;
    assert!(!b.is_complete());
}
#[test]
fn bundle_is_not_complete_wrong_status() {
    let mut b = valid_bundle();
    b.goal_definition = Some(valid_goal());
    b.execution_plan = Some(ExecutionPlan::new(Uuid::new_v4(), vec![valid_phase()]).unwrap());
    b.skills = vec![valid_skill()];
    b.status = BundleStatus::Pending;
    assert!(!b.is_complete());
}
#[test]
fn bundle_default_status_is_pending() {
    assert_eq!(valid_bundle().status, BundleStatus::Pending);
}

// BundleStatus

#[test]
fn bundle_status_transitions() {
    let mut s = BundleStatus::Pending;
    s = BundleStatus::Planning;  assert_eq!(s, BundleStatus::Planning);
    s = BundleStatus::Assembling; assert_eq!(s, BundleStatus::Assembling);
    s = BundleStatus::Ready;     assert_eq!(s, BundleStatus::Ready);
    s = BundleStatus::Failed;    assert_eq!(s, BundleStatus::Failed);
}
#[test]
fn bundle_status_serde_roundtrip() {
    for status in [BundleStatus::Pending, BundleStatus::Planning,
                   BundleStatus::Assembling, BundleStatus::Ready, BundleStatus::Failed] {
        let s2: BundleStatus = serde_json::from_str(&serde_json::to_string(&status).unwrap()).unwrap();
        assert_eq!(status, s2);
    }
}

// Complexity

#[test]
fn complexity_ordering() {
    assert!(Complexity::Simple < Complexity::Moderate);
    assert!(Complexity::Moderate < Complexity::Complex);
    assert!(Complexity::Simple < Complexity::Complex);
}
#[test]
fn complexity_serde_roundtrip() {
    for c in [Complexity::Simple, Complexity::Moderate, Complexity::Complex] {
        let c2: Complexity = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(c, c2);
    }
}
#[test]
fn complexity_snake_case_serialization() {
    assert_eq!(serde_json::to_string(&Complexity::Simple).unwrap(), "\"simple\"");
    assert_eq!(serde_json::to_string(&Complexity::Moderate).unwrap(), "\"moderate\"");
    assert_eq!(serde_json::to_string(&Complexity::Complex).unwrap(), "\"complex\"");
}

// GenerationMetrics

#[test]
fn generation_metrics_average_phase_time() {
    let m = GenerationMetrics {
        total_time_seconds: 30.0, analysis_time: 5.0, planning_time: 10.0,
        synthesis_time: 10.0, assembly_time: 5.0, skill_count: 4, phase_count: 3, bundle_size_kb: 12.5,
    };
    assert!((m.average_phase_time() - 10.0).abs() < f64::EPSILON);
}
#[test]
fn generation_metrics_zero_phases() {
    let m = GenerationMetrics {
        total_time_seconds: 10.0, analysis_time: 0.0, planning_time: 0.0,
        synthesis_time: 0.0, assembly_time: 0.0, skill_count: 0, phase_count: 0, bundle_size_kb: 0.0,
    };
    assert!((m.average_phase_time()).abs() < f64::EPSILON);
}
#[test]
fn generation_metrics_serde_roundtrip() {
    let m = GenerationMetrics {
        total_time_seconds: 1.0, analysis_time: 0.1, planning_time: 0.2,
        synthesis_time: 0.3, assembly_time: 0.4, skill_count: 2, phase_count: 1, bundle_size_kb: 5.0,
    };
    let m2: GenerationMetrics = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
    assert_eq!(m.skill_count, m2.skill_count);
    assert!((m.total_time_seconds - m2.total_time_seconds).abs() < f64::EPSILON);
}

// JSON field names match Python snake_case conventions

#[test]
fn json_field_names_match_python_goal() {
    let v: serde_json::Value = serde_json::to_value(&valid_goal()).unwrap();
    let obj = v.as_object().unwrap();
    for key in ["raw_prompt", "goal", "domain", "constraints", "success_criteria", "complexity"] {
        assert!(obj.contains_key(key), "missing key: {key}");
    }
}
#[test]
fn json_field_names_match_python_plan_phase() {
    let v: serde_json::Value = serde_json::to_value(&valid_phase()).unwrap();
    let obj = v.as_object().unwrap();
    for key in ["name", "required_capabilities", "estimated_duration", "parallel_safe", "success_indicators"] {
        assert!(obj.contains_key(key), "missing key: {key}");
    }
}
#[test]
fn json_field_names_match_python_bundle() {
    let v: serde_json::Value = serde_json::to_value(&valid_bundle()).unwrap();
    let obj = v.as_object().unwrap();
    for key in ["id", "name", "version", "skills", "sdk_tools", "sub_agent_configs", "status"] {
        assert!(obj.contains_key(key), "missing key: {key}");
    }
}
#[test]
fn json_field_names_match_python_metrics() {
    let m = GenerationMetrics {
        total_time_seconds: 0.0, analysis_time: 0.0, planning_time: 0.0,
        synthesis_time: 0.0, assembly_time: 0.0, skill_count: 0, phase_count: 0, bundle_size_kb: 0.0,
    };
    let v: serde_json::Value = serde_json::to_value(&m).unwrap();
    let obj = v.as_object().unwrap();
    for key in ["total_time_seconds", "analysis_time", "planning_time", "synthesis_time",
                "assembly_time", "skill_count", "phase_count", "bundle_size_kb"] {
        assert!(obj.contains_key(key), "missing key: {key}");
    }
}
