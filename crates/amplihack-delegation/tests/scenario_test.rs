use amplihack_delegation::{ScenarioCategory, ScenarioGenerator, TestScenario};

fn make_gen() -> ScenarioGenerator {
    ScenarioGenerator::new()
}

#[test]
fn generates_minimum_scenarios_for_generic_goal() {
    let s = make_gen().generate_scenarios("build a CLI tool", "it works");
    assert!(s.len() >= 6, "expected >= 6, got {}", s.len());
}

#[test]
fn api_goal_produces_more_scenarios() {
    let generic = make_gen().generate_scenarios("build a tool", "works");
    let api = make_gen().generate_scenarios("build a REST API endpoint", "returns JSON");
    assert!(api.len() > generic.len());
}

#[test]
fn auth_goal_includes_security_scenarios() {
    let s = make_gen().generate_scenarios("implement JWT authentication", "login returns token");
    let n = s
        .iter()
        .filter(|x| x.category == ScenarioCategory::Security)
        .count();
    assert!(n >= 3, "expected >= 3 security, got {n}");
}

#[test]
fn performance_goal_includes_performance_scenarios() {
    let s = make_gen().generate_scenarios("optimize performance of search", "handles load");
    let n = s
        .iter()
        .filter(|x| x.category == ScenarioCategory::Performance)
        .count();
    assert!(n >= 1);
}

#[test]
fn pagination_goal_includes_zero_results() {
    let s = make_gen().generate_scenarios("implement paginated list endpoint", "returns page");
    assert!(s.iter().any(|x| x.name == "Pagination with zero results"));
}

#[test]
fn all_scenarios_have_required_fields() {
    let s = make_gen().generate_scenarios("build auth API endpoint", "secure JWT login");
    for x in &s {
        assert!(!x.name.is_empty());
        assert!(!x.steps.is_empty());
        assert!(["high", "medium", "low"].contains(&x.priority.as_str()));
    }
}

#[test]
fn integration_always_present() {
    let s = make_gen().generate_scenarios("anything", "something");
    assert!(
        s.iter()
            .any(|x| x.category == ScenarioCategory::Integration)
    );
}

#[test]
fn error_handling_always_has_missing_data() {
    let s = make_gen().generate_scenarios("do stuff", "it works");
    assert!(
        s.iter()
            .any(|x| x.name == "Missing required data is rejected")
    );
}

#[test]
fn scenario_serde_roundtrip() {
    let scenario = TestScenario {
        name: "test".into(),
        category: ScenarioCategory::HappyPath,
        description: "desc".into(),
        preconditions: vec!["pre".into()],
        steps: vec!["step".into()],
        expected_outcome: "pass".into(),
        priority: "high".into(),
        tags: vec!["tag".into()],
    };
    let json = serde_json::to_string(&scenario).unwrap();
    let back: TestScenario = serde_json::from_str(&json).unwrap();
    assert_eq!(back.name, "test");
    assert_eq!(back.category, ScenarioCategory::HappyPath);
}
