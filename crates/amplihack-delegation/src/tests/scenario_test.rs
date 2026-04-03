use super::*;

    fn make_generator() -> ScenarioGenerator {
        ScenarioGenerator::new()
    }

    #[test]
    fn generates_minimum_scenarios_for_generic_goal() {
        let scenarios = make_generator().generate_scenarios("build a CLI tool", "it works");
        // happy_path(1) + error_handling(1 + 1 missing) + boundary(2) + integration(1) = 6
        assert!(
            scenarios.len() >= 6,
            "expected ≥6 scenarios, got {}",
            scenarios.len()
        );
    }

    #[test]
    fn api_goal_produces_more_scenarios() {
        let generic = make_generator().generate_scenarios("build a tool", "works");
        let api = make_generator().generate_scenarios("build a REST API endpoint", "returns JSON");
        assert!(
            api.len() > generic.len(),
            "api ({}) should produce more scenarios than generic ({})",
            api.len(),
            generic.len()
        );
    }

    #[test]
    fn auth_goal_includes_security_scenarios() {
        let scenarios = make_generator()
            .generate_scenarios("implement JWT authentication", "login returns token");
        let security_count = scenarios
            .iter()
            .filter(|s| s.category == ScenarioCategory::Security)
            .count();
        assert!(
            security_count >= 3,
            "expected ≥3 security scenarios, got {security_count}"
        );
    }

    #[test]
    fn performance_goal_includes_performance_scenarios() {
        let scenarios = make_generator()
            .generate_scenarios("optimize performance of search", "handles load under 200ms");
        let perf_count = scenarios
            .iter()
            .filter(|s| s.category == ScenarioCategory::Performance)
            .count();
        assert!(perf_count >= 1, "expected ≥1 performance scenario");
    }

    #[test]
    fn pagination_goal_includes_zero_results_scenario() {
        let scenarios = make_generator()
            .generate_scenarios("implement paginated list endpoint", "returns correct page");
        let names: Vec<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"Pagination with zero results"),
            "expected pagination boundary scenario, got: {names:?}"
        );
    }

    #[test]
    fn all_scenarios_have_required_fields() {
        let scenarios = make_generator()
            .generate_scenarios("build auth API endpoint", "secure login with JWT tokens");
        for s in &scenarios {
            assert!(!s.name.is_empty(), "name must not be empty");
            assert!(!s.description.is_empty(), "description must not be empty");
            assert!(!s.steps.is_empty(), "steps must not be empty");
            assert!(
                !s.expected_outcome.is_empty(),
                "expected_outcome must not be empty"
            );
            assert!(
                ["high", "medium", "low"].contains(&s.priority.as_str()),
                "unexpected priority: {}",
                s.priority
            );
        }
    }

    #[test]
    fn integration_always_present() {
        let scenarios = make_generator().generate_scenarios("anything", "something");
        let integration = scenarios
            .iter()
            .filter(|s| s.category == ScenarioCategory::Integration)
            .count();
        assert!(integration >= 1);
    }

    #[test]
    fn error_handling_always_has_missing_data() {
        let scenarios = make_generator().generate_scenarios("do stuff", "it works");
        let missing = scenarios
            .iter()
            .any(|s| s.name == "Missing required data is rejected");
        assert!(missing, "should always include missing-data scenario");
    }

    #[test]
    fn domain_detection_api() {
        assert!(ScenarioGenerator::is_api("build rest api"));
        assert!(ScenarioGenerator::is_api("create http endpoint"));
        assert!(!ScenarioGenerator::is_api("build a cli tool"));
    }

    #[test]
    fn domain_detection_auth() {
        assert!(ScenarioGenerator::is_auth("jwt authentication"));
        assert!(ScenarioGenerator::is_auth("login page"));
        assert!(!ScenarioGenerator::is_auth("build a chart"));
    }

    #[test]
    fn domain_detection_perf() {
        assert!(ScenarioGenerator::is_perf("performance testing"));
        assert!(ScenarioGenerator::is_perf("load balancer"));
        assert!(!ScenarioGenerator::is_perf("build a form"));
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
