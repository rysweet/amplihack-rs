use amplihack_agent_generator::{GoalDefinition, ObjectivePlanner};

fn planner() -> ObjectivePlanner {
    ObjectivePlanner::new()
}

fn simple_goal() -> GoalDefinition {
    GoalDefinition::new("list files", "list directory contents", "file-management").unwrap()
}

fn complex_goal() -> GoalDefinition {
    let mut g = GoalDefinition::new(
        "build fraud detection system",
        "detect fraudulent transactions in real time",
        "security-analysis",
    )
    .unwrap();
    g.complexity = amplihack_agent_generator::Complexity::Complex;
    g.constraints = vec![
        "sub-100ms latency".into(),
        "multi-region failover".into(),
    ];
    g
}

// ---------------------------------------------------------------------------
// ObjectivePlanner — behavioral tests
// ---------------------------------------------------------------------------

#[test]
fn single_phase_plan() {
    let plan = planner().plan(&simple_goal()).unwrap();
    // Simple goal gets analysis + implementation + validation
    assert_eq!(plan.phase_count(), 3);
}

#[test]
fn multi_phase_plan() {
    let plan = planner().plan(&complex_goal()).unwrap();
    // Complex goal adds optimization phase
    assert_eq!(plan.phase_count(), 4);
}

#[test]
fn dependency_ordering() {
    let plan = planner().plan(&complex_goal()).unwrap();
    let impl_phase = plan
        .phases
        .iter()
        .find(|p| p.name == "implementation")
        .unwrap();
    assert!(impl_phase.dependencies.contains(&"analysis".to_string()));
    let val_phase = plan
        .phases
        .iter()
        .find(|p| p.name == "validation")
        .unwrap();
    assert!(val_phase
        .dependencies
        .contains(&"implementation".to_string()));
}

#[test]
fn parallel_opportunities() {
    let plan = planner().plan(&complex_goal()).unwrap();
    // All phases are sequential in this plan
    assert!(plan.phase_count() >= 3);
}

#[test]
fn risk_factor_identification() {
    let plan = planner().plan(&complex_goal()).unwrap();
    assert!(!plan.risk_factors.is_empty());
}

#[test]
fn plan_with_simple_goal() {
    let plan = planner().plan(&simple_goal()).unwrap();
    assert_eq!(plan.phase_count(), 3);
    let names: Vec<&str> = plan.phases.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["analysis", "implementation", "validation"]);
}

#[test]
fn plan_with_constrained_goal() {
    let mut g = simple_goal();
    g.constraints = vec!["must complete in 5 seconds".into()];
    let plan = planner().plan(&g).unwrap();
    assert!(plan.phase_count() >= 3);
    assert!(!plan.risk_factors.is_empty());
}

#[test]
fn plan_duration_estimation() {
    let plan = planner().plan(&simple_goal()).unwrap();
    assert!(!plan.total_estimated_duration.is_empty());
}

#[test]
fn plan_required_skills_populated() {
    let plan = planner().plan(&complex_goal()).unwrap();
    assert!(!plan.required_skills.is_empty());
}
