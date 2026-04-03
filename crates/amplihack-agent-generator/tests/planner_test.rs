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
// ObjectivePlanner — all tests hit todo!() and should panic
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn single_phase_plan() {
    let _ = planner().plan(&simple_goal());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn multi_phase_plan() {
    let _ = planner().plan(&complex_goal());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn dependency_ordering() {
    let _ = planner().plan(&complex_goal());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn parallel_opportunities() {
    let _ = planner().plan(&complex_goal());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn risk_factor_identification() {
    let _ = planner().plan(&complex_goal());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn plan_with_simple_goal() {
    let _ = planner().plan(&simple_goal());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn plan_with_constrained_goal() {
    let mut g = simple_goal();
    g.constraints = vec!["must complete in 5 seconds".into()];
    let _ = planner().plan(&g);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn plan_duration_estimation() {
    let _ = planner().plan(&simple_goal());
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn plan_required_skills_populated() {
    let _ = planner().plan(&complex_goal());
}
