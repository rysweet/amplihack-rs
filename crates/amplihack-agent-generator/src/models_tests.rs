use super::*;
use serde_json;

// ---------------------------------------------------------------------------
// Complexity
// ---------------------------------------------------------------------------

#[test]
fn complexity_ordering() {
    assert!(Complexity::Simple < Complexity::Moderate);
    assert!(Complexity::Moderate < Complexity::Complex);
    assert!(Complexity::Simple < Complexity::Complex);
}

#[test]
fn complexity_serde_roundtrip() {
    for variant in [
        Complexity::Simple,
        Complexity::Moderate,
        Complexity::Complex,
    ] {
        let json = serde_json::to_string(&variant).unwrap();
        let back: Complexity = serde_json::from_str(&json).unwrap();
        assert_eq!(variant, back);
    }
}

#[test]
fn complexity_serde_snake_case() {
    assert_eq!(
        serde_json::to_string(&Complexity::Simple).unwrap(),
        "\"simple\""
    );
    assert_eq!(
        serde_json::to_string(&Complexity::Moderate).unwrap(),
        "\"moderate\""
    );
    assert_eq!(
        serde_json::to_string(&Complexity::Complex).unwrap(),
        "\"complex\""
    );
}

// ---------------------------------------------------------------------------
// BundleStatus
// ---------------------------------------------------------------------------

#[test]
fn bundle_status_default_is_pending() {
    assert_eq!(BundleStatus::default(), BundleStatus::Pending);
}

#[test]
fn bundle_status_serde_roundtrip() {
    let variants = [
        BundleStatus::Pending,
        BundleStatus::Planning,
        BundleStatus::Assembling,
        BundleStatus::Ready,
        BundleStatus::Failed,
    ];
    for v in variants {
        let json = serde_json::to_string(&v).unwrap();
        let back: BundleStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }
}

// ---------------------------------------------------------------------------
// GoalDefinition
// ---------------------------------------------------------------------------

#[test]
fn goal_definition_new_valid() {
    let g = GoalDefinition::new("my prompt", "my goal", "security").unwrap();
    assert_eq!(g.raw_prompt, "my prompt");
    assert_eq!(g.goal, "my goal");
    assert_eq!(g.domain, "security");
    assert!(g.constraints.is_empty());
    assert!(g.success_criteria.is_empty());
    assert!(g.context.is_empty());
    assert_eq!(g.complexity, Complexity::Simple);
}

#[test]
fn goal_definition_rejects_empty_prompt() {
    let err = GoalDefinition::new("  ", "goal", "domain").unwrap_err();
    assert!(err.to_string().contains("raw_prompt"));
}

#[test]
fn goal_definition_rejects_empty_goal() {
    let err = GoalDefinition::new("prompt", "", "domain").unwrap_err();
    assert!(err.to_string().contains("goal"));
}

#[test]
fn goal_definition_rejects_empty_domain() {
    let err = GoalDefinition::new("prompt", "goal", "  ").unwrap_err();
    assert!(err.to_string().contains("domain"));
}

#[test]
fn goal_definition_serde_roundtrip() {
    let g = GoalDefinition::new("prompt", "goal", "dev").unwrap();
    let json = serde_json::to_string(&g).unwrap();
    let back: GoalDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(back.raw_prompt, g.raw_prompt);
    assert_eq!(back.goal, g.goal);
    assert_eq!(back.domain, g.domain);
}

// ---------------------------------------------------------------------------
// PlanPhase
// ---------------------------------------------------------------------------

#[test]
fn plan_phase_new_valid() {
    let p = PlanPhase::new("analysis", "do analysis", vec!["cap1".into()]).unwrap();
    assert_eq!(p.name, "analysis");
    assert!(p.parallel_safe);
    assert!(p.dependencies.is_empty());
}

#[test]
fn plan_phase_rejects_empty_name() {
    let err = PlanPhase::new("", "desc", vec!["cap".into()]).unwrap_err();
    assert!(err.to_string().contains("phase name"));
}

#[test]
fn plan_phase_rejects_empty_capabilities() {
    let err = PlanPhase::new("name", "desc", vec![]).unwrap_err();
    assert!(err.to_string().contains("required_capabilities"));
}

// ---------------------------------------------------------------------------
// ExecutionPlan
// ---------------------------------------------------------------------------

#[test]
fn execution_plan_new_valid() {
    let phase = PlanPhase::new("p1", "desc", vec!["cap".into()]).unwrap();
    let plan = ExecutionPlan::new(uuid::Uuid::new_v4(), vec![phase]).unwrap();
    assert_eq!(plan.phase_count(), 1);
}

#[test]
fn execution_plan_rejects_empty_phases() {
    let err = ExecutionPlan::new(uuid::Uuid::new_v4(), vec![]).unwrap_err();
    assert!(err.to_string().contains("at least 1 phase"));
}

#[test]
fn execution_plan_rejects_too_many_phases() {
    let phases: Vec<PlanPhase> = (0..11)
        .map(|i| PlanPhase::new(format!("p{i}"), "d", vec!["c".into()]).unwrap())
        .collect();
    let err = ExecutionPlan::new(uuid::Uuid::new_v4(), phases).unwrap_err();
    assert!(err.to_string().contains("at most 10"));
}

// ---------------------------------------------------------------------------
// SkillDefinition
// ---------------------------------------------------------------------------

#[test]
fn skill_definition_new_valid() {
    let s = SkillDefinition::new("analysis", PathBuf::from("skills/a.yaml"), "# content").unwrap();
    assert_eq!(s.name, "analysis");
    assert_eq!(s.match_score, 0.0);
}

#[test]
fn skill_definition_rejects_empty_name() {
    let err = SkillDefinition::new("", PathBuf::from("p"), "content").unwrap_err();
    assert!(err.to_string().contains("skill name"));
}

#[test]
fn skill_definition_rejects_empty_content() {
    let err = SkillDefinition::new("name", PathBuf::from("p"), "  ").unwrap_err();
    assert!(err.to_string().contains("skill content"));
}

#[test]
fn skill_definition_with_match_score() {
    let s = SkillDefinition::new("s", PathBuf::from("p"), "c")
        .unwrap()
        .with_match_score(0.85)
        .unwrap();
    assert!((s.match_score - 0.85).abs() < f64::EPSILON);
}

#[test]
fn skill_definition_match_score_out_of_range() {
    let s = SkillDefinition::new("s", PathBuf::from("p"), "c").unwrap();
    assert!(s.with_match_score(1.5).is_err());
    let s2 = SkillDefinition::new("s", PathBuf::from("p"), "c").unwrap();
    assert!(s2.with_match_score(-0.1).is_err());
}

// ---------------------------------------------------------------------------
// SDKToolConfig
// ---------------------------------------------------------------------------

#[test]
fn sdk_tool_config_to_dict() {
    let cfg = SDKToolConfig {
        name: "tool1".into(),
        description: "desc".into(),
        category: "cat".into(),
    };
    let map = cfg.to_dict();
    assert_eq!(map.get("name").unwrap(), "tool1");
    assert_eq!(map.get("description").unwrap(), "desc");
    assert_eq!(map.get("category").unwrap(), "cat");
    assert_eq!(map.len(), 3);
}

// ---------------------------------------------------------------------------
// SubAgentConfig
// ---------------------------------------------------------------------------

#[test]
fn sub_agent_config_new() {
    let c = SubAgentConfig::new("reviewer");
    assert_eq!(c.role, "reviewer");
    assert_eq!(c.filename, "reviewer.yaml");
    assert!(c.config.is_empty());
}

// ---------------------------------------------------------------------------
// GoalAgentBundle
// ---------------------------------------------------------------------------

#[test]
fn goal_agent_bundle_new_valid() {
    let b = GoalAgentBundle::new("my-agent", "1.0.0").unwrap();
    assert_eq!(b.name, "my-agent");
    assert_eq!(b.version, "1.0.0");
    assert_eq!(b.status, BundleStatus::Pending);
    assert!(!b.is_complete());
}

#[test]
fn goal_agent_bundle_rejects_short_name() {
    let err = GoalAgentBundle::new("ab", "1.0").unwrap_err();
    assert!(err.to_string().contains("at least 3"));
}

#[test]
fn goal_agent_bundle_rejects_long_name() {
    let long = "a".repeat(51);
    let err = GoalAgentBundle::new(long, "1.0").unwrap_err();
    assert!(err.to_string().contains("at most 50"));
}

#[test]
fn goal_agent_bundle_is_complete() {
    let mut b = GoalAgentBundle::new("test-bundle", "0.1.0").unwrap();
    assert!(
        !b.is_complete(),
        "incomplete without goal/plan/skills/status"
    );

    b.goal_definition = Some(GoalDefinition::new("p", "g", "d").unwrap());
    assert!(!b.is_complete());

    let phase = PlanPhase::new("p1", "d", vec!["c".into()]).unwrap();
    b.execution_plan = Some(ExecutionPlan::new(uuid::Uuid::new_v4(), vec![phase]).unwrap());
    assert!(!b.is_complete());

    b.skills
        .push(SkillDefinition::new("s", PathBuf::from("p"), "c").unwrap());
    assert!(!b.is_complete(), "still Pending status");

    b.status = BundleStatus::Ready;
    assert!(b.is_complete());
}

// ---------------------------------------------------------------------------
// GenerationMetrics
// ---------------------------------------------------------------------------

#[test]
fn generation_metrics_average_phase_time() {
    let m = GenerationMetrics {
        total_time_seconds: 30.0,
        analysis_time: 5.0,
        planning_time: 5.0,
        synthesis_time: 10.0,
        assembly_time: 10.0,
        skill_count: 3,
        phase_count: 3,
        bundle_size_kb: 1.5,
    };
    assert!((m.average_phase_time() - 10.0).abs() < f64::EPSILON);
}

#[test]
fn generation_metrics_average_phase_time_zero_phases() {
    let m = GenerationMetrics {
        total_time_seconds: 10.0,
        analysis_time: 0.0,
        planning_time: 0.0,
        synthesis_time: 0.0,
        assembly_time: 0.0,
        skill_count: 0,
        phase_count: 0,
        bundle_size_kb: 0.0,
    };
    assert!((m.average_phase_time() - 0.0).abs() < f64::EPSILON);
}
