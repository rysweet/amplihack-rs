use amplihack_delegation::{ScenarioCategory, ScenarioGenerator};

#[test]
fn generates_minimum() {
    let generator = ScenarioGenerator::new("build a todo app");
    let scenarios = generator.generate(1);
    assert!(!scenarios.is_empty());
    assert_eq!(scenarios[0].goal, "build a todo app");
}

#[test]
fn api_more() {
    let generator = ScenarioGenerator::new("build rest api");
    let scenarios = generator.generate(2);
    assert!(scenarios.len() >= 1);
    assert!(
        scenarios
            .iter()
            .any(|s| s.category == ScenarioCategory::API)
    );
}

#[test]
fn auth_security() {
    let generator = ScenarioGenerator::new("jwt authentication");
    let scenarios = generator.generate(1);
    assert!(!scenarios.is_empty());
    assert_eq!(scenarios[0].category, ScenarioCategory::AuthSecurity);
}

#[test]
fn performance() {
    let generator = ScenarioGenerator::new("performance testing");
    let scenarios = generator.generate(1);
    assert!(!scenarios.is_empty());
    assert_eq!(scenarios[0].category, ScenarioCategory::Performance);
}

#[test]
fn pagination_zero() {
    let generator = ScenarioGenerator::new("build list page");
    let scenarios = generator.generate(0);
    assert_eq!(scenarios.len(), 0);
}

#[test]
fn required_fields() {
    let generator = ScenarioGenerator::new("test goal");
    let scenarios = generator.generate(1);
    assert!(!scenarios.is_empty());
    let scenario = &scenarios[0];
    assert!(!scenario.goal.is_empty());
    assert!(!scenario.test_steps.is_empty());
    assert!(!scenario.expected_outcome.is_empty());
    assert!(!scenario.priority.is_empty());
    assert!(!scenario.tags.is_empty());
}

#[test]
fn integration_always() {
    let generator = ScenarioGenerator::new("any goal");
    let scenarios = generator.generate(1);
    assert!(!scenarios.is_empty());
    assert!(scenarios[0].tags.contains(&"integration".into()));
}

#[test]
fn missing_data() {
    let generator = ScenarioGenerator::new("");
    let scenarios = generator.generate(1);
    assert!(!scenarios.is_empty());
}

#[test]
fn serde_roundtrip() {
    let generator = ScenarioGenerator::new("test");
    let scenarios = generator.generate(1);
    if !scenarios.is_empty() {
        let json = serde_json::to_string(&scenarios[0]).unwrap();
        let deserialized = serde_json::from_str(&json).expect("Failed to deserialize scenario");
        assert_eq!(scenarios[0].goal, deserialized.goal);
    }
}
