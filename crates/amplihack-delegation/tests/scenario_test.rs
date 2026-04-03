use amplihack_delegation::{ScenarioCategory, ScenarioGenerator};

#[test]
fn generates_happy_path_and_error_handling() {
    let generator = ScenarioGenerator::new();
    let scenarios = generator.generate_scenarios("build a todo app", "users can add/remove items");
    assert!(!scenarios.is_empty());
    assert!(scenarios.iter().any(|s| s.category == ScenarioCategory::HappyPath));
    assert!(scenarios.iter().any(|s| s.category == ScenarioCategory::ErrorHandling));
}

#[test]
fn api_goal_produces_api_specific_scenarios() {
    let generator = ScenarioGenerator::new();
    let scenarios = generator.generate_scenarios("build rest api", "CRUD endpoints work");
    assert!(scenarios.len() >= 3);
    // API goals should produce boundary conditions and integration at minimum
    assert!(scenarios.iter().any(|s| s.category == ScenarioCategory::BoundaryConditions));
    assert!(scenarios.iter().any(|s| s.category == ScenarioCategory::Integration));
}

#[test]
fn auth_goal_produces_security_scenarios() {
    let generator = ScenarioGenerator::new();
    let scenarios = generator.generate_scenarios("jwt authentication", "users can log in securely");
    assert!(!scenarios.is_empty());
    assert!(scenarios.iter().any(|s| s.category == ScenarioCategory::Security));
}

#[test]
fn performance_goal_produces_perf_scenarios() {
    let generator = ScenarioGenerator::new();
    let scenarios = generator.generate_scenarios("performance testing", "sub-second response times");
    assert!(!scenarios.is_empty());
    assert!(scenarios.iter().any(|s| s.category == ScenarioCategory::Performance));
}

#[test]
fn always_produces_integration() {
    let generator = ScenarioGenerator::new();
    let scenarios = generator.generate_scenarios("any goal", "some criteria");
    assert!(!scenarios.is_empty());
    assert!(scenarios.iter().any(|s| s.category == ScenarioCategory::Integration));
}

#[test]
fn empty_goal_still_generates() {
    let generator = ScenarioGenerator::new();
    let scenarios = generator.generate_scenarios("", "");
    assert!(!scenarios.is_empty());
}

#[test]
fn scenarios_have_required_fields() {
    let generator = ScenarioGenerator::new();
    let scenarios = generator.generate_scenarios("test goal", "test criteria");
    for s in &scenarios {
        assert!(!s.name.is_empty());
        assert!(!s.description.is_empty());
        assert!(!s.steps.is_empty());
        assert!(!s.expected_outcome.is_empty());
        assert!(!s.priority.is_empty());
    }
}

#[test]
fn serde_roundtrip() {
    let generator = ScenarioGenerator::new();
    let scenarios = generator.generate_scenarios("test serde", "round trip works");
    if let Some(s) = scenarios.first() {
        let json = serde_json::to_string(s).unwrap();
        let deser: amplihack_delegation::TestScenario =
            serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(s.name, deser.name);
        assert_eq!(s.category, deser.category);
    }
}
